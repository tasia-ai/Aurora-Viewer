use aurora_extension_api::{API_VERSION, ExtensionAction, ExtensionEvent, ExtensionRequest, PanelControl, PanelEventValue, Permission};
use aurora_extension_runtime::{
    ExtensionReview, ExtensionState, ExtensionSummary, MAX_PACKAGE_BYTES, PersistentExtensionManager, StagedExtension,
};
use aurora_i18n::tr;
use crossbeam_channel::{Sender, bounded};
use semver::Version;
use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
    thread,
};

const ACTION_BATCH_LIMIT: usize = 128;
const REGISTERED_PANEL_LIMIT: usize = 256;

/// Safe, UI-ready metadata for a package awaiting installation consent.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher_claim: String,
    pub content_sha256: String,
    pub requested_permissions: Vec<String>,
    pub is_update: bool,
}

/// A host-rendered, non-executable panel registered by an enabled extension.
#[derive(Clone, Debug)]
pub struct PanelView {
    pub extension_id: String,
    pub panel_id: String,
    pub title: String,
    pub controls: Vec<PanelControl>,
}

/// Validated, bounded actions returned by one guest event.
#[derive(Clone, Debug)]
pub struct ActionBatch {
    pub extension_id: String,
    pub message_id: Option<u64>,
    pub actions: Vec<ExtensionAction>,
}

/// A snapshot of extension state for the Preferences manager page.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub ready: bool,
    pub error: Option<String>,
    pub status: Option<String>,
    pub safe_mode: bool,
    pub installed: Vec<ExtensionSummary>,
    pub pending: Option<Review>,
    pub panels: Vec<PanelView>,
}

/// User-initiated lifecycle commands issued by the extension manager UI.
pub enum Command {
    ChooseInstall(PathBuf),
    ChooseUpdate {
        id: String,
        path: PathBuf,
    },
    CommitInstall {
        permissions: Vec<String>,
        enable: bool,
    },
    CancelInstall,
    Enable {
        id: String,
        permissions: Vec<String>,
    },
    Disable {
        id: String,
    },
    Revoke {
        id: String,
        permission: String,
    },
    Uninstall {
        id: String,
    },
    ClearQuarantine {
        id: String,
    },
    SafeMode(bool),
    NearbyChat(ExtensionEvent),
    PanelAction {
        id: String,
        panel_id: String,
        control_id: String,
        value: PanelEventValue,
    },
}

/// Non-blocking front end to the extension supervisor thread.
pub struct Service {
    tx: Sender<Command>,
    snapshot: Arc<RwLock<Snapshot>>,
    actions: Arc<RwLock<VecDeque<ActionBatch>>>,
}

struct PendingInstall {
    staged: StagedExtension,
    bytes: Vec<u8>,
    update_id: Option<String>,
    review: Review,
}

impl Service {
    /// Start the isolated manager worker using a per-user data directory.
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = bounded(64);
        let snapshot = Arc::new(RwLock::new(Snapshot::default()));
        let worker_snapshot = Arc::clone(&snapshot);
        let actions = Arc::new(RwLock::new(VecDeque::new()));
        let worker_actions = Arc::clone(&actions);
        if let Err(error) = thread::Builder::new()
            .name("aurora-extension-manager".into())
            .spawn(move || run_worker(root, rx, worker_snapshot, worker_actions))
            && let Ok(mut state) = snapshot.write()
        {
            state.error = Some(tr!("extensions-manager-error-start", error = error.to_string()));
        }
        Self { tx, snapshot, actions }
    }

    /// Queue one user command without blocking the UI thread.
    pub fn submit(&self, command: Command) -> bool {
        self.tx.try_send(command).is_ok()
    }

    /// Read the latest safe metadata snapshot without waiting on guest execution.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.read().map(|state| state.clone()).unwrap_or_default()
    }

    /// Drain validated guest actions for host-side application on the UI thread.
    pub fn take_actions(&self) -> Vec<ActionBatch> {
        self.actions
            .write()
            .map(|mut actions| actions.drain(..).collect())
            .unwrap_or_default()
    }
}

fn run_worker(
    root: PathBuf,
    rx: crossbeam_channel::Receiver<Command>,
    shared: Arc<RwLock<Snapshot>>,
    shared_actions: Arc<RwLock<VecDeque<ActionBatch>>>,
) {
    let opened = PersistentExtensionManager::open(root, Version::new(1, 0, 0));
    let mut manager = match opened {
        Ok(manager) => Some(manager),
        Err(error) => {
            publish_error(&shared, tr!("extensions-manager-error-load", error = error.to_string()));
            None
        }
    };
    let mut pending: Option<PendingInstall> = None;
    let mut panels = Vec::new();
    let mut next_request_id = 1u64;
    if let Some(manager) = manager.as_mut() {
        let enabled: Vec<_> = manager
            .list()
            .into_iter()
            .filter(|extension| extension.state == ExtensionState::Enabled)
            .map(|extension| extension.manifest.id)
            .collect();
        for id in enabled {
            initialize_extension(manager, &id, &mut next_request_id, &shared_actions, &mut panels);
        }
    }
    publish_state(&shared, manager.as_ref(), &panels, None);

    for command in rx {
        let Some(manager) = manager.as_mut() else {
            publish_error(&shared, tr!("extensions-manager-storage-unavailable").to_owned());
            continue;
        };
        let command = match command {
            Command::NearbyChat(event) => {
                if matches!(&event, ExtensionEvent::NearbyChat { .. }) && !manager.safe_mode() {
                    let eligible: Vec<_> = manager
                        .list()
                        .into_iter()
                        .filter(|extension| {
                            extension.state == ExtensionState::Enabled
                                && extension.granted_permissions.contains(&Permission::ChatNearbyRead)
                        })
                        .map(|extension| extension.manifest.id)
                        .collect();
                    for id in eligible {
                        let request = make_request(&mut next_request_id, event.clone());
                        dispatch_extension(manager, &id, request, &shared_actions, &mut panels);
                    }
                }
                publish_state(&shared, Some(manager), &panels, None);
                continue;
            }
            Command::PanelAction {
                id,
                panel_id,
                control_id,
                value,
            } => {
                if !manager.safe_mode() && panel_has_control(&panels, &id, &panel_id, &control_id) {
                    update_toggle_value(&mut panels, &id, &panel_id, &control_id, &value);
                    let request = make_request(
                        &mut next_request_id,
                        ExtensionEvent::PanelAction {
                            panel_id,
                            control_id,
                            value,
                        },
                    );
                    dispatch_extension(manager, &id, request, &shared_actions, &mut panels);
                }
                publish_state(&shared, Some(manager), &panels, None);
                continue;
            }
            other => other,
        };

        let result = match command {
            Command::ChooseInstall(path) => {
                if pending.is_some() {
                    Err(tr!("extensions-manager-finish-review").to_owned())
                } else {
                    stage_file(manager, &path, None).map(|candidate| {
                        update_pending(&shared, Some(candidate.review.clone()));
                        pending = Some(candidate);
                        tr!("extensions-manager-package-validated").to_owned()
                    })
                }
            }
            Command::ChooseUpdate { id, path } => {
                if pending.is_some() {
                    Err(tr!("extensions-manager-finish-review").to_owned())
                } else {
                    stage_file(manager, &path, Some(id)).map(|candidate| {
                        update_pending(&shared, Some(candidate.review.clone()));
                        pending = Some(candidate);
                        tr!("extensions-manager-update-validated").to_owned()
                    })
                }
            }
            Command::CommitInstall { permissions, enable } => {
                let Some(candidate) = pending.take() else {
                    publish_error(&shared, tr!("extensions-manager-no-pending").to_owned());
                    continue;
                };
                let PendingInstall {
                    staged,
                    bytes,
                    update_id,
                    review,
                } = candidate;
                match parse_permissions(&permissions) {
                    Ok(approved) => {
                        let result = if let Some(id) = update_id.as_deref() {
                            manager.update(id, staged, &bytes, &approved, enable)
                        } else {
                            manager.install(staged, &bytes, &approved, enable)
                        };
                        match result {
                            Ok(summary) => {
                                if enable {
                                    initialize_extension(manager, &summary.manifest.id, &mut next_request_id, &shared_actions, &mut panels);
                                } else {
                                    panels.retain(|panel| panel.extension_id != summary.manifest.id);
                                }
                                update_pending(&shared, None);
                                if review.is_update {
                                    Ok(tr!(
                                        "extensions-manager-updated-status",
                                        name = summary.manifest.name.as_str(),
                                        version = summary.manifest.version.as_str()
                                    ))
                                } else {
                                    Ok(tr!(
                                        "extensions-manager-installed-status",
                                        name = summary.manifest.name.as_str(),
                                        version = summary.manifest.version.as_str()
                                    ))
                                }
                            }
                            Err(error) => {
                                let retry = if let Some(id) = update_id.as_deref() {
                                    manager.stage_update(id, &bytes)
                                } else {
                                    manager.stage_install(&bytes)
                                };
                                pending = retry.ok().map(|staged| PendingInstall {
                                    staged,
                                    bytes,
                                    update_id,
                                    review,
                                });
                                Err(operation_error(error))
                            }
                        }
                    }
                    Err(error) => {
                        pending = Some(PendingInstall {
                            staged,
                            bytes,
                            update_id,
                            review,
                        });
                        Err(error)
                    }
                }
            }
            Command::CancelInstall => {
                pending = None;
                update_pending(&shared, None);
                Ok(tr!("extensions-manager-install-cancelled").to_owned())
            }
            Command::Enable { id, permissions } => match parse_permissions(&permissions) {
                Ok(grants) => match manager.enable(&id, &grants) {
                    Ok(()) => {
                        initialize_extension(manager, &id, &mut next_request_id, &shared_actions, &mut panels);
                        Ok(tr!("extensions-manager-enabled-status", id = id.as_str()))
                    }
                    Err(error) => Err(operation_error(error)),
                },
                Err(error) => Err(error),
            },
            Command::Disable { id } => manager
                .disable(&id)
                .map(|()| {
                    panels.retain(|panel| panel.extension_id != id);
                    tr!("extensions-manager-disabled-status", id = id.as_str())
                })
                .map_err(operation_error),
            Command::Revoke { id, permission } => Permission::from_manifest_key(&permission)
                .ok_or_else(|| tr!("extensions-manager-unknown-permission", permission = permission.as_str()))
                .and_then(|permission| {
                    manager.revoke_permission(&id, permission).map_err(operation_error)?;
                    panels.retain(|panel| panel.extension_id != id);
                    Ok(tr!(
                        "extensions-manager-revoked-status",
                        permission = permission.manifest_key(),
                        id = id.as_str()
                    ))
                }),
            Command::Uninstall { id } => manager
                .uninstall(&id)
                .map(|()| {
                    panels.retain(|panel| panel.extension_id != id);
                    tr!("extensions-manager-uninstalled-status", id = id.as_str())
                })
                .map_err(operation_error),
            Command::ClearQuarantine { id } => manager
                .clear_quarantine(&id)
                .map(|()| {
                    panels.retain(|panel| panel.extension_id != id);
                    tr!("extensions-manager-quarantine-cleared-status", id = id.as_str())
                })
                .map_err(operation_error),
            Command::SafeMode(enabled) => manager
                .set_safe_mode(enabled)
                .map(|()| {
                    if enabled {
                        panels.clear();
                    } else {
                        let enabled_ids: Vec<_> = manager
                            .list()
                            .into_iter()
                            .filter(|extension| extension.state == ExtensionState::Enabled)
                            .map(|extension| extension.manifest.id)
                            .collect();
                        for id in enabled_ids {
                            initialize_extension(manager, &id, &mut next_request_id, &shared_actions, &mut panels);
                        }
                    }
                    if enabled {
                        tr!("extensions-manager-safe-mode-on").to_owned()
                    } else {
                        tr!("extensions-manager-safe-mode-off").to_owned()
                    }
                })
                .map_err(operation_error),
            Command::NearbyChat(_) | Command::PanelAction { .. } => unreachable!("worker event handled above"),
        };
        match result {
            Ok(status) => publish_state(&shared, Some(manager), &panels, Some(status)),
            Err(error) => publish_error(&shared, error),
        }
    }
    if let Some(manager) = manager.as_mut()
        && !manager.safe_mode()
    {
        let enabled: Vec<_> = manager
            .list()
            .into_iter()
            .filter(|extension| extension.state == ExtensionState::Enabled)
            .map(|extension| extension.manifest.id)
            .collect();
        for id in enabled {
            let request = make_request(&mut next_request_id, ExtensionEvent::Shutdown);
            let _ = manager.dispatch(&id, &request);
        }
    }
}

fn make_request(next_request_id: &mut u64, event: ExtensionEvent) -> ExtensionRequest {
    let request_id = *next_request_id;
    *next_request_id = next_request_id.wrapping_add(1).max(1);
    ExtensionRequest {
        api_version: API_VERSION,
        request_id,
        event,
    }
}

fn initialize_extension(
    manager: &mut PersistentExtensionManager,
    id: &str,
    next_request_id: &mut u64,
    actions: &RwLock<VecDeque<ActionBatch>>,
    panels: &mut Vec<PanelView>,
) {
    panels.retain(|panel| panel.extension_id != id);
    let request = make_request(
        next_request_id,
        ExtensionEvent::Initialize {
            host_api_version: "1.0.0".into(),
        },
    );
    dispatch_extension(manager, id, request, actions, panels);
}

fn dispatch_extension(
    manager: &mut PersistentExtensionManager,
    id: &str,
    request: ExtensionRequest,
    action_queue: &RwLock<VecDeque<ActionBatch>>,
    panels: &mut Vec<PanelView>,
) {
    let message_id = match &request.event {
        ExtensionEvent::NearbyChat { message_id, .. } => Some(*message_id),
        _ => None,
    };
    match manager.dispatch(id, &request) {
        Ok(actions) => {
            for action in &actions {
                if let ExtensionAction::RegisterPanel { panel_id, title, controls } = action {
                    if let Some(panel) = panels
                        .iter_mut()
                        .find(|panel| panel.extension_id == id && panel.panel_id == *panel_id)
                    {
                        panel.title.clone_from(title);
                        panel.controls.clone_from(controls);
                    } else if panels.len() < REGISTERED_PANEL_LIMIT {
                        panels.push(PanelView {
                            extension_id: id.to_owned(),
                            panel_id: panel_id.clone(),
                            title: title.clone(),
                            controls: controls.clone(),
                        });
                    }
                }
            }
            if !actions.is_empty()
                && let Ok(mut queue) = action_queue.write()
            {
                if queue.len() >= ACTION_BATCH_LIMIT {
                    queue.pop_front();
                }
                queue.push_back(ActionBatch {
                    extension_id: id.to_owned(),
                    message_id,
                    actions,
                });
            }
        }
        Err(_) => {
            if manager
                .list()
                .iter()
                .any(|summary| summary.manifest.id == id && summary.state == ExtensionState::Quarantined)
            {
                panels.retain(|panel| panel.extension_id != id);
            }
        }
    }
}

fn panel_has_control(panels: &[PanelView], extension: &str, panel_id: &str, control_id: &str) -> bool {
    panels.iter().any(|panel| {
        panel.extension_id == extension
            && panel.panel_id == panel_id
            && panel.controls.iter().any(|control| match control {
                PanelControl::Label { id, .. } | PanelControl::Toggle { id, .. } | PanelControl::Button { id, .. } => id == control_id,
            })
    })
}

fn update_toggle_value(panels: &mut [PanelView], extension: &str, panel_id: &str, control_id: &str, value: &PanelEventValue) {
    let PanelEventValue::Toggled { value } = value else {
        return;
    };
    if let Some(PanelControl::Toggle { value: current, .. }) = panels
        .iter_mut()
        .find(|panel| panel.extension_id == extension && panel.panel_id == panel_id)
        .and_then(|panel| {
            panel
                .controls
                .iter_mut()
                .find(|control| matches!(control, PanelControl::Toggle { id, .. } if id == control_id))
        })
    {
        *current = *value;
    }
}

fn stage_file(manager: &PersistentExtensionManager, path: &PathBuf, update_id: Option<String>) -> Result<PendingInstall, String> {
    let metadata = fs::metadata(path).map_err(|error| tr!("extensions-manager-package-read-error", error = error.to_string()))?;
    if metadata.len() > MAX_PACKAGE_BYTES as u64 {
        return Err(tr!("extensions-manager-package-too-large").to_owned());
    }
    let bytes = fs::read(path).map_err(|error| tr!("extensions-manager-package-read-error", error = error.to_string()))?;
    if bytes.len() > MAX_PACKAGE_BYTES {
        return Err(tr!("extensions-manager-package-too-large").to_owned());
    }
    let staged = if let Some(id) = update_id.as_deref() {
        manager.stage_update(id, &bytes)
    } else {
        manager.stage_install(&bytes)
    }
    .map_err(|error| tr!("extensions-manager-package-invalid", error = error.to_string()))?;
    let review = review_view(staged.review(), update_id.is_some());
    Ok(PendingInstall {
        staged,
        bytes,
        update_id,
        review,
    })
}

fn review_view(review: &ExtensionReview, is_update: bool) -> Review {
    Review {
        id: review.manifest.id.clone(),
        name: review.manifest.name.clone(),
        version: review.manifest.version.clone(),
        publisher_claim: review.manifest.publisher.clone(),
        content_sha256: review.content_sha256.iter().map(|byte| format!("{byte:02x}")).collect(),
        requested_permissions: review
            .requested_permissions
            .iter()
            .map(|permission| permission.manifest_key().to_owned())
            .collect(),
        is_update,
    }
}

fn parse_permissions(keys: &[String]) -> Result<Vec<Permission>, String> {
    let mut seen = std::collections::HashSet::new();
    keys.iter()
        .map(|key| {
            let permission = Permission::from_manifest_key(key)
                .ok_or_else(|| tr!("extensions-manager-unknown-permission", permission = key.as_str()))?;
            if !seen.insert(permission) {
                return Err(tr!("extensions-manager-duplicate-permission", permission = key.as_str()));
            }
            Ok(permission)
        })
        .collect()
}

fn operation_error(error: impl std::fmt::Display) -> String {
    tr!("extensions-manager-operation-error", error = error.to_string())
}

fn publish_state(shared: &RwLock<Snapshot>, manager: Option<&PersistentExtensionManager>, panels: &[PanelView], status: Option<String>) {
    if let Ok(mut snapshot) = shared.write() {
        snapshot.ready = manager.is_some();
        if let Some(manager) = manager {
            snapshot.error = None;
            snapshot.safe_mode = manager.safe_mode();
            snapshot.installed = manager.list();
            snapshot.panels = panels.to_vec();
        }
        snapshot.status = status;
    }
}

fn publish_error(shared: &RwLock<Snapshot>, error: String) {
    if let Ok(mut snapshot) = shared.write() {
        snapshot.status = Some(error);
    }
}

fn update_pending(shared: &RwLock<Snapshot>, pending: Option<Review>) {
    if let Ok(mut snapshot) = shared.write() {
        snapshot.pending = pending;
    }
}

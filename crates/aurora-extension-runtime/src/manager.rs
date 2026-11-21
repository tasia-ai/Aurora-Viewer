use crate::{GuestModule, Manifest, PackageError, ValidatedPackage, validate};
use aurora_extension_api::{ExtensionAction, ExtensionEvent, ExtensionRequest, Permission};
use semver::Version;
use std::collections::{BTreeMap, HashSet};

/// The user-visible state of an installed extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionState {
    /// Installed but not enabled.
    Disabled,
    /// Explicitly enabled with all requested permissions granted.
    Enabled,
    /// Disabled after a guest trap, malformed response or capability violation.
    Quarantined,
}

/// Safe metadata shown to the extension manager before installation consent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionReview {
    /// Declared package identity and API compatibility.
    pub manifest: Manifest,
    /// Canonical package-content hash; integrity only, not publisher authentication.
    pub content_sha256: [u8; 32],
    /// Requested capabilities, parsed from the manifest allowlist.
    pub requested_permissions: Vec<Permission>,
}

/// A package that has passed validation but has not yet been installed or granted permissions.
pub struct StagedExtension {
    review: ExtensionReview,
    package: ValidatedPackage,
    guest: GuestModule,
}

impl StagedExtension {
    /// Read the package review data for an install or update consent screen.
    pub fn review(&self) -> &ExtensionReview {
        &self.review
    }
}

struct InstalledExtension {
    manifest: Manifest,
    content_sha256: [u8; 32],
    requested: HashSet<Permission>,
    granted: HashSet<Permission>,
    guest: GuestModule,
    state: ExtensionState,
}

/// Safe metadata for one installed extension; never contains guest code or secrets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionSummary {
    /// Parsed extension manifest.
    pub manifest: Manifest,
    /// Integrity hash of the installed package.
    pub content_sha256: [u8; 32],
    /// Requested permissions from the manifest.
    pub requested_permissions: Vec<Permission>,
    /// Permissions explicitly granted by the user.
    pub granted_permissions: Vec<Permission>,
    /// Installed lifecycle state.
    pub state: ExtensionState,
}

/// A lifecycle or dispatch failure. Errors are intentionally bounded and do not contain guest text.
#[derive(Debug)]
pub enum ManagerError {
    /// Package validation or WASM compilation failed.
    Package(PackageError),
    /// The package ID is already installed.
    AlreadyInstalled,
    /// The requested extension ID is not installed.
    NotInstalled,
    /// An attempted approval included an unrequested or duplicate capability.
    InvalidPermissionSet,
    /// Enabling requires explicit grants for every requested permission.
    MissingPermissions,
    /// The permission to revoke was not requested or is not currently granted.
    PermissionNotGranted,
    /// An update must keep the same extension ID and increase its semantic version.
    InvalidUpdate,
    /// Safe mode blocks activation and event execution.
    SafeMode,
    /// The extension is installed but disabled.
    Disabled,
    /// The extension is quarantined and requires explicit recovery.
    Quarantined,
    /// The host supplied a malformed or incompatible event.
    InvalidEvent,
    /// The incoming event needs a capability that has not been granted.
    EventPermissionDenied,
    /// The guest requested an output action without its corresponding grant.
    CapabilityViolation,
    /// The guest trapped, exhausted fuel or produced an invalid API response.
    RuntimeFailure,
}

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Package(error) => return write!(f, "{error}"),
            Self::AlreadyInstalled => "extension is already installed",
            Self::NotInstalled => "extension is not installed",
            Self::InvalidPermissionSet => "permission approval is invalid",
            Self::MissingPermissions => "all requested permissions require explicit approval",
            Self::PermissionNotGranted => "permission is not currently granted",
            Self::InvalidUpdate => "extension update is invalid",
            Self::SafeMode => "extension execution is disabled in safe mode",
            Self::Disabled => "extension is disabled",
            Self::Quarantined => "extension is quarantined",
            Self::InvalidEvent => "host event is invalid",
            Self::EventPermissionDenied => "event permission was not granted",
            Self::CapabilityViolation => "extension requested an ungranted capability",
            Self::RuntimeFailure => "extension failed and was quarantined",
        })
    }
}

impl std::error::Error for ManagerError {}

impl From<PackageError> for ManagerError {
    fn from(error: PackageError) -> Self {
        Self::Package(error)
    }
}

/// In-memory lifecycle and permission broker. The viewer persists packages and grants separately.
pub struct ExtensionManager {
    supported_api: Version,
    safe_mode: bool,
    extensions: BTreeMap<String, InstalledExtension>,
}

impl ExtensionManager {
    /// Create a manager for one supported API version; extensions start disabled and ungranted.
    pub fn new(supported_api: Version) -> Self {
        Self {
            supported_api,
            safe_mode: false,
            extensions: BTreeMap::new(),
        }
    }

    /// Enable or disable host-wide safe mode. This never changes stored user grants.
    pub fn set_safe_mode(&mut self, enabled: bool) {
        self.safe_mode = enabled;
    }

    /// Whether all third-party extension dispatch is currently blocked.
    pub fn safe_mode(&self) -> bool {
        self.safe_mode
    }

    /// Validate and stage a new package without installing or granting it.
    pub fn stage_install(&self, bytes: &[u8]) -> Result<StagedExtension, ManagerError> {
        let staged = self.stage_package(bytes)?;
        if self.extensions.contains_key(&staged.review.manifest.id) {
            return Err(ManagerError::AlreadyInstalled);
        }
        Ok(staged)
    }

    /// Validate a package as an update for an existing ID; no state changes until `apply_update`.
    pub fn stage_update(&self, id: &str, bytes: &[u8]) -> Result<StagedExtension, ManagerError> {
        let current = self.extensions.get(id).ok_or(ManagerError::NotInstalled)?;
        let staged = self.stage_package(bytes)?;
        if staged.review.manifest.id != id {
            return Err(ManagerError::InvalidUpdate);
        }
        let old_version = Version::parse(&current.manifest.version).map_err(|_| ManagerError::InvalidUpdate)?;
        let new_version = Version::parse(&staged.review.manifest.version).map_err(|_| ManagerError::InvalidUpdate)?;
        if new_version <= old_version {
            return Err(ManagerError::InvalidUpdate);
        }
        Ok(staged)
    }

    fn stage_package(&self, bytes: &[u8]) -> Result<StagedExtension, ManagerError> {
        let package = validate(bytes, &self.supported_api)?;
        let guest = GuestModule::compile(&package.module)?;
        let requested_permissions = parse_permissions(&package.manifest)?;
        let review = ExtensionReview {
            manifest: package.manifest.clone(),
            content_sha256: package.content_sha256,
            requested_permissions,
        };
        Ok(StagedExtension { review, package, guest })
    }

    /// Install a staged package. Grants are explicit; `enable` is a separate explicit choice.
    pub fn install(
        &mut self,
        staged: StagedExtension,
        approved_permissions: &[Permission],
        enable: bool,
    ) -> Result<ExtensionSummary, ManagerError> {
        if self.extensions.contains_key(&staged.review.manifest.id) {
            return Err(ManagerError::AlreadyInstalled);
        }
        if enable && self.safe_mode {
            return Err(ManagerError::SafeMode);
        }
        let granted = approved_set(&staged.review.requested_permissions, approved_permissions)?;
        if enable && granted != requested_set(&staged.review.requested_permissions) {
            return Err(ManagerError::MissingPermissions);
        }
        let state = if enable {
            ExtensionState::Enabled
        } else {
            ExtensionState::Disabled
        };
        let id = staged.package.manifest.id.clone();
        self.extensions.insert(
            id.clone(),
            InstalledExtension {
                manifest: staged.package.manifest,
                content_sha256: staged.package.content_sha256,
                requested: requested_set(&staged.review.requested_permissions),
                granted,
                guest: staged.guest,
                state,
            },
        );
        self.summary(&id)
    }

    /// Apply an update only after the caller supplies explicit approval for its new permission set.
    pub fn apply_update(
        &mut self,
        staged: StagedExtension,
        approved_permissions: &[Permission],
        enable: bool,
    ) -> Result<ExtensionSummary, ManagerError> {
        let id = staged.review.manifest.id.clone();
        let current = self.extensions.get(&id).ok_or(ManagerError::NotInstalled)?;
        let current_version = Version::parse(&current.manifest.version).map_err(|_| ManagerError::InvalidUpdate)?;
        let next_version = Version::parse(&staged.package.manifest.version).map_err(|_| ManagerError::InvalidUpdate)?;
        if next_version <= current_version {
            return Err(ManagerError::InvalidUpdate);
        }
        if enable && self.safe_mode {
            return Err(ManagerError::SafeMode);
        }
        let granted = approved_set(&staged.review.requested_permissions, approved_permissions)?;
        if enable && granted != requested_set(&staged.review.requested_permissions) {
            return Err(ManagerError::MissingPermissions);
        }
        let state = if enable {
            ExtensionState::Enabled
        } else {
            ExtensionState::Disabled
        };
        self.extensions.insert(
            id.clone(),
            InstalledExtension {
                manifest: staged.package.manifest,
                content_sha256: staged.package.content_sha256,
                requested: requested_set(&staged.review.requested_permissions),
                granted,
                guest: staged.guest,
                state,
            },
        );
        self.summary(&id)
    }

    /// Enable an installed extension after verifying the full, explicit grant set.
    pub fn enable(&mut self, id: &str, approved_permissions: &[Permission]) -> Result<(), ManagerError> {
        if self.safe_mode {
            return Err(ManagerError::SafeMode);
        }
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        if extension.state == ExtensionState::Quarantined {
            return Err(ManagerError::Quarantined);
        }
        let requested = permission_vec(&extension.requested);
        let granted = approved_set(&requested, approved_permissions)?;
        if granted != extension.requested {
            return Err(ManagerError::MissingPermissions);
        }
        extension.granted = granted;
        extension.state = ExtensionState::Enabled;
        Ok(())
    }

    /// Disable an extension without changing its explicitly granted permissions.
    pub fn disable(&mut self, id: &str) -> Result<(), ManagerError> {
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        if extension.state != ExtensionState::Quarantined {
            extension.state = ExtensionState::Disabled;
        }
        Ok(())
    }

    /// Revoke one grant. Any revocation disables the extension until all required grants are approved again.
    pub fn revoke_permission(&mut self, id: &str, permission: Permission) -> Result<(), ManagerError> {
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        if !extension.requested.contains(&permission) || !extension.granted.remove(&permission) {
            return Err(ManagerError::PermissionNotGranted);
        }
        extension.state = ExtensionState::Disabled;
        Ok(())
    }

    /// Clear quarantine after user review. This leaves the extension disabled; enabling still requires consent.
    pub fn clear_quarantine(&mut self, id: &str) -> Result<(), ManagerError> {
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        if extension.state == ExtensionState::Quarantined {
            extension.state = ExtensionState::Disabled;
        }
        Ok(())
    }

    /// Restore a durable quarantine marker from the local registry during startup.
    pub(crate) fn restore_quarantine(&mut self, id: &str) -> Result<(), ManagerError> {
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        extension.state = ExtensionState::Quarantined;
        Ok(())
    }

    /// Remove an extension and all in-memory grants. The viewer removes its package files separately.
    pub fn uninstall(&mut self, id: &str) -> Result<(), ManagerError> {
        self.extensions.remove(id).map(|_| ()).ok_or(ManagerError::NotInstalled)
    }

    /// Return safe metadata for every installed extension.
    pub fn list(&self) -> Vec<ExtensionSummary> {
        self.extensions.keys().filter_map(|id| self.summary(id).ok()).collect()
    }

    /// Return safe metadata for one installed extension.
    pub fn summary(&self, id: &str) -> Result<ExtensionSummary, ManagerError> {
        let extension = self.extensions.get(id).ok_or(ManagerError::NotInstalled)?;
        Ok(ExtensionSummary {
            manifest: extension.manifest.clone(),
            content_sha256: extension.content_sha256,
            requested_permissions: permission_vec(&extension.requested),
            granted_permissions: permission_vec(&extension.granted),
            state: extension.state,
        })
    }

    /// Deliver one event only when enabled and all required input/output grants are present.
    pub fn dispatch(&mut self, id: &str, request: &ExtensionRequest) -> Result<Vec<ExtensionAction>, ManagerError> {
        if self.safe_mode {
            return Err(ManagerError::SafeMode);
        }
        request.validate().map_err(|_| ManagerError::InvalidEvent)?;
        let extension = self.extensions.get_mut(id).ok_or(ManagerError::NotInstalled)?;
        match extension.state {
            ExtensionState::Disabled => return Err(ManagerError::Disabled),
            ExtensionState::Quarantined => return Err(ManagerError::Quarantined),
            ExtensionState::Enabled => {}
        }
        if let Some(permission) = event_permission(&request.event)
            && !extension.granted.contains(&permission)
        {
            return Err(ManagerError::EventPermissionDenied);
        }
        let response = match extension.guest.handle_event(request) {
            Ok(response) => response,
            Err(_) => {
                extension.state = ExtensionState::Quarantined;
                return Err(ManagerError::RuntimeFailure);
            }
        };
        for action in &response.actions {
            if !extension.granted.contains(&action_permission(action)) {
                extension.state = ExtensionState::Quarantined;
                return Err(ManagerError::CapabilityViolation);
            }
        }
        Ok(response.actions)
    }
}

fn parse_permissions(manifest: &Manifest) -> Result<Vec<Permission>, ManagerError> {
    manifest
        .permissions
        .iter()
        .map(|key| Permission::from_manifest_key(key).ok_or(ManagerError::InvalidPermissionSet))
        .collect()
}

fn permission_vec(permissions: &HashSet<Permission>) -> Vec<Permission> {
    let mut sorted: Vec<_> = permissions.iter().copied().collect();
    sorted.sort_by_key(|permission| permission.manifest_key());
    sorted
}

fn requested_set(permissions: &[Permission]) -> HashSet<Permission> {
    permissions.iter().copied().collect()
}

fn approved_set(requested: &[Permission], approved: &[Permission]) -> Result<HashSet<Permission>, ManagerError> {
    let requested = requested_set(requested);
    let approved_set: HashSet<_> = approved.iter().copied().collect();
    if approved_set.len() != approved.len() || !approved_set.is_subset(&requested) {
        return Err(ManagerError::InvalidPermissionSet);
    }
    Ok(approved_set)
}

fn event_permission(event: &ExtensionEvent) -> Option<Permission> {
    match event {
        ExtensionEvent::NearbyChat { .. } => Some(Permission::ChatNearbyRead),
        ExtensionEvent::PanelAction { .. } => Some(Permission::UiPanelRegister),
        ExtensionEvent::Initialize { .. } | ExtensionEvent::Shutdown => None,
    }
}

fn action_permission(action: &ExtensionAction) -> Permission {
    match action {
        ExtensionAction::AnnotateChat { .. } => Permission::UiChatAnnotate,
        ExtensionAction::ShowNotification { .. } => Permission::UiNotificationShow,
        ExtensionAction::RegisterPanel { .. } => Permission::UiPanelRegister,
    }
}

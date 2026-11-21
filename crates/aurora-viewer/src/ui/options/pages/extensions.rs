use super::super::cx::Cx;
use crate::extension_host::Command;
use crate::ui::widgets;
use aurora_extension_api::{PanelControl, PanelEventValue};
use aurora_extension_runtime::ExtensionState;
use aurora_i18n::tr;

fn permission_description(key: &str) -> String {
    match key {
        "chat.nearby.read" => tr!("extensions-permission-chat-nearby-read").to_owned(),
        "ui.chat.annotate" => tr!("extensions-permission-ui-chat-annotate").to_owned(),
        "ui.notification.show" => tr!("extensions-permission-ui-notification-show").to_owned(),
        "ui.panel.register" => tr!("extensions-permission-ui-panel-register").to_owned(),
        _ => tr!("extensions-manager-unknown-permission", permission = key),
    }
}

/// The Preferences › Extensions package manager.
pub fn page(cx: &mut Cx) {
    if !cx.shows_independent_controls() {
        return;
    }
    let p = cx.p;
    cx.ui.label(tr!("extensions-manager-intro"));
    cx.ui.label(tr!("extensions-manager-sandbox"));
    widgets::divider(cx.ui, &p);

    let mut safe_mode = cx.st.extensions.safe_mode;
    if widgets::switch_labeled(cx.ui, &p, &mut safe_mode, tr!("extensions-manager-safe-mode")).changed() {
        cx.st.extensions.safe_mode = safe_mode;
        cx.r.extension_command = Some(Command::SafeMode(safe_mode));
    }
    if let Some(error) = &cx.st.extensions.error {
        cx.ui.colored_label(cx.p.danger, error);
    }
    if let Some(status) = &cx.st.extensions.status {
        cx.ui.label(status);
    }

    if let Some(review) = cx.st.extensions.pending.clone() {
        if cx.st.extension_review_id.as_deref() != Some(review.id.as_str()) {
            cx.st.extension_review_id = Some(review.id.clone());
            cx.st.extension_grants.clear();
            cx.st.extension_enable_after_install = false;
        }
        widgets::divider(cx.ui, &p);
        cx.ui.heading(if review.is_update {
            tr!("extensions-manager-review-update")
        } else {
            tr!("extensions-manager-review-install")
        });
        cx.ui.label(format!("{} ({})", review.name, review.version));
        cx.ui.label(tr!("extensions-manager-id", id = review.id.as_str()));
        cx.ui
            .label(tr!("extensions-manager-publisher", publisher = review.publisher_claim.as_str()));
        cx.ui.label(tr!("extensions-manager-hash", hash = review.content_sha256.as_str()));
        cx.ui.label(tr!("extensions-manager-requested-permissions"));
        for permission in &review.requested_permissions {
            let mut checked = cx.st.extension_grants.contains(permission);
            let changed = widgets::checkbox(cx.ui, &p, &mut checked, permission).changed();
            cx.ui.label(permission_description(permission));
            if changed {
                if checked {
                    cx.st.extension_grants.insert(permission.clone());
                } else {
                    cx.st.extension_grants.remove(permission);
                    cx.st.extension_enable_after_install = false;
                }
            }
        }
        let all_approved = review
            .requested_permissions
            .iter()
            .all(|permission| cx.st.extension_grants.contains(permission));
        let mut enable = cx.st.extension_enable_after_install;
        if widgets::switch_labeled(
            cx.ui,
            &p,
            &mut enable,
            if review.is_update {
                tr!("extensions-manager-enable-update")
            } else {
                tr!("extensions-manager-enable-install")
            },
        )
        .changed()
        {
            cx.st.extension_enable_after_install = enable;
        }
        let can_install = !enable || all_approved;
        cx.ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    can_install,
                    egui::Button::new(if review.is_update {
                        tr!("extensions-manager-update")
                    } else {
                        tr!("extensions-manager-install")
                    }),
                )
                .clicked()
            {
                cx.r.extension_command = Some(Command::CommitInstall {
                    permissions: review
                        .requested_permissions
                        .iter()
                        .filter(|permission| cx.st.extension_grants.contains(*permission))
                        .cloned()
                        .collect(),
                    enable: cx.st.extension_enable_after_install,
                });
            }
            if ui.button(tr!("common-cancel")).clicked() {
                cx.st.extension_grants.clear();
                cx.st.extension_review_id = None;
                cx.st.extension_enable_after_install = false;
                cx.r.extension_command = Some(Command::CancelInstall);
            }
        });
    } else {
        cx.st.extension_review_id = None;
        cx.st.extension_grants.clear();
        cx.st.extension_enable_after_install = false;
        if cx
            .ui
            .add_enabled(cx.st.extensions.ready, egui::Button::new(tr!("extensions-manager-install-file")))
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title(tr!("extensions-manager-install-dialog"))
                .add_filter(tr!("extensions-manager-package-filter"), &["ext"])
                .pick_file()
        {
            cx.r.extension_command = Some(Command::ChooseInstall(path));
        }
    }

    if !cx.st.extensions.panels.is_empty() {
        widgets::divider(cx.ui, &p);
        cx.ui.heading(tr!("extensions-manager-panels"));
        for panel in cx.st.extensions.panels.clone() {
            widgets::surface(cx.ui, &p, |ui| {
                ui.heading(&panel.title);
                ui.label(tr!(
                    "extensions-manager-panel-identity",
                    extension = panel.extension_id.as_str(),
                    panel = panel.panel_id.as_str()
                ));
                ui.add_enabled_ui(!cx.st.extensions.safe_mode, |ui| {
                    for control in &panel.controls {
                        match control {
                            PanelControl::Label { text, .. } => {
                                ui.label(text);
                            }
                            PanelControl::Toggle { id, label, value } => {
                                let mut value = *value;
                                if widgets::switch_labeled(ui, &p, &mut value, label).changed() {
                                    cx.r.extension_command = Some(Command::PanelAction {
                                        id: panel.extension_id.clone(),
                                        panel_id: panel.panel_id.clone(),
                                        control_id: id.clone(),
                                        value: PanelEventValue::Toggled { value },
                                    });
                                }
                            }
                            PanelControl::Button { id, label } => {
                                if ui.button(label).clicked() {
                                    cx.r.extension_command = Some(Command::PanelAction {
                                        id: panel.extension_id.clone(),
                                        panel_id: panel.panel_id.clone(),
                                        control_id: id.clone(),
                                        value: PanelEventValue::Clicked,
                                    });
                                }
                            }
                        }
                    }
                });
            });
        }
    }

    widgets::divider(cx.ui, &p);
    cx.ui.heading(tr!("extensions-manager-installed"));
    if !cx.st.extensions.ready {
        cx.ui.label(tr!("extensions-manager-loading"));
    } else if cx.st.extensions.installed.is_empty() {
        cx.ui.label(tr!("extensions-manager-none"));
    }

    for extension in cx.st.extensions.installed.clone() {
        let id = extension.manifest.id.clone();
        widgets::surface(cx.ui, &p, |ui| {
            ui.heading(tr!(
                "extensions-manager-name-version",
                name = extension.manifest.name.as_str(),
                version = extension.manifest.version.as_str()
            ));
            ui.label(tr!(
                "extensions-manager-publisher-line",
                id = id.as_str(),
                publisher = extension.manifest.publisher.as_str()
            ));
            let state = match extension.state {
                ExtensionState::Disabled => tr!("extensions-manager-state-disabled"),
                ExtensionState::Enabled => tr!("extensions-manager-state-enabled"),
                ExtensionState::Quarantined => tr!("extensions-manager-state-quarantined"),
            };
            ui.label(tr!("extensions-manager-state", state = state));
            let requested = extension
                .requested_permissions
                .iter()
                .map(|permission| permission.manifest_key())
                .collect::<Vec<_>>()
                .join(", ");
            ui.label(tr!("extensions-manager-requested", permissions = requested.as_str()));
            for permission in &extension.requested_permissions {
                ui.small(permission_description(permission.manifest_key()));
            }
            let granted = extension
                .granted_permissions
                .iter()
                .map(|permission| permission.manifest_key())
                .collect::<Vec<_>>()
                .join(", ");
            ui.label(tr!("extensions-manager-granted", permissions = granted.as_str()));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(cx.st.extensions.ready, egui::Button::new(tr!("extensions-manager-update-file")))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title(tr!("extensions-manager-update-dialog"))
                        .add_filter(tr!("extensions-manager-package-filter"), &["ext"])
                        .pick_file()
                {
                    cx.r.extension_command = Some(Command::ChooseUpdate { id: id.clone(), path });
                }
                match extension.state {
                    ExtensionState::Enabled => {
                        if ui.button(tr!("extensions-manager-disable")).clicked() {
                            cx.r.extension_command = Some(Command::Disable { id: id.clone() });
                        }
                    }
                    ExtensionState::Disabled => {
                        let grants = extension
                            .requested_permissions
                            .iter()
                            .map(|p| p.manifest_key().to_owned())
                            .collect();
                        if ui
                            .add_enabled(
                                !cx.st.extensions.safe_mode,
                                egui::Button::new(tr!("extensions-manager-grant-enable")),
                            )
                            .clicked()
                        {
                            cx.r.extension_command = Some(Command::Enable {
                                id: id.clone(),
                                permissions: grants,
                            });
                        }
                    }
                    ExtensionState::Quarantined => {
                        if ui.button(tr!("extensions-manager-clear-quarantine")).clicked() {
                            cx.r.extension_command = Some(Command::ClearQuarantine { id: id.clone() });
                        }
                    }
                }
                for permission in &extension.granted_permissions {
                    let key = permission.manifest_key().to_owned();
                    if ui.button(tr!("extensions-manager-revoke", permission = key.as_str())).clicked() {
                        cx.r.extension_command = Some(Command::Revoke {
                            id: id.clone(),
                            permission: key,
                        });
                    }
                }
                if ui.button(tr!("extensions-manager-uninstall")).clicked() {
                    cx.r.extension_command = Some(Command::Uninstall { id: id.clone() });
                }
            });
        });
    }
}

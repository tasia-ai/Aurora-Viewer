use crate::{ExtensionManager, ExtensionState, ExtensionSummary, ManagerError, StagedExtension};
use aurora_extension_api::Permission;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const REGISTRY_VERSION: u32 = 1;
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const MAX_INSTALLED_EXTENSIONS: usize = 32;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Failure while loading or durably updating the per-user extension registry.
#[derive(Debug)]
pub enum PersistentManagerError {
    /// Package validation or lifecycle failure.
    Manager(ManagerError),
    /// Registry or package storage failed or contained invalid data.
    Storage(String),
}

impl fmt::Display for PersistentManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manager(error) => write!(f, "{error}"),
            Self::Storage(error) => write!(f, "extension storage error: {error}"),
        }
    }
}
impl Error for PersistentManagerError {}
impl From<ManagerError> for PersistentManagerError {
    fn from(error: ManagerError) -> Self {
        Self::Manager(error)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema_version: u32,
    safe_mode: bool,
    extensions: Vec<RegistryEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryEntry {
    id: String,
    archive_sha256: String,
    content_sha256: String,
    permissions: Vec<String>,
    state: StoredState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StoredState {
    Disabled,
    Enabled,
    Quarantined,
}

/// Persistent package/grant manager. Packages are immutable content-addressed files;
/// registry updates are written atomically and contain no guest code.
pub struct PersistentExtensionManager {
    root: PathBuf,
    supported_api: Version,
    manager: ExtensionManager,
    archive_hashes: BTreeMap<String, String>,
    registry: Registry,
}

impl PersistentExtensionManager {
    /// Open or create an extension store under a per-user data directory.
    pub fn open(root: impl Into<PathBuf>, supported_api: Version) -> Result<Self, PersistentManagerError> {
        let root = root.into();
        fs::create_dir_all(root.join("packages")).map_err(storage_error)?;
        let registry_path = root.join("registry.json");
        let registry = if registry_path.exists() {
            let metadata = fs::metadata(&registry_path).map_err(storage_error)?;
            if metadata.len() > MAX_REGISTRY_BYTES {
                return Err(storage("registry exceeds size limit"));
            }
            let bytes = fs::read(&registry_path).map_err(storage_error)?;
            serde_json::from_slice::<Registry>(&bytes).map_err(|_| storage("registry is malformed"))?
        } else {
            Registry {
                schema_version: REGISTRY_VERSION,
                safe_mode: false,
                extensions: Vec::new(),
            }
        };
        if registry.schema_version != REGISTRY_VERSION || registry.extensions.len() > MAX_INSTALLED_EXTENSIONS {
            return Err(storage("registry version or extension count is unsupported"));
        }

        let mut manager = ExtensionManager::new(supported_api.clone());
        let mut archive_hashes = BTreeMap::new();
        let mut ids = HashSet::new();
        for entry in &registry.extensions {
            if !valid_extension_id(&entry.id)
                || !valid_digest(&entry.archive_sha256)
                || !valid_digest(&entry.content_sha256)
                || !ids.insert(entry.id.clone())
            {
                return Err(storage("registry contains an invalid or duplicate extension record"));
            }
            let path = package_path(&root, &entry.id, &entry.archive_sha256);
            let metadata = fs::metadata(&path).map_err(storage_error)?;
            if metadata.len() > crate::MAX_PACKAGE_BYTES as u64 {
                return Err(storage("stored package exceeds size limit"));
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            fs::File::open(&path)
                .and_then(|mut file| file.read_to_end(&mut bytes))
                .map_err(storage_error)?;
            if digest_hex(&bytes) != entry.archive_sha256 {
                return Err(storage("stored package hash does not match registry"));
            }
            let staged = manager.stage_install(&bytes)?;
            if staged.review().manifest.id != entry.id || digest_hex(&staged.review().content_sha256) != entry.content_sha256 {
                return Err(storage("stored package identity does not match registry"));
            }
            let grants = parse_permissions(&entry.permissions)?;
            manager.install(staged, &grants, false)?;
            match entry.state {
                StoredState::Disabled => {}
                StoredState::Enabled => manager.enable(&entry.id, &grants)?,
                StoredState::Quarantined => manager.restore_quarantine(&entry.id)?,
            }
            archive_hashes.insert(entry.id.clone(), entry.archive_sha256.clone());
        }
        manager.set_safe_mode(registry.safe_mode);
        Ok(Self {
            root,
            supported_api,
            manager,
            archive_hashes,
            registry,
        })
    }

    /// Validate and stage a package without installing it or granting permissions.
    pub fn stage_install(&self, bytes: &[u8]) -> Result<StagedExtension, PersistentManagerError> {
        if self.registry.extensions.len() >= MAX_INSTALLED_EXTENSIONS {
            return Err(storage("installed extension limit reached"));
        }
        Ok(self.manager.stage_install(bytes)?)
    }

    /// Commit a reviewed package and only the permissions explicitly approved by the user.
    pub fn install(
        &mut self,
        staged: StagedExtension,
        package_bytes: &[u8],
        approved_permissions: &[Permission],
        enable: bool,
    ) -> Result<ExtensionSummary, PersistentManagerError> {
        let verified = self.manager.stage_install(package_bytes)?;
        if verified.review() != staged.review() {
            return Err(storage("staged package does not match install bytes"));
        }
        let review = staged.review().clone();
        let id = review.manifest.id.clone();
        let archive_hash = digest_hex(package_bytes);
        let path = package_path(&self.root, &id, &archive_hash);
        write_immutable(&path, package_bytes)?;
        let summary = match self.manager.install(staged, approved_permissions, enable) {
            Ok(summary) => summary,
            Err(error) => {
                remove_if_unreferenced(&path);
                return Err(error.into());
            }
        };
        self.archive_hashes.insert(id.clone(), archive_hash.clone());
        self.registry.extensions.push(entry_from_summary(&summary, archive_hash));
        if let Err(error) = self.save_registry() {
            let _ = self.manager.uninstall(&id);
            self.archive_hashes.remove(&id);
            self.registry.extensions.retain(|entry| entry.id != id);
            remove_if_unreferenced(&path);
            return Err(error);
        }
        Ok(summary)
    }

    /// Validate an update for an installed ID without changing packages or grants.
    pub fn stage_update(&self, id: &str, bytes: &[u8]) -> Result<StagedExtension, PersistentManagerError> {
        Ok(self.manager.stage_update(id, bytes)?)
    }

    /// Commit a newer package only after explicit review of its requested grants.
    pub fn update(
        &mut self,
        id: &str,
        staged: StagedExtension,
        package_bytes: &[u8],
        approved_permissions: &[Permission],
        enable: bool,
    ) -> Result<ExtensionSummary, PersistentManagerError> {
        let verified = self.manager.stage_update(id, package_bytes)?;
        if verified.review() != staged.review() {
            return Err(storage("staged update does not match install bytes"));
        }
        let review = staged.review().clone();
        let requested: HashSet<_> = review.requested_permissions.iter().copied().collect();
        let approved: HashSet<_> = approved_permissions.iter().copied().collect();
        if approved.len() != approved_permissions.len() || !approved.is_subset(&requested) {
            return Err(ManagerError::InvalidPermissionSet.into());
        }
        if enable && approved != requested {
            return Err(ManagerError::MissingPermissions.into());
        }
        if enable && self.registry.safe_mode {
            return Err(ManagerError::SafeMode.into());
        }

        let entry_index = self
            .registry
            .extensions
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(ManagerError::NotInstalled)?;
        let old_entry = self.registry.extensions[entry_index].clone();
        let old_hash = self
            .archive_hashes
            .get(id)
            .cloned()
            .ok_or_else(|| storage("package hash is missing"))?;
        let archive_hash = digest_hex(package_bytes);
        let new_package_path = package_path(&self.root, id, &archive_hash);
        write_immutable(&new_package_path, package_bytes)?;

        let mut permissions: Vec<_> = approved_permissions
            .iter()
            .map(|permission| permission.manifest_key().to_owned())
            .collect();
        permissions.sort();
        self.registry.extensions[entry_index] = RegistryEntry {
            id: id.to_owned(),
            archive_sha256: archive_hash.clone(),
            content_sha256: digest_hex(&review.content_sha256),
            permissions,
            state: if enable { StoredState::Enabled } else { StoredState::Disabled },
        };
        self.archive_hashes.insert(id.to_owned(), archive_hash);
        if let Err(error) = self.save_registry() {
            self.registry.extensions[entry_index] = old_entry;
            self.archive_hashes.insert(id.to_owned(), old_hash);
            remove_if_unreferenced(&new_package_path);
            return Err(error);
        }

        match self.manager.apply_update(staged, approved_permissions, enable) {
            Ok(summary) => {
                let _ = fs::remove_file(package_path(&self.root, id, &old_hash));
                Ok(summary)
            }
            Err(error) => {
                self.registry.extensions[entry_index] = old_entry;
                self.archive_hashes.insert(id.to_owned(), old_hash);
                if let Err(rollback_error) = self.save_registry() {
                    let _ = self.reload_manager();
                    return Err(rollback_error);
                }
                self.reload_manager()?;
                remove_if_unreferenced(&new_package_path);
                Err(error.into())
            }
        }
    }

    /// Enable only after explicit approval of every requested capability.
    pub fn enable(&mut self, id: &str, approved: &[Permission]) -> Result<(), PersistentManagerError> {
        let summary = self.manager.summary(id)?;
        let approved_set: HashSet<_> = approved.iter().copied().collect();
        let requested: HashSet<_> = summary.requested_permissions.iter().copied().collect();
        if summary.state == ExtensionState::Quarantined {
            return Err(ManagerError::Quarantined.into());
        }
        if approved_set.len() != approved.len() || approved_set != requested {
            return Err(ManagerError::MissingPermissions.into());
        }
        if self.registry.safe_mode {
            return Err(ManagerError::SafeMode.into());
        }
        let old_entry = self.entry_mut(id)?.clone();
        let entry = self.entry_mut(id)?;
        entry.permissions = approved.iter().map(|permission| permission.manifest_key().to_owned()).collect();
        entry.permissions.sort();
        entry.state = StoredState::Enabled;
        if let Err(error) = self.save_registry() {
            *self.entry_mut(id)? = old_entry;
            return Err(error);
        }
        self.manager.enable(id, approved)?;
        Ok(())
    }

    /// Disable an extension while retaining its grants.
    pub fn disable(&mut self, id: &str) -> Result<(), PersistentManagerError> {
        let summary = self.manager.summary(id)?;
        let old_entry = self.entry_mut(id)?.clone();
        self.entry_mut(id)?.state = if summary.state == ExtensionState::Quarantined {
            StoredState::Quarantined
        } else {
            StoredState::Disabled
        };
        if let Err(error) = self.save_registry() {
            *self.entry_mut(id)? = old_entry;
            return Err(error);
        }
        self.manager.disable(id)?;
        Ok(())
    }

    /// Revoke a granted capability; any revocation disables the extension.
    pub fn revoke_permission(&mut self, id: &str, permission: Permission) -> Result<(), PersistentManagerError> {
        let summary = self.manager.summary(id)?;
        if !summary.granted_permissions.contains(&permission) {
            return Err(ManagerError::PermissionNotGranted.into());
        }
        let old_entry = self.entry_mut(id)?.clone();
        let entry = self.entry_mut(id)?;
        entry
            .permissions
            .retain(|key| Permission::from_manifest_key(key) != Some(permission));
        entry.state = StoredState::Disabled;
        if let Err(error) = self.save_registry() {
            *self.entry_mut(id)? = old_entry;
            return Err(error);
        }
        self.manager.revoke_permission(id, permission)?;
        Ok(())
    }

    /// Uninstall by removing registry access first; orphan cleanup is best-effort.
    pub fn uninstall(&mut self, id: &str) -> Result<(), PersistentManagerError> {
        self.manager.summary(id)?;
        let hash = self
            .archive_hashes
            .get(id)
            .cloned()
            .ok_or_else(|| storage("package hash is missing"))?;
        let old_registry = self.registry.clone();
        self.registry.extensions.retain(|entry| entry.id != id);
        self.archive_hashes.remove(id);
        if let Err(error) = self.save_registry() {
            self.registry = old_registry;
            self.archive_hashes.insert(id.to_owned(), hash);
            return Err(error);
        }
        self.manager.uninstall(id)?;
        let _ = fs::remove_file(package_path(&self.root, id, &hash));
        Ok(())
    }

    /// Persist safe mode before applying it to the in-memory manager.
    pub fn set_safe_mode(&mut self, enabled: bool) -> Result<(), PersistentManagerError> {
        let previous = self.registry.safe_mode;
        self.registry.safe_mode = enabled;
        if let Err(error) = self.save_registry() {
            self.registry.safe_mode = previous;
            return Err(error);
        }
        self.manager.set_safe_mode(enabled);
        Ok(())
    }

    /// Dispatch one event and persist quarantine after a guest trap or capability violation.
    pub fn dispatch(
        &mut self,
        id: &str,
        request: &aurora_extension_api::ExtensionRequest,
    ) -> Result<Vec<aurora_extension_api::ExtensionAction>, PersistentManagerError> {
        let result = self.manager.dispatch(id, request);
        if matches!(
            result.as_ref(),
            Err(ManagerError::RuntimeFailure | ManagerError::CapabilityViolation)
        ) {
            let summary = self.manager.summary(id)?;
            self.entry_mut(id)?.state = StoredState::Quarantined;
            self.save_registry()?;
            debug_assert_eq!(state_from_summary(&summary), StoredState::Quarantined);
        }
        result.map_err(Into::into)
    }

    /// Clear quarantine after user review; the extension remains disabled.
    pub fn clear_quarantine(&mut self, id: &str) -> Result<(), PersistentManagerError> {
        let summary = self.manager.summary(id)?;
        if summary.state == ExtensionState::Quarantined {
            let old_entry = self.entry_mut(id)?.clone();
            self.entry_mut(id)?.state = StoredState::Disabled;
            if let Err(error) = self.save_registry() {
                *self.entry_mut(id)? = old_entry;
                return Err(error);
            }
        }
        self.manager.clear_quarantine(id)?;
        Ok(())
    }

    /// Whether extension execution is globally disabled.
    pub fn safe_mode(&self) -> bool {
        self.registry.safe_mode
    }

    /// Safe metadata for installed extensions.
    pub fn list(&self) -> Vec<ExtensionSummary> {
        self.manager.list()
    }

    fn reload_manager(&mut self) -> Result<(), PersistentManagerError> {
        let reopened = Self::open(self.root.clone(), self.supported_api.clone())?;
        self.manager = reopened.manager;
        self.archive_hashes = reopened.archive_hashes;
        self.registry = reopened.registry;
        Ok(())
    }

    fn entry_mut(&mut self, id: &str) -> Result<&mut RegistryEntry, PersistentManagerError> {
        self.registry
            .extensions
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or_else(|| storage("extension is not in the registry"))
    }

    fn save_registry(&self) -> Result<(), PersistentManagerError> {
        let mut bytes = serde_json::to_vec_pretty(&self.registry).map_err(|_| storage("could not serialize registry"))?;
        bytes.push(b'\n');
        atomic_write(&self.root.join("registry.json"), &bytes)
    }
}

fn entry_from_summary(summary: &ExtensionSummary, archive_sha256: String) -> RegistryEntry {
    let mut permissions: Vec<_> = summary
        .granted_permissions
        .iter()
        .map(|permission| permission.manifest_key().to_owned())
        .collect();
    permissions.sort();
    RegistryEntry {
        id: summary.manifest.id.clone(),
        archive_sha256,
        content_sha256: digest_hex(&summary.content_sha256),
        permissions,
        state: state_from_summary(summary),
    }
}

fn state_from_summary(summary: &ExtensionSummary) -> StoredState {
    match summary.state {
        ExtensionState::Disabled => StoredState::Disabled,
        ExtensionState::Enabled => StoredState::Enabled,
        ExtensionState::Quarantined => StoredState::Quarantined,
    }
}

fn parse_permissions(keys: &[String]) -> Result<Vec<Permission>, PersistentManagerError> {
    let mut seen = HashSet::new();
    keys.iter()
        .map(|key| {
            let permission = Permission::from_manifest_key(key).ok_or_else(|| storage("registry contains an unknown permission"))?;
            if !seen.insert(permission) {
                return Err(storage("registry contains a duplicate permission"));
            }
            Ok(permission)
        })
        .collect()
}

fn valid_extension_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.is_ascii()
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && part.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && part.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
fn package_path(root: &Path, id: &str, digest: &str) -> PathBuf {
    root.join("packages").join(format!("{id}-{digest}.ext"))
}

fn write_immutable(path: &Path, bytes: &[u8]) -> Result<(), PersistentManagerError> {
    if path.exists() {
        let existing = fs::read(path).map_err(storage_error)?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(storage("immutable package path contains different bytes"))
        };
    }
    atomic_write(path, bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PersistentManagerError> {
    let parent = path.parent().ok_or_else(|| storage("invalid storage path"))?;
    fs::create_dir_all(parent).map_err(storage_error)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp).map_err(storage_error)?;
        file.write_all(bytes).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        fs::rename(&temp, path).map_err(storage_error)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn remove_if_unreferenced(path: &Path) {
    let _ = fs::remove_file(path);
}
fn storage(message: &str) -> PersistentManagerError {
    PersistentManagerError::Storage(message.to_owned())
}
fn storage_error(error: std::io::Error) -> PersistentManagerError {
    PersistentManagerError::Storage(error.to_string())
}

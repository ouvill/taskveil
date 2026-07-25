use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    ensure_device_key, DeviceKeyStore, KeyStoreError, LocalKeyCapsule, LocalKeyCapsuleSlot,
    LocalKeyCapsuleStore, DEVICE_KEY_LEN,
};
#[cfg(any(test, target_os = "ios", target_os = "macos", target_os = "android"))]
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const DEVICE_KEY_FILE_NAME: &str = "device.key";
const SESSION_TOKEN_FILE_NAME: &str = "session.token";
const DEVICE_IDENTITY_FILE_NAME: &str = "device-identity.secret";
const WRAPPED_ACCOUNT_ROOT_FILE_NAME: &str = "account-root.wrapped";
const ACTIVE_CAPSULE_FILE_NAME: &str = "local-key.active.capsule";
const PENDING_CAPSULE_FILE_NAME: &str = "local-key.pending.capsule";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const KEYCHAIN_SERVICE: &str = "com.taskveil.app.device-key.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const SESSION_TOKEN_KEYCHAIN_SERVICE: &str = "com.taskveil.app.session-token.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const DEVICE_IDENTITY_KEYCHAIN_SERVICE: &str = "com.taskveil.app.device-identity.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const WRAPPED_ACCOUNT_ROOT_KEYCHAIN_SERVICE: &str = "com.taskveil.app.account-root-wrapped.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const ACTIVE_CAPSULE_KEYCHAIN_SERVICE: &str = "com.taskveil.app.local-key-capsule.active.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const PENDING_CAPSULE_KEYCHAIN_SERVICE: &str = "com.taskveil.app.local-key-capsule.pending.v1";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const KEYCHAIN_ACCOUNT: &str = "default";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const KEYCHAIN_ACCESS_GROUP_ENTITLEMENT: &str = "keychain-access-groups";
#[cfg(any(target_os = "ios", target_os = "macos"))]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
#[cfg(target_os = "macos")]
const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

/// Platform-selected local capsule store.
///
/// Apple production uses Data Protection Keychain. Android production uses
/// the JNI-backed Android Keystore sealer. Plaintext files are accepted only
/// by debug/test builds and never selected by a release process.
pub struct PlatformLocalKeyCapsuleStore {
    db_dir: PathBuf,
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    namespace: String,
}

impl PlatformLocalKeyCapsuleStore {
    pub fn new(db_dir: impl AsRef<Path>) -> Self {
        let db_dir = db_dir.as_ref().to_path_buf();
        Self {
            #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
            namespace: profile_store_namespace(&db_dir),
            db_dir,
        }
    }

    fn file_store(&self, slot: LocalKeyCapsuleSlot) -> FileSecretStore {
        FileSecretStore::new(
            &self.db_dir,
            match slot {
                LocalKeyCapsuleSlot::Active => ACTIVE_CAPSULE_FILE_NAME,
                LocalKeyCapsuleSlot::Pending => PENDING_CAPSULE_FILE_NAME,
            },
        )
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    fn apple_store(&self, slot: LocalKeyCapsuleSlot) -> AppleKeychainSecretStore {
        let service = match slot {
            LocalKeyCapsuleSlot::Active => ACTIVE_CAPSULE_KEYCHAIN_SERVICE,
            LocalKeyCapsuleSlot::Pending => PENDING_CAPSULE_KEYCHAIN_SERVICE,
        };
        AppleKeychainSecretStore::new_data_protection_only(format!("{service}.{}", self.namespace))
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    fn plaintext_allowed() -> bool {
        cfg!(debug_assertions)
            || cfg!(test)
            || std::env::var_os("FLUTTER_TEST").is_some()
            || std::env::var_os("DART_TEST").is_some()
    }

    fn load_bytes(
        &self,
        slot: LocalKeyCapsuleSlot,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            if is_flutter_test_process() {
                return self
                    .file_store(slot)
                    .load()
                    .map(|value| value.map(Zeroizing::new));
            }
            self.apple_store(slot)
                .load()
                .map(|value| value.map(Zeroizing::new))
        }

        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            #[cfg(target_os = "android")]
            {
                if !Self::plaintext_allowed() {
                    return crate::android_capsule_store::load(&self.namespace, slot)
                        .map(|value| value.map(Zeroizing::new));
                }
            }
            if !Self::plaintext_allowed() {
                return Err(KeyStoreError::PlaintextStoreForbidden);
            }
            self.file_store(slot)
                .load()
                .map(|value| value.map(Zeroizing::new))
        }
    }

    fn store_bytes(&self, slot: LocalKeyCapsuleSlot, value: &[u8]) -> Result<(), KeyStoreError> {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            if is_flutter_test_process() {
                return self.file_store(slot).store(value);
            }
            self.apple_store(slot).store(value)
        }

        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            #[cfg(target_os = "android")]
            {
                if !Self::plaintext_allowed() {
                    return crate::android_capsule_store::store(&self.namespace, slot, value);
                }
            }
            if !Self::plaintext_allowed() {
                return Err(KeyStoreError::PlaintextStoreForbidden);
            }
            self.file_store(slot).store(value)
        }
    }

    fn delete_slot(&self, slot: LocalKeyCapsuleSlot) -> Result<(), KeyStoreError> {
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        {
            if is_flutter_test_process() {
                return self.file_store(slot).delete();
            }
            self.apple_store(slot).delete()
        }

        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            #[cfg(target_os = "android")]
            {
                if !Self::plaintext_allowed() {
                    return crate::android_capsule_store::delete(&self.namespace, slot);
                }
            }
            if !Self::plaintext_allowed() {
                return Err(KeyStoreError::PlaintextStoreForbidden);
            }
            self.file_store(slot).delete()
        }
    }
}

#[cfg(any(test, target_os = "ios", target_os = "macos", target_os = "android"))]
fn profile_store_namespace(db_dir: &Path) -> String {
    let canonical = fs::canonicalize(db_dir).unwrap_or_else(|_| db_dir.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let mut namespace = String::with_capacity(33);
    namespace.push('p');
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in &digest[..16] {
        namespace.push(HEX[(byte >> 4) as usize] as char);
        namespace.push(HEX[(byte & 0x0f) as usize] as char);
    }
    namespace
}

#[cfg(test)]
mod profile_namespace_tests {
    use super::profile_store_namespace;
    use tempfile::TempDir;

    #[test]
    fn namespace_is_stable_per_profile_and_distinct_across_profiles() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let first_namespace = profile_store_namespace(first.path());

        assert_eq!(first_namespace, profile_store_namespace(first.path()));
        assert_ne!(first_namespace, profile_store_namespace(second.path()));
        assert_eq!(first_namespace.len(), 33);
        assert!(first_namespace.starts_with('p'));
    }
}

impl LocalKeyCapsuleStore for PlatformLocalKeyCapsuleStore {
    fn load(&self, slot: LocalKeyCapsuleSlot) -> Result<Option<LocalKeyCapsule>, KeyStoreError> {
        self.load_bytes(slot)?
            .map(|encoded| LocalKeyCapsule::decode(&encoded))
            .transpose()
    }

    fn store(
        &mut self,
        slot: LocalKeyCapsuleSlot,
        capsule: &LocalKeyCapsule,
    ) -> Result<(), KeyStoreError> {
        self.store_bytes(slot, &capsule.encode())
    }

    fn delete(&mut self, slot: LocalKeyCapsuleSlot) -> Result<(), KeyStoreError> {
        self.delete_slot(slot)
    }
}

pub fn load_or_create_device_key(
    db_dir: impl AsRef<Path>,
) -> Result<[u8; DEVICE_KEY_LEN], KeyStoreError> {
    let mut file_store = FileDeviceKeyStore::new(db_dir);

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        if is_flutter_test_process() {
            return ensure_device_key(&mut file_store);
        }

        let mut keychain_store = AppleKeychainDeviceKeyStore::new();
        ensure_device_key_with_migration(&mut keychain_store, &mut file_store)
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        ensure_device_key(&mut file_store)
    }
}

#[derive(Clone, Copy)]
pub enum AccountSecretKind {
    SessionTokens,
    DeviceIdentity,
    WrappedAccountRoot,
}

pub fn load_account_secret(
    db_dir: impl AsRef<Path>,
    kind: AccountSecretKind,
) -> Result<Option<Vec<u8>>, KeyStoreError> {
    let db_dir = db_dir.as_ref();
    let file_store = FileSecretStore::new(db_dir, kind.file_name());

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        if cfg!(debug_assertions) || is_flutter_test_process() {
            return file_store.load();
        }
        AppleKeychainSecretStore::new_data_protection_only(format!(
            "{}.{}",
            kind.keychain_service(),
            profile_store_namespace(db_dir)
        ))
        .load()
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        #[cfg(target_os = "android")]
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return crate::android_capsule_store::load_named(
                &profile_store_namespace(db_dir),
                kind.android_name(),
            );
        }
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        file_store.load()
    }
}

pub fn store_account_secret(
    db_dir: impl AsRef<Path>,
    kind: AccountSecretKind,
    value: &[u8],
) -> Result<(), KeyStoreError> {
    let db_dir = db_dir.as_ref();
    let file_store = FileSecretStore::new(db_dir, kind.file_name());

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        if cfg!(debug_assertions) || is_flutter_test_process() {
            return file_store.store(value);
        }
        AppleKeychainSecretStore::new_data_protection_only(format!(
            "{}.{}",
            kind.keychain_service(),
            profile_store_namespace(db_dir)
        ))
        .store(value)
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        #[cfg(target_os = "android")]
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return crate::android_capsule_store::store_named(
                &profile_store_namespace(db_dir),
                kind.android_name(),
                value,
            );
        }
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        file_store.store(value)
    }
}

pub fn delete_account_secret(
    db_dir: impl AsRef<Path>,
    kind: AccountSecretKind,
) -> Result<(), KeyStoreError> {
    let db_dir = db_dir.as_ref();
    let file_store = FileSecretStore::new(db_dir, kind.file_name());

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        if cfg!(debug_assertions) || is_flutter_test_process() {
            return file_store.delete();
        }
        AppleKeychainSecretStore::new_data_protection_only(format!(
            "{}.{}",
            kind.keychain_service(),
            profile_store_namespace(db_dir)
        ))
        .delete()
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    {
        #[cfg(target_os = "android")]
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return crate::android_capsule_store::delete_named(
                &profile_store_namespace(db_dir),
                kind.android_name(),
            );
        }
        if kind.requires_protected_storage() && !PlatformLocalKeyCapsuleStore::plaintext_allowed() {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        file_store.delete()
    }
}

impl AccountSecretKind {
    fn file_name(&self) -> &'static str {
        match self {
            AccountSecretKind::SessionTokens => SESSION_TOKEN_FILE_NAME,
            AccountSecretKind::DeviceIdentity => DEVICE_IDENTITY_FILE_NAME,
            AccountSecretKind::WrappedAccountRoot => WRAPPED_ACCOUNT_ROOT_FILE_NAME,
        }
    }

    #[cfg(any(target_os = "ios", target_os = "macos"))]
    fn keychain_service(&self) -> &'static str {
        match self {
            AccountSecretKind::SessionTokens => SESSION_TOKEN_KEYCHAIN_SERVICE,
            AccountSecretKind::DeviceIdentity => DEVICE_IDENTITY_KEYCHAIN_SERVICE,
            AccountSecretKind::WrappedAccountRoot => WRAPPED_ACCOUNT_ROOT_KEYCHAIN_SERVICE,
        }
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    fn requires_protected_storage(&self) -> bool {
        matches!(
            self,
            AccountSecretKind::SessionTokens | AccountSecretKind::DeviceIdentity
        )
    }

    #[cfg(target_os = "android")]
    fn android_name(&self) -> &'static str {
        match self {
            AccountSecretKind::SessionTokens => "session-tokens",
            AccountSecretKind::DeviceIdentity => "device-identity",
            AccountSecretKind::WrappedAccountRoot => "account-root-wrapped",
        }
    }
}

#[cfg(any(test, target_os = "ios", target_os = "macos"))]
pub fn ensure_device_key_with_migration(
    primary_store: &mut impl DeviceKeyStore,
    file_store: &mut impl DeviceKeyStore,
) -> Result<[u8; DEVICE_KEY_LEN], KeyStoreError> {
    if let Some(key) = primary_store.load()? {
        return Ok(key);
    }

    if let Some(file_key) = file_store.load()? {
        match primary_store.store(&file_key) {
            Ok(()) => {
                let _ = file_store.delete();
            }
            Err(_) => {
                return Ok(file_key);
            }
        }

        return Ok(file_key);
    }

    ensure_device_key(primary_store)
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
struct AppleKeychainSecretStore {
    service: String,
    account: String,
    #[cfg(target_os = "macos")]
    allow_legacy: bool,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl AppleKeychainSecretStore {
    fn new_data_protection_only(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: KEYCHAIN_ACCOUNT.to_string(),
            #[cfg(target_os = "macos")]
            allow_legacy: false,
        }
    }

    fn options(
        &self,
        backend: KeychainBackend,
        _operation: KeychainOperation,
    ) -> security_framework::base::Result<security_framework::passwords::PasswordOptions> {
        use security_framework::{
            access_control::{ProtectionMode, SecAccessControl},
            passwords::{AccessControlOptions, PasswordOptions},
        };

        let mut options = PasswordOptions::new_generic_password(&self.service, &self.account);
        options.set_access_synchronized(Some(false));
        match backend {
            KeychainBackend::DataProtection => {
                let access_control = SecAccessControl::create_with_protection(
                    Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
                    AccessControlOptions::empty().bits(),
                )?;
                options.set_access_control(access_control);
                if let Some(access_group) = current_keychain_access_group() {
                    options.set_access_group(&access_group);
                }
                options.use_protected_keychain();
            }
            #[cfg(target_os = "macos")]
            KeychainBackend::Legacy => {
                // The signed production/development path is the Data Protection
                // Keychain with the app's keychain-access-groups entitlement.
                // This legacy login-keychain ACL path exists only to keep
                // unsigned or entitlement-less local macOS builds usable.
                if _operation == KeychainOperation::Store {
                    add_macos_legacy_trusted_access(&mut options, &self.service)?;
                }
            }
        }
        Ok(options)
    }

    fn load(&self) -> Result<Option<Vec<u8>>, KeyStoreError> {
        #[cfg(target_os = "macos")]
        {
            match self.load_from_backend(KeychainBackend::DataProtection) {
                Ok(bytes) => Ok(bytes),
                Err(error) if is_keychain_missing_entitlement(&error) && self.allow_legacy => {
                    log_legacy_keychain_fallback();
                    self.load_from_legacy_backend_and_migrate()
                        .map_err(keychain_error)
                }
                Err(error) => Err(keychain_error(error)),
            }
        }

        #[cfg(target_os = "ios")]
        {
            self.load_from_backend(KeychainBackend::DataProtection)
                .map_err(keychain_error)
        }
    }

    fn store(&self, value: &[u8]) -> Result<(), KeyStoreError> {
        #[cfg(target_os = "macos")]
        {
            match self.store_in_backend(KeychainBackend::DataProtection, value) {
                Ok(()) => Ok(()),
                Err(error) if is_keychain_missing_entitlement(&error) && self.allow_legacy => {
                    log_legacy_keychain_fallback();
                    self.store_in_legacy_backend_with_acl(value, None)
                        .map_err(keychain_error)
                }
                Err(error) => Err(keychain_error(error)),
            }
        }

        #[cfg(target_os = "ios")]
        {
            self.store_in_backend(KeychainBackend::DataProtection, value)
                .map_err(keychain_error)
        }
    }

    fn delete(&self) -> Result<(), KeyStoreError> {
        #[cfg(target_os = "macos")]
        {
            if !self.allow_legacy {
                return self
                    .delete_from_backend(KeychainBackend::DataProtection)
                    .map_err(keychain_error);
            }
            let data_protection_error =
                match self.delete_from_backend(KeychainBackend::DataProtection) {
                    Ok(()) => None,
                    Err(error) if is_keychain_missing_entitlement(&error) => {
                        log_legacy_keychain_fallback();
                        None
                    }
                    Err(error) => Some(error),
                };
            let legacy_error = self.delete_from_backend(KeychainBackend::Legacy).err();
            if let Some(error) = data_protection_error {
                return Err(keychain_error(error));
            }
            if let Some(error) = legacy_error {
                return Err(keychain_error(error));
            }
            Ok(())
        }

        #[cfg(target_os = "ios")]
        {
            self.delete_from_backend(KeychainBackend::DataProtection)
                .map_err(keychain_error)
        }
    }

    fn load_from_backend(
        &self,
        backend: KeychainBackend,
    ) -> security_framework::base::Result<Option<Vec<u8>>> {
        match security_framework::passwords::generic_password(
            self.options(backend, KeychainOperation::Query)?,
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if is_keychain_item_not_found(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[cfg(target_os = "macos")]
    fn load_from_legacy_backend_and_migrate(
        &self,
    ) -> security_framework::base::Result<Option<Vec<u8>>> {
        let bytes = self.load_from_backend(KeychainBackend::Legacy)?;
        if let Some(value) = bytes.as_deref() {
            let _ = self.store_in_legacy_backend_with_acl(value, Some(value));
        }
        Ok(bytes)
    }

    fn store_in_backend(
        &self,
        backend: KeychainBackend,
        value: &[u8],
    ) -> security_framework::base::Result<()> {
        security_framework::passwords::set_generic_password_options(
            value,
            self.options(backend, KeychainOperation::Store)?,
        )
    }

    #[cfg(target_os = "macos")]
    fn store_in_legacy_backend_with_acl(
        &self,
        value: &[u8],
        existing_value: Option<&[u8]>,
    ) -> security_framework::base::Result<()> {
        store_macos_legacy_generic_password_with_acl(
            self.options(KeychainBackend::Legacy, KeychainOperation::Query)?,
            self.options(KeychainBackend::Legacy, KeychainOperation::Store)?,
            value,
            existing_value,
        )
    }

    fn delete_from_backend(
        &self,
        backend: KeychainBackend,
    ) -> security_framework::base::Result<()> {
        match security_framework::passwords::delete_generic_password_options(
            self.options(backend, KeychainOperation::Query)?,
        ) {
            Ok(()) => Ok(()),
            Err(error) if is_keychain_item_not_found(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub struct AppleKeychainDeviceKeyStore {
    service: String,
    account: String,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl AppleKeychainDeviceKeyStore {
    pub fn new() -> Self {
        Self::with_service_account(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
    }

    fn with_service_account(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    fn options(
        &self,
        backend: KeychainBackend,
        _operation: KeychainOperation,
    ) -> security_framework::base::Result<security_framework::passwords::PasswordOptions> {
        use security_framework::{
            access_control::{ProtectionMode, SecAccessControl},
            passwords::{AccessControlOptions, PasswordOptions},
        };

        let mut options = PasswordOptions::new_generic_password(&self.service, &self.account);
        options.set_access_synchronized(Some(false));
        match backend {
            KeychainBackend::DataProtection => {
                let access_control = SecAccessControl::create_with_protection(
                    Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
                    AccessControlOptions::empty().bits(),
                )?;
                options.set_access_control(access_control);
                if let Some(access_group) = current_keychain_access_group() {
                    options.set_access_group(&access_group);
                }
                options.use_protected_keychain();
            }
            #[cfg(target_os = "macos")]
            KeychainBackend::Legacy => {
                // The signed production/development path is the Data Protection
                // Keychain with the app's keychain-access-groups entitlement.
                // This legacy login-keychain ACL path exists only to keep
                // unsigned or entitlement-less local macOS builds usable.
                if _operation == KeychainOperation::Store {
                    add_macos_legacy_trusted_access(&mut options, &self.service)?;
                }
            }
        }

        Ok(options)
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[derive(Clone, Copy)]
enum KeychainBackend {
    DataProtection,
    #[cfg(target_os = "macos")]
    Legacy,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeychainOperation {
    Query,
    Store,
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl Default for AppleKeychainDeviceKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl DeviceKeyStore for AppleKeychainDeviceKeyStore {
    fn load(&self) -> Result<Option<[u8; DEVICE_KEY_LEN]>, KeyStoreError> {
        self.load_with_fallback()
    }

    fn store(&mut self, key: &[u8; DEVICE_KEY_LEN]) -> Result<(), KeyStoreError> {
        self.store_with_fallback(key)
    }

    fn delete(&mut self) -> Result<(), KeyStoreError> {
        self.delete_with_fallback()
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
impl AppleKeychainDeviceKeyStore {
    #[cfg(target_os = "macos")]
    fn load_with_fallback(&self) -> Result<Option<[u8; DEVICE_KEY_LEN]>, KeyStoreError> {
        match self.load_from_backend(KeychainBackend::DataProtection) {
            Ok(Some(bytes)) => key_from_keychain_bytes(bytes).map(Some),
            Ok(None) => self
                .load_from_legacy_backend_and_migrate()
                .map_err(keychain_error)
                .and_then(|bytes| bytes.map(key_from_keychain_bytes).transpose()),
            Err(error) if is_keychain_missing_entitlement(&error) => {
                log_legacy_keychain_fallback();
                self.load_from_legacy_backend_and_migrate()
                    .map_err(keychain_error)
                    .and_then(|bytes| bytes.map(key_from_keychain_bytes).transpose())
            }
            Err(error) => Err(keychain_error(error)),
        }
    }

    #[cfg(target_os = "ios")]
    fn load_with_fallback(&self) -> Result<Option<[u8; DEVICE_KEY_LEN]>, KeyStoreError> {
        self.load_from_backend(KeychainBackend::DataProtection)
            .map_err(keychain_error)
            .and_then(|bytes| bytes.map(key_from_keychain_bytes).transpose())
    }

    #[cfg(target_os = "macos")]
    fn store_with_fallback(&self, key: &[u8; DEVICE_KEY_LEN]) -> Result<(), KeyStoreError> {
        match self.store_in_backend(KeychainBackend::DataProtection, key) {
            Ok(()) => Ok(()),
            Err(error) if is_keychain_missing_entitlement(&error) => {
                log_legacy_keychain_fallback();
                self.store_in_legacy_backend_with_acl(key, None)
                    .map_err(keychain_error)
            }
            Err(error) => Err(keychain_error(error)),
        }
    }

    #[cfg(target_os = "ios")]
    fn store_with_fallback(&self, key: &[u8; DEVICE_KEY_LEN]) -> Result<(), KeyStoreError> {
        self.store_in_backend(KeychainBackend::DataProtection, key)
            .map_err(keychain_error)
    }

    #[cfg(target_os = "macos")]
    fn delete_with_fallback(&self) -> Result<(), KeyStoreError> {
        let data_protection_error = match self.delete_from_backend(KeychainBackend::DataProtection)
        {
            Ok(()) => None,
            Err(error) if is_keychain_missing_entitlement(&error) => {
                log_legacy_keychain_fallback();
                None
            }
            Err(error) => Some(error),
        };

        let legacy_error = self.delete_from_backend(KeychainBackend::Legacy).err();

        if let Some(error) = data_protection_error {
            return Err(keychain_error(error));
        }

        if let Some(error) = legacy_error {
            return Err(keychain_error(error));
        }

        Ok(())
    }

    #[cfg(target_os = "ios")]
    fn delete_with_fallback(&self) -> Result<(), KeyStoreError> {
        self.delete_from_backend(KeychainBackend::DataProtection)
            .map_err(keychain_error)
    }

    fn load_from_backend(
        &self,
        backend: KeychainBackend,
    ) -> security_framework::base::Result<Option<Vec<u8>>> {
        match security_framework::passwords::generic_password(
            self.options(backend, KeychainOperation::Query)?,
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if is_keychain_item_not_found(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[cfg(target_os = "macos")]
    fn load_from_legacy_backend_and_migrate(
        &self,
    ) -> security_framework::base::Result<Option<Vec<u8>>> {
        let bytes = self.load_from_backend(KeychainBackend::Legacy)?;
        if let Some(value) = bytes.as_deref() {
            let _ = self.store_in_legacy_backend_with_acl(value, Some(value));
        }
        Ok(bytes)
    }

    fn store_in_backend(
        &self,
        backend: KeychainBackend,
        key: &[u8; DEVICE_KEY_LEN],
    ) -> security_framework::base::Result<()> {
        security_framework::passwords::set_generic_password_options(
            key,
            self.options(backend, KeychainOperation::Store)?,
        )
    }

    #[cfg(target_os = "macos")]
    fn store_in_legacy_backend_with_acl(
        &self,
        key: &[u8],
        existing_value: Option<&[u8]>,
    ) -> security_framework::base::Result<()> {
        store_macos_legacy_generic_password_with_acl(
            self.options(KeychainBackend::Legacy, KeychainOperation::Query)?,
            self.options(KeychainBackend::Legacy, KeychainOperation::Store)?,
            key,
            existing_value,
        )
    }

    fn delete_from_backend(
        &self,
        backend: KeychainBackend,
    ) -> security_framework::base::Result<()> {
        match security_framework::passwords::delete_generic_password_options(
            self.options(backend, KeychainOperation::Query)?,
        ) {
            Ok(()) => Ok(()),
            Err(error) if is_keychain_item_not_found(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn current_keychain_access_group() -> Option<String> {
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType},
        string::CFString,
    };
    use core_foundation_sys::base::{CFRelease, CFTypeRef};
    use std::ptr;

    extern "C" {
        fn SecTaskCreateFromSelf(allocator: CFTypeRef) -> CFTypeRef;
        fn SecTaskCopyValueForEntitlement(
            task: CFTypeRef,
            entitlement: core_foundation_sys::string::CFStringRef,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
    }

    let task = unsafe { SecTaskCreateFromSelf(ptr::null()) };
    if task.is_null() {
        return None;
    }

    let entitlement = CFString::from_static_string(KEYCHAIN_ACCESS_GROUP_ENTITLEMENT);
    let value = unsafe {
        SecTaskCopyValueForEntitlement(task, entitlement.as_concrete_TypeRef(), ptr::null_mut())
    };
    unsafe {
        CFRelease(task);
    }

    if value.is_null() {
        return None;
    }

    let value = unsafe { CFType::wrap_under_create_rule(value) };
    let groups = value.downcast::<CFArray>()?;
    let first = groups.get(0)?;
    let first_value = unsafe { CFType::wrap_under_get_rule((*first) as CFTypeRef) };
    first_value
        .downcast::<CFString>()
        .map(|group| group.to_string())
        .filter(|group| !group.is_empty())
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn is_keychain_item_not_found(error: &security_framework::base::Error) -> bool {
    error.code() == ERR_SEC_ITEM_NOT_FOUND
}

#[cfg(target_os = "macos")]
fn is_keychain_missing_entitlement(error: &security_framework::base::Error) -> bool {
    error.code() == ERR_SEC_MISSING_ENTITLEMENT
}

#[cfg(target_os = "macos")]
fn is_keychain_duplicate_item(error: &security_framework::base::Error) -> bool {
    error.code() == security_framework_sys::base::errSecDuplicateItem
}

#[cfg(target_os = "macos")]
fn add_macos_legacy_trusted_access(
    options: &mut security_framework::passwords::PasswordOptions,
    descriptor: &str,
) -> security_framework::base::Result<()> {
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType},
        string::CFString,
    };
    use core_foundation_sys::{base::OSStatus, string::CFStringRef};
    use security_framework_sys::base::SecAccessRef;
    use std::{ffi::c_char, ptr};

    type SecTrustedApplicationRef = *mut std::ffi::c_void;

    extern "C" {
        static kSecAttrAccess: CFStringRef;

        fn SecTrustedApplicationCreateFromPath(
            path: *const c_char,
            app: *mut SecTrustedApplicationRef,
        ) -> OSStatus;

        fn SecAccessCreate(
            descriptor: CFStringRef,
            trustedlist: core_foundation_sys::array::CFArrayRef,
            access_ref: *mut SecAccessRef,
        ) -> OSStatus;
    }

    let descriptor = CFString::from(descriptor);
    let mut trusted_app: SecTrustedApplicationRef = ptr::null_mut();
    let status = unsafe { SecTrustedApplicationCreateFromPath(ptr::null(), &mut trusted_app) };
    if status != security_framework_sys::base::errSecSuccess {
        return Err(security_framework::base::Error::from_code(status));
    }

    let trusted_app = unsafe { CFType::wrap_under_create_rule(trusted_app.cast()) };
    let trusted_list = CFArray::from_CFTypes(&[trusted_app]);
    let mut access_ref: SecAccessRef = ptr::null_mut();
    let status = unsafe {
        SecAccessCreate(
            descriptor.as_concrete_TypeRef(),
            trusted_list.as_concrete_TypeRef(),
            &mut access_ref,
        )
    };
    if status != security_framework_sys::base::errSecSuccess {
        return Err(security_framework::base::Error::from_code(status));
    }

    let access = unsafe { CFType::wrap_under_create_rule(access_ref.cast()) };
    #[allow(deprecated)]
    unsafe {
        options
            .query
            .push((CFString::wrap_under_get_rule(kSecAttrAccess), access));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn store_macos_legacy_generic_password_with_acl(
    query_options: security_framework::passwords::PasswordOptions,
    add_options: security_framework::passwords::PasswordOptions,
    value: &[u8],
    existing_value: Option<&[u8]>,
) -> security_framework::base::Result<()> {
    match add_macos_legacy_generic_password(&add_options, value) {
        Ok(()) => Ok(()),
        Err(error) if is_keychain_duplicate_item(&error) => {
            let value_to_restore = existing_value.unwrap_or(value);
            delete_macos_legacy_generic_password(&query_options)?;

            match add_macos_legacy_generic_password(&add_options, value) {
                Ok(()) => Ok(()),
                Err(error) => {
                    match security_framework::passwords::set_generic_password_options(
                        value_to_restore,
                        query_options,
                    ) {
                        Ok(()) => Ok(()),
                        Err(_) => Err(error),
                    }
                }
            }
        }
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn add_macos_legacy_generic_password(
    options: &security_framework::passwords::PasswordOptions,
    value: &[u8],
) -> security_framework::base::Result<()> {
    use core_foundation::{
        base::{CFType, TCFType},
        data::CFData,
        dictionary::CFDictionary,
        string::CFString,
    };
    use security_framework_sys::{item::kSecValueData, keychain_item::SecItemAdd};

    #[allow(deprecated)]
    let mut query = options.query.clone();
    unsafe {
        query.push((
            CFString::wrap_under_get_rule(kSecValueData),
            CFData::from_buffer(value).into_CFType(),
        ));
    }
    let params: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&query);
    let status = unsafe { SecItemAdd(params.as_concrete_TypeRef(), std::ptr::null_mut()) };
    if status == security_framework_sys::base::errSecSuccess {
        Ok(())
    } else {
        Err(security_framework::base::Error::from_code(status))
    }
}

#[cfg(target_os = "macos")]
fn delete_macos_legacy_generic_password(
    options: &security_framework::passwords::PasswordOptions,
) -> security_framework::base::Result<()> {
    use core_foundation::{
        base::{CFType, TCFType},
        dictionary::CFDictionary,
        string::CFString,
    };
    use security_framework_sys::keychain_item::SecItemDelete;

    #[allow(deprecated)]
    let params: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&options.query);
    let status = unsafe { SecItemDelete(params.as_concrete_TypeRef()) };
    if status == security_framework_sys::base::errSecSuccess
        || status == security_framework_sys::base::errSecItemNotFound
    {
        Ok(())
    } else {
        Err(security_framework::base::Error::from_code(status))
    }
}

#[cfg(target_os = "macos")]
fn log_legacy_keychain_fallback() {
    static LOG_ONCE: std::sync::Once = std::sync::Once::new();
    LOG_ONCE.call_once(|| eprintln!("keychain: legacy fallback"));
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn keychain_error(error: security_framework::base::Error) -> KeyStoreError {
    KeyStoreError::Backend(format!("Apple Keychain error code {}", error.code()))
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn key_from_keychain_bytes(bytes: Vec<u8>) -> Result<[u8; DEVICE_KEY_LEN], KeyStoreError> {
    if bytes.len() != DEVICE_KEY_LEN {
        return Err(KeyStoreError::Backend(format!(
            "invalid Keychain device key length: expected {DEVICE_KEY_LEN}, got {}",
            bytes.len()
        )));
    }

    let mut key = [0u8; DEVICE_KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(any(target_os = "ios", target_os = "macos"))]
fn is_flutter_test_process() -> bool {
    if std::env::var_os("FLUTTER_TEST").is_some() || std::env::var_os("DART_TEST").is_some() {
        return true;
    }

    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name == "flutter_tester"))
        .unwrap_or(false)
}

/// Development-only Device Key Store backed by a plaintext file.
///
/// This stores the 32-byte DK as raw binary plaintext in `device.key`, so it
/// must not be used as the primary production store. It remains only as a
/// migration fallback for existing local development installs and as the
/// non-Apple development store.
pub struct FileDeviceKeyStore {
    path: PathBuf,
}

impl FileDeviceKeyStore {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            path: dir.as_ref().join(DEVICE_KEY_FILE_NAME),
        }
    }
}

struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    fn new(dir: impl AsRef<Path>, file_name: &str) -> Self {
        Self {
            path: dir.as_ref().join(file_name),
        }
    }

    fn load(&self) -> Result<Option<Vec<u8>>, KeyStoreError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(KeyStoreError::Backend(error.to_string())),
        }
    }

    fn store(&self, value: &[u8]) -> Result<(), KeyStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| KeyStoreError::Backend(error.to_string()))?;
        }

        let temporary_path = self.path.with_extension("tmp");
        fs::write(&temporary_path, value)
            .and_then(|()| fs::rename(&temporary_path, &self.path))
            .map_err(|error| KeyStoreError::Backend(error.to_string()))
    }

    fn delete(&self) -> Result<(), KeyStoreError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(KeyStoreError::Backend(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeDeviceKeyStore {
        key: Option<[u8; DEVICE_KEY_LEN]>,
        fail_store: bool,
        delete_count: usize,
    }

    impl FakeDeviceKeyStore {
        fn empty() -> Self {
            Self {
                key: None,
                fail_store: false,
                delete_count: 0,
            }
        }

        fn with_key(byte: u8) -> Self {
            Self {
                key: Some([byte; DEVICE_KEY_LEN]),
                fail_store: false,
                delete_count: 0,
            }
        }
    }

    impl DeviceKeyStore for FakeDeviceKeyStore {
        fn load(&self) -> Result<Option<[u8; DEVICE_KEY_LEN]>, KeyStoreError> {
            Ok(self.key)
        }

        fn store(&mut self, key: &[u8; DEVICE_KEY_LEN]) -> Result<(), KeyStoreError> {
            if self.fail_store {
                return Err(KeyStoreError::Backend("injected store failure".to_string()));
            }

            self.key = Some(*key);
            Ok(())
        }

        fn delete(&mut self) -> Result<(), KeyStoreError> {
            self.delete_count += 1;
            self.key = None;
            Ok(())
        }
    }

    fn assert_key_matches(actual: [u8; DEVICE_KEY_LEN], expected: [u8; DEVICE_KEY_LEN]) {
        assert!(actual == expected, "device key mismatch");
    }

    fn assert_optional_key_matches(
        actual: Option<[u8; DEVICE_KEY_LEN]>,
        expected: Option<[u8; DEVICE_KEY_LEN]>,
    ) {
        assert!(actual == expected, "device key mismatch");
    }

    #[test]
    fn migration_moves_file_key_to_primary_and_deletes_file_key() {
        let mut primary_store = FakeDeviceKeyStore::empty();
        let mut file_store = FakeDeviceKeyStore::with_key(0x42);

        let key = ensure_device_key_with_migration(&mut primary_store, &mut file_store).unwrap();

        assert_key_matches(key, [0x42; DEVICE_KEY_LEN]);
        assert_optional_key_matches(primary_store.key, Some([0x42; DEVICE_KEY_LEN]));
        assert!(file_store.key.is_none(), "file device key was not deleted");
        assert_eq!(file_store.delete_count, 1);
    }

    #[test]
    fn migration_keeps_file_key_when_primary_store_fails() {
        let mut primary_store = FakeDeviceKeyStore::empty();
        primary_store.fail_store = true;
        let mut file_store = FakeDeviceKeyStore::with_key(0x42);

        let key = ensure_device_key_with_migration(&mut primary_store, &mut file_store).unwrap();

        assert_key_matches(key, [0x42; DEVICE_KEY_LEN]);
        assert!(primary_store.key.is_none(), "primary device key was stored");
        assert_optional_key_matches(file_store.key, Some([0x42; DEVICE_KEY_LEN]));
        assert_eq!(file_store.delete_count, 0);
    }

    #[test]
    fn migration_uses_primary_key_when_both_stores_have_keys() {
        let mut primary_store = FakeDeviceKeyStore::with_key(0x11);
        let mut file_store = FakeDeviceKeyStore::with_key(0x22);

        let key = ensure_device_key_with_migration(&mut primary_store, &mut file_store).unwrap();

        assert_key_matches(key, [0x11; DEVICE_KEY_LEN]);
        assert_optional_key_matches(primary_store.key, Some([0x11; DEVICE_KEY_LEN]));
        assert_optional_key_matches(file_store.key, Some([0x22; DEVICE_KEY_LEN]));
        assert_eq!(file_store.delete_count, 0);
    }

    #[test]
    fn migration_generates_primary_key_when_both_stores_are_empty() {
        let mut primary_store = FakeDeviceKeyStore::empty();
        let mut file_store = FakeDeviceKeyStore::empty();

        let key = ensure_device_key_with_migration(&mut primary_store, &mut file_store).unwrap();

        assert_optional_key_matches(primary_store.key, Some(key));
        assert!(file_store.key.is_none(), "file device key was stored");
        assert_eq!(file_store.delete_count, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "touches the real macOS Keychain; run manually during platform verification"]
    fn apple_keychain_device_key_store_round_trips_real_keychain_item() {
        let service = format!("{}.test.{}", KEYCHAIN_SERVICE, std::process::id());
        let mut store =
            AppleKeychainDeviceKeyStore::with_service_account(service, KEYCHAIN_ACCOUNT);
        let key = [0x7b; DEVICE_KEY_LEN];

        let _ = store.delete();
        store.store(&key).unwrap();
        assert_optional_key_matches(store.load().unwrap(), Some(key));
        store.delete().unwrap();
        assert!(
            store.load().unwrap().is_none(),
            "device key was not deleted"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "touches the real macOS Keychain; run manually during platform verification"]
    fn apple_data_protection_keychain_round_trips_capsule_v2_without_prompt_flags() {
        let service = format!(
            "{}.test.{}",
            ACTIVE_CAPSULE_KEYCHAIN_SERVICE,
            std::process::id()
        );
        let store = AppleKeychainSecretStore::new_data_protection_only(service);
        let capsule = LocalKeyCapsule::new(2, [0x5a; DEVICE_KEY_LEN], Some(vec![1, 2, 3])).unwrap();

        let _ = store.delete();
        store.store(&capsule.encode()).unwrap();
        let decoded = LocalKeyCapsule::decode(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(decoded.generation(), 2);
        assert_eq!(decoded.device_key(), &[0x5a; DEVICE_KEY_LEN]);
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}

impl DeviceKeyStore for FileDeviceKeyStore {
    fn load(&self) -> Result<Option<[u8; DEVICE_KEY_LEN]>, KeyStoreError> {
        if !cfg!(debug_assertions) && !cfg!(test) {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        match fs::read(&self.path) {
            Ok(bytes) => {
                if bytes.len() != DEVICE_KEY_LEN {
                    return Err(KeyStoreError::Backend(format!(
                        "invalid device key length: expected {DEVICE_KEY_LEN}, got {}",
                        bytes.len()
                    )));
                }

                let mut key = [0u8; DEVICE_KEY_LEN];
                key.copy_from_slice(&bytes);
                Ok(Some(key))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(KeyStoreError::Backend(error.to_string())),
        }
    }

    fn store(&mut self, key: &[u8; DEVICE_KEY_LEN]) -> Result<(), KeyStoreError> {
        if !cfg!(debug_assertions) && !cfg!(test) {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| KeyStoreError::Backend(error.to_string()))?;
        }

        fs::write(&self.path, key).map_err(|error| KeyStoreError::Backend(error.to_string()))
    }

    fn delete(&mut self) -> Result<(), KeyStoreError> {
        if !cfg!(debug_assertions) && !cfg!(test) {
            return Err(KeyStoreError::PlaintextStoreForbidden);
        }
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(KeyStoreError::Backend(error.to_string())),
        }
    }
}

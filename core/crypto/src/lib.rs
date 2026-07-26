//! `taskveil-crypto`: 鍵導出・レコード暗号化を提供する crate。
//!
//! 詳細は `docs/03_技術仕様書.md` §4 暗号設計 を参照。
//!
//! suite IDはOPAQUE、key wrap、record envelopeで共有し、このcrate rootだけで
//! 定義する。

pub const CRYPTO_SUITE_ID: u16 = 0x0002;

#[cfg(target_os = "android")]
mod android_capsule_store;

pub mod aead;
pub mod dev_key_store;
pub mod device_key;
pub mod kdf;
pub mod key_hierarchy;
pub mod key_manifest;
pub mod local_capsule;
pub mod opaque;
pub mod organization;
pub mod organization_manifest;

pub use aead::{decrypt, encrypt, CryptoError};
pub use dev_key_store::{
    delete_account_secret, load_account_secret, load_or_create_device_key, store_account_secret,
    AccountSecretKind, FileDeviceKeyStore, PlatformLocalKeyCapsuleStore,
};
pub use device_key::{
    derive_local_db_key, ensure_device_key, generate_device_key, DeviceKeyStore,
    InMemoryDeviceKeyStore, KeyStoreError, DEVICE_KEY_LEN, LOCAL_DB_KEY_INFO,
};
pub use kdf::derive_key;
pub use key_manifest::{
    derive_personal_manifest_auth_key, KeyManifest, KeyManifestError,
    MIN_AUTHENTICATED_MANIFEST_LEN, PERSONAL_MANIFEST_AUTH_INFO,
};
pub use local_capsule::{
    InMemoryLocalKeyCapsuleStore, LocalKeyCapsule, LocalKeyCapsuleSlot, LocalKeyCapsuleStore,
    LOCAL_KEY_CAPSULE_VERSION,
};
pub use opaque::{
    login_parameters as opaque_login_parameters,
    registration_parameters as opaque_registration_parameters, TaskveilCipherSuite,
};
pub use organization_manifest::{OrganizationKeyManifest, OrganizationManifestError};

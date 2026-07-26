//! Canonical authenticated key-generation manifest from ADR-020.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use taskveil_protocol::RotationStatus;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::CRYPTO_SUITE_ID;

pub const PERSONAL_MANIFEST_AUTH_INFO: &[u8] = b"taskveil/personal-key-manifest-auth/v1";
const MANIFEST_MAGIC: &[u8; 4] = b"TKM2";
const MANIFEST_PAYLOAD_PREFIX_LEN: usize = 75;
const MANIFEST_AUTHENTICATOR_LEN: usize = 32;
pub const MIN_AUTHENTICATED_MANIFEST_LEN: usize =
    MANIFEST_PAYLOAD_PREFIX_LEN + MANIFEST_AUTHENTICATOR_LEN;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyManifest {
    pub tenant_id: Uuid,
    pub suite_id: u16,
    pub generation: u64,
    pub status: RotationStatus,
    pub minimum_write_generation: u64,
    pub previous_manifest_hash: [u8; 32],
    pub recipient_fingerprints: Vec<[u8; 32]>,
    pub authenticator: [u8; 32],
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KeyManifestError {
    #[error("manifest identity is invalid")]
    InvalidIdentity,
    #[error("manifest suite is unsupported")]
    UnsupportedSuite,
    #[error("manifest generation is invalid")]
    InvalidGeneration,
    #[error("manifest recipients are not canonical")]
    NonCanonicalRecipients,
    #[error("manifest authentication failed")]
    AuthenticationFailed,
    #[error("manifest chain is stale or forked")]
    ChainMismatch,
    #[error("manifest status transition is invalid")]
    InvalidTransition,
    #[error("manifest encoding overflow")]
    EncodingOverflow,
}

impl KeyManifest {
    #[allow(clippy::too_many_arguments)]
    pub fn organization_unsigned(
        tenant_id: Uuid,
        generation: u64,
        status: RotationStatus,
        minimum_write_generation: u64,
        previous_manifest_hash: [u8; 32],
        mut recipient_fingerprints: Vec<[u8; 32]>,
    ) -> Result<Self, KeyManifestError> {
        recipient_fingerprints.sort_unstable();
        recipient_fingerprints.dedup();
        let manifest = Self {
            tenant_id,
            suite_id: CRYPTO_SUITE_ID,
            generation,
            status,
            minimum_write_generation,
            previous_manifest_hash,
            recipient_fingerprints,
            authenticator: [0; 32],
        };
        manifest.validate_fields()?;
        Ok(manifest)
    }

    pub fn from_authenticated_bytes(bytes: &[u8]) -> Result<Self, KeyManifestError> {
        if bytes.len() < MIN_AUTHENTICATED_MANIFEST_LEN || &bytes[..4] != MANIFEST_MAGIC {
            return Err(KeyManifestError::InvalidIdentity);
        }
        let tenant_id =
            Uuid::from_slice(&bytes[4..20]).map_err(|_| KeyManifestError::InvalidIdentity)?;
        let suite_id = u16::from_be_bytes(
            bytes[20..22]
                .try_into()
                .map_err(|_| KeyManifestError::InvalidIdentity)?,
        );
        let generation = u64::from_be_bytes(
            bytes[22..30]
                .try_into()
                .map_err(|_| KeyManifestError::InvalidIdentity)?,
        );
        let status = match bytes[30] {
            1 => RotationStatus::Prepared,
            2 => RotationStatus::Active,
            3 => RotationStatus::Migrating,
            4 => RotationStatus::Retired,
            _ => return Err(KeyManifestError::InvalidTransition),
        };
        let minimum_write_generation = u64::from_be_bytes(
            bytes[31..39]
                .try_into()
                .map_err(|_| KeyManifestError::InvalidIdentity)?,
        );
        let previous_manifest_hash = bytes[39..71]
            .try_into()
            .map_err(|_| KeyManifestError::InvalidIdentity)?;
        let recipient_count = u32::from_be_bytes(
            bytes[71..75]
                .try_into()
                .map_err(|_| KeyManifestError::InvalidIdentity)?,
        ) as usize;
        let payload_len = MANIFEST_PAYLOAD_PREFIX_LEN
            .checked_add(
                recipient_count
                    .checked_mul(32)
                    .ok_or(KeyManifestError::EncodingOverflow)?,
            )
            .ok_or(KeyManifestError::EncodingOverflow)?;
        let authenticated_len = payload_len
            .checked_add(32)
            .ok_or(KeyManifestError::EncodingOverflow)?;
        if bytes.len() != authenticated_len {
            return Err(KeyManifestError::InvalidIdentity);
        }
        let recipient_fingerprints = bytes[MANIFEST_PAYLOAD_PREFIX_LEN..payload_len]
            .chunks_exact(32)
            .map(|chunk| {
                chunk
                    .try_into()
                    .map_err(|_| KeyManifestError::InvalidIdentity)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let authenticator = bytes[payload_len..]
            .try_into()
            .map_err(|_| KeyManifestError::InvalidIdentity)?;
        let manifest = Self {
            tenant_id,
            suite_id,
            generation,
            status,
            minimum_write_generation,
            previous_manifest_hash,
            recipient_fingerprints,
            authenticator,
        };
        manifest.validate_fields()?;
        Ok(manifest)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn authenticate_personal(
        tenant_id: Uuid,
        generation: u64,
        status: RotationStatus,
        minimum_write_generation: u64,
        previous_manifest_hash: [u8; 32],
        mut recipient_fingerprints: Vec<[u8; 32]>,
        master_key: &[u8; 32],
    ) -> Result<Self, KeyManifestError> {
        recipient_fingerprints.sort_unstable();
        recipient_fingerprints.dedup();
        let mut manifest = Self {
            tenant_id,
            suite_id: CRYPTO_SUITE_ID,
            generation,
            status,
            minimum_write_generation,
            previous_manifest_hash,
            recipient_fingerprints,
            authenticator: [0; 32],
        };
        let payload = manifest.canonical_payload()?;
        manifest.authenticator = personal_mac(master_key, &payload)?;
        Ok(manifest)
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, KeyManifestError> {
        self.validate_fields()?;
        let count = u32::try_from(self.recipient_fingerprints.len())
            .map_err(|_| KeyManifestError::EncodingOverflow)?;
        let mut out = Vec::with_capacity(
            MANIFEST_PAYLOAD_PREFIX_LEN + self.recipient_fingerprints.len() * 32,
        );
        out.extend_from_slice(MANIFEST_MAGIC);
        out.extend_from_slice(self.tenant_id.as_bytes());
        out.extend_from_slice(&self.suite_id.to_be_bytes());
        out.extend_from_slice(&self.generation.to_be_bytes());
        out.push(self.status as u8);
        out.extend_from_slice(&self.minimum_write_generation.to_be_bytes());
        out.extend_from_slice(&self.previous_manifest_hash);
        out.extend_from_slice(&count.to_be_bytes());
        for fingerprint in &self.recipient_fingerprints {
            out.extend_from_slice(fingerprint);
        }
        Ok(out)
    }

    pub fn verify_personal(&self, master_key: &[u8; 32]) -> Result<(), KeyManifestError> {
        let key = derive_personal_manifest_auth_key(master_key)?;
        self.verify_personal_with_auth_key(&key)
    }

    pub fn verify_personal_with_auth_key(
        &self,
        auth_key: &[u8; 32],
    ) -> Result<(), KeyManifestError> {
        let payload = self.canonical_payload()?;
        let mut mac = HmacSha256::new_from_slice(auth_key)
            .map_err(|_| KeyManifestError::AuthenticationFailed)?;
        mac.update(&payload);
        mac.verify_slice(&self.authenticator)
            .map_err(|_| KeyManifestError::AuthenticationFailed)
    }

    pub fn authenticated_hash(&self) -> Result<[u8; 32], KeyManifestError> {
        let mut hash = Sha256::new();
        hash.update(self.canonical_payload()?);
        hash.update(self.authenticator);
        Ok(hash.finalize().into())
    }

    pub fn authenticated_bytes(&self) -> Result<Vec<u8>, KeyManifestError> {
        let mut bytes = self.canonical_payload()?;
        bytes.extend_from_slice(&self.authenticator);
        Ok(bytes)
    }

    pub fn verify_successor(
        &self,
        next: &Self,
        master_key: &[u8; 32],
    ) -> Result<(), KeyManifestError> {
        let key = derive_personal_manifest_auth_key(master_key)?;
        self.verify_successor_with_auth_key(next, &key)
    }

    pub fn verify_successor_with_auth_key(
        &self,
        next: &Self,
        auth_key: &[u8; 32],
    ) -> Result<(), KeyManifestError> {
        next.verify_personal_with_auth_key(auth_key)?;
        if self.tenant_id != next.tenant_id
            || self.suite_id != next.suite_id
            || next.previous_manifest_hash != self.authenticated_hash()?
        {
            return Err(KeyManifestError::ChainMismatch);
        }
        if self.generation == next.generation {
            if !can_transition(self.status, next.status) {
                return Err(KeyManifestError::InvalidTransition);
            }
        } else if next.generation != self.generation + 1 || next.status != RotationStatus::Prepared
        {
            return Err(KeyManifestError::InvalidTransition);
        }
        if next.minimum_write_generation < self.minimum_write_generation {
            return Err(KeyManifestError::InvalidTransition);
        }
        Ok(())
    }

    fn validate_fields(&self) -> Result<(), KeyManifestError> {
        if self.tenant_id.is_nil() {
            return Err(KeyManifestError::InvalidIdentity);
        }
        if self.suite_id != CRYPTO_SUITE_ID {
            return Err(KeyManifestError::UnsupportedSuite);
        }
        if self.generation == 0
            || self.minimum_write_generation == 0
            || self.minimum_write_generation > self.generation
        {
            return Err(KeyManifestError::InvalidGeneration);
        }
        if self
            .recipient_fingerprints
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(KeyManifestError::NonCanonicalRecipients);
        }
        Ok(())
    }
}

fn can_transition(current: RotationStatus, next: RotationStatus) -> bool {
    matches!(
        (current, next),
        (RotationStatus::Prepared, RotationStatus::Active)
            | (RotationStatus::Active, RotationStatus::Migrating)
            | (RotationStatus::Migrating, RotationStatus::Retired)
    )
}

pub fn derive_personal_manifest_auth_key(
    master_key: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, KeyManifestError> {
    let hkdf = Hkdf::<Sha256>::new(None, master_key);
    let mut key = Zeroizing::new([0; 32]);
    hkdf.expand(PERSONAL_MANIFEST_AUTH_INFO, &mut *key)
        .map_err(|_| KeyManifestError::AuthenticationFailed)?;
    Ok(key)
}

fn personal_mac(master_key: &[u8; 32], payload: &[u8]) -> Result<[u8; 32], KeyManifestError> {
    let key = derive_personal_manifest_auth_key(master_key)?;
    let mut mac =
        HmacSha256::new_from_slice(&*key).map_err(|_| KeyManifestError::AuthenticationFailed)?;
    mac.update(payload);
    Ok(mac.finalize().into_bytes().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> KeyManifest {
        KeyManifest::authenticate_personal(
            Uuid::from_u128(1),
            2,
            RotationStatus::Prepared,
            1,
            [0; 32],
            vec![[2; 32], [1; 32], [2; 32]],
            &[7; 32],
        )
        .unwrap()
    }

    #[test]
    fn manifest_has_canonical_tkm2_bytes_and_sorted_unique_recipients() {
        let manifest = manifest();
        let payload = manifest.canonical_payload().unwrap();
        assert_eq!(&payload[..4], b"TKM2");
        assert_eq!(manifest.recipient_fingerprints, vec![[1; 32], [2; 32]]);
        assert_eq!(payload.len(), 4 + 16 + 2 + 8 + 1 + 8 + 32 + 4 + 64);
        manifest.verify_personal(&[7; 32]).unwrap();
        assert_eq!(
            manifest.verify_personal(&[8; 32]),
            Err(KeyManifestError::AuthenticationFailed)
        );
    }

    #[test]
    fn manifest_rejects_overflowing_recipient_length() {
        let mut encoded = vec![0; MIN_AUTHENTICATED_MANIFEST_LEN];
        encoded[..4].copy_from_slice(b"TKM2");
        encoded[71..75].copy_from_slice(&u32::MAX.to_be_bytes());

        assert!(KeyManifest::from_authenticated_bytes(&encoded).is_err());
    }

    #[test]
    fn manifest_rejects_replay_fork_and_skipped_status() {
        let first = manifest();
        let active = KeyManifest::authenticate_personal(
            first.tenant_id,
            first.generation,
            RotationStatus::Active,
            2,
            first.authenticated_hash().unwrap(),
            first.recipient_fingerprints.clone(),
            &[7; 32],
        )
        .unwrap();
        first.verify_successor(&active, &[7; 32]).unwrap();

        let mut replay = active.clone();
        replay.previous_manifest_hash = [9; 32];
        assert_eq!(
            first.verify_successor(&replay, &[7; 32]),
            Err(KeyManifestError::AuthenticationFailed)
        );
        let retired = KeyManifest::authenticate_personal(
            first.tenant_id,
            first.generation,
            RotationStatus::Retired,
            2,
            first.authenticated_hash().unwrap(),
            first.recipient_fingerprints.clone(),
            &[7; 32],
        )
        .unwrap();
        assert_eq!(
            first.verify_successor(&retired, &[7; 32]),
            Err(KeyManifestError::InvalidTransition)
        );
    }
}

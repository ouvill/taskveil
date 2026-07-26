//! Strict Tenant-scoped encrypted sync envelope v5.

use taskveil_crypto::{
    decrypt, encrypt,
    key_hierarchy::{derive_record_key, KeyHierarchyError},
    CryptoError, CRYPTO_SUITE_ID,
};
use taskveil_protocol::{parse_envelope_header as parse_wire_envelope_header, EnvelopeHeaderError};
pub use taskveil_protocol::{
    EnvelopeHeader, ENVELOPE_HEADER_LEN, ENVELOPE_MAGIC, ENVELOPE_MIN_LEN, ENVELOPE_VERSION,
    MAX_ENCRYPTED_BLOB_LEN,
};
use thiserror::Error;
use uuid::Uuid;

use crate::field_map::SyncPlaintext;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvelopeError {
    #[error("encrypted blob is too short")]
    BlobTooShort,
    #[error("unsupported encrypted blob version")]
    UnsupportedVersion,
    #[error("unsupported crypto suite")]
    UnsupportedSuite,
    #[error("key generation must be positive")]
    InvalidGeneration,
    #[error("invalid tenant or record identity")]
    InvalidIdentity,
    #[error("collection name is too long")]
    CollectionTooLong,
    #[error("encrypted blob exceeds 64KB limit")]
    BlobTooLarge,
    #[error("plaintext JSON serialization failed")]
    Serialization,
    #[error("plaintext JSON deserialization failed")]
    Deserialization,
    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("record key derivation failed")]
    KeyDerivation,
}

impl From<KeyHierarchyError> for EnvelopeError {
    fn from(_: KeyHierarchyError) -> Self {
        Self::KeyDerivation
    }
}

pub fn parse_envelope_header(blob: &[u8]) -> Result<EnvelopeHeader, EnvelopeError> {
    let header = parse_wire_envelope_header(blob).map_err(|error| match error {
        EnvelopeHeaderError::BlobTooShort => EnvelopeError::BlobTooShort,
        EnvelopeHeaderError::UnsupportedVersion => EnvelopeError::UnsupportedVersion,
        EnvelopeHeaderError::InvalidGeneration => EnvelopeError::InvalidGeneration,
        EnvelopeHeaderError::BlobTooLarge => EnvelopeError::BlobTooLarge,
    })?;
    if header.suite_id != CRYPTO_SUITE_ID {
        return Err(EnvelopeError::UnsupportedSuite);
    }
    Ok(header)
}

pub fn encrypt_plaintext(
    tenant_key: &[u8; 32],
    tenant_id: Uuid,
    key_generation: u64,
    collection: &str,
    record_id: Uuid,
    plaintext: &SyncPlaintext,
) -> Result<Vec<u8>, EnvelopeError> {
    if tenant_id.is_nil() || record_id.is_nil() {
        return Err(EnvelopeError::InvalidIdentity);
    }
    if key_generation == 0 {
        return Err(EnvelopeError::InvalidGeneration);
    }
    plaintext
        .validate_for_collection(collection, &record_id.to_string())
        .map_err(|_| EnvelopeError::Serialization)?;
    let aad = aad(tenant_id, key_generation, collection, record_id)?;
    let plaintext_json = serde_json::to_vec(plaintext).map_err(|_| EnvelopeError::Serialization)?;
    let record_key =
        derive_record_key(tenant_key, tenant_id, key_generation, collection, record_id)?;
    let inner = encrypt(&record_key, &plaintext_json, &aad)?;
    let mut out = Vec::with_capacity(ENVELOPE_HEADER_LEN + inner.len());
    out.extend_from_slice(ENVELOPE_MAGIC);
    out.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
    out.extend_from_slice(&key_generation.to_be_bytes());
    out.extend_from_slice(&inner);
    if out.len() > MAX_ENCRYPTED_BLOB_LEN {
        return Err(EnvelopeError::BlobTooLarge);
    }
    Ok(out)
}

pub fn decrypt_plaintext(
    tenant_key: &[u8; 32],
    tenant_id: Uuid,
    expected_generation: u64,
    collection: &str,
    record_id: Uuid,
    blob: &[u8],
) -> Result<SyncPlaintext, EnvelopeError> {
    if tenant_id.is_nil() || record_id.is_nil() {
        return Err(EnvelopeError::InvalidIdentity);
    }
    let header = parse_envelope_header(blob)?;
    if expected_generation == 0 || header.key_generation != expected_generation {
        return Err(EnvelopeError::InvalidGeneration);
    }
    let aad = aad(tenant_id, header.key_generation, collection, record_id)?;
    let record_key = derive_record_key(
        tenant_key,
        tenant_id,
        header.key_generation,
        collection,
        record_id,
    )?;
    let plaintext_json = decrypt(&record_key, &blob[ENVELOPE_HEADER_LEN..], &aad)?;
    let plaintext: SyncPlaintext =
        serde_json::from_slice(&plaintext_json).map_err(|_| EnvelopeError::Deserialization)?;
    plaintext
        .validate_for_collection(collection, &record_id.to_string())
        .map_err(|_| EnvelopeError::Deserialization)?;
    Ok(plaintext)
}

fn aad(
    tenant_id: Uuid,
    generation: u64,
    collection: &str,
    record_id: Uuid,
) -> Result<Vec<u8>, EnvelopeError> {
    let collection_len =
        u16::try_from(collection.len()).map_err(|_| EnvelopeError::CollectionTooLong)?;
    let mut aad = Vec::with_capacity(4 + 2 + 8 + 16 + 2 + collection.len() + 16);
    aad.extend_from_slice(b"TDA5");
    aad.extend_from_slice(&CRYPTO_SUITE_ID.to_be_bytes());
    aad.extend_from_slice(&generation.to_be_bytes());
    aad.extend_from_slice(tenant_id.as_bytes());
    aad.extend_from_slice(&collection_len.to_be_bytes());
    aad.extend_from_slice(collection.as_bytes());
    aad.extend_from_slice(record_id.as_bytes());
    Ok(aad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::Hlc;
    use taskveil_domain::{new_list, List};

    const GENERATION: u64 = 7;

    fn key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn tenant_id() -> Uuid {
        Uuid::from_u128(1)
    }

    fn record_id() -> Uuid {
        Uuid::from_u128(2)
    }

    fn plaintext() -> SyncPlaintext {
        let hlc = Hlc {
            wall_ms: 1_799_000_000_000,
            counter: 1,
            device_id: "device-a".to_string(),
        };
        SyncPlaintext::from_list(&sample_list(), hlc).unwrap()
    }

    fn sample_list() -> List {
        let mut list =
            new_list("Inbox".into(), "7fffffffffffffffffffffffffffffff".into(), 1).unwrap();
        list.id = record_id();
        list
    }

    fn envelope() -> Vec<u8> {
        encrypt_plaintext(
            &key(0x42),
            tenant_id(),
            GENERATION,
            "lists",
            record_id(),
            &plaintext(),
        )
        .unwrap()
    }

    #[test]
    fn envelope_v5_roundtrips_and_has_canonical_header() {
        let blob = envelope();
        let header = parse_envelope_header(&blob).unwrap();

        assert_eq!(&blob[..4], b"TDE5");
        assert_eq!(&blob[4..6], &CRYPTO_SUITE_ID.to_be_bytes());
        assert_eq!(&blob[6..14], &GENERATION.to_be_bytes());
        assert_eq!(header.key_generation, GENERATION);
        assert_eq!(
            decrypt_plaintext(
                &key(0x42),
                tenant_id(),
                GENERATION,
                "lists",
                record_id(),
                &blob,
            )
            .unwrap(),
            plaintext()
        );
    }

    #[test]
    fn envelope_rejects_wrong_key_or_aad_dimension() {
        let blob = envelope();
        assert!(matches!(
            decrypt_plaintext(
                &key(0x24),
                tenant_id(),
                GENERATION,
                "lists",
                record_id(),
                &blob
            ),
            Err(EnvelopeError::Crypto(CryptoError::DecryptionFailed))
        ));
        assert!(decrypt_plaintext(
            &key(0x42),
            Uuid::from_u128(3),
            GENERATION,
            "lists",
            record_id(),
            &blob
        )
        .is_err());
        assert!(decrypt_plaintext(
            &key(0x42),
            tenant_id(),
            GENERATION,
            "tasks",
            record_id(),
            &blob
        )
        .is_err());
        assert!(decrypt_plaintext(
            &key(0x42),
            tenant_id(),
            GENERATION,
            "lists",
            Uuid::from_u128(4),
            &blob
        )
        .is_err());
        assert_eq!(
            decrypt_plaintext(
                &key(0x42),
                tenant_id(),
                GENERATION + 1,
                "lists",
                record_id(),
                &blob
            ),
            Err(EnvelopeError::InvalidGeneration)
        );
    }

    #[test]
    fn parser_rejects_v3_unknown_suite_generation_zero_and_short_blob() {
        let mut v3 = envelope();
        v3[..4].copy_from_slice(b"TDE3");
        assert_eq!(
            parse_envelope_header(&v3),
            Err(EnvelopeError::UnsupportedVersion)
        );

        let mut suite = envelope();
        suite[4..6].copy_from_slice(&(CRYPTO_SUITE_ID + 1).to_be_bytes());
        assert_eq!(
            parse_envelope_header(&suite),
            Err(EnvelopeError::UnsupportedSuite)
        );

        let mut generation = envelope();
        generation[6..14].fill(0);
        assert_eq!(
            parse_envelope_header(&generation),
            Err(EnvelopeError::InvalidGeneration)
        );
        assert_eq!(
            parse_envelope_header(&[0; 13]),
            Err(EnvelopeError::BlobTooShort)
        );
    }

    #[test]
    fn envelope_rejects_tampering_and_oversize() {
        let mut blob = envelope();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(decrypt_plaintext(
            &key(0x42),
            tenant_id(),
            GENERATION,
            "lists",
            record_id(),
            &blob
        )
        .is_err());
        assert_eq!(
            parse_envelope_header(&vec![0; MAX_ENCRYPTED_BLOB_LEN + 1]),
            Err(EnvelopeError::BlobTooLarge)
        );
    }
}

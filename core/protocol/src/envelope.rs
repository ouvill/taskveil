//! Encrypted record envelope framing contract.

use thiserror::Error;

pub const ENVELOPE_VERSION: u8 = 5;
pub const ENVELOPE_MAGIC: &[u8; 4] = b"TDE5";
pub const ENVELOPE_HEADER_LEN: usize = 4 + 2 + 8;
pub const ENVELOPE_MIN_LEN: usize = ENVELOPE_HEADER_LEN + 24 + 16;
pub const MAX_ENCRYPTED_BLOB_LEN: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeHeader {
    pub suite_id: u16,
    pub key_generation: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeHeaderError {
    #[error("encrypted blob is too short")]
    BlobTooShort,
    #[error("unsupported encrypted blob version")]
    UnsupportedVersion,
    #[error("key generation must be positive")]
    InvalidGeneration,
    #[error("encrypted blob exceeds 64KB limit")]
    BlobTooLarge,
}

pub fn parse_envelope_header(blob: &[u8]) -> Result<EnvelopeHeader, EnvelopeHeaderError> {
    if blob.len() > MAX_ENCRYPTED_BLOB_LEN {
        return Err(EnvelopeHeaderError::BlobTooLarge);
    }
    if blob.len() < ENVELOPE_MIN_LEN {
        return Err(EnvelopeHeaderError::BlobTooShort);
    }
    if &blob[..4] != ENVELOPE_MAGIC {
        return Err(EnvelopeHeaderError::UnsupportedVersion);
    }
    let suite_id = u16::from_be_bytes([blob[4], blob[5]]);
    let key_generation = u64::from_be_bytes(
        blob[6..14]
            .try_into()
            .map_err(|_| EnvelopeHeaderError::BlobTooShort)?,
    );
    if key_generation == 0 {
        return Err(EnvelopeHeaderError::InvalidGeneration);
    }
    Ok(EnvelopeHeader {
        suite_id,
        key_generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(suite_id: u16, key_generation: u64) -> Vec<u8> {
        let mut blob = Vec::with_capacity(ENVELOPE_MIN_LEN);
        blob.extend_from_slice(ENVELOPE_MAGIC);
        blob.extend_from_slice(&suite_id.to_be_bytes());
        blob.extend_from_slice(&key_generation.to_be_bytes());
        blob.resize(ENVELOPE_MIN_LEN, 0);
        blob
    }

    #[test]
    fn canonical_header_preserves_suite_and_generation() {
        assert_eq!(
            parse_envelope_header(&envelope(2, 7)),
            Ok(EnvelopeHeader {
                suite_id: 2,
                key_generation: 7,
            })
        );
    }

    #[test]
    fn framing_rejects_wrong_magic_zero_generation_and_oversized_blob() {
        let mut wrong_magic = envelope(2, 7);
        wrong_magic[..4].copy_from_slice(b"TDE4");
        assert_eq!(
            parse_envelope_header(&wrong_magic),
            Err(EnvelopeHeaderError::UnsupportedVersion)
        );
        assert_eq!(
            parse_envelope_header(&envelope(2, 0)),
            Err(EnvelopeHeaderError::InvalidGeneration)
        );
        assert_eq!(
            parse_envelope_header(&vec![0; MAX_ENCRYPTED_BLOB_LEN + 1]),
            Err(EnvelopeHeaderError::BlobTooLarge)
        );
    }
}

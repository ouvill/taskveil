use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use taskveil_protocol::sync::{StableRecordCursor, SYNC_PROTOCOL_VERSION};
use thiserror::Error;
use uuid::Uuid;

pub const KEY_CURRENT_ID: &str = "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT_ID";
pub const KEY_CURRENT: &str = "TASKVEIL_RESYNC_TOKEN_KEY_CURRENT";
pub const KEY_PREVIOUS_ID: &str = "TASKVEIL_RESYNC_TOKEN_KEY_PREVIOUS_ID";
pub const KEY_PREVIOUS: &str = "TASKVEIL_RESYNC_TOKEN_KEY_PREVIOUS";

const TOKEN_VERSION: u8 = 1;
const TOKEN_DOMAIN: &[u8] = b"taskveil/resync-page-token/v1\0";
const MAX_TOKEN_LEN: usize = 4_096;
/// Resync page chains are resumable for at most 24 hours from their initial
/// server issue time. Derived page and completion tokens inherit this expiry.
pub const MAX_TOKEN_LIFETIME_SECS: i64 = 24 * 60 * 60;
/// Verification tolerates five minutes of deployment clock disagreement.
pub const TOKEN_CLOCK_MARGIN_SECS: i64 = 5 * 60;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum ResyncTokenConfigError {
    #[error("missing resync token configuration variable {0}")]
    Missing(&'static str),
    #[error("invalid resync token configuration variable {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Error)]
pub enum ResyncTokenError {
    #[error("failed to encode resync token claims")]
    Encoding,
    #[error("invalid resync token")]
    Invalid,
}

#[derive(Debug, Clone)]
struct SigningKey {
    id: String,
    material: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct ResyncTokenKeyring {
    current: SigningKey,
    previous: Option<SigningKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncTokenKind {
    Page,
    Completion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResyncTokenClaims {
    pub version: u8,
    pub kind: ResyncTokenKind,
    pub tenant_id: Uuid,
    pub device_id: Uuid,
    pub generation: i64,
    pub base_seq: i64,
    pub cursor: Option<StableRecordCursor>,
    pub protocol_version: u16,
    pub issued_at: i64,
    pub expires_at: i64,
}

impl ResyncTokenClaims {
    pub fn page(
        tenant_id: Uuid,
        device_id: Uuid,
        generation: i64,
        base_seq: i64,
        cursor: Option<StableRecordCursor>,
        now_unix_seconds: i64,
    ) -> Result<Self, ResyncTokenError> {
        let expires_at = now_unix_seconds
            .checked_add(MAX_TOKEN_LIFETIME_SECS)
            .ok_or(ResyncTokenError::Invalid)?;
        Ok(Self {
            version: TOKEN_VERSION,
            kind: ResyncTokenKind::Page,
            tenant_id,
            device_id,
            generation,
            base_seq,
            cursor,
            protocol_version: SYNC_PROTOCOL_VERSION,
            issued_at: now_unix_seconds,
            expires_at,
        })
    }

    pub fn page_from(page: &Self, cursor: Option<StableRecordCursor>) -> Self {
        Self {
            kind: ResyncTokenKind::Page,
            cursor,
            ..page.clone()
        }
    }

    pub fn completion_from(page: &Self, cursor: Option<StableRecordCursor>) -> Self {
        Self {
            kind: ResyncTokenKind::Completion,
            cursor,
            ..page.clone()
        }
    }
}

impl ResyncTokenKeyring {
    pub fn from_string_values(
        lookup: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, ResyncTokenConfigError> {
        let current = SigningKey {
            id: parse_key_id(
                lookup(KEY_CURRENT_ID).ok_or(ResyncTokenConfigError::Missing(KEY_CURRENT_ID))?,
                KEY_CURRENT_ID,
            )?,
            material: parse_key(
                &lookup(KEY_CURRENT).ok_or(ResyncTokenConfigError::Missing(KEY_CURRENT))?,
                KEY_CURRENT,
            )?,
        };
        let previous = match (lookup(KEY_PREVIOUS_ID), lookup(KEY_PREVIOUS)) {
            (None, None) => None,
            (Some(id), Some(material)) => Some(SigningKey {
                id: parse_key_id(id, KEY_PREVIOUS_ID)?,
                material: parse_key(&material, KEY_PREVIOUS)?,
            }),
            (None, Some(_)) => return Err(ResyncTokenConfigError::Missing(KEY_PREVIOUS_ID)),
            (Some(_), None) => return Err(ResyncTokenConfigError::Missing(KEY_PREVIOUS)),
        };
        if previous.as_ref().is_some_and(|previous| {
            previous.id == current.id || previous.material == current.material
        }) {
            return Err(ResyncTokenConfigError::Invalid(KEY_PREVIOUS));
        }
        Ok(Self { current, previous })
    }

    #[cfg(any(test, debug_assertions))]
    pub fn for_tests() -> Self {
        Self {
            current: SigningKey {
                id: "test-current".to_string(),
                material: [0x52; 32],
            },
            previous: None,
        }
    }

    pub fn sign(
        &self,
        claims: &ResyncTokenClaims,
        now_unix_seconds: i64,
    ) -> Result<String, ResyncTokenError> {
        validate_temporal_claims(claims, now_unix_seconds)?;
        let payload = serde_json::to_vec(claims).map_err(|_| ResyncTokenError::Encoding)?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signed = format!("{}.{}", self.current.id, payload);
        let signature = signature(&self.current.material, signed.as_bytes());
        Ok(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    pub fn verify(
        &self,
        token: &str,
        now_unix_seconds: i64,
    ) -> Result<ResyncTokenClaims, ResyncTokenError> {
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return Err(ResyncTokenError::Invalid);
        }
        let mut parts = token.split('.');
        let (Some(key_id), Some(payload), Some(encoded_signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ResyncTokenError::Invalid);
        };
        let key = [&self.current]
            .into_iter()
            .chain(self.previous.iter())
            .find(|key| key.id == key_id)
            .ok_or(ResyncTokenError::Invalid)?;
        let signature = URL_SAFE_NO_PAD
            .decode(encoded_signature)
            .map_err(|_| ResyncTokenError::Invalid)?;
        if URL_SAFE_NO_PAD.encode(&signature) != encoded_signature {
            return Err(ResyncTokenError::Invalid);
        }
        let signed = format!("{key_id}.{payload}");
        let mut mac =
            HmacSha256::new_from_slice(&key.material).map_err(|_| ResyncTokenError::Invalid)?;
        mac.update(TOKEN_DOMAIN);
        mac.update(signed.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| ResyncTokenError::Invalid)?;
        let encoded_payload = payload;
        let payload = URL_SAFE_NO_PAD
            .decode(encoded_payload)
            .map_err(|_| ResyncTokenError::Invalid)?;
        if URL_SAFE_NO_PAD.encode(&payload) != encoded_payload {
            return Err(ResyncTokenError::Invalid);
        }
        let claims: ResyncTokenClaims =
            serde_json::from_slice(&payload).map_err(|_| ResyncTokenError::Invalid)?;
        if claims.version != TOKEN_VERSION
            || claims.protocol_version != SYNC_PROTOCOL_VERSION
            || claims.generation <= 0
            || claims.base_seq < 0
        {
            return Err(ResyncTokenError::Invalid);
        }
        validate_temporal_claims(&claims, now_unix_seconds)?;
        Ok(claims)
    }
}

fn validate_temporal_claims(
    claims: &ResyncTokenClaims,
    now_unix_seconds: i64,
) -> Result<(), ResyncTokenError> {
    let lifetime = claims
        .expires_at
        .checked_sub(claims.issued_at)
        .ok_or(ResyncTokenError::Invalid)?;
    let latest_issued_at = now_unix_seconds
        .checked_add(TOKEN_CLOCK_MARGIN_SECS)
        .ok_or(ResyncTokenError::Invalid)?;
    if claims.issued_at < 0
        || lifetime <= 0
        || lifetime > MAX_TOKEN_LIFETIME_SECS
        || claims.issued_at > latest_issued_at
        || now_unix_seconds >= claims.expires_at
    {
        return Err(ResyncTokenError::Invalid);
    }
    Ok(())
}

fn signature(key: &[u8; 32], value: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts 32-byte keys");
    mac.update(TOKEN_DOMAIN);
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn parse_key(value: &str, variable: &'static str) -> Result<[u8; 32], ResyncTokenConfigError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| ResyncTokenConfigError::Invalid(variable))?;
    decoded
        .try_into()
        .map_err(|_| ResyncTokenConfigError::Invalid(variable))
}

fn parse_key_id(value: String, variable: &'static str) -> Result<String, ResyncTokenConfigError> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ResyncTokenConfigError::Invalid(variable));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn claims() -> ResyncTokenClaims {
        ResyncTokenClaims::page(Uuid::now_v7(), Uuid::now_v7(), 3, 9, None, NOW).unwrap()
    }

    #[test]
    fn signed_token_rejects_tampering_and_wrong_key() {
        let keyring = ResyncTokenKeyring::for_tests();
        let claims = claims();
        let token = keyring.sign(&claims, NOW).unwrap();
        assert_eq!(keyring.verify(&token, NOW).unwrap(), claims);

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'a' { b'b' } else { b'a' };
        assert!(keyring
            .verify(std::str::from_utf8(&tampered).unwrap(), NOW)
            .is_err());
        assert!(keyring.verify(&"x".repeat(MAX_TOKEN_LEN + 1), NOW).is_err());
    }

    #[test]
    fn token_parser_requires_canonical_base64url_tail_bits() {
        let keyring = ResyncTokenKeyring::for_tests();
        let claims = claims();
        let token = keyring.sign(&claims, NOW).unwrap();
        let mut parts = token.split('.').map(str::to_string).collect::<Vec<_>>();
        let last = parts[2].pop().unwrap();
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let index = alphabet.find(last).unwrap();
        assert_eq!(index % 4, 0);
        parts[2].push(alphabet.as_bytes()[index + 1] as char);
        assert!(keyring.verify(&parts.join("."), NOW).is_err());
    }

    #[test]
    fn rotated_keyring_verifies_previous_tokens_and_signs_only_with_current_key() {
        let old_key = STANDARD.encode([0x21; 32]);
        let new_key = STANDARD.encode([0x22; 32]);
        let old = ResyncTokenKeyring::from_string_values(|name| match name {
            KEY_CURRENT_ID => Some("old".to_string()),
            KEY_CURRENT => Some(old_key.clone()),
            _ => None,
        })
        .unwrap();
        let rotated = ResyncTokenKeyring::from_string_values(|name| match name {
            KEY_CURRENT_ID => Some("new".to_string()),
            KEY_CURRENT => Some(new_key.clone()),
            KEY_PREVIOUS_ID => Some("old".to_string()),
            KEY_PREVIOUS => Some(old_key.clone()),
            _ => None,
        })
        .unwrap();
        let claims = claims();

        let old_token = old.sign(&claims, NOW).unwrap();
        assert_eq!(rotated.verify(&old_token, NOW).unwrap(), claims);
        assert!(rotated.sign(&claims, NOW).unwrap().starts_with("new."));
        assert!(rotated.verify(&old_token, claims.expires_at).is_err());
    }

    #[test]
    fn temporal_claims_enforce_24_hour_expiry_and_future_clock_margin() {
        let keyring = ResyncTokenKeyring::for_tests();
        let claims = claims();
        let token = keyring.sign(&claims, NOW).unwrap();
        assert!(keyring.verify(&token, claims.expires_at - 1).is_ok());
        assert!(keyring.verify(&token, claims.expires_at).is_err());

        let future = ResyncTokenClaims::page(
            Uuid::now_v7(),
            Uuid::now_v7(),
            1,
            0,
            None,
            NOW + TOKEN_CLOCK_MARGIN_SECS + 1,
        )
        .unwrap();
        assert!(keyring.sign(&future, NOW).is_err());

        let mut too_long = claims.clone();
        too_long.expires_at += 1;
        assert!(keyring.sign(&too_long, NOW).is_err());
        let mut reversed = claims.clone();
        reversed.expires_at = reversed.issued_at;
        assert!(keyring.sign(&reversed, NOW).is_err());
        let mut negative = claims;
        negative.issued_at = -1;
        assert!(keyring.sign(&negative, NOW).is_err());
    }

    #[test]
    fn page_chain_and_completion_ack_inherit_original_expiry_without_extension() {
        let keyring = ResyncTokenKeyring::for_tests();
        let initial = claims();
        let next = ResyncTokenClaims::page_from(
            &initial,
            Some(StableRecordCursor {
                collection: taskveil_protocol::sync::SyncCollection::Tasks,
                record_id: Uuid::now_v7(),
            }),
        );
        let completion = ResyncTokenClaims::completion_from(&next, next.cursor.clone());
        assert_eq!(next.issued_at, initial.issued_at);
        assert_eq!(next.expires_at, initial.expires_at);
        assert_eq!(completion.issued_at, initial.issued_at);
        assert_eq!(completion.expires_at, initial.expires_at);

        let next_token = keyring.sign(&next, NOW + 60).unwrap();
        assert_eq!(
            keyring.verify(&next_token, NOW + 60).unwrap().expires_at,
            initial.expires_at
        );
        assert_eq!(
            keyring.verify(&next_token, NOW + 60).unwrap(),
            keyring.verify(&next_token, NOW + 60).unwrap(),
            "a lost response replays the same still-bounded token"
        );
        assert!(keyring.sign(&completion, initial.expires_at).is_err());
    }
}

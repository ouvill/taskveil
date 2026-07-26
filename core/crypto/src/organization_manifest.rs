//! Account-root signed Organization key-generation manifests.

use sha2::{Digest, Sha256};

use crate::{
    key_manifest::{KeyManifest, KeyManifestError},
    organization::{
        sign_account_root_payload, verify_account_root_payload, AccountRootPrivateKeys,
        AccountRootPublicKeys, AccountRootSignature, OrganizationCryptoError,
        ED25519_SIGNATURE_LEN, ML_DSA_65_SIGNATURE_LEN, ROOT_FINGERPRINT_LEN,
    },
};

const ORGANIZATION_MANIFEST_MAGIC: &[u8; 4] = b"TOM1";
const ORGANIZATION_MANIFEST_DOMAIN: &[u8] = b"taskveil/organization-key-manifest/v1";

#[derive(Debug, thiserror::Error)]
pub enum OrganizationManifestError {
    #[error("invalid organization manifest encoding")]
    InvalidEncoding,
    #[error("key manifest error")]
    KeyManifest(#[from] KeyManifestError),
    #[error("organization signature error")]
    Signature(#[from] OrganizationCryptoError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrganizationKeyManifest {
    pub manifest: KeyManifest,
    pub root_fingerprint: [u8; ROOT_FINGERPRINT_LEN],
    pub signature: AccountRootSignature,
}

impl OrganizationKeyManifest {
    pub fn sign(
        manifest: KeyManifest,
        root_private: &AccountRootPrivateKeys,
        root_public: &AccountRootPublicKeys,
    ) -> Result<Self, OrganizationManifestError> {
        if manifest.authenticator != [0u8; 32] {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        let root_fingerprint = root_public.fingerprint()?;
        let transcript = manifest_transcript(&manifest, &root_fingerprint)?;
        let signature = sign_account_root_payload(root_private, root_public, &transcript)?;
        Ok(Self {
            manifest,
            root_fingerprint,
            signature,
        })
    }

    pub fn verify(
        &self,
        root_public: &AccountRootPublicKeys,
    ) -> Result<(), OrganizationManifestError> {
        if self.manifest.authenticator != [0u8; 32]
            || self.root_fingerprint != root_public.fingerprint()?
        {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        verify_account_root_payload(
            root_public,
            &manifest_transcript(&self.manifest, &self.root_fingerprint)?,
            &self.signature,
        )?;
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, OrganizationManifestError> {
        if self.signature.ml_dsa_65_signature.len() != ML_DSA_65_SIGNATURE_LEN {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        let payload = self.manifest.canonical_payload()?;
        let mut output = Vec::with_capacity(
            4 + 4
                + payload.len()
                + ROOT_FINGERPRINT_LEN
                + ED25519_SIGNATURE_LEN
                + ML_DSA_65_SIGNATURE_LEN,
        );
        output.extend_from_slice(ORGANIZATION_MANIFEST_MAGIC);
        output.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| OrganizationManifestError::InvalidEncoding)?
                .to_be_bytes(),
        );
        output.extend_from_slice(&payload);
        output.extend_from_slice(&self.root_fingerprint);
        output.extend_from_slice(&self.signature.ed25519_signature);
        output.extend_from_slice(&self.signature.ml_dsa_65_signature);
        Ok(output)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, OrganizationManifestError> {
        const TRAILER: usize =
            ROOT_FINGERPRINT_LEN + ED25519_SIGNATURE_LEN + ML_DSA_65_SIGNATURE_LEN;
        if bytes.len() <= 8 + TRAILER || &bytes[..4] != ORGANIZATION_MANIFEST_MAGIC {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        let payload_len = usize::try_from(u32::from_be_bytes(
            bytes[4..8]
                .try_into()
                .map_err(|_| OrganizationManifestError::InvalidEncoding)?,
        ))
        .map_err(|_| OrganizationManifestError::InvalidEncoding)?;
        let payload_end = 8usize
            .checked_add(payload_len)
            .ok_or(OrganizationManifestError::InvalidEncoding)?;
        let encoded_len = payload_end
            .checked_add(TRAILER)
            .ok_or(OrganizationManifestError::InvalidEncoding)?;
        if bytes.len() != encoded_len {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        let mut personal_shape = bytes[8..payload_end].to_vec();
        personal_shape.extend_from_slice(&[0u8; 32]);
        let manifest = KeyManifest::from_authenticated_bytes(&personal_shape)?;
        if manifest.authenticator != [0u8; 32] {
            return Err(OrganizationManifestError::InvalidEncoding);
        }
        let root_end = payload_end + ROOT_FINGERPRINT_LEN;
        let ed_end = root_end + ED25519_SIGNATURE_LEN;
        Ok(Self {
            manifest,
            root_fingerprint: bytes[payload_end..root_end]
                .try_into()
                .map_err(|_| OrganizationManifestError::InvalidEncoding)?,
            signature: AccountRootSignature {
                ed25519_signature: bytes[root_end..ed_end]
                    .try_into()
                    .map_err(|_| OrganizationManifestError::InvalidEncoding)?,
                ml_dsa_65_signature: bytes[ed_end..].to_vec(),
            },
        })
    }

    pub fn authenticated_hash(&self) -> Result<[u8; 32], OrganizationManifestError> {
        Ok(Sha256::digest(self.encode()?).into())
    }
}

fn manifest_transcript(
    manifest: &KeyManifest,
    root_fingerprint: &[u8; ROOT_FINGERPRINT_LEN],
) -> Result<Vec<u8>, OrganizationManifestError> {
    let payload = manifest.canonical_payload()?;
    let mut output = Vec::with_capacity(
        ORGANIZATION_MANIFEST_DOMAIN.len() + ROOT_FINGERPRINT_LEN + 4 + payload.len(),
    );
    output.extend_from_slice(ORGANIZATION_MANIFEST_DOMAIN);
    output.extend_from_slice(root_fingerprint);
    output.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(|_| OrganizationManifestError::InvalidEncoding)?
            .to_be_bytes(),
    );
    output.extend_from_slice(&payload);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use taskveil_protocol::RotationStatus;
    use uuid::Uuid;

    use super::*;
    use crate::organization::generate_account_root;

    #[test]
    fn manifest_rejects_mutation_and_root_substitution() {
        let root = generate_account_root(Uuid::now_v7()).unwrap();
        let manifest = KeyManifest::organization_unsigned(
            Uuid::now_v7(),
            3,
            RotationStatus::Active,
            3,
            [0x11; 32],
            vec![[0x22; 32]],
        )
        .unwrap();
        let signed = OrganizationKeyManifest::sign(manifest, &root.private, &root.public).unwrap();
        let decoded = OrganizationKeyManifest::decode(&signed.encode().unwrap()).unwrap();
        decoded.verify(&root.public).unwrap();

        let mut recipient_added = decoded.clone();
        recipient_added
            .manifest
            .recipient_fingerprints
            .push([0x33; 32]);
        assert!(recipient_added.verify(&root.public).is_err());

        let substituted_root = generate_account_root(root.public.user_id).unwrap();
        assert!(decoded.verify(&substituted_root.public).is_err());
    }

    #[test]
    fn manifest_rejects_overflowing_payload_length() {
        let mut encoded = vec![0; 8 + 32 + 64 + 3_309 + 1];
        encoded[..4].copy_from_slice(b"TOM1");
        encoded[4..8].copy_from_slice(&u32::MAX.to_be_bytes());

        assert!(OrganizationKeyManifest::decode(&encoded).is_err());
    }
}

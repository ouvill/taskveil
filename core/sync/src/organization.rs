//! Organization verification and per-device recipient wire contracts.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use taskveil_crypto::organization::AccountRootPublicKeys;
pub use taskveil_crypto::{OrganizationKeyManifest, OrganizationManifestError};
use uuid::Uuid;

#[cfg(test)]
use crate::KeyManifest;
use crate::{account::ActiveKeyBundleDto, RotationStatus};
pub use taskveil_protocol::organization::{
    OrganizationDeviceDto, OrganizationDeviceRevocationRequest, OrganizationDeviceRosterDto,
    OrganizationInviteRequest, OrganizationMemberResponse, OrganizationSafetyConfirmRequest,
    OrganizationSafetyResponse, RecipientPackageRequest, RecipientPackageResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum OrganizationBundleError {
    #[error("invalid organization bundle encoding")]
    InvalidEncoding,
    #[error("organization bundle generation was replayed")]
    GenerationReplay,
    #[error("organization manifest error")]
    Manifest(#[from] OrganizationManifestError),
}

pub fn verify_organization_active_bundle(
    bundle: &ActiveKeyBundleDto,
    tenant_id: Uuid,
    minimum_generation: u64,
    owner_root: &AccountRootPublicKeys,
    recipient_certificate: &taskveil_crypto::organization::DeviceCertificate,
    expected_recipient_fingerprints: &[[u8; 32]],
) -> Result<(), OrganizationBundleError> {
    if bundle.suite_id != taskveil_crypto::CRYPTO_SUITE_ID
        || bundle.generation == 0
        || bundle.generation < minimum_generation
        || !bundle.wrapped_tenant_root_dek.is_empty()
    {
        return if bundle.generation < minimum_generation {
            Err(OrganizationBundleError::GenerationReplay)
        } else {
            Err(OrganizationBundleError::InvalidEncoding)
        };
    }
    let recipient_fingerprint = recipient_certificate
        .recipient_key_fingerprint()
        .map_err(OrganizationManifestError::Signature)?;
    let mut expected_recipients = expected_recipient_fingerprints.to_vec();
    expected_recipients.sort_unstable();
    let original_len = expected_recipients.len();
    expected_recipients.dedup();
    if expected_recipients.is_empty() || expected_recipients.len() != original_len {
        return Err(OrganizationBundleError::InvalidEncoding);
    }
    verify_active_manifest(
        &bundle.signed_manifest,
        owner_root,
        tenant_id,
        bundle.generation,
        recipient_fingerprint,
        &expected_recipients,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_active_manifest(
    encoded: &str,
    owner_root: &AccountRootPublicKeys,
    tenant_id: Uuid,
    generation: u64,
    recipient_fingerprint: [u8; 32],
    expected_recipients: &[[u8; 32]],
) -> Result<(), OrganizationBundleError> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| OrganizationBundleError::InvalidEncoding)?;
    let signed = OrganizationKeyManifest::decode(&bytes)?;
    signed.verify(owner_root)?;
    if signed.manifest.tenant_id != tenant_id
        || signed.manifest.generation != generation
        || signed.manifest.status != RotationStatus::Active
        || signed.manifest.minimum_write_generation != generation
        || signed
            .manifest
            .recipient_fingerprints
            .binary_search(&recipient_fingerprint)
            .is_err()
        || signed.manifest.recipient_fingerprints != expected_recipients
    {
        return Err(OrganizationBundleError::InvalidEncoding);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskveil_crypto::organization::{
        generate_account_root, generate_device_keys, issue_device_certificate,
    };

    #[test]
    fn organization_manifest_rejects_recipient_addition_generation_replay_and_root_substitution() {
        let tenant_id = Uuid::now_v7();
        let root = generate_account_root(Uuid::now_v7()).unwrap();
        let manifest = KeyManifest::organization_unsigned(
            tenant_id,
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

        let mut replayed_generation = decoded.clone();
        replayed_generation.manifest.generation = 4;
        replayed_generation.manifest.minimum_write_generation = 4;
        assert!(replayed_generation.verify(&root.public).is_err());

        let substituted_root = generate_account_root(root.public.user_id).unwrap();
        assert!(decoded.verify(&substituted_root.public).is_err());
    }

    #[test]
    fn organization_manifest_rejects_overflowing_payload_length() {
        let mut encoded = vec![0; 8 + 32 + 64 + 3_309 + 1];
        encoded[..4].copy_from_slice(b"TOM1");
        encoded[4..8].copy_from_slice(&u32::MAX.to_be_bytes());

        assert!(OrganizationKeyManifest::decode(&encoded).is_err());
    }

    #[test]
    fn active_bundle_requires_current_generation_signature_and_recipient_membership() {
        let tenant_id = Uuid::now_v7();
        let user_id = Uuid::now_v7();
        let root = generate_account_root(user_id).unwrap();
        let device = generate_device_keys().unwrap();
        let certificate = issue_device_certificate(
            &root.private,
            &root.public,
            Uuid::now_v7(),
            &device,
            1_000,
            10_000,
        )
        .unwrap();
        let recipient = certificate.recipient_key_fingerprint().unwrap();
        let manifest = KeyManifest::organization_unsigned(
            tenant_id,
            3,
            RotationStatus::Active,
            3,
            [0x11; 32],
            vec![recipient],
        )
        .unwrap();
        let signed = OrganizationKeyManifest::sign(manifest, &root.private, &root.public).unwrap();
        let bundle = ActiveKeyBundleDto {
            suite_id: taskveil_crypto::CRYPTO_SUITE_ID,
            generation: 3,
            wrapped_tenant_root_dek: String::new(),
            signed_manifest: STANDARD.encode(signed.encode().unwrap()),
            migrating_generations: Vec::new(),
        };
        verify_organization_active_bundle(
            &bundle,
            tenant_id,
            3,
            &root.public,
            &certificate,
            &[recipient],
        )
        .unwrap();
        assert!(matches!(
            verify_organization_active_bundle(
                &bundle,
                tenant_id,
                3,
                &root.public,
                &certificate,
                &[recipient, [0x99; 32]],
            ),
            Err(OrganizationBundleError::InvalidEncoding)
        ));
        assert!(matches!(
            verify_organization_active_bundle(
                &bundle,
                tenant_id,
                4,
                &root.public,
                &certificate,
                &[recipient],
            ),
            Err(OrganizationBundleError::GenerationReplay)
        ));

        let outsider = generate_device_keys().unwrap();
        let outsider_certificate = issue_device_certificate(
            &root.private,
            &root.public,
            Uuid::now_v7(),
            &outsider,
            1_000,
            10_000,
        )
        .unwrap();
        assert!(matches!(
            verify_organization_active_bundle(
                &bundle,
                tenant_id,
                3,
                &root.public,
                &outsider_certificate,
                &[recipient],
            ),
            Err(OrganizationBundleError::InvalidEncoding)
        ));
    }
}

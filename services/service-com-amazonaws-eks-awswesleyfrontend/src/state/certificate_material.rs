// SPDX-License-Identifier: CC0-1.0

//! The X.509 material a cluster's certificate authorities are made of.
//!
//! `Certificate$data` is "the Base64-encoded certificate data required to
//! communicate with your cluster. Add this to the `certificate-authority-data`
//! section of the kubeconfig file for your cluster"
//! (`smithy_query(query="shape", shape="Certificate")`), and a
//! `CertificateAuthority$data` is the same thing for one certificate authority of
//! the cluster — so what the member carries has to be a certificate a client can
//! decode, parse and install as a trust anchor, not text in the shape of one.
//! Every client built from a cluster's own kubeconfig reads it that way.
//!
//! A child module of [`super`] rather than more of `state.rs`: issuing material
//! is one concern, and it is the only part of this service that holds a private
//! key at all — the key is generated here, used to sign the certificate, and
//! dropped. Nothing stores or reports it: no shape of this service has a member
//! for a private key.

use super::*;

/// The subject common name a cluster's certificate authority is issued under.
///
/// Kubernetes names a cluster's own root of trust `kubernetes`, which is the
/// subject every client that inspects the CA of a cluster sees — so a certificate
/// authority of this service is issued under the same name rather than under a
/// name this emulator invents.
pub const CERTIFICATE_AUTHORITY_COMMON_NAME: &str = "kubernetes";

/// A certificate authority's `data`: the Base64 of the PEM-encoded X.509
/// certificate it signs with, over the validity period the record reports.
///
/// A REAL self-signed certificate authority certificate, issued through the
/// maintained generator this tree already uses for certificate material: it is
/// marked as a CA and carries the key usages a CA signs certificates and
/// revocation lists with, so a client that Base64-decodes the member gets a PEM
/// block it can parse and install as a trust anchor — which is what a kubeconfig
/// built from `DescribeCluster` does with it.
///
/// The private half is generated here and dropped: the cluster's control plane is
/// what would sign with it, and no shape of this service declares a member for a
/// private key, so nothing this service stores or reports can present it.
///
/// `not_before`/`not_after` are the record's own validity period
/// ([`CertificateAuthorityRecord::not_before`]), so the window the certificate
/// bears and the one `DescribeCertificateAuthority` reports are one value. The
/// material is issued ONCE, when the certificate authority is created, and stored
/// on the record; no read re-issues it, so every read reports the same
/// certificate.
///
/// `None` when the material does not issue, which the operation that asked for it
/// reports rather than storing a certificate authority with nothing in it.
pub fn mint_certificate_authority_data(
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<String> {
    match issue_certificate_authority_pem(not_before, not_after) {
        Some(pem) => Some(::aws_smithy_types::base64::encode(pem)),
        None => {
            // The caller answers its modelled server error for this, which says
            // nothing about WHY: the generator's own failure is worth a line so
            // the path is diagnosable if it ever fires.
            ::tracing::warn!("the certificate authority's certificate could not be issued");
            None
        }
    }
}

/// The PEM-encoded certificate itself, which [`mint_certificate_authority_data`]
/// is the Base64 of.
fn issue_certificate_authority_pem(
    not_before: SystemTime,
    not_after: SystemTime,
) -> Option<String> {
    let key = ::rcgen::KeyPair::generate().ok()?;
    // No subject alternative name: a certificate authority certifies the names its
    // issued certificates carry, it does not serve one itself.
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(
        ::rcgen::DnType::CommonName,
        CERTIFICATE_AUTHORITY_COMMON_NAME,
    );
    params.distinguished_name = subject;
    params.not_before = offset_date_time(not_before)?;
    params.not_after = offset_date_time(not_after)?;
    params.is_ca = ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        ::rcgen::KeyUsagePurpose::KeyCertSign,
        ::rcgen::KeyUsagePurpose::CrlSign,
        ::rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    Some(params.self_signed(&key).ok()?.pem())
}

/// `at` as the date-time type the certificate generator takes a validity bound
/// in, at the one-second resolution an X.509 validity period is stated at.
///
/// `None` for an instant outside the range that type represents, which is the
/// same answer the caller gives for material that does not issue.
fn offset_date_time(at: SystemTime) -> Option<::time::OffsetDateTime> {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())?;
    ::time::OffsetDateTime::from_unix_timestamp(seconds).ok()
}

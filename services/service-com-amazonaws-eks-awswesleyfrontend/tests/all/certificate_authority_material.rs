// SPDX-License-Identifier: CC0-1.0

//! What a cluster and its certificate authorities report as certificate data is a
//! certificate a client can actually use.
//!
//! The RULE under test: `Certificate$data` is "the Base64-encoded certificate data
//! required to communicate with your cluster. Add this to the
//! `certificate-authority-data` section of the `kubeconfig` file for your cluster"
//! (`smithy_query(query="shape", shape="Certificate")`), and
//! `CertificateAuthority$data` is the same for one certificate authority of the
//! cluster — so Base64-decoding either yields a PEM-encoded X.509 certificate
//! authority certificate, valid over exactly the period the certificate authority
//! reports, which a verifying TLS client can parse and install as a trust anchor.
//!
//! This suite reads the member the way such a client does — decode the Base64,
//! parse the PEM, then read the certificate's own fields — rather than matching on
//! its text, so nothing but a real certificate passes it.
//!
//! The rows differ along what the rule generalises over: which certificate
//! authority of the cluster (the one Amazon EKS provisions with it, which is also
//! what the cluster reports as its own `certificateAuthority.data`, and a successor
//! the caller appends), how many clusters are in play, and the region the request is
//! signed for — plus repeated reads, because material is issued once and stored, so
//! two reads of one certificate authority report the same certificate.
//!
//! The wire harness and the certificate-authority drivers are the shared ones
//! (`tests/all/wire.rs`, `tests/all/certificate_authority.rs`).

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CA_DISTRIBUTION_DELAY, CERTIFICATE_AUTHORITY_COMMON_NAME, CLUSTER_ACTIVATION_DELAY,
};
use x509_cert::der::oid::AssociatedOid;
use x509_cert::der::{Decode, DecodePem, Encode};

use crate::certificate_authority::{
    append_authority, create_cluster, describe_authority, describe_cluster, epoch, settle,
    str_member, REGION,
};
use crate::wire::emulator;

/// The second region a row is signed for, to show the rule holds wherever the
/// cluster lives.
const OTHER_REGION: &str = "eu-central-1";

/// What a verifying client gets out of the member: the decoded PEM, and the
/// certificate it parses to.
struct Material {
    pem: String,
    certificate: x509_cert::Certificate,
}

/// Read `data` the way a TLS client built from a kubeconfig does: Base64-decode the
/// member, then parse the PEM block as an X.509 certificate.
fn material(case: &str, data: &str) -> Material {
    let decoded = aws_smithy_types::base64::decode(data).unwrap_or_else(|error| {
        panic!("case {case:?}: the member must be Base64, got {data:?} ({error})")
    });
    let pem = String::from_utf8(decoded).unwrap_or_else(|error| {
        panic!("case {case:?}: the decoded certificate data must be text ({error})")
    });
    assert!(
        pem.starts_with("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: the decoded data must be a PEM certificate block, got {pem:?}"
    );
    let certificate = x509_cert::Certificate::from_pem(pem.as_bytes()).unwrap_or_else(|error| {
        panic!("case {case:?}: the PEM block must parse as an X.509 certificate ({error}): {pem}")
    });
    Material { pem, certificate }
}

/// The certificate extension of type `T`, decoded, or `None` when the certificate
/// carries none of that type.
fn extension<T>(certificate: &x509_cert::Certificate) -> Option<T>
where
    T: AssociatedOid + for<'der> Decode<'der>,
{
    certificate
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|extension| extension.extn_id == T::OID)
        .and_then(|extension| T::from_der(extension.extn_value.as_bytes()).ok())
}

/// Assert everything the rule requires of one certificate authority's material:
/// that it is a certificate authority certificate, issued under the name a
/// Kubernetes cluster's root of trust carries, over the validity period the
/// certificate authority reports.
fn assert_usable_authority_certificate(
    case: &str,
    data: &str,
    reported_not_before: f64,
    reported_not_after: f64,
) -> Material {
    let material = material(case, data);
    let tbs = &material.certificate.tbs_certificate;

    let subject = tbs.subject.to_string();
    assert!(
        subject.contains(CERTIFICATE_AUTHORITY_COMMON_NAME),
        "case {case:?}: a cluster's root of trust is issued under the common name \
         {CERTIFICATE_AUTHORITY_COMMON_NAME:?}, got {subject:?}"
    );
    assert_eq!(
        subject,
        tbs.issuer.to_string(),
        "case {case:?}: a cluster's certificate authority is self-signed, so its issuer is \
         its own subject, got issuer {:?}",
        tbs.issuer.to_string()
    );

    let basic_constraints: x509_cert::ext::pkix::BasicConstraints =
        extension(&material.certificate).unwrap_or_else(|| {
            panic!("case {case:?}: a certificate authority certificate carries basic constraints")
        });
    assert!(
        basic_constraints.ca,
        "case {case:?}: the certificate must be marked as a certificate authority, got \
         {basic_constraints:?}"
    );
    let key_usage: x509_cert::ext::pkix::KeyUsage = extension(&material.certificate)
        .unwrap_or_else(|| panic!("case {case:?}: the certificate carries its key usages"));
    assert!(
        key_usage.key_cert_sign(),
        "case {case:?}: a certificate authority signs certificates, got {key_usage:?}"
    );

    // The window the certificate bears is the one the certificate authority reports,
    // to the whole second an X.509 validity period is stated at.
    let bearing = |at: x509_cert::time::Time| at.to_unix_duration().as_secs() as f64;
    assert!(
        (bearing(tbs.validity.not_before) - reported_not_before).abs() <= 1.0,
        "case {case:?}: the certificate must begin where the CA reports it does \
         ({reported_not_before}), got {}",
        bearing(tbs.validity.not_before)
    );
    assert!(
        (bearing(tbs.validity.not_after) - reported_not_after).abs() <= 1.0,
        "case {case:?}: the certificate must expire where the CA reports it does \
         ({reported_not_after}), got {}",
        bearing(tbs.validity.not_after)
    );
    material
}

/// The certificate's public key, as the bytes two certificates are compared by.
fn public_key(material: &Material) -> Vec<u8> {
    material
        .certificate
        .tbs_certificate
        .subject_public_key_info
        .to_der()
        .expect("a parsed certificate re-encodes its public key")
}

// ---------------------------------------------------------------------------
// RULE: every certificate authority of a cluster — the one Amazon EKS provisions
// with it and a successor the caller appends — reports a usable certificate, and
// the cluster's own `certificateAuthority.data` is its signer's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_and_its_authorities_report_usable_certificates() {
    let router = emulator();

    // Rows: two clusters in one region and a third in another, so the rule is shown
    // to hold per cluster and per region rather than once.
    let clusters = [
        ("the first cluster", REGION, "ca-material-one"),
        ("the second cluster", REGION, "ca-material-two"),
        ("a cluster in another region", OTHER_REGION, "ca-material-one"),
    ];
    for (case, region, cluster) in clusters {
        let created = create_cluster(&router, region, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    // Every certificate this suite reads, so no two certificate authorities are
    // shown to hand out the same key material.
    let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();

    for (case, region, cluster) in clusters {
        let described = describe_cluster(&router, region, cluster).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeCluster, got {described:?}"
        );
        let cluster_data = str_member(
            case,
            "cluster.certificateAuthority.data",
            &described.body["cluster"]["certificateAuthority"]["data"],
        );
        let signer_id = str_member(
            case,
            "cluster.certificateAuthority.active.id",
            &described.body["cluster"]["certificateAuthority"]["active"]["id"],
        );

        // The signer, as `DescribeCertificateAuthority` reports it: the validity
        // period the certificate must bear comes from the record, not from this test.
        let signer = describe_authority(&router, region, cluster, &signer_id).await;
        assert_eq!(
            signer.status, 200,
            "case {case:?}: the cluster's own CA must be readable, got {signer:?}"
        );
        let authority = &signer.body["certificateAuthority"];
        let signer_data = str_member(case, "certificateAuthority.data", &authority["data"]);
        assert_eq!(
            signer_data, cluster_data,
            "case {case:?}: the cluster reports its signer's certificate, got {signer_data:?}"
        );
        let not_before = epoch(case, "validity.notBefore", &authority["validity"]["notBefore"]);
        let not_after = epoch(case, "validity.notAfter", &authority["validity"]["notAfter"]);
        let signer_material = assert_usable_authority_certificate(
            &format!("{case}: the provisioned signer"),
            &signer_data,
            not_before,
            not_after,
        );
        assert!(
            keys.insert(public_key(&signer_material)),
            "case {case:?}: each certificate authority issues its own key material, but one \
             was reported twice"
        );

        // A second read reports the same certificate: the material is issued once,
        // when the certificate authority is created, and stored.
        let again = describe_authority(&router, region, cluster, &signer_id).await;
        assert_eq!(
            again.body["certificateAuthority"]["data"],
            serde_json::json!(signer_data),
            "case {case:?}: a second read must report the same certificate, got {:?}",
            again.body["certificateAuthority"]["data"]
        );

        // A successor the caller appends carries a certificate of its own, under the
        // same rule.
        let appended = append_authority(&router, region, cluster, None).await;
        assert_eq!(
            appended.status, 200,
            "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
        );
        let successor_id = str_member(
            case,
            "certificateAuthority.id",
            &appended.body["certificateAuthority"]["id"],
        );
        settle(CA_DISTRIBUTION_DELAY).await;
        let successor = describe_authority(&router, region, cluster, &successor_id).await;
        assert_eq!(
            successor.status, 200,
            "case {case:?}: the appended CA must be readable, got {successor:?}"
        );
        let successor_authority = &successor.body["certificateAuthority"];
        let successor_data =
            str_member(case, "certificateAuthority.data", &successor_authority["data"]);
        let successor_material = assert_usable_authority_certificate(
            &format!("{case}: the appended successor"),
            &successor_data,
            epoch(
                case,
                "validity.notBefore",
                &successor_authority["validity"]["notBefore"],
            ),
            epoch(
                case,
                "validity.notAfter",
                &successor_authority["validity"]["notAfter"],
            ),
        );
        assert!(
            keys.insert(public_key(&successor_material)),
            "case {case:?}: an appended CA issues its own key material, but one was reported \
             twice"
        );
        assert_ne!(
            successor_material.pem, signer_material.pem,
            "case {case:?}: a successor's certificate is not the signer's"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The ONE certificate a private CA signs before it holds a certificate of its own.
//!
//! The rule under test: a private CA signs with its own key, so it issues only while it is
//! `ACTIVE` — except for the certificate that MAKES it active, a root CA's self-signed
//! certificate. That is the documented procedure for creating a root CA (create the CA,
//! read its certification request, issue it under the root CA certificate template, import
//! what comes back), so `IssueCertificate` is answered for a CA still waiting for its
//! certificate when, and only when, all four hold: the CA is a `ROOT`, it is in
//! `PENDING_CERTIFICATE`, the request names the root CA certificate template, and the
//! certification request presented is the CA's OWN. Anything else asked of a CA outside
//! `ACTIVE` is `InvalidStateException`.
//!
//! The rows differ along each of those four conditions in turn, and the control rows are
//! the issuances a CA in `ACTIVE` answers. Every call goes over the wire through the
//! harness in [`super::private_ca_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::ISSUANCE_INVALID_STATE_MESSAGE;

use crate::private_ca_lifecycle::{
    blob, root_ca_request, string, wait_for_the_clock, Emulator,
};
use crate::resource_policy::error_message;

/// The template that produces a ROOT CA's own certificate.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// The template that produces an end-entity (leaf) certificate, which is what a CA issues
/// once it is active.
const END_ENTITY_TEMPLATE: &str = "arn:aws:acm-pca:::template/EndEntityCertificate/V1";

/// A CA of `kind` with `common_name` as its subject, waited past the transition its
/// creation owes so its certification request is available, and that request.
async fn pending_ca(emulator: &Emulator, case: &str, common_name: &str, kind: &str) -> (String, String) {
    let mut body = root_ca_request(common_name);
    body["CertificateAuthorityType"] = json!(kind);
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    // A test may wait for a due instant; a handler may not.
    wait_for_the_clock();
    let reported = emulator
        .ok(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    let csr = string(&reported, "Csr", case);
    (arn, csr)
}

/// An `IssueCertificate` request body for `ca_arn` over `csr`, under `template`.
fn issuance(ca_arn: &str, csr: &str, template: Option<&str>) -> Value {
    let mut body = json!({
        "CertificateAuthorityArn": ca_arn,
        "Csr": blob(csr),
        "SigningAlgorithm": "SHA256WITHRSA",
        "Validity": {"Type": "DAYS", "Value": 3650},
    });
    if let Some(template) = template {
        body["TemplateArn"] = json!(template);
    }
    body
}

/// A CA waiting for its certificate signs its OWN certification request under the root CA
/// certificate template, and nothing else: each condition the exception rests on, dropped
/// in turn, is the state refusal.
#[tokio::test]
async fn only_a_root_cas_own_request_is_signed_before_it_holds_a_certificate() {
    let emulator = Emulator::new();

    let (root, root_csr) = pending_ca(&emulator, "a pending root CA", "root-self", "ROOT").await;
    let (subordinate, subordinate_csr) = pending_ca(
        &emulator,
        "a pending subordinate CA",
        "sub-self",
        "SUBORDINATE",
    )
    .await;

    // Each row drops exactly one of the four conditions the exception rests on.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the root CA's own request under the end-entity template",
            issuance(&root, &root_csr, Some(END_ENTITY_TEMPLATE)),
        ),
        (
            "the root CA's own request under no template at all",
            issuance(&root, &root_csr, None),
        ),
        (
            "another CA's request under the root CA template",
            issuance(&root, &subordinate_csr, Some(ROOT_CA_TEMPLATE)),
        ),
        (
            "a subordinate CA signing its own request under the root CA template",
            issuance(&subordinate, &subordinate_csr, Some(ROOT_CA_TEMPLATE)),
        ),
    ];

    for (case, body) in &rows {
        let refused = emulator
            .refused(case, "IssueCertificate", body, "InvalidStateException")
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            message, ISSUANCE_INVALID_STATE_MESSAGE,
            "case {case:?}: expected the service's issuance state sentence, got {message:?}"
        );
    }

    // And the one request that satisfies all four is signed — which is what makes the CA
    // active: the certificate it answers with is the one the import then holds.
    let case = "the root CA's own request under the root CA template";
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &issuance(&root, &root_csr, Some(ROOT_CA_TEMPLATE)),
        )
        .await;
    let certificate_arn = string(&issued, "CertificateArn", case);
    assert!(
        certificate_arn.starts_with(&format!("{root}/certificate/")),
        "case {case:?}: expected a certificate ARN of {root:?}, got {certificate_arn:?}"
    );
    // Issuance completes on the clock, as every other issuance of this service does.
    wait_for_the_clock();
    let served = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": root,
                "CertificateArn": certificate_arn,
            }),
        )
        .await;
    let certificate = string(&served, "Certificate", case);
    assert!(
        certificate.contains("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: expected a PEM-framed certificate, got {certificate:?}"
    );

    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": root,
                "Certificate": blob(&certificate),
            }),
        )
        .await;
    let described = emulator
        .ok(
            case,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": root}),
        )
        .await;
    let status = string(
        described
            .get("CertificateAuthority")
            .unwrap_or(&Value::Null),
        "Status",
        case,
    );
    assert_eq!(
        status, "ACTIVE",
        "case {case:?}: expected the imported certificate to activate the CA, got {status:?}"
    );

    // The control: an ACTIVE CA signs a request that is not its own, under the template a
    // leaf is issued with — the issuance the exception above is NOT about. Its validity is
    // shorter than the root certificate's, because a leaf may not outlive its issuer
    // (`IssueCertificateRequest$Validity`).
    let case = "an active CA signing another request";
    let mut leaf_request = issuance(&root, &subordinate_csr, Some(END_ENTITY_TEMPLATE));
    leaf_request["Validity"] = json!({"Type": "DAYS", "Value": 30});
    let leaf = emulator.ok(case, "IssueCertificate", &leaf_request).await;
    let leaf_arn = string(&leaf, "CertificateArn", case);
    assert!(
        leaf_arn.starts_with(&format!("{root}/certificate/")),
        "case {case:?}: expected a certificate ARN of {root:?}, got {leaf_arn:?}"
    );

    // And the CA that was never activated still signs nothing, however long it waits: its
    // own request under the root template is the one row it could ever answer, and it is a
    // subordinate.
    let case = "the subordinate CA after the root was activated";
    let refused = emulator
        .refused(
            case,
            "IssueCertificate",
            &issuance(&subordinate, &root_csr, Some(END_ENTITY_TEMPLATE)),
            "InvalidStateException",
        )
        .await;
    let message = error_message(&refused, case);
    assert_eq!(
        message, ISSUANCE_INVALID_STATE_MESSAGE,
        "case {case:?}: expected the service's issuance state sentence, got {message:?}"
    );
}

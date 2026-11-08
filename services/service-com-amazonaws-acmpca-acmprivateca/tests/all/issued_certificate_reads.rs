// SPDX-License-Identifier: CC0-1.0

//! Which refusal `GetCertificate` answers for a certificate it cannot serve.
//!
//! The rule under test: the three refusals say three different things, and a client branches
//! on them.
//!
//! * A CA that holds no certificate of its own has not been in a state to issue
//!   certificates, so a certificate ARN it has nothing for is the STATE refusal
//!   (`InvalidStateException`), naming the authority — a client polling such a CA would
//!   otherwise retry for a certificate that is never coming.
//! * A CA that does hold one, asked for a certificate it did not issue, is the absence of
//!   that CERTIFICATE (`ResourceNotFoundException`).
//! * `RequestInProgressException` says one thing only: the certificate asked for is still
//!   being minted, so the same request later succeeds.
//!
//! The rows span the statuses a CA passes through and both kinds of certificate ARN — one
//! the CA issued and one it did not.

use serde_json::json;
use service_com_amazonaws_acmpca_acmprivateca::state::not_in_state_to_have_issued_message;
use service_com_amazonaws_acmpca_acmprivateca::state::CERTIFICATE_NOT_FOUND_MESSAGE;

use crate::private_ca_lifecycle::{
    active_ca, blob, csr_pem, own_ca_certificate, root_ca_request, string,
    wait_for_the_clock, Emulator,
};
use crate::resource_policy::error_message;

/// The template that produces a ROOT CA's own certificate.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// A certificate ARN under `ca_arn` that no certificate of that CA carries: a serial of the
/// right form the CA never allocated.
fn unissued_certificate_arn(ca_arn: &str) -> String {
    format!("{ca_arn}/certificate/0123456789abcdef0123456789abcdef")
}

/// A `GetCertificate` body.
fn read(ca_arn: &str, certificate_arn: &str) -> serde_json::Value {
    json!({"CertificateAuthorityArn": ca_arn, "CertificateArn": certificate_arn})
}

/// A CA with no certificate of its own answers the state refusal, in every status it can be
/// in before it takes one, and whatever certificate ARN the request names.
#[tokio::test]
async fn a_ca_that_cannot_have_issued_certificates_answers_the_state_refusal() {
    let emulator = Emulator::new();

    // Two CAs: one read while its creation has not settled, one read after it has.
    let creating = string(
        &emulator
            .ok("creating", "CreateCertificateAuthority", &root_ca_request("creating-root"))
            .await,
        "CertificateAuthorityArn",
        "creating",
    );
    let pending = string(
        &emulator
            .ok("pending", "CreateCertificateAuthority", &root_ca_request("pending-root"))
            .await,
        "CertificateAuthorityArn",
        "pending",
    );

    // CREATING: the CA exists and has no certificate yet.
    for (case, arn) in [("a CA still being created", &creating)] {
        let refused = emulator
            .refused(
                case,
                "GetCertificate",
                &read(arn, &unissued_certificate_arn(arn)),
                "InvalidStateException",
            )
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            message,
            not_in_state_to_have_issued_message(arn),
            "case {case:?}: expected the sentence about the authority, got {message:?}"
        );
    }

    wait_for_the_clock();

    // PENDING_CERTIFICATE: the same refusal, because the CA still holds no certificate of
    // its own to have signed anything with.
    for (case, arn) in [
        ("a CA waiting for its certificate", &pending),
        ("a CA whose creation has settled since", &creating),
    ] {
        let refused = emulator
            .refused(
                case,
                "GetCertificate",
                &read(arn, &unissued_certificate_arn(arn)),
                "InvalidStateException",
            )
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            message,
            not_in_state_to_have_issued_message(arn),
            "case {case:?}: expected the sentence about the authority, got {message:?}"
        );
    }

    // The ONE certificate such a CA can have issued is its own root certificate, and that
    // one is served: the state refusal is about a CA with nothing to show, not about the
    // status it is in.
    let case = "the root certificate a pending CA signed for itself";
    let csr = string(
        &emulator
            .ok(
                case,
                "GetCertificateAuthorityCsr",
                &json!({"CertificateAuthorityArn": pending}),
            )
            .await,
        "Csr",
        case,
    );
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": pending,
                "Csr": blob(&csr),
                "SigningAlgorithm": "SHA256WITHRSA",
                "TemplateArn": ROOT_CA_TEMPLATE,
                "Validity": {"Type": "YEARS", "Value": 3},
            }),
        )
        .await;
    let root_arn = string(&issued, "CertificateArn", case);

    // Until that issuance completes, the retry signal is the one that says so.
    let in_progress = emulator
        .refused(
            case,
            "GetCertificate",
            &read(&pending, &root_arn),
            "RequestInProgressException",
        )
        .await;
    assert!(
        error_message(&in_progress, case).contains(&root_arn),
        "case {case:?}: the in-progress refusal is about the certificate being minted"
    );
    wait_for_the_clock();
    let served = emulator
        .ok(case, "GetCertificate", &read(&pending, &root_arn))
        .await;
    assert!(
        string(&served, "Certificate", case).contains("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: a certificate the CA did issue is served whatever status it is in"
    );
    // Any OTHER ARN on that same CA is still the state refusal: it holds no certificate of
    // its own yet.
    let case = "another ARN on the same pending CA";
    let refused = emulator
        .refused(
            case,
            "GetCertificate",
            &read(&pending, &unissued_certificate_arn(&pending)),
            "InvalidStateException",
        )
        .await;
    assert_eq!(
        error_message(&refused, case),
        not_in_state_to_have_issued_message(&pending),
        "case {case:?}: expected the sentence about the authority"
    );
}

/// Once a CA holds its certificate, an ARN it never issued is the absence of the
/// CERTIFICATE — in every status such a CA can rest in.
#[tokio::test]
async fn a_ca_holding_its_certificate_reports_the_certificate_as_the_absent_one() {
    let emulator = Emulator::new();
    let (arn, _) = active_ca(&emulator, "served-root").await;

    // A certificate it did issue, read back once issuance has completed.
    let case = "a leaf of an active CA";
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob(&csr_pem("an-end-entity")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Type": "DAYS", "Value": 30},
            }),
        )
        .await;
    let leaf = string(&issued, "CertificateArn", case);
    wait_for_the_clock();
    emulator.ok(case, "GetCertificate", &read(&arn, &leaf)).await;

    // (case, the status the CA is put into before the read)
    let statuses: &[(&str, Option<&str>)] = &[
        ("an active CA", None),
        ("a disabled CA", Some("DISABLED")),
        ("an active CA again", Some("ACTIVE")),
    ];
    for (case, status) in statuses {
        if let Some(status) = status {
            emulator
                .ok(
                    case,
                    "UpdateCertificateAuthority",
                    &json!({"CertificateAuthorityArn": arn, "Status": status}),
                )
                .await;
        }
        let refused = emulator
            .refused(
                case,
                "GetCertificate",
                &read(&arn, &unissued_certificate_arn(&arn)),
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            message, CERTIFICATE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the sentence about the certificate, got {message:?}"
        );
        // And the certificate it did issue is still served in that status.
        emulator.ok(case, "GetCertificate", &read(&arn, &leaf)).await;
    }
}

/// A CA whose certificate was imported rather than issued here answers the same way: what
/// decides the refusal is whether the CA HOLDS a certificate, not which call produced it.
#[tokio::test]
async fn an_imported_certificate_is_what_moves_the_ca_past_the_state_refusal() {
    let emulator = Emulator::new();
    let case = "imported";
    let arn = string(
        &emulator
            .ok(case, "CreateCertificateAuthority", &root_ca_request("imported-root"))
            .await,
        "CertificateAuthorityArn",
        case,
    );
    wait_for_the_clock();

    let before = emulator
        .refused(
            case,
            "GetCertificate",
            &read(&arn, &unissued_certificate_arn(&arn)),
            "InvalidStateException",
        )
        .await;
    assert_eq!(
        error_message(&before, case),
        not_in_state_to_have_issued_message(&arn),
        "case {case:?}: before its certificate, the refusal is about the authority"
    );

    let own = own_ca_certificate(&emulator, case, &arn).await;
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": arn, "Certificate": blob(&own)}),
        )
        .await;

    let after = emulator
        .refused(
            case,
            "GetCertificate",
            &read(&arn, &unissued_certificate_arn(&arn)),
            "ResourceNotFoundException",
        )
        .await;
    assert_eq!(
        error_message(&after, case),
        CERTIFICATE_NOT_FOUND_MESSAGE,
        "case {case:?}: with its certificate, the refusal is about the certificate"
    );
}

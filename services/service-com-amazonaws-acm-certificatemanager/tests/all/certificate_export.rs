// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportCertificate`.
//!
//! RULE under test: `ExportCertificate` classifies the certificate the request names
//! against the store and answers the modelled error for the condition it is in — an ARN
//! that names no ACM certificate, an ARN naming no stored certificate, a certificate
//! export was never allowed for (which every certificate created without the `Export`
//! opt-in is, and the opt-in "cannot [be updated] after the the certificate is
//! created"), and a certificate whose material ACM holds none of yet. Every one of
//! these is HTTP 400 with its own code and sentence, and none of them is a server
//! fault.
//!
//! The table spans the classes the rule generalises over: both kinds of certificate a
//! client can create (imported and requested), the export option absent, `DISABLED` and
//! `ENABLED`, a pending and an issued request, an ARN naming nothing and an ARN naming
//! another kind of ACM resource — and, so the classification is shown to be independent
//! of the passphrase, a passphrase at each end of the modelled `@length 4..128` range
//! plus one byte past its lower bound.

use serde_json::{json, Value};

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, request_certificate, router_with_engine, ACCOUNT,
    REGION,
};
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The `ValidationException` sentence ACM answers when the certificate named is not
/// one export was ever enabled for, as recorded against the real service.
fn expected_not_export_enabled_message(arn: &str) -> String {
    format!("Certificate ARN: {arn} is not an export enabled certificate")
}

/// The `RequestInProgressException` sentence, the error class's own documented sentence
/// ("The certificate request is in process and the certificate in your account has not
/// yet been issued").
const EXPECTED_REQUEST_IN_PROGRESS_MESSAGE: &str =
    "The certificate request is in process and the certificate in your account has not yet \
     been issued.";

/// An `ExportCertificate` call with `passphrase` as the `Passphrase` blob.
async fn export(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
    passphrase: &[u8],
) -> (u16, Value) {
    call(
        router,
        "ExportCertificate",
        &json!({
            "CertificateArn": arn,
            "Passphrase": aws_smithy_types::base64::encode(passphrase),
        }),
    )
    .await
}

/// RULE: every certificate this emulator can hold, and every ARN it cannot resolve, is
/// classified by `ExportCertificate` with the modelled 400 for its own condition.
#[tokio::test]
async fn export_classifies_the_certificate_the_request_names() {
    let (router, engine) = router_with_engine();

    // An imported certificate: `ImportCertificate` carries no options, so export was
    // never opted in to for it.
    let imported = import(
        &router,
        "imported",
        &import_body(&generate(&Blueprint {
            common_name: Some("imported.export.example.com"),
            ..Blueprint::default()
        })),
    )
    .await;

    // A requested certificate per export option: absent, DISABLED and ENABLED. The
    // private-CA requests have nothing to prove, so the clock carries them to ISSUED;
    // the public one is still PENDING_VALIDATION when it is exported below.
    let default_option = request_certificate(
        &router,
        &json!({
            "DomainName": "default.export.example.com",
            "ValidationMethod": "DNS",
        }),
    )
    .await;
    let disabled = request_certificate(
        &router,
        &json!({
            "DomainName": "disabled.export.example.com",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
            "Options": { "Export": "DISABLED" },
        }),
    )
    .await;
    let pending_enabled = request_certificate(
        &router,
        &json!({
            "DomainName": "pending.export.example.com",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;
    let issued_enabled = request_certificate(
        &router,
        &json!({
            "DomainName": "issued.export.example.com",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;

    // An ARN of the form the member's `@pattern` admits that names no stored
    // certificate, and one naming another kind of ACM resource entirely.
    let absent = format!(
        "arn:aws:acm:{REGION}:{ACCOUNT}:certificate/00000000-0000-0000-0000-000000000000"
    );
    let not_a_certificate =
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/00000000-0000-0000-0000-000000000000");

    // The private-CA requests reach ISSUED; the public one stays pending, so the rows
    // below cover a certificate in each state.
    let (status, response) = call(
        &router,
        "DescribeCertificate",
        &json!({ "CertificateArn": &pending_enabled }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case fixture: DescribeCertificate must answer 200, got {status} {response}"
    );
    make_validation_due_now(&engine);
    for (arn, expected_status) in [
        (&disabled, "ISSUED"),
        (&issued_enabled, "ISSUED"),
        (&imported, "ISSUED"),
    ] {
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some(expected_status),
            "case fixture {arn}: expected the fixture to be {expected_status}, got {detail}"
        );
    }

    // Each row: what it is, the ARN to export, the passphrase to send, and the modelled
    // code and sentence it must be refused with.
    //
    // `Passphrase` is `@length 4..128`; the first three rows send the same imported
    // certificate with the shortest, the longest and a mid-range passphrase, so the
    // classification is shown not to depend on it.
    let cases: &[(&str, &String, Vec<u8>, &str, String)] = &[
        (
            "an imported certificate, four-byte passphrase",
            &imported,
            b"Pas1".to_vec(),
            "ValidationException",
            expected_not_export_enabled_message(&imported),
        ),
        (
            "an imported certificate, 128-byte passphrase",
            &imported,
            vec![b'p'; 128],
            "ValidationException",
            expected_not_export_enabled_message(&imported),
        ),
        (
            "an imported certificate",
            &imported,
            b"Sim2RealPass123".to_vec(),
            "ValidationException",
            expected_not_export_enabled_message(&imported),
        ),
        (
            "a requested certificate whose options named no export option",
            &default_option,
            b"Sim2RealPass123".to_vec(),
            "ValidationException",
            expected_not_export_enabled_message(&default_option),
        ),
        (
            "an issued certificate created with Export DISABLED",
            &disabled,
            b"Sim2RealPass123".to_vec(),
            "ValidationException",
            expected_not_export_enabled_message(&disabled),
        ),
        (
            "a pending certificate created with Export ENABLED",
            &pending_enabled,
            b"Sim2RealPass123".to_vec(),
            "RequestInProgressException",
            EXPECTED_REQUEST_IN_PROGRESS_MESSAGE.to_string(),
        ),
        (
            "an issued certificate created with Export ENABLED",
            &issued_enabled,
            b"Sim2RealPass123".to_vec(),
            "RequestInProgressException",
            EXPECTED_REQUEST_IN_PROGRESS_MESSAGE.to_string(),
        ),
        (
            "a certificate ARN no record answers to",
            &absent,
            b"Sim2RealPass123".to_vec(),
            "ResourceNotFoundException",
            format!("Could not find certificate {absent}."),
        ),
        (
            "an ACM ARN that is not a certificate",
            &not_a_certificate,
            b"Sim2RealPass123".to_vec(),
            "InvalidArnException",
            format!("{not_a_certificate} is not a valid ARN"),
        ),
    ];

    for (case, arn, passphrase, expected_code, expected_message) in cases {
        let (status, response) = export(&router, arn, passphrase).await;
        assert_eq!(
            status, 400,
            "case {case:?}: every ExportCertificate refusal is a 400, got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            &code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected the sentence {expected_message:?}, got {message:?}"
        );
    }
}

/// RULE: a `Passphrase` outside the modelled `@length 4..128` range never reaches the
/// classification above — the request walk refuses it first, with the constraint
/// refusal rather than an export sentence.
#[tokio::test]
async fn a_passphrase_past_the_modelled_bound_is_refused_before_the_store_is_read() {
    let (router, _engine) = router_with_engine();
    let arn = import(
        &router,
        "the passphrase bound",
        &import_body(&generate(&Blueprint {
            common_name: Some("passphrase.export.example.com"),
            ..Blueprint::default()
        })),
    )
    .await;

    // One byte inside each published bound, and one byte past the lower one.
    for (case, passphrase, refused) in [
        ("four bytes, the shortest the model admits", vec![b'p'; 4], false),
        ("128 bytes, the longest the model admits", vec![b'p'; 128], false),
        ("three bytes, one short of the range", vec![b'p'; 3], true),
        ("129 bytes, one past the range", vec![b'p'; 129], true),
    ] {
        let (status, response) = export(&router, &arn, &passphrase).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        let message = error_message(&response);
        let export_sentence = expected_not_export_enabled_message(&arn);
        if refused {
            assert_ne!(
                message, export_sentence,
                "case {case:?}: a passphrase outside the range must be refused on the \
                 constraint, not on the certificate, got {message:?}"
            );
            assert!(
                message.contains("passphrase"),
                "case {case:?}: expected the refusal to name the member, got {message:?}"
            );
        } else {
            assert_eq!(
                message, export_sentence,
                "case {case:?}: a passphrase inside the range reaches the store, so the \
                 refusal is the certificate's, got {message:?}"
            );
        }
    }
}

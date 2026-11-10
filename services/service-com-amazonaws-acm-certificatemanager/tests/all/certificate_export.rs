// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportCertificate`.
//!
//! RULE under test: `ExportCertificate` classifies the certificate the request names
//! against the store and answers the modelled error for the condition it is in — an ARN
//! that names no ACM certificate, an ARN naming no stored certificate, a certificate
//! export was never allowed for (which every certificate created without the `Export`
//! opt-in is, and the opt-in "cannot [be updated] after the the certificate is
//! created"), an ARN under which no ISSUED certificate stands — the member names "an
//! ARN of the issued certificate" — and a certificate whose material ACM holds none of
//! yet. Every one of these is HTTP 400 with its own code and sentence, and none of them
//! is a server fault.
//!
//! The table spans the classes the rule generalises over: both kinds of certificate a
//! client can create (imported and requested), the export option absent, `DISABLED` and
//! `ENABLED`, an issued request and one that never reached ISSUED, an ARN naming nothing
//! and an ARN naming another kind of ACM resource — and, so the classification is shown
//! to be independent of the passphrase, a passphrase at each end of the modelled
//! `@length 4..128` range plus one byte past its lower bound.

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

/// The `RequestInProgressException` sentence ACM answers when it holds no issued
/// material for the certificate the request named: the ARN from the request and the
/// account that ARN names, inside fixed text and with no terminating period — the same
/// sentence `GetCertificate` reports for the same gap.
fn expected_not_yet_issued_message(arn: &str) -> String {
    format!("Certificate {arn} in account {ACCOUNT} not yet issued")
}

/// The `ResourceNotFoundException` sentence ACM answers when the ARN resolves but names
/// no ISSUED certificate, as recorded against the real service.
fn expected_no_issued_certificate_message(arn: &str) -> String {
    format!("Could not find an issued certificate {arn}")
}

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
    // the public one names a domain the public DNS delegates to nobody, so the clock
    // carries it to a terminal status that is not ISSUED.
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
    let unissued_enabled = request_certificate(
        &router,
        &json!({
            "DomainName": "unissued.export.invalid",
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

    // The private-CA requests reach ISSUED; the public one reaches a terminal status
    // that is not ISSUED, so the rows below cover a certificate in each state.
    let (status, response) = call(
        &router,
        "DescribeCertificate",
        &json!({ "CertificateArn": &unissued_enabled }),
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
        (&unissued_enabled, "FAILED"),
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
            "an unissued certificate created with Export ENABLED",
            &unissued_enabled,
            b"Sim2RealPass123".to_vec(),
            "ResourceNotFoundException",
            expected_no_issued_certificate_message(&unissued_enabled),
        ),
        (
            "an issued certificate created with Export ENABLED",
            &issued_enabled,
            b"Sim2RealPass123".to_vec(),
            "RequestInProgressException",
            expected_not_yet_issued_message(&issued_enabled),
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

/// The passphrase every row below sends, inside the modelled `@length 4..128` range.
const A_PASSPHRASE: &[u8] = b"Sim2RealPass123";

/// RULE: the ARN this operation takes is "an Amazon Resource Name (ARN) of the issued
/// certificate" (`ExportCertificateRequest$CertificateArn` doc), so an export-enabled
/// certificate the account holds in any `CertificateStatus` other than `ISSUED` is an
/// ARN under which there is no issued certificate: HTTP 400 `ResourceNotFoundException`
/// with `Could not find an issued certificate <arn>` — whichever non-issued status the
/// record is in — and the refusal leaves the record as it was.
#[tokio::test]
async fn an_export_enabled_certificate_that_is_not_issued_names_no_issued_certificate() {
    let (router, engine) = router_with_engine();

    // Three export-enabled requests, differing in what the clock owes each: a domain
    // whose control COULD be published, so the request waits its validation window out;
    // a domain the public DNS delegates to nobody, so the request fails; and a
    // private-CA request
    // ("if you are requesting a private certificate, domain validation is not
    // required"), which is the ISSUED row the rule stops at.
    let waiting = request_certificate(
        &router,
        &json!({
            "DomainName": "waiting.export.probe-domain.com",
            "ValidationMethod": "DNS",
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;
    let unprovable = request_certificate(
        &router,
        &json!({
            "DomainName": "*.unprovable.export.invalid",
            "ValidationMethod": "HTTP",
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;
    let issued = request_certificate(
        &router,
        &json!({
            "DomainName": "boundary.export.probe-domain.net",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
            "Options": { "Export": "ENABLED" },
        }),
    )
    .await;

    // Before any outcome falls due, both public requests are in the status a requested
    // certificate "enters upon being requested".
    for (case, arn) in [
        ("a request still awaiting domain validation", &waiting),
        ("a request whose domain can never be validated", &unprovable),
    ] {
        assert_no_issued_certificate(&router, case, arn, "PENDING_VALIDATION").await;
    }

    // Then the clock: each stored outcome falls due, carrying the two public requests to
    // the terminal statuses they were headed for.
    make_validation_due_now(&engine);
    for (case, arn, status) in [
        (
            "a request that waited out its validation window",
            &waiting,
            "VALIDATION_TIMED_OUT",
        ),
        (
            "a request whose domain can never be validated",
            &unprovable,
            "FAILED",
        ),
    ] {
        assert_no_issued_certificate(&router, case, arn, status).await;
    }

    // The ISSUED record bounds the rule: an issued certificate stands under that ARN, so
    // whatever this emulator can hand back for it, the answer is not "there is no issued
    // certificate here".
    let case = "an issued certificate created with Export ENABLED";
    let detail = describe(&router, &issued).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case:?}: expected the fixture to be ISSUED, got {detail}"
    );
    let (status, response) = export(&router, &issued, A_PASSPHRASE).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400, got {status} {response}"
    );
    let code = error_code(&response);
    assert_ne!(
        code, "ResourceNotFoundException",
        "case {case:?}: an issued certificate is found under its ARN, got {code} {response}"
    );
    let message = error_message(&response);
    assert_ne!(
        message,
        expected_no_issued_certificate_message(&issued),
        "case {case:?}: an issued certificate is found under its ARN, got {message:?}"
    );
}

/// Export `arn`, which must be in the non-issued `expected_status`, and assert the
/// refusal is the modelled "no issued certificate under this ARN" one — and that it
/// changed nothing about the record.
async fn assert_no_issued_certificate(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
    expected_status: &str,
) {
    let detail = describe(router, arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some(expected_status),
        "case {case:?}: expected the fixture to be {expected_status}, got {detail}"
    );

    let (status, response) = export(router, arn, A_PASSPHRASE).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400, got {status} {response}"
    );
    let code = error_code(&response);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {case:?}: a certificate in {expected_status} is no issued certificate, expected \
         ResourceNotFoundException, got {code} {response}"
    );
    let message = error_message(&response);
    let expected_message = expected_no_issued_certificate_message(arn);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the sentence {expected_message:?}, got {message:?}"
    );

    let after = describe(router, arn).await;
    assert_eq!(
        after["Status"].as_str(),
        Some(expected_status),
        "case {case:?}: a refused export changes nothing, expected {expected_status}, got {after}"
    );
}

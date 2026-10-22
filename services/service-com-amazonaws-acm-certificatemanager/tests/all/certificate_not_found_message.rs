// SPDX-License-Identifier: CC0-1.0

//! The errors ACM answers with when a certificate ARN does not resolve to a stored
//! record.
//!
//! RULE: every operation that resolves a certificate ARN renders its ARN-resolution
//! failure as one sentence built from the ARN the request named, identical on every
//! such operation and HTTP 400 in each case: `Could not find certificate
//! <requested-arn>.` for a well-formed ACM certificate ARN with no record behind it
//! (`ResourceNotFoundException`), and `<requested-arn> is not a valid ARN` for an ARN
//! that is not an ACM certificate ARN at all (`InvalidArnException`).
//!
//! The table below spans the classes the rule generalises over: the operations that
//! model the error and are implemented, the ways an ARN can have no record behind it
//! (never created, deleted, another region, another account) and the forms a
//! certificate id can take (a UUID, a hyphenated label, a single character, a long
//! one). The wire harness is shared with [`crate::certificate_lifecycle`], so every
//! call goes through `MultiServiceRouter::dispatch`.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};

/// The implemented operations that resolve `CertificateArn` and model
/// `ResourceNotFoundException`
/// (`smithy_query(query="operation", operation="DescribeCertificate,DeleteCertificate,AddTagsToCertificate,ListTagsForCertificate")`).
const CERTIFICATE_LOOKUPS: &[&str] = &[
    "AddTagsToCertificate",
    "DeleteCertificate",
    "DescribeCertificate",
    "ListTagsForCertificate",
];

/// The `ResourceNotFoundException` sentence the rule requires for `arn`.
fn expected_not_found_message(arn: &str) -> String {
    format!("Could not find certificate {arn}.")
}

/// The `InvalidArnException` sentence the rule requires for `arn`.
fn expected_invalid_arn_message(arn: &str) -> String {
    format!("{arn} is not a valid ARN")
}

/// One request body for `operation` naming `arn`, carrying whatever other members
/// that operation requires.
fn request_body(operation: &str, arn: &str) -> Value {
    let mut body = json!({ "CertificateArn": arn });
    if operation == "AddTagsToCertificate" {
        // `Tags` is required and `TagList` has a minimum length of 1, so the request
        // has to carry one to reach the certificate lookup at all.
        body["Tags"] = json!([{ "Key": "env", "Value": "test" }]);
    }
    body
}

/// RULE: a well-formed ACM certificate ARN with no stored record behind it draws
/// HTTP 400 `ResourceNotFoundException` whose message is
/// `Could not find certificate <requested-arn>.` — the same template, built from the
/// request, on every operation that resolves a certificate ARN.
#[tokio::test]
async fn missing_certificates_report_the_not_found_sentence() {
    let router = router();

    // A live certificate, so the table proves the sentence is drawn by the lookup
    // failing and not by the operation always failing.
    let live = request_certificate(
        &router,
        &json!({ "DomainName": "live.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    // Each row: what makes the ARN resolve to nothing, and the ARN. Every ARN
    // satisfies the member's modelled length (20..2048) and `@pattern`, so each one
    // reaches the handler's lookup. The id forms differ along what the rule
    // generalises over: a UUID, the all-zero UUID, a hyphenated label, one character,
    // a 64-character id, an upper-case id (certificate ids are matched exactly).
    let cases: &[(&str, String)] = &[
        (
            "a UUID with nothing behind it",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "the all-zero UUID",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a hyphenated label that is not a UUID",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/sim2real-probe-acm-nonexistent-10066c6d"),
        ),
        (
            "a single-character id",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/z"),
        ),
        (
            "a 64-character id",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/{}", "a".repeat(64)),
        ),
        (
            "an upper-case id",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/ABCDEF01-2345-6789-ABCD-EF0123456789"),
        ),
        (
            "a live certificate's id in another region",
            live.replace(REGION, "eu-central-1"),
        ),
        (
            "a live certificate's id under another account",
            live.replace(ACCOUNT, "123456789012"),
        ),
        (
            "an ARN in another partition",
            format!("arn:aws-us-gov:acm:{REGION}:{ACCOUNT}:certificate/22222222-3333-4444-5555-666666666666"),
        ),
    ];

    for operation in CERTIFICATE_LOOKUPS {
        for (case, arn) in cases {
            let (status, response) = call(&router, operation, &request_body(operation, arn)).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation}/{case}: expected 400 ResourceNotFoundException for {arn}, \
                 got {status} {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                expected_not_found_message(arn),
                "case {operation}/{case}: expected message {:?}, got {message:?}",
                expected_not_found_message(arn)
            );
        }
    }

    // The refusals left the live certificate alone: it still answers, and with no
    // not-found sentence.
    let (status, response) = call(
        &router,
        "DescribeCertificate",
        &json!({ "CertificateArn": live }),
    )
    .await;
    assert_eq!(
        (status, response["Certificate"]["CertificateArn"].as_str()),
        (200, Some(live.as_str())),
        "case live certificate: expected the stored record, got {status} {response}"
    );
}

/// RULE: the sentence names the ARN the request asked for, and keeps naming it once
/// the certificate it named has been deleted — the post-delete path renders the same
/// template as a never-created ARN.
#[tokio::test]
async fn a_deleted_certificate_reports_the_same_sentence() {
    let router = router();

    // Two certificates created with different shapes of request, so the template is
    // not read off one stored record's fields.
    let arns = [
        request_certificate(
            &router,
            &json!({ "DomainName": "doomed.example.com", "ValidationMethod": "DNS" }),
        )
        .await,
        request_certificate(
            &router,
            &json!({
                "DomainName": "*.doomed.example.net",
                "SubjectAlternativeNames": ["alt.doomed.example.net"],
                "ValidationMethod": "EMAIL",
                "Tags": [{ "Key": "env", "Value": "dev" }]
            }),
        )
        .await,
    ];

    for arn in &arns {
        let (status, response) = call(
            &router,
            "DeleteCertificate",
            &json!({ "CertificateArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn}: DeleteCertificate must succeed, got {status} {response}"
        );

        for operation in CERTIFICATE_LOOKUPS {
            let (status, response) = call(&router, operation, &request_body(operation, arn)).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation}/deleted {arn}: expected 400 ResourceNotFoundException, \
                 got {status} {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                expected_not_found_message(arn),
                "case {operation}/deleted {arn}: expected message {:?}, got {message:?}",
                expected_not_found_message(arn)
            );
        }
    }
}

/// RULE: an ARN that is not an ACM certificate ARN is refused before any lookup with
/// `InvalidArnException` and `<requested-arn> is not a valid ARN`, never with the
/// not-found sentence.
#[tokio::test]
async fn an_invalid_arn_reports_the_invalid_arn_sentence() {
    let router = router();

    // Each row satisfies the member's modelled `@pattern` but names something that is
    // not a certificate, so the handler classifies it before any lookup.
    let cases: &[(&str, String)] = &[
        (
            "a resource type that is not a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij"),
        ),
        (
            "an ACM ARN with no resource type",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate"),
        ),
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra"),
        ),
        (
            "an ACM resource with no type separator at all",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:not-a-real-arn"),
        ),
        (
            "a global ACM ARN with an empty region",
            "arn:aws:acm::0:aaaaaaaaaaaaaaaaaaaa".to_string(),
        ),
    ];

    for operation in CERTIFICATE_LOOKUPS {
        for (case, arn) in cases {
            let (status, response) = call(&router, operation, &request_body(operation, arn)).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidArnException"),
                "case {operation}/{case}: expected 400 InvalidArnException for {arn}, \
                 got {status} {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                expected_invalid_arn_message(arn),
                "case {operation}/{case}: expected message {:?}, got {message:?}",
                expected_invalid_arn_message(arn)
            );
            assert!(
                !message.starts_with("Could not find certificate"),
                "case {operation}/{case}: an invalid ARN must not report the not-found \
                 sentence, got {message:?}"
            );
        }
    }
}

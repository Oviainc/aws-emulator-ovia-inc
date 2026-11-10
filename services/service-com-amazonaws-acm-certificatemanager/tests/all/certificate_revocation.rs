// SPDX-License-Identifier: CC0-1.0

//! `RevokeCertificate`, end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: `RevokeCertificate` applies only to a stored certificate export
//! was allowed for, that has been exported, and whose revocation is not already
//! recorded ("Revokes a public ACM certificate. You can only revoke certificates that
//! have been previously exported. Once a certificate is revoked, you cannot reuse the
//! certificate. Revoking a certificate is permanent." — `RevokeCertificate` doc;
//! export itself is opt-in and fixed at creation — `CertificateOptions$Export`). For
//! such a certificate it records the revocation on the record — `REVOKED`, with the
//! instant it happened and the `RevocationReason` the caller gave, the two members
//! that "exist only when the certificate status is REVOKED"
//! (`CertificateDetail$RevokedAt` / `$RevocationReason` docs) — so every later read
//! reports it; any other certificate is refused with the modelled error for its
//! condition and left exactly as it was.
//!
//! The tables below span what the rule generalises over: every `RevocationReason` the
//! model declares, several domain forms and SAN counts, and every status a stored
//! certificate can be in when the call arrives. Each assertion writes with one
//! operation and reads back with another (`DescribeCertificate`, `ListCertificates`),
//! so nothing here can pass on a response that was not computed from the persisted
//! record.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::ComAmazonawsAcmCertificatemanagerState;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router_with_engine, ACCOUNT, REGION,
};

/// Every `RevocationReason` the model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#RevocationReason")`).
const REVOCATION_REASONS: &[&str] = &[
    "UNSPECIFIED",
    "KEY_COMPROMISE",
    "CA_COMPROMISE",
    "AFFILIATION_CHANGED",
    "SUPERCEDED",
    "SUPERSEDED",
    "CESSATION_OF_OPERATION",
    "CERTIFICATE_HOLD",
    "REMOVE_FROM_CRL",
    "PRIVILEGE_WITHDRAWN",
    "A_A_COMPROMISE",
];

/// A private certificate authority ARN of the form `CertificateAuthorityArn`'s
/// pattern requires. A request naming one needs no proof of domain ownership, so the
/// certificate it creates is the one this emulator carries through to `ISSUED` — the
/// only state an export, and therefore a revocation, can follow.
const PRIVATE_CA_ARN: &str =
    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012";

/// One success row: the `RequestCertificate` body of the certificate to revoke and
/// the `RevocationReason` to revoke it for.
type RevokeCase<'a> = (Value, &'a str);

/// The `RequestCertificate` body of a certificate that can be carried all the way to
/// a revocation: a private-CA request (nothing to prove, so it reaches `ISSUED`) that
/// opts in to export, which "you cannot update ... after the the certificate is
/// created" (`CertificateOptions$Export` doc).
pub(crate) fn revocable_request(domain: &str, sans: &[String]) -> Value {
    let mut body = json!({
        "DomainName": domain,
        "CertificateAuthorityArn": PRIVATE_CA_ARN,
        "Options": { "Export": "ENABLED" },
    });
    if !sans.is_empty() {
        body["SubjectAlternativeNames"] = json!(sans);
    }
    body
}

/// The `ValidationException` sentence ACM answers for a certificate export was never
/// allowed for, as recorded against the real service.
fn expected_export_not_allowed_message(arn: &str) -> String {
    format!("Export not allowed for certificate {arn}")
}

/// Move every stored validation outcome into the past, so the next request
/// materialises it.
///
/// This writes only the due instant the emulator itself stores; the transition is
/// still applied by the service's own `materialize_due` on the next call, exactly as
/// in production.
fn make_validation_due_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for certificate in store.certificates.values_mut() {
        if let Some(pending) = certificate.pending_validation.as_mut() {
            pending.due_at = past;
        }
    }
}

/// Mark the stored certificate `arn` as exported — the one fact `RevokeCertificate`
/// requires of a certificate ("You can only revoke certificates that have been
/// previously exported").
///
/// `ExportCertificate` is what sets this flag in production, and it cannot set it here:
/// this service mints no key pair for a certificate it was asked to issue, so there is
/// no material for an export to hand back. The test therefore writes the single boolean
/// the emulator itself stores — exactly the
/// record that operation will leave behind — and drives the revocation and every read
/// of it over the wire.
pub(crate) fn mark_exported(engine: &Arc<engine::InMemoryEngine>, arn: &str) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    store
        .certificates
        .get_mut(arn)
        .unwrap_or_else(|| panic!("{arn} has no stored certificate"))
        .exported = true;
}

/// Request the certificate `body` asks for and carry it to `ISSUED`, returning its
/// ARN. Every step goes over the wire bar the stored due instant, which is moved into
/// the past so the test does not race the settle delay.
pub(crate) async fn issued_certificate(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    body: &Value,
) -> String {
    let arn = request_certificate(router, body).await;
    make_validation_due_now(engine);
    let detail = describe(router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {body}: the certificate to revoke must be issued first, got {detail}"
    );
    arn
}

/// Wall-clock now as epoch seconds, the form an awsJson1_1 timestamp takes.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A numeric timestamp member of a response object, as epoch seconds.
fn timestamp(case: &str, object: &Value, member: &str) -> f64 {
    object[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {object}"))
}

/// The `CertificateSummary` `ListCertificates` reports for `arn`, under `filters`, or
/// `None` when the listing does not carry it.
async fn summary(router: &MultiServiceRouter, filters: &Value, arn: &str) -> Option<Value> {
    let (status, response) = call(router, "ListCertificates", filters).await;
    assert_eq!(
        status, 200,
        "case {filters}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filters}: no CertificateSummaryList in {response}"))
        .iter()
        .find(|summary| summary["CertificateArn"].as_str() == Some(arn))
        .cloned()
}

/// RULE: revoking an exported certificate records the revocation on the stored
/// record, and every later read reports it — for every `RevocationReason` the model
/// declares and whatever shape the certificate's name set has.
#[tokio::test]
async fn revoking_an_exported_certificate_is_reported_by_every_reader() {
    // One row per `RevocationReason`, each on a certificate of a different shape: a
    // plain name, a wildcard, several subject alternative names, a deep label.
    let cases: Vec<RevokeCase<'_>> = REVOCATION_REASONS
        .iter()
        .enumerate()
        .map(|(index, reason)| {
            let body = match index % 4 {
                0 => revocable_request(&format!("plain-{index}.example.com"), &[]),
                1 => revocable_request(&format!("*.wildcard-{index}.example.net"), &[]),
                2 => revocable_request(
                    &format!("multi-{index}.example.org"),
                    &[
                        format!("alt-a-{index}.example.org"),
                        format!("alt-b-{index}.example.org"),
                    ],
                ),
                _ => revocable_request(&format!("edge.deep.label-{index}.example.io"), &[]),
            };
            (body, *reason)
        })
        .collect();

    let (router, engine) = router_with_engine();
    let mut revoked: Vec<String> = Vec::new();

    for (body, reason) in &cases {
        let case = format!("{reason} on {body}");
        let arn = issued_certificate(&router, &engine, body).await;
        mark_exported(&engine, &arn);

        // Before the call the record carries neither member, which is what the rule
        // ties to the status.
        let before = describe(&router, &arn).await;
        assert!(
            before["RevokedAt"].is_null() && before["RevocationReason"].is_null(),
            "case {case}: an unrevoked certificate carries neither RevokedAt nor \
             RevocationReason, got {before}"
        );

        let called_at = now_epoch_seconds();
        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": reason }),
        )
        .await;
        assert_eq!(
            (status, response["CertificateArn"].as_str()),
            (200, Some(arn.as_str())),
            "case {case}: expected 200 and the revoked ARN, got {status} {response}"
        );

        // DescribeCertificate reports the record the revocation wrote.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after["Status"].as_str(),
            Some("REVOKED"),
            "case {case}: expected Status REVOKED, got {after}"
        );
        assert_eq!(
            after["RevocationReason"].as_str(),
            Some(*reason),
            "case {case}: expected RevocationReason {reason}, got {after}"
        );
        let revoked_at = timestamp(&case, &after, "RevokedAt");
        assert!(
            revoked_at >= called_at - 1.0 && revoked_at <= now_epoch_seconds() + 1.0,
            "case {case}: expected RevokedAt at the revoking call, got {revoked_at} \
             outside [{called_at}, {}]",
            now_epoch_seconds()
        );
        assert!(
            revoked_at >= timestamp(&case, &after, "IssuedAt"),
            "case {case}: a certificate cannot be revoked before it was issued, got {after}"
        );

        // Revocation changes the status and those two members only: the issuance facts
        // the certificate already carried are untouched.
        for member in ["IssuedAt", "NotBefore", "NotAfter", "CreatedAt"] {
            assert_eq!(
                after[member], before[member],
                "case {case}: {member} must survive the revocation, was {} and is now {}",
                before[member], after[member]
            );
        }

        // And so does ListCertificates, which projects the same record: the ARN is
        // reported under the REVOKED status filter, with the same instant.
        let listed = summary(&router, &json!({ "CertificateStatuses": ["REVOKED"] }), &arn)
            .await
            .unwrap_or_else(|| panic!("case {case}: {arn} is not listed under REVOKED"));
        assert_eq!(
            listed["Status"].as_str(),
            Some("REVOKED"),
            "case {case}: expected the summary to report REVOKED, got {listed}"
        );
        assert_eq!(
            listed["RevokedAt"], after["RevokedAt"],
            "case {case}: expected the summary's RevokedAt to be the detail's {}, got {}",
            after["RevokedAt"], listed["RevokedAt"]
        );
        assert_eq!(
            summary(&router, &json!({ "CertificateStatuses": ["ISSUED"] }), &arn).await,
            None,
            "case {case}: a revoked certificate is no longer listed as ISSUED"
        );

        revoked.push(arn);
    }

    // Every revocation stuck: each ARN is still reported as revoked after the later
    // rows ran, so no row's write overwrote another's.
    for arn in &revoked {
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("REVOKED"),
            "case {arn}: a revocation is permanent, got {detail}"
        );
    }
}

/// RULE: a certificate the store does not hold as exported is outside the state
/// `RevokeCertificate` applies to, whatever status it is in, so the call is refused
/// with HTTP 400 `ValidationException` and the record is left as it was — and a
/// certificate export was never allowed for at all is refused with the sentence ACM
/// answers for that condition, built from the ARN the request named.
#[tokio::test]
async fn revoking_a_certificate_that_was_never_exported_is_refused() {
    // Each row: what it is, the `RequestCertificate` body, whether the stored
    // validation outcome is materialised before the revoke, the status the record is
    // then in, the reason the revoke asks for, and whether export was opted in for the
    // certificate. The rows span both ways a certificate can fail to be exported and
    // every status a stored certificate can be in when the call arrives. Every row runs
    // on its own store, so holding one certificate pending cannot disturb another.
    let cases: &[(&str, Value, bool, &str, &str, bool)] = &[
        (
            "pending validation, export not opted in",
            json!({ "DomainName": "pending.example.com", "ValidationMethod": "DNS" }),
            false,
            "PENDING_VALIDATION",
            "UNSPECIFIED",
            false,
        ),
        (
            "a failed public request, export not opted in",
            json!({ "DomainName": "failed.invalid", "ValidationMethod": "EMAIL" }),
            true,
            "FAILED",
            "KEY_COMPROMISE",
            false,
        ),
        (
            "issued, export not opted in",
            json!({
                "DomainName": "*.issued.example.net",
                "SubjectAlternativeNames": ["alt.issued.example.net"],
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            true,
            "ISSUED",
            "CA_COMPROMISE",
            false,
        ),
        (
            "issued with export opted in but never exported",
            revocable_request("exportable.example.com", &[]),
            true,
            "ISSUED",
            "CESSATION_OF_OPERATION",
            true,
        ),
        (
            "pending validation with export opted in",
            json!({
                "DomainName": "exportable-pending.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
            }),
            false,
            "PENDING_VALIDATION",
            "A_A_COMPROMISE",
            true,
        ),
    ];

    for (case, body, settle, expected_status, reason, export_allowed) in cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        if *settle {
            make_validation_due_now(&engine);
        } else {
            hold_pending_validations(&engine);
        }

        let before = describe(&router, &arn).await;
        assert_eq!(
            before["Status"].as_str(),
            Some(*expected_status),
            "case {case}: expected the record in {expected_status} before the revoke, got {before}"
        );

        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": reason }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for an unexported certificate, \
             got {status} {response}"
        );
        let message = error_message(&response);
        let expected_export_message = expected_export_not_allowed_message(&arn);
        if *export_allowed {
            assert!(
                message.contains(&arn) && message != expected_export_message,
                "case {case}: expected a refusal naming {arn} for a certificate export is \
                 allowed for but which was never exported, got {message:?}"
            );
        } else {
            assert_eq!(
                message, expected_export_message,
                "case {case}: expected message {expected_export_message:?}, got {message:?}"
            );
        }

        // The refusal wrote nothing: the record is exactly what it was.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case}: a refused revocation must leave the record alone, was {before} \
             and is now {after}"
        );
    }
}

/// RULE: revocation is permanent — a second revocation of the same certificate is
/// refused and the recorded revocation is the first one, whatever reason the later
/// call names.
#[tokio::test]
async fn revoking_an_already_revoked_certificate_keeps_the_first_revocation() {
    let (router, engine) = router_with_engine();

    // One row per later reason, including the reason the first call used, so the test
    // spans both a different reason and a repeat of the same one.
    for second_reason in ["SUPERSEDED", "KEY_COMPROMISE", "UNSPECIFIED"] {
        let case = format!("re-revoked as {second_reason}");
        let arn = issued_certificate(
            &router,
            &engine,
            &revocable_request(
                &format!("twice-{}.example.com", second_reason.to_lowercase()),
                &[],
            ),
        )
        .await;
        mark_exported(&engine, &arn);

        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": "KEY_COMPROMISE" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the first revocation must succeed, got {status} {response}"
        );
        let first = describe(&router, &arn).await;

        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": second_reason }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException for an already revoked \
             certificate, got {status} {response}"
        );

        let after = describe(&router, &arn).await;
        assert_eq!(
            after, first,
            "case {case}: the first revocation stands, was {first} and is now {after}"
        );
        assert_eq!(
            after["RevocationReason"].as_str(),
            Some("KEY_COMPROMISE"),
            "case {case}: expected the first call's reason to be the recorded one, got {after}"
        );
    }
}

/// RULE: the revocation is recorded against the certificate the request named and
/// against no other — the sibling certificates of the same account keep their status.
#[tokio::test]
async fn revoking_one_certificate_leaves_its_siblings_alone() {
    let (router, engine) = router_with_engine();

    // Three exported, issued certificates; only the middle one is revoked.
    let mut arns = Vec::new();
    for index in 0..3 {
        let arn = issued_certificate(
            &router,
            &engine,
            &revocable_request(&format!("sibling-{index}.example.com"), &[]),
        )
        .await;
        mark_exported(&engine, &arn);
        arns.push(arn);
    }

    let target = arns[1].clone();
    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": target, "RevocationReason": "CESSATION_OF_OPERATION" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case sibling revoke: expected 200, got {status} {response}"
    );

    for arn in &arns {
        let detail = describe(&router, arn).await;
        let expected = if *arn == target { "REVOKED" } else { "ISSUED" };
        assert_eq!(
            detail["Status"].as_str(),
            Some(expected),
            "case {arn}: expected Status {expected} after revoking {target}, got {detail}"
        );
    }
}

/// RULE: a `RevocationReason` the enum does not declare is refused with HTTP 400
/// `ValidationException` and nothing is revoked — the request walk checks the modelled
/// enum before the handler runs.
#[tokio::test]
async fn an_unmodelled_revocation_reason_is_refused() {
    let (router, engine) = router_with_engine();
    let arn = issued_certificate(
        &router,
        &engine,
        &revocable_request("reason.example.com", &[]),
    )
    .await;
    mark_exported(&engine, &arn);

    // Rows differing along what the constraint generalises over: a value from a
    // neighbouring enum, a lower-cased modelled value, and the empty string.
    for reason in ["REVOKED", "key_compromise", ""] {
        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": reason }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {reason:?}: expected 400 ValidationException for an unmodelled \
             RevocationReason, got {status} {response}"
        );
        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("ISSUED"),
            "case {reason:?}: a refused reason must revoke nothing, got {detail}"
        );
    }

    // The same certificate still revokes for a modelled reason, so the rows above were
    // refused for their reason and not because this certificate could never be revoked.
    let (status, response) = call(
        &router,
        "RevokeCertificate",
        &json!({ "CertificateArn": arn, "RevocationReason": "PRIVILEGE_WITHDRAWN" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case modelled reason: expected 200, got {status} {response}"
    );
    let detail = describe(&router, &arn).await;
    assert_eq!(
        (
            detail["Status"].as_str(),
            detail["RevocationReason"].as_str()
        ),
        (Some("REVOKED"), Some("PRIVILEGE_WITHDRAWN")),
        "case modelled reason: expected the revocation recorded, got {detail}"
    );
}

/// RULE: `RevokeCertificate` resolves `CertificateArn` the way every certificate
/// operation does — an ARN that is not an ACM certificate ARN is `InvalidArnException`
/// before any lookup, and a well-formed one with no record behind it is
/// `ResourceNotFoundException` — and revokes nothing in either case.
#[tokio::test]
async fn revoking_an_unresolvable_arn_is_refused_before_any_write() {
    let (router, engine) = router_with_engine();
    let live = issued_certificate(
        &router,
        &engine,
        &revocable_request("live.example.com", &[]),
    )
    .await;
    mark_exported(&engine, &live);

    // Each row satisfies the member's modelled length and `@pattern`, so it reaches
    // the handler: first the ARNs that name no certificate at all, then well-formed
    // certificate ARNs with no record behind them.
    let cases: &[(&str, String, &str)] = &[
        (
            "a resource type that is not a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij"),
            "InvalidArnException",
        ),
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra"),
            "InvalidArnException",
        ),
        (
            "a UUID with nothing behind it",
            format!(
                "arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"
            ),
            "ResourceNotFoundException",
        ),
        (
            "the live certificate in another region",
            live.replace(REGION, "eu-central-1"),
            "ResourceNotFoundException",
        ),
        (
            "the live certificate under another account",
            live.replace(ACCOUNT, "123456789012"),
            "ResourceNotFoundException",
        ),
    ];

    for (case, arn, expected) in cases {
        let (status, response) = call(
            &router,
            "RevokeCertificate",
            &json!({ "CertificateArn": arn, "RevocationReason": "UNSPECIFIED" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected),
            "case {case}: expected 400 {expected} for {arn}, got {status} {response}"
        );
    }

    // None of those refusals touched the certificate that does exist.
    let detail = describe(&router, &live).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case live certificate: an unresolvable ARN must revoke nothing, got {detail}"
    );
}

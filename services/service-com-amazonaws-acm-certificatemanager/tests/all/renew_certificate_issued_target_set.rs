// SPDX-License-Identifier: CC0-1.0

//! Which certificates `RenewCertificate` resolves its `CertificateArn` against, end to
//! end over the wire (`POST /`, `X-Amz-Target: CertificateManager.RenewCertificate`,
//! awsJson1_1).
//!
//! RULE: a renewal re-issues a certificate ACM has ALREADY issued, so this operation's
//! `CertificateArn` addresses one of ACM's ISSUED certificates. An ARN that resolves to a
//! record in any other `CertificateStatus` — one still awaiting its domain validation,
//! one past that without being issued, or one that is no longer issued — is a MISS on
//! that set: 400 `ResourceNotFoundException` with `Could not find an issued certificate `
//! and the ARN the request carried. The verdict is read off the stored record and writes
//! nothing, so every concurrent caller takes it and the record keeps the status it had.
//! `RequestInProgressException` is left to the one condition on an ISSUED certificate
//! that means "wait": a renewal this operation already started and has not finished.
//!
//! The tables span what the rule generalises over: every `CertificateStatus` this service
//! can put a record in, how the record got there (a request still validating, a request
//! that failed up front, a request that waited its validation window out, a revocation),
//! and — on the other side of the rule — an ISSUED certificate that is renewable and one
//! that is not. Every row reads the record back with `DescribeCertificate`, so a refusal
//! that quietly wrote something cannot pass.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router_with_engine, ACCOUNT, REGION,
};
use crate::certificate_revocation::{mark_exported, revocable_request};
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// How many callers race one another in the concurrency rows: more than one, so the
/// refusal cannot be a verdict the first call wrote onto the record for the rest.
const RACERS: usize = 5;

/// The refusal sentence ACM answers for an ARN under which there is no issued
/// certificate: the fixed text, the ARN taken from the request, and no terminating
/// period.
fn expected_not_found_message(arn: &str) -> String {
    format!("Could not find an issued certificate {arn}")
}

/// `RenewCertificate` for one ARN, as `(status, body)`.
async fn renew(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "RenewCertificate", &json!({ "CertificateArn": arn })).await
}

/// Assert `arn` is refused as a miss on the issued certificates, naming the ARN the
/// request carried.
fn assert_not_an_issued_certificate(case: &str, arn: &str, status: u16, response: &Value) {
    assert_eq!(
        (status, error_code(response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: expected 400 ResourceNotFoundException, got {status} {response}"
    );
    assert_eq!(
        error_message(response),
        expected_not_found_message(arn),
        "case {case:?}: expected the refusal built from the request's ARN, got {response}"
    );
}

/// The `Status` `DescribeCertificate` reports for `arn`.
async fn status_of(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let detail = describe(router, arn).await;
    detail["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the certificate carried no Status: {detail}"))
        .to_string()
}

/// A certificate in each status a stored record can be in without ever having been
/// issued, plus one that was issued and then revoked — every one of them reached over the
/// wire on one store, with only the due instants the emulator itself stores moved.
async fn certificates_outside_the_issued_set(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<(&'static str, String, &'static str)> {
    // Issued, exported, then revoked: a certificate that WAS issued and is not any more.
    let revoked = request_certificate(
        router,
        &revocable_request("revoked.renew-target.probe-domain.com", &[]),
    )
    .await;
    make_validation_due_now(engine);
    mark_exported(engine, &revoked);
    let (status, response) = call(
        router,
        "RevokeCertificate",
        &json!({ "CertificateArn": &revoked, "RevocationReason": "KEY_COMPROMISE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "revoking an exported certificate must succeed, got {status} {response}"
    );

    // A name the public DNS delegates to nobody, which ACM refuses up front, and an
    // ordinary name whose evidence nobody published, which times out: both reach a
    // terminal status without ever being issued.
    let failed = request_certificate(
        router,
        &json!({ "DomainName": "failed.renew-target.invalid", "ValidationMethod": "EMAIL" }),
    )
    .await;
    let timed_out = request_certificate(
        router,
        &json!({ "DomainName": "timeout.renew-target.probe-domain.com", "ValidationMethod": "DNS" }),
    )
    .await;
    make_validation_due_now(engine);

    // Still waiting on proof of domain control. Created last, so holding the outcome it
    // is owed cannot hold back the rows above.
    let pending = request_certificate(
        router,
        &json!({ "DomainName": "pending.renew-target.probe-domain.com", "ValidationMethod": "DNS" }),
    )
    .await;
    hold_pending_validations(engine);

    let rows = vec![
        (
            "a request still awaiting domain validation",
            pending,
            "PENDING_VALIDATION",
        ),
        ("a request ACM refused up front", failed, "FAILED"),
        (
            "a request whose validation window ran out",
            timed_out,
            "VALIDATION_TIMED_OUT",
        ),
        (
            "a certificate that was issued and then revoked",
            revoked,
            "REVOKED",
        ),
    ];
    for (case, arn, expected) in &rows {
        assert_eq!(
            status_of(router, case, arn).await,
            *expected,
            "case {case:?}: the row must start in {expected}"
        );
    }
    rows
}

/// RULE: every ARN under which there is no ISSUED certificate is refused as a miss on
/// `RenewCertificate`'s target set, with the sentence built from the request's own ARN —
/// and the record it named is left exactly as it was.
#[tokio::test]
async fn an_arn_with_no_issued_certificate_is_not_found() {
    let (router, engine) = router_with_engine();
    let rows = certificates_outside_the_issued_set(&router, &engine).await;

    for (case, arn, expected_status) in &rows {
        let before = describe(&router, arn).await;
        let (status, response) = renew(&router, arn).await;
        assert_not_an_issued_certificate(case, arn, status, &response);

        let after = describe(&router, arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused renewal must leave the record alone, was {before} and \
             is now {after}"
        );
        assert_eq!(
            after["Status"].as_str(),
            Some(*expected_status),
            "case {case:?}: the certificate must still be {expected_status}, got {after}"
        );
        assert!(
            after["RenewalSummary"].is_null(),
            "case {case:?}: a refused renewal must record none, got {after}"
        );
    }

    // The ARNs that are not even a lookup on this set keep their own classifications, so
    // the refusal above is about the certificate's issuance and not about every refusal.
    let unresolved =
        format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555");
    let (status, response) = renew(&router, &unresolved).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case an ARN naming no certificate at all: expected 400 ResourceNotFoundException, \
         got {status} {response}"
    );
    assert_eq!(
        error_message(&response),
        format!("Could not find certificate {unresolved}."),
        "case an ARN naming no certificate at all: expected this service's lookup-miss \
         sentence, got {response}"
    );
    let other_resource = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij");
    let (status, response) = renew(&router, &other_resource).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidArnException"),
        "case an ARN naming another ACM resource type: expected 400 InvalidArnException, \
         got {status} {response}"
    );
}

/// RULE, the other side: an ISSUED certificate is a hit on the target set, so what
/// decides the answer there is ACM's renewal eligibility — and the one refusal that still
/// says "wait" is a renewal already in progress.
#[tokio::test]
async fn an_issued_certificate_is_judged_on_its_renewal_eligibility() {
    let (router, engine) = router_with_engine();

    // An imported certificate: ISSUED, and one ACM never renews.
    let generated = generate(&Blueprint {
        common_name: Some("imported.renew-target.probe-domain.com"),
        ..Blueprint::default()
    });
    let imported = import(&router, "an imported certificate", &import_body(&generated)).await;

    // An ACM-issued certificate that was never exported: ISSUED, and still INELIGIBLE.
    let unexported = request_certificate(
        &router,
        &json!({
            "DomainName": "unexported.renew-target.probe-domain.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    // An exported private certificate: the one ACM renews on request.
    let eligible = request_certificate(
        &router,
        &revocable_request("eligible.renew-target.probe-domain.com", &[]),
    )
    .await;
    make_validation_due_now(&engine);
    mark_exported(&engine, &eligible);

    for (case, arn) in [
        ("an imported certificate", &imported),
        ("an issued certificate that was never exported", &unexported),
    ] {
        assert_eq!(
            status_of(&router, case, arn).await,
            "ISSUED",
            "case {case:?}: the row must start ISSUED"
        );
        let (status, response) = renew(&router, arn).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: an ISSUED certificate ACM does not renew is refused on its \
             eligibility, got {status} {response}"
        );
        assert_eq!(
            describe(&router, arn).await["RenewalEligibility"].as_str(),
            Some("INELIGIBLE"),
            "case {case:?}: the eligibility it was refused for is the one that is reported"
        );
    }

    // The eligible one is renewed, and only then does the request-in-progress refusal
    // apply — to a certificate that IS in the target set and whose renewal is running.
    let case = "an exported private certificate";
    let (status, response) = renew(&router, &eligible).await;
    assert_eq!(
        status, 200,
        "case {case:?}: renewing an eligible certificate must succeed, got {status} {response}"
    );
    let running = describe(&router, &eligible).await;
    assert_eq!(
        running["RenewalSummary"]["RenewalStatus"].as_str(),
        Some("PENDING_AUTO_RENEWAL"),
        "case {case:?}: the started renewal is recorded on the certificate, got {running}"
    );
    let (status, response) = renew(&router, &eligible).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "RequestInProgressException"),
        "case {case:?}: a renewal already running is the condition that says wait, got \
         {status} {response}"
    );
}

/// RULE, concurrently: the refusal is per-call and stateless, so every one of several
/// simultaneous callers gets the same code and the same sentence, and the certificate
/// keeps the status it had throughout.
#[tokio::test]
async fn concurrent_renewals_outside_the_issued_set_all_take_the_same_refusal() {
    // One store per row: each row holds the whole store in the status it is about.
    for row in ["PENDING_VALIDATION", "VALIDATION_TIMED_OUT"] {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": "concurrent.renew-target.probe-domain.com",
                "ValidationMethod": "DNS",
            }),
        )
        .await;
        if row == "PENDING_VALIDATION" {
            hold_pending_validations(&engine);
        } else {
            make_validation_due_now(&engine);
        }
        assert_eq!(
            status_of(&router, row, &arn).await,
            row,
            "case {row:?}: the row must start in {row}"
        );

        let router = Arc::new(router);
        let mut racing = Vec::new();
        for _ in 0..RACERS {
            let racer = Arc::clone(&router);
            let arn = arn.clone();
            racing.push(tokio::spawn(
                async move { renew(&racer, &arn).await },
            ));
        }
        for racer in racing {
            let (status, response) = racer.await.expect("the concurrent caller completes");
            assert_not_an_issued_certificate(row, &arn, status, &response);
        }

        let after = describe(&router, &arn).await;
        assert_eq!(
            after["Status"].as_str(),
            Some(row),
            "case {row:?}: {RACERS} refused renewals must leave the status alone, got {after}"
        );
        assert!(
            after["RenewalSummary"].is_null(),
            "case {row:?}: {RACERS} refused renewals must record none, got {after}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The validation state machine of a requested ACM certificate, end to end over the
//! wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE under test: `PENDING_VALIDATION` is the status a request WAITS in, and what it
//! waits for is fixed by the request. The clock owes a terminal status — `ISSUED` when
//! the request needs no proof of domain ownership, `FAILED` with its `FailureReason`
//! when it names a domain the public DNS delegates to nobody, and
//! `VALIDATION_TIMED_OUT` at the end of the documented 72-hour validation window
//! otherwise — and every reader reports that outcome once it has fallen due, so a
//! client polling for a terminal status terminates.
//!
//! The table spans what the rule generalises over: both outcomes, every modelled
//! `ValidationMethod` and the absence of one, and one name as against several. Each
//! row runs on its own store, writes with `RequestCertificate` and reads back with
//! `DescribeCertificate` and `ListCertificates`, so nothing here can pass on a
//! response that was not computed from the persisted record.
//!
//! The emulator never sleeps for the due instant: it stores it. A row that needs the
//! outcome materialised moves that stored instant into the past — exactly the state
//! the wall clock would have left behind — and one test waits the real span out, to
//! pin that no call is needed for the outcome to become due.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    ComAmazonawsAcmCertificatemanagerState, VALIDATION_SETTLE_DELAY, VALIDATION_WINDOW,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{call, describe, request_certificate, router_with_engine};

/// A private certificate authority ARN of the form `CertificateAuthorityArn`'s
/// pattern requires (`^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`).
pub(crate) const PRIVATE_CA_ARN: &str =
    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/12345678-1234-1234-1234-123456789012";

/// How long a certificate this emulator issues is valid: ACM certificates are valid
/// for 13 months (395 days) (ACM user guide, "Managed certificate renewal").
const CERTIFICATE_VALIDITY_SECONDS: f64 = 395.0 * 24.0 * 60.0 * 60.0;

/// `FailureReason` of a request whose domain ownership could not be verified
/// (`com.amazonaws.acm#FailureReason`).
const ADDITIONAL_VERIFICATION_REQUIRED: &str = "ADDITIONAL_VERIFICATION_REQUIRED";

/// One row of the state-machine table: what it is, the `RequestCertificate` body,
/// the terminal `CertificateStatus` the clock owes the certificate it creates, the
/// `ValidationMethod` its domains are validated by (the request's, else the `EMAIL` ACM
/// falls back to), and the span that terminal status falls due within.
type OutcomeCase<'a> = (&'a str, Value, &'a str, &'a str, Duration);

/// How long until the stored validation outcome of `arn` falls due, or `None` when
/// the record owes none (it has already reached a terminal status).
///
/// Reads the very instant the emulator stored, so the test asserts the service's own
/// bookkeeping rather than a guess about it.
pub(crate) fn pending_due_in(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<Duration> {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let store = store.lock();
    let pending = store
        .certificates
        .get(arn)
        .unwrap_or_else(|| panic!("{arn} has no stored certificate"))
        .pending_validation
        .as_ref()?;
    Some(
        pending
            .due_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

/// Move every stored validation outcome into the past, so the next request
/// materialises it.
///
/// This writes only the due instant the emulator itself stores; the transition is
/// still applied by `materialize_due` on the next call, exactly as in production.
pub(crate) fn make_validation_due_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for certificate in store.certificates.values_mut() {
        if let Some(pending) = certificate.pending_validation.as_mut() {
            pending.due_at = past;
        }
    }
}

/// The `DomainValidationOptions` of a described certificate.
pub(crate) fn validations(case: &str, detail: &Value) -> Vec<Value> {
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"))
        .clone()
}

/// A numeric timestamp member, as epoch seconds.
fn timestamp(case: &str, detail: &Value, member: &str) -> f64 {
    detail[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {detail}"))
}

/// Assert everything the terminal status `expected` implies for a described
/// certificate: the members that come into existence with it, and the ones that
/// cannot exist without it. `method` is the `ValidationMethod` the certificate's
/// domains are validated by, which decides whether `ValidationEmails` applies at all.
fn assert_terminal(case: &str, detail: &Value, expected: &str, method: &str) {
    assert_eq!(
        detail["Status"].as_str(),
        Some(expected),
        "case {case}: expected the clock-owed terminal status {expected}, got {detail}"
    );

    match expected {        "ISSUED" => {
            // "This value exists only when the certificate status is FAILED"
            // (`CertificateDetail$FailureReason` doc).
            assert!(
                detail["FailureReason"].is_null(),
                "case {case}: an issued certificate carries no FailureReason, got {detail}"
            );
            for validation in validations(case, detail) {
                assert_eq!(
                    validation["ValidationStatus"].as_str(),
                    Some("SUCCESS"),
                    "case {case}: an issued certificate's domains are proven, got {validation}"
                );
            }
            // Issuance opens the validity window and creates the X.509 extensions.
            let issued_at = timestamp(case, detail, "IssuedAt");
            let not_before = timestamp(case, detail, "NotBefore");
            let not_after = timestamp(case, detail, "NotAfter");
            assert!(
                (issued_at - not_before).abs() < 1.0,
                "case {case}: expected NotBefore at the issuance instant, got {detail}"
            );
            assert!(
                (not_after - not_before - CERTIFICATE_VALIDITY_SECONDS).abs() < 1.0,
                "case {case}: expected a {CERTIFICATE_VALIDITY_SECONDS}s validity window, got {detail}"
            );
            let key_usages: Vec<&str> = detail["KeyUsages"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: no KeyUsages in {detail}"))
                .iter()
                .map(|usage| usage["Name"].as_str().unwrap_or_default())
                .collect();
            assert_eq!(
                key_usages,
                vec!["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"],
                "case {case}: expected the key-usage extensions of an issued certificate, got {detail}"
            );
            assert!(
                detail["ExtendedKeyUsages"]
                    .as_array()
                    .is_some_and(|usages| !usages.is_empty()),
                "case {case}: expected the extended key usages of an issued certificate, got {detail}"
            );
        }
        "FAILED" | "VALIDATION_TIMED_OUT" => {
            // "This value exists only when the certificate status is FAILED"
            // (`CertificateDetail$FailureReason` doc), so the reason is reported beside
            // the up-front failure and nowhere else — a request that waited the
            // validation window out carries none.
            if expected == "FAILED" {
                assert_eq!(
                    detail["FailureReason"].as_str(),
                    Some(ADDITIONAL_VERIFICATION_REQUIRED),
                    "case {case}: expected the FailureReason of an unverifiable request, got {detail}"
                );
            } else {
                assert!(
                    detail["FailureReason"].is_null(),
                    "case {case}: a timed-out request carries no FailureReason, got {detail}"
                );
            }
            for validation in validations(case, detail) {
                assert_eq!(
                    validation["ValidationStatus"].as_str(),
                    Some("FAILED"),
                    "case {case}: a domain left unproven is FAILED, got {validation}"
                );
                // "A list of email addresses that ACM used to send domain validation
                // emails" (`DomainValidation$ValidationEmails` doc): for DNS or HTTP
                // validation ACM sends no mail and the member does not apply.
                let emails = &validation["ValidationEmails"];
                if method == "EMAIL" {
                    assert!(
                        emails.is_array(),
                        "case {case}: an email-validated domain reports its mailboxes, \
                         got {validation}"
                    );
                } else {
                    assert!(
                        emails.is_null(),
                        "case {case}: {method} validation sends no mail, got {validation}"
                    );
                }
            }
            // Nothing was issued, so no issuance member exists. The X.509 key-usage
            // lists are not among them: a certificate ACM has not issued reports them
            // as EMPTY LISTS (the recorded DescribeCertificate response of a
            // not-yet-issued certificate carries `KeyUsages: []` and
            // `ExtendedKeyUsages: []`), not as missing members.
            for absent in ["IssuedAt", "NotBefore", "NotAfter"] {
                assert!(
                    detail[absent].is_null(),
                    "case {case}: {absent} cannot exist without issuance, got {detail}"
                );
            }
            for empty in ["KeyUsages", "ExtendedKeyUsages"] {
                assert_eq!(
                    detail[empty],
                    json!([]),
                    "case {case}: expected {empty} to be the empty list without issuance, got {detail}"
                );
            }
        }
        other => panic!("case {case}: the table named an unexpected terminal status {other}"),
    }
}

/// Every certificate ARN `ListCertificates` reports for a status filter.
pub(crate) async fn arns_in_statuses(router: &MultiServiceRouter, statuses: &[&str]) -> Vec<String> {
    let body = json!({ "CertificateStatuses": statuses, "MaxItems": 100 });
    let (status, response) = call(router, "ListCertificates", &body).await;
    assert_eq!(
        status, 200,
        "case {body}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {body}: no CertificateSummaryList in {response}"))
        .iter()
        .map(|summary| {
            summary["CertificateArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: a requested certificate is `PENDING_VALIDATION` until the clock owes it a
/// terminal status — `ISSUED` where no proof of domain ownership is required, `FAILED`
/// with `ADDITIONAL_VERIFICATION_REQUIRED` where the name is one the public DNS
/// delegates to nobody, and `VALIDATION_TIMED_OUT` at the end of the documented 72-hour
/// validation window for a name whose holder could have published the evidence — and
/// every reader reports the outcome once it is due.
#[tokio::test]
async fn a_requested_certificate_reaches_a_terminal_status() {
    // Rows differ along what the rule generalises over: the validation method asked
    // for (every modelled value, and none), the number of names covered, whether the
    // request names a private certificate authority — the one case that needs no domain
    // validation at all, and so the one that resolves to ISSUED — and whether the names
    // it covers are ones the public DNS delegates to nobody (which ACM refuses up
    // front) or ordinary ones (which it waits the validation window out for).
    let cases: &[OutcomeCase<'_>] = &[
        (
            "DNS validation, one undelegated name",
            json!({ "DomainName": "dns.invalid", "ValidationMethod": "DNS" }),
            "FAILED",
            "DNS",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "EMAIL validation, two undelegated names",
            json!({
                "DomainName": "email.localhost",
                "SubjectAlternativeNames": ["mail.email.localhost"],
                "ValidationMethod": "EMAIL",
            }),
            "FAILED",
            "EMAIL",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "HTTP validation, undelegated wildcard name",
            json!({ "DomainName": "*.http.test", "ValidationMethod": "HTTP" }),
            "FAILED",
            "HTTP",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "no validation method named, undelegated name",
            json!({ "DomainName": "defaulted.example" }),
            "FAILED",
            "EMAIL",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "DNS validation, one ordinary name",
            json!({ "DomainName": "dns.probe-domain.com", "ValidationMethod": "DNS" }),
            "VALIDATION_TIMED_OUT",
            "DNS",
            VALIDATION_WINDOW,
        ),
        (
            "EMAIL validation with a validation domain and three ordinary names",
            json!({
                "DomainName": "a.deep.probe-domain.io",
                "SubjectAlternativeNames": ["b.deep.probe-domain.io", "c.deep.probe-domain.io"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.deep.probe-domain.io", "ValidationDomain": "probe-domain.io" }
                ],
            }),
            "VALIDATION_TIMED_OUT",
            "EMAIL",
            VALIDATION_WINDOW,
        ),
        (
            "HTTP validation, an ordinary wildcard name",
            json!({ "DomainName": "*.http.probe-domain.net", "ValidationMethod": "HTTP" }),
            "VALIDATION_TIMED_OUT",
            "HTTP",
            VALIDATION_WINDOW,
        ),
        (
            "one undelegated name beside an ordinary one",
            json!({
                "DomainName": "mixed.probe-domain.com",
                "SubjectAlternativeNames": ["mixed.invalid"],
                "ValidationMethod": "DNS",
            }),
            "FAILED",
            "DNS",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "private certificate authority, DNS validation asked for",
            json!({
                "DomainName": "private.example.com",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "ISSUED",
            "DNS",
            VALIDATION_SETTLE_DELAY,
        ),
        (
            "private certificate authority, two names and no method",
            json!({
                "DomainName": "pki.example.com",
                "SubjectAlternativeNames": ["*.pki.example.com"],
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "ISSUED",
            "EMAIL",
            VALIDATION_SETTLE_DELAY,
        ),
    ];

    for (case, body, expected, method, due_within) in cases {
        // A fresh store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;

        // The pending status is real: the span is never zero, so a caller reading
        // straight after the request still sees PENDING_VALIDATION and its evidence.
        let pending = describe(&router, &arn).await;
        assert_eq!(
            pending["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: a requested certificate enters PENDING_VALIDATION, got {pending}"
        );
        assert!(
            pending["FailureReason"].is_null(),
            "case {case}: nothing has failed yet, got {pending}"
        );
        for validation in validations(case, &pending) {
            assert_eq!(
                validation["ValidationStatus"].as_str(),
                Some("PENDING_VALIDATION"),
                "case {case}: an unproven domain is PENDING_VALIDATION, got {validation}"
            );
        }
        let due_in = pending_due_in(&engine, &arn)
            .unwrap_or_else(|| panic!("case {case}: the request owed no validation outcome"));
        assert!(
            due_in > Duration::ZERO && due_in <= *due_within,
            "case {case}: expected an outcome due within {due_within:?} and not instantly, got {due_in:?}"
        );

        // Once the stored instant has passed, the outcome is owed whether or not
        // anybody polled, and the next read reports it.
        make_validation_due_now(&engine);
        let terminal = describe(&router, &arn).await;
        assert_terminal(case, &terminal, expected, method);

        // A terminal status is terminal: reading again neither moves it on nor
        // re-opens the validation.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, terminal,
            "case {case}: a terminal certificate must read the same every time, got {again}"
        );
        assert!(
            pending_due_in(&engine, &arn).is_none(),
            "case {case}: a terminal certificate owes the clock nothing further"
        );

        // The other reader agrees: the status filter selects on the materialised
        // status, not on the one the certificate was created in.
        let in_terminal = arns_in_statuses(&router, &[expected]).await;
        assert_eq!(
            in_terminal,
            vec![arn.clone()],
            "case {case}: expected ListCertificates to report the certificate as {expected}, got {in_terminal:?}"
        );
        let still_pending = arns_in_statuses(&router, &["PENDING_VALIDATION"]).await;
        assert!(
            still_pending.is_empty(),
            "case {case}: no certificate is still pending validation, got {still_pending:?}"
        );
    }
}

/// RULE: the terminal status is owed by the CLOCK, not by the poll — the outcome falls
/// due a bounded span after `RequestCertificate` whether or not anybody calls in
/// between, so a client that polls `DescribeCertificate` for a terminal status always
/// terminates.
#[tokio::test]
async fn a_poll_for_a_terminal_status_terminates() {
    let (router, _engine) = router_with_engine();
    // Taken before the request, so the span measured here starts no later than the
    // instant the service dates the certificate from.
    let started = std::time::Instant::now();
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "polled.invalid", "ValidationMethod": "DNS" }),
    )
    .await;

    // Nothing touches the store between the request and the first poll: if the
    // transition needed a call to start it, this loop would never terminate.
    // A cap far above the emulator's own span, so an expired budget is a real failure
    // rather than a tight-timing artefact.
    let budget = VALIDATION_SETTLE_DELAY * 20;
    let mut polls = 0_u32;
    let terminal = loop {
        let detail = describe(&router, &arn).await;
        let status = detail["Status"].as_str().unwrap_or_default().to_string();
        if status != "PENDING_VALIDATION" {
            break status;
        }
        polls += 1;
        assert!(
            started.elapsed() < budget,
            "polling for a terminal status gave up after {polls} polls in {:?}: still {detail}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    assert_eq!(
        terminal, "FAILED",
        "a public certificate whose domains cannot be verified ends FAILED, got {terminal}"
    );
    assert!(
        polls > 0,
        "the transient PENDING_VALIDATION status must be observable before the outcome falls due"
    );
    assert!(
        started.elapsed() >= VALIDATION_SETTLE_DELAY,
        "the outcome must not fall due before its span has passed, got {:?}",
        started.elapsed()
    );

    // The outcome a poll observed is the stored one, and the service never slept for
    // it: a second reader sees the same record.
    let detail = describe(&router, &arn).await;
    assert_terminal("polled certificate", &detail, "FAILED", "DNS");
}

/// RULE: the X.509 key-usage extensions exist only once a certificate has been
/// issued, so `ListCertificates`' `Includes.keyUsage` filter finds an issued
/// certificate and never a pending or failed one.
#[tokio::test]
async fn key_usage_filters_find_only_issued_certificates() {
    let (router, engine) = router_with_engine();
    let issued = request_certificate(
        &router,
        &json!({
            "DomainName": "issued.example.com",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;
    let failed = request_certificate(
        &router,
        &json!({ "DomainName": "unverifiable.invalid", "ValidationMethod": "DNS" }),
    )
    .await;

    // While both are pending, neither carries any extension.
    let pending_hits = key_usage_hits(&router, &json!({ "keyUsage": ["DIGITAL_SIGNATURE"] })).await;
    assert!(
        pending_hits.is_empty(),
        "case pending: a certificate carries no key-usage extensions before issuance, got {pending_hits:?}"
    );

    make_validation_due_now(&engine);

    // Rows: an extension the issued certificate carries, one it does not, and an
    // extended key usage — every one of which the failed certificate must miss.
    let filter_cases: &[(&str, Value, bool)] = &[
        (
            "a key usage the issued certificate carries",
            json!({ "keyUsage": ["DIGITAL_SIGNATURE"] }),
            true,
        ),
        (
            "two key usages, both carried",
            json!({ "keyUsage": ["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"] }),
            true,
        ),
        (
            "a key usage no ACM certificate carries",
            json!({ "keyUsage": ["CERTIFICATE_SIGNING"] }),
            false,
        ),
        (
            "an extended key usage the issued certificate carries",
            json!({ "extendedKeyUsage": ["TLS_WEB_SERVER_AUTHENTICATION"] }),
            true,
        ),
        (
            "an extended key usage it does not carry",
            json!({ "extendedKeyUsage": ["CODE_SIGNING"] }),
            false,
        ),
    ];
    for (case, includes, matches) in filter_cases {
        let hits = key_usage_hits(&router, includes).await;
        let expected: Vec<String> = if *matches {
            vec![issued.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            hits, expected,
            "case {case}: expected {expected:?} from Includes={includes}, got {hits:?}"
        );
        assert!(
            !hits.contains(&failed),
            "case {case}: a failed certificate carries no extensions, got {hits:?}"
        );
    }
}

/// The ARNs `ListCertificates` reports for one `Includes` filter.
async fn key_usage_hits(router: &MultiServiceRouter, includes: &Value) -> Vec<String> {
    let body = json!({ "Includes": includes, "MaxItems": 100 });
    let (status, response) = call(router, "ListCertificates", &body).await;
    assert_eq!(
        status, 200,
        "case {body}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {body}: no CertificateSummaryList in {response}"))
        .iter()
        .map(|summary| {
            summary["CertificateArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

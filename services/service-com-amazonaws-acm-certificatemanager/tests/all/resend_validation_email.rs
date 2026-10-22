// SPDX-License-Identifier: CC0-1.0

//! `ResendValidationEmail` end to end over the wire (`POST /`,
//! `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULES under test, each asserted over a table spanning the class it generalises
//! over rather than one recorded request:
//!
//! 1. A resend re-sends ONE domain's approval mail: the `ValidationDomain` the request
//!    names becomes the suffix of the addresses ACM used for that domain, so a later
//!    `DescribeCertificate` reports it and the five mailboxes at it — and reports every
//!    other domain's validation exactly as it was.
//! 2. `ValidationDomain` must be `Domain` or a superdomain of it, and `Domain` must be
//!    a name the certificate covers; either is
//!    `InvalidDomainValidationOptionsException` (HTTP 400) and leaves the stored
//!    validations untouched.
//! 3. There is mail to resend only while the certificate is still
//!    `PENDING_VALIDATION`, that domain is awaiting proof BY EMAIL, and the documented
//!    72-hour window since the mail was last sent has not elapsed; otherwise
//!    `InvalidStateException` (HTTP 400).
//!
//! Every call goes through `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`], and every success is read back through
//! `DescribeCertificate`, so nothing here can pass on a response the store does not
//! back. A row that needs the certificate to stay `PENDING_VALIDATION` holds the
//! stored due instant out (the emulator's own bookkeeping), and a row about the
//! 72-hour window moves the stored last-sent instant into the past, so neither races
//! the wall clock.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    ComAmazonawsAcmCertificatemanagerState, RESEND_VALIDATION_WINDOW,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router_with_engine, ACCOUNT, REGION,
};
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// The mailbox local-parts ACM sends a domain's approval mail to: "ACM sends email to
/// the the following five addresses: admin@subdomain.example.com
/// administrator@subdomain.example.com hostmaster@subdomain.example.com
/// postmaster@subdomain.example.com webmaster@subdomain.example.com"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#ResendValidationEmailRequest",
/// member="ValidationDomain")`). Compared as a SET, so the order ACM lists them in is
/// not asserted.
const VALIDATION_LOCAL_PARTS: &[&str] = &[
    "admin",
    "administrator",
    "hostmaster",
    "postmaster",
    "webmaster",
];

/// One accepted-resend row: what it exercises, the `RequestCertificate` body, the
/// `Domain` to resend for, the `ValidationDomain` to resend through, and the mail
/// suffix the stored validation must carry afterwards.
type ResendCase<'a> = (&'a str, Value, &'a str, &'a str, &'a str);

/// One refused-resend row: what it exercises, the `Domain`, the `ValidationDomain`,
/// and the value the refusal must name (the one the rule rejected).
type RefusalCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// The mailboxes the rule requires at `mail_suffix`, lower-cased and sorted.
fn expected_mailboxes(mail_suffix: &str) -> Vec<String> {
    let suffix = mail_suffix.to_ascii_lowercase();
    let mut mailboxes: Vec<String> = VALIDATION_LOCAL_PARTS
        .iter()
        .map(|local_part| format!("{local_part}@{suffix}"))
        .collect();
    mailboxes.sort();
    mailboxes
}

/// A validation's reported `ValidationEmails`, lower-cased and sorted.
fn reported_mailboxes(case: &str, validation: &Value) -> Vec<String> {
    let mut mailboxes: Vec<String> = validation["ValidationEmails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ValidationEmails in {validation}"))
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_ascii_lowercase())
        .collect();
    mailboxes.sort();
    mailboxes
}

/// The `DomainValidation` entry a described certificate reports for `domain`.
fn validation_for(case: &str, detail: &Value, domain: &str) -> Value {
    detail["DomainValidationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no DomainValidationOptions in {detail}"))
        .iter()
        .find(|validation| {
            validation["DomainName"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(domain))
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no validation for {domain} in {detail}"))
}

/// Every validation entry EXCEPT the one for `domain`, so a resend's effect can be
/// pinned to the single domain it named.
fn validations_other_than(detail: &Value, domain: &str) -> Vec<Value> {
    detail["DomainValidationOptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|validation| {
            validation["DomainName"]
                .as_str()
                .is_none_or(|name| !name.eq_ignore_ascii_case(domain))
        })
        .collect()
}

/// Request a certificate and hold it in `PENDING_VALIDATION` for the rest of the row,
/// so the state the resend rules are about is the one under test.
async fn pending_certificate(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    body: &Value,
) -> String {
    let arn = request_certificate(router, body).await;
    hold_pending_validations(engine);
    arn
}

/// Move the instant the certificate's validation mail was last sent `age` into the
/// past.
///
/// This writes only the instant the emulator itself stores, so the documented
/// 72-hour window is exercised against the service's own bookkeeping instead of a
/// test that waits three days.
fn backdate_validation_email(engine: &Arc<engine::InMemoryEngine>, arn: &str, age: Duration) {
    let store = engine.state::<ComAmazonawsAcmCertificatemanagerState>();
    let mut store = store.lock();
    let certificate = store
        .certificates
        .get_mut(arn)
        .unwrap_or_else(|| panic!("{arn} has no stored certificate"));
    certificate.validation_email_sent_at = Some(SystemTime::now() - age);
}

/// Call `ResendValidationEmail` for one `(arn, domain, validation domain)`.
async fn resend(
    router: &MultiServiceRouter,
    arn: &str,
    domain: &str,
    validation_domain: &str,
) -> (u16, Value) {
    call(
        router,
        "ResendValidationEmail",
        &json!({
            "CertificateArn": arn,
            "Domain": domain,
            "ValidationDomain": validation_domain,
        }),
    )
    .await
}

/// RULE: a resend records the mail ACM sent — the named `ValidationDomain`, without a
/// wildcard label, becomes the suffix of that domain's `ValidationDomain` and of the
/// five mailboxes its `ValidationEmails` reports — and touches no other domain of the
/// certificate.
#[tokio::test]
async fn a_resend_records_the_mail_it_sent() {
    // Rows differ along what the rule generalises over: the depth of the validation
    // domain (the name itself, one label up, two), which covered name is resent for
    // (the DomainName, a subject alternative name), the form of that name (plain,
    // wildcard), the casing of both, and whether the certificate already carried a
    // validation domain from its request.
    let cases: &[ResendCase<'_>] = &[
        (
            "the domain itself",
            json!({ "DomainName": "site.example.com", "ValidationMethod": "EMAIL" }),
            "site.example.com",
            "site.example.com",
            "site.example.com",
        ),
        (
            "one label up",
            json!({ "DomainName": "site.example.com", "ValidationMethod": "EMAIL" }),
            "site.example.com",
            "example.com",
            "example.com",
        ),
        (
            "two labels up from a deep name",
            json!({ "DomainName": "a.b.example.net", "ValidationMethod": "EMAIL" }),
            "a.b.example.net",
            "example.net",
            "example.net",
        ),
        (
            "a subject alternative name rather than the DomainName",
            json!({
                "DomainName": "www.example.org",
                "SubjectAlternativeNames": ["mail.example.org"],
                "ValidationMethod": "EMAIL",
            }),
            "mail.example.org",
            "example.org",
            "example.org",
        ),
        (
            // A mail suffix carries no wildcard label: `*.wild.example.io` is mailed
            // at `wild.example.io`, the domain ACM proves control of.
            "a wildcard name validated through itself",
            json!({
                "DomainName": "x.example.io",
                "SubjectAlternativeNames": ["*.wild.example.io"],
                "ValidationMethod": "EMAIL",
            }),
            "*.wild.example.io",
            "*.wild.example.io",
            "wild.example.io",
        ),
        (
            "a wildcard name validated through its base",
            json!({
                "DomainName": "*.wild.example.co",
                "ValidationMethod": "EMAIL",
            }),
            "*.wild.example.co",
            "example.co",
            "example.co",
        ),
        (
            // DNS names are case-insensitive, so casing never decides the verdict.
            "both names in another case",
            json!({ "DomainName": "cased.example.com", "ValidationMethod": "EMAIL" }),
            "CASED.Example.COM",
            "ExAmPlE.CoM",
            "example.com",
        ),
        (
            // The request already chose a validation domain; the resend re-points the
            // mail, so the stored suffix must be the resend's and not the request's.
            "re-pointing a validation domain the request chose",
            json!({
                "DomainName": "n.deep.example.com",
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "n.deep.example.com", "ValidationDomain": "deep.example.com" }
                ],
            }),
            "n.deep.example.com",
            "example.com",
            "example.com",
        ),
    ];

    for (case, body, domain, validation_domain, mail_suffix) in cases {
        // A store per row, so one row's certificate cannot explain another's.
        let (router, engine) = router_with_engine();
        let arn = pending_certificate(&router, &engine, body).await;
        let before = describe(&router, &arn).await;
        let untouched_before = validations_other_than(&before, domain);

        let (status, response) = resend(&router, &arn, domain, validation_domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ResendValidationEmail must succeed for {validation_domain}, \
             got {status} {response}"
        );

        // Read the stored record back: the resend's effect is what DescribeCertificate
        // reports, not what the response echoed (it carries no members at all).
        let after = describe(&router, &arn).await;
        let validation = validation_for(case, &after, domain);
        assert!(
            validation["ValidationDomain"]
                .as_str()
                .is_some_and(|stored| stored.eq_ignore_ascii_case(mail_suffix)),
            "case {case:?}: expected the stored ValidationDomain to be {mail_suffix}, \
             got {validation}"
        );
        let mailboxes = reported_mailboxes(case, &validation);
        assert_eq!(
            mailboxes,
            expected_mailboxes(mail_suffix),
            "case {case:?}: expected the five validation mailboxes at {mail_suffix}, \
             got {mailboxes:?}"
        );
        // A resend sends mail; it proves nothing, so the domain and the certificate
        // are still waiting.
        assert_eq!(
            validation["ValidationStatus"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: a resent domain is still awaiting proof, got {validation}"
        );
        assert_eq!(
            after["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: a resend does not move the certificate's status, got {after}"
        );
        // Only the named domain was mailed again.
        let untouched_after = validations_other_than(&after, domain);
        assert_eq!(
            untouched_after, untouched_before,
            "case {case:?}: a resend must leave every other domain's validation as it \
             was, got {untouched_after:?}"
        );
    }
}

/// RULE: a resend is an ATTEMPT, repeatable while the certificate waits — each call
/// records the mail it sent, so the stored suffix is the latest one asked for.
#[tokio::test]
async fn repeated_resends_record_the_latest_validation_domain() {
    let (router, engine) = router_with_engine();
    let arn = pending_certificate(
        &router,
        &engine,
        &json!({ "DomainName": "repeat.sub.example.com", "ValidationMethod": "EMAIL" }),
    )
    .await;

    // Every suffix the superdomain rule allows for this name, asked for in turn.
    for suffix in ["repeat.sub.example.com", "sub.example.com", "example.com"] {
        let (status, response) = resend(&router, &arn, "repeat.sub.example.com", suffix).await;
        assert_eq!(
            status, 200,
            "case {suffix:?}: a repeated resend must succeed, got {status} {response}"
        );
        let detail = describe(&router, &arn).await;
        let validation = validation_for(suffix, &detail, "repeat.sub.example.com");
        assert_eq!(
            validation["ValidationDomain"].as_str(),
            Some(suffix),
            "case {suffix:?}: expected the latest validation domain, got {validation}"
        );
        let mailboxes = reported_mailboxes(suffix, &validation);
        assert_eq!(
            mailboxes,
            expected_mailboxes(suffix),
            "case {suffix:?}: expected the mailboxes at the latest suffix, got {mailboxes:?}"
        );
    }
}

/// RULE: `ValidationDomain` must be `Domain` or a superdomain of it, and `Domain` must
/// be a name the certificate covers; anything else is
/// `InvalidDomainValidationOptionsException` (HTTP 400) and the stored validations are
/// left exactly as they were.
#[tokio::test]
async fn an_incorrect_domain_pair_is_refused() {
    // The certificate every row resends against: a deep name plus a wildcard and a
    // sibling, so a row can name a covered name, a name one label off it, or a name
    // the certificate does not cover at all.
    let request = json!({
        "DomainName": "probe.sub.example.com",
        "SubjectAlternativeNames": ["*.wild.example.com", "other.example.com"],
        "ValidationMethod": "EMAIL",
    });

    // Rows span both halves of the rule: a ValidationDomain that is not above the
    // Domain (off the label boundary, a sibling, a subdomain, an unrelated name,
    // another TLD's parent, a wildcard suffix), and a Domain the certificate does not
    // cover (a sibling, a superdomain of a covered name, a subdomain of one, an
    // unrelated name) — each with a ValidationDomain the first half accepts, so the
    // refusal can only come from the coverage check.
    let cases: &[RefusalCase<'_>] = &[
        (
            "a suffix off the label boundary",
            "probe.sub.example.com",
            "ample.com",
            "ample.com",
        ),
        (
            "a sibling of the domain",
            "probe.sub.example.com",
            "other.sub.example.com",
            "other.sub.example.com",
        ),
        (
            "a subdomain of the domain, the wrong direction",
            "probe.sub.example.com",
            "deep.probe.sub.example.com",
            "deep.probe.sub.example.com",
        ),
        (
            "an unrelated registrable domain",
            "probe.sub.example.com",
            "totally-unrelated.org",
            "totally-unrelated.org",
        ),
        (
            "another TLD's parent",
            "probe.sub.example.com",
            "example.org",
            "example.org",
        ),
        (
            "a wildcard validation domain of a plain name",
            "probe.sub.example.com",
            "*.example.com",
            "*.example.com",
        ),
        (
            "a domain the certificate does not cover",
            "absent.example.com",
            "example.com",
            "absent.example.com",
        ),
        (
            "a superdomain of a covered name is not itself covered",
            "sub.example.com",
            "example.com",
            "sub.example.com",
        ),
        (
            "a subdomain of a covered name is not itself covered",
            "deep.probe.sub.example.com",
            "example.com",
            "deep.probe.sub.example.com",
        ),
        (
            "a name covered only by the certificate's wildcard",
            "host.wild.example.com",
            "wild.example.com",
            "host.wild.example.com",
        ),
    ];

    for (case, domain, validation_domain, named) in cases {
        let (router, engine) = router_with_engine();
        let arn = pending_certificate(&router, &engine, &request).await;
        let before = describe(&router, &arn).await;

        let (status, response) = resend(&router, &arn, domain, validation_domain).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDomainValidationOptionsException"),
            "case {case:?}: expected 400 InvalidDomainValidationOptionsException for \
             Domain={domain} ValidationDomain={validation_domain}, got {status} {response}"
        );
        // The refusal names the value it rejected, so a caller can tell which half of
        // the rule it broke.
        let message = error_message(&response);
        assert!(
            message.contains(named),
            "case {case:?}: the refusal must name the rejected {named}, got {message:?}"
        );

        // A refused resend sent nothing, so the stored validations are untouched.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after["DomainValidationOptions"], before["DomainValidationOptions"],
            "case {case:?}: a refused resend must leave the stored validations as they \
             were, got {after}"
        );
    }
}

/// RULE: there is validation mail to resend only while the certificate is
/// `PENDING_VALIDATION` and the domain is awaiting proof BY EMAIL; a certificate in a
/// terminal status, or a domain validated by DNS or HTTP, is
/// `InvalidStateException` (HTTP 400).
#[tokio::test]
async fn only_a_domain_awaiting_email_approval_has_mail_to_resend() {
    // (what it exercises, the RequestCertificate body, whether the row needs the
    // validation outcome materialised, the domain to resend for)
    let cases: &[(&str, Value, bool, &str)] = &[
        (
            "a pending domain validated by DNS",
            json!({ "DomainName": "dns.example.com", "ValidationMethod": "DNS" }),
            false,
            "dns.example.com",
        ),
        (
            "a pending domain validated by HTTP",
            json!({ "DomainName": "http.example.com", "ValidationMethod": "HTTP" }),
            false,
            "http.example.com",
        ),
        (
            "a certificate whose request has FAILED",
            json!({ "DomainName": "failed.example.com", "ValidationMethod": "EMAIL" }),
            true,
            "failed.example.com",
        ),
        (
            // A private certificate needs no domain validation, so it is ISSUED and
            // has no approval mail of its own.
            "an ISSUED certificate",
            json!({
                "DomainName": "issued.example.com",
                "ValidationMethod": "EMAIL",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            true,
            "issued.example.com",
        ),
    ];

    for (case, body, materialise, domain) in cases {
        let (router, engine) = router_with_engine();
        let arn = request_certificate(&router, body).await;
        if *materialise {
            make_validation_due_now(&engine);
        } else {
            hold_pending_validations(&engine);
        }
        let before = describe(&router, &arn).await;

        let (status, response) = resend(&router, &arn, domain, domain).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidStateException"),
            "case {case:?}: expected 400 InvalidStateException, got {status} {response}"
        );
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused resend must change nothing, got {after}"
        );
    }
}

/// RULE: the resend window is the documented 72 hours since the validation mail was
/// last sent — inside it a resend succeeds, past it there is nothing left to resend
/// and the answer is `InvalidStateException`. Each published bound is exercised, and
/// one step past it.
#[tokio::test]
async fn the_resend_window_closes_72_hours_after_the_last_mail() {
    // (what it exercises, how long ago the mail was last sent, whether a resend still
    // works). The bound itself is `RESEND_VALIDATION_WINDOW`, read from the service so
    // the table cannot drift from the constant the doc sentence fixed.
    let cases: &[(&str, Duration, bool)] = &[
        ("sent just now", Duration::ZERO, true),
        ("sent an hour ago", Duration::from_secs(3600), true),
        (
            "sent a second inside the window",
            RESEND_VALIDATION_WINDOW - Duration::from_secs(1),
            true,
        ),
        (
            "sent a second past the window",
            RESEND_VALIDATION_WINDOW + Duration::from_secs(1),
            false,
        ),
        (
            "sent a week ago",
            Duration::from_secs(7 * 24 * 60 * 60),
            false,
        ),
    ];

    assert_eq!(
        RESEND_VALIDATION_WINDOW,
        Duration::from_secs(72 * 60 * 60),
        "the documented resend window is 72 hours, got {RESEND_VALIDATION_WINDOW:?}"
    );

    for (case, age, accepted) in cases {
        let (router, engine) = router_with_engine();
        let arn = pending_certificate(
            &router,
            &engine,
            &json!({ "DomainName": "window.example.com", "ValidationMethod": "EMAIL" }),
        )
        .await;
        backdate_validation_email(&engine, &arn, *age);

        let (status, response) = resend(&router, &arn, "window.example.com", "example.com").await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a resend inside the 72-hour window must succeed, \
                 got {status} {response}"
            );
            let detail = describe(&router, &arn).await;
            let validation = validation_for(case, &detail, "window.example.com");
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some("example.com"),
                "case {case:?}: expected the resent validation domain, got {validation}"
            );
            // The accepted resend restarted the window: another one works right after.
            let (again, response) =
                resend(&router, &arn, "window.example.com", "window.example.com").await;
            assert_eq!(
                again, 200,
                "case {case:?}: an accepted resend restarts the window, got {again} {response}"
            );
        } else {
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidStateException"),
                "case {case:?}: expected 400 InvalidStateException past the 72-hour \
                 window, got {status} {response}"
            );
            let detail = describe(&router, &arn).await;
            let validation = validation_for(case, &detail, "window.example.com");
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some("window.example.com"),
                "case {case:?}: a refused resend must leave the stored validation \
                 domain as it was, got {validation}"
            );
        }
    }
}

/// RULE: a resend is scoped to the certificate it names — one certificate's resend
/// changes no other certificate's stored validation, even for the same domain name.
#[tokio::test]
async fn a_resend_touches_only_the_certificate_it_names() {
    let (router, engine) = router_with_engine();
    let body = json!({ "DomainName": "shared.sub.example.com", "ValidationMethod": "EMAIL" });
    let first = pending_certificate(&router, &engine, &body).await;
    let second = pending_certificate(&router, &engine, &body).await;
    assert_ne!(
        first, second,
        "two requests without an idempotency token create two certificates"
    );

    let (status, response) = resend(&router, &first, "shared.sub.example.com", "example.com").await;
    assert_eq!(
        status, 200,
        "case the named certificate: the resend must succeed, got {status} {response}"
    );

    let resent = validation_for(
        "the named certificate",
        &describe(&router, &first).await,
        "shared.sub.example.com",
    );
    assert_eq!(
        resent["ValidationDomain"].as_str(),
        Some("example.com"),
        "case the named certificate: expected the resent suffix, got {resent}"
    );

    let other = validation_for(
        "the other certificate",
        &describe(&router, &second).await,
        "shared.sub.example.com",
    );
    assert_eq!(
        other["ValidationDomain"].as_str(),
        Some("shared.sub.example.com"),
        "case the other certificate: its validation must be untouched, got {other}"
    );
    let mailboxes = reported_mailboxes("the other certificate", &other);
    assert!(
        mailboxes.is_empty(),
        "case the other certificate: its mailbox list must be untouched — nothing has \
         mailed it, so it names no address — got {mailboxes:?}"
    );

    // And an ARN with no record behind it is a lookup failure, not a resend: the
    // shared ARN-resolution table in `certificate_not_found_message` covers the
    // sentence; this pins that the operation reaches that path at all.
    let missing = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/00000000-0000-0000-0000-00000000dead");
    let (status, response) = resend(&router, &missing, "shared.sub.example.com", "example.com").await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case an unknown ARN: expected 400 ResourceNotFoundException, got {status} {response}"
    );
}

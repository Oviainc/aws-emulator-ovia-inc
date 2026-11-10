// SPDX-License-Identifier: CC0-1.0

//! What a client sees while the transitions ACM owes a REQUESTED certificate are still
//! owed, end to end over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`,
//! awsJson1_1).
//!
//! RULE under test: every transition this emulator owes a requested certificate on its
//! own — the validation outcome it reaches without waiting on proof of domain control,
//! and the initial domain-validation mail — falls due a span after `RequestCertificate`
//! that is longer than the calls a client makes next, so the transient
//! `PENDING_VALIDATION`, not-yet-mailed state and every refusal keyed on it are what
//! that client observes. The spans are stored and never slept on, so the span's LENGTH
//! is the only thing that decides it: one shorter than a client's own call sequence
//! answers a fast client one way and a slow client another for the identical requests.
//! The narrower of the two spans is what bounds the window, because an outcome that
//! falls due first drops the owed mail
//! (`service_com_amazonaws_acm_certificatemanager::state::Certificate::materialize_due`),
//! so the rule holds of BOTH spans and not just of the dispatch.
//!
//! The rows below span what the rule generalises over: the validation method (named and
//! defaulted), the number and form of the covered names (one name, several, a wildcard),
//! whether the request chose the mail suffix itself, and — the axis the spans differ
//! along — whether the name is one whose control could ever be proven, since a request
//! for a name the public DNS delegates to nobody is the one whose owed OUTCOME closes
//! the window instead of the owed mail.
//!
//! Every call goes through `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`], and each claim is read back through
//! `DescribeCertificate`, so nothing here can pass on a response the store does not
//! back. These rules are about the REAL clock, so the test waits on it rather than
//! moving the stored due instants.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    OWED_TRANSITION_SPAN, VALIDATION_EMAIL_DISPATCH_DELAY, VALIDATION_SETTLE_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, request_certificate, router_with_engine,
};

/// ACM's one fixed sentence for a resend asked of a certificate it has sent no
/// validation mail for yet: all fixed text, no identifier interpolated, no terminating
/// period.
const REQUEST_STILL_IN_PROGRESS: &str =
    "The certificate request is still in progress. Try again later";

/// The least span a transition this emulator owes may stay owed for, as an ABSOLUTE
/// duration rather than one derived from the constants under test: a client that
/// requests a certificate and then describes it and acts on it makes several calls over
/// the network, and a span it can overrun is one that makes the state it guards
/// unobservable.
const CLIENT_SEQUENCE_FLOOR: Duration = Duration::from_secs(3);

/// How long this test lets pass between the request and the calls that must still see
/// the pre-dispatch state: a real pause, on the real clock, standing for the client call
/// sequence the span has to outlast. Half the floor, so the assertions below have the
/// other half as slack.
const CLIENT_SEQUENCE_PAUSE: Duration = Duration::from_millis(1_500);

/// The names one row's certificate covers, paired with whether each is validated BY
/// EMAIL (and so has validation mail to resend at all).
type CoveredNames<'a> = &'a [(&'a str, bool)];

/// One row: what it exercises, the `RequestCertificate` body, and the names it covers.
type PendingCase<'a> = (&'a str, Value, CoveredNames<'a>);

/// One requested row: what it exercises, the ARN its request minted, and its names.
type RequestedCase<'a> = (&'a str, String, CoveredNames<'a>);

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

/// Call `ResendValidationEmail` for one `(arn, domain)` through the domain itself.
async fn resend(router: &MultiServiceRouter, arn: &str, domain: &str) -> (u16, Value) {
    call(
        router,
        "ResendValidationEmail",
        &json!({
            "CertificateArn": arn,
            "Domain": domain,
            "ValidationDomain": domain,
        }),
    )
    .await
}

/// RULE, on the spans themselves: both transitions a requested certificate is owed stay
/// owed for at least a client's call sequence, and the outcome still falls due first so
/// a request that needs no proof of control is never mailed.
///
/// The floor is absolute: a span that satisfies it cannot be narrowed without this
/// failing, which is what the behavioural rows below depend on.
#[test]
fn every_owed_transition_outlasts_a_client_call_sequence() {
    for (name, span) in [
        ("the validation outcome", VALIDATION_SETTLE_DELAY),
        ("the initial mail dispatch", VALIDATION_EMAIL_DISPATCH_DELAY),
    ] {
        assert!(
            span >= CLIENT_SEQUENCE_FLOOR,
            "case {name:?}: a transition this emulator owes must stay owed for at least \
             {CLIENT_SEQUENCE_FLOOR:?}, so a client's own next calls land before it, got \
             {span:?}"
        );
    }
    assert_eq!(
        OWED_TRANSITION_SPAN, CLIENT_SEQUENCE_FLOOR,
        "the span this service gives an owed transition is the client-call floor, got \
         {OWED_TRANSITION_SPAN:?}"
    );
    assert!(
        VALIDATION_SETTLE_DELAY < VALIDATION_EMAIL_DISPATCH_DELAY,
        "an outcome that waits on no evidence must fall due before the initial mail \
         would have, got {VALIDATION_SETTLE_DELAY:?} and {VALIDATION_EMAIL_DISPATCH_DELAY:?}"
    );
}

/// RULE, over the wire: a client that pauses for a call sequence after
/// `RequestCertificate` still finds the certificate `PENDING_VALIDATION` with nothing
/// mailed, and its resend still draws ACM's fixed in-progress sentence — including for a
/// name whose control can never be proven, whose owed outcome is what used to close the
/// window first.
#[tokio::test]
async fn a_client_that_pauses_still_sees_the_pre_dispatch_state() {
    let cases: &[PendingCase<'_>] = &[
        (
            "a name whose control can never be proven",
            json!({ "DomainName": "paused.invalid", "ValidationMethod": "EMAIL" }),
            &[("paused.invalid", true)],
        ),
        (
            // No method named: ACM validates a public certificate by email unless the
            // request asks for DNS or HTTP validation.
            "the same with no validation method named",
            json!({
                "DomainName": "paused.localhost",
                "SubjectAlternativeNames": ["mail.paused.localhost"],
            }),
            &[("paused.localhost", true), ("mail.paused.localhost", true)],
        ),
        (
            "one email-validated name whose control could be proven",
            json!({ "DomainName": "paused.probe-domain.com", "ValidationMethod": "EMAIL" }),
            &[("paused.probe-domain.com", true)],
        ),
        (
            "three names, one of them mailed at a suffix the request chose",
            json!({
                "DomainName": "a.paused.probe-domain.io",
                "SubjectAlternativeNames": [
                    "b.paused.probe-domain.io",
                    "*.wild.paused.probe-domain.io",
                ],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    {
                        "DomainName": "a.paused.probe-domain.io",
                        "ValidationDomain": "paused.probe-domain.io",
                    }
                ],
            }),
            &[
                ("a.paused.probe-domain.io", true),
                ("b.paused.probe-domain.io", true),
                ("*.wild.paused.probe-domain.io", true),
            ],
        ),
        (
            "a name validated by DNS, which has no validation mail at all",
            json!({ "DomainName": "paused.probe-domain.org", "ValidationMethod": "DNS" }),
            &[("paused.probe-domain.org", false)],
        ),
    ];

    // One store for the table: each row has its own certificate and reads only that one,
    // so the pause below is waited out once for every row at once.
    let (router, _engine) = router_with_engine();
    let started = Instant::now();
    let mut requested: Vec<RequestedCase<'_>> = Vec::new();
    for (case, body, names) in cases {
        let arn = request_certificate(&router, body).await;
        requested.push((case, arn, names));
    }

    // The client's own call sequence, on the real clock. Nothing nudges the stored due
    // instants, so what the calls below see is what the spans decide.
    tokio::time::sleep(CLIENT_SEQUENCE_PAUSE).await;

    for (case, arn, names) in &requested {
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: {:?} after the request the certificate is still awaiting \
             validation, got {detail}",
            started.elapsed()
        );
        for (name, by_email) in *names {
            let validation = validation_for(case, &detail, name);
            let mailboxes = validation["ValidationEmails"].as_array();
            if *by_email {
                assert_eq!(
                    mailboxes,
                    Some(&Vec::new()),
                    "case {case:?}: no mail has gone out for {name} yet, so its mailbox \
                     list is empty, got {validation}"
                );
            } else {
                assert_eq!(
                    mailboxes, None,
                    "case {case:?}: {name} is validated without mail, so it reports no \
                     mailbox list, got {validation}"
                );
            }

            let (status, response) = resend(&router, arn, name).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidStateException"),
                "case {case:?}: a resend for {name} before any mail went out is refused, \
                 got {status} {response}"
            );
            if *by_email {
                let message = error_message(&response);
                assert_eq!(
                    message, REQUEST_STILL_IN_PROGRESS,
                    "case {case:?}: {:?} after the request, a resend for {name} must still \
                     draw ACM's fixed in-progress sentence, got {message:?}",
                    started.elapsed()
                );
            }
        }

        // A refused resend sent nothing, so the record is exactly as it was.
        let after = describe(&router, arn).await;
        assert_eq!(
            after, detail,
            "case {case:?}: a refused resend must change nothing, was {detail} and is now \
             {after}"
        );
    }
}

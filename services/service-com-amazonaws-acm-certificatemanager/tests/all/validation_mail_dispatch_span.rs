// SPDX-License-Identifier: CC0-1.0

//! What a client sees in the span BEFORE ACM's initial domain-validation mail goes out,
//! end to end over the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`,
//! awsJson1_1).
//!
//! RULES under test:
//!
//! 1. The initial dispatch is owed for a span that leaves a client room to make its next
//!    call: a `DescribeCertificate` and a `ResendValidationEmail` issued straight after
//!    `RequestCertificate` — on the real clock, with nothing nudging the stored due
//!    instants — still observe the UNMAILED state. Every name validated by EMAIL reports
//!    the EMPTY mailbox list, because `ValidationEmails` is "a list of email addresses
//!    that ACM used to send domain validation emails"
//!    (`smithy_query(query="doc", shape="com.amazonaws.acm#DomainValidation",
//!    member="ValidationEmails")`) and ACM has used none yet; and the resend is refused
//!    400 `InvalidStateException` with ACM's one fixed sentence for a request it has not
//!    finished processing, which names nothing of the request.
//! 2. That refusal is keyed on whether validation mail has gone out for the certificate,
//!    not on the bookkeeping behind the owed dispatch, so it is the answer for every
//!    email-validated name of a `PENDING_VALIDATION` certificate ACM has mailed nothing
//!    for — whatever the request asked for.
//! 3. A request whose outcome ACM reaches WITHOUT waiting on proof of domain control
//!    settles before that dispatch falls due, so it is never mailed at all: the mailbox
//!    list is still empty once the outcome is in and stays empty past the span the
//!    dispatch would have fallen due in, and the resend then reports the certificate's
//!    status instead of the in-progress gate.
//!
//! Every call goes through `MultiServiceRouter::dispatch` on the shared harness in
//! [`crate::certificate_lifecycle`] and every claim is read back through
//! `DescribeCertificate`, so nothing here can pass on a response the store does not
//! back. These rules are about the REAL clock, so they wait on it rather than moving the
//! stored due instants.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    VALIDATION_EMAIL_DISPATCH_DELAY, VALIDATION_SETTLE_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, request_certificate, router, router_with_engine,
};
use crate::certificate_validation_state_machine::PRIVATE_CA_ARN;

/// ACM's one fixed sentence for a resend asked of a certificate it has sent no
/// validation mail for yet: all fixed text, no identifier interpolated, no terminating
/// period.
const REQUEST_STILL_IN_PROGRESS: &str =
    "The certificate request is still in progress. Try again later";

/// The least span that leaves a client room to make its next call before the initial
/// dispatch falls due: a describe or a resend issued straight after
/// `RequestCertificate` travels the network, so a span shorter than this is one no
/// client could observe the unmailed state in.
const CLIENT_CALL_FLOOR: Duration = Duration::from_secs(1);

/// How long a bounded poll of the real clock may run before the test fails it: far
/// above every span this emulator owes a transition in.
const POLL_BUDGET: Duration = Duration::from_secs(20);

/// One pre-dispatch row: what it exercises, the `RequestCertificate` body, and the
/// names it covers paired with whether each is validated BY EMAIL (and so reports a
/// mailbox list at all).
type UnmailedCase<'a> = (&'a str, Value, &'a [(&'a str, bool)]);

/// One never-mailed row: what it exercises, the `RequestCertificate` body and the
/// terminal `CertificateStatus` the clock owes it without waiting on proof of control.
type SettledCase<'a> = (&'a str, Value, &'a str);

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

/// A validation's reported `ValidationEmails`, or `None` when the member is absent.
fn reported_mailboxes(validation: &Value) -> Option<Vec<String>> {
    Some(
        validation["ValidationEmails"]
            .as_array()?
            .iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect(),
    )
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

/// RULES 1 and 2: straight after `RequestCertificate`, on the real clock, every
/// email-validated name reports the empty mailbox list and a resend for it is refused
/// with ACM's fixed in-progress sentence — and the refusal writes nothing.
#[tokio::test]
async fn a_client_calling_again_still_sees_the_unmailed_certificate() {
    // The span has to be longer than one client call, or the state AWS reports to a
    // client that describes right after requesting could never be observed here.
    assert!(
        VALIDATION_EMAIL_DISPATCH_DELAY >= CLIENT_CALL_FLOOR,
        "the initial dispatch must stay owed for at least {CLIENT_CALL_FLOOR:?}, so a \
         client's next call lands before it, got {VALIDATION_EMAIL_DISPATCH_DELAY:?}"
    );

    // Rows differ along what the rules generalise over: the validation method (named
    // and defaulted), the number of covered names, the form of each name (plain,
    // wildcard), whether the request chose the mail suffix itself, whether the name is
    // one whose control could ever be proven, and a certificate validated by DNS beside
    // the email-validated ones.
    let cases: &[UnmailedCase<'_>] = &[
        (
            "one email-validated name",
            json!({ "DomainName": "fresh.probe-domain.com", "ValidationMethod": "EMAIL" }),
            &[("fresh.probe-domain.com", true)],
        ),
        (
            // No method named: ACM validates a public certificate by email unless the
            // request asks for DNS or HTTP validation.
            "a request that named no validation method",
            json!({ "DomainName": "defaulted.probe-domain.net" }),
            &[("defaulted.probe-domain.net", true)],
        ),
        (
            "three names, one of them mailed at a suffix the request chose",
            json!({
                "DomainName": "a.deep.probe-domain.io",
                "SubjectAlternativeNames": ["b.deep.probe-domain.io", "*.wild.probe-domain.io"],
                "ValidationMethod": "EMAIL",
                "DomainValidationOptions": [
                    { "DomainName": "a.deep.probe-domain.io", "ValidationDomain": "probe-domain.io" }
                ],
            }),
            &[
                ("a.deep.probe-domain.io", true),
                ("b.deep.probe-domain.io", true),
                ("*.wild.probe-domain.io", true),
            ],
        ),
        (
            // A name whose top-level label the public DNS delegates to nobody (RFC
            // 6761) is one ACM refuses up front — but not yet: the request is accepted,
            // so the gate the resend draws here is the in-progress one, exactly as
            // above.
            "a name whose control can never be proven",
            json!({ "DomainName": "reserved.invalid", "ValidationMethod": "EMAIL" }),
            &[("reserved.invalid", true)],
        ),
        (
            "a name validated by DNS, which reports no mailbox list at all",
            json!({ "DomainName": "dns.probe-domain.org", "ValidationMethod": "DNS" }),
            &[("dns.probe-domain.org", false)],
        ),
    ];

    for (case, body, names) in cases {
        // A store per row, so one row's certificate cannot explain another's.
        let router = router();
        let arn = request_certificate(&router, body).await;

        // Nothing moves the stored due instant: this is what a client reading straight
        // after its own request sees.
        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case:?}: a requested certificate enters PENDING_VALIDATION, got {detail}"
        );
        for (name, by_email) in *names {
            let validation = validation_for(case, &detail, name);
            let mailboxes = reported_mailboxes(&validation);
            if *by_email {
                assert_eq!(
                    mailboxes,
                    Some(Vec::new()),
                    "case {case:?}: no mail has gone out for {name}, so its mailbox list \
                     is empty, got {validation}"
                );
            } else {
                assert_eq!(
                    mailboxes, None,
                    "case {case:?}: {name} is validated without mail, so it reports no \
                     mailbox list, got {validation}"
                );
            }

            let (status, response) = resend(&router, &arn, name).await;
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
                    "case {case:?}: expected ACM's fixed in-progress sentence for {name}, \
                     got {message:?}"
                );
                assert!(
                    !message.contains(&arn) && !message.contains(name),
                    "case {case:?}: the in-progress sentence names nothing of the \
                     request, got {message:?}"
                );
            }
        }

        // A refused resend sent nothing, so the record is exactly as it was.
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, detail,
            "case {case:?}: a refused resend must change nothing, was {detail} and is \
             now {after}"
        );
    }
}

/// RULE 3: a request ACM reaches an outcome for without waiting on proof of domain
/// control settles before the initial dispatch falls due, so it is never mailed — the
/// mailbox list is empty when the outcome lands, still empty past the dispatch span,
/// and the resend then reports the certificate's status.
#[tokio::test]
async fn a_request_that_needs_no_proof_of_control_is_never_mailed() {
    // The ordering is what drops the owed dispatch: an outcome ACM needs no evidence
    // for falls due first, and a request it has finished processing gets no validation
    // mail afterwards.
    assert!(
        VALIDATION_SETTLE_DELAY < VALIDATION_EMAIL_DISPATCH_DELAY,
        "an outcome that waits on no evidence must fall due before the initial mail \
         would have, got {VALIDATION_SETTLE_DELAY:?} and {VALIDATION_EMAIL_DISPATCH_DELAY:?}"
    );

    // Rows span both requests whose outcome needs no proof of control: a name the
    // public DNS delegates to nobody (which ACM refuses up front, with the method named
    // and defaulted) and a private certificate, for which "domain validation is not
    // required" (`RequestCertificate` doc).
    let cases: &[SettledCase<'_>] = &[
        (
            "a name whose control can never be proven",
            json!({ "DomainName": "unprovable.invalid", "ValidationMethod": "EMAIL" }),
            "FAILED",
        ),
        (
            "the same with no validation method named",
            json!({
                "DomainName": "unprovable.localhost",
                "SubjectAlternativeNames": ["mail.unprovable.localhost"],
            }),
            "FAILED",
        ),
        (
            "a private certificate, which needs no domain validation",
            json!({
                "DomainName": "private.probe-domain.com",
                "ValidationMethod": "EMAIL",
                "CertificateAuthorityArn": PRIVATE_CA_ARN,
            }),
            "ISSUED",
        ),
    ];

    // One store for the table, so the span past the dispatch is waited out once: each
    // row has its own certificate, and no row reads another's.
    let (router, _engine) = router_with_engine();
    let started = Instant::now();
    let mut requested: Vec<(&str, String, &str)> = Vec::new();
    for (case, body, expected) in cases {
        let arn = request_certificate(&router, body).await;
        requested.push((case, arn, expected));
    }

    // The outcome is owed by the clock: poll until every row has reached it, without
    // touching the stored instants.
    for (case, arn, expected) in &requested {
        let terminal = loop {
            let detail = describe(&router, arn).await;
            let status = detail["Status"].as_str().unwrap_or_default().to_string();
            if status != "PENDING_VALIDATION" {
                break status;
            }
            assert!(
                started.elapsed() < POLL_BUDGET,
                "case {case:?}: polling for the outcome gave up after {:?}: still {detail}",
                started.elapsed()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(
            &terminal, *expected,
            "case {case:?}: expected the outcome {expected}, got {terminal}"
        );
    }

    // Past the span the dispatch would have fallen due in, so a mail that was still
    // owed would have gone out by now.
    let waited = started.elapsed();
    tokio::time::sleep(
        (VALIDATION_EMAIL_DISPATCH_DELAY.saturating_sub(waited)) + CLIENT_CALL_FLOOR,
    )
    .await;

    for (case, arn, expected) in &requested {
        let detail = describe(&router, arn).await;
        assert_eq!(
            detail["Status"].as_str(),
            Some(*expected),
            "case {case:?}: a settled outcome is terminal, got {detail}"
        );
        for validation in detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no DomainValidationOptions in {detail}"))
        {
            let name = validation["DomainName"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                reported_mailboxes(validation),
                Some(Vec::new()),
                "case {case:?}: ACM mailed {name} nothing, so its mailbox list is still \
                 empty, got {validation}"
            );

            // And the resend reports the certificate's state, not the gate a request
            // still being processed draws.
            let (status, response) = resend(&router, arn, &name).await;
            let message = error_message(&response);
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "InvalidStateException"),
                "case {case:?}: a resend for {name} past the outcome is refused, got \
                 {status} {response}"
            );
            assert_eq!(
                message,
                format!("Certificate {arn} is not in pending validation state"),
                "case {case:?}: expected the state refusal built from the request for \
                 {name}, got {message:?}"
            );
        }
    }
}

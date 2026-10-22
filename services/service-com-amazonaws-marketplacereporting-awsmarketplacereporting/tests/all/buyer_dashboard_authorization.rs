// SPDX-License-Identifier: CC0-1.0

//! `GetBuyerDashboard`'s availability gate, end to end over HTTP.
//!
//! The rule: `GetBuyerDashboard` is available only to the callers its own documentation
//! names — "Amazon Web Services Organization management accounts or delegated
//! administrators registered for the procurement insights
//! (procurement-insights.marketplace.amazonaws.com) feature" — and that grant is held by
//! AWS Organizations, which this service cannot enrol anybody in, so it exists here only
//! as a record in the store. Every other caller, including one the store holds no record
//! for, is refused with the modelled `UnauthorizedException` (HTTP 401, `@httpError(401)`
//! on the shape) and gets no success member back; a delegated administrator that is not
//! registered gets the modelled `AccessDeniedException` (403). The decision reads only
//! recorded state, so repeated and concurrent identical calls are refused identically,
//! and it is made BEFORE the modelled input constraints, so a request that also violates
//! `dashboardIdentifier`'s `@pattern` or `embeddingDomains`' `@length` is refused the same
//! way rather than as a `ValidationException`.
//!
//! The harness (`Emulator`, `request`, the two dashboard ARNs) is the one
//! `buyer_dashboard.rs` owns, reused rather than copied.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacereporting_awsmarketplacereporting::state::CallerAccess;

use crate::buyer_dashboard::{
    agreement_summary, cost_analysis, request, token_of, Answer, Emulator, DEFAULT_PRINCIPAL,
};

/// The wire answer the gate owes a caller the API is not available to: the modelled
/// `UnauthorizedException`, at the status the model binds it to.
const REFUSED: (u16, &str) = (401, "UnauthorizedException");

/// Everything a refusal must NOT carry: `GetBuyerDashboardOutput`'s three members. The
/// failure path produces no response body, so none of them may appear — least of all the
/// embedding URL, which is a credential.
const OUTPUT_MEMBERS: [&str; 3] = ["embedUrl", "dashboardIdentifier", "embeddingDomains"];

/// Assert `answer` is the refusal above and leaks no part of the success body.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        REFUSED,
        "case {case:?}: expected {} {:?}, got {} {:?}",
        REFUSED.0,
        REFUSED.1,
        answer.status,
        answer.body
    );
    for member in OUTPUT_MEMBERS {
        assert!(
            answer.body.get(member).is_none(),
            "case {case:?}: a refusal must not carry {member:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_caller_with_no_recorded_grant_is_refused_whatever_it_asks_for() {
    // The class the rule generalises over: either published dashboard, any account the
    // ARN pattern admits, and one or two embedding domains of every documented form. The
    // request takes no part in the decision, so every row must come to the same refusal.
    let cases: [(&str, String, Vec<&str>); 5] = [
        (
            "the issue's own request",
            agreement_summary("000000000000"),
            vec!["https://example.com"],
        ),
        (
            "the other published dashboard",
            cost_analysis("123456789012"),
            vec!["https://*.amazon.com"],
        ),
        (
            "two embedding domains",
            agreement_summary("210987654321"),
            vec!["https://a.example.com", "https://b.example.com"],
        ),
        (
            "the localhost development form",
            cost_analysis("999999999999"),
            vec!["http://localhost:3000"],
        ),
        (
            "a domain with a path and a query",
            agreement_summary("123456789012"),
            vec!["https://example.co.uk/embed?x=1"],
        ),
    ];

    let emulator = Emulator::new();
    for (case, dashboard, domains) in &cases {
        let answer = emulator.call(&request(dashboard, domains)).await;
        assert_refused(case, &answer);
        assert_eq!(
            answer.error_message(),
            "Unauthorized",
            "case {case:?}: the refusal must carry the service's own wording, got {:?}",
            answer.body
        );
        assert!(
            emulator.store().lock().embed_sessions.is_empty(),
            "case {case:?}: a refused call must generate no session"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_and_concurrent_calls_are_refused_identically() {
    // The decision is a read of recorded state keyed by the caller, so it cannot depend
    // on how many calls came before or how many run at once — and it mints nothing, so
    // no two of them can differ the way two generated URLs do.
    let emulator = Arc::new(Emulator::new());
    let body = request(&agreement_summary("000000000000"), &["https://example.com"]);

    let mut answers: Vec<(String, Answer)> = Vec::new();
    for attempt in 1..=3 {
        answers.push((
            format!("sequential attempt {attempt}"),
            emulator.call(&body).await,
        ));
    }

    let mut running = Vec::new();
    for attempt in 1..=8 {
        let emulator = Arc::clone(&emulator);
        let body = body.clone();
        running.push(tokio::spawn(async move {
            (
                format!("concurrent attempt {attempt}"),
                emulator.call(&body).await,
            )
        }));
    }
    for task in running {
        answers.push(task.await.expect("the concurrent call did not panic"));
    }

    let mut distinct: HashSet<String> = HashSet::new();
    for (case, answer) in &answers {
        assert_refused(case, answer);
        distinct.insert(answer.body.to_string());
    }
    assert_eq!(
        distinct.len(),
        1,
        "every call must answer identically, got {distinct:?}"
    );
    assert!(
        emulator.store().lock().embed_sessions.is_empty(),
        "no refused call may generate a session"
    );
}

#[tokio::test]
async fn the_availability_decision_precedes_the_modelled_input_constraints() {
    // Each row is a request a caller the API IS available to would be refused for, with
    // the status and code that refusal carries: the boundary's `ValidationException` for a
    // value outside a modelled constraint, and the handler's own `BadRequestException` for
    // one the constraint admits and the documented form does not. A caller the API is NOT
    // available to must get the availability refusal for every one of them instead.
    let dashboard = agreement_summary("123456789012");
    let cases: [(&str, Value, u16, &str); 8] = [
        (
            "an identifier that is not an ARN at all",
            request("not-an-arn-at-all", &["https://example.com"]),
            400,
            "ValidationException",
        ),
        (
            "an account id of eleven digits",
            request(&agreement_summary("12345678901"), &["https://example.com"]),
            400,
            "ValidationException",
        ),
        (
            "a trailing extra segment",
            request(
                &format!("{dashboard}/extra"),
                &["https://example.com"],
            ),
            400,
            "ValidationException",
        ),
        (
            "a partition other than aws",
            request(
                "arn:aws-us-gov:aws-marketplace::123456789012:AWSMarketplace/ReportingData/Agreement_V1/Dashboard/AgreementSummary_V1",
                &["https://example.com"],
            ),
            400,
            "ValidationException",
        ),
        (
            "a resource path the pattern does not publish",
            request(
                "arn:aws:aws-marketplace::123456789012:AWSMarketplace/ReportingData/Nope_V1/Dashboard/Nope_V1",
                &["https://example.com"],
            ),
            400,
            "ValidationException",
        ),
        (
            "three embedding domains",
            request(
                &dashboard,
                &[
                    "https://a.example.com",
                    "https://b.example.com",
                    "https://c.example.com",
                ],
            ),
            400,
            "ValidationException",
        ),
        (
            "no embedding domain",
            request(&dashboard, &[]),
            400,
            "ValidationException",
        ),
        (
            "a domain the pattern admits and the documented form does not",
            request(&dashboard, &["https://*.com"]),
            400,
            "BadRequestException",
        ),
    ];

    let unrecorded = Emulator::new();
    let available = Emulator::available_to_default_caller();

    for (case, body, status, code) in &cases {
        let refused = unrecorded.call(body).await;
        assert_refused(case, &refused);

        // The same body, from a caller with the grant on record: now the input is what
        // decides, which is what makes the order above observable rather than a coincidence.
        let answer = available.call(body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (*status, *code),
            "case {case:?}: an available caller must be refused on the INPUT, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // And a body that is not even a modelled shape — the gate decides before anything
    // reads it.
    let refused = unrecorded
        .call(&json!({ "nothing": "the model names" }))
        .await;
    assert_refused("a body naming no modelled member", &refused);
}

#[tokio::test]
async fn a_recorded_grant_is_what_opens_the_api() {
    // Both callers the rule names, as records in the store — the only place this service
    // can hold the grant, since nothing in its model enrols an account.
    let cases: [(&str, CallerAccess); 2] = [
        ("management account", CallerAccess::management_account()),
        (
            "delegated administrator registered for procurement insights",
            CallerAccess::registered_delegated_admin(),
        ),
    ];

    let emulator = Emulator::new();
    let dashboard = cost_analysis("123456789012");
    let body = request(&dashboard, &["https://*.amazon.com"]);

    for (index, (case, access)) in cases.into_iter().enumerate() {
        let principal = format!("AKIDGRANTED{index}");
        emulator
            .store()
            .lock()
            .access
            .insert(principal.clone(), access);

        let answer = emulator.get_buyer_dashboard(&principal, &body).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (200, ""),
            "case {case:?}: a recorded grant must open the API, got {} {:?}",
            answer.status,
            answer.body
        );

        // Read the generated session back: the URL the caller got is the record this
        // service kept, attributed to the principal whose grant authorised the call.
        let url = answer.embed_url().to_string();
        let token = token_of(&url).to_string();
        let store = emulator.store();
        let held = store.lock();
        let session = held
            .embed_sessions
            .get(&token)
            .unwrap_or_else(|| panic!("case {case:?}: no session was persisted for {token:?}"));
        assert_eq!(
            (
                session.embed_url.as_str(),
                session.principal.as_str(),
                session.dashboard_identifier.as_str()
            ),
            (url.as_str(), principal.as_str(), dashboard.as_str()),
            "case {case:?}: the stored session must agree with the answer, got {session:?}"
        );
    }

    // The grant is per caller: the default principal still has none on record.
    let refused = emulator.call(&body).await;
    assert_refused("a caller beside the two granted ones", &refused);
    assert_eq!(
        emulator.store().lock().embed_sessions.len(),
        2,
        "only the two granted calls may have generated a session"
    );
    assert!(
        !emulator
            .store()
            .lock()
            .access
            .contains_key(DEFAULT_PRINCIPAL),
        "the default caller must stay unrecorded — a read may not grant anything"
    );
}

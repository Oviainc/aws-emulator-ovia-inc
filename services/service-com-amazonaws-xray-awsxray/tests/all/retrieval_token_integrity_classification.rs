// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal class a `RetrievalToken` falls in, on every operation that
//! takes one (the wire harness in [`super::xray_wire`]).
//!
//! The rule the rows below generalise over: a `RetrievalToken` is
//! INTEGRITY-CHECKED before it is resolved, so the class of the refusal is
//! decided by the submitted value alone — a value this service could not have
//! issued is a malformed parameter (`InvalidRequestException`, HTTP 400),
//! whatever the store holds, and `ResourceNotFoundException` (HTTP 404) is
//! reachable ONLY by a value that verifies and yet names no live retrieval.
//!
//! Rows differ along what that rule generalises over: WHERE a genuine token is
//! damaged (every position of it, swept), whether the store holds any retrieval
//! at all when the damaged value arrives, which operation was called, and — for
//! the 404 class — which verifying value nothing stands behind.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{
    retrieval_token_check, RETRIEVAL_STATUS_CANCELLED, RETRIEVAL_TOKEN_HEX_DIGITS,
};

use super::xray_wire::{root_document, trace_id, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The operations that take a `RetrievalToken`
/// (`smithy_query(query="operation",operation="CancelTraceRetrieval,
/// ListRetrievedTraces,GetRetrievedTracesGraph")`), each with the name its own
/// refusal gives the member.
const TOKEN_READERS: [(&str, &str, &str); 3] = [
    ("CancelTraceRetrieval", "/CancelTraceRetrieval", "retrieval token"),
    ("ListRetrievedTraces", "/ListRetrievedTraces", "execution id"),
    (
        "GetRetrievedTracesGraph",
        "/GetRetrievedTracesGraph",
        "execution id",
    ),
];

/// The request body a reader needs to reach its token check: `TraceFormat` is a
/// member of `ListRetrievedTraces` alone, and that operation judges it first.
fn body(path: &str, token: &str) -> Value {
    let mut body = json!({ "RetrievalToken": token });
    if path == "/ListRetrievedTraces" {
        body["TraceFormat"] = json!("XRAY");
    }
    body
}

/// What a value of the issued form whose integrity check does not verify is
/// refused with, by the message rule the sibling suite
/// [`super::malformed_retrieval_token_message`] owns.
fn undecodable_message(member: &str, token: &str) -> String {
    format!("Invalid {member} {token} passed: Invalid execution id provided: {token}")
}

/// A genuine `RetrievalToken`, and the trace it was started for: one segment
/// ingested, then a retrieval over the window it falls in.
async fn genuine_token(xray: &Xray, n: u32) -> String {
    let start = (now() - 600.0).trunc();
    let trace = trace_id(n);
    let ingested = xray
        .put_segments(&[root_document(
            "2222222222222222",
            &trace,
            "probe",
            start + 1.0,
            Some(start + 2.0),
        )
        .to_string()])
        .await;
    assert_eq!(
        ingested.status, 200,
        "the seed segment must be accepted, got {ingested:?}"
    );
    let started = xray
        .call(
            "/StartTraceRetrieval",
            &json!({"TraceIds": [trace], "StartTime": start, "EndTime": start + 300.0}),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "starting the retrieval must succeed, got {started:?}"
    );
    let token = started
        .get("RetrievalToken")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        token.len(),
        RETRIEVAL_TOKEN_HEX_DIGITS,
        "a minted RetrievalToken is {RETRIEVAL_TOKEN_HEX_DIGITS} hexadecimal digits, got {token:?}"
    );
    token
}

/// `token` with the digit at `index` replaced by another of the hexadecimal
/// alphabet: the same width, the same alphabet, so only the token's own
/// integrity check can tell it from the issued value.
fn flip_digit(token: &str, index: usize) -> String {
    let mut digits: Vec<char> = token.chars().collect();
    digits[index] = if digits[index] == '0' { '1' } else { '0' };
    digits.into_iter().collect()
}

#[tokio::test]
async fn damage_at_any_position_of_a_token_is_a_bad_request_never_a_missing_retrieval() {
    let xray = Xray::new();
    let real = genuine_token(&xray, 11).await;

    // The invariant, swept over EVERY position of the token: wherever the damage
    // is, the value no longer verifies, and a value that does not verify is the
    // request being wrong rather than the retrieval being gone.
    for index in 0..real.len() {
        let damaged = flip_digit(&real, index);
        let case = format!("digit {index} of a genuine token flipped");
        assert_ne!(
            damaged, real,
            "case {case:?}: the sweep must submit a value other than the issued one"
        );
        for (operation, path, member) in TOKEN_READERS {
            let refused = xray.call(path, &body(path, &damaged)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let expected = undecodable_message(member, &damaged);
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
        }
    }

    // The genuine value still verifies after all that, so the sweep refused the
    // damaged copies rather than the retrieval behind them.
    let listed = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": real, "TraceFormat": "XRAY"}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "the issued token must still read, got {listed:?}"
    );
    assert_ne!(
        listed.get("RetrievalStatus").as_str(),
        Some(RETRIEVAL_STATUS_CANCELLED),
        "none of the refused calls may have cancelled the retrieval, got {listed:?}"
    );
}

#[tokio::test]
async fn the_class_of_a_non_verifying_token_does_not_depend_on_what_the_store_holds() {
    // The same submitted value on two emulators: one that has never minted a
    // retrieval, one holding a live retrieval the value was derived from. The
    // value is judged before any lookup, so both answer the same way.
    let empty = Xray::new();
    let seeded = Xray::new();
    let real = genuine_token(&seeded, 12).await;
    let damaged = flip_digit(&real, real.len() / 2);

    for (case, xray) in [
        ("an emulator holding no retrieval at all", &empty),
        ("an emulator holding the retrieval it was derived from", &seeded),
    ] {
        for (operation, path, member) in TOKEN_READERS {
            let refused = xray.call(path, &body(path, &damaged)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let expected = undecodable_message(member, &damaged);
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_404_class_belongs_to_a_token_that_verifies_and_names_no_retrieval() {
    let xray = Xray::new();

    // Rows differ in the identifying half the verifying value carries: each is
    // built the way the mint builds one — the identifying half, then the check
    // half derived from it — so each is a value this service COULD have issued
    // and nothing stands behind.
    for (case, identity) in [
        ("an all-zero identifying half", "0".repeat(16)),
        ("an all-f identifying half", "f".repeat(16)),
        ("every digit of the alphabet", "0123456789abcdef".to_string()),
    ] {
        let verifying = format!("{identity}{}", retrieval_token_check(&identity));
        for (operation, path, _member) in TOKEN_READERS {
            let answered = xray.call(path, &body(path, &verifying)).await;
            assert_eq!(
                (answered.status, answered.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} on {operation}: expected 404 ResourceNotFoundException, got \
                 {answered:?}"
            );
            let expected = format!(
                "RetrievalToken `{verifying}` was not issued by this service, or its retrieval \
                 has timed out"
            );
            let said = answered.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_that_verifies_reaches_the_retrieval_it_names() {
    let xray = Xray::new();
    let real = genuine_token(&xray, 13).await;

    // The gate lets a verifying value through to the store, so the cancellation
    // it asks for happens and every later read of the token reports it.
    let cancelled = xray
        .call("/CancelTraceRetrieval", &json!({"RetrievalToken": real}))
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the issued token must cancel its retrieval, got {cancelled:?}"
    );
    let listed = xray
        .call(
            "/ListRetrievedTraces",
            &json!({"RetrievalToken": real, "TraceFormat": "XRAY"}),
        )
        .await;
    assert_eq!(
        listed.get("RetrievalStatus").as_str(),
        Some(RETRIEVAL_STATUS_CANCELLED),
        "the cancelled retrieval must read back as cancelled, got {listed:?}"
    );

    // And a retrieval that is no longer ongoing is the THIRD class: the request
    // is refused because there is no job left to cancel, naming the status it is
    // in — not because the value was wrong and not because it is gone.
    let again = xray
        .call("/CancelTraceRetrieval", &json!({"RetrievalToken": real}))
        .await;
    assert_eq!(
        (again.status, again.error_type.as_deref()),
        (400, Some("InvalidRequestException")),
        "cancelling a cancelled retrieval must be refused, got {again:?}"
    );
    let said = again.message().unwrap_or_default();
    assert!(
        said.contains(RETRIEVAL_STATUS_CANCELLED) && said.contains(real.as_str()),
        "the refusal must name the token and the status it is in, got {said:?}"
    );
}

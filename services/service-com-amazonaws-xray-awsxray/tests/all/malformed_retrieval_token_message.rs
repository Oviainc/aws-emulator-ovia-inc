// SPDX-License-Identifier: CC0-1.0

//! What a `RetrievalToken` this service could not have ISSUED is refused with,
//! word for word, on every operation that takes one (the wire harness in
//! [`super::xray_wire`]).
//!
//! The rule the rows below generalise over: the refusal is TWO clauses — the
//! operation's own clause naming the parameter and the value the request
//! submitted, then the fixed cause clause of the execution-id decoder the value
//! was handed to. The rejected value is the sentence's only variable and it
//! appears once, in the outer clause; the cause clause is the same text whatever
//! operation was called and whatever the value was.
//!
//! Rows differ along what that rule generalises over: how the value misses the
//! minted form (a character flipped inside the minted alphabet and width, a
//! width either side of it, a digit outside the alphabet, another service's id
//! form, prose, the longest value the model allows), which operation was called,
//! and therefore which parameter the outer clause names.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::RETRIEVAL_STATUS_CANCELLED;

use super::xray_wire::{root_document, trace_id, Xray};

/// Now, in the epoch seconds every timestamp on this wire travels as.
fn now() -> f64 {
    service_com_amazonaws_xray_awsxray::state::now_epoch_seconds()
}

/// The operations that take a `RetrievalToken`
/// (`smithy_query(query="operation",operation="CancelTraceRetrieval,
/// ListRetrievedTraces,GetRetrievedTracesGraph")`), each with the name its own
/// refusal gives the member in the outer clause.
const TOKEN_READERS: [(&str, &str, &str); 3] = [
    ("CancelTraceRetrieval", "/CancelTraceRetrieval", "retrieval token"),
    ("ListRetrievedTraces", "/ListRetrievedTraces", "execution id"),
    (
        "GetRetrievedTracesGraph",
        "/GetRetrievedTracesGraph",
        "execution id",
    ),
];

/// The decoder's cause clause, as X-Ray words it: fixed text, capitalised
/// exactly this way, and naming no value.
const DECODER_CLAUSE: &str = "Invalid Execution Id";

/// The request body a reader needs to reach its token check: `TraceFormat` is a
/// member of `ListRetrievedTraces` alone, and that operation judges it first.
fn body(path: &str, token: &str) -> Value {
    let mut body = json!({ "RetrievalToken": token });
    if path == "/ListRetrievedTraces" {
        body["TraceFormat"] = json!("XRAY");
    }
    body
}

/// A genuine `RetrievalToken`: one segment ingested, then a retrieval started
/// over the window it falls in.
async fn genuine_token(xray: &Xray) -> String {
    let start = (now() - 600.0).trunc();
    let trace = trace_id(7);
    let ingested = xray
        .put_segments(&[
            root_document("1111111111111111", &trace, "probe", start + 1.0, Some(start + 2.0))
                .to_string(),
        ])
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
        32,
        "a minted RetrievalToken is 32 hexadecimal digits, got {token:?}"
    );
    token
}

/// `token` with the character at `index` changed to another of the hexadecimal
/// alphabet: the same length and the same charset, so only the token's own
/// integrity check can tell it from the issued value.
fn flip_digit(token: &str, index: usize) -> String {
    let mut digits: Vec<char> = token.chars().collect();
    digits[index] = if digits[index] == '0' { '1' } else { '0' };
    digits.into_iter().collect()
}

#[tokio::test]
async fn a_malformed_retrieval_token_is_named_in_both_clauses_of_the_refusal() {
    let xray = Xray::new();
    let real = genuine_token(&xray).await;

    let rejected = [
        // The class the integrity check exists for: a genuine token damaged in
        // transit, its width and alphabet intact.
        ("a genuine token with its middle digit flipped", flip_digit(&real, real.len() / 2)),
        ("a genuine token with its first digit flipped", flip_digit(&real, 0)),
        ("a genuine token with its last digit flipped", flip_digit(&real, real.len() - 1)),
        // Widths either side of the minted one, a digit outside the alphabet,
        // another id form, prose, and the longest value `RetrievalToken`'s
        // modelled `@length` 0..1020 admits
        // (`smithy_query(query="shape",shape="RetrievalToken")`).
        ("one digit short of the minted width", "c".repeat(31)),
        ("one digit past the minted width", "c".repeat(33)),
        ("a digit outside the hexadecimal alphabet", format!("{}z", "c".repeat(31))),
        (
            "a UUID, which is not the form this service mints",
            "59adda79-1d1f-4b1e-9f3a-2c7d5e8a9b01".to_string(),
        ),
        ("prose", "not-a-retrieval-token".to_string()),
        ("the longest token the model allows", "c".repeat(1_020)),
    ];

    for (case, token) in &rejected {
        for (operation, path, member) in TOKEN_READERS {
            let expected = format!("Invalid {member} {token} passed: {DECODER_CLAUSE}");
            let refused = xray.call(path, &body(path, token)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
            // The outer clause names the rejected value, which is what tells an
            // operator WHICH of several concurrent tokens failed; the cause
            // clause states only why it could not be read.
            let (outer, inner) = said
                .split_once(" passed: ")
                .unwrap_or_else(|| panic!("case {case:?} on {operation}: expected two clauses, got {said:?}"));
            assert!(
                outer.contains(token.as_str()),
                "case {case:?} on {operation}: the parameter clause must name the value, got {outer:?}"
            );
            assert_eq!(
                inner, DECODER_CLAUSE,
                "case {case:?} on {operation}: the cause clause is fixed text, got {inner:?}"
            );
        }
    }
}

#[tokio::test]
async fn refusing_a_damaged_token_leaves_the_retrieval_it_was_derived_from_alone() {
    let xray = Xray::new();
    let real = genuine_token(&xray).await;

    // Rows differ in where the damage is: the refusal is about the submitted
    // value, so none of them may reach the retrieval the value was derived from.
    for (case, damaged) in [
        ("the middle digit flipped", flip_digit(&real, real.len() / 2)),
        ("the first digit flipped", flip_digit(&real, 0)),
    ] {
        let refused = xray
            .call("/CancelTraceRetrieval", &json!({"RetrievalToken": damaged}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: a damaged token must be refused, got {refused:?}"
        );
        let listed = xray
            .call(
                "/ListRetrievedTraces",
                &json!({"RetrievalToken": real, "TraceFormat": "XRAY"}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the genuine token must still read, got {listed:?}"
        );
        assert_ne!(
            listed.get("RetrievalStatus").as_str(),
            Some(RETRIEVAL_STATUS_CANCELLED),
            "case {case:?}: the refused cancel must not have cancelled the retrieval, got {listed:?}"
        );
    }

    // And the genuine token is still accepted afterwards.
    let cancelled = xray
        .call("/CancelTraceRetrieval", &json!({"RetrievalToken": real}))
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the genuine token must still cancel the retrieval, got {cancelled:?}"
    );
}

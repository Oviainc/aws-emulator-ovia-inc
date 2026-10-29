// SPDX-License-Identifier: CC0-1.0

//! What a `RetrievalToken` this service could not have ISSUED is refused with,
//! word for word, on every operation that takes one (the wire harness in
//! [`super::xray_wire`]).
//!
//! The rule the rows below generalise over: the refusal is TWO clauses — the
//! operation's own clause naming the parameter and the value the request
//! submitted, then the clause of the execution-id decoder the value was handed
//! to. The submitted value is the sentence's only variable, and WHICH decoder
//! clause follows depends on how far the value got: a value that is not of the
//! issued token form at all never decoded, so the clause states only that the
//! execution id was invalid and names nothing; a value of the issued form whose
//! integrity check does not verify WAS decoded and rejected, so the clause names
//! the value it could not decode. Both classes are the same refusal class — 400
//! `InvalidRequestException`, before any lookup.
//!
//! Rows differ along what that rule generalises over: how the value misses the
//! issued value (a character flipped inside the minted alphabet and width, a
//! width either side of it, a digit outside the alphabet, another service's id
//! form, prose, the longest value the model allows), which operation was called,
//! and therefore which parameter the outer clause names; and, for the cause
//! clause, along the characters the value carries (JSON metacharacters, a
//! newline, non-ASCII text, the clause separator itself) and along whether the
//! request carried a member the refusal is not about.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{retrieval_token_check, RETRIEVAL_STATUS_CANCELLED};

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

/// The decoder's clause for a value that is not of the issued token form at all:
/// fixed text, capitalised exactly this way, and naming no value.
const DECODER_CLAUSE: &str = "Invalid Execution Id";

/// The decoder's clause for a value of the issued token form whose integrity
/// check does not verify: fixed text, then the value the decoder could not
/// decode.
const UNDECODABLE_CLAUSE: &str = "Invalid execution id provided: ";

/// Which of the two bad-token classes a submitted value falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Not of the issued token form at all.
    NotTokenForm,
    /// Of the issued token form, but its integrity check does not verify.
    Undecodable,
}

/// The refusal `token` must draw from the reader that names the member
/// `member`, by the rule in this module's header.
fn expected_message(member: &str, token: &str, class: Class) -> String {
    match class {
        Class::NotTokenForm => format!("Invalid {member} {token} passed: {DECODER_CLAUSE}"),
        Class::Undecodable => {
            format!("Invalid {member} {token} passed: {UNDECODABLE_CLAUSE}{token}")
        }
    }
}

/// How often the rule has the sentence name the submitted value: once for a
/// value that never decoded, twice for one the decoder rejected by name.
fn expected_echoes(class: Class) -> usize {
    match class {
        Class::NotTokenForm => 1,
        Class::Undecodable => 2,
    }
}

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
async fn a_malformed_retrieval_token_is_refused_in_the_words_of_its_own_class() {
    let xray = Xray::new();
    let real = genuine_token(&xray).await;

    // The minted width and alphabet over a check half that is deliberately NOT
    // the one the mint derives for that identifying half, upper-cased: a value of
    // the issued form that this service could not have issued however it is
    // cased.
    let identity = "0123456789abcdef";
    let miscased =
        format!("{identity}{}", flip_digit(&retrieval_token_check(identity), 0)).to_uppercase();

    let rejected = [
        // The class the integrity check exists for: a genuine token damaged in
        // transit, its width and alphabet intact.
        (
            "a genuine token with its middle digit flipped",
            flip_digit(&real, real.len() / 2),
            Class::Undecodable,
        ),
        (
            "a genuine token with its first digit flipped",
            flip_digit(&real, 0),
            Class::Undecodable,
        ),
        (
            "a genuine token with its last digit flipped",
            flip_digit(&real, real.len() - 1),
            Class::Undecodable,
        ),
        (
            "the minted width and alphabet, carrying no check",
            "c".repeat(32),
            Class::Undecodable,
        ),
        (
            "the minted width and alphabet upper-cased, carrying no check",
            miscased,
            Class::Undecodable,
        ),
        // Widths either side of the minted one, a digit outside the alphabet,
        // another id form, prose, and the longest value `RetrievalToken`'s
        // modelled `@length` 0..1020 admits
        // (`smithy_query(query="shape",shape="RetrievalToken")`).
        ("one digit short of the minted width", "c".repeat(31), Class::NotTokenForm),
        ("one digit past the minted width", "c".repeat(33), Class::NotTokenForm),
        (
            "a digit outside the hexadecimal alphabet",
            format!("{}z", "c".repeat(31)),
            Class::NotTokenForm,
        ),
        (
            "a UUID, which is not the form this service mints",
            "59adda79-1d1f-4b1e-9f3a-2c7d5e8a9b01".to_string(),
            Class::NotTokenForm,
        ),
        ("prose", "not-a-retrieval-token".to_string(), Class::NotTokenForm),
        (
            "the longest token the model allows",
            "c".repeat(1_020),
            Class::NotTokenForm,
        ),
    ];

    for (case, token, class) in &rejected {
        for (operation, path, member) in TOKEN_READERS {
            let expected = expected_message(member, token, *class);
            let refused = xray.call(path, &body(path, token)).await;
            // BOTH classes are the request being wrong, never the retrieval
            // being gone: the 404 is reserved for a token that verifies.
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
            // clause names it again only when the decoder read it.
            let (outer, inner) = said
                .split_once(" passed: ")
                .unwrap_or_else(|| panic!("case {case:?} on {operation}: expected two clauses, got {said:?}"));
            assert!(
                outer.contains(token.as_str()),
                "case {case:?} on {operation}: the parameter clause must name the value, got {outer:?}"
            );
            let expected_inner = match class {
                Class::NotTokenForm => DECODER_CLAUSE.to_string(),
                Class::Undecodable => format!("{UNDECODABLE_CLAUSE}{token}"),
            };
            assert_eq!(
                inner, expected_inner,
                "case {case:?} on {operation}: expected the cause clause {expected_inner:?}, got {inner:?}"
            );
            assert_eq!(
                said.matches(token.as_str()).count(),
                expected_echoes(*class),
                "case {case:?} on {operation}: expected the value named {} time(s), got {said:?}",
                expected_echoes(*class)
            );
        }
    }
}

#[tokio::test]
async fn the_cause_clause_is_one_phrase_whatever_the_value_or_the_other_members() {
    let xray = Xray::new();

    // Rows differ in the CHARACTERS the value carries rather than only in how it
    // misses the minted form: the submitted value is echoed into the sentence
    // verbatim, and `RetrievalToken` is modelled `@length` 0..1020 with no
    // `@pattern` (`smithy_query(query="shape",shape="RetrievalToken")`), so each
    // of these reaches the handler rather than the request layer. None of them is
    // of the issued token form, so each draws the decoder's entry clause.
    let rows: Vec<(&str, String)> = vec![
        (
            "a value carrying JSON metacharacters",
            "not\"a\\token".to_string(),
        ),
        ("a value carrying a newline", "first\nsecond".to_string()),
        ("a value of non-ASCII text", "\u{30c8}\u{30fc}\u{30af}\u{30f3}".to_string()),
        ("a value of spaces alone", "   ".to_string()),
        ("the clause separator inside the value", "a passed: b".to_string()),
    ];

    // Every cause clause the matrix below produced: the rule is that there is
    // exactly ONE for this class, whatever the operation and whatever the value.
    let mut causes: BTreeSet<String> = BTreeSet::new();

    for (case, token) in &rows {
        for (operation, path, member) in TOKEN_READERS {
            let expected = expected_message(member, token, Class::NotTokenForm);
            let refused = xray.call(path, &body(path, token)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let said = refused.message().unwrap_or_default().to_string();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
            // Split from the END, so a value that itself contains the separator
            // still yields the sentence's trailing cause clause.
            let (_, cause) = said.rsplit_once(" passed: ").unwrap_or_else(|| {
                panic!("case {case:?} on {operation}: expected two clauses, got {said:?}")
            });
            causes.insert(cause.to_string());

            // Nothing in the sentence is derived per call: the same refusal
            // asked for twice is the same sentence.
            let again = xray.call(path, &body(path, token)).await;
            assert_eq!(
                again.message().unwrap_or_default(),
                said,
                "case {case:?} on {operation}: the refusal must not vary between calls, got {again:?}"
            );

            // A member the refusal is not about cannot change it: the two
            // readers that model `NextToken`
            // (`smithy_query(query="operation",operation="ListRetrievedTraces,
            // GetRetrievedTracesGraph")`) judge the token before any paging.
            if path != "/CancelTraceRetrieval" {
                let mut paging = body(path, token);
                paging["NextToken"] = json!("0");
                let paged = xray.call(path, &paging).await;
                assert_eq!(
                    (paged.status, paged.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {case:?} on {operation}: an unrelated member must not change the class of the refusal, got {paged:?}"
                );
                assert_eq!(
                    paged.message().unwrap_or_default(),
                    said,
                    "case {case:?} on {operation}: an unrelated member must not change the refusal, got {paged:?}"
                );
            }
        }
    }

    assert_eq!(
        causes.iter().map(String::as_str).collect::<Vec<_>>(),
        vec![DECODER_CLAUSE],
        "the cause clause must be one fixed phrase across every operation and value, got {causes:?}"
    );
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

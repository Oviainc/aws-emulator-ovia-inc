// SPDX-License-Identifier: CC0-1.0

//! The integrity check a `RetrievalToken` is judged by, over the EXACT
//! characters the request submitted, on every operation that takes one (the wire
//! harness in [`super::xray_wire`]).
//!
//! The rule the rows below generalise over: a retrieval token is an opaque
//! string every character of which is significant, so a value that differs from
//! an issued one in ANY single character — the CASE of a digit included — is a
//! value this service could not have issued, and a value of the token form that
//! does not verify is refused as a malformed parameter
//! (`InvalidRequestException`, HTTP 400) naming the value the execution-id
//! decoder could not decode, never as a retrieval that is gone
//! (`ResourceNotFoundException`, HTTP 404) and never served. The 404 belongs to a
//! value that verifies character for character and yet names no live retrieval.
//!
//! Rows differ along what that rule generalises over: WHICH character of a token
//! is mutated (swept over both halves and, on a genuine token, over the first,
//! middle and last position), WHAT it is mutated to (every other character of the
//! minted alphabet, in both cases, so a case-only mutation is one row of the
//! sweep), which operation was called and therefore which member its refusal
//! names, and — for the contrast rows — which of the two bad-token classes the
//! value falls in.

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
pub(crate) const TOKEN_READERS: [(&str, &str, &str); 3] = [
    ("CancelTraceRetrieval", "/CancelTraceRetrieval", "retrieval token"),
    ("ListRetrievedTraces", "/ListRetrievedTraces", "execution id"),
    (
        "GetRetrievedTracesGraph",
        "/GetRetrievedTracesGraph",
        "execution id",
    ),
];

/// Every character a minted `RetrievalToken` can carry, plus the upper case of
/// each hexadecimal letter: the sweep below mutates a token to each of these in
/// turn, so a mutation that only re-cases a character is one of its rows.
const MUTATIONS: &str = "0123456789abcdefABCDEF";

/// The decoder's clause for a value that is not of the issued token form at all:
/// fixed text, naming no value.
const NOT_TOKEN_FORM_CLAUSE: &str = "Invalid Execution Id";

/// The decoder's clause for a value of the issued token form whose integrity
/// check does not verify: fixed text, then the value it could not decode.
const UNDECODABLE_CLAUSE: &str = "Invalid execution id provided: ";

/// The request body a reader needs to reach its token check: `TraceFormat` is a
/// member of `ListRetrievedTraces` alone, and that operation judges it first.
pub(crate) fn body(path: &str, token: &str) -> Value {
    let mut body = json!({ "RetrievalToken": token });
    if path == "/ListRetrievedTraces" {
        body["TraceFormat"] = json!("XRAY");
    }
    body
}

/// What a value of the issued form whose integrity check does not verify is
/// refused with: the submitted value in the outer clause and again in the
/// decoder's.
pub(crate) fn undecodable_message(member: &str, token: &str) -> String {
    format!("Invalid {member} {token} passed: {UNDECODABLE_CLAUSE}{token}")
}

/// What a value that is not of the issued form at all is refused with: the
/// submitted value in the outer clause only.
fn not_token_form_message(member: &str, token: &str) -> String {
    format!("Invalid {member} {token} passed: {NOT_TOKEN_FORM_CLAUSE}")
}

/// A value assembled the way the mint assembles one — the identifying half, then
/// the check half derived from it — so it is a value this service COULD have
/// issued, character for character.
fn mintable(identity: &str) -> String {
    format!("{identity}{}", retrieval_token_check(identity))
}

/// `token` with the character at `index` in the other case and every other
/// character left exactly as it was.
fn recase(token: &str, index: usize) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    characters[index] = if characters[index].is_ascii_uppercase() {
        characters[index].to_ascii_lowercase()
    } else {
        characters[index].to_ascii_uppercase()
    };
    characters.into_iter().collect()
}

/// `token` with its first hexadecimal LETTER in the other case: the one
/// mutation that is invisible to a comparison which folds case, and the only one
/// a value made of digits alone cannot carry.
fn recase_a_letter(token: &str) -> String {
    let index = token
        .chars()
        .position(char::is_alphabetic)
        .unwrap_or_else(|| panic!("{token:?} must carry a letter to re-case"));
    recase(token, index)
}

/// `token` with the character at `index` replaced by `mutation`.
fn replace(token: &str, index: usize, mutation: char) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    characters[index] = mutation;
    characters.into_iter().collect()
}

/// A genuine `RetrievalToken`, issued by this emulator: one segment ingested,
/// then a retrieval started over the window it falls in.
pub(crate) async fn genuine_token(xray: &Xray, n: u32) -> String {
    let start = (now() - 600.0).trunc();
    let trace = trace_id(n);
    let ingested = xray
        .put_segments(&[root_document(
            "3333333333333333",
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
        "a minted RetrievalToken is {RETRIEVAL_TOKEN_HEX_DIGITS} characters, got {token:?}"
    );
    token
}

#[tokio::test]
async fn a_value_re_cased_at_any_position_no_longer_verifies() {
    let xray = Xray::new();

    // A value this service could have issued, carrying a letter in each half so
    // the sweep below has something to re-case on both sides of the token.
    let intact = mintable("0123456789abcdef");

    // It verifies character for character, so it is eligible for the 404 class:
    // a value that could have been issued and names no retrieval.
    for (operation, path, _member) in TOKEN_READERS {
        let answered = xray.call(path, &body(path, &intact)).await;
        assert_eq!(
            (answered.status, answered.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "on {operation}: a value that verifies and names no retrieval must be a 404, got \
             {answered:?}"
        );
    }

    // Re-casing ONE character of it, anywhere, leaves a value this service never
    // issued: the check no longer derives from the identifying half, so every
    // reader refuses it as a malformed parameter in the integrity-failure words.
    let mut swept = 0;
    for index in 0..intact.len() {
        if !intact.chars().nth(index).is_some_and(char::is_alphabetic) {
            continue;
        }
        swept += 1;
        let mutated = recase(&intact, index);
        let case = format!("character {index} of a mintable value re-cased");
        assert_ne!(
            mutated, intact,
            "case {case:?}: the sweep must submit a value other than the mintable one"
        );
        for (operation, path, member) in TOKEN_READERS {
            let refused = xray.call(path, &body(path, &mutated)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let expected = undecodable_message(member, &mutated);
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
        }
    }
    assert!(
        swept > 0,
        "the sweep must have found a letter to re-case in {intact:?}"
    );
}

#[tokio::test]
async fn every_single_character_mutation_of_an_issued_token_is_an_integrity_failure() {
    let xray = Xray::new();
    let real = genuine_token(&xray, 21).await;

    // Rows differ in WHERE the mutation lands — the identifying half, the join
    // between the halves, and the check half — and in WHAT the character becomes:
    // every other character of the minted alphabet and every upper case of it, so
    // a mutation that only re-cases the character is swept alongside one that
    // changes its value.
    for index in [0, real.len() / 2, real.len() - 1] {
        for mutation in MUTATIONS.chars() {
            let mutated = replace(&real, index, mutation);
            if mutated == real {
                continue;
            }
            let case = format!("character {index} of an issued token mutated to {mutation:?}");
            for (operation, path, member) in TOKEN_READERS {
                let refused = xray.call(path, &body(path, &mutated)).await;
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {case:?} on {operation}: expected 400 InvalidRequestException, got \
                     {refused:?}"
                );
                let expected = undecodable_message(member, &mutated);
                let said = refused.message().unwrap_or_default();
                assert_eq!(
                    said, expected,
                    "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
                );
            }
        }
    }

    // None of those refusals reached the retrieval the values were derived from:
    // the issued token still reads, still is not cancelled, and still cancels.
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
        "no refused call may have cancelled the retrieval, got {listed:?}"
    );
    let cancelled = xray
        .call("/CancelTraceRetrieval", &json!({"RetrievalToken": real}))
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the issued token must still cancel its retrieval, got {cancelled:?}"
    );
}

#[tokio::test]
async fn each_bad_token_class_keeps_its_own_words() {
    let xray = Xray::new();
    let real = genuine_token(&xray, 22).await;

    // Both classes are the same refusal class — 400 `InvalidRequestException`,
    // before any lookup — and they differ only in the decoder clause: a value of
    // the issued form that was decoded and rejected names itself again, a value
    // that is not of that form at all names nothing. Rows differ in how the value
    // misses an issued one: re-cased, changed in value, and — for the contrast —
    // of another width or another alphabet entirely.
    let undecodable = [
        (
            "an issued token with one of its letters re-cased",
            recase_a_letter(&real),
        ),
        (
            "a mintable value with its first character re-cased",
            recase(&mintable("abcdef0123456789"), 0),
        ),
        (
            "the minted width and alphabet, carrying no check",
            "c".repeat(RETRIEVAL_TOKEN_HEX_DIGITS),
        ),
    ];
    let not_token_form = [
        (
            "one character short of the minted width",
            "c".repeat(RETRIEVAL_TOKEN_HEX_DIGITS - 1),
        ),
        (
            "a character outside the minted alphabet",
            format!("{}z", "c".repeat(RETRIEVAL_TOKEN_HEX_DIGITS - 1)),
        ),
        ("prose", "not-a-retrieval-token".to_string()),
    ];

    for (case, token) in undecodable.iter().chain(not_token_form.iter()) {
        let undecodable_class = undecodable.iter().any(|(_, row)| row == token);
        for (operation, path, member) in TOKEN_READERS {
            let refused = xray.call(path, &body(path, token)).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("InvalidRequestException")),
                "case {case:?} on {operation}: expected 400 InvalidRequestException, got {refused:?}"
            );
            let expected = if undecodable_class {
                undecodable_message(member, token)
            } else {
                not_token_form_message(member, token)
            };
            let said = refused.message().unwrap_or_default();
            assert_eq!(
                said, expected,
                "case {case:?} on {operation}: expected {expected:?}, got {said:?}"
            );
            // How often the sentence names the submitted value is what separates
            // the classes: twice when the decoder read it, once when it did not.
            let expected_echoes = if undecodable_class { 2 } else { 1 };
            assert_eq!(
                said.matches(token.as_str()).count(),
                expected_echoes,
                "case {case:?} on {operation}: expected the value named {expected_echoes} \
                 time(s), got {said:?}"
            );
        }
    }
}

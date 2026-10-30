// SPDX-License-Identifier: CC0-1.0

//! The WORDS this endpoint refuses a request with, end to end over HTTP.
//!
//! Three renderings are pinned here, each over a SET of inputs rather than over
//! one request, because each is a rule about a class of refusal:
//!
//! 1. a constraint the model declares on the ELEMENTS of a collection member is
//!    stated once at that MEMBER — no element position, no `member` step and no
//!    value — with every requirement the element shape declares nested inside
//!    `Member must satisfy constraint: [ … ]`. A constraint broken by the
//!    collection ITSELF keeps its own clause, and a scalar member keeps the
//!    value it echoes;
//! 2. a request that does not name exactly one stream — neither member or both —
//!    is refused with ONE fixed sentence, whichever of the two cases it is;
//! 3. a `StreamName` or `StreamARN` that resolves to no readable stream is
//!    refused with one fixed sentence that names neither the stream nor the
//!    member that addressed it.
//!
//! The harness is `archived_media_read_path`'s: the real `MultiServiceRouter`
//! with this one service registered, so every request walks routing, restJson1
//! deserialization, constraint validation and the handler.

use crate::archived_media_read_path::{
    body_json, fragment_number, read_loop_operations, seeded, with_members, STREAM,
};

/// The element constraint set `GetMediaForFragmentList$Fragments` states.
///
/// Read off the model, not off the request: `Fragments` targets
/// `FragmentNumberList`, whose member targets `FragmentNumberString`
/// (`smithy_query(query="shape",shape="FragmentNumberList,FragmentNumberString")`
/// — `@length(min: 1, max: 128)` and `@pattern("^[0-9]+$")`). Every element of
/// that member has to satisfy all three, which is what the clause states
/// whichever of them a request broke.
const FRAGMENT_ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                                    than or equal to 128, Member must have length greater than or \
                                    equal to 1, Member must satisfy regular expression pattern: \
                                    ^[0-9]+$]";

/// The code this endpoint answers a modelled-constraint refusal with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The sentence a request that does not name exactly one stream is refused with.
const STREAM_ADDRESSING: &str = "One of StreamName or StreamARN must be provided, but not both.";

/// The sentence a stream that cannot be read is refused with.
const STREAM_NOT_FOUND: &str = "The requested stream is not found or not active.";

/// The `__type` and `message`/`Message` a refusal reports.
fn reported(response: &http::Response<bytes::Bytes>) -> (String, String) {
    let body = body_json(response);
    let code = body["__type"].as_str().unwrap_or_default().to_string();
    // restJson1 carries the framework's constraint refusal under `message` and
    // this service's modelled errors under their `Message` member; a refusal
    // states exactly one of the two.
    let message = body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// A numeric fragment number of `digits` digits — the only shape the element
/// shape's `@pattern` admits, so a row is outside the `@length` maximum for its
/// length alone.
fn digits(count: usize) -> String {
    "9".repeat(count)
}

/// RULE: a constraint the model declares on the ELEMENTS of a collection member
/// is reported against that member, with the element shape's whole constraint
/// set and no value; a constraint the COLLECTION itself broke keeps its own
/// clause; a scalar member keeps the framework's clause and its echoed value.
///
/// The rows differ along what the rule generalises over: which of the element
/// shape's constraints was broken, how long the offending value is, how many
/// elements offended and at which positions, and whether the violation is about
/// the elements, the collection or another member entirely.
#[tokio::test]
async fn an_element_constraint_is_reported_against_the_collection_member() {
    let harness = seeded();
    let valid = fragment_number(0);
    // (case, the `Fragments` the request carries, the message expected)
    let rows: Vec<(&str, Vec<String>, String)> = vec![
        (
            "one element outside the element pattern",
            vec!["not-a-fragment-number".to_string()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an empty element, under the element minimum",
            vec![String::new()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an element one digit over the element maximum",
            vec![digits(129)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an element far over the element maximum",
            vec![digits(4096)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "two offending elements, breaking two different constraints",
            vec![valid.clone(), "abc".to_string(), digits(200)],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            "an offending element after several valid ones",
            vec![valid.clone(), valid.clone(), "0x2a".to_string()],
            format!(
                "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
        (
            // The collection's OWN `@length` is not an element constraint: it
            // keeps its own requirement, and still states no value, because a
            // list has no scalar rendering to quote.
            "the empty list, under the collection's own minimum",
            Vec::new(),
            "1 validation error detected: Value at 'fragments' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
    ];

    for (case, fragments, expected) in rows {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, message) = reported(&response);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a constraint broken by a SCALAR member is still the framework's own
/// clause — its path, its one requirement and the value the caller submitted —
/// and the envelope counts one clause per refused member, in the order the
/// model declares them.
#[tokio::test]
async fn a_scalar_member_keeps_its_own_clause_and_its_echoed_value() {
    let harness = seeded();
    let valid = fragment_number(0);
    // (case, the request body, the message expected)
    let rows: Vec<(&str, serde_json::Value, String)> = vec![
        (
            "a StreamName outside its pattern",
            serde_json::json!({ "StreamName": "bad name!", "Fragments": [valid.clone()] }),
            "1 validation error detected: Value 'bad name!' at 'streamName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"
                .to_string(),
        ),
        (
            "a StreamARN outside its pattern",
            serde_json::json!({ "StreamARN": "not-an-arn", "Fragments": [valid.clone()] }),
            "1 validation error detected: Value 'not-an-arn' at 'streamARN' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$"
                .to_string(),
        ),
        (
            "a scalar member and an element, both refused",
            serde_json::json!({ "StreamName": "bad name!", "Fragments": ["nope"] }),
            format!(
                "2 validation errors detected: Value 'bad name!' at 'streamName' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 ^[a-zA-Z0-9_.-]+$; Value at 'fragments' failed to satisfy constraint: \
                 {FRAGMENT_ELEMENT_SET}"
            ),
        ),
    ];

    for (case, body, expected) in rows {
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, message) = reported(&response);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {case}: expected {VALIDATION_EXCEPTION}, got {code:?}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: every operation that reads a stream refuses a request that does not
/// name exactly one with ONE fixed sentence (`InvalidArgumentException`, 400),
/// and one whose name or ARN resolves to no readable stream with one fixed
/// sentence that carries nothing of the request (`ResourceNotFoundException`,
/// 404).
///
/// The rows differ along what the two sentences generalise over: which of the
/// two addressing members was supplied, the spelling and length of the name, and
/// the ARN's region, account and creation suffix — none of which may appear in
/// either message.
#[tokio::test]
async fn the_stream_addressing_refusals_are_one_fixed_sentence_each() {
    let harness = seeded();
    // (case, the stream-naming members, the status, the error, the message)
    let cases: Vec<(&str, serde_json::Value, u16, &str, &str)> = vec![
        (
            "both StreamName and StreamARN",
            serde_json::json!({ "StreamName": STREAM, "StreamARN": harness.arn.clone() }),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "both, where neither names a held stream",
            serde_json::json!({
                "StreamName": "other-stream",
                "StreamARN": "arn:aws:kinesisvideo:eu-west-1:000000000001:stream/other/1700000000",
            }),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "neither StreamName nor StreamARN",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
            STREAM_ADDRESSING,
        ),
        (
            "a StreamName no stream answers to",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a differently spelled absent StreamName",
            serde_json::json!({ "StreamName": "probe.stream_7-ABSENT" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a one-character absent StreamName",
            serde_json::json!({ "StreamName": "x" }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "a StreamARN no stream answers to",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000"
            }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
        (
            "an absent StreamARN in another region and account",
            serde_json::json!({
                "StreamARN": "arn:aws-cn:kinesisvideo:cn-north-1:123456789012:stream/gone/1",
            }),
            404,
            "ResourceNotFoundException",
            STREAM_NOT_FOUND,
        ),
    ];

    for (path, extra) in read_loop_operations() {
        for (case, stream, status, error, expected) in &cases {
            let body = with_members(stream, &extra);
            let response = harness.post(path, &body).await;
            assert_eq!(
                response.status().as_u16(),
                *status,
                "case {path} / {case}: expected HTTP {status}, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let (code, message) = reported(&response);
            assert_eq!(
                code, *error,
                "case {path} / {case}: expected {error}, got {code:?}"
            );
            assert_eq!(
                message, *expected,
                "case {path} / {case}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

/// RULE: the not-found sentence is value-free — it names neither the stream the
/// request asked for nor the member that addressed it — so no part of the
/// request can be read out of it.
#[tokio::test]
async fn the_not_found_sentence_carries_nothing_of_the_request() {
    let harness = seeded();
    // Names and ARNs whose substrings would be visible in any message built
    // from the request.
    let probes = [
        ("a StreamName", "StreamName", "sim2real-probe-kvam-lf-missing"),
        (
            "a StreamARN",
            "StreamARN",
            "arn:aws:kinesisvideo:us-west-2:000000000000:stream/probe-missing/1699999999",
        ),
    ];
    for (case, member, value) in probes {
        let body = serde_json::json!({
            member: value,
            "Fragments": [fragment_number(0)],
        });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            404,
            "case {case}: expected HTTP 404, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (_, message) = reported(&response);
        assert_eq!(
            message, STREAM_NOT_FOUND,
            "case {case}: expected {STREAM_NOT_FOUND:?}, got {message:?}"
        );
        for leaked in [value, member] {
            assert!(
                !message.contains(leaked),
                "case {case}: the message must carry nothing of the request, but {message:?} \
                 contains {leaked:?}"
            );
        }
    }
}

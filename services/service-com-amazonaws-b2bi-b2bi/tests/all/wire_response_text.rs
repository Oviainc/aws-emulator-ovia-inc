// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT and the STATUS this service answers with, driven over
//! the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same router the
//! emulator boots, so every assertion is about what a real SDK client observes.
//!
//! Three rules are under test, each over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! * a success of this service is HTTP 200. awsJson1_0 is an RPC protocol, so the `@http`
//!   code the model puts on an operation is inert on the wire
//!   (`smithy_query(query="operation",operation=...)` reports `httpBindingsInert`), and
//!   the rows are every operation whose modelled code is not 200.
//! * an unusable `nextToken` is refused with ONE fixed sentence, naming neither the
//!   member it arrived in nor the token itself, on every paginated operation — so a
//!   client that tells a garbage or expired cursor from another `ValidationException` by
//!   the message reads the same sentence from all of them. The rows differ in HOW the
//!   token is unusable and in its length.
//! * `TestMapping` answers the mapped DOCUMENT as text in its `String` member,
//!   pretty-printed with two-space indentation — a document, not a structure. The rows
//!   differ in the result's JSON type, its member count, how deeply it nests, and the
//!   order the request's own text writes its members in.
//!
//! The starter mapping template's own fields are covered by
//! `tests/all/starter_mapping_template.rs`; this suite adds the one rule about the value
//! it writes wherever the caller must supply content.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::ops::create_starter_mapping_template::PLACEHOLDER;
use service_com_amazonaws_b2bi_b2bi::state::INVALID_PAGE_TOKEN_MESSAGE;

use crate::edi_parsing::{given_object, BUCKET, X12_850};
use crate::onboarding_loop::{
    b2bi_router, call, capability_configuration, error_code, given_capability,
    given_capability_buckets, given_profile, given_transformer,
};

/// The X12 transaction set and version every row that needs one names.
fn x12_details() -> Value {
    json!({ "transactionSet": "X12_850", "version": "VERSION_4010" })
}

/// Invoke `TestMapping` over `input` with `template`.
async fn test_mapping(
    router: &smithy_server_runtime::MultiServiceRouter,
    input: &str,
    template: &str,
) -> (u16, Value) {
    call(
        router,
        "TestMapping",
        json!({
            "fileFormat": "JSON",
            "inputFileContent": input,
            "mappingTemplate": template
        }),
    )
    .await
}

/// RULE: a success of this service is HTTP 200 — awsJson1_0 is an RPC protocol, so the
/// `@http` code an operation's model carries is inert on the wire.
///
/// The rows are every operation of this service whose modelled `@http` code is NOT 200
/// (`CreateProfile`, `CreateCapability`, `CreatePartnership`, `CreateTransformer`,
/// `TestMapping` and `TestParsing` all carry 201), so none of them may answer that code;
/// two operations whose modelled code IS 200 ride along, so the table says 200 is the
/// answer rather than merely the number this emulator writes everywhere.
#[tokio::test]
async fn every_success_of_this_service_answers_200() {
    let router = b2bi_router();
    given_object(&router, "status/sample-850.edi", X12_850).await;
    let profile_id = given_profile(&router, "status-profile").await;
    let capability_id = given_capability(&router, "status-capability").await;
    let transformer_id = given_transformer(&router, "status-capability-transformer").await;
    given_capability_buckets(&router).await;

    // Each row: the operation and a request body satisfying its modelled required members
    // (`smithy_query(query="operation",operation=...)`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateProfile",
            json!({
                "name": "status-200",
                "phone": "555-555-0100",
                "businessName": "Status Corp",
                "logging": "DISABLED"
            }),
        ),
        (
            "CreateTransformer",
            json!({
                "name": "status-transformer",
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": { "x12": x12_details() }
                },
                "mapping": { "templateLanguage": "JSONATA", "template": "$" }
            }),
        ),
        (
            "CreateCapability",
            json!({
                "name": "status-edi",
                "type": "edi",
                "configuration": capability_configuration(
                    &transformer_id,
                    "X12_850",
                    "VERSION_4010"
                )
            }),
        ),
        (
            "CreatePartnership",
            json!({
                "profileId": profile_id,
                "name": "status-partnership",
                "email": "partner@example.com",
                "capabilities": [capability_id]
            }),
        ),
        (
            "TestMapping",
            json!({
                "fileFormat": "JSON",
                "inputFileContent": "{\"total\":1}",
                "mappingTemplate": "$"
            }),
        ),
        (
            "TestParsing",
            json!({
                "inputFile": { "bucketName": BUCKET, "key": "status/sample-850.edi" },
                "fileFormat": "JSON",
                "ediType": { "x12Details": x12_details() }
            }),
        ),
        // Modelled 200 already, so the table is about the protocol rather than about one
        // scaffolded constant.
        ("ListProfiles", json!({})),
        (
            "CreateStarterMappingTemplate",
            json!({ "mappingType": "JSONATA", "templateDetails": { "x12": x12_details() } }),
        ),
    ];

    for (operation, body) in cases {
        let (status, response) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: a success of an awsJson1_0 service is 200, got {status} \
             {response}"
        );
    }
}

/// RULE: a `nextToken` this service cannot use is refused with the modelled
/// `ValidationException` (HTTP 400) carrying ONE fixed sentence — it names neither the
/// member the token arrived in nor the token's own text — on every paginated operation.
///
/// The rows differ in how the token is unusable (characters outside the token alphabet, an
/// odd number of them, bytes that decode to no text) and in its length, from one character
/// to the longest the modelled `PageToken` (`@length 1..2048`) admits.
#[tokio::test]
async fn an_unusable_pagination_token_is_refused_with_one_fixed_sentence() {
    let router = b2bi_router();

    // The recorded sentence, pinned literally here and asserted to be the one the handlers
    // share, so neither the tests nor the three operations can drift from it.
    const REFUSAL: &str = "Invalid pagination token";
    assert_eq!(
        INVALID_PAGE_TOKEN_MESSAGE, REFUSAL,
        "the service's pagination refusal must be the recorded sentence"
    );

    // Each row: the case and the token. Every one is within the modelled `len 1..2048`,
    // so it reaches the handler rather than the boundary validator.
    let longest = "z".repeat(2048);
    let cases: Vec<(&str, &str)> = vec![
        ("one character outside the alphabet", "z"),
        ("a word", "not-a-token"),
        ("an odd number of token characters", "abc"),
        ("characters outside the alphabet", "zzzz"),
        ("bytes that decode to no text", "ff"),
        ("a token of the greatest modelled length", longest.as_str()),
    ];

    for operation in ["ListProfiles", "ListCapabilities", "ListPartnerships"] {
        for (case, token) in &cases {
            let case = format!("{operation} / {case}");
            let (status, body) = call(&router, operation, json!({ "nextToken": token })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an unusable token is refused with 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(REFUSAL),
                "case {case:?}: expected {REFUSAL:?}, got {body}"
            );
        }
    }
}

/// RULE: `TestMapping` answers the mapped DOCUMENT as text in its `String` member,
/// pretty-printed with two-space indentation — a document, not a structure, and not the
/// compact serialization of one — with the text JSON-string-encoded into the member, so a
/// client parses the member once for the mapped file's text.
///
/// Every object's members are written in sorted key order, which makes the answer a
/// function of the request alone: the interpreter holds an object's members in a hash map,
/// so its own order varies between evaluations, and the order the real service answers
/// (the one its mapping constructed) is not reproducible from it. The rows differ in the
/// result's JSON type (object, array, string, number), its member count, how deeply it
/// nests, and the order the request's own text writes the members in — which, being
/// sorted here, the answer does not follow.
#[tokio::test]
async fn a_mapped_document_is_answered_as_pretty_printed_text() {
    let router = b2bi_router();

    // Each row: the case, the input document, the template, and the EXACT text the
    // `mappedFileContent` string carries.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "three members, written in neither the template's nor the document's order",
            r#"{"quantity":3,"item":"widget","orderId":"12345"}"#,
            r#"{"orderId": orderId, "item": item, "quantity": quantity}"#,
            "{\n  \"item\": \"widget\",\n  \"orderId\": \"12345\",\n  \"quantity\": 3\n}",
        ),
        (
            "a computed member and a counted one",
            r#"{"items":[{"amount":3},{"amount":4}]}"#,
            r#"{"total": $sum(items.amount), "count": $count(items)}"#,
            "{\n  \"count\": 2,\n  \"total\": 7\n}",
        ),
        (
            "a nested object, indented one level deeper",
            r#"{"z":1,"y":2}"#,
            r#"{"outer": {"second": y, "first": z}, "after": z}"#,
            "{\n  \"after\": 1,\n  \"outer\": {\n    \"first\": 1,\n    \"second\": 2\n  }\n}",
        ),
        (
            "an array of objects",
            r#"{"rows":[{"b":1,"a":2}]}"#,
            r#"{"rows": [{"b": rows[0].b, "a": rows[0].a}]}"#,
            "{\n  \"rows\": [\n    {\n      \"a\": 2,\n      \"b\": 1\n    }\n  ]\n}",
        ),
        (
            "the identity template over a document written out of order",
            r#"{"third":3,"first":1,"second":2}"#,
            "$",
            "{\n  \"first\": 1,\n  \"second\": 2,\n  \"third\": 3\n}",
        ),
        (
            "a member whose name the template computes",
            r#"{"b":1}"#,
            r#"{"b": b, $string("z"): 1, $string("a"): 2}"#,
            "{\n  \"a\": 2,\n  \"b\": 1,\n  \"z\": 1\n}",
        ),
        (
            "an empty object",
            r#"{"a":1}"#,
            "{}",
            "{}",
        ),
        (
            "an array of scalars",
            r#"{"a":[1,2,3]}"#,
            "a",
            "[\n  1,\n  2,\n  3\n]",
        ),
        (
            "a string result, which is one JSON value and carries no indentation",
            r#"{"a":"text"}"#,
            "a",
            "\"text\"",
        ),
        (
            "a number result",
            r#"{"a":[1,2,3]}"#,
            "$sum(a)",
            "6",
        ),
    ];

    for (case, input, template, expected) in cases {
        let (status, body) = test_mapping(&router, input, template).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the mapping must be served, got {status} {body}"
        );
        let member = body["mappedFileContent"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: mappedFileContent must be a string, got {body}")
        });
        // The member is a JSON string literal whose content is the mapped file's text:
        // one parse yields that text.
        let mapped: String = serde_json::from_str(member).unwrap_or_else(|failure| {
            panic!(
                "case {case:?}: mappedFileContent {member:?} must hold the mapped file's text: \
                 {failure}"
            )
        });
        assert_eq!(
            mapped, expected,
            "case {case:?}: expected the mapped document as {expected:?}, got {mapped:?}"
        );
        // And it is the mapped document, not merely text that looks like one: the string
        // parses back as JSON.
        serde_json::from_str::<Value>(&mapped).unwrap_or_else(|failure| {
            panic!("case {case:?}: mappedFileContent {mapped:?} must be JSON: {failure}")
        });
    }
}

/// RULE: a starter mapping template generated from `templateDetails` writes the service's
/// one placeholder value wherever the caller must supply the content, and the segments of
/// the transaction's body sit in its `segments` list — each one an object carrying its
/// `segment_id` and its elements keyed by position — which is the shape this service's X12
/// reader builds for a parsed document and therefore the shape a mapping written against
/// the template produces.
///
/// The rows differ in the transaction set (so in the beginning segment and its element
/// count) and in the X12 release.
#[tokio::test]
async fn a_starter_template_marks_every_caller_supplied_field_with_one_placeholder() {
    let router = b2bi_router();

    // Each row: the case, the `X12TransactionSet` and `X12Version` values, and the X12
    // segment id that opens that transaction set's body.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        ("850 purchase order", "X12_850", "VERSION_4010", "BEG"),
        ("810 invoice at release 5010", "X12_810", "VERSION_5010", "BIG"),
        ("820 payment order", "X12_820", "VERSION_4060", "BPR"),
        (
            "834 benefit enrollment at release 5010 HIPAA",
            "X12_834",
            "VERSION_5010_HIPAA",
            "BGN",
        ),
    ];

    for (case, transaction_set, version, segment_id) in cases {
        let (status, body) = call(
            &router,
            "CreateStarterMappingTemplate",
            json!({
                "mappingType": "JSONATA",
                "templateDetails": {
                    "x12": { "transactionSet": transaction_set, "version": version }
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the template must be generated, got {status} {body}"
        );
        let template = body["mappingTemplate"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a mappingTemplate, got {body}")
        });
        // The layout the generated template is recorded with: two spaces of indentation
        // per level, a space on BOTH sides of a member's colon, and an array on the line
        // it opens on — so the interchange object's own members sit four spaces in.
        assert!(
            template.starts_with("{\n  \"interchanges\" : [ {\n    \"ISA_01_"),
            "case {case:?}: expected the recorded layout, got {template}"
        );
        assert!(
            !template.contains("\":"),
            "case {case:?}: a member's colon carries a space on both sides, got {template}"
        );
        let document: Value = serde_json::from_str(template).unwrap_or_else(|failure| {
            panic!("case {case:?}: a JSONATA template is a JSON document: {failure}: {template}")
        });

        let transaction =
            &document["interchanges"][0]["functional_groups"][0]["transactions"][0];
        let body_segment = &transaction["segments"][0];
        assert_eq!(
            body_segment["segment_id"].as_str(),
            Some(segment_id),
            "case {case:?}: the body's first segment must be {segment_id}, got {transaction}"
        );
        assert!(
            template.contains(&format!("\"segment_id\" : \"{segment_id}\"")),
            "case {case:?}: the segments list carries the segment id, got {template}"
        );

        // Every element of that segment is the placeholder, keyed by segment and
        // position alone.
        let elements = body_segment
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a segment is an object, got {body_segment}"));
        let mut positions = 0usize;
        for (key, value) in elements {
            if key == "segment_id" {
                continue;
            }
            positions += 1;
            assert_eq!(
                key,
                &format!("{segment_id}_{positions:02}"),
                "case {case:?}: a body element is keyed by segment and position, got {key:?}"
            );
            assert_eq!(
                value.as_str(),
                Some(PLACEHOLDER),
                "case {case:?}: {key} must be the placeholder, got {value}"
            );
        }
        assert!(
            positions > 0,
            "case {case:?}: {segment_id} must carry its elements, got {body_segment}"
        );

        // Nothing anywhere in the template is a sentence this emulator composed: every
        // value is either a value the request determines or the placeholder itself.
        for value in flat_values(&document) {
            assert!(
                value == PLACEHOLDER || !value.contains(' '),
                "case {case:?}: a template value is a determined value or the placeholder, \
                 got {value:?}"
            );
        }
    }
}

/// Every string value of `document`, at any depth.
fn flat_values(document: &Value) -> Vec<String> {
    match document {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items.iter().flat_map(flat_values).collect(),
        Value::Object(members) => members.values().flat_map(flat_values).collect(),
        _ => Vec::new(),
    }
}

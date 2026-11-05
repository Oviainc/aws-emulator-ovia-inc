// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `TestMapping` CARRIES the mapped file, driven over the wire
//! (awsJson1_0, `x-amz-target: B2BI.TestMapping`) through the same router the emulator
//! boots.
//!
//! The rule under test: `mappedFileContent` is a `String`, and what it holds is the mapped
//! file's TEXT as a JSON string literal — so a client parses the member once for that text
//! and parses the text for the mapped document. The encoding is a property of the MEMBER,
//! not of the result's JSON type: a mapping whose result is a scalar is recovered by the
//! same two steps as one whose result is an object, which is what makes the member
//! readable at all for a projecting mapping.
//!
//! The rows differ along what the rule generalises over: the JSON type of the mapped
//! result (object, array, string, number, boolean, null), whether the mapped text needs
//! escaping (quotes, newlines, a backslash) and whether the result has no value at all.
//! What the mapping COMPUTES is asserted by `tests/all/test_mapping.rs`; this suite is
//! about the layer that carries it.

use serde_json::{json, Value};

use crate::onboarding_loop::{b2bi_router, call};

/// One row: the case, the input document, the template, and the TEXT the mapped file must
/// hold (what one parse of the member yields).
type EncodingCase<'a> = (&'a str, &'a str, &'a str, &'a str);

/// Invoke `TestMapping` and return the raw `mappedFileContent` member.
async fn mapped_member(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    input: &str,
    template: &str,
) -> String {
    let (status, body) = call(
        router,
        "TestMapping",
        json!({
            "fileFormat": "JSON",
            "inputFileContent": input,
            "mappingTemplate": template,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the mapping must be served, got {status} {body}"
    );
    body["mappedFileContent"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: mappedFileContent must be a string, got {body}"))
        .to_string()
}

/// RULE: `mappedFileContent` holds the mapped file's text JSON-string-encoded, whatever
/// the mapped result's JSON type.
#[tokio::test]
async fn mapped_file_content_holds_the_mapped_text_json_string_encoded() {
    let router = b2bi_router();

    let cases: Vec<EncodingCase> = vec![
        (
            "an object result",
            r#"{"orderId":"PO-1","amount":2}"#,
            "$",
            "{\n  \"amount\": 2,\n  \"orderId\": \"PO-1\"\n}",
        ),
        (
            "an array result",
            r#"{"items":[1,2]}"#,
            "items",
            "[\n  1,\n  2\n]",
        ),
        (
            "a string result, the projecting mapping a caller validates with",
            r#"{"orderId":"PO-10042"}"#,
            "orderId",
            "\"PO-10042\"",
        ),
        (
            "a number result",
            r#"{"items":[1,2,3]}"#,
            "$sum(items)",
            "6",
        ),
        (
            "a boolean result",
            r#"{"amount":40}"#,
            "amount > 10",
            "true",
        ),
        (
            "a null member",
            r#"{"note":null}"#,
            "{\"note\": note}",
            "{\n  \"note\": null\n}",
        ),
        (
            "a string carrying the characters JSON escapes",
            r#"{"note":"a \"quoted\" line\nand a \\ backslash"}"#,
            "note",
            "\"a \\\"quoted\\\" line\\nand a \\\\ backslash\"",
        ),
        (
            "an object whose member value needs escaping",
            r#"{"note":"line\none"}"#,
            "$",
            "{\n  \"note\": \"line\\none\"\n}",
        ),
    ];

    for (case, input, template, expected_text) in cases {
        let member = mapped_member(&router, case, input, template).await;

        // The member is a JSON string literal: ONE parse yields the mapped file's text,
        // never the document itself.
        let parsed: Value = serde_json::from_str(&member).unwrap_or_else(|failure| {
            panic!("case {case:?}: mappedFileContent {member:?} must be JSON: {failure}")
        });
        let text = parsed.as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: one parse of mappedFileContent must yield the mapped file's \
                 text, got {parsed}"
            )
        });
        assert_eq!(
            text, expected_text,
            "case {case:?}: expected the mapped text {expected_text:?}, got {text:?}"
        );

        // …and parsing that text yields the mapped document, which is what the caller
        // reads the member for.
        let document: Value = serde_json::from_str(text).unwrap_or_else(|failure| {
            panic!("case {case:?}: the mapped text {text:?} must be the document: {failure}")
        });
        let round_tripped = serde_json::to_string(&document).unwrap_or_default();
        assert!(
            !round_tripped.is_empty(),
            "case {case:?}: the mapped document must be a JSON value, got {document}"
        );
    }
}

/// RULE (the same member, read as a client reads it): the two parses are what recover the
/// mapped document, and the identity template recovers the INPUT document — for a document
/// of any shape.
///
/// The rows differ along the document's shape (object, array, nested, scalar members) and
/// the characters its values carry.
#[tokio::test]
async fn two_parses_recover_the_input_document_for_an_identity_mapping() {
    let router = b2bi_router();

    let documents: &[(&str, &str)] = &[
        ("a flat object", r#"{"a":1,"b":"two","c":false,"d":null}"#),
        ("an array of objects", r#"[{"a":1},{"a":2}]"#),
        (
            "a nested document",
            r#"{"outer":{"inner":[1,{"deep":"value"}]}}"#,
        ),
        ("an empty object", "{}"),
        ("an empty array", "[]"),
        (
            "values carrying quotes and newlines",
            r#"{"note":"a \"quoted\" line\nand more"}"#,
        ),
    ];

    for (case, input) in documents {
        let member = mapped_member(&router, case, input, "$").await;
        let text: String = serde_json::from_str(&member).unwrap_or_else(|failure| {
            panic!("case {case:?}: mappedFileContent {member:?} must hold text: {failure}")
        });
        let mapped: Value = serde_json::from_str(&text)
            .unwrap_or_else(|failure| panic!("case {case:?}: {text:?} must be JSON: {failure}"));
        let expected: Value = serde_json::from_str(input)
            .unwrap_or_else(|failure| panic!("case {case:?}: the row's own document: {failure}"));
        assert_eq!(
            mapped, expected,
            "case {case:?}: the identity mapping must recover the input document, got {mapped}"
        );
    }
}

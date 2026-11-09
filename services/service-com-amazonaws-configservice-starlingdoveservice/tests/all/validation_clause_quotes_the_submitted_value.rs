// SPDX-License-Identifier: CC0-1.0

//! The VALUE a `ValidationException` clause quotes, on the wire.
//!
//! A Config request-validation refusal is built from the request —
//! `N validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>` — and the text between the quotes is the submitted value
//! itself, however long it is. It is the only place the refusal reports the rejected input
//! back, so a caller (or an operator reading a log) that recovers the value out of the
//! message gets its own bytes: no prefix of them, and no annotation summarising the rest.
//!
//! The rule is a property of the RENDERING, not of one operation or one member, so each
//! case below is a row of a table spanning what it generalises over: the operation, the
//! member and the casing its path is spelled in, the constraint class the value broke, how
//! far past the declared bound the value sits, where in a collection it sits, and whether
//! its characters are single- or multi-byte.
//!
//! Every case drives the emulator the way a client does, through the same
//! `MultiServiceRouter` the production router builds, reusing the `Emulator` harness
//! `config_setup_and_rule_loop` provides and the `refusal` helper `config_connectors`
//! shares.

use serde_json::{json, Value};

use crate::config_connectors::refusal;
use crate::config_setup_and_rule_loop::Emulator;

/// An entry of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const S3_BUCKET: &str = "AWS::S3::Bucket";

/// An ARN of the form `TagResource$ResourceArn` takes (`AmazonResourceName`, `@length`
/// 1..1000): well formed, so the only rule the tagging requests below break is the one
/// each case is about.
const RULE_ARN: &str = "arn:aws:config:us-east-1:000000000000:config-rule/config-rule-abc123";

/// A conformance pack name satisfying `ConformancePackName` (`@length` 1..256,
/// `^[a-zA-Z][-a-zA-Z0-9]*$`), for the case whose only broken rule is the template's
/// length.
const PACK_NAME: &str = "pack-one";

/// The clause Config states for a value longer than its member's `@length` maximum: the
/// fixed text, the submitted value quoted in full, the member path the validating walk
/// names, and the maximum the model declares for that member.
fn over_maximum(value: &str, path: &str, max: usize) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must have length less than or equal to {max}"
    )
}

/// A `Tags` list whose entry at the 1-based position `at` carries `value`, every earlier
/// entry being well formed — so the clause's path names the position the over-long value
/// was sent at.
fn tags_with(at: usize, value: &str) -> Value {
    let mut tags: Vec<Value> = Vec::new();
    for position in 1..at {
        tags.push(json!({ "Key": format!("key-{position}"), "Value": "v" }));
    }
    tags.push(json!({ "Key": "overlong", "Value": value }));
    Value::Array(tags)
}

#[tokio::test]
async fn a_constraint_clause_quotes_the_whole_submitted_value() {
    let emulator = Emulator::new();

    // The member maxima each row sits past come from the model:
    // `smithy_query(query="operation", operation="GetResourceConfigHistory"|"TagResource"
    // |"PutConformancePack")` — `resourceId` -> `ResourceId` 1..768, a tag's `Value` ->
    // `TagValue` 0..256, `TemplateBody` 1..51200, the largest `@length` maximum this model
    // declares.
    let one_past_resource_id = "x".repeat(769);
    let far_past_resource_id = "y".repeat(2_000);
    // 769 CHARACTERS of two bytes each: `@length` counts characters, so this breaks the
    // same bound while the quoted text is 1538 bytes long.
    let multibyte_resource_id = "é".repeat(769);
    let one_past_tag_value = "v".repeat(257);
    let far_past_tag_value = "w".repeat(1_500);
    let one_past_template = "t".repeat(51_201);

    let rows: [(&str, &str, Value, String); 7] = [
        (
            "a resource id one character past its maximum",
            "GetResourceConfigHistory",
            json!({ "resourceType": S3_BUCKET, "resourceId": one_past_resource_id }),
            over_maximum(&one_past_resource_id, "resourceId", 768),
        ),
        (
            "a resource id far past its maximum",
            "GetResourceConfigHistory",
            json!({ "resourceType": S3_BUCKET, "resourceId": far_past_resource_id }),
            over_maximum(&far_past_resource_id, "resourceId", 768),
        ),
        (
            "a resource id of multi-byte characters",
            "GetResourceConfigHistory",
            json!({ "resourceType": S3_BUCKET, "resourceId": multibyte_resource_id }),
            over_maximum(&multibyte_resource_id, "resourceId", 768),
        ),
        (
            "a tag value one character past its maximum",
            "TagResource",
            json!({ "ResourceArn": RULE_ARN, "Tags": tags_with(1, &one_past_tag_value) }),
            over_maximum(&one_past_tag_value, "tags.1.member.value", 256),
        ),
        (
            "a tag value far past its maximum",
            "TagResource",
            json!({ "ResourceArn": RULE_ARN, "Tags": tags_with(1, &far_past_tag_value) }),
            over_maximum(&far_past_tag_value, "tags.1.member.value", 256),
        ),
        (
            "an over-long tag value at a later position",
            "TagResource",
            json!({ "ResourceArn": RULE_ARN, "Tags": tags_with(3, &one_past_tag_value) }),
            over_maximum(&one_past_tag_value, "tags.3.member.value", 256),
        ),
        (
            "the largest member this model declares, one character past it",
            "PutConformancePack",
            json!({ "ConformancePackName": PACK_NAME, "TemplateBody": one_past_template }),
            over_maximum(&one_past_template, "templateBody", 51_200),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, code, message) = refusal(&emulator, operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?} on {operation}: expected HTTP 400 ValidationException, got \
             {status} {code} with a {} byte message",
            message.len()
        );
        assert_eq!(
            message,
            expected,
            "case {case:?} on {operation}: expected the clause to quote the submitted value \
             in full ({} bytes), got a {} byte message ending {:?}",
            expected.len(),
            message.len(),
            message.chars().rev().take(80).collect::<String>(),
        );
    }
}

#[tokio::test]
async fn a_quoted_value_carries_no_summary_of_itself() {
    let emulator = Emulator::new();

    // Rows differ along the length of the value sent, from one the framework quotes
    // without help to ones well past its own quoting ceiling: whatever the length, the
    // clause carries the value and nothing else about it. A rendering that summarised the
    // value would say so in brackets, which is what this looks for.
    let lengths: [(&str, usize); 5] = [
        ("one character past the maximum", 769),
        ("a thousand characters", 1_000),
        ("four thousand characters", 4_000),
        ("sixteen thousand characters", 16_000),
        ("sixty thousand characters", 60_000),
    ];

    for (case, length) in lengths {
        let value = "z".repeat(length);
        let (status, code, message) = refusal(
            &emulator,
            "GetResourceConfigHistory",
            &json!({ "resourceType": S3_BUCKET, "resourceId": value }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
        assert_eq!(
            message,
            over_maximum(&value, "resourceId", 768),
            "case {case:?}: expected the clause to quote all {length} characters, got a {} \
             byte message",
            message.len()
        );
        assert!(
            !message.contains("bytes]"),
            "case {case:?}: a clause states no summary of the value it quotes, got a {} byte \
             message ending {:?}",
            message.len(),
            message.chars().rev().take(80).collect::<String>(),
        );
    }
}

#[tokio::test]
async fn a_value_that_breaks_two_constraints_is_quoted_in_both_clauses() {
    let emulator = Emulator::new();

    // `DeletePendingAggregationRequest$RequesterAccountId` targets `AccountId`
    // (`smithy_query(query="shape", shape="AccountId")`: `^\d{12}$`), which Config states
    // as both a length and a pattern requirement — so one over-long value is quoted in
    // each of the two clauses. Rows differ along the value's length and its characters.
    let rows: [(&str, String); 3] = [
        ("thirteen digits", "9".repeat(13)),
        ("three hundred digits", "8".repeat(300)),
        ("a thousand characters that are not digits", "q".repeat(1_000)),
    ];

    for (case, value) in rows {
        let (status, code, message) = refusal(
            &emulator,
            "DeletePendingAggregationRequest",
            &json!({ "RequesterAccountId": value, "RequesterAwsRegion": "us-east-1" }),
        )
        .await;
        let expected = format!(
            "2 validation errors detected: Value '{value}' at 'requesterAccountId' failed \
             to satisfy constraint: Member must have length less than or equal to 12; Value \
             '{value}' at 'requesterAccountId' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: \\d{{12}}"
        );
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {code}"
        );
        assert_eq!(
            message,
            expected,
            "case {case:?}: expected both clauses to quote the submitted value in full, got \
             a {} byte message",
            message.len()
        );
    }
}

#[tokio::test]
async fn a_value_inside_its_bound_is_served_rather_than_refused() {
    let emulator = Emulator::new();

    // The controls: the rule above is about a value OUTSIDE a declared bound, so a value
    // at the bound reaches the handler, which answers its own refusal for a resource key
    // the recorder never captured — quoting the same long id it was asked about. Rows
    // differ along the length of a resource id the model admits.
    for (case, length) in [
        ("the length maximum", 768_usize),
        ("a short id", 12),
        ("a single character", 1),
    ] {
        let resource_id = "b".repeat(length);
        let (status, code, message) = refusal(
            &emulator,
            "GetResourceConfigHistory",
            &json!({ "resourceType": S3_BUCKET, "resourceId": resource_id }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotDiscoveredException"),
            "case {case:?}: a value the model admits is judged by the handler, got {status} \
             {code} with message {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "Resource {resource_id} of resourceType:{S3_BUCKET} is unknown or has not \
                 been discovered"
            ),
            "case {case:?}: expected the never-discovered sentence naming the key asked \
             about, got a {} byte message",
            message.len()
        );
    }

    // And a tag list every entry of which is inside its bounds is not refused by the
    // boundary at all: the tagging refusal that follows is about the resource, not the
    // values sent.
    let (status, code, _) = refusal(
        &emulator,
        "TagResource",
        &json!({
            "ResourceArn": RULE_ARN,
            "Tags": [{ "Key": "owner", "Value": "v".repeat(256) }],
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ResourceNotFoundException"),
        "case \"a tag value at its maximum\": expected the resource refusal, got {status} \
         {code}"
    );
}

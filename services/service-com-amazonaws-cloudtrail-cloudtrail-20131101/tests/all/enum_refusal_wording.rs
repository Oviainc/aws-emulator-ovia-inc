// SPDX-License-Identifier: CC0-1.0

//! What a boundary refusal of a value outside a modelled enum value set SAYS.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the
//! error code and the `Message`.
//!
//! The rule under test: CloudTrail refuses a value outside an enumerated type's value
//! set with `400 ValidationException` whose clause names the MEMBER PATH and the value
//! set the MODEL declares, and quotes NOTHING of the rejected value — `1 validation
//! error detected: Value at '<member path>' failed to satisfy constraint: Member must
//! satisfy enum value set: [<the model's values>]`. That is the wording the recorded
//! answer to a `ListInsightsData` whose `DataType` is outside its enum carries; the
//! message is built from the request and the model's own constraint, so the RULE is the
//! requirement rather than that one sentence, and the rows below hold it over five enum
//! members on three operations — a top-level member and a member nested inside a list
//! element, required and optional, on operations that are implemented and on one that is
//! still a scaffold stub (the refusal is the service's, not a handler's).
//!
//! Each expected value set is read from the GENERATED value list of the model's enum
//! (`ops::types`), not written out here, so a value an enum gains cannot leave this
//! suite asserting a stale set.
//!
//! Two control cases pin the other half of the rule: a class this service has no
//! recorded value-free wording for (`@length`) keeps the framework's own sentence, value
//! and all, and an out-of-set enum inside a trail's event or Insights selectors is
//! answered with the modelled `InvalidEventSelectorsException` /
//! `InvalidInsightSelectorsException` instead — the refusals `src/service.rs` and those
//! handlers answer ahead of this envelope, whose own rows live in
//! `tests/all/refusal_fidelity.rs` and `tests/all/insight_selectors.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_CATEGORY_VALUES, INSIGHTS_METRIC_DATA_TYPE_VALUES, INSIGHT_TYPE_VALUES,
    LIST_INSIGHTS_DATA_TYPE_VALUES, LOOKUP_ATTRIBUTE_KEY_VALUES, READ_WRITE_TYPE_VALUES,
};

use super::emulator::Emulator;

/// The code and status AWS answers an input-constraint violation with on the JSON
/// protocols.
const VALIDATION_EXCEPTION: &str = "ValidationException";
/// See [`VALIDATION_EXCEPTION`].
const VALIDATION_STATUS: u16 = 400;

/// The envelope an out-of-set `member` states its clause in, up to the value set: the
/// framework renders one clause per violation, and this is the whole sentence bar the
/// list of values (which [`assert_states_the_models_value_set`] checks as a SET, because
/// the clause lists the values in the order the MODEL declares them, which is neither
/// the generated list's order nor alphabetical).
fn clause_prefix(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: ["
    )
}

/// Assert `message` is the value-free clause for `member` and lists exactly the values
/// the model's enum declares, whatever order it lists them in.
fn assert_states_the_models_value_set(case: &str, message: &str, member: &str, allowed: &[&str]) {
    let prefix = clause_prefix(member);
    let listed = message
        .strip_prefix(&prefix)
        .and_then(|tail| tail.strip_suffix(']'));
    let Some(listed) = listed else {
        panic!(
            "case {case}: expected the value-free clause {prefix:?}…] for {member}, got \
             {message:?}"
        );
    };
    let mut listed: Vec<&str> = listed.split(", ").collect();
    listed.sort_unstable();
    let mut expected: Vec<&str> = allowed.to_vec();
    expected.sort_unstable();
    assert_eq!(
        listed, expected,
        "case {case}: expected the model's own value set for {member}, got {message:?}"
    );
}

/// A value outside a modelled enum value set is refused naming the member and the
/// model's value set, without the value.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_without_quoting_it() {
    let emulator = Emulator::new();

    // (case, operation, request, the member at fault, the value set the model declares
    // for it). Exactly one member of each request is outside its enum; every other
    // member satisfies its constraints, so the envelope carries one clause.
    let rows: Vec<(&str, &str, Value, &str, &[&str])> = vec![
        (
            // The recorded case: `ListInsightsData`'s own `DataType`.
            "a DataType outside ListInsightsDataType",
            "ListInsightsData",
            json!({
                "InsightSource": "arn:aws:cloudtrail:us-east-1:000000000000:trail/enum-wording",
                "DataType": "NotAValidType",
            }),
            "dataType",
            LIST_INSIGHTS_DATA_TYPE_VALUES,
        ),
        (
            // The sibling operation's `DataType`, a different enum on a different
            // operation: the rule is the service's, not this one member's.
            "a DataType outside InsightsMetricDataType",
            "ListInsightsMetricData",
            json!({
                "EventSource": "s3.amazonaws.com",
                "EventName": "PutObject",
                "InsightType": INSIGHT_TYPE_VALUES[0],
                "DataType": "FillWithOnes",
            }),
            "dataType",
            INSIGHTS_METRIC_DATA_TYPE_VALUES,
        ),
        (
            // A REQUIRED enum member, where the one above is optional.
            "an InsightType outside its enum",
            "ListInsightsMetricData",
            json!({
                "EventSource": "s3.amazonaws.com",
                "EventName": "PutObject",
                "InsightType": "ApiLatencyInsight",
            }),
            "insightType",
            INSIGHT_TYPE_VALUES,
        ),
        (
            // An operation that is still a scaffold stub: the boundary refuses the
            // request before any handler runs, so the wording does not depend on one.
            "an EventCategory outside its enum",
            "LookupEvents",
            json!({"EventCategory": "management"}),
            "eventCategory",
            EVENT_CATEGORY_VALUES,
        ),
        (
            // A member nested inside a LIST element, which the validating walk spells
            // with a 1-based `<index>.member` step.
            "an AttributeKey outside its enum, on the second lookup attribute",
            "LookupEvents",
            json!({"LookupAttributes": [
                {"AttributeKey": LOOKUP_ATTRIBUTE_KEY_VALUES[0], "AttributeValue": "AKIA"},
                {"AttributeKey": "EventNames", "AttributeValue": "PutObject"},
            ]}),
            "lookupAttributes.2.member.attributeKey",
            LOOKUP_ATTRIBUTE_KEY_VALUES,
        ),
    ];

    for (case, operation, request, member, allowed) in rows {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(case, VALIDATION_STATUS, VALIDATION_EXCEPTION);
        assert_states_the_models_value_set(case, reply.message(), member, allowed);
    }

    // Every value the model's enum declares is accepted by the boundary, so the rows
    // above are about the values outside it. `LookupEvents` is a scaffold stub, so an
    // accepted request reaches its 501 rather than a refusal — which is exactly the
    // signal that the boundary let it through.
    for value in EVENT_CATEGORY_VALUES {
        let reply = emulator
            .call("LookupEvents", &json!({"EventCategory": value}))
            .await;
        assert_ne!(
            reply.status, VALIDATION_STATUS,
            "case the {value} event category: a value the model declares must not be \
             refused by the boundary, got {:?}",
            reply.body
        );
    }
}

/// A class this service has no recorded value-free wording for keeps the framework's own
/// sentence, which quotes the value — so the rewording above is the recorded classes and
/// nothing wider.
#[tokio::test]
async fn a_length_violation_beside_an_enum_member_keeps_the_frameworks_own_sentence() {
    let emulator = Emulator::new();
    // `InsightSource` is `ResourceArn` @length 3..256 (model), on the same operation
    // whose `DataType` enum is reworded above.
    let reply = emulator
        .call(
            "ListInsightsData",
            &json!({"InsightSource": "ab", "DataType": LIST_INSIGHTS_DATA_TYPE_VALUES[0]}),
        )
        .await;
    reply.expect_error(
        "an InsightSource below the modelled minimum length",
        VALIDATION_STATUS,
        VALIDATION_EXCEPTION,
    );
    assert_eq!(
        reply.message(),
        "1 validation error detected: Value 'ab' at 'insightSource' failed to satisfy \
         constraint: Member must have length greater than or equal to 3",
        "case an InsightSource below the modelled minimum length: expected the \
         framework's own sentence, got {:?}",
        reply.body
    );
}

/// An out-of-set enum inside a trail's SELECTORS is the error the model declares for
/// that case — `InvalidEventSelectorsException` for an event selector,
/// `InvalidInsightSelectorsException` for an Insights selector — and not the
/// `ValidationException` envelope above: those refusals are answered ahead of it, and
/// widening the value-free envelope to the enum class does not take them over.
#[tokio::test]
async fn an_enum_violation_inside_a_selector_keeps_its_modelled_error() {
    let emulator = Emulator::new();
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "enum-selector", "S3BucketName": "bucket-enum-selector"}),
        )
        .await
        .expect_ok("CreateTrail enum-selector");

    for value in ["read-only", "all", ""] {
        let case = format!("a ReadWriteType of {value:?}");
        let reply = emulator
            .call(
                "PutEventSelectors",
                &json!({
                    "TrailName": "enum-selector",
                    "EventSelectors": [
                        {"ReadWriteType": READ_WRITE_TYPE_VALUES[0]},
                        {"ReadWriteType": value},
                    ],
                }),
            )
            .await;
        reply.expect_error(&case, 400, "InvalidEventSelectorsException");
        assert!(
            !reply.message().is_empty(),
            "case {case}: a refusal must say what was wrong with the request, got {reply:?}"
        );
    }

    for value in ["ApiCallRate", "apicallrateinsight", ""] {
        let case = format!("an InsightType of {value:?}");
        let reply = emulator
            .call(
                "PutInsightSelectors",
                &json!({
                    "TrailName": "enum-selector",
                    "InsightSelectors": [{"InsightType": value}],
                }),
            )
            .await;
        reply.expect_error(&case, 400, "InvalidInsightSelectorsException");
        assert!(
            !reply.message().is_empty(),
            "case {case}: a refusal must say what was wrong with the request, got {reply:?}"
        );
    }
}

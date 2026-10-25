// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's tags addressed BY ARN, end to end over the wire: `TagResource` ->
//! `ListTagsForStream`.
//!
//! RULE: `TagResource` ASSIGNS every key-value pair its request carries to the stored
//! resource its `ResourceARN` names — a key the resource already carries takes the new
//! value, every tag the request does not name stays as it was — and a later read reports
//! that STORED set. It is the SAME stored tag set `AddTagsToStream` writes and
//! `ListTagsForStream` reports, reached through the resource's ARN rather than through a
//! stream name, so the two operations are two ways into one set: whatever either assigns is
//! what a read reports, and re-assigning what is already there changes nothing.
//!
//! The refusals are the ones that rule implies and the operation models: an ARN this
//! account and region hold no resource under is `ResourceNotFoundException` (including an
//! ARN of a resource KIND this emulator holds none of, and the ARN of a stream that lives
//! in another region), a resource that is not `ACTIVE` yet is `ResourceInUseException`, and
//! an assignment that would leave the resource carrying more tags than one resource may
//! carry is `LimitExceededException` — refused before the stored map grows, so it changes
//! nothing.
//!
//! The rows differ along what the rule generalises over: how many pairs an assignment
//! carries (one, several, the whole ceiling), whether its keys are new or already present,
//! whether the tags were put there by this operation or by `AddTagsToStream`, the form of a
//! key and a value, and the order the keys arrive in.
//!
//! The wire harness, the ARN builder and the tag helpers are [`crate::stream_tags`]',
//! reused rather than copied.

use serde_json::json;
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_active_message, stream_not_found_message, tag_ceiling_exceeded_message,
    MAX_STREAM_TAGS, STREAM_CREATING,
};

use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, reported_tags,
    stream_arn, tag_map, Emulator, REGION,
};

/// One row of the assignment table: what the stream was created with, what successive
/// calls assign to it (each through the operation named), and the tag set it must then
/// report.
struct Row {
    label: &'static str,
    /// The `Tags` the create carries, empty on a row whose create names none.
    created_with: Vec<(String, String)>,
    /// One request each, served in order, as `(operation, the pairs it assigns)`.
    assignments: Vec<(&'static str, Vec<(String, String)>)>,
    /// The tags the stream must report, given UNSORTED so the key order a read promises is
    /// what the assertion checks.
    expected: Vec<(String, String)>,
}

/// The table the assignment rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: one pair onto an untagged stream, by ARN.
        Row {
            label: "one-pair-onto-an-untagged-stream",
            created_with: Vec::new(),
            assignments: vec![("TagResource", vec![pair("Environment", "production")])],
            expected: vec![pair("Environment", "production")],
        },
        // Several pairs at once, arriving in an order that is not key order.
        Row {
            label: "three-pairs-at-once-out-of-key-order",
            created_with: Vec::new(),
            assignments: vec![(
                "TagResource",
                vec![
                    pair("zeta", "last-by-key"),
                    pair("alpha", "first-by-key"),
                    pair("mu", "middle-by-key"),
                ],
            )],
            expected: vec![
                pair("alpha", "first-by-key"),
                pair("mu", "middle-by-key"),
                pair("zeta", "last-by-key"),
            ],
        },
        // A key the create already put there takes the new value; the other stays.
        Row {
            label: "overwrites-a-key-the-create-carried",
            created_with: vec![pair("Environment", "staging"), pair("Team", "platform")],
            assignments: vec![("TagResource", vec![pair("Environment", "production")])],
            expected: vec![pair("Environment", "production"), pair("Team", "platform")],
        },
        // ONE stored set, two operations into it: what `AddTagsToStream` wrote is still
        // there, and what this operation overwrites takes effect.
        Row {
            label: "shares-one-stored-set-with-add-tags-to-stream",
            created_with: Vec::new(),
            assignments: vec![
                (
                    "AddTagsToStream",
                    vec![pair("by-add-tags", "kept"), pair("shared", "written-first")],
                ),
                (
                    "TagResource",
                    vec![pair("by-tag-resource", "added"), pair("shared", "overwritten")],
                ),
            ],
            expected: vec![
                pair("by-add-tags", "kept"),
                pair("by-tag-resource", "added"),
                pair("shared", "overwritten"),
            ],
        },
        // Re-assigning exactly what is already there changes nothing.
        Row {
            label: "re-assigning-the-same-pairs-changes-nothing",
            created_with: vec![pair("Environment", "production")],
            assignments: vec![
                ("TagResource", vec![pair("Environment", "production")]),
                ("TagResource", vec![pair("Environment", "production")]),
            ],
            expected: vec![pair("Environment", "production")],
        },
        // The whole documented ceiling in one assignment.
        Row {
            label: "the-whole-tag-ceiling-at-once",
            created_with: Vec::new(),
            assignments: vec![("TagResource", pairs(MAX_STREAM_TAGS))],
            expected: pairs(MAX_STREAM_TAGS),
        },
    ]
}

/// Every row's assignments are served through the operation it names, and the stream then
/// reports the union of what was there and what was assigned, in key order.
#[tokio::test]
async fn tags_assigned_by_arn_are_reported_back_in_key_order() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every stream is created first, so the rows share ONE wait for the transition the
    // creates armed rather than one each.
    for row in &rows {
        let mut body = json!({ "StreamName": row.label, "ShardCount": 1 });
        if !row.created_with.is_empty() {
            body["Tags"] = tag_map(&row.created_with);
        }
        emulator.ok(row.label, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let arn = stream_arn(REGION, row.label);
        for (operation, assignment) in &row.assignments {
            let body = match *operation {
                "TagResource" => json!({ "ResourceARN": arn, "Tags": tag_map(assignment) }),
                _ => json!({ "StreamARN": arn, "Tags": tag_map(assignment) }),
            };
            let answered = emulator.ok(row.label, operation, &body).await;
            assert!(
                answered.as_object().is_some_and(|body| body.is_empty())
                    || answered.is_null(),
                "case {}: expected {operation} to answer the modelled empty body, got \
                 {answered}",
                row.label
            );
        }

        let read = emulator
            .ok(
                row.label,
                "ListTagsForStream",
                &json!({ "StreamARN": arn }),
            )
            .await;
        assert_eq!(
            reported_tags(row.label, &read),
            in_key_order(&row.expected),
            "case {}: expected the stored tag set in key order, got {read}",
            row.label
        );
    }
}

/// An ARN this account and region hold no resource under names nothing to tag, whatever
/// form it takes: another region's stream, a stream nothing created, and a resource KIND
/// this emulator holds none of.
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let name = "tag-resource-refusals";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let rows: Vec<(&str, String)> = vec![
        ("a stream nothing created", stream_arn(REGION, "never-created")),
        (
            "a stream of another region",
            stream_arn("eu-west-1", name),
        ),
        (
            "a resource kind this emulator holds none of",
            format!(
                "arn:aws:kinesis:{REGION}:{}:channel/a-channel",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ];
    for (case, arn) in &rows {
        let (status, response) = emulator
            .call(
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("k", "v")]) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            stream_not_found_message(arn),
            "case {case}: expected the not-found sentence this service states, got {response}"
        );
    }

    // The stream this account does hold is untouched by the refusals.
    let read = emulator
        .ok(
            "after the refusals",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    assert!(
        reported_tags("after the refusals", &read).is_empty(),
        "case after the refusals: a refused assignment stores nothing, got {read}"
    );
}

/// A resource that is not `ACTIVE` yet is not available for a write to its tags, and the
/// write lands once the transition the create armed has fallen due.
#[tokio::test]
async fn an_assignment_to_a_resource_that_is_not_active_yet_is_refused() {
    let emulator = Emulator::new();
    let name = "tag-resource-not-active-yet";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    let arn = stream_arn(REGION, name);

    // A caller acting straight after the create still sees the transient state the create
    // armed.
    let (status, response) = emulator
        .call(
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("k", "v")]) }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case not-active-yet: expected the modelled refusal, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case not-active-yet: expected ResourceInUseException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        stream_not_active_message(&arn, STREAM_CREATING),
        "case not-active-yet: expected the not-active sentence this service states, got \
         {response}"
    );

    // Once the transition is due, the same request is served and the tag is stored.
    tokio::time::sleep(past_activation()).await;
    emulator
        .ok(
            "once active",
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("k", "v")]) }),
        )
        .await;
    let read = emulator
        .ok(
            "once active",
            "ListTagsForStream",
            &json!({ "StreamARN": arn }),
        )
        .await;
    assert_eq!(
        reported_tags("once active", &read),
        vec![pair("k", "v")],
        "case once active: expected the assignment to be stored, got {read}"
    );
}

/// An assignment that would take the resource past the documented tag ceiling is refused
/// before the stored map grows, so the tags it already carries are untouched — while an
/// assignment that only overwrites keys it already carries stays inside the ceiling and is
/// served.
#[tokio::test]
async fn an_assignment_past_the_tag_ceiling_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "tag-resource-ceiling";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);
    emulator
        .ok(
            name,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&stored) }),
        )
        .await;

    let (status, response) = emulator
        .call(
            "TagResource",
            &json!({
                "ResourceARN": arn,
                "Tags": tag_map(&[pair("one-too-many", "refused")]),
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case past the ceiling: expected the modelled refusal, got {status} with body \
         {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case past the ceiling: expected LimitExceededException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        tag_ceiling_exceeded_message(MAX_STREAM_TAGS + 1),
        "case past the ceiling: expected the one sentence this service states the ceiling \
         in, got {response}"
    );

    // Overwriting a key the resource already carries stays inside the ceiling.
    emulator
        .ok(
            "an overwrite at the ceiling",
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&[pair("key-00", "replaced")]) }),
        )
        .await;

    let read = emulator
        .ok(
            "ceiling-read-back",
            "ListTagsForStream",
            &json!({ "StreamARN": arn, "Limit": MAX_STREAM_TAGS }),
        )
        .await;
    let mut expected = in_key_order(&stored);
    expected[0] = pair("key-00", "replaced");
    assert_eq!(
        reported_tags("ceiling-read-back", &read),
        expected,
        "case ceiling-read-back: the refused assignment left the stored tags alone and the \
         overwrite took effect, got {read}"
    );
}

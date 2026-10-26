// SPDX-License-Identifier: CC0-1.0

//! WHEN an input-constraint refusal is answered, relative to the state the operation
//! would otherwise read, over the `awsJson1_1` wire.
//!
//! The rule under test: a request that violates a modelled input constraint is refused
//! with HTTP 400 `ValidationException`, naming the member path and the constraint it
//! failed, BEFORE the operation looks anything up — so the refusal it gets does not
//! depend on the store at all (the named stream missing, a different stream present, the
//! named stream present, the named stream already at a documented ceiling), and none of
//! the refusals an operation states FROM state (`ResourceNotFoundException`,
//! `ResourceInUseException`, `LimitExceededException`) can be answered in its place. A
//! refused request also writes nothing.
//!
//! The rule is a property of the request walk, so it holds for every operation that takes
//! a constrained value beside a resource name: `TagDeliveryStream`, `UntagDeliveryStream`,
//! `ListTagsForDeliveryStream` and `CreateDeliveryStream`. Each table below spans a SET of
//! violated constraints — the look-ahead `TagKey` pattern a handler applies itself, a
//! `@length` bound on a string and on a list, a `@range` bound on an integer, and an
//! absent `@required` member — crossed with a SET of store contents.
//!
//! What a tag key may BE is the `tag_key_pattern` suite's subject, over the whole character
//! class and every member that takes one; the axis here is WHEN the refusal is decided, so
//! the tables below carry one request per violated constraint rather than one per key.
//!
//! The harness (`Emulator`, `s3_configuration`, `error_code`, `error_message`) is the
//! lifecycle suite's, the tag helpers are the listing suite's, the constraint clauses are
//! the `refusal_messages` suite's and the two tag-key refusal sentences are the
//! `tag_key_pattern` suite's, so every request here goes through the same real router and
//! every expectation is worded once.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{error_code, error_message, s3_configuration, Emulator};
use crate::delivery_stream_tags::{
    create_tagged, generated_tags, list_tags, listed, owned, MAX_TAGS,
};
use crate::refusal_messages::{null_at, value_at};
use crate::tag_key_pattern::{element_refusal, pattern_refusal};

/// The stream name every request in this suite addresses, so what varies between the
/// store contents below is only whether that stream is there.
const ADDRESSED: &str = "ordered-stream";

/// A stream that exists under a DIFFERENT name, for the store that holds one.
const OTHER: &str = "other-stream";

/// `TagKey`'s modelled `@length` maximum (`smithy_query(query="shape",shape="TagKey")`).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// `Limit`'s modelled `@range` maximum
/// (`smithy_query(query="shape",shape="ListTagsForDeliveryStreamInputLimit")`: `1..50`).
/// Its own constant: it coincides with the fifty-tag ceiling without being it, so a change
/// to either bound cannot keep a row of the other green.
const LIMIT_MAX: usize = 50;

/// What `ADDRESSED` holds when the store is stocked with it.
fn held_tags() -> Vec<(String, String)> {
    owned(&[("env", "prod"), ("team", "data")])
}

/// What the store holds when an invalid request is answered. The rule is that no row of
/// this changes the answer.
#[derive(Debug, Clone, Copy)]
enum Stock {
    /// Nothing at all: the account's register is empty, so a lookup would resolve no
    /// stream and the operation would have a `ResourceNotFoundException` to state.
    Empty,
    /// A stream exists, but not the one the request names — the same lookup outcome,
    /// reached with a non-empty register.
    OtherStreamOnly,
    /// The stream the request names exists and carries tags, so a lookup would succeed.
    NamedStream,
    /// The named stream exists and already holds the documented ceiling of fifty tags, so
    /// a further distinct key would be refused as `LimitExceededException` from state.
    NamedStreamAtTagCeiling,
}

impl Stock {
    /// A fresh emulator stocked this way.
    async fn emulator(self) -> Emulator {
        let emulator = Emulator::new();
        match self {
            Stock::Empty => {}
            Stock::OtherStreamOnly => create_tagged(&emulator, OTHER, &held_tags()).await,
            Stock::NamedStream => create_tagged(&emulator, ADDRESSED, &held_tags()).await,
            Stock::NamedStreamAtTagCeiling => {
                create_tagged(&emulator, ADDRESSED, &generated_tags(MAX_TAGS)).await
            }
        }
        emulator
    }

    /// Every store content the rule is checked against.
    fn every() -> Vec<Stock> {
        vec![
            Stock::Empty,
            Stock::OtherStreamOnly,
            Stock::NamedStream,
            Stock::NamedStreamAtTagCeiling,
        ]
    }
}

/// One invalid request: the case it stands for, the operation, the body, and the refusal
/// the walk owes it whatever the store holds.
type InvalidRequest = (&'static str, &'static str, Value, String);

/// A request per violated constraint, every one of them addressing [`ADDRESSED`].
///
/// The bounds come from the model
/// (`smithy_query(query="shape",shape="TagKey,TagDeliveryStreamInputTagList,
/// ListTagsForDeliveryStreamInputLimit")`: `TagKey` is `@length(1..128)` and patterned,
/// the tag list is `@length(1..50)`, `Limit` is `@range(1..50)`).
fn invalid_requests() -> Vec<InvalidRequest> {
    vec![
        (
            "a tag key carrying the reserved prefix",
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "Tags": [{ "Key": "aws:owner", "Value": "v" }],
            }),
            pattern_refusal("tags.1.member.key"),
        ),
        (
            "a tag key outside the modelled class, second of two",
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "Tags": [{ "Key": "zone", "Value": "c" }, { "Key": "bad*key", "Value": "v" }],
            }),
            pattern_refusal("tags.2.member.key"),
        ),
        (
            "a tag key one character past its maximum",
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "Tags": [{ "Key": "k".repeat(TAG_KEY_MAX_LENGTH + 1), "Value": "v" }],
            }),
            value_at(
                "tags.1.member.key",
                &format!("Member must have length less than or equal to {TAG_KEY_MAX_LENGTH}"),
            ),
        ),
        (
            "an empty tag list",
            "TagDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED, "Tags": [] }),
            value_at("tags", "Member must have length greater than or equal to 1"),
        ),
        (
            "an absent required tag list",
            "TagDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED }),
            null_at("tags"),
        ),
        (
            "a tag key outside the modelled class, in a list of keys",
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED, "TagKeys": ["env", "aws:owner"] }),
            element_refusal("tagKeys"),
        ),
        (
            "an absent required key list",
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED }),
            null_at("tagKeys"),
        ),
        (
            "a start key outside the modelled class",
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED, "ExclusiveStartTagKey": "bad*key" }),
            pattern_refusal("exclusiveStartTagKey"),
        ),
        (
            "a limit below its modelled minimum",
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED, "Limit": 0 }),
            value_at("limit", "Member must have value greater than or equal to 1"),
        ),
        (
            "a limit above its modelled maximum",
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": ADDRESSED, "Limit": LIMIT_MAX + 1 }),
            value_at(
                "limit",
                &format!("Member must have value less than or equal to {LIMIT_MAX}"),
            ),
        ),
        (
            "a tag key carrying the reserved prefix, on a create",
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": "aws:owner", "Value": "v" }],
            }),
            pattern_refusal("tags.1.member.key"),
        ),
    ]
}

#[tokio::test]
async fn an_input_constraint_refusal_does_not_depend_on_what_the_store_holds() {
    for stock in Stock::every() {
        let emulator = stock.emulator().await;
        for (reason, operation, request, expected) in invalid_requests() {
            let case = format!("{operation} with {reason}, {stock:?}");
            let (status, refused) = emulator.call(operation, request).await;
            assert_eq!(status, 400, "case {case:?}: got {refused:?}");
            // Not `ResourceNotFoundException`, `ResourceInUseException` or
            // `LimitExceededException`: the constraint was decided before the lookup that
            // would have stated one of those.
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: got {refused:?}"
            );
            let message = error_message(&refused);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_refused_for_an_input_constraint_leaves_the_store_as_it_was() {
    for stock in Stock::every() {
        let emulator = stock.emulator().await;
        let before = read_tags(&emulator).await;
        for (reason, operation, request, _) in invalid_requests() {
            let case = format!("{operation} with {reason}, {stock:?}");
            let (status, refused) = emulator.call(operation, request).await;
            assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        }
        let after = read_tags(&emulator).await;
        assert_eq!(
            after, before,
            "{stock:?}: every refused request wrote nothing, expected {before:?}, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_valid_request_still_gets_the_refusal_the_store_owes_it() {
    // The converse of the ordering: a request the constraint walk admits is answered FROM
    // state, so neither resource refusal is swallowed by the check that now runs first.
    let emulator = Stock::NamedStreamAtTagCeiling.emulator().await;

    // A legal key naming a stream that does not exist: the lookup is reached and states
    // the resource fault.
    for (operation, request) in [
        (
            "TagDeliveryStream",
            json!({ "DeliveryStreamName": OTHER, "Tags": [{ "Key": "zone", "Value": "c" }] }),
        ),
        (
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": OTHER, "TagKeys": ["zone"] }),
        ),
        (
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": OTHER, "ExclusiveStartTagKey": "zone" }),
        ),
    ] {
        let (status, refused) = emulator.call(operation, request).await;
        assert_eq!(status, 400, "{operation} on an absent stream: {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "{operation} on an absent stream: got {refused:?}"
        );
    }

    // A legal key that would be the fifty-first tag: the ceiling is reached and states the
    // limit fault.
    let (status, refused) = emulator
        .call(
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "Tags": [{ "Key": "one-too-many", "Value": "x" }],
            }),
        )
        .await;
    assert_eq!(status, 400, "a fifty-first tag: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a fifty-first tag: got {refused:?}"
    );

    // A legal key on a create whose name is already taken: the uniqueness check is reached
    // and states the in-use fault.
    let (status, refused) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": ADDRESSED,
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": "zone", "Value": "c" }],
            }),
        )
        .await;
    assert_eq!(status, 400, "a create of a taken name: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "a create of a taken name: got {refused:?}"
    );
}

/// The tags [`ADDRESSED`] and [`OTHER`] hold now, read back through the modelled reader:
/// `None` for a stream that does not exist, so a stream a refused create might have
/// written shows up as one appearing.
async fn read_tags(emulator: &Emulator) -> Vec<(&'static str, Option<Vec<(String, String)>>)> {
    let mut read = Vec::new();
    for name in [ADDRESSED, OTHER] {
        let (status, response) = list_tags(emulator, name, None, None).await;
        read.push(match status {
            200 => (name, Some(listed(&response))),
            _ => (name, None),
        });
    }
    read
}

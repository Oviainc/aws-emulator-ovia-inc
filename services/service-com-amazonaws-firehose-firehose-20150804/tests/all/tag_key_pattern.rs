// SPDX-License-Identifier: CC0-1.0

//! What a TAG KEY may be, over the `awsJson1_1` wire, on every member that takes one.
//!
//! The rule under test: a tag key is admitted exactly by the `@pattern` the modelled
//! `TagKey` shape carries (`smithy_query(query="shape",shape="TagKey")`:
//! `^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$` — Unicode letters, separators and digits plus
//! `_ . : / = + - @ %`, and not the reserved `aws:` prefix). A key outside it is refused
//! with HTTP 400 `ValidationException` naming the member it was submitted at and the
//! constraint — its own dotted path for a named member, the LIST member and the whole
//! constraint set its elements carry for an element of a list of keys — and the request
//! writes nothing; a key inside it is stored and read back unchanged. The rule
//! is a property of the SHAPE, so it holds for every member that targets it —
//! `TagDeliveryStream`'s `Tags[].Key`, `UntagDeliveryStream`'s `TagKeys[]`,
//! `CreateDeliveryStream`'s `Tags[].Key` and `ListTagsForDeliveryStream`'s
//! `ExclusiveStartTagKey` — and each table below spans a SET of keys rather than one
//! recorded request.
//!
//! The harness (`Emulator`, `s3_configuration`, `error_code`, `error_message`) is the
//! lifecycle suite's and the tag helpers are the listing suite's, so every request here
//! goes through the same real router.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator,
};
use crate::delivery_stream_tags::{create_tagged, list_tags, listed, owned};

/// `TagKey`'s `@pattern` as a constraint clause reports it: exactly as the model spells
/// it, anchors and look-ahead included — an expression carrying a look-around cannot lose
/// its anchoring without admitting values the constraint refuses (see the
/// `refusal_messages` suite for the patterns whose anchoring IS normalised away).
const REPORTED_TAG_KEY_PATTERN: &str = r"^(?!aws:)[\p{L}\p{Z}\p{N}_.:\/=+\-@%]*$";

/// The requirement clause `TagKey`'s pattern states, worded by the suite that owns this
/// service's `@pattern` clause (`crate::refusal_messages::must_match`), so both suites
/// expect one sentence for one constraint.
fn must_match_tag_key_pattern() -> String {
    crate::refusal_messages::must_match(REPORTED_TAG_KEY_PATTERN)
}

/// The message a refusal of the tag key at the NAMED member `member` states: the one
/// constraint the key violated, at the member's own dotted path.
///
/// `pub(crate)` so a sibling suite asserting the same refusal words it the same way
/// rather than carrying a second copy of the sentence.
pub(crate) fn pattern_refusal(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: {}",
        must_match_tag_key_pattern()
    )
}

/// The message a refusal of a tag key submitted as an ELEMENT of `member` states: the
/// list member, and the whole constraint set its elements carry (`TagKey` is
/// `@length(1..128)` as well as patterned), because an element has no member name of its
/// own to report one constraint at.
///
/// `pub(crate)` beside [`pattern_refusal`], for the same reason.
pub(crate) fn element_refusal(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy constraint: [Member must have length less than or equal to \
         128, Member must have length greater than or equal to 1, {}]",
        must_match_tag_key_pattern()
    )
}

/// Keys the modelled pattern does NOT admit.
///
/// One row per reason: a character outside the class the pattern lists (every ASCII
/// punctuation mark it leaves out, so none of them is admitted by accident), and the
/// reserved prefix the look-ahead excludes — alone, and in front of anything.
fn rejected_keys() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        ("the reserved prefix alone", "aws:".to_string()),
        (
            "the reserved prefix in front of a key",
            "aws:cloudformation:stack-name".to_string(),
        ),
        (
            "the reserved prefix in front of an otherwise legal key",
            "aws:owner".to_string(),
        ),
        (
            "the reserved prefix in front of a Unicode key",
            "aws:ключ".to_string(),
        ),
    ];
    // Every printable ASCII punctuation mark the character class does NOT list. The class
    // is `[\p{L}\p{Z}\p{N}_.:\/=+\-@%]`, so these are the marks left over.
    for mark in [
        '!', '"', '#', '$', '&', '\'', '(', ')', '*', ',', ';', '<', '>', '?', '[', '\\',
        ']', '^', '`', '{', '|', '}', '~',
    ] {
        rows.push(("a character outside the modelled class", format!("ke{mark}y")));
    }
    rows
}

/// Keys the modelled pattern DOES admit: one per character the class lists, a Unicode
/// letter, a separator, a digit, and keys that contain the reserved prefix WITHOUT
/// beginning with it (the look-ahead excludes a prefix, not an occurrence).
fn accepted_keys() -> Vec<(&'static str, String)> {
    let mut rows: Vec<(&'static str, String)> = vec![
        ("a plain key", "owner".to_string()),
        ("one character", "k".to_string()),
        ("a Unicode letter key", "ключ".to_string()),
        ("an ideographic key", "键".to_string()),
        ("a separator inside the key", "cost centre".to_string()),
        ("digits only", "2024".to_string()),
        ("the reserved prefix without its colon", "awsome".to_string()),
        ("the reserved prefix not at the start", "my-aws:key".to_string()),
        ("the longest key the length bound admits", "k".repeat(128)),
    ];
    // One row per character the class lists beyond letters, separators and digits.
    for mark in ['_', '.', ':', '/', '=', '+', '-', '@', '%'] {
        rows.push(("a character the modelled class lists", format!("ke{mark}y")));
    }
    rows
}

/// The four requests that carry a tag key, each as `(expected refusal, operation, body)`
/// for the key `key` on a stream called `stream`.
fn requests_carrying(stream: &str, key: &str) -> Vec<(String, &'static str, Value)> {
    vec![
        (
            pattern_refusal("tags.1.member.key"),
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": stream,
                "Tags": [{ "Key": key, "Value": "v" }],
            }),
        ),
        (
            element_refusal("tagKeys"),
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": stream, "TagKeys": [key] }),
        ),
        (
            pattern_refusal("tags.1.member.key"),
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": format!("{stream}-created"),
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": key, "Value": "v" }],
            }),
        ),
        (
            pattern_refusal("exclusiveStartTagKey"),
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": stream, "ExclusiveStartTagKey": key }),
        ),
    ]
}

#[tokio::test]
async fn a_key_the_modelled_pattern_rejects_is_refused_on_every_member_that_takes_one() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "keyed-stream", &owned(&[("env", "prod")])).await;

    for (reason, key) in rejected_keys() {
        for (expected, operation, request) in requests_carrying("keyed-stream", &key) {
            let case = format!("{operation} with {reason} ({key:?})");
            let (status, refused) = emulator.call(operation, request).await;
            assert_eq!(status, 400, "case {case:?}: got {refused:?}");
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
            // The refused key is the caller's own value, and this service states no
            // rejected value in a constraint refusal at all: there is no `Value '…'`
            // subject for one to reach the body through.
            assert!(
                !message.contains("Value '"),
                "case {case:?}: expected no rejected key in the message, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_key_the_modelled_pattern_admits_round_trips_through_every_member() {
    for (index, (reason, key)) in accepted_keys().into_iter().enumerate() {
        let case = format!("{reason} ({key:?})");
        let emulator = Emulator::new();
        let stream = format!("accepted-stream-{index}");
        // Created WITH the key: `CreateDeliveryStream`'s `Tags` takes the same shape.
        create_tagged(&emulator, &stream, &[(key.clone(), "created".to_string())]).await;
        let listed_now = list_tags(&emulator, &stream, None, None).await;
        assert_eq!(
            listed(&listed_now.1),
            vec![(key.clone(), "created".to_string())],
            "case {case:?}: a created tag reads back, got {:?}",
            listed_now.1
        );

        // Replaced through `TagDeliveryStream`.
        let (status, response) = emulator
            .call(
                "TagDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "Tags": [{ "Key": key, "Value": "tagged" }],
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: tag got {response:?}");
        let (_, after_tag) = list_tags(&emulator, &stream, None, None).await;
        assert_eq!(
            listed(&after_tag),
            vec![(key.clone(), "tagged".to_string())],
            "case {case:?}: the tagged value reads back, got {after_tag:?}"
        );

        // Accepted as a paging start key, which reports what follows it.
        let (status, paged) = list_tags(&emulator, &stream, Some(&key), None).await;
        assert_eq!(status, 200, "case {case:?}: start key got {paged:?}");
        assert!(
            listed(&paged).is_empty(),
            "case {case:?}: nothing follows the stream's only key, got {paged:?}"
        );

        // Removed through `UntagDeliveryStream`.
        let (status, response) = emulator
            .call(
                "UntagDeliveryStream",
                json!({ "DeliveryStreamName": stream, "TagKeys": [key] }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: untag got {response:?}");
        let (_, after_untag) = list_tags(&emulator, &stream, None, None).await;
        assert!(
            listed(&after_untag).is_empty(),
            "case {case:?}: the key is gone, got {after_untag:?}"
        );
    }
}

#[tokio::test]
async fn the_length_bound_is_reported_as_the_separate_constraint_it_is() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "bounded-key-stream", &owned(&[("env", "prod")])).await;

    // `TagKey` is `@length(1..128)` as well as patterned
    // (`smithy_query(query="shape",shape="TagKey")`), so a key past that bound is refused
    // under the LENGTH constraint, not the pattern one — and a key AT the bound is
    // accepted (`a_key_the_modelled_pattern_admits_round_trips_through_every_member`).
    let (status, refused) = emulator
        .call(
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": "bounded-key-stream",
                "Tags": [{ "Key": "k".repeat(129), "Value": "v" }],
            }),
        )
        .await;
    assert_eq!(status, 400, "a 129-character key: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a 129-character key: got {refused:?}"
    );
    let expected = "1 validation error detected: Value at 'tags.1.member.key' failed to \
                    satisfy constraint: Member must have length less than or equal to 128";
    let message = error_message(&refused);
    assert_eq!(
        message, expected,
        "a 129-character key: expected {expected:?}, got {message:?}"
    );

    // The same key as an ELEMENT of `TagKeys` has no member name of its own, so it is
    // reported against the list with the element's whole constraint set — whichever of
    // those constraints it violated.
    let (status, refused) = emulator
        .call(
            "UntagDeliveryStream",
            json!({
                "DeliveryStreamName": "bounded-key-stream",
                "TagKeys": ["k".repeat(129)],
            }),
        )
        .await;
    assert_eq!(status, 400, "a 129-character key in TagKeys: {refused:?}");
    let expected = element_refusal("tagKeys");
    let message = error_message(&refused);
    assert_eq!(
        message, expected,
        "a 129-character key in TagKeys: expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_list_of_tag_keys_is_reported_once_however_many_of_its_elements_are_refused() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "listed-key-stream", &owned(&[("env", "prod")])).await;

    // (case, the keys the untag names)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("the only key", vec!["aws:one"]),
        ("the second of three", vec!["env", "aws:two", "three"]),
        ("the last of three", vec!["env", "two", "bad*three"]),
        ("two of three", vec!["aws:one", "env", "bad*three"]),
        ("every key", vec!["aws:one", "bad*two"]),
    ];

    for (case, keys) in cases {
        let (status, refused) = emulator
            .call(
                "UntagDeliveryStream",
                json!({ "DeliveryStreamName": "listed-key-stream", "TagKeys": keys }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        let expected = element_refusal("tagKeys");
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // Nothing was removed by any of them.
    let (_, after) = list_tags(&emulator, "listed-key-stream", None, None).await;
    assert_eq!(
        listed(&after),
        owned(&[("env", "prod")]),
        "a refused untag removes nothing, got {after:?}"
    );
}

#[tokio::test]
async fn a_refused_key_is_reported_at_the_position_it_was_submitted_at() {
    let emulator = Emulator::new();
    create_tagged(&emulator, "positioned-stream", &owned(&[("env", "prod")])).await;

    // (case, the keys the request carries, the paths the refusal names)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("the only key", vec!["aws:one"], vec!["tags.1.member.key"]),
        (
            "the second of three",
            vec!["first", "aws:second", "third"],
            vec!["tags.2.member.key"],
        ),
        (
            "the last of three",
            vec!["first", "second", "bad*third"],
            vec!["tags.3.member.key"],
        ),
        (
            "two of three",
            vec!["aws:first", "second", "bad*third"],
            vec!["tags.1.member.key", "tags.3.member.key"],
        ),
    ];

    for (case, keys, paths) in cases {
        let (status, refused) = emulator
            .call(
                "TagDeliveryStream",
                json!({
                    "DeliveryStreamName": "positioned-stream",
                    "Tags": keys
                        .iter()
                        .map(|key| json!({ "Key": key, "Value": "v" }))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        let message = error_message(&refused);
        // Every violating key is reported, in the order the request carried them — the
        // way the request boundary accumulates violations rather than stopping at one.
        let clauses: Vec<String> = paths
            .iter()
            .map(|path| {
                format!(
                    "Value at '{path}' failed to satisfy constraint: {}",
                    must_match_tag_key_pattern()
                )
            })
            .collect();
        let expected = match clauses.len() {
            1 => format!("1 validation error detected: {}", clauses[0]),
            count => format!(
                "{count} validation errors detected: {}",
                clauses.join("; ")
            ),
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_request_refused_for_its_tag_key_writes_nothing() {
    let emulator = Emulator::new();
    let held = owned(&[("env", "prod"), ("app", "api")]);
    create_tagged(&emulator, "untouched-stream", &held).await;

    // A tag naming one legal and one illegal key adds neither.
    let (status, refused) = emulator
        .call(
            "TagDeliveryStream",
            json!({
                "DeliveryStreamName": "untouched-stream",
                "Tags": [
                    { "Key": "zone", "Value": "c" },
                    { "Key": "aws:owner", "Value": "x" },
                ],
            }),
        )
        .await;
    assert_eq!(status, 400, "a mixed tag request: got {refused:?}");
    let (_, after_tag) = list_tags(&emulator, "untouched-stream", None, None).await;
    assert_eq!(
        listed(&after_tag),
        owned(&[("app", "api"), ("env", "prod")]),
        "a refused tag adds nothing, got {after_tag:?}"
    );

    // An untag naming one carried and one illegal key removes neither.
    let (status, refused) = emulator
        .call(
            "UntagDeliveryStream",
            json!({
                "DeliveryStreamName": "untouched-stream",
                "TagKeys": ["env", "aws:owner"],
            }),
        )
        .await;
    assert_eq!(status, 400, "a mixed untag request: got {refused:?}");
    let (_, after_untag) = list_tags(&emulator, "untouched-stream", None, None).await;
    assert_eq!(
        listed(&after_untag),
        owned(&[("app", "api"), ("env", "prod")]),
        "a refused untag removes nothing, got {after_untag:?}"
    );

    // A create carrying an illegal tag key creates no stream at all.
    let (status, refused) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "never-created-stream",
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": "aws:owner", "Value": "x" }],
            }),
        )
        .await;
    assert_eq!(status, 400, "a create with an illegal tag key: {refused:?}");
    let (status, absent) = list_tags(&emulator, "never-created-stream", None, None).await;
    assert_eq!(status, 400, "the stream was never created: got {absent:?}");
    assert_eq!(
        error_code(&absent),
        "ResourceNotFoundException",
        "a refused create persists no stream, got {absent:?}"
    );
}

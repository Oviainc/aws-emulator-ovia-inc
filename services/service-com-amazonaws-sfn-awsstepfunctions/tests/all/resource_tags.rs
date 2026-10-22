// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Step Functions' own tagging calls (`TagResource`,
//! `ListTagsForResource`), driven over this service's wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through the
//! production `MultiServiceRouter`, so what is asserted is what a signed AWS SDK
//! request observes.
//!
//! The rule under test: a tagging call identifies its resource by `resourceArn` —
//! "the ARN for the Step Functions state machine or activity" — and treats that
//! resource's tags as a MAP keyed by tag key. `TagResource` merges the pairs it is
//! given into the resource's persisted tag set (a key already there takes the new
//! value, a key that is not is added) and answers an empty 200; a later
//! `ListTagsForResource` reports exactly that persisted set, the same on every read.
//! An ARN that is not a taggable Step Functions ARN is `InvalidArn` (400), a
//! taggable ARN the store holds no resource under is `ResourceNotFound` (404), and a
//! merge that would take a resource past the per-resource tag limit is `TooManyTags`
//! (400) with nothing written.
//!
//! The harness (`Emulator`, `create_machine`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::MAX_TAGS;

use crate::state_machine_lifecycle::{
    create_machine, string, Emulator, ACCOUNT, HELLO_DEFINITION, PAST_THE_DELETE_DELAY, REGION,
    ROLE_ARN,
};

/// One tag pair as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value })
}

/// Call `TagResource`, requiring the documented empty 200, for `arn`.
async fn tag_resource(emulator: &Emulator, case: &str, arn: &str, tags: &[Value]) {
    let (status, body) = emulator
        .call("TagResource", &json!({ "resourceArn": arn, "tags": tags }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TagResource to answer 200, got {status} with body {body:?}"
    );
    // "returns an empty 200 body": the output shape has no members, so the body is
    // an empty JSON object (or nothing at all).
    let empty = match &body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: expected an empty TagResource body, got {body:?}"
    );
}

/// The tag set `ListTagsForResource` reports for `arn`, as a key -> value map.
///
/// Reads twice and requires the two reads to be identical: the reported set is a
/// function of what is stored, so a read never changes what a later reader sees.
async fn tags_of(emulator: &Emulator, case: &str, arn: &str) -> BTreeMap<String, String> {
    let request = json!({ "resourceArn": arn });
    let first = emulator.ok("ListTagsForResource", &request).await;
    let second = emulator.ok("ListTagsForResource", &request).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads of {arn} must report the same tags, got {first:?} then {second:?}"
    );
    let listed = first
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a 'tags' array, got {first:?}"))
        .clone();
    listed
        .iter()
        .map(|entry| {
            (
                string(entry, "key"),
                entry
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// `expected` as the map [`tags_of`] returns.
fn expected_map(expected: &[(&str, &str)]) -> BTreeMap<String, String> {
    expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Tag -> ListTagsForResource: the persisted set, not the request
// ---------------------------------------------------------------------------

/// One row of the merge table: the state machine to create, the tags to create it
/// with, the SEQUENCE of `TagResource` calls to make on it, and the tag set a later
/// `ListTagsForResource` must report.
struct MergeCase {
    case: &'static str,
    name: &'static str,
    created_with: Vec<Value>,
    calls: Vec<Vec<Value>>,
    expected: Vec<(&'static str, &'static str)>,
}

/// Every row is one state machine, the tags it was created with, and a SEQUENCE of
/// `TagResource` calls; the tag set a later `ListTagsForResource` reports must be the
/// merge of all of them, keyed by tag key.
///
/// The rows differ along what the rule generalises over: how many pairs one call
/// carries, whether a key is new or already present, how many calls the set is built
/// from, and the shape of the key and the value — including the character classes the
/// `Tag` documentation names ("Unicode letters, digits, white space, or these
/// symbols: `_ . : / = + - @`") and the modelled lengths of `Tag$key` (1..128) and
/// `Tag$value` (0..256).
#[tokio::test]
async fn tag_resource_merges_its_pairs_into_the_persisted_tag_set() {
    let emulator = Emulator::new();

    // `Tag$key` is modelled length 1..128 and `Tag$value` length 0..256
    // (smithy_query shape="Tag"), so these are the longest of each the model admits.
    let longest_key: String = "k".repeat(128);
    let longest_value: String = "v".repeat(256);

    let cases: Vec<MergeCase> = vec![
        MergeCase {
            case: "one tag on an untagged machine",
            name: "tag-one",
            created_with: vec![],
            calls: vec![vec![tag("k", "v")]],
            expected: vec![("k", "v")],
        },
        MergeCase {
            case: "three tags in one call",
            name: "tag-three",
            created_with: vec![],
            calls: vec![vec![
                tag("env", "prod"),
                tag("team", "platform"),
                tag("owner", "ada"),
            ]],
            expected: vec![("env", "prod"), ("owner", "ada"), ("team", "platform")],
        },
        MergeCase {
            case: "a repeated key takes the new value",
            name: "tag-overwrite",
            created_with: vec![],
            calls: vec![vec![tag("env", "dev")], vec![tag("env", "prod")]],
            expected: vec![("env", "prod")],
        },
        MergeCase {
            case: "a repeated key inside one call takes the last value",
            name: "tag-overwrite-in-one-call",
            created_with: vec![],
            calls: vec![vec![tag("env", "dev"), tag("env", "prod")]],
            expected: vec![("env", "prod")],
        },
        MergeCase {
            case: "a second call adds a key and keeps the first",
            name: "tag-accumulate",
            created_with: vec![],
            calls: vec![
                vec![tag("a", "1")],
                vec![tag("b", "2")],
                vec![tag("c", "3")],
            ],
            expected: vec![("a", "1"), ("b", "2"), ("c", "3")],
        },
        MergeCase {
            case: "merged into the tags CreateStateMachine stored",
            name: "tag-created-with-tags",
            created_with: vec![tag("team", "platform")],
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![("env", "prod"), ("team", "platform")],
        },
        MergeCase {
            case: "an existing key is overwritten, a created one is kept",
            name: "tag-overwrite-created",
            created_with: vec![tag("team", "platform"), tag("env", "dev")],
            calls: vec![vec![tag("env", "prod")]],
            expected: vec![("env", "prod"), ("team", "platform")],
        },
        MergeCase {
            case: "an empty tag list changes nothing",
            name: "tag-empty-list",
            created_with: vec![tag("team", "platform")],
            calls: vec![vec![]],
            expected: vec![("team", "platform")],
        },
        MergeCase {
            case: "an empty value, which the model admits (Tag$value length 0..256)",
            name: "tag-empty-value",
            created_with: vec![],
            calls: vec![vec![tag("marker", "")]],
            expected: vec![("marker", "")],
        },
        MergeCase {
            // The symbol set the `Tag` documentation names, plus digits, white space
            // and a non-ASCII letter.
            case: "the documented character classes",
            name: "tag-characters",
            created_with: vec![],
            calls: vec![vec![
                tag("cost_center", "a_b.c:d/e=f+g-h@i"),
                tag("Name 1", "deux éléphants"),
            ]],
            expected: vec![
                ("Name 1", "deux éléphants"),
                ("cost_center", "a_b.c:d/e=f+g-h@i"),
            ],
        },
    ];

    for MergeCase {
        case,
        name,
        created_with,
        calls,
        expected,
    } in cases
    {
        let created = emulator
            .ok(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": HELLO_DEFINITION,
                    "roleArn": ROLE_ARN,
                    "tags": created_with,
                }),
            )
            .await;
        let arn = string(&created, "stateMachineArn");

        for tags in &calls {
            tag_resource(&emulator, case, &arn, tags).await;
        }

        let reported = tags_of(&emulator, case, &arn).await;
        assert_eq!(
            reported,
            expected_map(&expected),
            "case {case:?}: expected the merged tag set {:?}, got {reported:?}",
            expected_map(&expected)
        );
    }

    // The modelled bounds of `Tag$key` and `Tag$value`, written at the bound and
    // read back, then one character past each bound — which the boundary validator
    // refuses before the handler runs, so nothing is stored.
    let arn = create_machine(&emulator, "tag-bounds", HELLO_DEFINITION).await;
    tag_resource(
        &emulator,
        "the longest key and value the model admits",
        &arn,
        &[tag(&longest_key, &longest_value)],
    )
    .await;
    let reported = tags_of(&emulator, "at the modelled bounds", &arn).await;
    assert_eq!(
        reported.get(&longest_key).map(String::as_str),
        Some(longest_value.as_str()),
        "case \"at the modelled bounds\": expected the 128-character key to carry the \
         256-character value, got {reported:?}"
    );

    // (case, the tag one character past a bound)
    let past_the_bound: Vec<(&str, Value)> = vec![
        ("a 129-character key", tag(&"k".repeat(129), "v")),
        ("a 257-character value", tag("k", &"v".repeat(257))),
        ("an empty key", tag("", "v")),
    ];
    for (case, refused) in past_the_bound {
        let (status, code) = emulator
            .err(
                "TagResource",
                &json!({ "resourceArn": arn, "tags": [refused] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 outside the modelled bound, got {status} {code}"
        );
        let after = tags_of(&emulator, case, &arn).await;
        assert_eq!(
            after.len(),
            1,
            "case {case:?}: a refused write must store nothing, got {after:?}"
        );
    }
}

/// A tag with no key names nothing to write: the model does not mark `key` required,
/// so the request reaches the handler and is refused there with a 400 — never with a
/// 501, and never with a silent success.
#[tokio::test]
async fn tag_resource_refuses_a_tag_that_carries_no_key() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "tag-keyless", HELLO_DEFINITION).await;
    tag_resource(&emulator, "a tag to keep", &arn, &[tag("keep", "me")]).await;

    // (case, the tags of one refused request)
    let cases: Vec<(&str, Value)> = vec![
        ("a single keyless tag", json!([{ "value": "v" }])),
        (
            "a keyless tag beside a valid one",
            json!([{ "key": "env", "value": "prod" }, { "value": "v" }]),
        ),
    ];

    for (case, tags) in cases {
        let (status, code) = emulator
            .err("TagResource", &json!({ "resourceArn": arn, "tags": tags }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 for a tag with no key, got {status} {code}"
        );
        let after = tags_of(&emulator, case, &arn).await;
        assert_eq!(
            after,
            expected_map(&[("keep", "me")]),
            "case {case:?}: a refused request must write nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The ARN rule: unreadable ARN vs. a resource that is not there
// ---------------------------------------------------------------------------

/// An ARN that is not a taggable Step Functions ARN is `InvalidArn` (400); a
/// taggable ARN the store holds no resource under is `ResourceNotFound` (404). Both
/// tagging operations read the same rule, so each row is asserted on both.
#[tokio::test]
async fn both_tagging_operations_read_the_same_arn_rule() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "tag-arn-rule", HELLO_DEFINITION).await;
    tag_resource(&emulator, "a tag to keep", &arn, &[tag("keep", "me")]).await;

    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // (case, resourceArn, expected status, expected error code)
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "not an ARN at all",
            "tag-arn-rule".to_string(),
            400,
            "InvalidArn",
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:a-queue"),
            400,
            "InvalidArn",
        ),
        (
            "an execution ARN, which is not taggable",
            format!("{prefix}:execution:tag-arn-rule:run-1"),
            400,
            "InvalidArn",
        ),
        (
            "a Step Functions resource type that is not taggable",
            format!("{prefix}:mapRun:tag-arn-rule/abc"),
            400,
            "InvalidArn",
        ),
        (
            "a stateMachine ARN with no name",
            format!("{prefix}:stateMachine:"),
            400,
            "InvalidArn",
        ),
        (
            "an ARN with no partition",
            format!("arn::states:{REGION}:{ACCOUNT}:stateMachine:tag-arn-rule"),
            400,
            "InvalidArn",
        ),
        (
            "a state machine that was never created",
            format!("{prefix}:stateMachine:never-created"),
            404,
            "ResourceNotFound",
        ),
        (
            "the same name in another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:tag-arn-rule"),
            404,
            "ResourceNotFound",
        ),
        (
            // Versions and aliases are not implemented, so no qualified resource
            // exists for one to name.
            "a qualified state machine ARN",
            format!("{prefix}:stateMachine:tag-arn-rule:1"),
            404,
            "ResourceNotFound",
        ),
        (
            // `CreateActivity` still answers 501, so no activity can exist: the ARN
            // is readable, the resource is not there.
            "an activity ARN",
            format!("{prefix}:activity:an-activity"),
            404,
            "ResourceNotFound",
        ),
    ];

    for (case, resource_arn, expected_status, expected_code) in cases {
        for operation in ["TagResource", "ListTagsForResource"] {
            let mut request = json!({ "resourceArn": resource_arn });
            if operation == "TagResource" {
                request["tags"] = json!([tag("k", "v")]);
            }
            let (status, code) = emulator.err(operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (expected_status, expected_code),
                "case {case:?} on {operation}: expected {expected_status} {expected_code}, got \
                 {status} {code}"
            );
        }
    }

    let after = tags_of(&emulator, "after every refusal", &arn).await;
    assert_eq!(
        after,
        expected_map(&[("keep", "me")]),
        "no refused call may touch the state machine that does exist, got {after:?}"
    );
}

/// A state machine the clock has already removed is gone for both tagging
/// operations: a started deletion settles, and the ARN then names nothing.
#[tokio::test]
async fn tagging_a_removed_state_machine_is_resource_not_found() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "tag-then-delete", HELLO_DEFINITION).await;
    tag_resource(&emulator, "before the delete", &arn, &[tag("env", "prod")]).await;

    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    // `DeleteStateMachine` is asynchronous: the record is removed once the clock
    // makes the removal due, so the wait is what makes the resource gone.
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    for operation in ["TagResource", "ListTagsForResource"] {
        let mut request = json!({ "resourceArn": arn });
        if operation == "TagResource" {
            request["tags"] = json!([tag("k", "v")]);
        }
        let (status, code) = emulator.err(operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFound"),
            "case {operation:?} after the deletion settled: expected 404 ResourceNotFound, got \
             {status} {code}"
        );
    }
}

// ---------------------------------------------------------------------------
// The per-resource tag limit
// ---------------------------------------------------------------------------

/// A merge that would leave a resource holding more than the per-resource tag limit
/// is `TooManyTags` (400) and writes nothing — whether the request is one oversized
/// call or one tag too many on a resource already at the limit — while a request
/// that only overwrites keys already there is accepted at the limit, because it does
/// not grow the set.
#[tokio::test]
async fn tag_resource_refuses_a_merge_past_the_per_resource_tag_limit() {
    let emulator = Emulator::new();

    // One oversized call: MAX_TAGS + 1 distinct keys in a single request, and a call
    // an order of magnitude past the limit — the refusal is on the incoming count,
    // so neither does per-tag work and neither stores anything.
    let arn = create_machine(&emulator, "tag-limit-one-call", HELLO_DEFINITION).await;
    for (case, count) in [
        ("one past the limit", MAX_TAGS + 1),
        ("ten times the limit", MAX_TAGS * 10),
    ] {
        let too_many: Vec<Value> = (0..count)
            .map(|index| tag(&format!("k{index}"), "v"))
            .collect();
        let (status, code) = emulator
            .err(
                "TagResource",
                &json!({ "resourceArn": arn, "tags": too_many }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTags"),
            "case {case:?}: expected 400 TooManyTags for {count} tags in one call, got {status} \
             {code}"
        );
        let after = tags_of(&emulator, case, &arn).await;
        assert!(
            after.is_empty(),
            "case {case:?}: a refused write must store nothing, got {after:?}"
        );
    }

    // Filled to the limit over several calls, then one key too many.
    let arn = create_machine(&emulator, "tag-limit-accumulated", HELLO_DEFINITION).await;
    let at_the_limit: Vec<Value> = (0..MAX_TAGS)
        .map(|index| tag(&format!("k{index}"), "v"))
        .collect();
    tag_resource(&emulator, "up to the limit", &arn, &at_the_limit).await;
    let filled = tags_of(&emulator, "up to the limit", &arn).await;
    assert_eq!(
        filled.len(),
        MAX_TAGS,
        "case \"up to the limit\": expected {MAX_TAGS} tags stored, got {filled:?}"
    );

    let (status, code) = emulator
        .err(
            "TagResource",
            &json!({ "resourceArn": arn, "tags": [tag("one-too-many", "v")] }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "TooManyTags"),
        "case \"one key too many\": expected 400 TooManyTags past {MAX_TAGS} tags, got {status} \
         {code}"
    );
    let after = tags_of(&emulator, "one key too many", &arn).await;
    assert_eq!(
        after, filled,
        "case \"one key too many\": a refused write must leave the tag set as it was, got \
         {after:?}"
    );

    // At the limit, overwriting a key already there does not grow the set, so it is
    // accepted and the new value is what a later read reports.
    tag_resource(
        &emulator,
        "an overwrite at the limit",
        &arn,
        &[tag("k0", "replaced")],
    )
    .await;
    let after = tags_of(&emulator, "an overwrite at the limit", &arn).await;
    assert_eq!(
        after.len(),
        MAX_TAGS,
        "case \"an overwrite at the limit\": an overwrite must not grow the set, got {after:?}"
    );
    assert_eq!(
        after.get("k0").map(String::as_str),
        Some("replaced"),
        "case \"an overwrite at the limit\": expected the overwritten value, got {after:?}"
    );
}

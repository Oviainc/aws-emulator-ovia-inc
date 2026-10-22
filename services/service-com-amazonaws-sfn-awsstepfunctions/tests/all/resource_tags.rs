// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Step Functions' own tagging calls (`TagResource`,
//! `UntagResource`, `ListTagsForResource`), driven over this service's wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`) through
//! the production `MultiServiceRouter`, so what is asserted is what a signed AWS SDK
//! request observes.
//!
//! The rule under test: a tagging call identifies its resource by `resourceArn` —
//! "the ARN for the Step Functions state machine or activity" — and treats that
//! resource's tags as a MAP keyed by tag key. `TagResource` merges the pairs it is
//! given into the resource's persisted tag set (a key already there takes the new
//! value, a key that is not is added) and answers an empty 200, `UntagResource`
//! removes the keys `tagKeys` names, and a later `ListTagsForResource` reports
//! exactly the persisted set, the same on every read.
//! An ARN that is not a taggable Step Functions ARN of this endpoint's region is
//! `InvalidArn` (400) for all three calls. Past that, the READ reports the tag set
//! recorded under the ARN and never asserts the resource is there — an ARN nothing
//! is tagged under answers 200 with an empty `TagList` — while the WRITES need the
//! resource, so they are `ResourceNotFound` (404) when the store holds none. A merge
//! that would take a resource past the per-resource tag limit is `TooManyTags`
//! (400), carrying the service's fixed text, with nothing written.
//!
//! The harness (`Emulator`, `create_machine`, `error_code`, `string`) is the one
//! `state_machine_lifecycle` already builds; `tests/all.rs` is this binary's crate
//! root, so it is reached through `crate::state_machine_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sfn_awsstepfunctions::state::{MAX_TAGS, TOO_MANY_TAGS_MESSAGE};

use crate::state_machine_lifecycle::{
    create_machine, error_code, string, Emulator, ACCOUNT, HELLO_DEFINITION,
    PAST_THE_DELETE_DELAY, REGION, ROLE_ARN,
};

/// One tag pair as the wire carries it.
fn tag(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": value })
}

/// Call one operation, requiring a refusal, and return `(status, errorCode, message)`.
async fn refusal(emulator: &Emulator, operation: &str, request: &Value) -> (u16, String, String) {
    let (status, body) = emulator.call(operation, request).await;
    assert_ne!(
        status, 200,
        "case {operation}: expected a refusal, got 200 with body {body:?}"
    );
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string();
    (status, error_code(&body), message)
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

/// One row of the ARN table: one `resourceArn`, what the READ must answer for it and
/// what the WRITE must answer for it.
struct ArnCase {
    case: &'static str,
    arn: String,
    /// `None` when `ListTagsForResource` must answer 200 with an EMPTY tag list;
    /// `Some((status, errorCode))` when the read is refused.
    read: Option<(u16, &'static str)>,
    /// The `(status, errorCode)` `TagResource` must answer.
    write: (u16, &'static str),
}

/// The ARN rule, over a table that spans the classes of `resourceArn`.
///
/// An ARN that is not a taggable Step Functions ARN of this endpoint's region is
/// `InvalidArn` (400) for BOTH calls — it names nothing either could act on. Past
/// that validation the two calls differ, because one reports and the other writes: a
/// well-formed, in-region taggable ARN the store holds nothing under reads as 200
/// with an empty `TagList` (the tag set of an ARN nothing is tagged under is empty),
/// while tagging it is `ResourceNotFound` (404), since a write needs the resource.
#[tokio::test]
async fn both_tagging_operations_read_the_same_arn_rule() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "tag-arn-rule", HELLO_DEFINITION).await;
    tag_resource(&emulator, "a tag to keep", &arn, &[tag("keep", "me")]).await;

    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    let cases: Vec<ArnCase> = vec![
        ArnCase {
            case: "not an ARN at all",
            arn: "tag-arn-rule".to_string(),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "another service's ARN",
            arn: format!("arn:aws:sqs:{REGION}:{ACCOUNT}:a-queue"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "an execution ARN, which is not taggable",
            arn: format!("{prefix}:execution:tag-arn-rule:run-1"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "a Step Functions resource type that is not taggable",
            arn: format!("{prefix}:mapRun:tag-arn-rule/abc"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "a stateMachine ARN with no name",
            arn: format!("{prefix}:stateMachine:"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "an ARN with no partition",
            arn: format!("arn::states:{REGION}:{ACCOUNT}:stateMachine:tag-arn-rule"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            // An endpoint serves one region, so another region's ARN is an ARN
            // neither call can use — not a resource that is absent here.
            case: "the same name in another region",
            arn: format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:tag-arn-rule"),
            read: Some((400, "InvalidArn")),
            write: (400, "InvalidArn"),
        },
        ArnCase {
            case: "a state machine that exists but carries no tags",
            arn: create_machine(&emulator, "tag-arn-rule-untagged", HELLO_DEFINITION).await,
            read: None,
            write: (200, "<none>"),
        },
        ArnCase {
            case: "a state machine that was never created",
            arn: format!("{prefix}:stateMachine:never-created"),
            read: None,
            write: (404, "ResourceNotFound"),
        },
        ArnCase {
            // A second never-created name: the answer follows the rule, not the name.
            case: "another state machine that was never created",
            arn: format!("{prefix}:stateMachine:also-never-created-0123456789"),
            read: None,
            write: (404, "ResourceNotFound"),
        },
        ArnCase {
            // Versions and aliases are not implemented, so no qualified resource
            // exists for one to name.
            case: "a qualified state machine ARN",
            arn: format!("{prefix}:stateMachine:tag-arn-rule:1"),
            read: None,
            write: (404, "ResourceNotFound"),
        },
        ArnCase {
            // `CreateActivity` still answers 501, so no activity can exist: nothing
            // is tagged under the ARN, and nothing is there to tag either.
            case: "an activity ARN",
            arn: format!("{prefix}:activity:an-activity"),
            read: None,
            write: (404, "ResourceNotFound"),
        },
    ];

    for ArnCase {
        case,
        arn: resource_arn,
        read,
        write,
    } in &cases
    {
        let read_request = json!({ "resourceArn": resource_arn });
        match read {
            Some((expected_status, expected_code)) => {
                let (status, code, message) =
                    refusal(&emulator, "ListTagsForResource", &read_request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (*expected_status, *expected_code),
                    "case {case:?} on ListTagsForResource: expected {expected_status} \
                     {expected_code}, got {status} {code} ({message})"
                );
            }
            None => {
                let listed = emulator.ok("ListTagsForResource", &read_request).await;
                let tags = listed.get("tags").and_then(Value::as_array);
                assert_eq!(
                    tags.map(Vec::len),
                    Some(0),
                    "case {case:?} on ListTagsForResource: expected 200 with an empty tags \
                     array, got {listed:?}"
                );
            }
        }

        let write_request = json!({ "resourceArn": resource_arn, "tags": [tag("k", "v")] });
        let untag_request = json!({ "resourceArn": resource_arn, "tagKeys": ["k"] });
        let (expected_status, expected_code) = *write;
        if expected_status == 200 {
            tag_resource(&emulator, case, resource_arn, &[tag("k", "v")]).await;
            // The two writes read one rule, so the resource that takes a tag takes
            // the removal of one too.
            emulator.ok("UntagResource", &untag_request).await;
        } else {
            // Every WRITE reads the same rule: `TagResource` and `UntagResource`
            // answer the same status and code for the same ARN.
            for (operation, request) in [
                ("TagResource", &write_request),
                ("UntagResource", &untag_request),
            ] {
                let (status, code, message) = refusal(&emulator, operation, request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (expected_status, expected_code),
                    "case {case:?} on {operation}: expected {expected_status} {expected_code}, \
                     got {status} {code} ({message})"
                );
            }
        }
    }

    let after = tags_of(&emulator, "after every refusal", &arn).await;
    assert_eq!(
        after,
        expected_map(&[("keep", "me")]),
        "no refused call may touch the state machine that does exist, got {after:?}"
    );
}

/// The `InvalidArn` message rule: the CONDITION the ARN failed, in the service's
/// `Invalid Arn: '<condition>'` envelope, built from the request.
///
/// Two conditions are the service's own recorded text — a value whose prefix is not
/// `arn:` reports `Invalid ARN prefix: <value>`, and an ARN of another region
/// reports `Expected the ARN <arn> to be within region (<region>).` — and the rest
/// name the first ARN component that is wrong, in the same shape. The rows differ
/// along the component that fails and the form of the value, and every row is
/// asserted on all three tagging calls, which read one rule.
#[tokio::test]
async fn an_unusable_arn_reports_the_condition_it_failed() {
    let emulator = Emulator::new();
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // (case, resourceArn, the message the rule produces)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "no 'arn:' prefix at all",
            "not-a-valid-arn".to_string(),
            "Invalid Arn: 'Invalid ARN prefix: not-a-valid-arn'".to_string(),
        ),
        (
            "a colon-delimited value that is not an ARN",
            "states:us-west-2:stateMachine:x".to_string(),
            "Invalid Arn: 'Invalid ARN prefix: states:us-west-2:stateMachine:x'".to_string(),
        ),
        (
            // The prefix is case-sensitive.
            "an upper-case prefix",
            format!("ARN:aws:states:{REGION}:{ACCOUNT}:stateMachine:tag-arn-message"),
            format!(
                "Invalid Arn: 'Invalid ARN prefix: ARN:aws:states:{REGION}:{ACCOUNT}\
                 :stateMachine:tag-arn-message'"
            ),
        ),
        (
            "fewer than the six fields an ARN carries",
            format!("arn:aws:states:{REGION}"),
            format!(
                "Invalid Arn: 'Invalid ARN: expected 6 colon-delimited fields in \
                 arn:aws:states:{REGION}'"
            ),
        ),
        (
            "an empty partition",
            format!("arn::states:{REGION}:{ACCOUNT}:stateMachine:tag-arn-message"),
            format!(
                "Invalid Arn: 'Invalid ARN partition: arn::states:{REGION}:{ACCOUNT}\
                 :stateMachine:tag-arn-message'"
            ),
        ),
        (
            "another service's ARN",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:a-queue"),
            "Invalid Arn: 'Invalid ARN service: sqs, expected states'".to_string(),
        ),
        (
            "an execution ARN, which a tagging call does not accept",
            format!("{prefix}:execution:tag-arn-message:run-1"),
            "Invalid Arn: 'Invalid ARN resource type: execution, expected stateMachine or \
             activity'"
                .to_string(),
        ),
        (
            "a map-run ARN",
            format!("{prefix}:mapRun:tag-arn-message/abc"),
            "Invalid Arn: 'Invalid ARN resource type: mapRun, expected stateMachine or activity'"
                .to_string(),
        ),
        (
            "a taggable resource type with no name",
            format!("{prefix}:stateMachine:"),
            format!("Invalid Arn: 'Invalid ARN stateMachine name: {prefix}:stateMachine:'"),
        ),
        (
            "an activity ARN with no name",
            format!("{prefix}:activity:"),
            "Invalid Arn: 'Invalid ARN activity resource: activity:'".to_string(),
        ),
        (
            "a state machine ARN of another region",
            format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:tag-arn-message"),
            format!(
                "Invalid Arn: 'Expected the ARN arn:aws:states:eu-west-1:{ACCOUNT}\
                 :stateMachine:tag-arn-message to be within region ({REGION}).'"
            ),
        ),
        (
            "an activity ARN of another region",
            format!("arn:aws:states:ap-south-1:{ACCOUNT}:activity:an-activity"),
            format!(
                "Invalid Arn: 'Expected the ARN arn:aws:states:ap-south-1:{ACCOUNT}\
                 :activity:an-activity to be within region ({REGION}).'"
            ),
        ),
    ];

    for (case, resource_arn, expected_message) in cases {
        for operation in ["TagResource", "ListTagsForResource", "UntagResource"] {
            let mut request = json!({ "resourceArn": resource_arn });
            if operation == "TagResource" {
                request["tags"] = json!([tag("k", "v")]);
            }
            if operation == "UntagResource" {
                request["tagKeys"] = json!(["k"]);
            }
            let (status, code, message) = refusal(&emulator, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidArn"),
                "case {case:?} on {operation}: expected 400 InvalidArn, got {status} {code} \
                 ({message})"
            );
            assert_eq!(
                message, expected_message,
                "case {case:?} on {operation}: expected {expected_message:?}, got {message:?}"
            );
        }
    }
}

/// A state machine the clock has already removed is gone for the tag WRITE — its
/// ARN names no resource to tag — while the READ still answers 200 with an empty
/// `TagList`, because the tags recorded under that ARN went with the record.
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

    let (status, code, message) = refusal(
        &emulator,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": [tag("k", "v")] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFound"),
        "case \"TagResource after the deletion settled\": expected 404 ResourceNotFound, got \
         {status} {code} ({message})"
    );

    let after = tags_of(&emulator, "after the deletion settled", &arn).await;
    assert!(
        after.is_empty(),
        "case \"ListTagsForResource after the deletion settled\": expected 200 with no tags, got \
         {after:?}"
    );
}

// ---------------------------------------------------------------------------
// UntagResource -> ListTagsForResource: the keys named are the keys gone
// ---------------------------------------------------------------------------

/// One row of the untag table: the tags the state machine is created with, the
/// SEQUENCE of `UntagResource` calls to make on it, and the tag set a later
/// `ListTagsForResource` must report.
struct UntagCase {
    case: &'static str,
    name: &'static str,
    created_with: Vec<Value>,
    calls: Vec<Vec<&'static str>>,
    expected: Vec<(&'static str, &'static str)>,
}

/// `UntagResource` removes exactly the keys `tagKeys` names from the persisted set,
/// and a later `ListTagsForResource` reports what is left — including the EMPTY list
/// when the last key is gone, which is a 200 and not a not-found: the resource is
/// still there.
///
/// The rows differ along what the rule generalises over: how many keys one call
/// names, whether a named key is one the resource carries, how many calls the
/// removal is spread over, repeated keys inside one call, and the empty key list.
#[tokio::test]
async fn untag_resource_removes_the_keys_it_names() {
    let emulator = Emulator::new();

    // `TagKey` is modelled length 1..128 (smithy_query shape="TagKey"), so this is
    // the longest key the model admits.
    let longest_key: String = "k".repeat(128);

    let cases: Vec<UntagCase> = vec![
        UntagCase {
            case: "the only key",
            name: "untag-one",
            created_with: vec![tag("env", "prod")],
            calls: vec![vec!["env"]],
            expected: vec![],
        },
        UntagCase {
            case: "one of three keys",
            name: "untag-one-of-three",
            created_with: vec![
                tag("env", "prod"),
                tag("team", "platform"),
                tag("owner", "ada"),
            ],
            calls: vec![vec!["team"]],
            expected: vec![("env", "prod"), ("owner", "ada")],
        },
        UntagCase {
            case: "two keys in one call",
            name: "untag-two-in-one-call",
            created_with: vec![
                tag("env", "prod"),
                tag("team", "platform"),
                tag("owner", "ada"),
            ],
            calls: vec![vec!["team", "owner"]],
            expected: vec![("env", "prod")],
        },
        UntagCase {
            case: "a key the resource does not carry",
            name: "untag-unknown-key",
            created_with: vec![tag("env", "prod")],
            calls: vec![vec!["absent"]],
            expected: vec![("env", "prod")],
        },
        UntagCase {
            case: "the same key twice in one call",
            name: "untag-repeated-key",
            created_with: vec![tag("env", "prod"), tag("team", "platform")],
            calls: vec![vec!["env", "env"]],
            expected: vec![("team", "platform")],
        },
        UntagCase {
            case: "the same key in two calls",
            name: "untag-repeated-call",
            created_with: vec![tag("env", "prod"), tag("team", "platform")],
            calls: vec![vec!["env"], vec!["env"]],
            expected: vec![("team", "platform")],
        },
        UntagCase {
            case: "every key, one call at a time",
            name: "untag-all-one-by-one",
            created_with: vec![tag("a", "1"), tag("b", "2"), tag("c", "3")],
            calls: vec![vec!["a"], vec!["b"], vec!["c"]],
            expected: vec![],
        },
        UntagCase {
            case: "an empty key list, which removes nothing",
            name: "untag-empty-list",
            created_with: vec![tag("env", "prod")],
            calls: vec![vec![]],
            expected: vec![("env", "prod")],
        },
    ];

    for UntagCase {
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

        for keys in &calls {
            let (status, body) = emulator
                .call(
                    "UntagResource",
                    &json!({ "resourceArn": arn, "tagKeys": keys }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UntagResource to answer 200, got {status} with body \
                 {body:?}"
            );
        }

        let reported = tags_of(&emulator, case, &arn).await;
        assert_eq!(
            reported,
            expected_map(&expected),
            "case {case:?}: expected the remaining tag set {:?}, got {reported:?}",
            expected_map(&expected)
        );
    }

    // The modelled bound of `TagKey`: a 128-character key is written and removed by
    // name, and the resource is still readable afterwards with an empty tag list.
    let arn = create_machine(&emulator, "untag-longest-key", HELLO_DEFINITION).await;
    tag_resource(
        &emulator,
        "the longest key the model admits",
        &arn,
        &[tag(&longest_key, "v")],
    )
    .await;
    emulator
        .ok(
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": [&longest_key] }),
        )
        .await;
    let reported = tags_of(&emulator, "after the longest key was removed", &arn).await;
    assert!(
        reported.is_empty(),
        "case \"the longest key the model admits\": expected an empty tag set, got {reported:?}"
    );

    // Reading tags after the LAST one was removed is a 200 with an empty list, and
    // the state machine is still there: a describe still answers for it.
    let described = emulator
        .ok("DescribeStateMachine", &json!({ "stateMachineArn": arn }))
        .await;
    assert_eq!(
        string(&described, "stateMachineArn"),
        arn,
        "case \"after the last tag was removed\": the state machine must still exist, got \
         {described:?}"
    );
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

/// `TooManyTags` carries the service's FIXED text, whatever the request.
///
/// The message is a constant, not a sentence built from the request, so it is the
/// SAME string for every way of exceeding the limit: one call ten times the limit,
/// one call a single tag past it, one tag too many on a resource already at the
/// limit, and a `CreateStateMachine` that arrives with too many tags. A message that
/// reported the request's count or the limit would differ between these rows.
#[tokio::test]
async fn too_many_tags_carries_the_fixed_service_message() {
    let emulator = Emulator::new();
    let arn = create_machine(&emulator, "tag-limit-message", HELLO_DEFINITION).await;

    /// `count` distinct tag pairs.
    fn tags(count: usize) -> Vec<Value> {
        (0..count)
            .map(|index| tag(&format!("k{index}"), "v"))
            .collect()
    }

    // (case, the refused request) — one row per way past the limit, on each
    // operation whose model declares `TooManyTags`.
    let mut cases: Vec<(&str, &str, Value)> = vec![
        (
            "TagResource, one tag past the limit",
            "TagResource",
            json!({ "resourceArn": arn, "tags": tags(MAX_TAGS + 1) }),
        ),
        (
            "TagResource, ten times the limit",
            "TagResource",
            json!({ "resourceArn": arn, "tags": tags(MAX_TAGS * 10) }),
        ),
        (
            "CreateStateMachine, one tag past the limit",
            "CreateStateMachine",
            json!({
                "name": "tag-limit-at-create",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": tags(MAX_TAGS + 1),
            }),
        ),
        (
            "CreateStateMachine, twice the limit",
            "CreateStateMachine",
            json!({
                "name": "tag-limit-at-create-2",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": tags(MAX_TAGS * 2),
            }),
        ),
    ];

    // One more row whose refusal comes from the MERGE rather than the request's own
    // length: a resource already at the limit, plus one key it does not carry.
    let filled = create_machine(&emulator, "tag-limit-message-filled", HELLO_DEFINITION).await;
    tag_resource(&emulator, "up to the limit", &filled, &tags(MAX_TAGS)).await;
    cases.push((
        "TagResource, one key too many on a resource at the limit",
        "TagResource",
        json!({ "resourceArn": filled, "tags": [tag("one-too-many", "v")] }),
    ));

    for (case, operation, request) in cases {
        let (status, code, message) = refusal(&emulator, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTags"),
            "case {case:?}: expected 400 TooManyTags, got {status} {code} ({message})"
        );
        assert_eq!(
            message, TOO_MANY_TAGS_MESSAGE,
            "case {case:?}: expected the fixed service text {TOO_MANY_TAGS_MESSAGE:?}, got \
             {message:?}"
        );
    }
}

/// The READ reports the tag set recorded under the ARN and nothing else: an ARN
/// nothing is tagged under answers 200 with an empty `TagList`, and an ARN whose tags
/// were all taken away by a refused-then-accepted sequence reports exactly what is
/// stored. Each row varies what left the set empty, so the answer follows the rule
/// rather than any one history.
#[tokio::test]
async fn listing_tags_of_an_untagged_arn_is_an_empty_list() {
    let emulator = Emulator::new();
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    // A machine created WITH tags whose last tag was then removed, a qualifier
    // nothing was written under, an ARN of a resource that never existed, and an ARN
    // whose resource the clock has removed: every way an ARN can carry no tags.
    let untagged = create_machine(&emulator, "list-empty-untagged", HELLO_DEFINITION).await;
    let emptied = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "list-empty-emptied",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": [tag("env", "prod")],
            }),
        )
        .await;
    let emptied = string(&emptied, "stateMachineArn");
    emulator
        .ok(
            "UntagResource",
            &json!({ "resourceArn": &emptied, "tagKeys": ["env"] }),
        )
        .await;

    let tagged = emulator
        .ok(
            "CreateStateMachine",
            &json!({
                "name": "list-empty-qualified",
                "definition": HELLO_DEFINITION,
                "roleArn": ROLE_ARN,
                "tags": [tag("env", "prod")],
            }),
        )
        .await;
    let tagged = string(&tagged, "stateMachineArn");

    let deleted = create_machine(&emulator, "list-empty-deleted", HELLO_DEFINITION).await;
    emulator
        .ok("DeleteStateMachine", &json!({ "stateMachineArn": &deleted }))
        .await;
    std::thread::sleep(PAST_THE_DELETE_DELAY);

    // (case, resourceArn)
    let cases: Vec<(&str, String)> = vec![
        ("a state machine created with no tags", untagged),
        ("a state machine whose last tag was removed", emptied),
        (
            "a version qualifier of a TAGGED state machine",
            format!("{tagged}:1"),
        ),
        (
            "an alias qualifier of a TAGGED state machine",
            format!("{tagged}:live"),
        ),
        (
            "a state machine that was never created",
            format!("{prefix}:stateMachine:never-created"),
        ),
        (
            "a one-character state machine name that was never created",
            format!("{prefix}:stateMachine:z"),
        ),
        (
            "an activity that was never created",
            format!("{prefix}:activity:never-created-activity"),
        ),
        ("a state machine the clock removed", deleted),
    ];

    for (case, resource_arn) in cases {
        let reported = tags_of(&emulator, case, &resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected 200 with an empty tag list, got {reported:?}"
        );
    }

    // The tagged machine itself still reports its own tag set: the empty answers
    // above are what is stored under those ARNs, not a blanket empty list.
    let reported = tags_of(&emulator, "the tagged state machine itself", &tagged).await;
    assert_eq!(
        reported,
        expected_map(&[("env", "prod")]),
        "case \"the tagged state machine itself\": expected the stored tag set, got {reported:?}"
    );
}

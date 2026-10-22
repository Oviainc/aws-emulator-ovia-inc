// SPDX-License-Identifier: CC0-1.0

//! The tagging surface of a rescore execution plan, end to end over the wire:
//! `TagResource`, `UntagResource` and `ListTagsForResource`, every answer computed
//! from the tags this service persisted on the plan the `ResourceARN` resolves to.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded call:
//! the write-then-read rule tagging follows, the one-tag-per-key replacement rule,
//! the ignore-absent-keys rule of untagging, and the refusal rule for an ARN that
//! names no plan.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness every suite in this crate shares (`tests/all/harness.rs`): the
// suites differ in the rules they assert, not in how they reach the service.
use super::harness::{
    call, call_in, create_plan_with_tags, emulator, error_code, sleep_past_the_transition,
};

/// The tags `ListTagsForResource` reports for `arn`, as `(Key, Value)` pairs in the
/// order the service returned them.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, answer) = call(router, "ListTagsForResource", json!({ "ResourceARN": arn })).await;
    assert_eq!(status, 200, "list tags for {arn}: unexpected answer {answer}");
    answer["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("list tags for {arn}: no Tags list in {answer}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A `TagList` request member built from `(Key, Value)` pairs.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// `(Key, Value)` pairs as the owned form [`list_tags`] answers in.
fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn a_tagged_plan_reports_exactly_the_tags_that_were_written() {
    let router = emulator();

    // One row per class of tag content the model admits: the member constraints are
    // `TagKey` 1..128 and `TagValue` 0..256, and a key or value "can consist of
    // Unicode letters, digits, white space, and any of the following symbols:
    // _ . : / = + - @" (`Tag` in the Smithy model), so the rows span count, length,
    // the empty value and non-ASCII text.
    let max_key = "k".repeat(128);
    let max_value = "v".repeat(256);
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no tags at all", vec![]),
        ("a single tag", vec![("team", "search")]),
        (
            "several tags keep the order they were written in",
            vec![("team", "search"), ("env", "prod"), ("cost-center", "42")],
        ),
        ("a tag whose value is the empty string", vec![("owner", "")]),
        (
            "a key and a value at their published maximum lengths",
            vec![(max_key.as_str(), max_value.as_str())],
        ),
        (
            "Unicode letters, white space and the documented symbols",
            vec![("clé.équipe", "recherche de données"), ("a_b:c/d=e+f-g@h", "1")],
        ),
    ];

    for (case, pairs) in cases {
        let (_, arn) = create_plan_with_tags(&router, "tag-target", Value::Null).await;
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list(&pairs) }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            owned(&pairs),
            "case {case:?}: expected {:?}, got {listed:?}",
            owned(&pairs)
        );

        // Listing is a report: a second read answers the same thing.
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again, listed,
            "case {case:?}: a repeated read changed the answer to {again:?}"
        );
    }
}

#[tokio::test]
async fn one_tag_per_key_so_a_repeated_key_replaces_its_value() {
    let router = emulator();

    // "If the tag already exists, the existing value is replaced with the new value."
    // (`TagResource`), and "Keys are not case sensitive and must be unique."
    // (`Tag$Key`) — so a key identifies at most one tag, however it is spelled, and
    // the replacement keeps that key's position in the list.
    struct ReplacementCase {
        case: &'static str,
        initial: Vec<(&'static str, &'static str)>,
        second: Vec<(&'static str, &'static str)>,
        expected: Vec<(&'static str, &'static str)>,
    }

    let cases = vec![
        ReplacementCase {
            case: "the same key written again, in place",
            initial: vec![("team", "search"), ("env", "prod")],
            second: vec![("team", "ranking")],
            expected: vec![("team", "ranking"), ("env", "prod")],
        },
        ReplacementCase {
            case: "the same key in a different case is the same key",
            initial: vec![("team", "search"), ("env", "prod")],
            second: vec![("TEAM", "ranking")],
            expected: vec![("TEAM", "ranking"), ("env", "prod")],
        },
        ReplacementCase {
            case: "a non-ASCII key in a different case is the same key",
            initial: vec![("équipe", "recherche")],
            second: vec![("ÉQUIPE", "classement")],
            expected: vec![("ÉQUIPE", "classement")],
        },
        ReplacementCase {
            case: "one request naming a key twice persists its last value",
            initial: vec![("env", "staging")],
            second: vec![("env", "prod"), ("env", "canary")],
            expected: vec![("env", "canary")],
        },
        ReplacementCase {
            case: "a new key is appended after the keys already held",
            initial: vec![("team", "search")],
            second: vec![("env", "prod")],
            expected: vec![("team", "search"), ("env", "prod")],
        },
        ReplacementCase {
            case: "an empty tag list changes nothing",
            initial: vec![("team", "search")],
            second: vec![],
            expected: vec![("team", "search")],
        },
    ];

    for ReplacementCase {
        case,
        initial,
        second,
        expected,
    } in cases
    {
        // The first write goes in through `CreateRescoreExecutionPlan`, so the rule is
        // asserted over tags from both writers.
        let (_, arn) = create_plan_with_tags(&router, "replacement-target", tag_list(&initial)).await;
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tag_list(&second) }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            owned(&expected),
            "case {case:?}: expected {:?}, got {listed:?}",
            owned(&expected)
        );
    }
}

#[tokio::test]
async fn untagging_removes_the_listed_keys_and_ignores_the_absent_ones() {
    let router = emulator();

    // "If a tag key does not exist on the resource, it is ignored."
    // (`UntagResourceRequest$TagKeys`) — so after the call every listed key is absent
    // and every other tag is untouched, whether or not the key was there to begin
    // with. The rows span how many keys are listed, present versus absent keys, key
    // case and the empty key list.
    struct UntagCase {
        case: &'static str,
        initial: Vec<(&'static str, &'static str)>,
        remove: Vec<&'static str>,
        expected: Vec<(&'static str, &'static str)>,
    }

    let cases = vec![
        UntagCase {
            case: "one of three keys",
            initial: vec![("team", "search"), ("env", "prod"), ("tier", "gold")],
            remove: vec!["env"],
            expected: vec![("team", "search"), ("tier", "gold")],
        },
        UntagCase {
            case: "two keys at once",
            initial: vec![("team", "search"), ("env", "prod"), ("tier", "gold")],
            remove: vec!["team", "tier"],
            expected: vec![("env", "prod")],
        },
        UntagCase {
            case: "a key the resource does not carry is ignored",
            initial: vec![("team", "search")],
            remove: vec!["nothing-like-this"],
            expected: vec![("team", "search")],
        },
        UntagCase {
            case: "a mix of a present and an absent key",
            initial: vec![("team", "search"), ("env", "prod")],
            remove: vec!["env", "absent"],
            expected: vec![("team", "search")],
        },
        UntagCase {
            case: "a key named in a different case is the same key",
            initial: vec![("Team", "search"), ("env", "prod")],
            remove: vec!["tEaM"],
            expected: vec![("env", "prod")],
        },
        UntagCase {
            case: "an empty key list removes nothing",
            initial: vec![("team", "search")],
            remove: vec![],
            expected: vec![("team", "search")],
        },
        UntagCase {
            case: "every key leaves the resource untagged",
            initial: vec![("team", "search"), ("env", "prod")],
            remove: vec!["team", "env"],
            expected: vec![],
        },
    ];

    for UntagCase {
        case,
        initial,
        remove,
        expected,
    } in cases
    {
        let (_, arn) = create_plan_with_tags(&router, "untag-target", tag_list(&initial)).await;

        // Removing a key is idempotent, so the same call twice settles on the same
        // tags: the second round is the idempotence assertion.
        for round in 1..=2 {
            let (status, answer) = call(
                &router,
                "UntagResource",
                json!({ "ResourceARN": arn, "TagKeys": remove }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} (round {round}): unexpected answer {answer}"
            );
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed,
                owned(&expected),
                "case {case:?} (round {round}): expected {:?}, got {listed:?}",
                owned(&expected)
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_names_no_plan_is_refused_with_resource_unavailable() {
    let router = emulator();

    // A plan that exists, and a plan that was deleted: the deleted one's ARN names no
    // resource any more, so it belongs in the refusal table.
    let (live_id, live_arn) = create_plan_with_tags(&router, "live-plan", tag_list(&[("team", "search")])).await;
    let (gone_id, gone_arn) = create_plan_with_tags(&router, "doomed-plan", Value::Null).await;
    // A plan is `CREATING` right after the create and can only be deleted once the
    // clock has made it `ACTIVE`.
    sleep_past_the_transition();
    let (status, answer) = call(
        &router,
        "DeleteRescoreExecutionPlan",
        json!({ "Id": gone_id }),
    )
    .await;
    assert_eq!(status, 200, "delete the doomed plan: got {answer}");

    // One row per class of ARN that resolves to nothing: a well-formed plan ARN for an
    // id this store never minted, a deleted plan's ARN, the same plan's ARN in another
    // region, another service's ARN, and a string that is not an ARN at all.
    let unknown_plan = live_arn.replace(&live_id, "00000000-0000-4000-8000-000000000000");
    let other_region = live_arn.replace("us-east-1", "eu-west-1");
    let other_service = live_arn.replace("kendra-ranking", "kendra");
    let cases: Vec<(&str, &str)> = vec![
        ("an id this store never minted", unknown_plan.as_str()),
        ("a deleted plan's ARN", gone_arn.as_str()),
        ("the plan's ARN in another region", other_region.as_str()),
        ("another service's ARN", other_service.as_str()),
        ("a string that is not an ARN", "not-an-arn"),
    ];

    for (case, arn) in cases {
        for (operation, body) in [
            ("TagResource", json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": "v" }] })),
            ("UntagResource", json!({ "ResourceARN": arn, "TagKeys": ["k"] })),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ] {
            let (status, answer) = call(&router, operation, body).await;
            // Every client-side failure of this RPC service travels with the
            // protocol's client-error status; the error TYPE in the body is what
            // says which failure it was (see `wire_error_status.rs`).
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceUnavailableException",
                "case {case:?} on {operation}: got {answer}"
            );
        }
    }

    // A refused call changed nothing: the live plan still carries exactly its tag.
    let listed = list_tags(&router, &live_arn).await;
    assert_eq!(
        listed,
        owned(&[("team", "search")]),
        "the refused calls must not touch the live plan, got {listed:?}"
    );
}

#[tokio::test]
async fn tags_belong_to_the_resource_the_arn_names() {
    let router = emulator();

    let (_, first) = create_plan_with_tags(&router, "first-plan", tag_list(&[("team", "search")])).await;
    let (_, second) = create_plan_with_tags(&router, "second-plan", Value::Null).await;

    let (status, answer) = call(
        &router,
        "TagResource",
        json!({ "ResourceARN": second, "Tags": tag_list(&[("team", "ranking"), ("env", "prod")]) }),
    )
    .await;
    assert_eq!(status, 200, "tag the second plan: got {answer}");

    let (status, answer) = call(
        &router,
        "UntagResource",
        json!({ "ResourceARN": second, "TagKeys": ["env"] }),
    )
    .await;
    assert_eq!(status, 200, "untag the second plan: got {answer}");

    let first_tags = list_tags(&router, &first).await;
    assert_eq!(
        first_tags,
        owned(&[("team", "search")]),
        "the first plan keeps its own tags, got {first_tags:?}"
    );
    let second_tags = list_tags(&router, &second).await;
    assert_eq!(
        second_tags,
        owned(&[("team", "ranking")]),
        "the second plan keeps its own tags, got {second_tags:?}"
    );

    // A plan's ARN is addressable from a request signed for another region: the ARN,
    // not the caller's credential scope, names the resource.
    let (status, answer) = call_in(
        &router,
        "eu-west-1",
        "ListTagsForResource",
        json!({ "ResourceARN": first }),
    )
    .await;
    assert_eq!(status, 200, "cross-region read of {first}: got {answer}");
}

#[tokio::test]
async fn a_plans_tag_set_is_bounded_and_a_request_past_the_ceiling_writes_nothing() {
    let router = emulator();
    let ceiling =
        service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state::MAX_TAGS_PER_PLAN;

    // `TagResource` grows the set across calls, so the ceiling is per PLAN, not per
    // request: a write that would take a plan past it is refused whole with the
    // modelled `ValidationException` and leaves the stored tags exactly as they were.
    let (_, arn) = create_plan_with_tags(&router, "bounded-plan", Value::Null).await;

    // Fill to the ceiling, in two calls, to show the bound is on the accumulation.
    for half in [0..ceiling / 2, ceiling / 2..ceiling] {
        let tags: Vec<Value> = half
            .map(|n| json!({ "Key": format!("k{n}"), "Value": "v" }))
            .collect();
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tags }),
        )
        .await;
        assert_eq!(status, 200, "filling to the ceiling: got {status} {answer}");
    }
    let at_ceiling = list_tags(&router, &arn).await;
    assert_eq!(
        at_ceiling.len(),
        ceiling,
        "at the ceiling: expected {ceiling} tags, got {}",
        at_ceiling.len()
    );

    // One row per class of write against a plan already at the ceiling: a single new
    // key, a batch of new keys, and a replacement of a key already held (which
    // introduces none, so it stays within the bound).
    let overflow_batch: Vec<Value> = (0..10)
        .map(|n| json!({ "Key": format!("extra{n}"), "Value": "v" }))
        .collect();
    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "one key past the ceiling",
            json!([{ "Key": "one-too-many", "Value": "v" }]),
            400,
        ),
        ("a batch of keys past the ceiling", json!(overflow_batch), 400),
        (
            "a replacement of a key already held introduces none",
            json!([{ "Key": "k0", "Value": "replaced" }]),
            200,
        ),
    ];
    for (case, tags, expected_status) in cases {
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: got {answer}"
            );
            // Refused whole: not one of the request's keys was written.
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed, at_ceiling,
                "case {case:?}: a refused write must change nothing, got {} tags",
                listed.len()
            );
        }
    }
    let after = list_tags(&router, &arn).await;
    assert_eq!(
        after.len(),
        ceiling,
        "after the accepted replacement: expected {ceiling} tags, got {}",
        after.len()
    );
    assert_eq!(
        after.first().map(|(_, value)| value.as_str()),
        Some("replaced"),
        "the replacement kept its key's place and took the new value, got {after:?}"
    );

    // Removing a key makes room again, so the bound is on what is held, not on what
    // was ever written.
    let (status, answer) = call(
        &router,
        "UntagResource",
        json!({ "ResourceARN": arn, "TagKeys": ["k0"] }),
    )
    .await;
    assert_eq!(status, 200, "make room: got {status} {answer}");
    let (status, answer) = call(
        &router,
        "TagResource",
        json!({ "ResourceARN": arn, "Tags": [{ "Key": "room-made", "Value": "v" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a write into the room a removal made: got {status} {answer}"
    );
    let refilled = list_tags(&router, &arn).await;
    assert_eq!(
        refilled.len(),
        ceiling,
        "after refilling: expected {ceiling} tags, got {}",
        refilled.len()
    );
}

#[tokio::test]
async fn a_tag_outside_a_modeled_constraint_is_refused_with_validation_exception() {
    let router = emulator();
    let (_, arn) = create_plan_with_tags(&router, "constrained-plan", Value::Null).await;

    // The published bounds: `TagKey` is 1..128, `TagValue` is 0..256 and both
    // `TagList` and `TagKeyList` are 0..200 (the Smithy model). Each is accepted AT
    // the bound (see the suite above for the maximum-length key and value) and refused
    // one past it.
    let too_long_key = "k".repeat(129);
    let too_long_value = "v".repeat(257);
    let max_list: Vec<Value> = (0..200)
        .map(|n| json!({ "Key": format!("k{n}"), "Value": "v" }))
        .collect();
    let over_list: Vec<Value> = (0..201)
        .map(|n| json!({ "Key": format!("k{n}"), "Value": "v" }))
        .collect();
    let max_keys: Vec<String> = (0..200).map(|n| format!("k{n}")).collect();
    let over_keys: Vec<String> = (0..201).map(|n| format!("k{n}")).collect();

    let cases: Vec<(&str, &str, Value, u16)> = vec![
        (
            "a tag key below its minimum length",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "", "Value": "v" }] }),
            400,
        ),
        (
            "a tag key one past its maximum length",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": too_long_key, "Value": "v" }] }),
            400,
        ),
        (
            "a tag value one past its maximum length",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": too_long_value }] }),
            400,
        ),
        (
            "a tag with no key at all",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Value": "v" }] }),
            400,
        ),
        (
            "an absent required Tags member",
            "TagResource",
            json!({ "ResourceARN": arn }),
            400,
        ),
        (
            "an absent required ResourceARN member",
            "TagResource",
            json!({ "Tags": [{ "Key": "k", "Value": "v" }] }),
            400,
        ),
        (
            "an empty ResourceARN, below its minimum length",
            "ListTagsForResource",
            json!({ "ResourceARN": "" }),
            400,
        ),
        (
            "a tag list at its published maximum",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": max_list }),
            200,
        ),
        (
            "a tag list one past its published maximum",
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": over_list }),
            400,
        ),
        (
            "an untag key below its minimum length",
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": [""] }),
            400,
        ),
        (
            "a tag key list at its published maximum",
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": max_keys }),
            200,
        ),
        (
            "a tag key list one past its published maximum",
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": over_keys }),
            400,
        ),
    ];

    for (case, operation, body, expected_status) in cases {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} on {operation}: expected {expected_status}, got {status} {answer}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?} on {operation}: got {answer}"
            );
        }
    }

    // The two accepted rows above wrote 200 tags and then removed all of them, so the
    // plan ends untagged — the refusals in between persisted nothing.
    let listed = list_tags(&router, &arn).await;
    assert!(
        listed.is_empty(),
        "after the maximum-size write and its removal: expected no tags, got {listed:?}"
    );
}

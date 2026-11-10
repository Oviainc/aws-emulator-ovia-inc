// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` and the form `ListTagsForResource` reports a tag in, end to end over
//! the service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`), through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! Two rules are asserted here.
//!
//! 1. A tag is identified by its KEY, so untagging deletes from the addressed resource
//!    exactly the stored tags whose key the request names, leaves every other one as it
//!    is, and removes nothing for a key the resource does not carry — which is what the
//!    next `ListTagsForResource` then reads back.
//! 2. Every structure the service returns in an awsJson list carries the type
//!    discriminator naming its own shape, so each element of `tags` reports
//!    `ResourceTag`'s shape id beside the stored key and value.
//!
//! The rows vary what rule 1 generalises over: how many of the stored keys are removed
//! (none, one, a middle subset, all), whether the removed keys are present at all, how
//! many tags the resource started with, the form of the removed tag's value (text, empty,
//! absent), and the key's shape (plain, symbol-bearing, non-ASCII, the published length
//! bound). Rule 2 is asserted on every tag of every response those rows produce, so it is
//! exercised over the same spread rather than on one recorded key.

use std::collections::BTreeSet;

use serde_json::json;

use super::domain_tagging::{
    checked_tags, registered_with_tags, Tag, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};
use super::workflow_execution_lifecycle::{error_code, Emulator, ACCOUNT, REGION};

/// Read the tags of `resource_arn` back, asserting rule 2 on every element through the
/// check `domain_tagging` shares, and answer the `(key, value)` pairs the response
/// reported.
async fn tags_of(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
) -> BTreeSet<(String, Option<String>)> {
    let body = emulator
        .ok(
            case,
            "ListTagsForResource",
            &json!({ "resourceArn": resource_arn }),
        )
        .await;
    checked_tags(case, &body)
        .into_iter()
        .map(|tag| (tag.key, tag.value))
        .collect()
}

/// One untagging: the tags the domain is registered with, the keys the call removes, and
/// the keys that must survive it.
struct UntagCase {
    name: &'static str,
    domain: &'static str,
    stored: Vec<Tag>,
    removed: Vec<&'static str>,
    surviving: Vec<&'static str>,
}

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_nothing_else() {
    let emulator = Emulator::new();
    // The key and value forms come from `ResourceTag`'s own model
    // (`smithy_query(query="shape",shape="ResourceTag")`: key `ResourceTagKey` len
    // 1..128, value `ResourceTagValue` len 0..256) and from its doc's character class:
    // "Tags may only contain unicode letters, digits, whitespace, or these symbols:
    // _ . : / = + - @".
    let long_key: String = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value: String = "v".repeat(MAX_TAG_VALUE_LEN);
    let cases = [
        UntagCase {
            name: "every key removed leaves no tags at all",
            domain: "untag-all",
            stored: vec![
                Tag::new("a", Some("1")),
                Tag::new("b", Some("2")),
                Tag::new("c", Some("3")),
            ],
            removed: vec!["a", "b", "c"],
            surviving: vec![],
        },
        UntagCase {
            name: "a middle subset removed leaves the rest",
            domain: "untag-subset",
            stored: vec![
                Tag::new("env", Some("probe")),
                Tag::new("stage", Some("test")),
                Tag::new("team", Some("workflow")),
                Tag::new("owner", Some("probe-suite")),
            ],
            removed: vec!["team", "owner"],
            surviving: vec!["env", "stage"],
        },
        UntagCase {
            name: "no key named removes nothing",
            domain: "untag-none",
            stored: vec![Tag::new("only", Some("kept"))],
            removed: vec![],
            surviving: vec!["only"],
        },
        UntagCase {
            name: "a key the domain does not carry removes nothing",
            domain: "untag-absent",
            stored: vec![Tag::new("present", Some("kept"))],
            removed: vec!["absent"],
            surviving: vec!["present"],
        },
        UntagCase {
            name: "a key named twice is removed once",
            domain: "untag-repeated",
            stored: vec![Tag::new("gone", Some("x")), Tag::new("kept", Some("y"))],
            removed: vec!["gone", "gone"],
            surviving: vec!["kept"],
        },
        UntagCase {
            name: "the value's form does not matter: text, empty and absent",
            domain: "untag-value-forms",
            stored: vec![
                Tag::new("text", Some("value")),
                Tag::new("empty", Some("")),
                Tag::new("absent", None),
            ],
            removed: vec!["empty", "absent"],
            surviving: vec!["text"],
        },
        UntagCase {
            name: "symbol-bearing and non-ASCII keys",
            domain: "untag-key-shapes",
            stored: vec![
                Tag::new("cost.centre/eu-west_1:prod=a+b-c@d", Some("symbols")),
                Tag::new("ключ", Some("non-ascii")),
                Tag::new("kept", Some("z")),
            ],
            removed: vec!["cost.centre/eu-west_1:prod=a+b-c@d", "ключ"],
            surviving: vec!["kept"],
        },
    ];

    for case in &cases {
        let arn = registered_with_tags(&emulator, case.name, case.domain, &case.stored).await;
        let before = tags_of(&emulator, case.name, &arn).await;
        let expected_before: BTreeSet<(String, Option<String>)> = case
            .stored
            .iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect();
        assert_eq!(
            before, expected_before,
            "case {:?}: the registered tags must read back before anything is removed",
            case.name
        );

        let answered = emulator
            .ok(
                case.name,
                "UntagResource",
                &json!({ "resourceArn": arn, "tagKeys": case.removed }),
            )
            .await;
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: UntagResource answers with an empty body, got {answered:?}",
            case.name
        );

        let after = tags_of(&emulator, case.name, &arn).await;
        let expected_after: BTreeSet<(String, Option<String>)> = case
            .stored
            .iter()
            .filter(|tag| case.surviving.contains(&tag.key.as_str()))
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect();
        assert_eq!(
            after.len(),
            case.surviving.len(),
            "case {:?}: exactly the {} surviving key(s) must be reported, got {after:?}",
            case.name,
            case.surviving.len()
        );
        assert_eq!(
            after, expected_after,
            "case {:?}: untagging removes exactly the named keys and leaves the others \
             with the values they had, got {after:?}",
            case.name
        );
    }

    // The published length bounds of a key and a value, removed the same way as any
    // other: one row for the longest key, which is also the row with the longest value.
    let bounded = vec![
        Tag::new(&long_key, Some(&long_value)),
        Tag::new("short", Some("kept")),
    ];
    let arn = registered_with_tags(&emulator, "published bounds", "untag-bounds", &bounded).await;
    emulator
        .ok(
            "published bounds",
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": [long_key] }),
        )
        .await;
    let after = tags_of(&emulator, "published bounds", &arn).await;
    assert_eq!(
        after,
        BTreeSet::from([("short".to_string(), Some("kept".to_string()))]),
        "case \"published bounds\": a key at the modelled maximum length is removed like \
         any other, got {after:?}"
    );
}

#[tokio::test]
async fn untagging_is_scoped_to_the_one_resource_the_arn_names() {
    let emulator = Emulator::new();
    let shared = vec![Tag::new("env", Some("probe")), Tag::new("keep", Some("me"))];
    let first = registered_with_tags(&emulator, "isolation", "untag-first", &shared).await;
    let second = registered_with_tags(&emulator, "isolation", "untag-second", &shared).await;

    emulator
        .ok(
            "isolation",
            "UntagResource",
            &json!({ "resourceArn": first, "tagKeys": ["env", "keep"] }),
        )
        .await;

    assert!(
        tags_of(&emulator, "isolation", &first).await.is_empty(),
        "case \"isolation\": the addressed domain keeps no tag it was asked to drop"
    );
    assert_eq!(
        tags_of(&emulator, "isolation", &second).await,
        BTreeSet::from([
            ("env".to_string(), Some("probe".to_string())),
            ("keep".to_string(), Some("me".to_string())),
        ]),
        "case \"isolation\": another domain's identically keyed tags are untouched"
    );
}

#[tokio::test]
async fn untagging_a_resource_that_is_not_a_stored_domain_is_refused() {
    let emulator = Emulator::new();
    let arn = registered_with_tags(
        &emulator,
        "unknown",
        "untag-known",
        &[Tag::new("env", Some("probe"))],
    )
    .await;

    // Each row is a different way an ARN fails to name THIS service's stored domain: the
    // name, the region, the account, the service and the resource type each scope the
    // lookup, and a string that is not an ARN names nothing at all.
    let cases = [
        (
            "unknown domain name",
            format!("arn:aws:swf:{REGION}:{ACCOUNT}:/domain/never-registered"),
        ),
        (
            "another region",
            format!("arn:aws:swf:eu-west-1:{ACCOUNT}:/domain/untag-known"),
        ),
        (
            "another account",
            format!("arn:aws:swf:{REGION}:999999999999:/domain/untag-known"),
        ),
        (
            "another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:/domain/untag-known"),
        ),
        (
            "another resource type",
            format!("arn:aws:swf:{REGION}:{ACCOUNT}:/activity/untag-known"),
        ),
        ("not an ARN at all", "untag-known".to_string()),
    ];

    for (case, resource_arn) in &cases {
        let (status, body) = emulator
            .call(
                "UntagResource",
                &json!({ "resourceArn": resource_arn, "tagKeys": ["env"] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "UnknownResourceFault"),
            "case {case:?}: an ARN that names no stored domain is the modelled unknown \
             resource, got {status} with {body:?}"
        );
    }

    assert_eq!(
        tags_of(&emulator, "unknown", &arn).await,
        BTreeSet::from([("env".to_string(), Some("probe".to_string()))]),
        "case \"unknown\": a refused untagging removes nothing from any stored domain"
    );
}

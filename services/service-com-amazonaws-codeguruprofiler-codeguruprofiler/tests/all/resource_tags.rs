// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource`, end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rule every case here asserts: `ListTagsForResource` REPORTS the tag map of the
//! one stored resource its `resourceArn` identifies — the whole map, only that
//! resource's map, and whatever the last tag write left in it — and writes nothing, so
//! two reads and a `DescribeProfilingGroup` of the same resource agree. An ARN this
//! store holds no resource for answers the modelled 404 `ResourceNotFoundException`; a
//! `resourceArn` that is not a profiling-group ARN of this service answers the modelled
//! 400 `ValidationException`.
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds, and the
//! writer the round-trip case reads back through is the `UntagResource` request
//! [`crate::untag_resource`] already sends. The tables vary what the rule generalises
//! over: the NUMBER of tags (none, one, a few, fifty), the LENGTH and CHARACTER CLASS
//! of keys and values, WHICH resource an ARN selects, and the form of an ARN that names
//! no resource of this service.

use std::collections::BTreeMap;

use crate::profiling_group_lifecycle::{group_arn, Answer, Harness, ACCOUNT, REGION};
use crate::untag_resource::{encode, untag};

/// `CreateProfilingGroup` with the given tag map (omitting `tags` entirely for an empty
/// map, which is what an SDK sends when the caller passes no tags).
async fn create(harness: &Harness, name: &str, tags: &BTreeMap<String, String>) -> Answer {
    let mut body = serde_json::Map::new();
    body.insert(
        "profilingGroupName".to_string(),
        serde_json::Value::from(name),
    );
    if !tags.is_empty() {
        body.insert(
            "tags".to_string(),
            serde_json::Value::Object(
                tags.iter()
                    .map(|(key, value)| (key.clone(), serde_json::Value::from(value.clone())))
                    .collect(),
            ),
        );
    }
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            "application/json",
            serde_json::to_vec(&serde_json::Value::Object(body)).expect("a serializable body"),
        )
        .await
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}`, the ARN in one path segment.
async fn list_tags(harness: &Harness, resource_arn: &str) -> Answer {
    harness
        .send(
            "GET",
            &format!("/tags/{}", encode(resource_arn)),
            "application/json",
            Vec::new(),
        )
        .await
}

/// The `tags` member of a `ListTagsForResource` answer, as a sorted map so two answers
/// compare independently of JSON member order. An answer carrying no `tags` member at
/// all is distinguishable from one carrying an empty map, so this panics rather than
/// defaulting: the operation must report the stored map either way.
fn listed_tags(answer: &Answer, case: &str) -> BTreeMap<String, String> {
    let body = answer.json();
    let tags = body
        .get("tags")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags member in {body}"));
    tags.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: tag {key} is not a string in {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The `tags` of a `DescribeProfilingGroup` answer (`profilingGroup` is that response's
/// `@httpPayload`, so the description IS the body), for cross-checking the list against
/// the stored record.
fn described_tags(answer: &Answer) -> BTreeMap<String, String> {
    answer
        .json()
        .pointer("/tags")
        .and_then(serde_json::Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// One tag map to store and read back. Rows differ in tag COUNT (0, 1, 2, 3, 50), in key
/// and value LENGTH (one character to 256), and in CHARACTER CLASS (lower, mixed case,
/// dotted, slashed, prefixed, non-ASCII, empty value) — `TagsMap` is a
/// `map<String, String>` with no modelled constraints, so the rule must hold for all of
/// them. The recorded case is the single `env`/`dev` pair of `one-tag`; no row is
/// privileged.
fn tag_cases() -> Vec<(&'static str, BTreeMap<String, String>)> {
    let many: BTreeMap<String, String> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index}")))
        .collect();
    vec![
        ("untagged", BTreeMap::new()),
        (
            "one-tag",
            BTreeMap::from([("env".to_string(), "dev".to_string())]),
        ),
        (
            "three-tags-one-empty-value",
            BTreeMap::from([
                ("team".to_string(), "payments".to_string()),
                ("tier".to_string(), "prod".to_string()),
                ("blank".to_string(), String::new()),
            ]),
        ),
        (
            "punctuation-and-case",
            BTreeMap::from([
                ("aws:created-by".to_string(), "Ops/Team_1".to_string()),
                ("Cost.Center".to_string(), "cc-42".to_string()),
            ]),
        ),
        (
            "non-ascii",
            BTreeMap::from([
                ("propriétaire".to_string(), "équipe-paiements".to_string()),
                ("環境".to_string(), "本番".to_string()),
            ]),
        ),
        (
            "long-key-and-value",
            BTreeMap::from([("k".repeat(128), "v".repeat(256))]),
        ),
        ("fifty-tags", many),
    ]
}

/// The profiling-group name this row's resource gets (within the modelled
/// `ProfilingGroupName` pattern `^[\w-]+$`).
fn group_name(case: &str) -> String {
    format!("listtags-{case}")
}

#[tokio::test]
async fn list_tags_reports_the_tag_map_stored_on_the_resource() {
    let harness = Harness::new();

    for (case, tags) in tag_cases() {
        let name = group_name(case);
        let created = create(&harness, &name, &tags).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: create must be 201, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let listed = list_tags(&harness, &group_arn(&name)).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: ListTagsForResource must be 200, got {} {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let actual = listed_tags(&listed, case);
        assert_eq!(
            actual, tags,
            "case {case:?}: expected the stored tag map {tags:?}, got {actual:?}"
        );

        // A report must not change what a later caller sees: the same call answers the
        // same map, and the stored record still carries it.
        let again = listed_tags(&list_tags(&harness, &group_arn(&name)).await, case);
        assert_eq!(
            again, actual,
            "case {case:?}: a repeated list changed the answer, got {again:?}"
        );
        let described = described_tags(&harness.describe(&name).await);
        assert_eq!(
            described, actual,
            "case {case:?}: the list disagrees with the stored group, got {described:?}"
        );
    }
}

#[tokio::test]
async fn each_arn_reports_only_the_tags_of_the_resource_it_names() {
    let harness = Harness::new();

    let cases = tag_cases();
    for (case, tags) in &cases {
        let name = format!("shared-{case}");
        assert_eq!(
            create(&harness, &name, tags).await.status,
            201,
            "case {case:?}: create must be 201"
        );
    }

    // Every resource exists at once, so an answer that echoed the request or served one
    // canned map would show up here.
    for (case, tags) in &cases {
        let name = format!("shared-{case}");
        let actual = listed_tags(&list_tags(&harness, &group_arn(&name)).await, case);
        assert_eq!(
            &actual, tags,
            "case {case:?}: the ARN of {name} must report its own tags {tags:?}, got {actual:?}"
        );
    }
}

/// One round-trip row: the keys one `UntagResource` request names, and the tags the
/// resource must then report.
struct RoundTripCase {
    label: &'static str,
    remove: &'static [&'static str],
    remaining: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn list_tags_reports_what_untag_resource_left() {
    let harness = Harness::new();

    // Rows differ in how many of the stored keys one `UntagResource` request removes:
    // none of them (an absent key), one of several, and every key.
    let cases = &[
        RoundTripCase {
            label: "absent-key",
            remove: &["nope"],
            remaining: &[("env", "dev"), ("tier", "prod")],
        },
        RoundTripCase {
            label: "one-of-two",
            remove: &["tier"],
            remaining: &[("env", "dev")],
        },
        RoundTripCase {
            label: "every-key",
            remove: &["env", "tier"],
            remaining: &[],
        },
    ];
    let stored = BTreeMap::from([
        ("env".to_string(), "dev".to_string()),
        ("tier".to_string(), "prod".to_string()),
    ]);

    for RoundTripCase {
        label: case,
        remove,
        remaining,
    } in cases
    {
        let name = format!("roundtrip-{case}");
        assert_eq!(
            create(&harness, &name, &stored).await.status,
            201,
            "case {case:?}: create must be 201"
        );
        let arn = group_arn(&name);
        let before = listed_tags(&list_tags(&harness, &arn).await, case);
        assert_eq!(
            before, stored,
            "case {case:?}: the created tags must list before the untag, got {before:?}"
        );

        let untagged = untag(&harness, &arn, remove).await;
        assert_eq!(
            untagged.status,
            204,
            "case {case:?}: UntagResource must be 204, got {} {}",
            untagged.status,
            String::from_utf8_lossy(&untagged.body)
        );

        let expected: BTreeMap<String, String> = remaining
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let listed = list_tags(&harness, &arn).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: ListTagsForResource must be 200 after the untag, got {}",
            listed.status
        );
        let actual = listed_tags(&listed, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the surviving tags {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_this_store_holds_no_resource_for_is_not_found() {
    let harness = Harness::new();

    let tags = BTreeMap::from([("env".to_string(), "staging".to_string())]);
    assert_eq!(
        create(&harness, "present", &tags).await.status,
        201,
        "create must be 201"
    );
    assert_eq!(
        listed_tags(&list_tags(&harness, &group_arn("present")).await, "present"),
        tags,
        "the created resource must report its tags"
    );

    // Rows differ in why no resource answers to the ARN: a name never created, a name
    // whose resource was deleted, and — because an ARN identifies ONE resource — the
    // same name in another region, account or partition.
    let cases: Vec<(&str, String)> = vec![
        ("never-created", group_arn("no-such-group")),
        (
            "another-region",
            format!("arn:aws:codeguru-profiler:eu-west-1:{ACCOUNT}:profilingGroup/present"),
        ),
        (
            "another-account",
            format!("arn:aws:codeguru-profiler:{REGION}:210987654321:profilingGroup/present"),
        ),
        (
            "another-partition",
            format!("arn:aws-cn:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/present"),
        ),
    ];
    for (case, arn) in &cases {
        let missing = list_tags(&harness, arn).await;
        assert_eq!(
            missing.status,
            404,
            "case {case:?} ({arn}): expected 404, got {} {}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        assert_eq!(
            missing.error_code(),
            "ResourceNotFoundException",
            "case {case:?} ({arn}): expected ResourceNotFoundException, got {}",
            String::from_utf8_lossy(&missing.body)
        );
    }

    // Deleted: the ARN was answering a moment ago and must stop.
    assert_eq!(
        harness
            .send(
                "DELETE",
                "/profilingGroups/present",
                "application/json",
                Vec::new()
            )
            .await
            .status,
        204,
        "delete must be 204"
    );
    let deleted = list_tags(&harness, &group_arn("present")).await;
    assert_eq!(
        deleted.status,
        404,
        "a deleted resource must answer 404, got {} {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // Re-created with a different tag map, the same ARN reports the NEW map: the answer
    // tracks the stored record, not the name it was asked about.
    let replacement = BTreeMap::from([("env".to_string(), "prod".to_string())]);
    assert_eq!(
        create(&harness, "present", &replacement).await.status,
        201,
        "re-create must be 201"
    );
    let relisted = listed_tags(
        &list_tags(&harness, &group_arn("present")).await,
        "re-created",
    );
    assert_eq!(
        relisted, replacement,
        "the re-created resource must report its new tags, got {relisted:?}"
    );
}

#[tokio::test]
async fn a_resource_arn_that_is_not_a_profiling_group_arn_is_refused() {
    let harness = Harness::new();

    // The resource the valid form of these ARNs would name exists, so a refusal cannot
    // be mistaken for "the resource is missing".
    assert_eq!(
        create(
            &harness,
            "demo-app",
            &BTreeMap::from([("env".to_string(), "dev".to_string())]),
        )
        .await
        .status,
        201,
        "create must be 201"
    );

    // `ProfilingGroupArn` carries no modelled length or pattern constraint, so the
    // boundary validator accepts any string: each row is a string that is not a
    // profiling-group ARN of this service, and the handler is what must refuse it.
    let cases: Vec<(&str, String)> = vec![
        ("not-an-arn", "demo-app".to_string()),
        ("arn-prefix-only", "arn".to_string()),
        (
            "another-service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/demo-app"),
        ),
        (
            "empty-partition",
            format!("arn::codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/demo-app"),
        ),
        (
            "another-resource-type",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:notificationChannel/demo-app"),
        ),
        (
            "resource-type-without-separator",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroupDemo"),
        ),
        (
            "no-resource-name",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup"),
        ),
        (
            "empty-resource-name",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/"),
        ),
        (
            "nested-resource-name",
            format!("arn:aws:codeguru-profiler:{REGION}:{ACCOUNT}:profilingGroup/demo-app/sub"),
        ),
        (
            "too-few-fields",
            format!("arn:aws:codeguru-profiler:{REGION}"),
        ),
    ];

    for (case, arn) in &cases {
        let refused = list_tags(&harness, arn).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?} ({arn}): expected 400, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?} ({arn}): expected ValidationException, got {}",
            String::from_utf8_lossy(&refused.body)
        );
    }

    // The well-formed ARN of that same resource still answers, so the refusals above
    // are about the ARN and nothing else.
    let listed = list_tags(&harness, &group_arn("demo-app")).await;
    assert_eq!(
        listed.status,
        200,
        "the well-formed ARN must still answer 200, got {} {}",
        listed.status,
        String::from_utf8_lossy(&listed.body)
    );
}

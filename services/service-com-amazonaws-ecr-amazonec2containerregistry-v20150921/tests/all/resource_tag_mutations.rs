// SPDX-License-Identifier: CC0-1.0

//! Tagging and untagging an ECR resource, end to end over the wire.
//!
//! The rule under test: `TagResource` merges the request's key-value pairs into the tag
//! set of the repository its `resourceArn` names — a key already held takes the new
//! value, a key not held is added, a key the request does not name is untouched —
//! `UntagResource` removes exactly the keys it names, and `ListTagsForResource` reports
//! the set those mutations left. (The read's own contract is the sibling suite
//! `resource_tags.rs`; this one is about the two mutations.) Every case goes through the
//! router, so it exercises routing, deserialization, the handler, the persisted state
//! and serialization.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok, refused, REGION, REGISTRY_ID};

/// "Tag keys can have a maximum character length of 128 characters, and tag values can
/// have a maximum length of 256 characters" — `InvalidTagParameterException`'s own
/// documentation in this service's Smithy model.
const MAX_TAG_KEY_LEN: usize = 128;
/// See [`MAX_TAG_KEY_LEN`].
const MAX_TAG_VALUE_LEN: usize = 256;

/// "The maximum number of tags that can be applied to a repository is 50" —
/// `TooManyTagsException`'s own documentation in this service's Smithy model.
const MAX_TAGS: usize = 50;

/// The tag-key prefix AWS reserves for itself (AWS General Reference, "Tagging AWS
/// resources": "the `aws:` prefix is reserved for AWS use").
const RESERVED_PREFIX: &str = "aws:";

/// A resource's tag set, keyed by tag key — the order a listing reports is not part of
/// the contract, the mapping is.
type TagSet = BTreeMap<String, String>;

/// One row of the merge table: the case name, the `tags` the request carries, and the
/// tag set the resource holds after it.
type MergeCase<'a> = (&'a str, Vec<(String, String)>, TagSet);

/// The `tags` member of a request, from key-value pairs. The model names the members
/// `Key` and `Value`, so that is what goes on the wire.
fn tags_json<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key.as_ref(), "Value": value.as_ref()}))
            .collect(),
    )
}

/// The ARN of a repository in this emulator's registry.
fn repository_arn(name: &str) -> String {
    format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}")
}

/// Create a repository, carrying `tags` unless they are `Value::Null`.
async fn given_repository(router: &MultiServiceRouter, name: &str, tags: Value) -> Value {
    let mut request = json!({"repositoryName": name});
    if !tags.is_null() {
        request["tags"] = tags;
    }
    ok(router, name, "CreateRepository", &request).await
}

/// The tag set `ListTagsForResource` reports for `name`.
async fn listed_tags(router: &MultiServiceRouter, case: &str, name: &str) -> TagSet {
    let body = ok(
        router,
        case,
        "ListTagsForResource",
        &json!({"resourceArn": repository_arn(name)}),
    )
    .await;
    let tags = body["tags"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: ListTagsForResource must report a `tags` list, got {body:?}")
    });
    tags.iter()
        .map(|tag| {
            let key = tag["Key"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every reported tag carries a Key, got {tag:?}")
            });
            let value = tag["Value"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every reported tag carries a Value, got {tag:?}")
            });
            (key.to_string(), value.to_string())
        })
        .collect()
}

/// A [`TagSet`] from pairs, for the expectation side of a table row.
fn tag_set(pairs: &[(&str, &str)]) -> TagSet {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `TagResource` merges what it is given into what the resource holds, and
/// `ListTagsForResource` reports the result — over a table of requests that differ in
/// how many keys they carry, whether those keys are new, and how long and what kind of
/// text the keys and values are.
#[tokio::test]
async fn tagging_merges_into_the_stored_tag_set() {
    let router = emulator();
    given_repository(&router, "merge-target", json!([])).await;

    // A key and a value at exactly the published maximum length.
    let max_key = "k".repeat(MAX_TAG_KEY_LEN);
    let max_value = "v".repeat(MAX_TAG_VALUE_LEN);

    // Each row is applied to the same repository in order, so the expected set is
    // cumulative: that is what "existing tags are not changed if they are not specified"
    // means.
    let cases: Vec<MergeCase<'_>> = vec![
        (
            "one key, on a repository holding none",
            vec![("env".to_string(), "prod".to_string())],
            tag_set(&[("env", "prod")]),
        ),
        (
            "a second key leaves the first alone",
            vec![("team".to_string(), "infra".to_string())],
            tag_set(&[("env", "prod"), ("team", "infra")]),
        ),
        (
            "a key already held takes the new value in place",
            vec![("env".to_string(), "staging".to_string())],
            tag_set(&[("env", "staging"), ("team", "infra")]),
        ),
        (
            "three keys in one request, one of them already held",
            vec![
                ("env".to_string(), "dev".to_string()),
                ("cost-centre".to_string(), "4711".to_string()),
                ("owner".to_string(), "platform".to_string()),
            ],
            tag_set(&[
                ("cost-centre", "4711"),
                ("env", "dev"),
                ("owner", "platform"),
                ("team", "infra"),
            ]),
        ),
        (
            "an empty tag list changes nothing",
            vec![],
            tag_set(&[
                ("cost-centre", "4711"),
                ("env", "dev"),
                ("owner", "platform"),
                ("team", "infra"),
            ]),
        ),
        (
            "an empty value is a value",
            vec![("blank".to_string(), String::new())],
            tag_set(&[
                ("blank", ""),
                ("cost-centre", "4711"),
                ("env", "dev"),
                ("owner", "platform"),
                ("team", "infra"),
            ]),
        ),
        (
            "a key of exactly the maximum length, and a value of exactly the maximum",
            vec![(max_key.clone(), max_value.clone())],
            {
                let mut expected = tag_set(&[
                    ("blank", ""),
                    ("cost-centre", "4711"),
                    ("env", "dev"),
                    ("owner", "platform"),
                    ("team", "infra"),
                ]);
                expected.insert(max_key.clone(), max_value.clone());
                expected
            },
        ),
        (
            "a non-ASCII key and value, counted in characters",
            vec![("проект".to_string(), "тест".to_string())],
            {
                let mut expected = tag_set(&[
                    ("blank", ""),
                    ("cost-centre", "4711"),
                    ("env", "dev"),
                    ("owner", "platform"),
                    ("team", "infra"),
                    ("проект", "тест"),
                ]);
                expected.insert(max_key.clone(), max_value.clone());
                expected
            },
        ),
        (
            "a key that contains the reserved prefix without beginning with it",
            vec![("not-aws:internal".to_string(), "mine".to_string())],
            {
                let mut expected = tag_set(&[
                    ("blank", ""),
                    ("cost-centre", "4711"),
                    ("env", "dev"),
                    ("not-aws:internal", "mine"),
                    ("owner", "platform"),
                    ("team", "infra"),
                    ("проект", "тест"),
                ]);
                expected.insert(max_key.clone(), max_value.clone());
                expected
            },
        ),
    ];

    for (case, tags, expected) in cases {
        ok(
            &router,
            case,
            "TagResource",
            &json!({
                "resourceArn": repository_arn("merge-target"),
                "tags": tags_json(&tags),
            }),
        )
        .await;

        let listed = listed_tags(&router, case, "merge-target").await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the tag set {expected:?}, got {listed:?}"
        );
    }
}

/// `UntagResource` removes exactly the keys it names, and nothing else.
#[tokio::test]
async fn untagging_removes_only_the_keys_it_names() {
    let router = emulator();
    let held = [
        ("a", "1"),
        ("b", "2"),
        ("c", "3"),
        ("d", "4"),
        ("env", "prod"),
    ];
    given_repository(&router, "untag-target", tags_json(&held)).await;

    let cases: Vec<(&str, Vec<&str>, TagSet)> = vec![
        (
            "one held key",
            vec!["env"],
            tag_set(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
        ),
        (
            "a key the repository does not carry removes nothing",
            vec!["never-set"],
            tag_set(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
        ),
        (
            "several keys in one request",
            vec!["a", "c"],
            tag_set(&[("b", "2"), ("d", "4")]),
        ),
        (
            "a held key beside one that is not",
            vec!["b", "never-set"],
            tag_set(&[("d", "4")]),
        ),
        ("an empty key list", vec![], tag_set(&[("d", "4")])),
        ("every remaining key", vec!["d"], TagSet::new()),
        (
            "every key again, on a resource now holding none",
            vec!["d"],
            TagSet::new(),
        ),
    ];

    for (case, keys, expected) in cases {
        ok(
            &router,
            case,
            "UntagResource",
            &json!({
                "resourceArn": repository_arn("untag-target"),
                "tagKeys": keys,
            }),
        )
        .await;

        let listed = listed_tags(&router, case, "untag-target").await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the tag set {expected:?}, got {listed:?}"
        );
    }
}

/// A tag the service cannot store is refused with the modelled error, and the resource's
/// tag set is left exactly as it was.
#[tokio::test]
async fn a_tag_the_service_cannot_store_is_refused() {
    let router = emulator();
    let held = [("env", "prod")];
    given_repository(&router, "refusal-target", tags_json(&held)).await;

    let arn = repository_arn("refusal-target");
    let over_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    // Reserved keys AWS itself sets, none of them the one the issue cited.
    let reserved: Vec<String> = vec![
        format!("{RESERVED_PREFIX}cloudformation:stack-name"),
        format!("{RESERVED_PREFIX}created-by"),
        RESERVED_PREFIX.to_string(),
    ];

    let mut cases: Vec<(String, Value, &str)> = vec![
        (
            format!("a key of {} characters", MAX_TAG_KEY_LEN + 1),
            json!({"resourceArn": arn, "tags": tags_json(&[(&over_long_key, "x")])}),
            "InvalidTagParameterException",
        ),
        (
            format!("a value of {} characters", MAX_TAG_VALUE_LEN + 1),
            json!({"resourceArn": arn, "tags": tags_json(&[("long", &over_long_value)])}),
            "InvalidTagParameterException",
        ),
        (
            format!("{} distinct keys in one request", MAX_TAGS + 1),
            json!({
                "resourceArn": arn,
                "tags": Value::Array(
                    (0..=MAX_TAGS)
                        .map(|index| json!({"Key": format!("k{index:03}"), "Value": "x"}))
                        .collect(),
                ),
            }),
            "TooManyTagsException",
        ),
    ];
    for key in &reserved {
        cases.push((
            format!("the reserved key {key:?}"),
            json!({"resourceArn": arn, "tags": tags_json(&[(key, "x")])}),
            "InvalidParameterException",
        ));
        cases.push((
            format!("the reserved key {key:?} beside an acceptable one"),
            json!({
                "resourceArn": arn,
                "tags": tags_json(&[(key.as_str(), "x"), ("fine", "yes")]),
            }),
            "InvalidParameterException",
        ));
    }

    for (case, request, expected) in cases {
        refused(&router, &case, "TagResource", &request, expected).await;
        let listed = listed_tags(&router, &case, "refusal-target").await;
        assert_eq!(
            listed,
            tag_set(&held),
            "case {case:?}: a refused TagResource must store nothing, got {listed:?}"
        );
    }

    // The same keys cannot be REMOVED either: AWS's own tags are not a caller's to
    // delete, and an over-long key is no more nameable here than it is above.
    for key in reserved.iter().chain([&over_long_key]) {
        let case = format!("UntagResource naming {key:?}");
        let expected = if key.starts_with(RESERVED_PREFIX) {
            "InvalidParameterException"
        } else {
            "InvalidTagParameterException"
        };
        refused(
            &router,
            &case,
            "UntagResource",
            &json!({"resourceArn": arn, "tagKeys": [key]}),
            expected,
        )
        .await;
        let listed = listed_tags(&router, &case, "refusal-target").await;
        assert_eq!(
            listed,
            tag_set(&held),
            "case {case:?}: a refused UntagResource must remove nothing, got {listed:?}"
        );
    }
}

/// The tag limit counts the tags the resource ends up with: a key already held is
/// replaced rather than added, so re-tagging a full resource is accepted and one more
/// key is not.
#[tokio::test]
async fn the_tag_limit_counts_the_resulting_set() {
    let router = emulator();
    given_repository(&router, "limit-target", json!([])).await;
    let arn = repository_arn("limit-target");

    let full: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|index| (format!("k{index:03}"), index.to_string()))
        .collect();
    ok(
        &router,
        "the resource is filled to the limit",
        "TagResource",
        &json!({"resourceArn": arn, "tags": tags_json(&full)}),
    )
    .await;
    let listed = listed_tags(&router, "filled to the limit", "limit-target").await;
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case {:?}: expected {MAX_TAGS} tags, got {listed:?}",
        "filled to the limit"
    );

    // A key already held: a replacement, so still 50 tags.
    ok(
        &router,
        "replacing a held key at the limit",
        "TagResource",
        &json!({"resourceArn": arn, "tags": tags_json(&[("k000", "replaced")])}),
    )
    .await;
    let listed = listed_tags(&router, "replacing a held key at the limit", "limit-target").await;
    assert_eq!(
        listed.get("k000").map(String::as_str),
        Some("replaced"),
        "case {:?}: the held key takes the new value, got {listed:?}",
        "replacing a held key at the limit"
    );
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case {:?}: a replacement adds no tag, got {listed:?}",
        "replacing a held key at the limit"
    );

    // One more key would be the 51st.
    refused(
        &router,
        "one key past the limit",
        "TagResource",
        &json!({"resourceArn": arn, "tags": tags_json(&[("one-too-many", "x")])}),
        "TooManyTagsException",
    )
    .await;
    let listed = listed_tags(&router, "one key past the limit", "limit-target").await;
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case {:?}: the refused request stored nothing, got {listed:?}",
        "one key past the limit"
    );

    // And the limit is a ceiling, not a trap: freeing a key makes room for another.
    ok(
        &router,
        "a key is freed",
        "UntagResource",
        &json!({"resourceArn": arn, "tagKeys": ["k000"]}),
    )
    .await;
    ok(
        &router,
        "the freed slot takes a new key",
        "TagResource",
        &json!({"resourceArn": arn, "tags": tags_json(&[("one-too-many", "x")])}),
    )
    .await;
    let listed = listed_tags(&router, "the freed slot takes a new key", "limit-target").await;
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case {:?}: expected {MAX_TAGS} tags again, got {listed:?}",
        "the freed slot takes a new key"
    );
    assert!(
        listed.contains_key("one-too-many") && !listed.contains_key("k000"),
        "case {:?}: expected the new key and not the freed one, got {listed:?}",
        "the freed slot takes a new key"
    );
}

/// Both mutations address their resource by ARN through the same resolution, so each one
/// refuses the same ARNs — a malformed ARN, one of another service or resource type, and
/// one of another registry are all bad parameters, while a well-formed ARN naming a
/// repository this registry does not hold is a missing repository.
#[tokio::test]
async fn an_arn_that_names_no_repository_here_is_refused_by_both_mutations() {
    let router = emulator();
    given_repository(&router, "present", json!([])).await;

    let bad_arns: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("a bare name", "present".to_string()),
        ("a string that is not an ARN at all", "not-an-arn".to_string()),
        (
            "an ARN with too few fields",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}"),
        ),
        ("another service's ARN", "arn:aws:s3:::my-bucket".to_string()),
        (
            "an ECR ARN of another resource type",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:image/present"),
        ),
        (
            "a repository ARN with no name",
            format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/"),
        ),
        (
            "an ARN with an empty partition",
            format!("arn::ecr:{REGION}:{REGISTRY_ID}:repository/present"),
        ),
        (
            "another registry's repository ARN",
            format!("arn:aws:ecr:{REGION}:123456789012:repository/present"),
        ),
    ];

    // The two mutations, each with the rest of its required input.
    let operations: [(&str, Value); 2] = [
        ("TagResource", json!({"tags": tags_json(&[("env", "prod")])})),
        ("UntagResource", json!({"tagKeys": ["env"]})),
    ];

    for (operation, extra) in &operations {
        for (case, arn) in &bad_arns {
            let mut request = extra.clone();
            request["resourceArn"] = json!(arn);
            let case = format!("{operation}: {case}");
            refused(
                &router,
                &case,
                operation,
                &request,
                "InvalidParameterException",
            )
            .await;
        }

        // A well-formed ARN of this registry naming a repository it does not hold is a
        // missing repository, not a bad parameter.
        let mut request = extra.clone();
        request["resourceArn"] = json!(repository_arn("missing-repo"));
        refused(
            &router,
            &format!("{operation}: a repository this registry does not hold"),
            operation,
            &request,
            "RepositoryNotFoundException",
        )
        .await;

        // And the repository that IS there is reached by the same ARN.
        let mut request = extra.clone();
        request["resourceArn"] = json!(repository_arn("present"));
        ok(
            &router,
            &format!("{operation}: the repository this registry holds"),
            operation,
            &request,
        )
        .await;
    }
}

/// The tags a repository is created with are the same tags these operations manage: a
/// `TagResource` continues from the created set, and a create carrying a reserved key is
/// refused exactly as a `TagResource` carrying one is.
#[tokio::test]
async fn a_creates_tags_are_the_tags_the_mutations_continue_from() {
    let router = emulator();

    let cases: Vec<(&str, Value, TagSet)> = vec![
        ("no tags member at all", Value::Null, TagSet::new()),
        ("an empty tags list", json!([]), TagSet::new()),
        (
            "one tag",
            tags_json(&[("env", "prod")]),
            tag_set(&[("env", "prod")]),
        ),
        (
            "several tags",
            tags_json(&[("env", "prod"), ("team", "infra"), ("blank", "")]),
            tag_set(&[("blank", ""), ("env", "prod"), ("team", "infra")]),
        ),
    ];

    for (index, (case, tags, expected)) in cases.into_iter().enumerate() {
        let name = format!("created-{index}");
        given_repository(&router, &name, tags).await;
        let listed = listed_tags(&router, case, &name).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the created repository to carry {expected:?}, got {listed:?}"
        );

        // And the tagging operations continue from there rather than from nothing.
        ok(
            &router,
            case,
            "TagResource",
            &json!({
                "resourceArn": repository_arn(&name),
                "tags": tags_json(&[("added", "later")]),
            }),
        )
        .await;
        let mut after = expected.clone();
        after.insert("added".to_string(), "later".to_string());
        let listed = listed_tags(&router, case, &name).await;
        assert_eq!(
            listed, after,
            "case {case:?}: expected {after:?} after the tag, got {listed:?}"
        );
    }

    // A reserved key is refused at creation too, and no repository is stored for it.
    let refused_name = "created-with-a-reserved-key";
    refused(
        &router,
        "CreateRepository with a reserved tag key",
        "CreateRepository",
        &json!({
            "repositoryName": refused_name,
            "tags": tags_json(&[(format!("{RESERVED_PREFIX}created-by"), "someone")]),
        }),
        "InvalidParameterException",
    )
    .await;
    refused(
        &router,
        "CreateRepository with a reserved tag key stores nothing",
        "DescribeRepositories",
        &json!({"repositoryNames": [refused_name]}),
        "RepositoryNotFoundException",
    )
    .await;
}

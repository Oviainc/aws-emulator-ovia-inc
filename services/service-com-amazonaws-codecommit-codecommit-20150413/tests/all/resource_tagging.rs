// SPDX-License-Identifier: CC0-1.0

//! Resource tagging over the wire: `TagResource`, `ListTagsForResource` and
//! `UntagResource`, driven as awsJson1_1 requests through the router that serves
//! the real emulator.
//!
//! The rule under test is that a CodeCommit tagging call addresses the repository
//! its `resourceArn` names and computes its answer from that repository's stored
//! tag set: tagging merges the request's pairs into the stored set (overwriting a
//! key it already carries), untagging removes the named keys, and
//! `ListTagsForResource` returns exactly what the stored set holds, while an ARN
//! that addresses no CodeCommit resource and a tag set outside the documented
//! limits are refused with the modelled error and persist nothing.
//!
//! The tables below span the class the rule covers — tag counts, key and value
//! lengths at the modelled bounds, overwrites, removals of absent keys, and every
//! shape of unusable ARN — rather than one recorded request. The wire harness is
//! the one `repository_lifecycle` already owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::repository_lifecycle::{call, emulator, error, ok, REGION};

/// The ARN form CodeCommit publishes for a repository, which is what its tagging
/// operations address:
/// `arn:aws:codecommit:<region>:<account-id>:<repository-name>`.
fn repository_arn(name: &str) -> String {
    format!(
        "arn:aws:codecommit:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// A string of `length` 'a's — used to reach a modelled length bound exactly.
fn filler(length: usize) -> String {
    "a".repeat(length)
}

/// The tags `ListTagsForResource` reports for a resource, as an ordered map so a
/// comparison does not depend on JSON member order.
async fn listed_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let payload = ok(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    assert!(
        payload.get("nextToken").is_none_or(Value::is_null),
        "a tag set within the per-resource limit fits in one page, so nextToken must be absent: {payload}"
    );
    payload
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One refused `CreateRepository` tag set: its label, the repository name it
/// would have created, the tags it carries and the modelled error it answers.
type CreateCase = (&'static str, &'static str, Vec<(String, String)>, &'static str);

/// A `TagsMap` as JSON, which both `TagResource` and `CreateRepository` carry.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<serde_json::Map<String, Value>>(),
    )
}

/// A `TagResource` request body: the resource ARN and the pairs to set.
fn tag_body(resource_arn: &str, pairs: &[(String, String)]) -> Value {
    json!({ "resourceArn": resource_arn, "tags": tag_map(pairs) })
}

/// An `UntagResource` request body: the resource ARN and the keys to remove.
fn untag_body(resource_arn: &str, tag_keys: &[String]) -> Value {
    json!({ "resourceArn": resource_arn, "tagKeys": tag_keys })
}

/// One tagging round trip: the tags the repository is created with, the pairs
/// `TagResource` adds, the keys `UntagResource` then removes, and what
/// `ListTagsForResource` must report after each step.
struct Case {
    label: &'static str,
    created_with: Vec<(String, String)>,
    tagged: Vec<(String, String)>,
    untagged: Vec<String>,
    after_tag: Vec<(String, String)>,
    after_untag: Vec<(String, String)>,
}

fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// The rows span what the rule generalises over: how many pairs a call carries,
/// whether a key is new or already stored, a value at `TagValue`'s modelled
/// minimum length (0) and a key and value at `TagKey`/`TagValue`'s modelled
/// maximum lengths (128 and 256 —
/// `smithy_query(query="shape",shape="TagKey,TagValue")`), and removing a key the
/// resource does not carry.
fn roundtrip_cases() -> Vec<Case> {
    let long_key = filler(128);
    let long_value = filler(256);
    vec![
        Case {
            label: "one tag added to an untagged repository, then removed",
            created_with: Vec::new(),
            tagged: owned(&[("env", "prod")]),
            untagged: keys(&["env"]),
            after_tag: owned(&[("env", "prod")]),
            after_untag: Vec::new(),
        },
        Case {
            label: "several tags, one overwriting the value a creation tag stored",
            created_with: owned(&[("stage", "alpha"), ("keep", "yes")]),
            tagged: owned(&[("stage", "beta"), ("owner", "team-a"), ("cost", "42")]),
            untagged: keys(&["stage", "cost"]),
            after_tag: owned(&[
                ("cost", "42"),
                ("keep", "yes"),
                ("owner", "team-a"),
                ("stage", "beta"),
            ]),
            after_untag: owned(&[("keep", "yes"), ("owner", "team-a")]),
        },
        Case {
            label: "a value at TagValue's modelled minimum length (0)",
            created_with: Vec::new(),
            tagged: owned(&[("empty", ""), ("other", "set")]),
            untagged: keys(&["other"]),
            after_tag: owned(&[("empty", ""), ("other", "set")]),
            after_untag: owned(&[("empty", "")]),
        },
        Case {
            label: "a key and a value at their modelled maximum lengths (128 and 256)",
            created_with: Vec::new(),
            tagged: vec![(long_key.clone(), long_value.clone())],
            untagged: vec![long_key.clone()],
            after_tag: vec![(long_key.clone(), long_value)],
            after_untag: Vec::new(),
        },
        Case {
            label: "removing a key the resource does not carry leaves the rest stored",
            created_with: owned(&[("x", "1")]),
            tagged: owned(&[("y", "2")]),
            untagged: keys(&["absent", "y", "absent"]),
            after_tag: owned(&[("x", "1"), ("y", "2")]),
            after_untag: owned(&[("x", "1")]),
        },
    ]
}

#[tokio::test]
async fn tags_written_by_tag_resource_are_read_back_and_removed_by_untag_resource() {
    let router = emulator();
    for (index, case) in roundtrip_cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("tagged-repo-{index}");
        let created = ok(
            &router,
            "CreateRepository",
            &json!({
                "repositoryName": name,
                "tags": case
                    .created_with
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect::<serde_json::Map<String, Value>>(),
            }),
        )
        .await;
        // The ARN the service itself reported is what a caller tags with.
        let arn = created
            .pointer("/repositoryMetadata/Arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {label:?}: CreateRepository reported no ARN"))
            .to_string();
        assert_eq!(
            arn,
            repository_arn(&name),
            "case {label:?}: the repository ARN must be the CodeCommit resource ARN, got {arn:?}"
        );

        // The tags the creation call stored are already the resource's tag set.
        let listed = listed_tags(&router, &arn).await;
        let created_tags: BTreeMap<String, String> = case.created_with.iter().cloned().collect();
        assert_eq!(
            listed, created_tags,
            "case {label:?}: ListTagsForResource must report the creation tags, got {listed:?}"
        );

        ok(&router, "TagResource", &tag_body(&arn, &case.tagged)).await;
        let listed = listed_tags(&router, &arn).await;
        let after_tag: BTreeMap<String, String> = case.after_tag.iter().cloned().collect();
        assert_eq!(
            listed, after_tag,
            "case {label:?}: TagResource must merge its pairs into the stored tag set, got {listed:?}"
        );
        // A read does not change what the next caller sees.
        let again = listed_tags(&router, &arn).await;
        assert_eq!(
            again, after_tag,
            "case {label:?}: ListTagsForResource must not change the stored tag set, got {again:?}"
        );

        ok(&router, "UntagResource", &untag_body(&arn, &case.untagged)).await;
        let listed = listed_tags(&router, &arn).await;
        let after_untag: BTreeMap<String, String> = case.after_untag.iter().cloned().collect();
        assert_eq!(
            listed, after_untag,
            "case {label:?}: UntagResource must remove exactly the keys it named, got {listed:?}"
        );
        // Removing the same keys a second time is the same stored set: the
        // operation is idempotent.
        ok(&router, "UntagResource", &untag_body(&arn, &case.untagged)).await;
        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed, after_untag,
            "case {label:?}: a repeated UntagResource must leave the stored set as it was, got {listed:?}"
        );
    }
}

/// Every shape of `resourceArn` that addresses no CodeCommit repository, with the
/// modelled error each one answers. The repository-name rows sit on
/// `RepositoryName`'s published bounds (modelled `length` 1..100 and `pattern`
/// `^[\w\.-]+$`, `smithy_query(query="shape",shape="RepositoryName")`): a name one
/// character past the maximum, and a character the pattern excludes.
fn unusable_arn_cases(existing: &str) -> Vec<(String, String, &'static str)> {
    let account = engine::emulator_account_id();
    vec![
        (
            "an empty ARN".to_string(),
            String::new(),
            "ResourceArnRequiredException",
        ),
        (
            "a blank ARN".to_string(),
            "   ".to_string(),
            "ResourceArnRequiredException",
        ),
        (
            "a value that is not an ARN".to_string(),
            "not-an-arn".to_string(),
            "InvalidResourceArnException",
        ),
        (
            "an ARN missing its resource field".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}"),
            "InvalidResourceArnException",
        ),
        (
            "an ARN with an empty resource field".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:"),
            "InvalidResourceArnException",
        ),
        (
            "an ARN of another service".to_string(),
            format!("arn:aws:s3:{REGION}:{account}:a-bucket"),
            "InvalidResourceArnException",
        ),
        (
            "an ARN with no region".to_string(),
            format!("arn:aws:codecommit::{account}:{existing}"),
            "InvalidResourceArnException",
        ),
        (
            "an ARN whose account id is not twelve digits".to_string(),
            format!("arn:aws:codecommit:{REGION}:12345:{existing}"),
            "InvalidResourceArnException",
        ),
        (
            "an ARN whose resource field carries a further colon".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:{existing}:extra"),
            "InvalidRepositoryNameException",
        ),
        (
            "a repository name holding a character the modelled pattern excludes".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:bad name"),
            "InvalidRepositoryNameException",
        ),
        (
            // CodeCommit documents "The suffix .git is prohibited" on every
            // operation that takes a repository name, so a name no repository can
            // carry is not a valid one here either.
            "a repository name carrying the prohibited .git suffix".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:archive.git"),
            "InvalidRepositoryNameException",
        ),
        (
            // `\w` in a Smithy (ECMA-262) pattern is ASCII, so a non-ASCII letter
            // is outside `RepositoryName` however letter-like it is.
            "a repository name holding a non-ASCII letter".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:répo"),
            "InvalidRepositoryNameException",
        ),
        (
            "a repository name one character past the modelled maximum (101)".to_string(),
            format!("arn:aws:codecommit:{REGION}:{account}:{}", filler(101)),
            "InvalidRepositoryNameException",
        ),
        (
            "a well-formed ARN naming a repository that does not exist".to_string(),
            repository_arn("never-created"),
            "RepositoryDoesNotExistException",
        ),
        (
            "a well-formed ARN whose name is at the modelled maximum (100)".to_string(),
            repository_arn(&filler(100)),
            "RepositoryDoesNotExistException",
        ),
        (
            "a well-formed ARN whose name is at the modelled minimum (1)".to_string(),
            repository_arn("r"),
            "RepositoryDoesNotExistException",
        ),
    ]
}

#[tokio::test]
async fn every_tagging_operation_refuses_an_arn_that_addresses_no_repository() {
    let router = emulator();
    let existing = "arn-case-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": existing }),
    )
    .await;

    for (label, arn, expected_code) in unusable_arn_cases(existing) {
        // The same ARN rule holds for all three tagging operations, so each row
        // is asserted on every one of them.
        let requests = [
            ("TagResource", json!({ "resourceArn": arn, "tags": { "k": "v" } })),
            ("ListTagsForResource", json!({ "resourceArn": arn })),
            ("UntagResource", json!({ "resourceArn": arn, "tagKeys": ["k"] })),
        ];
        for (operation, body) in requests {
            let code = error(&router, operation, &body, 400).await;
            assert_eq!(
                code, expected_code,
                "case {label:?} on {operation}: expected {expected_code}, got {code}"
            );
        }
    }

    // None of those refusals touched the repository's tag set.
    let listed = listed_tags(&router, &repository_arn(existing)).await;
    assert!(
        listed.is_empty(),
        "a refused tagging call must persist nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_tag_set_outside_the_documented_limits_is_refused_and_persists_nothing() {
    let router = emulator();
    let name = "limits-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
    let arn = repository_arn(name);

    // The limits, each with the row that sits on it and the row one past it.
    let limit = service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_TAGS_PER_RESOURCE;
    // AWS's documented per-resource tag limit: "You can add up to 50 tags to a
    // single CodeCommit repository" (CodeCommit User Guide, "Tag a repository"),
    // pinned here so the rows below assert the rule rather than the constant.
    assert_eq!(
        limit, 50,
        "the per-resource tag limit must be AWS's documented 50, got {limit}"
    );
    let at_limit: Vec<(String, String)> = (0..limit)
        .map(|index| (format!("key-{index:03}"), index.to_string()))
        .collect();
    let past_limit: Vec<(String, String)> = (0..=limit)
        .map(|index| (format!("key-{index:03}"), index.to_string()))
        .collect();

    // One past the limit is refused, and nothing is stored.
    let code = error(&router, "TagResource", &tag_body(&arn, &past_limit), 400).await;
    assert_eq!(
        code, "TooManyTagsException",
        "case \"one tag past the per-resource limit\": expected TooManyTagsException, got {code}"
    );
    let listed = listed_tags(&router, &arn).await;
    assert!(
        listed.is_empty(),
        "case \"one tag past the per-resource limit\": a refused call must persist nothing, got {} tags",
        listed.len()
    );

    // Exactly at the limit is accepted and read back whole.
    ok(&router, "TagResource", &tag_body(&arn, &at_limit)).await;
    let listed = listed_tags(&router, &arn).await;
    assert_eq!(
        listed,
        at_limit.iter().cloned().collect::<BTreeMap<_, _>>(),
        "case \"a tag set at the per-resource limit\": expected {limit} tags read back, got {} ",
        listed.len()
    );

    // One more tag on a resource already at the limit is refused, and the stored
    // set is untouched.
    let code = error(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": { "one-too-many": "x" } }),
        400,
    )
    .await;
    assert_eq!(
        code, "TooManyTagsException",
        "case \"a tag added to a resource already at the limit\": expected TooManyTagsException, got {code}"
    );
    let listed = listed_tags(&router, &arn).await;
    assert_eq!(
        listed.len(),
        limit,
        "case \"a tag added to a resource already at the limit\": the stored set must be unchanged, got {} tags",
        listed.len()
    );

    // Overwriting a key the resource already carries is not a new tag, so it is
    // accepted at the limit.
    ok(
        &router,
        "TagResource",
        &json!({ "resourceArn": arn, "tags": { "key-000": "rewritten" } }),
    )
    .await;
    let listed = listed_tags(&router, &arn).await;
    assert_eq!(
        listed.get("key-000").map(String::as_str),
        Some("rewritten"),
        "case \"overwriting a stored key at the limit\": expected the new value, got {:?}",
        listed.get("key-000")
    );
    assert_eq!(
        listed.len(),
        limit,
        "case \"overwriting a stored key at the limit\": the tag count must not grow, got {}",
        listed.len()
    );
}

#[tokio::test]
async fn a_tag_request_outside_the_documented_rules_is_refused_with_the_modeled_error() {
    let router = emulator();
    let name = "rules-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": name }),
    )
    .await;
    let arn = repository_arn(name);

    // "Key names cannot be prefixed with aws:." (InvalidSystemTagUsageException),
    // and the required-but-empty collections each operation documents.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an empty tags map",
            "TagResource",
            tag_body(&arn, &[]),
            "TagsMapRequiredException",
        ),
        (
            "a system tag key on TagResource",
            "TagResource",
            tag_body(&arn, &owned(&[("aws:created-by", "me")])),
            "InvalidSystemTagUsageException",
        ),
        (
            "a system tag key beside a usable one",
            "TagResource",
            tag_body(
                &arn,
                &owned(&[("team", "a"), ("aws:cloudformation:stack", "s")]),
            ),
            "InvalidSystemTagUsageException",
        ),
        (
            "an empty tag key list",
            "UntagResource",
            untag_body(&arn, &[]),
            "TagKeysListRequiredException",
        ),
        (
            "a system tag key on UntagResource",
            "UntagResource",
            untag_body(&arn, &keys(&["aws:created-by"])),
            "InvalidSystemTagUsageException",
        ),
        (
            "a tag key one character past TagKey's modelled maximum (129)",
            "TagResource",
            tag_body(&arn, &[(filler(129), "v".to_string())]),
            "InvalidTagsMapException",
        ),
        (
            "an empty tag key, below TagKey's modelled minimum (1)",
            "TagResource",
            tag_body(&arn, &[(String::new(), "v".to_string())]),
            "InvalidTagsMapException",
        ),
        (
            "a tag value one character past TagValue's modelled maximum (257)",
            "TagResource",
            tag_body(&arn, &[("k".to_string(), filler(257))]),
            "InvalidTagsMapException",
        ),
        (
            "a tag key list holding a key past the modelled maximum (129)",
            "UntagResource",
            untag_body(&arn, &[filler(129)]),
            "InvalidTagKeysListException",
        ),
    ];

    for (label, operation, body, expected_code) in cases {
        let code = error(&router, operation, &body, 400).await;
        assert_eq!(
            code, expected_code,
            "case {label:?} on {operation}: expected {expected_code}, got {code}"
        );
    }

    // Not one of those refusals stored anything.
    let listed = listed_tags(&router, &arn).await;
    assert!(
        listed.is_empty(),
        "a refused tagging call must persist nothing, got {listed:?}"
    );

    // The same `TagsMap` rules hold on the other operation that carries a tags
    // map, `CreateRepository`: a tag set it refuses creates no repository.
    let limit = service_com_amazonaws_codecommit_codecommit_20150413::state::MAX_TAGS_PER_RESOURCE;
    let past_limit: Vec<(String, String)> = (0..=limit)
        .map(|index| (format!("key-{index:03}"), index.to_string()))
        .collect();
    let create_cases: Vec<CreateCase> = vec![
        (
            "a tag key past TagKey's modelled maximum (129)",
            "created-with-long-key",
            vec![(filler(129), "v".to_string())],
            "InvalidTagsMapException",
        ),
        (
            "a tag value past TagValue's modelled maximum (257)",
            "created-with-long-value",
            vec![("k".to_string(), filler(257))],
            "InvalidTagsMapException",
        ),
        (
            "an empty tag key, below TagKey's modelled minimum (1)",
            "created-with-empty-key",
            vec![(String::new(), "v".to_string())],
            "InvalidTagsMapException",
        ),
        (
            "one tag past the per-resource limit",
            "created-past-the-limit",
            past_limit,
            "TooManyTagsException",
        ),
        (
            "a system tag key",
            "created-with-system-tag",
            owned(&[("aws:created-by", "me")]),
            "InvalidSystemTagUsageException",
        ),
    ];
    for (label, repository, tags, expected_code) in create_cases {
        let code = error(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": repository, "tags": tag_map(&tags) }),
            400,
        )
        .await;
        assert_eq!(
            code, expected_code,
            "case {label:?} on CreateRepository: expected {expected_code}, got {code}"
        );
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": repository }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {label:?}: a refused CreateRepository must create nothing, got {code}"
        );
    }
}

#[tokio::test]
async fn list_tags_for_resource_serves_one_page_and_refuses_a_token_it_did_not_issue() {
    let router = emulator();
    let name = "paging-repo";
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": name, "tags": { "a": "1", "b": "2", "c": "3" } }),
    )
    .await;
    let arn = repository_arn(name);

    // Continuation tokens are the service's own opaque tokens
    // (`state::page_of`), so none of these — a word, a bare decimal offset a
    // caller might invent, a negative or fractional one, or a hex string that
    // decodes to something else — is a token this operation issued, and each is
    // refused rather than silently serving the first page.
    for token in ["not-a-token", "0", "1", "99", "-1", "1.5", "6a6f"] {
        let (status, payload) = call(
            &router,
            "ListTagsForResource",
            &json!({ "resourceArn": arn, "nextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {token:?}: a nextToken this operation never issued must be refused, got {status} with {payload}"
        );
    }

    // A tag set within the per-resource limit comes back whole in one page, so
    // this operation mints no token for it.
    let listed = listed_tags(&router, &arn).await;
    assert_eq!(
        listed,
        expected(&[("a", "1"), ("b", "2"), ("c", "3")]),
        "the whole tag set must come back in one page, got {listed:?}"
    );
}

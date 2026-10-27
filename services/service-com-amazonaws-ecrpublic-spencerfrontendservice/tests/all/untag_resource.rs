// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, driven end to end over the wire.
//!
//! The rule under test: `UntagResource` "deletes specified tags from a resource" — the
//! repository its `resourceArn` names ends up carrying exactly the tags it held whose keys
//! the request did NOT list, each with the value it had, so a key the repository does not
//! carry removes nothing and asking for the same removal twice leaves the same set. A tag
//! key in the namespace AWS keeps for its own tags (`aws:`) names a system tag an API
//! caller may not change, a `resourceArn` that is not a well-formed ECR Public repository
//! ARN is an invalid parameter and a well-formed one naming no repository of this registry
//! is a missing repository. A refused call removes nothing at all.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>`), reusing the router harness in
//! [`super::repository_image_lifecycle`], and every tag set is read back through
//! `ListTagsForResource`, so nothing reaches into the state struct and a response that is
//! not computed from persisted state cannot pass.
//!
//! The tables span the classes the rule generalises over: how many of the held keys a
//! request names (none, one, several, all), keys it names that are not held at all, a key
//! named twice in one request, keys sent out of key order, keys outside ASCII, the tag set
//! at the published limit of 50, and the forms a repository name takes. The expected
//! remainder of each row is COMPUTED as the held set minus the named keys rather than
//! written out, so a row cannot agree with the implementation by being spelled to match it.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::error_message;
use super::repository_image_lifecycle::{call, ok, refused, service, ARN_PREFIX};

/// The fixed text ECR Public answers for a tag key in the namespace it keeps for its own
/// tags. Spelled out here rather than imported from the code under test.
const SYSTEM_TAG_MESSAGE: &str = "Caller is an end user and not allowed to mutate system tags";

/// The clause a tagging write answers for a `resourceArn` it cannot parse.
const UNPARSEABLE_ARN_MESSAGE: &str =
    "Invalid parameter at 'resourceArn' failed to satisfy constraint: 'Unable to parse ARN'";

/// The most tags a repository may carry: the limit `TooManyTagsException` documents ("the
/// maximum number of tags that can be applied to a repository is 50").
const MAX_TAGS: usize = 50;

/// A `tags` list, as a client sends it.
fn tags(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// The tag set `ListTagsForResource` reports for `arn`, as `(Key, Value)` pairs in the
/// order the response listed them.
async fn listed(router: &MultiServiceRouter, arn: &str, case: &str) -> Vec<(String, String)> {
    let body = ok(
        router,
        "ListTagsForResource",
        json!({"resourceArn": arn}),
        case,
    )
    .await;
    body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags list in {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a tag Key in {body}"))
                    .to_string(),
                tag["Value"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a tag Value in {body}"))
                    .to_string(),
            )
        })
        .collect()
}

/// Create `repository` carrying `held` and answer its ARN, so the tags a case starts from
/// reach the store the only way a client can put them there.
async fn given_repository(
    router: &MultiServiceRouter,
    repository: &str,
    held: &[(&str, &str)],
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateRepository",
        json!({"repositoryName": repository, "tags": tags(held)}),
        case,
    )
    .await;
    created["repository"]["repositoryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a repositoryArn in {created}"))
        .to_string()
}

/// What the held set minus the named keys is: the rule's own answer, computed here rather
/// than spelled out per row.
fn remaining(held: &[(&str, &str)], removed: &[&str]) -> Vec<(String, String)> {
    let named: BTreeSet<&str> = removed.iter().copied().collect();
    let mut left: Vec<(String, String)> = held
        .iter()
        .filter(|(key, _)| !named.contains(*key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    // The listing comes back in key order, and a key held twice in a row's set-up is one
    // tag of that key with the last value.
    left.sort();
    left.dedup_by(|later, earlier| later.0 == earlier.0);
    left
}

/// One repository, the tags it was created with, and the `tagKeys` one `UntagResource`
/// call names.
struct RemovalCase {
    case: &'static str,
    repository: &'static str,
    held: Vec<(&'static str, &'static str)>,
    /// The `tagKeys` member, exactly as a client sends it — which may name a key the
    /// repository does not hold, or the same key twice.
    named: Vec<&'static str>,
}

/// The removals a single `UntagResource` call can make, spanning what the rule generalises
/// over: how many of the held keys are named (none, one, several, all), a named key the
/// repository never held, a request that names only keys it never held, a key named twice,
/// keys sent out of key order, keys and values outside ASCII, and the repository-name forms
/// the modelled pattern admits (flat, dotted, namespaced).
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "an empty key list removes nothing",
            repository: "emptylist",
            held: vec![("env", "prod"), ("owner", "platform")],
            named: Vec::new(),
        },
        RemovalCase {
            case: "one of several keys",
            repository: "team/app",
            held: vec![("env", "prod"), ("owner", "platform"), ("tier", "1")],
            named: vec!["owner"],
        },
        RemovalCase {
            case: "several keys at once, named out of key order",
            repository: "deep/nested/name",
            held: vec![
                ("Alpha", "upper"),
                ("env", "prod"),
                ("middle", "mid"),
                ("zebra", "last"),
            ],
            named: vec!["zebra", "Alpha"],
        },
        RemovalCase {
            case: "every key the repository holds",
            repository: "allofthem",
            held: vec![("env", "prod"), ("owner", "platform")],
            named: vec!["env", "owner"],
        },
        RemovalCase {
            case: "the only key of a single-tag repository",
            repository: "single.tag_name-1",
            held: vec![("env", "prod")],
            named: vec!["env"],
        },
        RemovalCase {
            case: "a key the repository never held",
            repository: "neverheld",
            held: vec![("env", "prod")],
            named: vec!["owner"],
        },
        RemovalCase {
            case: "only keys the repository never held",
            repository: "nonematch",
            held: vec![("env", "prod"), ("owner", "platform")],
            named: vec!["tier", "cost-center"],
        },
        RemovalCase {
            case: "a key named twice in one request",
            repository: "repeatedkey",
            held: vec![("env", "prod"), ("owner", "platform")],
            named: vec!["env", "env"],
        },
        RemovalCase {
            case: "a held key beside one never held",
            repository: "team/mixed",
            held: vec![("env", "prod"), ("owner", "platform")],
            named: vec!["owner", "absent"],
        },
        RemovalCase {
            case: "an untagged repository",
            repository: "untagged",
            held: Vec::new(),
            named: vec!["env"],
        },
        RemovalCase {
            case: "a key whose value is empty",
            repository: "emptyvalue",
            held: vec![("marker", ""), ("env", "prod")],
            named: vec!["marker"],
        },
        RemovalCase {
            case: "keys and values outside ASCII",
            repository: "unicode",
            held: vec![
                ("cost center", "platform"),
                ("proyecto", "año-2026"),
                ("チーム", "基盤"),
            ],
            named: vec!["チーム", "cost center"],
        },
        RemovalCase {
            case: "a key that only resembles the reserved namespace",
            repository: "resembles",
            // The reserved prefix is the literal `aws:` at the start of the key, so these
            // are ordinary keys a caller owns and may remove.
            held: vec![("AWS:team", "upper"), ("awsteam", "no-colon"), ("env", "x")],
            named: vec!["AWS:team", "awsteam"],
        },
    ]
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_the_request_names() {
    let router = service();

    for case in removal_cases() {
        let arn = given_repository(&router, case.repository, &case.held, case.case).await;
        let before = listed(&router, &arn, case.case).await;
        assert_eq!(
            before,
            remaining(&case.held, &[]),
            "case {:?}: expected the created tags before the removal, got {before:?}",
            case.case
        );

        let answered = ok(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": case.named}),
            case.case,
        )
        .await;
        // `UntagResourceResponse` models no members: the 200 itself is the whole answer.
        assert_eq!(
            answered,
            json!({}),
            "case {:?}: expected an empty response body, got {answered}",
            case.case
        );

        let expected = remaining(&case.held, &case.named);
        let actual = listed(&router, &arn, case.case).await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            case.case
        );

        // Asking for the same removal again is the same single removal: a key that is
        // already gone takes nothing else with it.
        ok(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": case.named}),
            case.case,
        )
        .await;
        let again = listed(&router, &arn, case.case).await;
        assert_eq!(
            again, expected,
            "case {:?}: expected a repeated removal to leave {expected:?}, got {again:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn a_removal_and_a_write_compose_over_the_same_repository() {
    let router = service();
    let arn = given_repository(
        &router,
        "lifecycle/target",
        &[("created", "by-create"), ("env", "staging")],
        "setup",
    )
    .await;

    // The two writes are each other's inverse over one key: a key removed is gone until it
    // is tagged again, and tagging it again does not bring back a different key's tag.
    ok(
        &router,
        "UntagResource",
        json!({"resourceArn": arn, "tagKeys": ["env"]}),
        "remove env",
    )
    .await;
    let after_removal = listed(&router, &arn, "remove env").await;
    assert_eq!(
        after_removal,
        vec![("created".to_string(), "by-create".to_string())],
        "case \"remove env\": expected the other tag alone, got {after_removal:?}"
    );

    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&[("env", "prod")])}),
        "tag env again",
    )
    .await;
    let after_write = listed(&router, &arn, "tag env again").await;
    assert_eq!(
        after_write,
        vec![
            ("created".to_string(), "by-create".to_string()),
            ("env".to_string(), "prod".to_string()),
        ],
        "case \"tag env again\": expected the re-added tag beside the other, \
         got {after_write:?}"
    );

    // A removal touches only the repository its ARN names: another repository's identical
    // key is left alone.
    let other = given_repository(&router, "lifecycle/other", &[("env", "prod")], "other").await;
    ok(
        &router,
        "UntagResource",
        json!({"resourceArn": arn, "tagKeys": ["env", "created"]}),
        "remove both",
    )
    .await;
    let emptied = listed(&router, &arn, "remove both").await;
    assert!(
        emptied.is_empty(),
        "case \"remove both\": expected no tags left, got {emptied:?}"
    );
    let untouched = listed(&router, &other, "remove both").await;
    assert_eq!(
        untouched,
        vec![("env".to_string(), "prod".to_string())],
        "case \"remove both\": expected the other repository's tag untouched, \
         got {untouched:?}"
    );
}

/// A tag set of `count` pairs, keyed so that key order is not insertion order.
fn filler_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .rev()
        .map(|i| (format!("key-{i:03}"), format!("value-{i}")))
        .collect()
}

#[tokio::test]
async fn a_repository_at_the_tag_limit_can_be_emptied_a_slice_at_a_time() {
    let router = service();
    let arn = given_repository(&router, "limit/removal", &[], "setup").await;

    // The published bound itself: a repository carrying the most tags it may carry (50).
    let held = filler_tags(MAX_TAGS);
    let sent: Vec<(&str, &str)> = held
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&sent)}),
        "fill to the limit",
    )
    .await;
    let at_limit = listed(&router, &arn, "fill to the limit").await;
    assert_eq!(
        at_limit.len(),
        MAX_TAGS,
        "case \"fill to the limit\": expected {MAX_TAGS} tags, got {}",
        at_limit.len()
    );

    // Removing them in slices leaves exactly the tags no slice has named yet, and the
    // last slice empties the repository.
    let mut expected: Vec<(String, String)> = at_limit.clone();
    for (case, slice) in [
        ("the first ten keys", 0..10),
        ("the next thirty-nine", 10..49),
        ("the last one", 49..MAX_TAGS),
    ] {
        let named: Vec<String> = held[slice].iter().map(|(key, _)| key.clone()).collect();
        ok(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": named}),
            case,
        )
        .await;
        expected.retain(|(key, _)| !named.contains(key));
        let actual = listed(&router, &arn, case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
    let emptied = listed(&router, &arn, "after every slice").await;
    assert!(
        emptied.is_empty(),
        "case \"after every slice\": expected no tags left, got {emptied:?}"
    );

    // And the emptied repository can be filled to the limit again: the removals gave the
    // room back rather than leaving the count where it was.
    ok(
        &router,
        "TagResource",
        json!({"resourceArn": arn, "tags": tags(&sent)}),
        "fill it again",
    )
    .await;
    let refilled = listed(&router, &arn, "fill it again").await;
    assert_eq!(
        refilled.len(),
        MAX_TAGS,
        "case \"fill it again\": expected {MAX_TAGS} tags again, got {}",
        refilled.len()
    );
}

#[tokio::test]
async fn a_system_tag_key_is_not_one_a_caller_may_remove() {
    let router = service();
    let arn = given_repository(&router, "system/untag", &[("env", "prod")], "setup").await;

    // The reserved namespace is `aws:` exactly: the rows span the prefix on its own, the
    // prefix with a tail, and a key whose further segments are colon-separated too.
    for (case, key) in [
        ("the reserved prefix alone", "aws:"),
        ("a key under the reserved prefix", "aws:cost-center"),
        (
            "a key under a reserved sub-namespace",
            "aws:cloudformation:stack-name",
        ),
    ] {
        let body = refused(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": [key]}),
            "InvalidTagParameterException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message, SYSTEM_TAG_MESSAGE,
            "case {case:?}: expected the system-tag refusal, got {message:?}"
        );
    }

    // A refused call removes nothing, not even the key beside the reserved one that it
    // could have removed: the whole list is refused together.
    let body = refused(
        &router,
        "UntagResource",
        json!({"resourceArn": arn, "tagKeys": ["env", "aws:internal"]}),
        "InvalidTagParameterException",
        "a reserved key beside an ordinary one",
    )
    .await;
    let message = error_message(&body);
    assert_eq!(
        message, SYSTEM_TAG_MESSAGE,
        "case \"a reserved key beside an ordinary one\": expected the system-tag refusal, \
         got {message:?}"
    );
    let actual = listed(&router, &arn, "after the refusals").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the refusals\": expected the repository's tags to be untouched, \
         got {actual:?}"
    );
}

#[tokio::test]
async fn a_resource_arn_a_removal_cannot_parse_is_an_invalid_parameter() {
    let router = service();
    given_repository(&router, "team/app", &[("env", "prod")], "setup").await;

    // Every way the six-field ARN form
    // `arn:<partition>:ecr-public:<region>:<account>:repository/<name>` can be missed. The
    // repository named by the later rows DOES exist, so only the ARN's form refuses them.
    for (case, arn) in [
        ("a bare repository name", "team/app".to_string()),
        (
            "a URI instead of an ARN",
            "public.ecr.aws/emulator/team/app".to_string(),
        ),
        ("too few fields", "arn:aws:ecr-public::000000000000".to_string()),
        ("no resource field", "arn:aws:ecr-public::000000000000:".to_string()),
        ("a resource type with no name", ARN_PREFIX.to_string()),
        (
            "a resource type that is not repository",
            "arn:aws:ecr-public::000000000000:registry/team/app".to_string(),
        ),
        (
            "the private ECR service",
            "arn:aws:ecr:us-east-1:000000000000:repository/team/app".to_string(),
        ),
        ("another service entirely", "arn:aws:s3:::team/app".to_string()),
        (
            "an empty partition",
            "arn::ecr-public::000000000000:repository/team/app".to_string(),
        ),
        (
            "an empty account",
            "arn:aws:ecr-public:::repository/team/app".to_string(),
        ),
        (
            "the arn prefix miscased",
            format!("ARN{}", &format!("{ARN_PREFIX}team/app")[3..]),
        ),
        ("leading whitespace", format!(" {ARN_PREFIX}team/app")),
    ] {
        let body = refused(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": ["env"]}),
            "InvalidParameterException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert_eq!(
            message, UNPARSEABLE_ARN_MESSAGE,
            "case {case:?}: expected the resourceArn clause, got {message:?}"
        );
    }

    // None of them removed the tag of the repository that does exist.
    let actual = listed(&router, &format!("{ARN_PREFIX}team/app"), "after the refusals").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the refusals\": expected the created tag alone, got {actual:?}"
    );
}

#[tokio::test]
async fn a_well_formed_arn_naming_no_repository_of_this_registry_is_not_found() {
    let router = service();
    given_repository(&router, "team/app", &[("env", "prod")], "setup").await;

    for (case, arn) in [
        (
            "a name that was never created",
            format!("{ARN_PREFIX}team/missing"),
        ),
        ("a prefix of a stored name", format!("{ARN_PREFIX}team")),
        (
            "another account's registry",
            "arn:aws:ecr-public::123456789012:repository/team/app".to_string(),
        ),
        (
            "another partition",
            "arn:aws-cn:ecr-public::000000000000:repository/team/app".to_string(),
        ),
        (
            "the registry alias in the account field",
            "arn:aws:ecr-public::emulator:repository/team/app".to_string(),
        ),
    ] {
        let body = refused(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": ["env"]}),
            "RepositoryNotFoundException",
            case,
        )
        .await;
        let message = error_message(&body);
        assert!(
            message.contains("does not exist in the registry"),
            "case {case:?}: expected the missing-repository refusal, got {message:?}"
        );
    }

    // The tags of a deleted repository go with it, so a removal against the name after the
    // delete names no repository at all.
    let arn = format!("{ARN_PREFIX}team/app");
    ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "team/app"}),
        "delete",
    )
    .await;
    refused(
        &router,
        "UntagResource",
        json!({"resourceArn": arn, "tagKeys": ["env"]}),
        "RepositoryNotFoundException",
        "after the delete",
    )
    .await;
}

#[tokio::test]
async fn a_tag_key_outside_the_modelled_length_is_refused_before_anything_is_removed() {
    let router = service();
    let arn = given_repository(&router, "keylength", &[("env", "prod")], "setup").await;

    // `TagKey` is modelled `@length(1..128)` and `TagKeyList` `@length(0..200)`, so the
    // bounds are refused by the framework before the handler runs: the empty key, a key one
    // byte past the maximum, and a list one entry past its maximum.
    let too_long = "k".repeat(129);
    let too_many: Vec<String> = (0..201).map(|i| format!("key-{i}")).collect();
    for (case, keys) in [
        ("an empty key", json!([""])),
        ("a key one byte past the maximum", json!([too_long])),
        ("a list one entry past its maximum", json!(too_many)),
    ] {
        let (status, body) = call(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": keys}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled bound to refuse the request, \
             got {status} with {body}"
        );
    }

    // A key AT each bound is accepted: the longest key the model admits, and a list of the
    // most entries it admits.
    let at_max = "k".repeat(128);
    let max_list: Vec<String> = (0..200).map(|i| format!("key-{i}")).collect();
    for (case, keys) in [
        ("a key at the maximum length", json!([at_max])),
        ("a list at its maximum length", json!(max_list)),
    ] {
        ok(
            &router,
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": keys}),
            case,
        )
        .await;
    }

    // None of those keys was one the repository held, so its tag survived every call.
    let actual = listed(&router, &arn, "after the bound cases").await;
    assert_eq!(
        actual,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after the bound cases\": expected the created tag alone, got {actual:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `TagPolicy`, end to end over the awsQuery wire, read back through `GetPolicy`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the tag set of the
//! resource its identifier names — a supplied key the resource already holds has its
//! value overwritten and a key it does not hold is added — the merged set is bounded
//! by the per-resource tag quota, and a request carrying any invalid tag, or naming
//! no resource of the account, stores nothing. `TagPolicy`'s own documentation: "if a
//! tag with the same key name already exists, then that tag is overwritten with the
//! new value", and "if any one of the tags is invalid or if you exceed the allowed
//! maximum number of tags, then the entire request fails".
//!
//! A managed policy is identified by its ARN rather than by a name, so the suite also
//! asserts that the write is addressed exactly as a read of the same ARN is: a value
//! that is no managed-policy ARN at all is refused as an invalid parameter, an ARN in
//! the account's namespace that names no held policy is the missing entity, and an ARN
//! in AWS's own namespace names a policy the account cannot change.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour of a policy that was PERSISTED rather than a handler called
//! directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and
//! the parameter and tag readers are `tests/all/role_lifecycle.rs`'s, so a request is
//! built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the
//! held tag count, the number of calls, whether a key is new or held, the case of a
//! key, the policy's path, repeated keys inside one request, and the published length
//! bounds of `tagKeyType` (1..128) and `tagValueType` (0..256).

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
// The awsQuery tag-list encoding and the tag-table helpers have ONE definition in
// this crate's test tree, the one `tests/all/role_tags.rs` owns, so every tag suite
// builds a request the same way.
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// documentation describes. Every policy in this suite is created with it; none of
/// these assertions is about the document.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]}"#;

/// Create the policy `policy_name` under `path` carrying `initial`, asserting the
/// create succeeded, and answer the ARN it reported.
pub(crate) async fn create_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    path: Option<&str>,
    initial: &[(String, String)],
) -> String {
    let mut leading = vec![
        param("PolicyName", policy_name),
        param("PolicyDocument", POLICY_DOCUMENT),
    ];
    if let Some(path) = path {
        leading.push(param("Path", path));
    }
    let params = with_tag_params(leading, initial);
    let (status, body) = call(router, "CreatePolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One `TagPolicy` call: the HTTP status and the XML body.
pub(crate) async fn tag_policy(
    router: &MultiServiceRouter,
    policy_arn: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("PolicyArn", policy_arn)], list);
    call(router, "TagPolicy", &borrowed(&params)).await
}

/// The tag set `GetPolicy` reports for `policy_arn`, as a map, plus how many `<Key>`
/// elements the response carried — so a duplicated key cannot hide inside a map.
pub(crate) async fn stored_tags(
    router: &MultiServiceRouter,
    policy_arn: &str,
) -> (BTreeMap<String, String>, usize) {
    let (status, body) = call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must report the tagged policy, got {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key").len())
}

/// A managed policy ARN in the ACCOUNT's namespace — `arn:aws:iam::<account>:policy<path><name>`,
/// IAM being global, so the region segment is empty.
pub(crate) fn account_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// One row: a policy created under `path` with `initial`, then `calls` successive
/// `TagPolicy` requests, then the tag set the policy must hold.
struct MergeCase {
    name: &'static str,
    path: Option<&'static str>,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagPolicy` merges its tags into the policy's stored set — a held key is
/// overwritten, a new key is added — and `GetPolicy` reports exactly that merged set.
/// One row per thing the rule generalises over: the policy's path, the held tag
/// count, the number of calls, new versus held keys, the case of a key, a key
/// repeated inside one request, and the published length bounds of `tagKeyType` and
/// `tagValueType`.
#[tokio::test]
async fn tag_policy_merges_its_tags_into_the_stored_set() {
    // `tagValueType` is length 0..256 and `tagKeyType` length 1..128
    // (`smithy_query(query="shape", shape="Tag")`), so these are the published bounds
    // a stored tag must survive.
    let longest_value = "v".repeat(256);
    let longest_key = "k".repeat(128);

    let cases = vec![
        MergeCase {
            name: "a policy holding no tags gains one",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("team", "storage")])],
            expected: tags(&[("team", "storage")]),
        },
        MergeCase {
            name: "a new key is added beside the held ones",
            path: None,
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("owner", "dlm")])],
            expected: tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]),
        },
        MergeCase {
            name: "a held key is overwritten rather than duplicated",
            path: None,
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("env", "staging")])],
            expected: tags(&[("team", "storage"), ("env", "staging")]),
        },
        MergeCase {
            name: "one call both overwrites and adds",
            path: Some("/one-segment/"),
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![tags(&[("b", "two"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "two"), ("c", "3")]),
        },
        MergeCase {
            name: "successive calls accumulate, the last value winning",
            path: Some("/a/b/c/"),
            initial: Vec::new(),
            calls: vec![
                tags(&[("k1", "v1")]),
                tags(&[("k2", "v2")]),
                tags(&[("k1", "v1-again")]),
            ],
            expected: tags(&[("k1", "v1-again"), ("k2", "v2")]),
        },
        MergeCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM
            // User Guide), so two spellings of one word are two tags.
            name: "keys differing only in case are distinct tags",
            path: Some("/"),
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev"), ("ENV", "test")])],
            expected: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
        },
        MergeCase {
            name: "a key repeated inside one request settles on its last value",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("dup", "first"), ("dup", "last")])],
            expected: tags(&[("dup", "last")]),
        },
        MergeCase {
            name: "a value at the published minimum length",
            path: None,
            initial: tags(&[("team", "storage")]),
            calls: vec![tags(&[("empty", "")])],
            expected: tags(&[("team", "storage"), ("empty", "")]),
        },
        MergeCase {
            name: "a held value overwritten with the empty value",
            path: None,
            initial: tags(&[("env", "prod")]),
            calls: vec![tags(&[("env", "")])],
            expected: tags(&[("env", "")]),
        },
        MergeCase {
            name: "a value at the published maximum length",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("long", longest_value.as_str())])],
            expected: tags(&[("long", longest_value.as_str())]),
        },
        MergeCase {
            name: "a key at the published maximum length",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[(longest_key.as_str(), "fits")])],
            expected: tags(&[(longest_key.as_str(), "fits")]),
        },
        MergeCase {
            // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` and
            // `tagValueType`'s the same with `*`, so spaces, non-ASCII letters, digits
            // and every punctuation character of that class belong to the class the
            // rule covers.
            name: "a key and a value spanning the modelled alphabet",
            path: None,
            initial: Vec::new(),
            calls: vec![tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")])],
            expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")]),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let policy_name = format!("merge-{index}");
        let arn = create_policy(&router, &policy_name, case.path, &case.initial).await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_policy(&router, &arn, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagPolicy call {call_index} must succeed, got {status}: {body}",
                case.name
            );
        }

        let (reported, keys) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported,
            expected_map(&case.expected),
            "case {:?}: expected the merged tag set {:?}, got {reported:?}",
            case.name,
            expected_map(&case.expected)
        );
        assert_eq!(
            keys,
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {keys}",
            case.name,
            case.expected.len()
        );
    }
}

/// RULE: a tag call on a managed policy addresses it by the policy's own ARN,
/// exactly as a read of that policy does — so the ARN the create reported tags the
/// held policy, a value that is no managed-policy ARN at all is refused as an
/// invalid input parameter, and an ARN in the account's namespace that names no held
/// policy is the missing entity. Neither refusal stores anything.
#[tokio::test]
async fn the_policy_is_addressed_by_its_own_arn() {
    let router = router();
    let held = tags(&[("team", "storage")]);
    let held_arn = create_policy(&router, "held", Some("/held/"), &held).await;
    assert_eq!(
        held_arn,
        account_policy_arn("/held/", "held"),
        "the fixture's ARN must be the account's managed-policy ARN, got {held_arn:?}"
    );

    // A value that cannot be used as a managed-policy ARN: `arnType` constrains only the
    // length, so each row is 20 characters or longer and still names no policy. The code
    // is the layer that took the form — `ValidationError` for a value that is not an ARN
    // at all, the modelled `InvalidInput` for an ARN naming another service or resource
    // type.
    for (case, arn, expected_code) in [
        (
            "a bare string of the right length",
            "not-an-arn-at-all-xx".to_string(),
            "ValidationError",
        ),
        (
            "a role ARN",
            format!("arn:aws:iam::{}:role/some-role", account_id()),
            "InvalidInput",
        ),
        (
            "a user ARN",
            format!("arn:aws:iam::{}:user/some-user", account_id()),
            "InvalidInput",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket-name".to_string(),
            "InvalidInput",
        ),
        (
            "an IAM policy ARN carrying a region",
            format!("arn:aws:iam:us-west-2:{}:policy/held", account_id()),
            "InvalidInput",
        ),
        (
            "a policy ARN with no partition",
            format!("arn::iam::{}:policy/held", account_id()),
            "InvalidInput",
        ),
    ] {
        let (status, body) = tag_policy(&router, &arn, &tags(&[("env", "prod")])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
    }

    // An ARN in the account's own namespace that names no held policy: another name,
    // the held name under another path, and the held name in another case — a policy's
    // ARN is the identifier, so a different spelling of it names a different policy.
    for (case, arn) in [
        ("another name at the root", account_policy_arn("/", "absent")),
        ("the held name at the root", account_policy_arn("/", "held")),
        (
            "the held name under another path",
            account_policy_arn("/elsewhere/", "held"),
        ),
        ("a name under a nested path", account_policy_arn("/a/b/c/", "absent")),
        ("the held ARN in another case", account_policy_arn("/held/", "HELD")),
    ] {
        let (status, body) = tag_policy(&router, &arn, &tags(&[("env", "prod")])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for the unheld ARN {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // Neither refusal touched the held policy.
    let (reported, keys) = stored_tags(&router, &held_arn).await;
    assert_eq!(
        reported,
        expected_map(&held),
        "a refused request must store nothing, got {reported:?}"
    );
    assert_eq!(
        keys,
        held.len(),
        "expected {} stored tags, got {keys}",
        held.len()
    );
}

/// RULE: this operation tags "an IAM customer managed policy", so an ARN in AWS's own
/// managed-policy namespace (`arn:aws:iam::aws:policy/...`) names a policy the
/// account cannot change and is refused with the modelled `InvalidInputException` —
/// whether or not this emulator's catalog carries that entry, because no entry of it
/// could be tagged either.
#[tokio::test]
async fn an_aws_managed_policy_cannot_be_tagged() {
    let router = router();

    // SOURCE: the catalog this crate carries (`state::AWS_MANAGED_POLICIES`), which
    // records the AWS-managed policies the emulator serves; every entry is a row, so
    // no single one is the specification.
    let mut arns: Vec<(String, String)> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| (format!("the catalog entry {}", entry.policy_name), entry.arn()))
        .collect();
    // An ARN in AWS's namespace the catalog does NOT carry is refused the same way: the
    // account could not tag it even if the catalog grew to hold it.
    arns.push((
        "an AWS-managed ARN the catalog does not carry".to_string(),
        "arn:aws:iam::aws:policy/NoSuchAwsManagedPolicyHere".to_string(),
    ));
    arns.push((
        "an AWS-managed ARN under a path the catalog does not carry".to_string(),
        "arn:aws:iam::aws:policy/job-function/NoSuchAwsManagedPolicyHere".to_string(),
    ));
    assert!(
        arns.len() >= 3,
        "the catalog must carry entries for this suite to span, got {} rows",
        arns.len()
    );

    for (case, arn) in &arns {
        let (status, body) = tag_policy(&router, arn, &tags(&[("team", "storage")])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 tagging the AWS-owned {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refusal stored nothing. A catalog entry reads back carrying no tags of
        // its own; an ARN in AWS's namespace the catalog does not carry resolves to no
        // policy, so that read is the modelled `NoSuchEntity` rather than a 200.
        let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", arn)]).await;
        if case.starts_with("the catalog entry") {
            assert_eq!(
                status, 200,
                "case {case:?}: GetPolicy must report a carried catalog entry, got {status}: {read}"
            );
            let reported = reported_tags(&read);
            assert!(
                reported.is_empty(),
                "case {case:?}: a refused request must store nothing, got {reported:?}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: an ARN resolving to no policy is the modelled missing entity, got {status}: {read}"
            );
            assert_eq!(
                error_code(&read),
                "NoSuchEntity",
                "case {case:?}: expected the modelled awsQuery code, got {read}"
            );
        }
    }
}

/// RULE: the `aws:` tag-key prefix is reserved ("You cannot create tag keys that
/// begin with `aws:`", "Tagging IAM resources"), so a request naming one is refused
/// with the modelled `InvalidInputException` and the ENTIRE request is dropped — no
/// tag of it reaches the policy.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_stores_nothing() {
    let held = tags(&[("team", "storage")]);
    let refused = [
        ("the prefix alone", tags(&[("aws:", "x")])),
        (
            "a reserved key",
            tags(&[("aws:cloudformation:stack-name", "stack")]),
        ),
        // The prefix is reserved however it is capitalised.
        ("an upper-case prefix", tags(&[("AWS:Owner", "aws")])),
        ("a mixed-case prefix", tags(&[("Aws:Owner", "aws")])),
        (
            "a valid tag beside a reserved one",
            tags(&[("owner", "dlm"), ("aws:service", "dlm")]),
        ),
        (
            "a reserved key before a valid one",
            tags(&[("aws:service", "dlm"), ("owner", "dlm")]),
        ),
    ];

    let router = router();
    for (index, (case, list)) in refused.iter().enumerate() {
        let policy_name = format!("reserved-{index}");
        let arn = create_policy(&router, &policy_name, None, &held).await;

        let (status, body) = tag_policy(&router, &arn, list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a reserved tag key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The whole request failed, so the policy still holds exactly what it was
        // created with: neither the reserved key nor the valid one beside it.
        let (reported, keys) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported,
            expected_map(&held),
            "case {case:?}: a refused request must store nothing, got {reported:?}"
        );
        assert_eq!(
            keys,
            held.len(),
            "case {case:?}: expected {} stored tags, got {keys}",
            held.len()
        );
    }
}

/// RULE: a resource carries at most [`state::MAX_TAGS_PER_ENTITY`] tags (AWS's "IAM
/// and STS quotas": "Number of tags per IAM resource: 50"), and the bound is taken
/// against the MERGED set — so overwriting a held key is never refused, while a call
/// that would carry the resource past the bound is refused with the modelled
/// `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_tag_bound_is_taken_against_the_merged_set() {
    let bound = state::MAX_TAGS_PER_ENTITY;
    let fixture = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect()
    };

    // One row per position relative to the published bound: just inside it, at it, and
    // the first call that would pass it.
    let accepted = [
        ("one below the bound, one new key", bound - 1, fixture(1)),
        (
            "two below the bound, two new keys",
            bound - 2,
            vec![
                ("new-a".to_string(), "a".to_string()),
                ("new-b".to_string(), "b".to_string()),
            ],
        ),
        (
            "at the bound, overwriting a held key",
            bound,
            vec![("key-00".to_string(), "overwritten".to_string())],
        ),
        (
            "at the bound, overwriting several held keys",
            bound,
            vec![
                ("key-01".to_string(), "one".to_string()),
                ("key-02".to_string(), "two".to_string()),
            ],
        ),
    ];

    let router = router();
    for (index, (case, held_count, list)) in accepted.iter().enumerate() {
        let policy_name = format!("bound-ok-{index}");
        let held = fixture(*held_count);
        let arn = create_policy(&router, &policy_name, None, &held).await;

        let (status, body) = tag_policy(&router, &arn, list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 inside the tag bound, got {status}: {body}"
        );

        let (reported, keys) = stored_tags(&router, &arn).await;
        let mut expected: BTreeMap<String, String> = expected_map(&held);
        expected.extend(list.iter().cloned());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the merged tag set {expected:?}, got {reported:?}"
        );
        assert!(
            keys <= bound,
            "case {case:?}: a policy must never hold more than {bound} tags, got {keys}"
        );
    }

    let refused = [
        ("at the bound, one new key", bound, 1usize),
        ("one below the bound, two new keys", bound - 1, 2),
        ("two below the bound, three new keys", bound - 2, 3),
    ];

    for (index, (case, held_count, new_keys)) in refused.iter().enumerate() {
        let policy_name = format!("bound-refused-{index}");
        let held = fixture(*held_count);
        let arn = create_policy(&router, &policy_name, None, &held).await;

        let list: Vec<(String, String)> = (0..*new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, body) = tag_policy(&router, &arn, &list).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 past the tag bound, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceeded",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // Nothing of the refused request was stored.
        let (reported, keys) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported,
            expected_map(&held),
            "case {case:?}: a refused request must store nothing, got {reported:?}"
        );
        assert_eq!(
            keys, *held_count,
            "case {case:?}: expected {held_count} stored tags, got {keys}"
        );
    }
}

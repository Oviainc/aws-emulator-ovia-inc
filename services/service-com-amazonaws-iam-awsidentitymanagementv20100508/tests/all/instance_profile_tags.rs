// SPDX-License-Identifier: CC0-1.0

//! `TagInstanceProfile`, end to end over the awsQuery wire, read back through
//! `GetInstanceProfile`.
//!
//! RULE under test: a tag call MERGES the supplied tags into the resource's stored
//! tag set — a supplied key the resource already holds has its value overwritten
//! and a key it does not hold is added — the merged set is bounded by the
//! per-resource tag quota, the resource is addressed by its case-folded IAM entity
//! name, and a request carrying any invalid tag stores nothing.
//! `TagInstanceProfile`'s own documentation: "if a tag with the same key name
//! already exists, then that tag is overwritten with the new value", and "if any one
//! of the tags is invalid or if you exceed the allowed maximum number of tags, then
//! the entire request fails".
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour of an instance profile that was PERSISTED rather than
//! a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, and the parameter and tag readers are
//! `tests/all/role_lifecycle.rs`'s, so a request is built and read exactly one way
//! across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over:
//! the held tag count, the number of calls, whether a key is new or held, the case
//! of a key, the case of the profile name, and the published length bounds of
//! `tagKeyType` (1..128) and `tagValueType` (0..256). What a key REPEATED inside one
//! request settles on is the shared merge helper's own rule, not this operation's, so
//! it is deliberately not pinned here.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{call, error_code, elements, router};

/// Own one row's tag list, which a table row builds from literals.
fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The `Tags.member.N.Key`/`.Value` parameters of an awsQuery tag list, after the
/// parameters `leading` carries.
fn with_tag_params(
    leading: Vec<(String, String)>,
    list: &[(String, String)],
) -> Vec<(String, String)> {
    let mut params = leading;
    for (index, (key, value)) in list.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    params
}

/// Create the instance profile `profile_name` carrying `initial`, asserting the
/// create succeeded.
async fn create_instance_profile(
    router: &MultiServiceRouter,
    profile_name: &str,
    initial: &[(String, String)],
) {
    let params = with_tag_params(vec![param("InstanceProfileName", profile_name)], initial);
    let (status, body) = call(router, "CreateInstanceProfile", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {profile_name:?}: the fixture CreateInstanceProfile must succeed, got \
         {status}: {body}"
    );
}

/// One `TagInstanceProfile` call: the HTTP status and the XML body.
async fn tag_instance_profile(
    router: &MultiServiceRouter,
    profile_name: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("InstanceProfileName", profile_name)], list);
    call(router, "TagInstanceProfile", &borrowed(&params)).await
}

/// The tag set `GetInstanceProfile` reports for `profile_name`, as a map, plus how
/// many `<Key>` elements the response carried — so a duplicated key cannot hide
/// inside a map.
async fn stored_tags(
    router: &MultiServiceRouter,
    profile_name: &str,
) -> (BTreeMap<String, String>, usize) {
    let (status, body) = call(
        router,
        "GetInstanceProfile",
        &[("InstanceProfileName", profile_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {profile_name:?}: GetInstanceProfile must report the tagged profile, got \
         {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key").len())
}

/// `expected` as the map `GetInstanceProfile` must report.
fn expected_map(expected: &[(String, String)]) -> BTreeMap<String, String> {
    expected.iter().cloned().collect()
}

/// One row: a profile created with `initial`, then `calls` successive
/// `TagInstanceProfile` requests, then the tag set the profile must hold.
struct MergeCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    calls: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// RULE: `TagInstanceProfile` merges its tags into the profile's stored set — a held
/// key is overwritten, a new key is added — and `GetInstanceProfile` reports exactly
/// that merged set. One row per thing the rule generalises over: the held tag count,
/// the number of calls, new versus held keys, the case of a key, and the published
/// length bounds of `tagKeyType` and `tagValueType`.
#[tokio::test]
async fn tag_instance_profile_merges_its_tags_into_the_stored_set() {
    // `tagValueType` is length 0..256 and `tagKeyType` length 1..128
    // (`smithy_query(query="shape", shape="Tag")`), so these are the published
    // bounds a stored tag must survive.
    let longest_value = "v".repeat(256);
    let longest_key = "k".repeat(128);

    let cases = vec![
        MergeCase {
            name: "a profile holding no tags gains one",
            initial: Vec::new(),
            calls: vec![tags(&[("team", "storage")])],
            expected: tags(&[("team", "storage")]),
        },
        MergeCase {
            name: "a new key is added beside the held ones",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("owner", "ec2")])],
            expected: tags(&[("team", "storage"), ("env", "prod"), ("owner", "ec2")]),
        },
        MergeCase {
            name: "a held key is overwritten rather than duplicated",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            calls: vec![tags(&[("env", "staging")])],
            expected: tags(&[("team", "storage"), ("env", "staging")]),
        },
        MergeCase {
            name: "one call both overwrites and adds",
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![tags(&[("b", "two"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "two"), ("c", "3")]),
        },
        MergeCase {
            name: "successive calls accumulate, the last value winning",
            initial: Vec::new(),
            calls: vec![
                tags(&[("k1", "v1")]),
                tags(&[("k2", "v2")]),
                tags(&[("k1", "v1-again")]),
            ],
            expected: tags(&[("k1", "v1-again"), ("k2", "v2")]),
        },
        MergeCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources",
            // IAM User Guide), so two spellings of one word are two tags.
            name: "keys differing only in case are distinct tags",
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev"), ("ENV", "test")])],
            expected: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
        },
        MergeCase {
            name: "a value at the published minimum length",
            initial: tags(&[("team", "storage")]),
            calls: vec![tags(&[("empty", "")])],
            expected: tags(&[("team", "storage"), ("empty", "")]),
        },
        MergeCase {
            name: "a held value overwritten with the empty value",
            initial: tags(&[("env", "prod")]),
            calls: vec![tags(&[("env", "")])],
            expected: tags(&[("env", "")]),
        },
        MergeCase {
            name: "a value at the published maximum length",
            initial: Vec::new(),
            calls: vec![tags(&[("long", longest_value.as_str())])],
            expected: tags(&[("long", longest_value.as_str())]),
        },
        MergeCase {
            name: "a key at the published maximum length",
            initial: Vec::new(),
            calls: vec![tags(&[(longest_key.as_str(), "fits")])],
            expected: tags(&[(longest_key.as_str(), "fits")]),
        },
        MergeCase {
            // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` and
            // `tagValueType`'s the same with `*`, so spaces, non-ASCII letters,
            // digits and every punctuation character of that class belong to the
            // class the rule covers.
            name: "a key and a value spanning the modelled alphabet",
            initial: Vec::new(),
            calls: vec![tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")])],
            expected: tags(&[("Cost Center_1.2:3/4=5+6-7@8", "Ünité 9 / A=B+C-D@E")]),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let profile_name = format!("merge-{index}");
        create_instance_profile(&router, &profile_name, &case.initial).await;

        for (call_index, list) in case.calls.iter().enumerate() {
            let (status, body) = tag_instance_profile(&router, &profile_name, list).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagInstanceProfile call {call_index} must succeed, got \
                 {status}: {body}",
                case.name
            );
        }

        let (reported, keys) = stored_tags(&router, &profile_name).await;
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

/// RULE: a tag call names its instance profile by an IAM entity name, which is
/// unique within the account and "not distinguished by case", so any casing of a
/// held name tags the SAME profile — and a name the account does not hold is refused
/// with the modelled `NoSuchEntityException`, which `GetInstanceProfile` reports the
/// same way.
#[tokio::test]
async fn the_instance_profile_is_found_by_its_case_folded_name() {
    let router = router();
    create_instance_profile(&router, "MyProfile", &tags(&[("team", "storage")])).await;

    // Each spelling tags the one held profile, so the set accumulates across them
    // instead of each spelling getting a profile of its own.
    for (index, spelling) in ["MyProfile", "myprofile", "MYPROFILE", "mYpRoFiLe"]
        .iter()
        .enumerate()
    {
        let added = tags(&[(&format!("key-{index}"), &format!("value-{index}"))]);
        let (status, body) = tag_instance_profile(&router, spelling, &added).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200 for a held name, got {status}: {body}"
        );
        let (reported, keys) = stored_tags(&router, spelling).await;
        assert_eq!(
            keys,
            index + 2,
            "case {spelling:?}: expected {} stored tags, got {keys}: {reported:?}",
            index + 2
        );
        assert_eq!(
            reported.get(&format!("key-{index}")).map(String::as_str),
            Some(format!("value-{index}").as_str()),
            "case {spelling:?}: the tag must be stored on the held profile, got \
             {reported:?}"
        );
    }

    // A role of that name is no instance profile, so tagging it is still refused.
    let trust_policy = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "role-without-profile"),
            ("AssumeRolePolicyDocument", trust_policy),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture CreateRole must succeed: {body}");

    for absent in [
        "OtherProfile",
        "myprofile2",
        "my_profile",
        "profile",
        "role-without-profile",
    ] {
        let (status, body) =
            tag_instance_profile(&router, absent, &tags(&[("team", "storage")])).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected 404 for a name the account does not hold, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// RULE: the `aws:` tag-key prefix is reserved ("You cannot create tag keys that
/// begin with `aws:`", "Tagging IAM resources"), so a request naming one is refused
/// with the modelled `InvalidInputException` and the ENTIRE request is dropped — no
/// tag of it reaches the profile.
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
            tags(&[("owner", "ec2"), ("aws:service", "ec2")]),
        ),
        (
            "a reserved key before a valid one",
            tags(&[("aws:service", "ec2"), ("owner", "ec2")]),
        ),
    ];

    let router = router();
    for (index, (case, list)) in refused.iter().enumerate() {
        let profile_name = format!("reserved-{index}");
        create_instance_profile(&router, &profile_name, &held).await;

        let (status, body) = tag_instance_profile(&router, &profile_name, list).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for a reserved tag key, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The whole request failed, so the profile still holds exactly what it was
        // created with: neither the reserved key nor the valid one beside it.
        let (reported, keys) = stored_tags(&router, &profile_name).await;
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

/// RULE: a resource carries at most [`MAX_TAGS_PER_ENTITY`] tags (AWS's "IAM and STS
/// quotas": "Number of tags per IAM resource: 50"), and the bound is taken against
/// the MERGED set — so overwriting a held key is never refused, while a call that
/// would carry the resource past the bound is refused with the modelled
/// `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_tag_bound_is_taken_against_the_merged_set() {
    let bound = MAX_TAGS_PER_ENTITY;
    let fixture = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
            .collect()
    };

    // One row per position relative to the published bound: just inside it, at it,
    // and the first call that would pass it.
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
        let profile_name = format!("bound-ok-{index}");
        let held = fixture(*held_count);
        create_instance_profile(&router, &profile_name, &held).await;

        let (status, body) = tag_instance_profile(&router, &profile_name, list).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 inside the tag bound, got {status}: {body}"
        );

        let (reported, keys) = stored_tags(&router, &profile_name).await;
        let mut expected: BTreeMap<String, String> = expected_map(&held);
        expected.extend(list.iter().cloned());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the merged tag set {expected:?}, got {reported:?}"
        );
        assert!(
            keys <= bound,
            "case {case:?}: a profile must never hold more than {bound} tags, got {keys}"
        );
    }

    let refused = [
        ("at the bound, one new key", bound, 1usize),
        ("one below the bound, two new keys", bound - 1, 2),
        ("two below the bound, three new keys", bound - 2, 3),
    ];

    for (index, (case, held_count, new_keys)) in refused.iter().enumerate() {
        let profile_name = format!("bound-refused-{index}");
        let held = fixture(*held_count);
        create_instance_profile(&router, &profile_name, &held).await;

        let list: Vec<(String, String)> = (0..*new_keys)
            .map(|position| (format!("new-{position}"), format!("value-{position}")))
            .collect();
        let (status, body) = tag_instance_profile(&router, &profile_name, &list).await;
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
        let (reported, keys) = stored_tags(&router, &profile_name).await;
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

/// RULE: tagging a resource changes only its tags — the identity and the role
/// binding the profile already carried are reported unchanged, and the tag call is
/// idempotent: repeating it leaves the same set.
#[tokio::test]
async fn tagging_changes_only_the_tag_set() {
    let trust_policy = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
    let router = router();
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "EMR_EC2_DefaultRole"),
            ("AssumeRolePolicyDocument", trust_policy),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture CreateRole must succeed: {body}");

    let created = with_tag_params(
        vec![
            param("InstanceProfileName", "bound-profile"),
            param("Path", "/service-role/"),
        ],
        &tags(&[("env", "prod")]),
    );
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "the fixture CreateInstanceProfile must succeed: {body}"
    );
    let (status, body) = call(
        &router,
        "AddRoleToInstanceProfile",
        &[
            ("InstanceProfileName", "bound-profile"),
            ("RoleName", "EMR_EC2_DefaultRole"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AddRoleToInstanceProfile must succeed: {body}"
    );

    let (status, before) = call(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", "bound-profile")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read before tagging must report the profile, got {status}: {before}"
    );

    let added = tags(&[("env", "staging"), ("team", "platform")]);
    for round in 0..2 {
        let (status, body) = tag_instance_profile(&router, "bound-profile", &added).await;
        assert_eq!(
            status, 200,
            "case round {round}: expected 200, got {status}: {body}"
        );
        let (reported, keys) = stored_tags(&router, "bound-profile").await;
        assert_eq!(
            reported,
            expected_map(&added),
            "case round {round}: expected the merged tag set {:?}, got {reported:?}",
            expected_map(&added)
        );
        assert_eq!(
            keys,
            added.len(),
            "case round {round}: expected {} stored tags, got {keys}",
            added.len()
        );
    }

    let (status, after) = call(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", "bound-profile")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read after tagging must report the profile, got {status}: {after}"
    );
    for member in [
        "InstanceProfileId",
        "InstanceProfileName",
        "Arn",
        "Path",
        "CreateDate",
        "RoleName",
    ] {
        let expected = elements(&before, member);
        let actual = elements(&after, member);
        assert_eq!(
            actual, expected,
            "member {member}: tagging must leave it alone, expected {expected:?}, got \
             {actual:?}"
        );
    }
}

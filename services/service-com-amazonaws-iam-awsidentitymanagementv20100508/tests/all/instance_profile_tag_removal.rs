// SPDX-License-Identifier: CC0-1.0

//! `UntagInstanceProfile`, end to end over the awsQuery wire, read back through
//! `GetInstanceProfile` and `ListInstanceProfileTags`.
//!
//! RULE under test: an untag call drops from the resource's stored tag set exactly
//! the tags whose keys it names, comparing keys exactly, and leaves every other held
//! tag with its value — so a named key the resource does not hold removes nothing and
//! the call still succeeds, which makes it idempotent, and the keys it did remove are
//! free to be tagged again. `UntagInstanceProfile`'s own documentation: "removes the
//! specified tags from the IAM instance profile", with "the tags with matching keys
//! are removed from the specified instance profile"
//! (`UntagInstanceProfile$TagKeys`). The profile is addressed by its IAM entity name,
//! which is unique within the account and not distinguished by case, and a name the
//! account holds no profile for is refused with the modelled `NoSuchEntityException`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour of an instance profile that was PERSISTED rather than a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns, the key-list and quota fixtures are `tests/all/role_tag_removal.rs`'s, and
//! the profile and tag fixtures are `tests/all/instance_profile_tags.rs`'s, so a
//! request is built and read exactly one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the
//! held tag count, how many keys one call names, whether a named key is held or not,
//! the case of a key, the case of the profile name, the number of calls, the alphabet
//! and length bounds `tagKeyType` publishes (1..128), and the published per-resource
//! tag quota.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_profile_tags::{
    create_instance_profile, expected_map, stored_tags, tag_instance_profile, tags,
};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::user_lifecycle::{call, elements, error_code, router};

/// One `UntagInstanceProfile` call: the HTTP status and the XML body.
async fn untag_instance_profile(
    router: &MultiServiceRouter,
    profile_name: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("InstanceProfileName", profile_name)], removed);
    call(router, "UntagInstanceProfile", &borrowed(&params)).await
}

/// The tag set `ListInstanceProfileTags` reports for `profile_name`, so the removal is
/// proved through the dedicated tag reader as well as through `GetInstanceProfile`.
async fn listed_tags(
    router: &MultiServiceRouter,
    profile_name: &str,
) -> (BTreeMap<String, String>, usize) {
    let (status, body) = call(
        router,
        "ListInstanceProfileTags",
        &[("InstanceProfileName", profile_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {profile_name:?}: ListInstanceProfileTags must report the profile, got \
         {status}: {body}"
    );
    (reported_tags(&body), elements(&body, "Key").len())
}

/// One row: a profile created with `initial`, then one `UntagInstanceProfile` naming
/// `remove`, then the tag set the profile must hold.
struct RemoveCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagInstanceProfile` drops exactly the held tags whose keys the request
/// names and leaves every other held tag untouched, so `GetInstanceProfile` and
/// `ListInstanceProfileTags` both report the remaining set. One row per thing the rule
/// generalises over: the held tag count, how many keys one call names, held versus
/// unheld keys, the case of a key, the empty tag value, the published key-length bound
/// and alphabet, and the per-resource tag quota.
#[tokio::test]
async fn untag_instance_profile_removes_exactly_the_named_keys() {
    // `tagKeyType` is length 1..128 (`smithy_query(query="shape", shape="Tag")`), so
    // this is the longest key a stored tag can be named by.
    let longest_key = "k".repeat(128);
    // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so spaces, non-ASCII
    // letters, digits and that class's punctuation all name tags.
    let alphabet_key = "Cost Center_1.2:3/4=5+6-7@8";
    let quota = MAX_TAGS_PER_ENTITY;

    let cases = vec![
        RemoveCase {
            name: "the only held tag is removed, leaving none",
            initial: tags(&[("team", "storage")]),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            initial: tags(&[("team", "storage"), ("env", "prod"), ("owner", "ec2")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage"), ("owner", "ec2")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the profile does not hold removes nothing",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "storage"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            name: "a key named on a profile holding no tags",
            initial: Vec::new(),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM
            // User Guide), so a key spelled in another case names another tag.
            name: "keys differing only in case are distinct tags",
            initial: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
            remove: keys(&["env"]),
            expected: tags(&[("Env", "prod"), ("ENV", "test")]),
        },
        RemoveCase {
            name: "a key whose case the profile does not hold removes nothing",
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: tags(&[("Env", "prod")]),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "storage")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            initial: tags(&[("k", "short"), ("team", "storage")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a profile at the tag quota",
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50, and the quota is 50 tags per resource,
            // so this names the most keys one call can carry.
            name: "every key of a profile at the tag quota removed in one call",
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let profile_name = format!("remove-{index}");
        let name = case.name;
        create_instance_profile(&router, &profile_name, &case.initial).await;

        let (status, body) = untag_instance_profile(&router, &profile_name, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagInstanceProfile must succeed, got {status}: {body}"
        );

        let expected = expected_map(&case.expected);
        let (reported, key_count) = stored_tags(&router, &profile_name).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the remaining tag set {expected:?}, got {reported:?}"
        );
        assert_eq!(
            key_count,
            case.expected.len(),
            "case {name:?}: expected {} stored tags, got {key_count}",
            case.expected.len()
        );

        // The dedicated tag reader must agree with the profile read.
        let (listed, listed_count) = listed_tags(&router, &profile_name).await;
        assert_eq!(
            listed, expected,
            "case {name:?}: ListInstanceProfileTags expected {expected:?}, got {listed:?}"
        );
        assert_eq!(
            listed_count,
            case.expected.len(),
            "case {name:?}: ListInstanceProfileTags expected {} tags, got {listed_count}",
            case.expected.len()
        );
    }
}

/// RULE: removing a key the resource does not hold removes nothing and is not an
/// error, so repeating an untag call leaves the same tag set however many times it is
/// made.
#[tokio::test]
async fn repeating_an_untag_call_changes_nothing_further() {
    let held = tags(&[("team", "storage"), ("env", "prod"), ("owner", "ec2")]);
    let repeated = [
        (
            "one held key",
            keys(&["env"]),
            tags(&[("team", "storage"), ("owner", "ec2")]),
        ),
        (
            "two held keys",
            keys(&["env", "owner"]),
            tags(&[("team", "storage")]),
        ),
        ("only unheld keys", keys(&["absent", "other"]), held.clone()),
        ("every held key", keys(&["team", "env", "owner"]), Vec::new()),
    ];

    let router = router();
    for (index, (case, removed, expected)) in repeated.iter().enumerate() {
        let profile_name = format!("repeat-{index}");
        create_instance_profile(&router, &profile_name, &held).await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_instance_profile(&router, &profile_name, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagInstanceProfile attempt {attempt} must succeed, got \
                 {status}: {body}"
            );
            let wanted = expected_map(expected);
            let (reported, key_count) = stored_tags(&router, &profile_name).await;
            assert_eq!(
                reported, wanted,
                "case {case:?}: after attempt {attempt} expected {wanted:?}, got {reported:?}"
            );
            assert_eq!(
                key_count,
                expected.len(),
                "case {case:?}: after attempt {attempt} expected {} stored tags, got \
                 {key_count}",
                expected.len()
            );
        }
    }
}

/// RULE: an untag call names its instance profile by an IAM entity name, which is
/// unique within the account and "not distinguished by case"
/// (`CreateInstanceProfile$InstanceProfileName`), so any casing of a held name untags
/// the SAME profile — and a name the account does not hold is refused with the
/// modelled `NoSuchEntityException`.
#[tokio::test]
async fn the_instance_profile_is_found_by_its_case_folded_name() {
    let router = router();
    let held: Vec<(String, String)> = (0..4)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    create_instance_profile(&router, "MyProfile", &held).await;

    // Each spelling untags the one held profile, so the set shrinks across them
    // instead of each spelling getting a profile of its own.
    for (index, spelling) in ["MyProfile", "myprofile", "MYPROFILE", "mYpRoFiLe"]
        .iter()
        .enumerate()
    {
        let (status, body) =
            untag_instance_profile(&router, spelling, &keys(&[&format!("key-{index}")])).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200 for a held name, got {status}: {body}"
        );
        let (reported, key_count) = stored_tags(&router, spelling).await;
        let remaining = held.len() - index - 1;
        assert_eq!(
            key_count, remaining,
            "case {spelling:?}: expected {remaining} stored tags, got {key_count}: \
             {reported:?}"
        );
        assert!(
            !reported.contains_key(&format!("key-{index}")),
            "case {spelling:?}: the removed key must be gone from the held profile, got \
             {reported:?}"
        );
    }

    // A role of that name is no instance profile, so untagging it is still refused.
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
        let (status, body) = untag_instance_profile(&router, absent, &keys(&["team"])).await;
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

/// One row: a profile created with `initial`, then an `UntagInstanceProfile` naming
/// `removed`, then a `TagInstanceProfile` supplying `re_tagged`.
struct ReTagCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    removed: Vec<String>,
    re_tagged: Vec<(String, String)>,
}

/// RULE: an untagged key is simply not held any more, so it can be tagged again — and
/// because the per-resource tag quota is taken against the set the resource actually
/// holds, removing a tag frees room inside it.
#[tokio::test]
async fn a_removed_key_can_be_tagged_again() {
    let quota = MAX_TAGS_PER_ENTITY;

    // One row per starting point: a profile holding one tag, one holding several, and
    // one at the published quota, where the re-tag only fits because a key was
    // removed.
    let rows = [
        ReTagCase {
            name: "a profile holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a profile holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a profile at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let profile_name = format!("retag-{index}");
        let name = case.name;
        create_instance_profile(&router, &profile_name, &case.initial).await;

        let (status, body) = untag_instance_profile(&router, &profile_name, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagInstanceProfile must succeed, got {status}: {body}"
        );

        let (status, body) = tag_instance_profile(&router, &profile_name, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the profile holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());

        let (reported, key_count) = stored_tags(&router, &profile_name).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the tag set {expected:?}, got {reported:?}"
        );
        assert!(
            key_count <= quota,
            "case {name:?}: a profile must never hold more than {quota} tags, got \
             {key_count}"
        );
    }
}

/// RULE: untagging a resource changes only its tags — the identity and the role
/// binding the profile already carried are reported unchanged.
#[tokio::test]
async fn untagging_changes_only_the_tag_set() {
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

    let created = vec![
        param("InstanceProfileName", "bound-profile"),
        param("Path", "/service-role/"),
    ];
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
    let (status, body) = tag_instance_profile(
        &router,
        "bound-profile",
        &tags(&[("env", "prod"), ("team", "platform")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture TagInstanceProfile must succeed, got {status}: {body}"
    );

    let (status, before) = call(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", "bound-profile")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read before untagging must report the profile, got {status}: {before}"
    );

    let removed = keys(&["env"]);
    for round in 0..2 {
        let (status, body) = untag_instance_profile(&router, "bound-profile", &removed).await;
        assert_eq!(
            status, 200,
            "case round {round}: expected 200, got {status}: {body}"
        );
        let wanted = expected_map(&tags(&[("team", "platform")]));
        let (reported, key_count) = stored_tags(&router, "bound-profile").await;
        assert_eq!(
            reported, wanted,
            "case round {round}: expected the remaining tag set {wanted:?}, got {reported:?}"
        );
        assert_eq!(
            key_count, 1,
            "case round {round}: expected 1 stored tag, got {key_count}"
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
        "the read after untagging must report the profile, got {status}: {after}"
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
            "member {member}: untagging must leave it alone, expected {expected:?}, got \
             {actual:?}"
        );
    }
}

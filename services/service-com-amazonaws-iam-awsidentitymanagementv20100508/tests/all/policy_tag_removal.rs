// SPDX-License-Identifier: CC0-1.0

//! `UntagPolicy`, end to end over the awsQuery wire, read back through `GetPolicy`
//! and `ListPolicyTags`.
//!
//! RULE under test: an untag call drops from the stored tag set of the resource its
//! identifier names exactly the tags whose keys it names, comparing keys exactly, and
//! leaves every other held tag with its value and its place — so a named key the
//! resource does not hold removes nothing and the call still succeeds, which makes it
//! idempotent, and a key it did remove is free to be tagged again. `UntagPolicy`'s own
//! documentation: "removes the specified tags from the customer managed policy", with
//! "the tags with matching keys are removed from the specified policy"
//! (`UntagPolicy$TagKeys`).
//!
//! A managed policy is identified by its ARN rather than by a name, so the suites also
//! assert that the write is addressed exactly as a read of the same ARN is: a value
//! that is no managed-policy ARN at all is refused as an invalid parameter, an ARN in
//! the account's namespace that names no held policy is the missing entity, and an ARN
//! in AWS's own namespace names a policy the account cannot change. IAM also authorizes
//! its own API, so a SIGNED call is decided against the calling principal's
//! identity-based policies before any tag is removed.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour of a policy that was PERSISTED rather than a handler called
//! directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the
//! tag-list encoding and tag-table helpers are `tests/all/role_tags.rs`'s, the
//! tag-key-list encoding is `tests/all/role_tag_removal.rs`'s and the managed-policy
//! fixtures are `tests/all/policy_tags.rs`'s, so a request is built and read exactly
//! one way across this crate.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the
//! held tag count, how many keys one call names, whether a named key is held or not,
//! the case of a key, the policy's path, the number of calls, the alphabet and length
//! bounds `tagKeyType` publishes (1..128), the published per-resource tag quota, and
//! the identity policies in force on the caller.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::policy_tags::{account_policy_arn, create_policy, stored_tags, tag_policy};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::{fixture, keys, with_key_params};
use crate::role_tags::{expected_map, tags};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// One `UntagPolicy` call: the HTTP status and the XML body.
async fn untag_policy(
    router: &MultiServiceRouter,
    policy_arn: &str,
    removed: &[String],
) -> (u16, String) {
    let params = with_key_params(vec![param("PolicyArn", policy_arn)], removed);
    call(router, "UntagPolicy", &borrowed(&params)).await
}

/// The tag set `ListPolicyTags` reports for `policy_arn`, as a map — the SECOND
/// reader of the same stored set, so a removal is asserted through every reporter of
/// a policy's tags rather than only the one the write went through.
async fn listed_tags(router: &MultiServiceRouter, policy_arn: &str) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        "ListPolicyTags",
        &[("PolicyArn", policy_arn), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: ListPolicyTags must report the policy's tags, got {status}: {body}"
    );
    reported_tags(&body)
}

/// One row: a policy created under `path` carrying `initial`, then one `UntagPolicy`
/// naming `remove`, then the tag set the policy must hold.
struct RemoveCase {
    name: &'static str,
    path: Option<&'static str>,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// RULE: `UntagPolicy` drops exactly the held tags whose keys the request names and
/// leaves every other held tag untouched, so `GetPolicy` and `ListPolicyTags` report
/// the remaining set. One row per thing the rule generalises over: the policy's path,
/// the held tag count, how many keys one call names, held versus unheld keys, the case
/// of a key, the empty tag value, the published key-length bound and alphabet, and the
/// per-resource tag quota.
#[tokio::test]
async fn untag_policy_removes_exactly_the_named_keys() {
    // `tagKeyType` is length 1..128 (`smithy_query(query="shape", shape="Tag")`), so
    // this is the longest key a stored tag can be named by.
    let longest_key = "k".repeat(128);
    // `tagKeyType`'s pattern is `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`, so spaces, non-ASCII
    // letters, digits and that class's punctuation all name tags.
    let alphabet_key = "Cost Center_1.2:3/4=5+6-7@8";
    let quota = state::MAX_TAGS_PER_ENTITY;

    let cases = vec![
        RemoveCase {
            name: "the only held tag is removed, leaving none",
            path: None,
            initial: tags(&[("team", "storage")]),
            remove: keys(&["team"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "one of several held tags is removed",
            path: None,
            initial: tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]),
            remove: keys(&["env"]),
            expected: tags(&[("team", "storage"), ("owner", "dlm")]),
        },
        RemoveCase {
            name: "several held keys are removed in one call",
            path: Some("/one-segment/"),
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            remove: keys(&["a", "c"]),
            expected: tags(&[("b", "2"), ("d", "4")]),
        },
        RemoveCase {
            name: "every held key is removed in one call",
            path: Some("/a/b/c/"),
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            remove: keys(&["a", "b", "c"]),
            expected: Vec::new(),
        },
        RemoveCase {
            name: "a key the policy does not hold removes nothing",
            path: None,
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["absent"]),
            expected: tags(&[("team", "storage"), ("env", "prod")]),
        },
        RemoveCase {
            name: "a held key named beside an unheld one",
            path: Some("/"),
            initial: tags(&[("team", "storage"), ("env", "prod")]),
            remove: keys(&["team", "absent"]),
            expected: tags(&[("env", "prod")]),
        },
        RemoveCase {
            // "Tag keys and values are case sensitive" ("Tagging IAM resources", IAM
            // User Guide), so a key spelled in another case names another tag.
            name: "keys differing only in case are distinct tags",
            path: None,
            initial: tags(&[("Env", "prod"), ("env", "dev"), ("ENV", "test")]),
            remove: keys(&["env"]),
            expected: tags(&[("Env", "prod"), ("ENV", "test")]),
        },
        RemoveCase {
            name: "a key whose case the policy does not hold removes nothing",
            path: None,
            initial: tags(&[("Env", "prod")]),
            remove: keys(&["env", "ENV"]),
            expected: tags(&[("Env", "prod")]),
        },
        RemoveCase {
            name: "a tag carrying the empty value is removed by its key",
            path: None,
            initial: tags(&[("empty", ""), ("keep", "v")]),
            remove: keys(&["empty"]),
            expected: tags(&[("keep", "v")]),
        },
        RemoveCase {
            name: "a key at the published maximum length",
            path: None,
            initial: tags(&[(longest_key.as_str(), "fits"), ("team", "storage")]),
            remove: keys(&[longest_key.as_str()]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key at the published minimum length",
            path: None,
            initial: tags(&[("k", "short"), ("team", "storage")]),
            remove: keys(&["k"]),
            expected: tags(&[("team", "storage")]),
        },
        RemoveCase {
            name: "a key spanning the modelled alphabet",
            path: Some("/alphabet/"),
            initial: tags(&[(alphabet_key, "x"), ("keep", "y")]),
            remove: keys(&[alphabet_key]),
            expected: tags(&[("keep", "y")]),
        },
        RemoveCase {
            name: "one key removed from a policy at the tag quota",
            path: None,
            initial: fixture(quota),
            remove: keys(&["key-00"]),
            expected: fixture(quota)[1..].to_vec(),
        },
        RemoveCase {
            // `tagKeyListType` is length 0..50 and the quota is 50 tags per resource,
            // so this names the most keys one call can carry.
            name: "every key of a policy at the tag quota removed in one call",
            path: None,
            initial: fixture(quota),
            remove: fixture(quota).iter().map(|(key, _)| key.clone()).collect(),
            expected: Vec::new(),
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let policy_name = format!("remove-{index}");
        let arn = create_policy(&router, &policy_name, case.path, &case.initial).await;

        let (status, body) = untag_policy(&router, &arn, &case.remove).await;
        assert_eq!(
            status, 200,
            "case {:?}: UntagPolicy must succeed, got {status}: {body}",
            case.name
        );

        let (reported, key_count) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported,
            expected_map(&case.expected),
            "case {:?}: expected the remaining tag set {:?}, got {reported:?}",
            case.name,
            expected_map(&case.expected)
        );
        assert_eq!(
            key_count,
            case.expected.len(),
            "case {:?}: expected {} stored tags, got {key_count}",
            case.name,
            case.expected.len()
        );

        let listed = listed_tags(&router, &arn).await;
        assert_eq!(
            listed,
            expected_map(&case.expected),
            "case {:?}: ListPolicyTags must report the same remaining set {:?}, got {listed:?}",
            case.name,
            expected_map(&case.expected)
        );
    }
}

/// RULE: removing a key the resource does not hold removes nothing and is not an
/// error, so repeating an untag call leaves the same tag set however many times it is
/// made.
#[tokio::test]
async fn repeating_an_untag_call_changes_nothing_further() {
    let held = tags(&[("team", "storage"), ("env", "prod"), ("owner", "dlm")]);
    let repeated = [
        (
            "one held key",
            keys(&["env"]),
            tags(&[("team", "storage"), ("owner", "dlm")]),
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
        let policy_name = format!("repeat-{index}");
        let arn = create_policy(&router, &policy_name, None, &held).await;

        // Three identical calls: the first may change the set, the rest must not.
        for attempt in 0..3 {
            let (status, body) = untag_policy(&router, &arn, removed).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UntagPolicy attempt {attempt} must succeed, got {status}: {body}"
            );
            let (reported, key_count) = stored_tags(&router, &arn).await;
            assert_eq!(
                reported,
                expected_map(expected),
                "case {case:?}: after attempt {attempt} expected {:?}, got {reported:?}",
                expected_map(expected)
            );
            assert_eq!(
                key_count,
                expected.len(),
                "case {case:?}: after attempt {attempt} expected {} stored tags, got {key_count}",
                expected.len()
            );
        }
    }
}

/// RULE: an untag call on a managed policy addresses it by the policy's own ARN,
/// exactly as a read of that policy does — so the ARN the create reported untags the
/// held policy, a value that is no managed-policy ARN at all is refused as an invalid
/// input parameter, and an ARN in the account's namespace that names no held policy is
/// the missing entity. Neither refusal changes anything.
#[tokio::test]
async fn the_policy_is_addressed_by_its_own_arn() {
    let router = router();
    let held = tags(&[("team", "storage"), ("env", "prod")]);
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
    // type — and so is the sentence: this operation decides no ATTACHABILITY, so a value
    // it cannot read as an ARN is stated by IAM's generic ARN parser, which quotes nothing
    // of it, while an ARN that IS one is named in the refusal.
    let unparseable = "Invalid ARN:  Could not be parsed!";
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
        let (status, body) = untag_policy(&router, &arn, &keys(&["team"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
        let expected_message = match expected_code {
            "ValidationError" => unparseable.to_string(),
            _ => format!("ARN {arn} is not valid."),
        };
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {body}"
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
        (
            "a name under a nested path",
            account_policy_arn("/a/b/c/", "absent"),
        ),
        (
            "the held ARN in another case",
            account_policy_arn("/held/", "HELD"),
        ),
        // The refusal interpolates the value the request supplied, so a row carrying the
        // XML metacharacters the awsQuery envelope is built from proves the envelope
        // stays well formed and the value round-trips through it.
        (
            "a policy name carrying XML metacharacters",
            account_policy_arn("/", "<held>&\"it's\" not a policy"),
        ),
    ] {
        let (status, body) = untag_policy(&router, &arn, &keys(&["team"])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for the unheld ARN {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        // The refusal names the ARN it found no policy for, so the message is built
        // from the request rather than from any one recorded value.
        assert_eq!(
            element(&body, "Message"),
            format!("Policy {arn} was not found."),
            "case {case:?}: expected the refusal to name the ARN asked about, got {body}"
        );
    }

    // No refusal touched the held policy.
    let (reported, key_count) = stored_tags(&router, &held_arn).await;
    assert_eq!(
        reported,
        expected_map(&held),
        "a refused request must change nothing, got {reported:?}"
    );
    assert_eq!(
        key_count,
        held.len(),
        "expected {} stored tags, got {key_count}",
        held.len()
    );
}

/// RULE: this operation removes tags from "the customer managed policy", so an ARN in
/// AWS's own managed-policy namespace (`arn:aws:iam::aws:policy/...`) names a policy
/// the account cannot change and is refused with the modelled `InvalidInputException`
/// — whether or not this emulator's catalog carries that entry, because no entry of it
/// could carry a tag the account is able to remove either.
#[tokio::test]
async fn an_aws_managed_policy_cannot_be_untagged() {
    let router = router();

    // SOURCE: the catalog this crate carries (`state::AWS_MANAGED_POLICIES`), which
    // records the AWS-managed policies the emulator serves; every entry is a row, so no
    // single one is the specification.
    let mut arns: Vec<(String, String)> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| (format!("the catalog entry {}", entry.policy_name), entry.arn()))
        .collect();
    // An ARN in AWS's namespace the catalog does NOT carry is refused the same way: the
    // account could not untag it even if the catalog grew to hold it.
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
        let (status, body) = untag_policy(&router, arn, &keys(&["team"])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 untagging the AWS-owned {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refusal changed nothing. A catalog entry reads back carrying no tags of
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
                "case {case:?}: a refused request must change nothing, got {reported:?}"
            );
            assert!(
                elements(&read, "Key").is_empty(),
                "case {case:?}: a catalog entry carries no tags, got {read}"
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

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the policies in force permit the removal.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// RULE: IAM authorizes its own API, so a signed untag call is decided against the
/// signing principal's identity-based policies BEFORE any tag is removed — a `Deny`
/// about the request refuses it whatever any `Allow` says, an `Allow` with no such
/// `Deny` serves it, and a principal with neither is refused — and a refusal is HTTP
/// 403 `AccessDenied` whose message names the principal, the action and the policy the
/// decision was made on, with nothing removed.
///
/// One row per thing the rule generalises over: whether a statement is present at all,
/// its effect, whether its action element is a literal, a wildcard or another action,
/// and whether its resource element covers the policy the request addresses.
#[tokio::test]
async fn a_signed_untag_is_decided_against_the_callers_identity_policies() {
    let router = router();
    let held = tags(&[("team", "storage"), ("env", "prod")]);

    // Built per row, because the Allow's resource element is scoped to the row's own
    // policy ARN, which is only known once the policy exists.
    type Policies = Box<dyn Fn(&str) -> Vec<(String, String)>>;
    let rows: Vec<(&str, Policies, Decided)> = vec![
        (
            "no identity-based policy at all",
            Box::new(|_arn: &str| Vec::new()),
            Decided::NoAllow,
        ),
        (
            "an Allow naming the action on every resource",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-untag".to_string(),
                    statement("Allow", "iam:UntagPolicy", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow scoped to the policy the request addresses",
            Box::new(|arn: &str| {
                vec![(
                    "allow-this-policy".to_string(),
                    statement("Allow", "iam:UntagPolicy", arn),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-untag-star".to_string(),
                    statement("Allow", "iam:Untag*", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow for the tagging action only",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-tag".to_string(),
                    statement("Allow", "iam:TagPolicy", "*"),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "an Allow scoped to another policy's ARN",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-other-policy".to_string(),
                    statement(
                        "Allow",
                        "iam:UntagPolicy",
                        &account_policy_arn("/", "some-other-policy"),
                    ),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "a Deny naming the action beside an Allow",
            Box::new(|_arn: &str| {
                vec![
                    (
                        "allow-untag".to_string(),
                        statement("Allow", "iam:UntagPolicy", "*"),
                    ),
                    (
                        "deny-untag".to_string(),
                        statement("Deny", "iam:UntagPolicy", "*"),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
        (
            "a Deny on every action beside an Allow",
            Box::new(|_arn: &str| {
                vec![
                    (
                        "allow-untag".to_string(),
                        statement("Allow", "iam:UntagPolicy", "*"),
                    ),
                    ("deny-everything".to_string(), statement("Deny", "*", "*")),
                ]
            }),
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        let arn = create_policy(&router, &format!("signed-{index}"), None, &held).await;
        let user_name = format!("untagger{index}");
        let access_key_id = principal(&router, case, &user_name, &policies(&arn)).await;

        let params = with_key_params(vec![param("PolicyArn", &arn)], &keys(&["env"]));
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "UntagPolicy",
            &borrowed(&params),
        )
        .await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status}: {body}"
                );
                let (reported, _) = stored_tags(&router, &arn).await;
                assert_eq!(
                    reported,
                    expected_map(&tags(&[("team", "storage")])),
                    "case {case:?}: expected the named tag to be removed, got {reported:?}"
                );
            }
            Decided::ExplicitDeny | Decided::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {body}"
                );
                // The message is built from the request: the principal that signed it,
                // the action it named, the policy the decision was made on, which of
                // the two refusals this is, and where this one denial's authorization
                // details are readable.
                let reason = if *expect == Decided::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:UntagPolicy action"
                        .to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:UntagPolicy",
                    &format!("policy {arn}"),
                    &reason,
                );
                // A refused call removes nothing.
                let (reported, key_count) = stored_tags(&router, &arn).await;
                assert_eq!(
                    reported,
                    expected_map(&held),
                    "case {case:?}: a refused request must remove nothing, got {reported:?}"
                );
                assert_eq!(
                    key_count,
                    held.len(),
                    "case {case:?}: expected {} stored tags, got {key_count}",
                    held.len()
                );
            }
        }
    }
}

/// One row: a policy created with `initial`, then an `UntagPolicy` naming `removed`,
/// then a `TagPolicy` supplying `re_tagged`.
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
    let quota = state::MAX_TAGS_PER_ENTITY;

    // One row per starting point: a policy holding one tag, one holding several, and
    // one at the published quota, where the re-tag only fits because a key was removed.
    let rows = [
        ReTagCase {
            name: "a policy holding one tag",
            initial: tags(&[("env", "prod")]),
            removed: keys(&["env"]),
            re_tagged: tags(&[("env", "staging")]),
        },
        ReTagCase {
            name: "a policy holding several tags",
            initial: tags(&[("a", "1"), ("b", "2")]),
            removed: keys(&["a", "b"]),
            re_tagged: tags(&[("a", "one"), ("c", "3")]),
        },
        ReTagCase {
            name: "a policy at the published tag quota",
            initial: fixture(quota),
            removed: keys(&["key-00", "key-01"]),
            re_tagged: tags(&[("new-a", "a"), ("new-b", "b")]),
        },
    ];

    let router = router();
    for (index, case) in rows.iter().enumerate() {
        let policy_name = format!("retag-{index}");
        let name = case.name;
        let arn = create_policy(&router, &policy_name, None, &case.initial).await;

        let (status, body) = untag_policy(&router, &arn, &case.removed).await;
        assert_eq!(
            status, 200,
            "case {name:?}: UntagPolicy must succeed, got {status}: {body}"
        );

        let (status, body) = tag_policy(&router, &arn, &case.re_tagged).await;
        assert_eq!(
            status, 200,
            "case {name:?}: re-tagging a removed key must succeed, got {status}: {body}"
        );

        // What the policy holds now: the initial set less the removed keys, plus the
        // re-tagged ones.
        let mut expected: BTreeMap<String, String> = expected_map(&case.initial);
        for key in &case.removed {
            expected.remove(key);
        }
        expected.extend(case.re_tagged.iter().cloned());

        let (reported, key_count) = stored_tags(&router, &arn).await;
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the tag set {expected:?}, got {reported:?}"
        );
        assert!(
            key_count <= quota,
            "case {name:?}: a policy must never hold more than {quota} tags, got {key_count}"
        );
    }
}

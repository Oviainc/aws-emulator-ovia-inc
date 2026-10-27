// SPDX-License-Identifier: CC0-1.0

//! A SIGNED `TagPolicy` call, end to end over the awsQuery wire, read back through
//! `GetPolicy`.
//!
//! RULE under test: IAM authorizes its own API, so a signed tag call is decided against
//! the signing principal's identity-based policies before any tag is stored — a `Deny`
//! about the request refuses it whatever any `Allow` says, an `Allow` with no such
//! `Deny` serves it, and a principal with neither is refused — and a refusal is HTTP
//! 403 `AccessDenied` whose message names the principal, the action and the policy the
//! decision was made on, with nothing stored. Because the decision precedes the store
//! lookup, a caller that may not tag learns that it may not rather than whether the
//! account holds the policy its request names.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of a policy that was PERSISTED rather than a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the signing
//! principal and the policy documents are `tests/all/identity_policy_enforcement.rs`'s,
//! the tag-list encoding is `tests/all/role_tags.rs`'s and the managed-policy fixtures
//! are `tests/all/policy_tags.rs`'s, so a request is built and read exactly one way
//! across this crate.
//!
//! The suite is a table whose rows differ along what the rule generalises over: whether
//! a statement is in force at all, its effect, whether its action element is a literal,
//! a wildcard run or another action, whether its resource element covers the policy the
//! request addresses, and whether the policy the request names exists.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::policy_tags::{account_policy_arn, create_policy, stored_tags};
use crate::role_lifecycle::{borrowed, param};
use crate::role_tags::{expected_map, tags, with_tag_params};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call_signed_by, element, error_code, router,
};

/// One `TagPolicy` call presented as signed by `access_key_id`: the HTTP status and the
/// XML body.
async fn tag_policy_signed(
    router: &MultiServiceRouter,
    access_key_id: &str,
    policy_arn: &str,
    list: &[(String, String)],
) -> (u16, String) {
    let params = with_tag_params(vec![param("PolicyArn", policy_arn)], list);
    call_signed_by(router, Some(access_key_id), "TagPolicy", &borrowed(&params)).await
}

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the policies in force permit the tagging.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// The documents in force on the row's caller, built from the row's own policy ARN —
/// which is only known once the policy exists.
type Policies = Box<dyn Fn(&str) -> Vec<(String, String)>>;

/// RULE: a signed `TagPolicy` is decided against the signing principal's identity-based
/// policies before anything is stored, and a refusal is HTTP 403 `AccessDenied` naming
/// the principal, the action and the policy.
#[tokio::test]
async fn a_signed_tag_is_decided_against_the_callers_identity_policies() {
    let router = router();
    let held = tags(&[("team", "storage")]);
    let added = tags(&[("env", "prod"), ("team", "platform")]);

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
                    "allow-tag".to_string(),
                    statement("Allow", "iam:TagPolicy", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow scoped to the policy the request addresses",
            Box::new(|arn: &str| {
                vec![(
                    "allow-this-policy".to_string(),
                    statement("Allow", "iam:TagPolicy", arn),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-tag-star".to_string(),
                    statement("Allow", "iam:Tag*", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow for the untagging action only",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-untag".to_string(),
                    statement("Allow", "iam:UntagPolicy", "*"),
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
                        "iam:TagPolicy",
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
                        "allow-tag".to_string(),
                        statement("Allow", "iam:TagPolicy", "*"),
                    ),
                    (
                        "deny-tag".to_string(),
                        statement("Deny", "iam:TagPolicy", "*"),
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
                        "allow-tag".to_string(),
                        statement("Allow", "iam:TagPolicy", "*"),
                    ),
                    ("deny-everything".to_string(), statement("Deny", "*", "*")),
                ]
            }),
            Decided::ExplicitDeny,
        ),
        (
            "a Deny scoped to the policy the request addresses",
            Box::new(|arn: &str| {
                vec![
                    (
                        "allow-tag".to_string(),
                        statement("Allow", "iam:TagPolicy", "*"),
                    ),
                    (
                        "deny-this-policy".to_string(),
                        statement("Deny", "iam:TagPolicy", arn),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        let arn = create_policy(&router, &format!("signed-{index}"), None, &held).await;
        let user_name = format!("tagger{index}");
        let access_key_id = principal(&router, case, &user_name, &policies(&arn)).await;

        let (status, body) = tag_policy_signed(&router, &access_key_id, &arn, &added).await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status}: {body}"
                );
                // The merged set: the held tag whose key the request overwrote, plus the
                // new one.
                let mut expected = expected_map(&held);
                expected.extend(added.iter().cloned());
                let (reported, key_count) = stored_tags(&router, &arn).await;
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the merged tag set {expected:?}, got {reported:?}"
                );
                assert_eq!(
                    key_count,
                    expected.len(),
                    "case {case:?}: expected {} stored tags, got {key_count}",
                    expected.len()
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
                // the action it named, the policy the decision was made on, which of the
                // two refusals this is, and where this one denial's authorization details
                // are readable — asserted through the one reader of that wording this
                // crate's test tree holds.
                let reason = if *expect == Decided::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:TagPolicy action".to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:TagPolicy",
                    &format!("policy {arn}"),
                    &reason,
                );
                // A refused call stores nothing.
                let (reported, key_count) = stored_tags(&router, &arn).await;
                assert_eq!(
                    reported,
                    expected_map(&held),
                    "case {case:?}: a refused request must store nothing, got {reported:?}"
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

/// RULE: the decision precedes the store lookup, so a caller whose policies do not
/// allow the action is refused with HTTP 403 `AccessDenied` for an ARN the account holds
/// no policy under — the refusal says nothing about whether that policy exists — while
/// the same request from a caller the policies DO allow reaches the lookup and is
/// answered with the missing entity.
///
/// One row per ARN the account holds no policy under, differing in the name and the path
/// the ARN carries.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_before_the_policy_is_looked_up() {
    let router = router();
    let added = tags(&[("env", "prod")]);

    let absent = [
        ("a name at the account root", account_policy_arn("/", "absent")),
        (
            "a name under a nested path",
            account_policy_arn("/a/b/c/", "absent"),
        ),
        (
            "a name whose only unusual feature is its case",
            account_policy_arn("/", "ABSENT"),
        ),
    ];

    // The caller with no grant: every absent ARN answers the authorization refusal, not
    // the missing entity, so the refusal reveals nothing about the account's contents.
    let refused_key = principal(&router, "no grant", "unauthorized-tagger", &[]).await;
    for (case, arn) in &absent {
        let (status, body) = tag_policy_signed(&router, &refused_key, arn, &added).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {body}"
        );
    }

    // The caller the policies allow reaches the lookup, which is where the ARN naming no
    // held policy becomes the missing entity.
    let allowed_key = principal(
        &router,
        "an Allow on every resource",
        "authorized-tagger",
        &[(
            "allow-tag".to_string(),
            statement("Allow", "iam:TagPolicy", "*"),
        )],
    )
    .await;
    for (case, arn) in &absent {
        let (status, body) = tag_policy_signed(&router, &allowed_key, arn, &added).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the error code NoSuchEntity, got {body}"
        );
    }

    // And the same allowed caller tags a policy the account does hold, so the grant is
    // what the two answers above differ by rather than the request.
    let held = tags(&[("team", "storage")]);
    let arn = create_policy(&router, "held-by-the-account", None, &held).await;
    let (status, body) = tag_policy_signed(&router, &allowed_key, &arn, &added).await;
    assert_eq!(
        status, 200,
        "an allowed caller must tag a held policy, got {status}: {body}"
    );
    let mut expected = expected_map(&held);
    expected.extend(added.iter().cloned());
    let (reported, key_count) = stored_tags(&router, &arn).await;
    assert_eq!(
        reported, expected,
        "expected the merged tag set {expected:?}, got {reported:?}"
    );
    assert_eq!(
        key_count,
        expected.len(),
        "expected {} stored tags, got {key_count}",
        expected.len()
    );
}

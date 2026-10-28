// SPDX-License-Identifier: CC0-1.0

//! The reads of ONE IAM user, decided against the caller's policies, end to end over the
//! awsQuery wire.
//!
//! The rule under test: IAM authorizes its own API, so every operation that reports a
//! named IAM user — its record, the tags on it, the credentials it holds, a policy
//! embedded in it — is decided against the signing principal's identity-based policies on
//! the ARN of the user the request ADDRESSES, before the store is read. A statement is
//! therefore confined to the resources its `Resource` element names: a grant scoped to the
//! caller's own ARN serves the caller's read of itself and refuses the identical read of
//! another user, whichever of those operations makes it. `NotAction` grants the COMPLEMENT
//! of the actions it lists and widens the `Resource` element not at all, so excluding one
//! action refuses exactly that one on the named resource while the rest stay served, and
//! swapping which action is excluded swaps exactly those two answers.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the user-addressed reads is called, whether it addresses the caller itself, a sibling,
//! or no user the account holds, whether it names a user at all, and which action the
//! document excludes.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::principal;
use crate::user_lifecycle::{account_id, call, call_signed_by, element, error_code, router};

/// The inline policy the caller holds, by name: a row reads it back through
/// `GetUserPolicy`, which is one of the operations under test.
const POLICY_NAME: &str = "notaction";

/// The caller, and the sibling user whose records it must not reach.
const CALLER: &str = "confined";
const SIBLING: &str = "sibling";

/// A name the account holds no user under, so a row can show which refusal comes first.
const ABSENT: &str = "never-created";

/// The reads of one named user this suite drives: the awsQuery `Action`, the IAM action
/// the decision is made on, and whether the call needs an inline policy's name beside the
/// user's.
struct Read {
    action: &'static str,
    iam_action: &'static str,
    names_policy: bool,
}

/// The reads. Each addresses ONE user by name and reports something about it, so each is
/// decided on that user's ARN.
const READS: &[Read] = &[
    Read {
        action: "GetUser",
        iam_action: "iam:GetUser",
        names_policy: false,
    },
    Read {
        action: "GetUserPolicy",
        iam_action: "iam:GetUserPolicy",
        names_policy: true,
    },
    Read {
        action: "ListUserTags",
        iam_action: "iam:ListUserTags",
        names_policy: false,
    },
    Read {
        action: "ListAccessKeys",
        iam_action: "iam:ListAccessKeys",
        names_policy: false,
    },
];

/// An `Allow` whose action element is a `NotAction` listing `excluded`, scoped to the ONE
/// resource `resource`: it grants every other action on that resource and nothing on any
/// other.
fn not_action(excluded: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","NotAction":["{excluded}"],"Resource":"{resource}"}}]}}"#
    )
}

/// The ARN of an IAM user of this account at the root path.
fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// Make the call `read` addresses at `user_name`, signed by `access_key_id`.
async fn read_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    read: &Read,
    user_name: &str,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![("UserName", user_name)];
    if read.names_policy {
        params.push(("PolicyName", POLICY_NAME));
    }
    call_signed_by(router, Some(access_key_id), read.action, &params).await
}

/// Create the sibling user, with a key and a tag of its own, so every read under test has
/// something to report about it and a refusal cannot be an empty answer in disguise.
async fn sibling(router: &MultiServiceRouter, case: &str) {
    let (status, created) = call(
        router,
        "CreateUser",
        &[
            ("UserName", SIBLING),
            ("Tags.member.1.Key", "team"),
            ("Tags.member.1.Value", "payroll"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", SIBLING)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    let (status, put) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", SIBLING),
            ("PolicyName", POLICY_NAME),
            (
                "PolicyDocument",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {put}"
    );
}

/// RULE: under an `Allow` whose `NotAction` excludes one action and whose `Resource` names
/// only the caller's own ARN, each read of the caller's OWN record is served except the
/// excluded one, and the identical read of a sibling is refused — the `NotAction` widens
/// the resource element not at all. Swapping which action is excluded swaps exactly those
/// two own-resource answers.
#[tokio::test]
async fn a_notaction_grant_is_confined_to_the_resource_it_names() {
    // Rows: which action the document excludes. Both are reads this suite drives, so each
    // row's exclusion is observable on its own and on its sibling rows.
    for excluded in ["iam:ListAccessKeys", "iam:ListUserTags"] {
        let router = router();
        sibling(&router, excluded).await;
        let access_key_id = principal(
            &router,
            excluded,
            CALLER,
            &[(
                POLICY_NAME.to_string(),
                not_action(excluded, &user_arn(CALLER)),
            )],
        )
        .await;

        for read in READS {
            let case = format!("{} excluded, reading {}", excluded, read.action);

            // The caller's own ARN is the resource the statement names, so every action
            // but the excluded one is granted on it.
            let (status, body) = read_as(&router, &access_key_id, read, CALLER).await;
            if read.iam_action == excluded {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected the excluded action to be refused on the \
                     caller's own record, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected AccessDenied, got {:?}",
                    error_code(&body)
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the complement of the excluded action to be \
                     served on the caller's own record, got {status}: {body}"
                );
            }

            // The sibling's ARN is outside that element, so the same read of it is
            // refused whatever the action.
            let (status, body) = read_as(&router, &access_key_id, read, SIBLING).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected a sibling outside the statement's Resource to be \
                 refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied for the sibling, got {:?}",
                error_code(&body)
            );
            assert!(
                element(&body, "Message").contains(&format!("{} on resource: user {SIBLING}", read.iam_action)),
                "case {case:?}: expected the refusal to name the action and the user the \
                 request addressed, got {:?}",
                element(&body, "Message")
            );
        }
    }
}

/// RULE: the decision is made on the user the request ADDRESSES, so a nameless read —
/// which addresses the signing user itself — is decided on the caller's own ARN, and a
/// read of a name the account holds no user under is refused before the store is
/// consulted: an unauthorized caller learns that it may not read, never whether the user
/// exists.
#[tokio::test]
async fn the_decision_is_made_on_the_user_the_request_addresses() {
    let router = router();
    sibling(&router, "the addressed user").await;
    let access_key_id = principal(
        &router,
        "the addressed user",
        CALLER,
        &[(
            POLICY_NAME.to_string(),
            not_action("iam:ListAccessKeys", &user_arn(CALLER)),
        )],
    )
    .await;

    // A nameless `GetUser` addresses the signing user, which the statement's resource
    // element names, so it is served and reports that user.
    let (status, body) = call_signed_by(&router, Some(&access_key_id), "GetUser", &[]).await;
    assert_eq!(
        status, 200,
        "case \"a nameless read of the signing user\": expected the caller's own record, \
         got {status}: {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        CALLER,
        "case \"a nameless read of the signing user\": expected the signing user's record, \
         got {body}"
    );

    // Rows: a name outside the statement's resource element that the account holds, and
    // one it does not. Both are refused, and the missing one is NOT reported as missing.
    for (case, user_name) in [
        ("a sibling the account holds", SIBLING),
        ("a name the account holds nothing under", ABSENT),
    ] {
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "GetUser", &[("UserName", user_name)])
                .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the read outside the resource element to be refused, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied rather than a report of what the account \
             holds, got {:?}",
            error_code(&body)
        );
    }

    // The same read, by the user that IS the resource: the sibling holds no grant for it,
    // so the refusal is the caller's own policies' doing and not the operation's.
    let (status, created) = call(&router, "CreateAccessKey", &[("UserName", SIBLING)]).await;
    assert_eq!(
        status, 200,
        "case \"the sibling's own key\": the fixture CreateAccessKey must succeed, got \
         {status}: {created}"
    );
    let sibling_key = element(&created, "AccessKeyId");
    let (status, body) =
        call_signed_by(&router, Some(&sibling_key), "GetUser", &[("UserName", SIBLING)]).await;
    assert_eq!(
        status, 403,
        "case \"a user whose policies do not allow the read of itself\": expected the \
         refusal, got {status}: {body}"
    );
}

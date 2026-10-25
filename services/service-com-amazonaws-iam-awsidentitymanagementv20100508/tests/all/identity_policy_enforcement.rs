// SPDX-License-Identifier: CC0-1.0

//! IAM authorizes its OWN API: every call is decided against the signing principal's
//! identity-based policies before the operation reports or changes anything.
//!
//! The rule under test: for a request signed with an IAM user's access key, IAM reads the
//! identity-based documents in force on that user and decides the call the way its
//! documented evaluation does — a statement that is ABOUT the request (its action element
//! matches the requested action, its resource element the requested resource) and says
//! `Deny` refuses whatever any `Allow` says; an `Allow` with no such `Deny` permits; a
//! principal with neither is refused. A refusal is HTTP 403 with the awsQuery error code
//! `AccessDenied` and a message built from the principal, the action and the resource.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! effect, whether the action element is a literal, a wildcard or a `NotAction`, the case
//! the action is spelled in, how many documents carry the statements and in which order,
//! the resource the statement is scoped to, the path prefix the request addresses, and
//! which operation is called. No single request is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, call_signed_by, element, error_code, router};

/// What a row expects the signed call to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row: the inline policies the calling user holds, the call it makes, and the
/// answer the rule requires.
struct Case {
    case: String,
    /// `(PolicyName, PolicyDocument)` pairs, each put on the caller with
    /// `PutUserPolicy` in this order.
    policies: Vec<(String, String)>,
    /// The awsQuery `Action`, and the IAM action name the decision is made on.
    action: &'static str,
    iam_action: String,
    /// The form parameters the call carries.
    params: Vec<(String, String)>,
    /// The ARN of the resource the decision is made on — the collection the request
    /// addresses, which is what a refusal names.
    resource: String,
    expect: Expect,
}

/// An `Allow` for every action this suite calls, on every resource: the grant each row
/// that is about a `Deny` is layered on top of, so what the row demonstrates is the
/// `Deny` and not the absence of a grant.
fn allow_all_three() -> String {
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListGroups","iam:ListRoles","iam:ListUsers"],"Resource":"*"}]}"#
        .to_string()
}

/// A one-statement document with `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// The ARN of an IAM collection: the account's `<resource type>` under `path_prefix`,
/// which is the resource a list read is authorized on
/// (`arn:aws:iam::<account>:group<path prefix>`, the form `crate::state::group_arn`
/// builds).
fn collection(resource_type: &str, path_prefix: &str) -> String {
    format!(
        "arn:aws:iam::{}:{resource_type}{path_prefix}",
        account_id()
    )
}

/// The rows. Every one of them runs against a fresh IAM user, so a row's policies are
/// the only documents in force when it calls.
fn cases() -> Vec<Case> {
    let group_root = collection("group", "/");
    let group_eng = collection("group", "/eng/");
    let role_root = collection("role", "/");
    let user_root = collection("user", "/");
    vec![
        Case {
            case: "an Allow naming the action".to_string(),
            policies: vec![("allow-all-three".to_string(), allow_all_three())],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::Served,
        },
        Case {
            case: "a Deny naming the action, in a second document".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-listgroups".to_string(),
                    statement("Deny", "iam:ListGroups", "*"),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            case: "the sibling action the Deny does not name".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-listgroups".to_string(),
                    statement("Deny", "iam:ListGroups", "*"),
                ),
            ],
            action: "ListRoles",
            iam_action: "iam:ListRoles".to_string(),
            params: Vec::new(),
            resource: role_root.clone(),
            expect: Expect::Served,
        },
        Case {
            case: "a Deny whose action element is a wildcard run".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-list-star".to_string(),
                    statement("Deny", "iam:List*", "*"),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            case: "a Deny on every action, against a user listing".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                ("deny-everything".to_string(), statement("Deny", "*", "*")),
            ],
            action: "ListUsers",
            iam_action: "iam:ListUsers".to_string(),
            params: Vec::new(),
            resource: user_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            // IAM "compares action names case-insensitively", so the spelling of the
            // action element cannot decide whether a Deny applies.
            case: "a Deny spelling the action in another case".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-mixed-case".to_string(),
                    statement("Deny", "IAM:listGROUPS", "*"),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            // One document, the Deny written BEFORE the Allow: the decision does not
            // depend on the order the statements are written in.
            case: "a Deny and an Allow in one document, Deny first".to_string(),
            policies: vec![(
                "deny-then-allow".to_string(),
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"},{"Effect":"Allow","Action":"iam:ListGroups","Resource":"*"}]}"#
                    .to_string(),
            )],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            // `NotAction` matches every action its values do NOT, so a Deny over it
            // refuses the action it does not name.
            case: "a Deny whose NotAction names another action".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-not-listroles".to_string(),
                    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","NotAction":"iam:ListRoles","Resource":"*"}]}"#
                        .to_string(),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            case: "no identity-based policy at all".to_string(),
            policies: Vec::new(),
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::NoAllow,
        },
        Case {
            case: "an Allow for another action only".to_string(),
            policies: vec![(
                "allow-listroles".to_string(),
                statement("Allow", "iam:ListRoles", "*"),
            )],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::NoAllow,
        },
        Case {
            // The resource element decides whether a statement is about the request:
            // this Allow covers the groups under /eng/ and the request addresses them.
            case: "an Allow scoped to the path prefix the request lists".to_string(),
            policies: vec![(
                "allow-eng-groups".to_string(),
                statement("Allow", "iam:ListGroups", &format!("{group_eng}*")),
            )],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: vec![("PathPrefix".to_string(), "/eng/".to_string())],
            resource: group_eng.clone(),
            expect: Expect::Served,
        },
        Case {
            // The same Allow, against the whole collection: a different resource, which
            // that statement is not about, so nothing grants the call.
            case: "an Allow scoped to a path prefix the request does not list"
                .to_string(),
            policies: vec![(
                "allow-eng-groups".to_string(),
                statement("Allow", "iam:ListGroups", &format!("{group_eng}*")),
            )],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::NoAllow,
        },
        Case {
            case: "a Deny scoped to the path prefix the request lists".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-eng-groups".to_string(),
                    statement("Deny", "iam:ListGroups", &format!("{group_eng}*")),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: vec![("PathPrefix".to_string(), "/eng/".to_string())],
            resource: group_eng.clone(),
            expect: Expect::ExplicitDeny,
        },
        Case {
            case: "a Deny scoped to a path prefix the request does not list".to_string(),
            policies: vec![
                ("allow-all-three".to_string(), allow_all_three()),
                (
                    "deny-eng-groups".to_string(),
                    statement("Deny", "iam:ListGroups", &format!("{group_eng}*")),
                ),
            ],
            action: "ListGroups",
            iam_action: "iam:ListGroups".to_string(),
            params: Vec::new(),
            resource: group_root.clone(),
            expect: Expect::Served,
        },
    ]
}

/// Create a user holding `policies`, and answer the access key id a request signs with
/// to speak for it.
async fn principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policies: &[(String, String)],
) -> String {
    let (status, created) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    for (policy_name, document) in policies {
        let (status, put) = call(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture PutUserPolicy {policy_name:?} must succeed, \
             got {status}: {put}"
        );
    }
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// RULE: a call signed by an IAM user is decided against that user's identity-based
/// policies — a `Deny` about the request refuses it whatever any `Allow` says, an `Allow`
/// with no such `Deny` serves it, and a principal with neither is refused — and a
/// refusal is HTTP 403 `AccessDenied`.
#[tokio::test]
async fn identity_policies_decide_every_signed_call() {
    let router = router();

    for (row, case) in cases().into_iter().enumerate() {
        let user_name = format!("caller{row}");
        let access_key_id = principal(&router, &case.case, &user_name, &case.policies).await;
        let params: Vec<(&str, &str)> = case
            .params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();

        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), case.action, &params).await;

        match case.expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the call to be served, got {status}: {body}",
                    case.case
                );
            }
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {:?}: expected the error code AccessDenied, got {:?}",
                    case.case,
                    error_code(&body)
                );
                // The message is built from the request: the principal that signed it,
                // the action it named and the resource the decision was made on, plus
                // which of the two refusals this is.
                let reason = if case.expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    format!(
                        "because no identity-based policy allows the {} action",
                        case.iam_action
                    )
                };
                let expected = format!(
                    "User: arn:aws:iam::{}:user/{user_name} is not authorized to perform: {} \
                     on resource: {} {reason}",
                    account_id(),
                    case.iam_action,
                    case.resource
                );
                assert_eq!(
                    element(&body, "Message"),
                    expected,
                    "case {:?}: expected the refusal to name the principal, action and \
                     resource, got {:?}",
                    case.case,
                    element(&body, "Message")
                );
            }
        }
    }
}

/// RULE: the decision is made from the documents in force at the instant of the call, so
/// putting a `Deny` takes effect on the next call and deleting it restores the `Allow`
/// that was there all along — no state of the caller's is consumed by being refused.
#[tokio::test]
async fn a_deny_takes_effect_and_its_deletion_restores_access() {
    let router = router();
    let user_name = "swinger";
    let access_key_id = principal(
        &router,
        "deny lifecycle",
        user_name,
        &[("allow-both".to_string(), allow_all_three())],
    )
    .await;

    // Three times around the loop: a Deny is not a one-shot, and neither is its
    // deletion. The group listing is asked for twice in each state, so a refusal that
    // only held for the first call after the change would fail here.
    for round in 1..=3 {
        for attempt in 1..=2 {
            let (status, body) =
                call_signed_by(&router, Some(&access_key_id), "ListGroups", &[]).await;
            assert_eq!(
                status, 200,
                "round {round} attempt {attempt}: expected the Allow to serve \
                 ListGroups, got {status}: {body}"
            );
        }

        let (status, put) = call(
            &router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", "deny-listgroups"),
                (
                    "PolicyDocument",
                    &statement("Deny", "iam:ListGroups", "*"),
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "round {round}: PutUserPolicy of the Deny must succeed, got {status}: {put}"
        );

        for attempt in 1..=2 {
            let (status, body) =
                call_signed_by(&router, Some(&access_key_id), "ListGroups", &[]).await;
            assert_eq!(
                status, 403,
                "round {round} attempt {attempt}: expected the Deny to refuse \
                 ListGroups, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "round {round} attempt {attempt}: expected AccessDenied, got {:?}",
                error_code(&body)
            );
        }
        // The action the Deny does not name keeps working while it is in force.
        let (status, body) = call_signed_by(&router, Some(&access_key_id), "ListRoles", &[]).await;
        assert_eq!(
            status, 200,
            "round {round}: expected ListRoles to stay allowed, got {status}: {body}"
        );

        let (status, deleted) = call(
            &router,
            "DeleteUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", "deny-listgroups"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "round {round}: DeleteUserPolicy of the Deny must succeed, got {status}: {deleted}"
        );
    }
}

/// RULE: a credential the account holds no IAM user for carries no identity-based
/// policies, so there is nothing to refuse it on: such a request is served. Only a key
/// bound to a user is judged against that user's documents.
#[tokio::test]
async fn a_credential_no_iam_user_owns_is_not_refused() {
    let router = router();
    // A user whose policies deny everything, so a key bound to it would be refused.
    let denied_key = principal(
        &router,
        "denied principal",
        "locked-out",
        &[("deny-everything".to_string(), statement("Deny", "*", "*"))],
    )
    .await;

    // Rows spanning the credentials the IAM store holds no user for: no Authorization
    // header at all, a key id nothing minted, and the emulator's own ambient key id.
    for signer in [None, Some("AKIDNOTHINGMINTED"), Some("default")] {
        for action in ["ListGroups", "ListRoles", "ListUsers"] {
            let (status, body) = call_signed_by(&router, signer, action, &[]).await;
            assert_eq!(
                status, 200,
                "case {signer:?}/{action}: expected a credential no IAM user owns to be \
                 served, got {status}: {body}"
            );
        }
    }

    // The same operation, signed by the user whose policies deny it, is refused — so the
    // 200s above are about the credential and not about the gate being inert.
    let (status, body) = call_signed_by(&router, Some(&denied_key), "ListGroups", &[]).await;
    assert_eq!(
        status, 403,
        "case locked-out: expected the denied principal to be refused, got {status}: {body}"
    );
}

/// An AWS-managed policy ARN this emulator's catalog does not carry
/// (`crate::state::AWS_MANAGED_POLICIES` holds a subset of AWS's catalog, and
/// `AttachUserPolicy` accepts the ARNs outside it because AWS does). What it grants is
/// not held anywhere, so it is a grant the emulator cannot read.
const UNREADABLE_GRANT: &str = "arn:aws:iam::aws:policy/AWSXRayDaemonWriteAccess";

/// RULE: a grant whose document this emulator cannot read is not evidence either way —
/// it cannot refuse a call, and it cannot overturn an explicit `Deny` that a document
/// the account DOES hold states, because no grant overturns one.
#[tokio::test]
async fn an_unreadable_grant_allows_but_cannot_overturn_an_explicit_deny() {
    let router = router();

    // Rows: the principal carrying only the unreadable grant, and one carrying it
    // beside an explicit Deny on one of the two actions.
    for (user_name, policies, denied_action) in [
        ("unreadable", Vec::new(), None),
        (
            "unreadable-denied",
            vec![(
                "deny-listgroups".to_string(),
                statement("Deny", "iam:ListGroups", "*"),
            )],
            Some("ListGroups"),
        ),
    ] {
        let access_key_id = principal(&router, user_name, user_name, &policies).await;
        let (status, attached) = call(
            &router,
            "AttachUserPolicy",
            &[("UserName", user_name), ("PolicyArn", UNREADABLE_GRANT)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user_name}: the fixture AttachUserPolicy must succeed, got {status}: {attached}"
        );

        for action in ["ListGroups", "ListRoles"] {
            let (status, body) = call_signed_by(&router, Some(&access_key_id), action, &[]).await;
            let expected = if denied_action == Some(action) { 403 } else { 200 };
            assert_eq!(
                status, expected,
                "case {user_name}/{action}: expected {expected}, got {status}: {body}"
            );
            if expected == 403 {
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {user_name}/{action}: expected AccessDenied, got {:?}",
                    error_code(&body)
                );
                assert!(
                    element(&body, "Message")
                        .ends_with("with an explicit deny in an identity-based policy"),
                    "case {user_name}/{action}: expected the explicit-deny refusal, got {:?}",
                    element(&body, "Message")
                );
            }
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What a permissions boundary decides about a signed call to IAM, end to end over the
//! awsQuery wire.
//!
//! RULE: a permissions boundary attached to an IAM identity sets the MAXIMUM permissions
//! that identity's identity-based policies can grant it — "the permissions boundary sets
//! the maximum permissions that the entity can have"
//! (`SimulatePrincipalPolicy$PermissionsBoundaryPolicyInputList`) — so a signed call is
//! permitted only where the identity-based documents allow it AND the boundary allows it
//! too. The boundary is a ceiling, never a source of permission: a call only it allows is
//! refused exactly as one nothing allows is, and an explicit `Deny` in either document
//! refuses whatever the other says. A refusal is HTTP 403 with the awsQuery error code
//! `AccessDenied`, and it names the document that withheld the grant: the boundary where
//! the identity-based policies did allow the call, and those policies where nothing
//! allowed it at all. Where a statement OF the boundary explicitly denied the call, the
//! clause also names the boundary — by the ARN of the policy that is the boundary, the
//! same value `GetUser` reports under `PermissionsBoundary.PermissionsBoundaryArn` —
//! because in an account carrying several boundaries that is the only thing in the message
//! identifying which ceiling refused the call.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! document grants and which denies, whether the boundary states the action literally,
//! as a wildcard run, in another case or as a list, the resource the boundary scopes the
//! action to, whether a boundary is attached at all, and which operation is called.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{collection, statement};
use crate::policy_simulation::{reported, simulate, ALLOWED, IMPLICIT_DENY};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// A document carrying one statement per `(Effect, Action, Resource)` triple, in the
/// order written — the several-statement spelling of
/// [`crate::identity_policy_enforcement::statement`], which every row needing a single
/// statement takes instead.
///
/// `pub(crate)` because every suite that layers a grant, a ceiling and a deny in one
/// document builds it this one way
/// (`tests/all/boundary_refusal_message_wording.rs`).
pub(crate) fn statements(parts: &[(&str, &str, &str)]) -> String {
    let written: Vec<String> = parts
        .iter()
        .map(|(effect, action, resource)| {
            format!(r#"{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}"#)
        })
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        written.join(",")
    )
}

/// The ARN of an IAM user of this account at the default path, the resource a call
/// addressing that user by name is decided against (`arn:aws:iam::<account>:user/<name>`).
fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// The ARN of an IAM collection of this account at the root path, the resource an
/// account-wide list read is decided against — the one spelling
/// `tests/all/identity_policy_enforcement.rs` owns.
fn root_collection(resource_type: &str) -> String {
    collection(resource_type, "/")
}

/// Create `user_name`, embed `identity` in it as inline policies, attach the boundary
/// `boundary` names (as a customer-managed policy this account creates for it) and mint
/// an access key — answering the key id a request signs with to speak for the user.
async fn bounded_principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    identity: &[(&str, String)],
    boundary: Option<&str>,
) -> String {
    let (status, created) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {created}"
    );
    for (policy_name, document) in identity {
        put_user_policy(router, case, user_name, policy_name, document).await;
    }
    if let Some(document) = boundary {
        attach_boundary(router, case, user_name, &format!("{user_name}-ceiling"), document).await;
    }
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// Embed one inline policy in `user_name`.
async fn put_user_policy(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policy_name: &str,
    document: &str,
) {
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
        "case {case:?}: the fixture PutUserPolicy {policy_name:?} must succeed, got \
         {status}: {put}"
    );
}

/// Create the managed policy `policy_name` carrying `document` and set it as
/// `user_name`'s permissions boundary, answering its ARN.
///
/// `pub(crate)` because every suite that puts a caller under a ceiling of its own writing
/// builds that fixture this one way
/// (`tests/all/boundary_refusal_message_wording.rs`).
pub(crate) async fn attach_boundary(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreatePolicy {policy_name:?} must succeed, got \
         {status}: {created}"
    );
    let arn = element(&created, "Arn");
    let (status, put) = call(
        router,
        "PutUserPermissionsBoundary",
        &[("UserName", user_name), ("PermissionsBoundary", &arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutUserPermissionsBoundary must succeed, got \
         {status}: {put}"
    );
    // The boundary is in force because the account PERSISTED it, which is what the
    // reporting half of the put answers for.
    let (status, fetched) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture GetUser must succeed, got {status}: {fetched}"
    );
    assert_eq!(
        elements(&fetched, "PermissionsBoundaryArn"),
        vec![arn.clone()],
        "case {case:?}: expected the user to carry the boundary {arn:?}, got {:?}",
        elements(&fetched, "PermissionsBoundaryArn")
    );
    arn
}

/// The permissions boundary the account holds on `user_name`, read back the way a caller
/// reads it: the ARN `GetUser` reports under `PermissionsBoundary.PermissionsBoundaryArn`.
async fn boundary_arn(router: &MultiServiceRouter, case: &str, user_name: &str) -> String {
    let (status, fetched) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture GetUser must succeed, got {status}: {fetched}"
    );
    let reported = elements(&fetched, "PermissionsBoundaryArn");
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the user to carry one permissions boundary, got {reported:?}"
    );
    reported.into_iter().next().expect("one boundary ARN")
}

/// What a row requires of the signed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the identity-based policies grant the call and the boundary allows it.
    Served,
    /// HTTP 403 `AccessDenied` because a statement of an identity-based policy denies it.
    IdentityDeny,
    /// HTTP 403 `AccessDenied` because a statement of the boundary denies it.
    BoundaryDeny,
    /// HTTP 403 `AccessDenied` because nothing at all allows it — which includes a call
    /// only the boundary itself allows, the boundary being no grant.
    NoAllow,
    /// HTTP 403 `AccessDenied` because the identity-based policies DID allow it and the
    /// boundary withheld it: the document that refused it is the boundary, so the refusal
    /// names that one.
    BoundaryNoAllow,
}

impl Expect {
    /// The clause AWS's `AccessDenied` message carries for this outcome, for the `action`
    /// the call named and the permissions boundary `boundary` in force on the caller.
    fn reason(self, action: &str, boundary: &str) -> String {
        match self {
            Self::Served => String::new(),
            Self::IdentityDeny => "with an explicit deny in an identity-based policy".to_string(),
            Self::BoundaryDeny => {
                format!("with an explicit deny in a permissions boundary: {boundary}")
            }
            Self::NoAllow => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::BoundaryNoAllow => {
                format!("because no permissions boundary allows the {action} action")
            }
        }
    }
}

/// One row: the documents in force on the calling user, the call it makes and the answer
/// the rule requires.
struct Case {
    case: &'static str,
    /// The inline policy documents embedded in the caller.
    identity: Vec<String>,
    /// The boundary document attached to the caller, or `None` for a caller carrying no
    /// boundary at all.
    boundary: Option<String>,
    /// The awsQuery `Action` the call names, and the IAM action the decision is made on.
    action: &'static str,
    iam_action: &'static str,
    /// Whether the call carries the caller's own `UserName` (the operations that address
    /// one entity) — which is also how a refusal names the resource.
    names_user: bool,
    expect: Expect,
}

/// The rows. Every row runs against a fresh user, so its documents are the only ones in
/// force when it calls.
fn cases() -> Vec<Case> {
    let any = "*";
    let other_user = user_arn("someone-else");
    vec![
        Case {
            case: "both documents allow the action",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statement("Allow", "iam:ListAccessKeys", any)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // The boundary is not a grant: it allows this action and the identity-based
            // policy allows only another, so nothing grants the call.
            case: "only the boundary allows the action",
            identity: vec![statement("Allow", "iam:ListUserTags", any)],
            boundary: Some(statements(&[
                ("Allow", "iam:ListUserTags", any),
                ("Allow", "iam:ListAccessKeys", any),
            ])),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::NoAllow,
        },
        Case {
            // The identity-based grant stands only as far as the ceiling reaches, and
            // the boundary is then the document that withheld it.
            case: "only the identity-based policy allows the action",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statement("Allow", "iam:ListUserTags", any)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::BoundaryNoAllow,
        },
        Case {
            case: "the boundary denies an action the identity-based policy allows",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statements(&[
                ("Allow", any, any),
                ("Deny", "iam:ListAccessKeys", any),
            ])),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::BoundaryDeny,
        },
        Case {
            // The same explicit boundary deny on ANOTHER operation and another ceiling,
            // so the ARN the clause names is the boundary of the caller being refused
            // rather than one document's.
            case: "the boundary denies a tag read the identity-based policy allows",
            identity: vec![statement("Allow", "iam:ListUserTags", any)],
            boundary: Some(statements(&[
                ("Allow", any, any),
                ("Deny", "iam:ListUserTags", any),
            ])),
            action: "ListUserTags",
            iam_action: "iam:ListUserTags",
            names_user: true,
            expect: Expect::BoundaryDeny,
        },
        Case {
            case: "the identity-based policy denies an action the boundary allows",
            identity: vec![statements(&[
                ("Allow", any, any),
                ("Deny", "iam:ListAccessKeys", any),
            ])],
            boundary: Some(statement("Allow", any, any)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::IdentityDeny,
        },
        Case {
            // The control: with no boundary attached there is no ceiling, so the
            // identity-based grant alone decides.
            case: "no boundary is attached at all",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: None,
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // A boundary's `Resource` element decides which resources it leaves within
            // reach, so a grant on another user's keys caps this one to nothing.
            case: "the boundary allows the action only on another resource",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statement("Allow", "iam:ListAccessKeys", &other_user)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::BoundaryNoAllow,
        },
        Case {
            case: "the boundary allows the action through a wildcard run",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statement("Allow", "iam:List*", any)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // IAM "compares action names case-insensitively", so the case the boundary
            // spells its action in cannot decide whether the ceiling reaches the call.
            case: "the boundary spells the action in another case",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(statement("Allow", "IAM:listaccesskeys", any)),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // An `Action` element is either one string or a LIST of them: a boundary
            // naming the action among several allows it.
            case: "the boundary names the action in an action list",
            identity: vec![statement("Allow", "iam:ListAccessKeys", any)],
            boundary: Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListUserTags","iam:ListAccessKeys"],"Resource":"*"}]}"#
                    .to_string(),
            ),
            action: "ListAccessKeys",
            iam_action: "iam:ListAccessKeys",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // The same rule on another operation that names one entity, so what is
            // measured is the ceiling and not one handler.
            case: "a tag read the boundary does not reach",
            identity: vec![statement("Allow", "iam:ListUserTags", any)],
            boundary: Some(statement("Allow", "iam:ListAccessKeys", any)),
            action: "ListUserTags",
            iam_action: "iam:ListUserTags",
            names_user: true,
            expect: Expect::BoundaryNoAllow,
        },
        Case {
            case: "a tag read both documents allow",
            identity: vec![statement("Allow", "iam:ListUserTags", any)],
            boundary: Some(statement("Allow", "iam:ListUserTags", any)),
            action: "ListUserTags",
            iam_action: "iam:ListUserTags",
            names_user: true,
            expect: Expect::Served,
        },
        Case {
            // And on an ACCOUNT-WIDE read, whose resource is the collection rather than
            // one entity: the boundary is scoped to that same collection here.
            case: "an account-wide read the boundary reaches",
            identity: vec![statement("Allow", "iam:ListGroups", any)],
            boundary: Some(statement(
                "Allow",
                "iam:ListGroups",
                &root_collection("group"),
            )),
            action: "ListGroups",
            iam_action: "iam:ListGroups",
            names_user: false,
            expect: Expect::Served,
        },
        Case {
            case: "an account-wide read the boundary does not reach",
            identity: vec![statement("Allow", "iam:ListGroups", any)],
            boundary: Some(statement("Allow", "iam:ListUsers", any)),
            action: "ListGroups",
            iam_action: "iam:ListGroups",
            names_user: false,
            expect: Expect::BoundaryNoAllow,
        },
    ]
}

/// RULE: a signed call is permitted only where the caller's identity-based policies allow
/// it AND the permissions boundary in force on the caller allows it too — the boundary
/// caps those policies and grants nothing of its own — and an explicit `Deny` in either
/// document refuses the call whatever the other says.
#[tokio::test]
async fn the_boundary_caps_identity_grants_and_never_grants_one() {
    let router = router();

    for (row, case) in cases().into_iter().enumerate() {
        let user_name = format!("bounded{row}");
        let identity: Vec<(&str, String)> = case
            .identity
            .iter()
            .enumerate()
            .map(|(at, document)| {
                (
                    ["identity-1", "identity-2"][at.min(1)],
                    document.clone(),
                )
            })
            .collect();
        let access_key_id = bounded_principal(
            &router,
            case.case,
            &user_name,
            &identity,
            case.boundary.as_deref(),
        )
        .await;

        let params: Vec<(&str, &str)> = if case.names_user {
            vec![("UserName", user_name.as_str())]
        } else {
            Vec::new()
        };
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), case.action, &params).await;

        if case.expect == Expect::Served {
            assert_eq!(
                status, 200,
                "case {:?}: expected the call to be served, got {status}: {body}",
                case.case
            );
            continue;
        }
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
        // The message is built from the request: the principal that signed it, the action
        // it named, the resource the decision was made on — named as the request named it
        // for a call addressing one entity — and which of the four refusals this is,
        // naming the document that withheld the grant.
        let named_as = if case.names_user {
            format!("user {user_name}")
        } else {
            root_collection("group")
        };
        // The boundary the account holds on the caller: the ARN a refusal explained by an
        // explicit deny IN it names, read back from the account rather than rebuilt here.
        let boundary = match &case.boundary {
            Some(_) => boundary_arn(&router, case.case, &user_name).await,
            None => String::new(),
        };
        assert_access_denied_message(
            case.case,
            &element(&body, "Message"),
            &user_arn(&user_name),
            case.iam_action,
            &named_as,
            &case.expect.reason(case.iam_action, &boundary),
        );
    }
}

/// RULE: the decision is made from the documents in force at the instant of the call, so
/// a boundary-only permission stays refused until an identity-based policy grants it,
/// becomes available while that policy is in force, and is refused again once it is
/// deleted — the boundary itself never granting it, and never taking away what it does
/// allow.
#[tokio::test]
async fn a_boundary_only_permission_is_refused_until_a_policy_grants_it() {
    let router = router();
    let user_name = "ceilinged";
    let case = "the boundary-only permission";
    // The boundary admits both reads; the identity-based policy admits only the tag read.
    let access_key_id = bounded_principal(
        &router,
        case,
        user_name,
        &[("baseline", statement("Allow", "iam:ListUserTags", &user_arn(user_name)))],
        Some(&statements(&[
            ("Allow", "iam:ListUserTags", "*"),
            ("Allow", "iam:ListAccessKeys", "*"),
        ])),
    )
    .await;
    let params = [("UserName", user_name)];

    // Three times around: neither the refusal nor the grant is a one-shot, and the
    // boundary is attached and unchanged throughout.
    for round in 1..=3 {
        // The read BOTH documents allow works in every state below, so the rows that
        // follow are about the one action only the boundary allows.
        let (status, tags) =
            call_signed_by(&router, Some(&access_key_id), "ListUserTags", &params).await;
        assert_eq!(
            status, 200,
            "round {round}: expected the baseline grant to be served, got {status}: {tags}"
        );

        let (status, refused) =
            call_signed_by(&router, Some(&access_key_id), "ListAccessKeys", &params).await;
        assert_eq!(
            status, 403,
            "round {round}: expected the boundary-only permission to be refused, got \
             {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDenied",
            "round {round}: expected AccessDenied, got {:?}",
            error_code(&refused)
        );
        // Nothing GRANTED this call — the boundary allows it but is no grant — so the
        // refusal names the identity-based policies as the documents that allow nothing
        // here, not the boundary.
        assert_access_denied_message(
            &format!("round {round}"),
            &element(&refused, "Message"),
            &user_arn(user_name),
            "iam:ListAccessKeys",
            &format!("user {user_name}"),
            "because no identity-based policy allows the iam:ListAccessKeys action",
        );

        // The identity-based grant the boundary already admits: now something grants the
        // call, and the ceiling leaves it standing.
        put_user_policy(
            &router,
            case,
            user_name,
            "granted",
            &statement("Allow", "iam:ListAccessKeys", &user_arn(user_name)),
        )
        .await;
        let (status, served) =
            call_signed_by(&router, Some(&access_key_id), "ListAccessKeys", &params).await;
        assert_eq!(
            status, 200,
            "round {round}: expected the granted call to be served, got {status}: {served}"
        );
        assert_eq!(
            elements(&served, "AccessKeyId"),
            vec![access_key_id.clone()],
            "round {round}: expected the signing user's own key, got {:?}",
            elements(&served, "AccessKeyId")
        );

        let (status, deleted) = call(
            &router,
            "DeleteUserPolicy",
            &[("UserName", user_name), ("PolicyName", "granted")],
        )
        .await;
        assert_eq!(
            status, 200,
            "round {round}: the fixture DeleteUserPolicy must succeed, got {status}: {deleted}"
        );
    }
}

/// RULE: the ceiling in force is the one attached NOW, so replacing the boundary with one
/// that admits the action makes a grant that was capped effective — and replacing it with
/// one that does not caps it again. Replacing it never grants anything by itself.
#[tokio::test]
async fn the_ceiling_in_force_is_the_boundary_attached_now() {
    let router = router();
    let user_name = "recapped";
    let case = "the replaced boundary";
    let access_key_id = bounded_principal(
        &router,
        case,
        user_name,
        &[("granted", statement("Allow", "iam:ListAccessKeys", "*"))],
        Some(&statement("Allow", "iam:ListUserTags", "*")),
    )
    .await;
    let params = [("UserName", user_name)];

    // Rows: the ceiling that does not admit the action, one that does, and back again —
    // each a boundary of its own, so what moves is which policy is attached.
    for (index, (admits, expected)) in [
        ("iam:ListUserTags", 403),
        ("iam:ListAccessKeys", 200),
        ("iam:ListGroups", 403),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            attach_boundary(
                &router,
                case,
                user_name,
                &format!("{user_name}-ceiling-{index}"),
                &statement("Allow", admits, "*"),
            )
            .await;
        }
        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "ListAccessKeys", &params).await;
        assert_eq!(
            status, expected,
            "case {admits:?}: expected HTTP {expected} under a boundary admitting \
             {admits}, got {status}: {body}"
        );
        if expected == 403 {
            // The identity-based policy allows this call throughout, so the document
            // that withheld it is the boundary, and the refusal names that one.
            assert_access_denied_message(
                admits,
                &element(&body, "Message"),
                &user_arn(user_name),
                "iam:ListAccessKeys",
                &format!("user {user_name}"),
                "because no permissions boundary allows the iam:ListAccessKeys action",
            );
        }
    }
}

/// RULE: `SimulatePrincipalPolicy` reports the same ceiling the authorizer applies: with
/// no boundary supplied it simulates the one in force on the entity, reporting
/// `AllowedByPermissionsBoundary` for it and the implicit deny for an action it withholds;
/// a boundary supplied in the request is simulated INSTEAD of the attached one; and an
/// entity carrying none, asked with none, reports no boundary detail at all.
#[tokio::test]
async fn the_simulation_reports_the_boundary_in_force_on_the_entity() {
    let router = router();
    let case = "the simulated ceiling";
    let user_name = "simulated";
    bounded_principal(
        &router,
        case,
        user_name,
        &[(
            "granted",
            statements(&[
                ("Allow", "iam:ListUserTags", "*"),
                ("Allow", "iam:ListAccessKeys", "*"),
            ]),
        )],
        Some(&statement("Allow", "iam:ListUserTags", "*")),
    )
    .await;
    let arn = user_arn(user_name);
    let actions = ["iam:ListUserTags", "iam:ListAccessKeys"];

    // The attached boundary admits the first action and withholds the second, so the
    // decisions and the boundary's own verdicts differ per action.
    let (status, body) = simulate(&router, &arn, &actions, &[], &[]).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            ("iam:ListUserTags".to_string(), ALLOWED.to_string()),
            ("iam:ListAccessKeys".to_string(), IMPLICIT_DENY.to_string()),
        ],
        "case {case:?}: expected the attached boundary to cap the second action, got {body}"
    );
    assert_eq!(
        elements(&body, "AllowedByPermissionsBoundary"),
        vec!["true".to_string(), "false".to_string()],
        "case {case:?}: expected the attached boundary's own verdict per action, got {:?}",
        elements(&body, "AllowedByPermissionsBoundary")
    );

    // "If a permissions boundary is attached to an entity and you pass in a different
    // permissions boundary policy using this parameter, then the new permissions boundary
    // policy is used for the simulation": the supplied one replaces it, so the verdicts
    // swap.
    let (status, body) = simulate(
        &router,
        &arn,
        &actions,
        &[],
        &[(
            "PermissionsBoundaryPolicyInputList.member.1".to_string(),
            statement("Allow", "iam:ListAccessKeys", "*"),
        )],
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            ("iam:ListUserTags".to_string(), IMPLICIT_DENY.to_string()),
            ("iam:ListAccessKeys".to_string(), ALLOWED.to_string()),
        ],
        "case {case:?}: expected the supplied boundary to replace the attached one, got {body}"
    );
    assert_eq!(
        elements(&body, "AllowedByPermissionsBoundary"),
        vec!["false".to_string(), "true".to_string()],
        "case {case:?}: expected the supplied boundary's verdict per action, got {:?}",
        elements(&body, "AllowedByPermissionsBoundary")
    );

    // An entity carrying no boundary, asked with none: no ceiling took part in the
    // simulation, so no result reports one.
    let unbounded = "unbounded";
    bounded_principal(
        &router,
        case,
        unbounded,
        &[(
            "granted",
            statements(&[
                ("Allow", "iam:ListUserTags", "*"),
                ("Allow", "iam:ListAccessKeys", "*"),
            ]),
        )],
        None,
    )
    .await;
    let (status, body) = simulate(&router, &user_arn(unbounded), &actions, &[], &[]).await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            ("iam:ListUserTags".to_string(), ALLOWED.to_string()),
            ("iam:ListAccessKeys".to_string(), ALLOWED.to_string()),
        ],
        "case {case:?}: expected an unbounded entity's grants to stand, got {body}"
    );
    assert!(
        elements(&body, "AllowedByPermissionsBoundary").is_empty(),
        "case {case:?}: expected no boundary detail for an entity carrying none, got {body}"
    );
}

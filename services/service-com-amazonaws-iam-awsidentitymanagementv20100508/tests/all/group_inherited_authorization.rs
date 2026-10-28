// SPDX-License-Identifier: CC0-1.0

//! The policies a GROUP carries are in force on its members' signed calls.
//!
//! The rule under test: a signed call to IAM is decided against the UNION of the
//! identity-based documents attached to the signing user and those attached to every
//! group that user belongs to — inline and managed alike — and that union is decided the
//! way IAM's documented evaluation decides one set of documents: a statement about the
//! request that says `Deny` refuses it whatever any `Allow` says, an `Allow` with no such
//! `Deny` permits, and a request nothing in the union is about is refused as the implicit
//! deny. So a guardrail written as a `Deny` on a group refuses every member's call, and a
//! grant written as an `Allow` on a group permits one, with neither reachable from the
//! user's own documents.
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: where
//! the deciding document is attached (the user, the first group it belongs to, the
//! second), how it is attached (inline, or as the default version of a managed policy),
//! whether its action element is a literal or a wildcard, which resource it is scoped to,
//! and which operation is called — a read of one named user, a read of that user's tags,
//! and a read of a collection. Beside them, the invariant: the union follows the
//! MEMBERSHIP, so taking the user out of the group takes the group's `Deny` out of force
//! and putting it back puts the `Deny` back.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the fixture
//! principal the one `tests/all/identity_policy_enforcement.rs` owns, and the simulation
//! request spelling the one `tests/all/policy_simulation.rs` owns, so a request is built
//! and read exactly one way across this crate.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{collection, principal, statement};
use crate::policy_simulation::{simulate, EXPLICIT_DENY};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, error_code, router,
};

/// What a row expects the signed call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the union of the documents in force permits the call.
    Served,
    /// HTTP 403 `AccessDenied` because a statement in that union says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because nothing in that union says `Allow`.
    NoAllow,
}

/// Where one of a row's documents is attached, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attached {
    /// An inline policy embedded in the calling user (`PutUserPolicy`).
    UserInline,
    /// An inline policy embedded in the group at this index (`PutGroupPolicy`).
    GroupInline(usize),
    /// The default version of a managed policy attached to the group at this index
    /// (`CreatePolicy` + `AttachGroupPolicy`).
    GroupManaged(usize),
}

/// One document of a row: where it is attached and what it says.
struct Document {
    attached: Attached,
    /// The policy name it is stored under — an inline policy's own name, or the friendly
    /// name of the managed policy created to carry it.
    name: &'static str,
    /// The document itself.
    document: String,
}

/// The resource a row's call is decided on, as a refusal NAMES it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    /// The calling user itself, which a refusal names by type and name (`user Bob`).
    Caller,
    /// Every group of the account at the root path, which a refusal names by ARN.
    GroupCollection,
}

/// One row: how many groups the caller belongs to, the documents in force across the
/// caller and those groups, the call it makes, and the answer the rule requires.
struct Case {
    case: &'static str,
    /// How many groups the caller is made a member of. Every one of them is created and
    /// joined whether or not a document is attached to it.
    groups: usize,
    /// The row's documents, built from the calling user's own ARN — so a statement can
    /// scope itself to the caller without the row having to predict its name.
    documents: fn(&str) -> Vec<Document>,
    /// The awsQuery `Action` the call names.
    action: &'static str,
    /// Whether the request carries `UserName=<the caller>` (the operations above that
    /// take one).
    names_caller: bool,
    /// The IAM action the decision is made on.
    iam_action: &'static str,
    resource: Resource,
    expect: Expect,
}

/// An `Allow` of both single-user reads this suite calls, on every resource: the grant a
/// row about a `Deny` is layered on top of, so the row demonstrates the `Deny` and not a
/// missing grant.
const ALLOW_BOTH_READS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GetUser","iam:ListUserTags"],"Resource":"*"}]}"#;

/// The caller's own `Allow` for both single-user reads, as a row's first document.
fn own_allow() -> Document {
    Document {
        attached: Attached::UserInline,
        name: "own-allow",
        document: ALLOW_BOTH_READS.to_string(),
    }
}

/// The rows. Each runs against its own fresh user and its own fresh groups, so the
/// documents a row attaches are the only ones in force when it calls.
const CASES: &[Case] = &[
    // The reported case: the user's own `Allow` covers the call, and the group it
    // belongs to carries a `Deny` naming that one action on that one resource.
    Case {
        case: "a group's inline Deny over the user's own Allow",
        groups: 1,
        documents: |caller| {
            vec![
                own_allow(),
                Document {
                    attached: Attached::GroupInline(0),
                    name: "group-deny-getuser",
                    document: statement("Deny", "iam:GetUser", caller),
                },
            ]
        },
        action: "GetUser",
        names_caller: true,
        iam_action: "iam:GetUser",
        resource: Resource::Caller,
        expect: Expect::ExplicitDeny,
    },
    // The same documents, on the action the `Deny` does NOT name: the guardrail is about
    // one action, so the other read stays permitted.
    Case {
        case: "the action a group's Deny does not name",
        groups: 1,
        documents: |caller| {
            vec![
                own_allow(),
                Document {
                    attached: Attached::GroupInline(0),
                    name: "group-deny-getuser",
                    document: statement("Deny", "iam:GetUser", caller),
                },
            ]
        },
        action: "ListUserTags",
        names_caller: true,
        iam_action: "iam:ListUserTags",
        resource: Resource::Caller,
        expect: Expect::Served,
    },
    // The other direction: the only `Allow` is the group's, so the call is permitted by a
    // document the user itself does not carry.
    Case {
        case: "a grant reached only through the group",
        groups: 1,
        documents: |_| {
            vec![Document {
                attached: Attached::GroupInline(0),
                name: "group-allow-getuser",
                document: statement("Allow", "iam:GetUser", "*"),
            }]
        },
        action: "GetUser",
        names_caller: true,
        iam_action: "iam:GetUser",
        resource: Resource::Caller,
        expect: Expect::Served,
    },
    // The `Deny` is carried by the SECOND group the user belongs to, so the union is over
    // every membership rather than over the first one.
    Case {
        case: "a Deny carried by the second group the user belongs to",
        groups: 2,
        documents: |_| {
            vec![
                own_allow(),
                Document {
                    attached: Attached::GroupInline(0),
                    name: "group-allow-roles",
                    document: statement("Allow", "iam:ListRoles", "*"),
                },
                Document {
                    attached: Attached::GroupInline(1),
                    name: "group-deny-tags",
                    document: statement("Deny", "iam:ListUserTags", "*"),
                },
            ]
        },
        action: "ListUserTags",
        names_caller: true,
        iam_action: "iam:ListUserTags",
        resource: Resource::Caller,
        expect: Expect::ExplicitDeny,
    },
    // A wildcard action element in the group's `Deny` is about the request too.
    Case {
        case: "a group's Deny whose action element is a wildcard",
        groups: 1,
        documents: |_| {
            vec![
                own_allow(),
                Document {
                    attached: Attached::GroupInline(0),
                    name: "group-deny-gets",
                    document: statement("Deny", "iam:Get*", "*"),
                },
            ]
        },
        action: "GetUser",
        names_caller: true,
        iam_action: "iam:GetUser",
        resource: Resource::Caller,
        expect: Expect::ExplicitDeny,
    },
    // A group `Deny` scoped to ANOTHER resource is not about this request, so it refuses
    // nothing: the union is evaluated, not merely scanned for the effect.
    Case {
        case: "a group's Deny scoped to another user",
        groups: 1,
        documents: |_| {
            vec![
                own_allow(),
                Document {
                    attached: Attached::GroupInline(0),
                    name: "group-deny-elsewhere",
                    document: statement("Deny", "iam:GetUser", "arn:aws:iam::*:user/somebody-else"),
                },
            ]
        },
        action: "GetUser",
        names_caller: true,
        iam_action: "iam:GetUser",
        resource: Resource::Caller,
        expect: Expect::Served,
    },
    // The deciding document is the default version of a MANAGED policy attached to the
    // group, on a collection read rather than a single-entity one.
    Case {
        case: "a managed policy attached to the group carrying the Deny",
        groups: 1,
        documents: |_| {
            vec![
                Document {
                    attached: Attached::UserInline,
                    name: "own-allow-groups",
                    document: statement("Allow", "iam:ListGroups", "*"),
                },
                Document {
                    attached: Attached::GroupManaged(0),
                    name: "DenyListGroups",
                    document: statement("Deny", "iam:ListGroups", "*"),
                },
            ]
        },
        action: "ListGroups",
        names_caller: false,
        iam_action: "iam:ListGroups",
        resource: Resource::GroupCollection,
        expect: Expect::ExplicitDeny,
    },
    // And a managed policy attached to the group can be the only grant.
    Case {
        case: "a managed policy attached to the group carrying the only Allow",
        groups: 1,
        documents: |_| {
            vec![Document {
                attached: Attached::GroupManaged(0),
                name: "AllowListGroups",
                document: statement("Allow", "iam:ListGroups", "*"),
            }]
        },
        action: "ListGroups",
        names_caller: false,
        iam_action: "iam:ListGroups",
        resource: Resource::GroupCollection,
        expect: Expect::Served,
    },
    // Membership is not itself a grant: a group whose documents say nothing about the
    // request leaves the implicit deny in place.
    Case {
        case: "a group whose documents say nothing about the request",
        groups: 1,
        documents: |_| {
            vec![Document {
                attached: Attached::GroupInline(0),
                name: "group-allow-roles",
                document: statement("Allow", "iam:ListRoles", "*"),
            }]
        },
        action: "GetUser",
        names_caller: true,
        iam_action: "iam:GetUser",
        resource: Resource::Caller,
        expect: Expect::NoAllow,
    },
];

/// RULE: the call is decided against the union of the caller's own documents and those of
/// every group it belongs to, by IAM's documented evaluation of that union.
#[tokio::test]
async fn a_groups_policies_decide_its_members_signed_calls() {
    for (row, case) in CASES.iter().enumerate() {
        // One router per row, so a row's groups and documents are the only ones the
        // account holds while it calls.
        let router = router();
        let user_name = format!("member{row}");
        let user_arn = format!("arn:aws:iam::{}:user/{user_name}", account_id());
        let access_key_id = principal(&router, case.case, &user_name, &[]).await;
        let group_names: Vec<String> = (0..case.groups)
            .map(|index| format!("guardrail{row}_{index}"))
            .collect();
        for group_name in &group_names {
            create_group(&router, case.case, group_name).await;
            add_user_to_group(&router, case.case, group_name, &user_name).await;
        }
        for document in (case.documents)(&user_arn) {
            attach(&router, case.case, &user_name, &group_names, &document).await;
        }

        let params: Vec<(&str, &str)> = if case.names_caller {
            vec![("UserName", user_name.as_str())]
        } else {
            Vec::new()
        };
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
                let reason = if case.expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    format!(
                        "because no identity-based policy allows the {} action",
                        case.iam_action
                    )
                };
                let named_as = match case.resource {
                    Resource::Caller => format!("user {user_name}"),
                    Resource::GroupCollection => collection("group", "/"),
                };
                assert_access_denied_message(
                    case.case,
                    &element(&body, "Message"),
                    &user_arn,
                    case.iam_action,
                    &named_as,
                    &reason,
                );
            }
        }
    }
}

/// One step of the membership table: whether the user is a member after it, and the
/// status the unchanged request must then answer.
struct Step {
    case: &'static str,
    /// The membership write that gets there, or `None` for the state before any write.
    operation: Option<&'static str>,
    expected: u16,
}

/// RULE as an invariant: the union follows the MEMBERSHIP at the instant of the call, so
/// the same unchanged request is refused while the user is in the group carrying the
/// `Deny`, served once it is taken out, and refused again once it is put back.
#[tokio::test]
async fn the_groups_deny_is_in_force_exactly_while_the_user_is_a_member() {
    let router = router();
    let user_name = "revolving";
    let group_name = "revolving-door";
    let user_arn = format!("arn:aws:iam::{}:user/{user_name}", account_id());
    let access_key_id = principal(
        &router,
        "revolving membership",
        user_name,
        &[("own-allow".to_string(), ALLOW_BOTH_READS.to_string())],
    )
    .await;
    create_group(&router, "revolving membership", group_name).await;
    put_group_policy(
        &router,
        "revolving membership",
        group_name,
        "group-deny-getuser",
        &statement("Deny", "iam:GetUser", &user_arn),
    )
    .await;

    let steps: &[Step] = &[
        Step {
            case: "before the user joins the group",
            operation: None,
            expected: 200,
        },
        Step {
            case: "while the user is a member",
            operation: Some("AddUserToGroup"),
            expected: 403,
        },
        Step {
            case: "after the user is removed",
            operation: Some("RemoveUserFromGroup"),
            expected: 200,
        },
        Step {
            case: "after the user re-joins",
            operation: Some("AddUserToGroup"),
            expected: 403,
        },
    ];
    for step in steps {
        let case = step.case;
        if let Some(operation) = step.operation {
            let (status, body) = call(
                &router,
                operation,
                &[("GroupName", group_name), ("UserName", user_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture {operation} must succeed, got {status}: {body}"
            );
        }

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "GetUser",
            &[("UserName", user_name)],
        )
        .await;
        assert_eq!(
            status, step.expected,
            "case {case:?}: expected HTTP {} for the unchanged request, got {status}: {body}",
            step.expected
        );
        if step.expected == 403 {
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}",
                error_code(&body)
            );
        }
    }
}

/// RULE for the simulator that reports the same decision: a `Deny` reached through a
/// group is reported as `explicitDeny`, the value the live call is refused on, for both
/// places a group can carry one.
#[tokio::test]
async fn the_simulator_reports_a_groups_deny_as_an_explicit_deny() {
    // Rows: the group's `Deny` carried inline, and carried by a managed policy attached
    // to the group.
    for (row, (case, managed)) in [
        ("an inline policy of the group", false),
        ("a managed policy attached to the group", true),
    ]
    .into_iter()
    .enumerate()
    {
        let router = router();
        let user_name = format!("simulated{row}");
        let group_name = format!("simulated-guardrail{row}");
        let user_arn = format!("arn:aws:iam::{}:user/{user_name}", account_id());
        principal(
            &router,
            case,
            &user_name,
            &[("own-allow".to_string(), ALLOW_BOTH_READS.to_string())],
        )
        .await;
        create_group(&router, case, &group_name).await;
        add_user_to_group(&router, case, &group_name, &user_name).await;
        let deny = statement("Deny", "iam:GetUser", &user_arn);
        if managed {
            let policy_arn =
                create_policy(&router, case, &format!("DenyGetUser{row}"), &deny).await;
            attach_group_policy(&router, case, &group_name, &policy_arn).await;
        } else {
            put_group_policy(&router, case, &group_name, "group-deny-getuser", &deny).await;
        }

        let (status, body) =
            simulate(&router, &user_arn, &["iam:GetUser"], &[&user_arn], &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the simulation to be served, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            EXPLICIT_DENY,
            "case {case:?}: expected the group's Deny to be reported as {EXPLICIT_DENY}, got \
             {body}"
        );
    }
}

/// Attach one of a row's documents where the row says it belongs.
async fn attach(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    group_names: &[String],
    document: &Document,
) {
    match document.attached {
        Attached::UserInline => {
            let (status, body) = call(
                router,
                "PutUserPolicy",
                &[
                    ("UserName", user_name),
                    ("PolicyName", document.name),
                    ("PolicyDocument", &document.document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {body}"
            );
        }
        Attached::GroupInline(index) => {
            put_group_policy(
                router,
                case,
                &group_names[index],
                document.name,
                &document.document,
            )
            .await;
        }
        Attached::GroupManaged(index) => {
            let policy_arn = create_policy(router, case, document.name, &document.document).await;
            attach_group_policy(router, case, &group_names[index], &policy_arn).await;
        }
    }
}

/// Create the group `group_name`.
async fn create_group(router: &MultiServiceRouter, case: &str, group_name: &str) {
    let (status, body) = call(router, "CreateGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateGroup must succeed, got {status}: {body}"
    );
}

/// Make `user_name` a member of `group_name`.
async fn add_user_to_group(
    router: &MultiServiceRouter,
    case: &str,
    group_name: &str,
    user_name: &str,
) {
    let (status, body) = call(
        router,
        "AddUserToGroup",
        &[("GroupName", group_name), ("UserName", user_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture AddUserToGroup must succeed, got {status}: {body}"
    );
}

/// Embed `document` in `group_name` under `policy_name`.
async fn put_group_policy(
    router: &MultiServiceRouter,
    case: &str,
    group_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "PutGroupPolicy",
        &[
            ("GroupName", group_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutGroupPolicy must succeed, got {status}: {body}"
    );
}

/// Create a managed policy carrying `document`, answering the ARN it is held under.
async fn create_policy(
    router: &MultiServiceRouter,
    case: &str,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Attach the managed policy `policy_arn` to `group_name`.
async fn attach_group_policy(
    router: &MultiServiceRouter,
    case: &str,
    group_name: &str,
    policy_arn: &str,
) {
    let (status, body) = call(
        router,
        "AttachGroupPolicy",
        &[("GroupName", group_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture AttachGroupPolicy must succeed, got {status}: {body}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `AddUserToGroup`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rules under test, each a TABLE whose rows differ along what the rule generalises
//! over (the name alphabet and path of both entities, the case each is spelled in, how
//! many memberships one user and one group hold, the order they are written in, and the
//! policy documents in force):
//!
//! 1. a membership is a SET of the users a group holds: an add makes the user one of
//!    them whatever case the names are spelled in, a repeat add is the same membership,
//!    and a later read of the group reports it;
//! 2. a name the account holds no entity of the addressed TYPE under refers to no
//!    entity, so the add is refused `NoSuchEntity` (404) and writes nothing;
//! 3. one user belongs to a bounded number of groups, and an add past that bound is
//!    refused `LimitExceeded` (409);
//! 4. the policies in force on a group are in force on its members — "if you specify a
//!    user, the simulation also includes all policies that are attached to any groups
//!    the user belongs to" (`SimulatePrincipalPolicy$PolicySourceArn`);
//! 5. a membership is a subordinate entity, so neither the group nor the user is
//!    deleted while it exists (`DeleteConflict`, 409).

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// The fixture user set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="operation", operation="AddUserToGroup")` gives that
/// pattern for `UserName`), and paths spanning the root, a one-segment path and a
/// nested path.
const USERS: &[(&str, &str)] = &[
    ("ada", "/eng/"),
    ("Bruno", "/"),
    ("cleo_7", "/eng/tools/"),
    ("dmitri.x", "/sales/"),
    ("Enzo+1", "/eng/"),
    ("fatima=2", "/"),
    ("gustav@3", "/sales/west/"),
    ("hana-4", "/eng/tools/"),
];

/// The fixture group set: group names spanning the same alphabet and both cases, at
/// paths spanning the root and two depths.
const GROUPS: &[(&str, &str)] = &[
    ("admins", "/eng/"),
    ("Billing", "/"),
    ("core_7", "/eng/tools/"),
    ("data.x", "/sales/"),
    ("eng+1", "/sales/west/"),
];

/// Create every fixture user and group, answering the router they were created in.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    for (name, path) in USERS {
        let (status, body) = call(&router, "CreateUser", &[("UserName", name), ("Path", path)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the fixture CreateUser must succeed, got {status}: {body}"
        );
    }
    for (name, path) in GROUPS {
        let (status, body) =
            call(&router, "CreateGroup", &[("GroupName", name), ("Path", path)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
        );
    }
    router
}

/// One `AddUserToGroup` call.
async fn add(
    router: &MultiServiceRouter,
    group_name: &str,
    user_name: &str,
) -> (u16, String) {
    call(
        router,
        "AddUserToGroup",
        &[("GroupName", group_name), ("UserName", user_name)],
    )
    .await
}

/// The members `GetGroup` reports for the group `group_name` names.
async fn members_of(router: &MultiServiceRouter, group_name: &str) -> Vec<String> {
    let (status, body) = call(router, "GetGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: the read-back GetGroup must succeed, got {status}: {body}"
    );
    elements(&body, "UserName")
}

/// The member names in the order a read of the group must report them: ascending by the
/// case-folded user name, which is the key the membership is ordered on.
fn in_store_order(members: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = members.iter().map(|name| name.to_string()).collect();
    ordered.sort_by_key(|name| state::name_key(name));
    ordered.dedup();
    ordered
}

/// One group, the users added to it, and the case each add spells the two names in.
struct AddCase {
    /// The group the adds address, by index into [`GROUPS`].
    group: usize,
    /// `(user index, the case the add spells the group name in, the case it spells the
    /// user name in)`, in the order the adds are made.
    adds: Vec<(usize, String, String)>,
    /// The users the group must hold afterwards, by index into [`USERS`].
    expected: Vec<usize>,
}

/// The rows: memberships from one user to every fixture user, adds spelling either name
/// in another case, an add repeated, and the same user added twice in different cases.
fn add_cases() -> Vec<AddCase> {
    let upper = |name: &str| name.to_ascii_uppercase();
    let lower = |name: &str| name.to_ascii_lowercase();
    vec![
        // One member, both names spelled as they were created.
        AddCase {
            group: 0,
            adds: vec![(1, GROUPS[0].0.to_string(), USERS[1].0.to_string())],
            expected: vec![1],
        },
        // Three members, added out of the order a read reports them in.
        AddCase {
            group: 1,
            adds: vec![
                (6, GROUPS[1].0.to_string(), USERS[6].0.to_string()),
                (0, GROUPS[1].0.to_string(), USERS[0].0.to_string()),
                (2, GROUPS[1].0.to_string(), USERS[2].0.to_string()),
            ],
            expected: vec![0, 2, 6],
        },
        // Names "are not distinguished by case": the add resolves both entities
        // whatever case it spells them in.
        AddCase {
            group: 2,
            adds: vec![
                (4, upper(GROUPS[2].0), lower(USERS[4].0)),
                (5, lower(GROUPS[2].0), upper(USERS[5].0)),
            ],
            expected: vec![4, 5],
        },
        // The same membership written twice, the second time in another case: a user
        // belongs to a group at most once.
        AddCase {
            group: 3,
            adds: vec![
                (3, GROUPS[3].0.to_string(), USERS[3].0.to_string()),
                (3, upper(GROUPS[3].0), upper(USERS[3].0)),
                (3, GROUPS[3].0.to_string(), USERS[3].0.to_string()),
            ],
            expected: vec![3],
        },
        // Every fixture user in one group.
        AddCase {
            group: 4,
            adds: USERS
                .iter()
                .enumerate()
                .map(|(index, (name, _))| {
                    (index, GROUPS[4].0.to_string(), name.to_string())
                })
                .collect(),
            expected: (0..USERS.len()).collect(),
        },
    ]
}

/// RULE: an add makes the user one of the users the group holds — the entities are
/// resolved by name, which is not distinguished by case — and the membership is a SET,
/// so a repeat add is the same membership and a later read reports each member once.
#[tokio::test]
async fn add_user_to_group_writes_a_membership_a_later_read_reports() {
    for case in add_cases() {
        let router = fixture().await;
        let group_name = GROUPS[case.group].0;

        // Before the first add the group holds nobody, so what the read reports below
        // is what the adds wrote.
        assert!(
            members_of(&router, group_name).await.is_empty(),
            "case {group_name:?}: a group nothing was added to holds no member"
        );

        for (user, group_as, user_as) in &case.adds {
            let user_name = USERS[*user].0;
            let (status, body) = add(&router, group_as, user_as).await;
            assert_eq!(
                status, 200,
                "case {group_name:?}/{user_name:?} added as {group_as:?}/{user_as:?}: \
                 expected 200, got {status}: {body}"
            );
        }

        let expected_names: Vec<&str> = case.expected.iter().map(|user| USERS[*user].0).collect();
        let expected = in_store_order(&expected_names);
        let reported = members_of(&router, group_name).await;
        assert_eq!(
            reported, expected,
            "case {group_name:?}: expected the users the adds made members, got {reported:?}"
        );

        // Each member's own record still reads back, and the membership did not move
        // any OTHER group's: a membership belongs to the group it was written for.
        for (user, _, _) in &case.adds {
            let user_name = USERS[*user].0;
            let (status, body) = call(&router, "GetUser", &[("UserName", user_name)]).await;
            assert_eq!(
                status, 200,
                "case {group_name:?}/{user_name:?}: the member must still read back, \
                 got {status}: {body}"
            );
        }
        for (other, _) in GROUPS.iter().filter(|(name, _)| *name != group_name) {
            assert!(
                members_of(&router, other).await.is_empty(),
                "case {group_name:?}: the add must not make the user a member of {other:?}"
            );
        }
    }
}

/// RULE: one user belongs to several groups at once — the membership is held per
/// group, so each group reports exactly the users added to it and no other group is
/// changed by the add.
#[tokio::test]
async fn one_user_belongs_to_every_group_it_was_added_to() {
    let router = fixture().await;
    let user_name = USERS[2].0;
    let other_user = USERS[5].0;
    // Every fixture group but the last, so one group stands outside the memberships.
    let joined = &GROUPS[..GROUPS.len() - 1];
    let left_out = GROUPS[GROUPS.len() - 1].0;

    for (group_name, _) in joined {
        let (status, body) = add(&router, group_name, user_name).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: expected 200, got {status}: {body}"
        );
    }
    // A second user in one of them, so a group holding two members reports both.
    let shared = joined[1].0;
    let (status, body) = add(&router, shared, other_user).await;
    assert_eq!(
        status, 200,
        "case {shared:?}/{other_user:?}: expected 200, got {status}: {body}"
    );

    for (group_name, _) in joined {
        let expected = if *group_name == shared {
            in_store_order(&[user_name, other_user])
        } else {
            vec![user_name.to_string()]
        };
        assert_eq!(
            members_of(&router, group_name).await,
            expected,
            "case {group_name:?}: expected the users added to this group"
        );
    }
    assert!(
        members_of(&router, left_out).await.is_empty(),
        "case {left_out:?}: a group nobody was added to holds no member"
    );
}

/// RULE: a name the account holds no entity of the addressed TYPE under refers to no
/// entity, so the add is refused `NoSuchEntity` (404) naming that entity as the request
/// spelled it, and nothing is written.
#[tokio::test]
async fn add_user_to_group_refuses_a_name_that_names_no_such_entity() {
    let router = fixture().await;
    let held_group = GROUPS[0].0;
    let held_user = USERS[0].0;

    // Rows: a group name nothing was created under, one differing from a held group by
    // more than case, one that names a USER rather than a group, and names spanning the
    // alphabet and both cases.
    for group_name in [
        "nobody",
        "admins2",
        "Admin",
        held_user,
        "ghost_7",
        "void.x",
        "NOPE+1",
        "under=score",
    ] {
        let (status, body) = add(&router, group_name, held_user).await;
        assert_eq!(
            status, 404,
            "case group {group_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case group {group_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group with name {group_name} cannot be found."),
            "case group {group_name:?}: expected the refusal to name the group it could \
             not find, got {:?}",
            element(&body, "Message")
        );
    }

    // The same rows for the USER the add names, against a group the account does hold.
    for user_name in [
        "nobody",
        "ada2",
        held_group,
        "ghost_7",
        "void.x",
        "NOPE+1",
        "under=score",
        "dash-ed",
    ] {
        let (status, body) = add(&router, held_group, user_name).await;
        assert_eq!(
            status, 404,
            "case user {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case user {user_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The user with name {user_name} cannot be found."),
            "case user {user_name:?}: expected the refusal to name the user it could not \
             find, got {:?}",
            element(&body, "Message")
        );
    }

    // A refused add wrote nothing: every group still holds nobody.
    for (group_name, _) in GROUPS {
        assert!(
            members_of(&router, group_name).await.is_empty(),
            "case {group_name:?}: a refused add must write no membership"
        );
    }

    // The add the account CAN serve is still served, so the refusals above are about
    // the names and not about the operation.
    let (status, body) = add(&router, held_group, held_user).await;
    assert_eq!(
        status, 200,
        "expected the add naming two held entities to be served, got {status}: {body}"
    );
    assert_eq!(
        members_of(&router, held_group).await,
        vec![held_user.to_string()],
        "expected the served add to make the user a member"
    );
}

/// RULE: a user belongs to at most [`state::MAX_GROUPS_PER_USER`] groups, so the add
/// past that bound is refused `LimitExceeded` (409) with the quota and its value, the
/// bound is taken per USER, and an add that changes nothing never consumes a slot.
#[tokio::test]
async fn add_user_to_group_refuses_a_membership_past_the_quota() {
    let router = router();
    let quota = state::MAX_GROUPS_PER_USER;
    // One group more than the quota allows one user to join, so the bound is exercised
    // at it and one past it.
    let group_names: Vec<String> = (0..=quota).map(|index| format!("team{index}")).collect();
    for group_name in &group_names {
        let (status, body) = call(&router, "CreateGroup", &[("GroupName", group_name)]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
        );
    }
    for (user_name, _) in &USERS[..2] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: the fixture CreateUser must succeed, got {status}: {body}"
        );
    }
    let user_name = USERS[0].0;

    // Every add up to the quota is served, including the one that fills the last slot.
    for (count, group_name) in group_names[..quota].iter().enumerate() {
        let (status, body) = add(&router, group_name, user_name).await;
        assert_eq!(
            status, 200,
            "case membership {} of {quota}: expected 200, got {status}: {body}",
            count + 1
        );
    }

    // An add of a membership the user already holds changes nothing, so it is served
    // even with every slot taken: it consumes no second slot.
    for group_name in [&group_names[0], &group_names[quota - 1]] {
        let (status, body) = add(&router, group_name, user_name).await;
        assert_eq!(
            status, 200,
            "case repeat {group_name:?}: expected a membership the user already holds to \
             be served, got {status}: {body}"
        );
    }

    // One past the bound.
    let over = &group_names[quota];
    let (status, body) = add(&router, over, user_name).await;
    assert_eq!(
        status, 409,
        "case {over:?}: expected the add past the quota to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {over:?}: expected LimitExceeded, got {:?}",
        error_code(&body)
    );
    assert_eq!(
        element(&body, "Message"),
        format!("The maximum number of groups per user is exceeded for user: {user_name}."),
        "case {over:?}: expected the refusal to describe the limit exceeded, got {:?}",
        element(&body, "Message")
    );
    assert!(
        members_of(&router, over).await.is_empty(),
        "case {over:?}: the refused add must write no membership"
    );

    // The quota is taken per user: ANOTHER user still joins the group the first one
    // could not.
    let other = USERS[1].0;
    let (status, body) = add(&router, over, other).await;
    assert_eq!(
        status, 200,
        "case {over:?}/{other:?}: expected another user's add to be served, got {status}: {body}"
    );
    assert_eq!(
        members_of(&router, over).await,
        vec![other.to_string()],
        "case {over:?}: expected the other user to be the group's member"
    );

    // The second user, filled to the quota in turn: the refusal names the user the add
    // asked about, so the message is taken from the request and not written out once.
    for group_name in &group_names[..quota - 1] {
        let (status, body) = add(&router, group_name, other).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}/{other:?}: expected 200, got {status}: {body}"
        );
    }
    let over_for_other = &group_names[quota - 1];
    let (status, body) = add(&router, over_for_other, other).await;
    assert_eq!(
        status, 409,
        "case {over_for_other:?}/{other:?}: expected the add past the quota to be refused, \
         got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("The maximum number of groups per user is exceeded for user: {other}."),
        "case {over_for_other:?}/{other:?}: expected the refusal to name the user the add \
         asked about, got {:?}",
        element(&body, "Message")
    );
}

/// What a row of the inheritance table expects of the signed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the call.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One row: what the calling user carries itself, what the group it is offered carries,
/// whether it is added to that group, and the answer the rule requires.
struct InheritanceCase {
    case: &'static str,
    /// The document the user embeds itself, if any.
    own: Option<String>,
    /// The document the group embeds.
    group: String,
    /// Whether the user is added to that group.
    member: bool,
    /// The awsQuery action the user calls, and the IAM action the decision is made on.
    action: &'static str,
    iam_action: &'static str,
    expect: Expect,
}

/// The rows: a grant that reaches the user only through the group, a `Deny` the group
/// states over the user's own `Allow`, a group document about another action, and the
/// same documents with no membership at all.
fn inheritance_cases() -> Vec<InheritanceCase> {
    let allow_list_roles = statement("Allow", "iam:ListRoles", "*");
    let deny_list_roles = statement("Deny", "iam:ListRoles", "*");
    vec![
        InheritanceCase {
            case: "the group's Allow is the user's only grant",
            own: None,
            group: allow_list_roles.clone(),
            member: true,
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            expect: Expect::Served,
        },
        InheritanceCase {
            case: "the same Allow, with the user not a member",
            own: None,
            group: allow_list_roles.clone(),
            member: false,
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            expect: Expect::NoAllow,
        },
        InheritanceCase {
            case: "the group's Deny over the user's own Allow",
            own: Some(allow_list_roles.clone()),
            group: deny_list_roles.clone(),
            member: true,
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            expect: Expect::ExplicitDeny,
        },
        InheritanceCase {
            case: "the same Deny, with the user not a member",
            own: Some(allow_list_roles.clone()),
            group: deny_list_roles.clone(),
            member: false,
            action: "ListRoles",
            iam_action: "iam:ListRoles",
            expect: Expect::Served,
        },
        InheritanceCase {
            case: "the group's Deny on an action the user does not call",
            own: Some(statement("Allow", "iam:ListUsers", "*")),
            group: deny_list_roles.clone(),
            member: true,
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            expect: Expect::Served,
        },
        InheritanceCase {
            case: "the group's Allow for an action the user does not call",
            own: None,
            group: allow_list_roles.clone(),
            member: true,
            action: "ListGroups",
            iam_action: "iam:ListGroups",
            expect: Expect::NoAllow,
        },
        InheritanceCase {
            case: "the group's Deny on every action, over the user's own Allow",
            own: Some(statement("Allow", "iam:ListGroups", "*")),
            group: statement("Deny", "*", "*"),
            member: true,
            action: "ListGroups",
            iam_action: "iam:ListGroups",
            expect: Expect::ExplicitDeny,
        },
        InheritanceCase {
            case: "the group's Allow over a wildcard action run",
            own: None,
            group: statement("Allow", "iam:List*", "*"),
            member: true,
            action: "ListUsers",
            iam_action: "iam:ListUsers",
            expect: Expect::Served,
        },
    ]
}

/// RULE: the identity-based policies in force on a user include the policies of every
/// group it belongs to — "if you specify a user, the simulation also includes all
/// policies that are attached to any groups the user belongs to"
/// (`SimulatePrincipalPolicy$PolicySourceArn`) — so a group's grant reaches its members
/// and a group's explicit `Deny` refuses them, and neither reaches a user that is not a
/// member.
#[tokio::test]
async fn the_policies_of_a_group_are_in_force_on_its_members() {
    for (row, case) in inheritance_cases().into_iter().enumerate() {
        let router = router();
        let user_name = format!("member{row}");
        let group_name = format!("carrier{row}");
        let own: Vec<(String, String)> = case
            .own
            .clone()
            .map(|document| (format!("own{row}"), document))
            .into_iter()
            .collect();
        let access_key_id = principal(&router, case.case, &user_name, &own).await;

        let (status, body) = call(&router, "CreateGroup", &[("GroupName", &group_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture CreateGroup must succeed, got {status}: {body}",
            case.case
        );
        let (status, body) = call(
            &router,
            "PutGroupPolicy",
            &[
                ("GroupName", &group_name),
                ("PolicyName", &format!("carried{row}")),
                ("PolicyDocument", &case.group),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture PutGroupPolicy must succeed, got {status}: {body}",
            case.case
        );
        if case.member {
            let (status, body) = add(&router, &group_name, &user_name).await;
            assert_eq!(
                status, 200,
                "case {:?}: the AddUserToGroup under test must succeed, got {status}: {body}",
                case.case
            );
        }

        let (status, body) = call_signed_by(&router, Some(&access_key_id), case.action, &[]).await;
        match case.expect {
            Expect::Served => assert_eq!(
                status, 200,
                "case {:?}: expected the call to be served, got {status}: {body}",
                case.case
            ),
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {:?}: expected AccessDenied, got {:?}",
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
                let message = element(&body, "Message");
                assert!(
                    message.ends_with(&reason),
                    "case {:?}: expected a refusal ending {reason:?}, got {message:?}",
                    case.case
                );
            }
        }
    }
}

/// RULE: a membership is a subordinate entity of both entities it joins, so neither the
/// group nor the user is deleted while it exists: the delete is refused `DeleteConflict`
/// (409) and nothing is removed.
#[tokio::test]
async fn a_membership_refuses_the_delete_of_either_entity() {
    let router = fixture().await;
    let group_name = GROUPS[2].0;
    let user_name = USERS[3].0;
    let (status, body) = add(&router, group_name, user_name).await;
    assert_eq!(status, 200, "the add must succeed, got {status}: {body}");

    // Rows: both operations whose entity the membership hangs off, each refused with
    // the message that names what has to be removed first.
    for (operation, parameter, name) in [
        ("DeleteGroup", "GroupName", group_name),
        ("DeleteUser", "UserName", user_name),
    ] {
        let (status, body) = call(&router, operation, &[(parameter, name)]).await;
        assert_eq!(
            status, 409,
            "case {operation}: expected the delete to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {operation}: expected DeleteConflict, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            "Cannot delete entity, must remove users from group first.",
            "case {operation}: expected the refusal to name what must be removed first, \
             got {:?}",
            element(&body, "Message")
        );
    }

    // Nothing was removed: the group, the membership and the user are all still there.
    assert_eq!(
        members_of(&router, group_name).await,
        vec![user_name.to_string()],
        "a refused delete must leave the membership in place"
    );
    let (status, body) = call(&router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the user in place, got {status}: {body}"
    );

    // A group nobody was added to, and a user in no group, are both still deletable —
    // so the refusals above are about the membership and not about the operations.
    for (operation, parameter, name) in [
        ("DeleteGroup", "GroupName", GROUPS[4].0),
        ("DeleteUser", "UserName", USERS[7].0),
    ] {
        let (status, body) = call(&router, operation, &[(parameter, name)]).await;
        assert_eq!(
            status, 200,
            "case {operation} without a membership: expected 200, got {status}: {body}"
        );
    }
}

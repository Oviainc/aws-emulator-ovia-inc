// SPDX-License-Identifier: CC0-1.0

//! `RemoveUserFromGroup`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share. `tests/all/group_membership_writes.rs` is the
//! suite for `AddUserToGroup`, the operation that writes the membership these tests
//! take away.
//!
//! The rules under test, each a TABLE whose rows differ along what the rule
//! generalises over (the name alphabet, case and path of both entities, how many
//! memberships the group and the user hold, how many removals are made and in what
//! order, and whether the membership addressed exists at all):
//!
//! 1. a membership is a SET of the users a group holds, so a removal deletes exactly
//!    the one membership it addresses — the entities are resolved by name, which is
//!    not distinguished by case — leaving every other membership, the group and the
//!    user as they were, which a later `GetGroup` and `ListGroupsForUser` report;
//! 2. the removal names the membership to be gone, so removing one the account does
//!    not hold (a repeat, or a pair never added) succeeds and changes nothing;
//! 3. a name the account holds no entity of the addressed TYPE under refers to no
//!    entity, so the removal is refused `NoSuchEntity` (404) naming that entity as the
//!    request spelled it (`smithy_query(query="operation",
//!    operation="RemoveUserFromGroup")` declares that error) and nothing is written;
//! 4. a membership is a subordinate entity, so neither the group nor the user is
//!    deleted while it exists (`DeleteConflict`, 409) and both are once it is removed.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The fixture user set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="operation", operation="RemoveUserFromGroup")` gives that
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

/// The memberships the fixture writes: for each group (by index into [`GROUPS`]) the
/// users (by index into [`USERS`]) added to it. The rows span a group holding one
/// member, a group holding a few and a group holding nearly every fixture user, and
/// with them a user belonging to one group only (`USERS[5]`), to two, and to four.
const MEMBERSHIPS: &[(usize, &[usize])] = &[
    (0, &[0, 1, 2, 7]),
    (1, &[0]),
    (2, &[0, 3, 4]),
    (3, &[5, 6]),
    (4, &[0, 1, 2, 3, 4, 6, 7]),
];

/// The case a call spells a stored name in. IAM entity names "are not distinguished by
/// case" (`CreateGroup$GroupName`), so each of these addresses the same entity.
#[derive(Clone, Copy, Debug)]
enum Spelling {
    AsCreated,
    Upper,
    Lower,
}

/// `name` as `spelling` spells it.
fn spell(name: &str, spelling: Spelling) -> String {
    match spelling {
        Spelling::AsCreated => name.to_string(),
        Spelling::Upper => name.to_ascii_uppercase(),
        Spelling::Lower => name.to_ascii_lowercase(),
    }
}

/// One `RemoveUserFromGroup` call: the membership it addresses, and the case it spells
/// each of the two names in.
#[derive(Clone, Copy, Debug)]
struct Removal {
    group: usize,
    user: usize,
    group_as: Spelling,
    user_as: Spelling,
}

impl Removal {
    /// A removal addressing the membership `(group, user)` by the names as created.
    const fn held(group: usize, user: usize) -> Self {
        Self {
            group,
            user,
            group_as: Spelling::AsCreated,
            user_as: Spelling::AsCreated,
        }
    }

    /// The same membership, with the case each name is spelled in stated.
    const fn spelled(group: usize, user: usize, group_as: Spelling, user_as: Spelling) -> Self {
        Self {
            group,
            user,
            group_as,
            user_as,
        }
    }
}

/// A row of a removal table: the removals it makes, in order.
struct RemovalCase {
    /// What the row is about, named in every assertion message.
    case: &'static str,
    removals: Vec<Removal>,
}

/// Create every fixture user and group and write every fixture membership, answering
/// the router they live in.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    for (name, path) in USERS {
        let (status, body) =
            call(&router, "CreateUser", &[("UserName", name), ("Path", path)]).await;
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
    for (group, users) in MEMBERSHIPS {
        let group_name = GROUPS[*group].0;
        for user in *users {
            let user_name = USERS[*user].0;
            let (status, body) = call(
                &router,
                "AddUserToGroup",
                &[("GroupName", group_name), ("UserName", user_name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {group_name:?}/{user_name:?}: the fixture AddUserToGroup must \
                 succeed, got {status}: {body}"
            );
        }
    }
    router
}

/// One `RemoveUserFromGroup` call, spelled as `removal` says.
async fn remove(router: &MultiServiceRouter, removal: &Removal) -> (u16, String) {
    let group_name = spell(GROUPS[removal.group].0, removal.group_as);
    let user_name = spell(USERS[removal.user].0, removal.user_as);
    call(
        router,
        "RemoveUserFromGroup",
        &[
            ("GroupName", group_name.as_str()),
            ("UserName", user_name.as_str()),
        ],
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
    // The group's own record carries its `GroupName`; the members carry a `UserName`
    // each, which is what the membership is read back as.
    elements(&body, "UserName")
}

/// The groups `ListGroupsForUser` reports the user `user_name` names belongs to.
async fn groups_of(router: &MultiServiceRouter, user_name: &str) -> Vec<String> {
    let (status, body) = call(router, "ListGroupsForUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: the read-back ListGroupsForUser must succeed, got \
         {status}: {body}"
    );
    elements(&body, "GroupName")
}

/// The names in the order a read of a collection reports them: ascending by the
/// case-folded name, which is the key the store is ordered on.
fn in_store_order(names: &[&str]) -> Vec<String> {
    let mut ordered: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    ordered.sort_by_key(|name| state::name_key(name));
    ordered.dedup();
    ordered
}

/// The fixture memberships, as a user-index list per group index.
fn memberships() -> Vec<Vec<usize>> {
    let mut rows = vec![Vec::new(); GROUPS.len()];
    for (group, users) in MEMBERSHIPS {
        rows[*group] = users.to_vec();
    }
    rows
}

/// The memberships the account holds after `removals`: each one deletes the membership
/// it addresses, and one the account does not hold is already deleted.
fn after(removals: &[Removal]) -> Vec<Vec<usize>> {
    let mut rows = memberships();
    for removal in removals {
        rows[removal.group].retain(|user| *user != removal.user);
    }
    rows
}

/// Assert that every group reports exactly the members `expected` holds for it, and
/// that every user reports exactly the groups it is a member of in `expected` — the
/// membership read both ways round.
async fn assert_memberships(router: &MultiServiceRouter, expected: &[Vec<usize>], case: &str) {
    for (group, users) in expected.iter().enumerate() {
        let group_name = GROUPS[group].0;
        let names: Vec<&str> = users.iter().map(|user| USERS[*user].0).collect();
        let wanted = in_store_order(&names);
        let reported = members_of(router, group_name).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: group {group_name:?}: expected the members {wanted:?}, got \
             {reported:?}"
        );
    }
    for (user, (user_name, _)) in USERS.iter().enumerate() {
        let names: Vec<&str> = expected
            .iter()
            .enumerate()
            .filter(|(_, users)| users.contains(&user))
            .map(|(group, _)| GROUPS[group].0)
            .collect();
        let wanted = in_store_order(&names);
        let reported = groups_of(router, user_name).await;
        assert_eq!(
            reported, wanted,
            "case {case:?}: user {user_name:?}: expected the groups {wanted:?}, got \
             {reported:?}"
        );
    }
}

/// The rows of rule 1: one removal each, spanning the only membership of a group, one
/// of a few, one of a group holding nearly every user, a user's only membership, and
/// the same addressed in either case.
fn single_removals() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "the only membership the group holds",
            removals: vec![Removal::held(1, 0)],
        },
        RemovalCase {
            case: "one of the four memberships the group holds",
            removals: vec![Removal::held(0, 2)],
        },
        RemovalCase {
            case: "one of the memberships of the group nearly every user belongs to",
            removals: vec![Removal::held(4, 7)],
        },
        RemovalCase {
            case: "the only membership the user holds",
            removals: vec![Removal::held(3, 5)],
        },
        RemovalCase {
            case: "both names spelled upper-cased",
            removals: vec![Removal::spelled(2, 3, Spelling::Upper, Spelling::Upper)],
        },
        RemovalCase {
            case: "both names spelled lower-cased",
            removals: vec![Removal::spelled(3, 6, Spelling::Lower, Spelling::Lower)],
        },
        RemovalCase {
            case: "the group upper-cased and the user lower-cased",
            removals: vec![Removal::spelled(0, 1, Spelling::Upper, Spelling::Lower)],
        },
        RemovalCase {
            case: "the group lower-cased and the user upper-cased",
            removals: vec![Removal::spelled(2, 4, Spelling::Lower, Spelling::Upper)],
        },
    ]
}

/// The rows of rule 1 that make SEVERAL removals: every membership of one group, every
/// membership of one user, and removals interleaved across groups.
fn multiple_removals() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "every membership one group holds",
            removals: vec![Removal::held(3, 5), Removal::held(3, 6)],
        },
        RemovalCase {
            case: "every membership one user holds",
            removals: vec![
                Removal::held(0, 0),
                Removal::spelled(1, 0, Spelling::Upper, Spelling::Upper),
                Removal::held(2, 0),
                Removal::held(4, 0),
            ],
        },
        RemovalCase {
            case: "removals interleaved across groups",
            removals: vec![
                Removal::held(4, 1),
                Removal::held(0, 7),
                Removal::held(2, 4),
                Removal::held(4, 3),
                Removal::held(1, 0),
            ],
        },
    ]
}

/// RULE: a removal deletes exactly the membership it addresses — both entities are
/// resolved by name, which is not distinguished by case — so a later read of the group
/// and of the user stops reporting that one membership and reports every other one
/// unchanged, and both entities themselves survive.
#[tokio::test]
async fn remove_user_from_group_deletes_only_the_membership_it_addresses() {
    for row in single_removals().into_iter().chain(multiple_removals()) {
        let router = fixture().await;
        let case = row.case;

        // What the fixture wrote, so what the reads below report is what the removals
        // changed and nothing else.
        assert_memberships(&router, &memberships(), case).await;

        for removal in &row.removals {
            let (status, body) = remove(&router, removal).await;
            assert_eq!(
                status, 200,
                "case {case:?}: removal {removal:?}: expected 200, got {status}: {body}"
            );
        }

        assert_memberships(&router, &after(&row.removals), case).await;

        // The membership went; the entities it joined did not.
        for removal in &row.removals {
            let user_name = USERS[removal.user].0;
            let (status, body) = call(&router, "GetUser", &[("UserName", user_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: user {user_name:?}: the user must still read back, got \
                 {status}: {body}"
            );
            assert_eq!(
                element(&body, "UserName"),
                user_name,
                "case {case:?}: user {user_name:?}: expected the stored user record, got {body}"
            );
        }
    }
}

/// RULE: the removal names the membership that is to be gone, so a removal addressing
/// one the account does not hold — a repeat of a removal already made, or a pair never
/// added — succeeds and changes nothing.
#[tokio::test]
async fn removing_a_membership_the_account_does_not_hold_changes_nothing() {
    let rows = vec![
        RemovalCase {
            case: "the same membership removed twice",
            removals: vec![Removal::held(1, 0), Removal::held(1, 0)],
        },
        RemovalCase {
            case: "the same membership removed three times, each in another case",
            removals: vec![
                Removal::held(0, 2),
                Removal::spelled(0, 2, Spelling::Upper, Spelling::Upper),
                Removal::spelled(0, 2, Spelling::Lower, Spelling::Lower),
            ],
        },
        RemovalCase {
            case: "a membership never added",
            removals: vec![Removal::held(3, 0)],
        },
        RemovalCase {
            case: "a membership never added, spelled in another case",
            removals: vec![Removal::spelled(1, 5, Spelling::Upper, Spelling::Upper)],
        },
        RemovalCase {
            case: "every pair of a group and a user that is not a membership",
            removals: (0..GROUPS.len())
                .flat_map(|group| (0..USERS.len()).map(move |user| (group, user)))
                .filter(|(group, user)| !memberships()[*group].contains(user))
                .map(|(group, user)| Removal::held(group, user))
                .collect(),
        },
    ];

    for row in rows {
        let router = fixture().await;
        let case = row.case;
        for removal in &row.removals {
            let (status, body) = remove(&router, removal).await;
            assert_eq!(
                status, 200,
                "case {case:?}: removal {removal:?}: expected 200 for a membership the \
                 account does not hold, got {status}: {body}"
            );
        }
        assert_memberships(&router, &after(&row.removals), case).await;
    }
}

/// RULE: a name the account holds no entity of the addressed TYPE under refers to no
/// entity, so the removal is refused `NoSuchEntity` (404) naming that entity as the
/// request spelled it, and nothing is written.
#[tokio::test]
async fn remove_user_from_group_refuses_a_name_that_names_no_such_entity() {
    let router = fixture().await;
    let held_group = GROUPS[0].0;
    let held_user = USERS[0].0;

    // Rows: a name nothing was created under, one differing from a held name by more
    // than case, one naming an entity of the OTHER type, and names spanning the
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
        "mail@host",
        "dash-ed",
    ] {
        let (status, body) = call(
            &router,
            "RemoveUserFromGroup",
            &[("GroupName", group_name), ("UserName", held_user)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case group {group_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case group {group_name:?}: expected NoSuchEntity, got {:?}: {body}",
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

    // The same rows for the USER the removal names, against a group the account holds.
    for user_name in [
        "nobody",
        "ada2",
        "ADA_",
        held_group,
        "ghost_7",
        "void.x",
        "NOPE+1",
        "under=score",
        "mail@host",
        "dash-ed",
    ] {
        let (status, body) = call(
            &router,
            "RemoveUserFromGroup",
            &[("GroupName", held_group), ("UserName", user_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case user {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case user {user_name:?}: expected NoSuchEntity, got {:?}: {body}",
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

    // Nothing the refusals touched was written: every fixture membership still stands.
    assert_memberships(&router, &memberships(), "after the refused removals").await;
}

/// RULE: a membership is a subordinate entity of both the group and the user, so while
/// it exists each of them is refused deletion with `DeleteConflict` (409), and once it
/// is removed each is deleted.
#[tokio::test]
async fn an_entity_is_deletable_once_its_memberships_are_removed() {
    let router = fixture().await;
    // The group holding exactly the two users of `MEMBERSHIPS` row 3, one of which
    // (`USERS[5]`) belongs to no other group — so removing those two memberships
    // clears every membership of both the group and that user.
    let group_name = GROUPS[3].0;
    let sole_member = USERS[5].0;
    let shared_member = USERS[6].0;

    for (what, action, params) in [
        (
            "a group still holding members",
            "DeleteGroup",
            vec![("GroupName", group_name)],
        ),
        (
            "a user still belonging to a group",
            "DeleteUser",
            vec![("UserName", sole_member)],
        ),
    ] {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 409,
            "case {what:?}: expected {action} 409 while the membership stands, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {what:?}: expected DeleteConflict, got {:?}: {body}",
            error_code(&body)
        );
    }

    for removal in [Removal::held(3, 5), Removal::held(3, 6)] {
        let (status, body) = remove(&router, &removal).await;
        assert_eq!(
            status, 200,
            "case {removal:?}: expected 200, got {status}: {body}"
        );
    }

    for (what, action, params) in [
        (
            "the user whose only membership was removed",
            "DeleteUser",
            vec![("UserName", sole_member)],
        ),
        (
            "the group whose members were removed",
            "DeleteGroup",
            vec![("GroupName", group_name)],
        ),
    ] {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {what:?}: expected {action} 200 once the membership is gone, got \
             {status}: {body}"
        );
    }

    // The member the removal shared with another group kept that other membership: the
    // removal took away one membership, not the user's standing everywhere.
    let reported = groups_of(&router, shared_member).await;
    assert_eq!(
        reported,
        vec![GROUPS[4].0.to_string()],
        "case {shared_member:?}: expected the user's other membership to stand, got \
         {reported:?}"
    );
}

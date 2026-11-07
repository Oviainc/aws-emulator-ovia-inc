// SPDX-License-Identifier: CC0-1.0

//! `RemoveGroupMember`: taking an object of a directory out of one of its groups, end
//! to end over HTTP.
//!
//! The rule under test is the one a directory's membership relation states from the
//! removing side: a removal forgets exactly the one membership it names — the named
//! member on the named group's record of the named directory — so neither side of the
//! relation reports it afterwards while every other membership stands, and a name it
//! cannot resolve to a held membership (an unwritten directory, a group the directory
//! holds no object for, a member it holds no object for, a member in another domain, or
//! an object that group does not hold) is `ResourceNotFoundException`. Names are matched
//! the way Active Directory matches an account name: case insensitively.
//!
//! Every case drives the router, so it exercises routing, deserialization, the handler,
//! the persisted records and serialization — the same path a client takes. The table
//! below spans what the rule generalises over: the member's kind (a user object and a
//! nested group object), the spelling the removal names group and member in, how many
//! members the group holds and how many groups hold the member, whether the group is one
//! `CreateGroup` stored or one the domain was created with, whether the removal names the
//! member's realm, and the longest name each of `GroupName` and `MemberName` admits.
//!
//! `ClientToken` is the `@idempotencyToken` of this operation, so the last suite below
//! reads the documented window: the same token with the same parameters answers as the
//! call it repeats, and with different ones is a conflict.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, emulator, Answer, DIRECTORY};

/// A second `DirectoryId` inside `^d-[0-9a-f]{10}$`, for the case that is about one
/// directory not answering for another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// A domain name that is not the realm the emulator's directories carry, inside
/// `Realm`'s `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`.
const ANOTHER_REALM: &str = "elsewhere.example.org";

/// The group Active Directory creates with a domain and makes every user account's
/// primary group, so a created user is a member of it without anything adding it.
const PRIMARY_USER_GROUP: &str = "Domain Users";

/// A built-in group of the domain other than the primary user group, so a row can span a
/// group the directory holds because it IS a directory rather than one `CreateGroup`
/// stored.
const BUILTIN_GROUP: &str = "Domain Admins";

/// `GroupName`'s `@length` maximum
/// (`smithy_query(query="operation", operation="RemoveGroupMember")` — `GroupName`,
/// len 1..64).
const GROUP_NAME_CEILING: usize = 64;

/// `MemberName`'s `@length` maximum, from the same signature (len 1..63).
const MEMBER_NAME_CEILING: usize = 63;

/// `UserName`'s `@length` maximum (`smithy_query(query="shape", shape="UserName")` — len
/// 1..20): the longest name a user object of the directory can answer to, which is
/// shorter than what `MemberName` admits, because a member can also be a group.
const USER_NAME_CEILING: usize = 20;

const CREATE_USER: &str = "/Users/CreateUser";
const CREATE_GROUP: &str = "/Groups/CreateGroup";
const ADD_GROUP_MEMBER: &str = "/GroupMemberships/AddGroupMember";
const REMOVE_GROUP_MEMBER: &str = "/GroupMemberships/RemoveGroupMember";
const LIST_GROUP_MEMBERS: &str = "/GroupMemberships/ListGroupMembers";
const LIST_GROUPS_FOR_MEMBER: &str = "/GroupMemberships/ListGroupsForMember";

const NOT_FOUND: &str = "ResourceNotFoundException";
const CONFLICT: &str = "ConflictException";

async fn create_user(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        CREATE_USER,
        DIRECTORY,
        &json!({"SAMAccountName": name}),
    )
    .await
}

async fn create_group(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        CREATE_GROUP,
        DIRECTORY,
        &json!({"SAMAccountName": name}),
    )
    .await
}

async fn add_group_member(router: &MultiServiceRouter, group: &str, member: &str) -> Answer {
    call(
        router,
        ADD_GROUP_MEMBER,
        DIRECTORY,
        &json!({"GroupName": group, "MemberName": member}),
    )
    .await
}

async fn remove_group_member(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, REMOVE_GROUP_MEMBER, directory, body).await
}

/// The `SAMAccountName`s a `ListGroupMembers` page reported for a group.
async fn members_of(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let answer = call(
        router,
        LIST_GROUP_MEMBERS,
        DIRECTORY,
        &json!({"SAMAccountName": group}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "group {group:?}: ListGroupMembers must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Members"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "group {group:?}: ListGroupMembers must report a Members list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|member| {
            member["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `SAMAccountName`s a `ListGroupsForMember` page reported for a member.
async fn groups_of(router: &MultiServiceRouter, member: &str) -> Vec<String> {
    let answer = call(
        router,
        LIST_GROUPS_FOR_MEMBER,
        DIRECTORY,
        &json!({"SAMAccountName": member}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "member {member:?}: ListGroupsForMember must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Groups"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "member {member:?}: ListGroupsForMember must report a Groups list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|group| {
            group["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Whether `names` holds `wanted`, compared the way AD resolves an account name.
fn holds(names: &[String], wanted: &str) -> bool {
    names.iter().any(|name| name.eq_ignore_ascii_case(wanted))
}

/// A setup call whose answer the case itself does not read: it must have been served,
/// so a setup that stopped working fails here instead of surfacing as a puzzling
/// refusal from the call under test.
fn served(what: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "setup {what}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The realm of this emulator's directories, as the service itself reports it, so no
/// case below states a domain name the service did not choose.
async fn own_realm(router: &MultiServiceRouter) -> String {
    let answer = call(
        router,
        LIST_GROUP_MEMBERS,
        DIRECTORY,
        &json!({"SAMAccountName": PRIMARY_USER_GROUP}),
    )
    .await;
    answer.body["MemberRealm"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "ListGroupMembers must report the directory's MemberRealm, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// One row of the table: the objects and memberships the directory holds before the
/// removal, the removal itself, and the memberships that must survive it.
#[derive(Default)]
struct Case {
    what: String,
    /// Users `CreateUser` stores first.
    users: Vec<String>,
    /// Groups `CreateGroup` stores first.
    groups: Vec<String>,
    /// Memberships `AddGroupMember` records first: (group, member).
    memberships: Vec<(String, String)>,
    /// The group and member the removal names, in the spelling it names them.
    group: String,
    member: String,
    /// Whether the removal names the member's own realm (upper-cased, since a realm is
    /// case insensitive) rather than leaving `MemberRealm` out.
    name_realm: bool,
    /// The memberships that are no business of this removal: (group, member).
    remaining: Vec<(String, String)>,
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(group, member)| ((*group).to_string(), (*member).to_string()))
        .collect()
}

/// The table: one row per way the rule's inputs can differ.
fn cases() -> Vec<Case> {
    // The longest name each member admits, so the rule is exercised on its published
    // bound and not only on short names. The longest member is a GROUP object: a user
    // object's name stops at `UserName`'s own, shorter maximum.
    let longest_group = "g".repeat(GROUP_NAME_CEILING);
    let longest_member = "m".repeat(MEMBER_NAME_CEILING);
    let longest_user = "u".repeat(USER_NAME_CEILING);

    vec![
        Case {
            what: "a user taken out of a group it was added to".to_string(),
            users: names(&["jdoe"]),
            groups: names(&["engineering"]),
            memberships: pairs(&[("engineering", "jdoe")]),
            group: "engineering".to_string(),
            member: "jdoe".to_string(),
            // The membership the create gave the user stands: this removal names
            // another group.
            remaining: pairs(&[(PRIMARY_USER_GROUP, "jdoe")]),
            ..Case::default()
        },
        Case {
            what: "a group and a member named in another letter case".to_string(),
            users: names(&["jdoe"]),
            groups: names(&["engineering"]),
            memberships: pairs(&[("engineering", "jdoe")]),
            group: "ENGINEERING".to_string(),
            member: "JDoe".to_string(),
            remaining: pairs(&[(PRIMARY_USER_GROUP, "jdoe")]),
            ..Case::default()
        },
        Case {
            what: "a nested group object taken out of its parent group".to_string(),
            groups: names(&["engineering", "backend"]),
            memberships: pairs(&[("engineering", "backend")]),
            group: "engineering".to_string(),
            member: "backend".to_string(),
            ..Case::default()
        },
        Case {
            what: "one member of a group that holds several".to_string(),
            users: names(&["alice", "bob", "carol"]),
            groups: names(&["engineering"]),
            memberships: pairs(&[
                ("engineering", "alice"),
                ("engineering", "bob"),
                ("engineering", "carol"),
            ]),
            group: "engineering".to_string(),
            member: "bob".to_string(),
            remaining: pairs(&[
                ("engineering", "alice"),
                ("engineering", "carol"),
                (PRIMARY_USER_GROUP, "alice"),
                (PRIMARY_USER_GROUP, "bob"),
                (PRIMARY_USER_GROUP, "carol"),
            ]),
            ..Case::default()
        },
        Case {
            what: "a member held by two groups, taken out of one of them".to_string(),
            users: names(&["jdoe"]),
            groups: names(&["engineering", "oncall"]),
            memberships: pairs(&[("engineering", "jdoe"), ("oncall", "jdoe")]),
            group: "engineering".to_string(),
            member: "jdoe".to_string(),
            remaining: pairs(&[("oncall", "jdoe"), (PRIMARY_USER_GROUP, "jdoe")]),
            ..Case::default()
        },
        Case {
            what: "a user taken out of a group the domain was created with".to_string(),
            users: names(&["jdoe"]),
            memberships: pairs(&[(BUILTIN_GROUP, "jdoe")]),
            group: BUILTIN_GROUP.to_string(),
            member: "jdoe".to_string(),
            remaining: pairs(&[(PRIMARY_USER_GROUP, "jdoe")]),
            ..Case::default()
        },
        Case {
            what: "a removal naming the member's own realm".to_string(),
            users: names(&["jdoe"]),
            groups: names(&["engineering"]),
            memberships: pairs(&[("engineering", "jdoe")]),
            group: "engineering".to_string(),
            member: "jdoe".to_string(),
            name_realm: true,
            remaining: pairs(&[(PRIMARY_USER_GROUP, "jdoe")]),
        },
        Case {
            what: "the longest group name and member name the model admits".to_string(),
            groups: names(&[&longest_group, &longest_member]),
            memberships: pairs(&[(&longest_group, &longest_member)]),
            group: longest_group.clone(),
            member: longest_member.clone(),
            ..Case::default()
        },
        Case {
            what: "the longest name a user object can answer to".to_string(),
            users: names(&[&longest_user]),
            groups: names(&["engineering"]),
            memberships: pairs(&[("engineering", &longest_user)]),
            group: "engineering".to_string(),
            member: longest_user.clone(),
            remaining: pairs(&[(PRIMARY_USER_GROUP, &longest_user)]),
            ..Case::default()
        },
    ]
}

/// A removal forgets exactly the membership it names, on both sides of the relation,
/// and leaves every other membership standing.
#[tokio::test]
async fn a_removal_forgets_exactly_the_membership_it_names() {
    for case in cases() {
        let what = &case.what;
        // One directory per row, so a row's objects are the only ones in it.
        let router = emulator();
        for user in &case.users {
            served(
                &format!("case {what:?}: CreateUser({user:?})"),
                &create_user(&router, user).await,
            );
        }
        for group in &case.groups {
            served(
                &format!("case {what:?}: CreateGroup({group:?})"),
                &create_group(&router, group).await,
            );
        }
        for (group, member) in &case.memberships {
            served(
                &format!("case {what:?}: AddGroupMember({group:?}, {member:?})"),
                &add_group_member(&router, group, member).await,
            );
        }

        // The membership is there to be removed, so the reads below are about the
        // removal and not about an add that never happened.
        assert!(
            holds(&members_of(&router, &case.group).await, &case.member),
            "case {what:?}: {:?} must hold {:?} before the removal, got {:?}",
            case.group,
            case.member,
            members_of(&router, &case.group).await
        );

        let mut body = json!({"GroupName": case.group, "MemberName": case.member});
        if case.name_realm {
            body["MemberRealm"] = json!(own_realm(&router).await.to_uppercase());
        }
        let removed = remove_group_member(&router, DIRECTORY, &body).await;
        assert_eq!(
            removed.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            removed.status, removed.body
        );
        assert!(
            removed.body.get("__type").is_none() && removed.message().is_none(),
            "case {what:?}: a served removal carries no error member, got {:?}",
            removed.body
        );

        // Both sides of the one membership forgot it.
        let members = members_of(&router, &case.group).await;
        assert!(
            !holds(&members, &case.member),
            "case {what:?}: {:?} must no longer hold {:?}, got {members:?}",
            case.group,
            case.member
        );
        let groups = groups_of(&router, &case.member).await;
        assert!(
            !holds(&groups, &case.group),
            "case {what:?}: {:?} must no longer be reported for {:?}, got {groups:?}",
            case.group,
            case.member
        );

        // Nothing else moved: every other membership is still reported from both sides.
        for (group, member) in &case.remaining {
            let members = members_of(&router, group).await;
            assert!(
                holds(&members, member),
                "case {what:?}: {group:?} must still hold {member:?}, got {members:?}"
            );
            let groups = groups_of(&router, member).await;
            assert!(
                holds(&groups, group),
                "case {what:?}: {group:?} must still be reported for {member:?}, got {groups:?}"
            );
        }

        // The membership is gone, so the same removal has nothing left to name.
        let again = remove_group_member(&router, DIRECTORY, &body).await;
        assert_eq!(
            again.status, 404,
            "case {what:?}: repeating the removal must be refused, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            NOT_FOUND,
            "case {what:?}: expected {NOT_FOUND}, got {:?}",
            again.body
        );
    }
}

/// A name that resolves to no held membership is refused, and refusing changes nothing.
#[tokio::test]
async fn a_name_that_names_no_held_membership_is_refused() {
    let router = emulator();
    served("CreateUser(jdoe)", &create_user(&router, "jdoe").await);
    served("CreateUser(alice)", &create_user(&router, "alice").await);
    served(
        "CreateGroup(engineering)",
        &create_group(&router, "engineering").await,
    );
    served(
        "AddGroupMember(engineering, jdoe)",
        &add_group_member(&router, "engineering", "jdoe").await,
    );
    let realm = own_realm(&router).await;

    let refusals: Vec<(&str, &str, Value)> = vec![
        (
            "a directory nothing has ever written to",
            OTHER_DIRECTORY,
            json!({"GroupName": "engineering", "MemberName": "jdoe"}),
        ),
        (
            "a group the directory holds no object for",
            DIRECTORY,
            json!({"GroupName": "no-such-group", "MemberName": "jdoe"}),
        ),
        (
            "a member the directory holds no object for",
            DIRECTORY,
            json!({"GroupName": "engineering", "MemberName": "nobody"}),
        ),
        (
            "an object of the directory the group does not hold",
            DIRECTORY,
            json!({"GroupName": "engineering", "MemberName": "alice"}),
        ),
        (
            "a MemberRealm naming a domain this directory holds no objects in",
            DIRECTORY,
            json!({
                "GroupName": "engineering",
                "MemberName": "jdoe",
                "MemberRealm": ANOTHER_REALM,
            }),
        ),
    ];

    for (what, directory, body) in refusals {
        let refused = remove_group_member(&router, directory, &body).await;
        assert_eq!(
            refused.status, 404,
            "case {what:?}: expected HTTP 404, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            NOT_FOUND,
            "case {what:?}: expected {NOT_FOUND}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.error_type, NOT_FOUND,
            "case {what:?}: expected x-amzn-errortype {NOT_FOUND}, got {:?}",
            refused.error_type
        );
        assert!(
            refused.message().is_some_and(|prose| !prose.is_empty()),
            "case {what:?}: the refusal must state what was not found, got {:?}",
            refused.body
        );
    }

    // A refusal removed nothing: the membership and the objects are as they were.
    let members = members_of(&router, "engineering").await;
    assert!(
        holds(&members, "jdoe"),
        "a refused removal must leave the membership standing, got {members:?}"
    );
    assert!(
        holds(&groups_of(&router, "alice").await, PRIMARY_USER_GROUP),
        "a refused removal must leave the other objects' memberships standing, got {:?}",
        groups_of(&router, "alice").await
    );
    // The realm the cases above named is this directory's own, so the foreign-realm row
    // differs from the served ones only in the domain it names.
    assert!(
        !realm.eq_ignore_ascii_case(ANOTHER_REALM),
        "the foreign realm must not be the directory's own, got {realm:?}"
    );
}

/// `ClientToken` is this operation's `@idempotencyToken`: the same token with the same
/// parameters answers as the call it repeats, and with different ones is a conflict.
#[tokio::test]
async fn a_client_token_makes_a_repeat_answer_as_the_call_it_repeats() {
    let router = emulator();
    served("CreateUser(jdoe)", &create_user(&router, "jdoe").await);
    served("CreateUser(alice)", &create_user(&router, "alice").await);
    served(
        "CreateGroup(engineering)",
        &create_group(&router, "engineering").await,
    );
    served(
        "AddGroupMember(engineering, jdoe)",
        &add_group_member(&router, "engineering", "jdoe").await,
    );
    served(
        "AddGroupMember(engineering, alice)",
        &add_group_member(&router, "engineering", "alice").await,
    );

    let token = "remove-group-member-token-1";
    let first = remove_group_member(
        &router,
        DIRECTORY,
        &json!({"GroupName": "engineering", "MemberName": "jdoe", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first removal must be served, got {} with {:?}",
        first.status, first.body
    );

    // The same call again under the same token: the token names the call that was
    // already served, so the repeat answers as it did rather than as a removal with
    // nothing left to name.
    let replay = remove_group_member(
        &router,
        DIRECTORY,
        &json!({"GroupName": "engineering", "MemberName": "jdoe", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        replay.status, 200,
        "a repeat under the same token must answer as the first call, got {} with {:?}",
        replay.status, replay.body
    );
    let members = members_of(&router, "engineering").await;
    assert!(
        !holds(&members, "jdoe"),
        "the repeat must leave the records as the first call left them, got {members:?}"
    );

    // The same token for a DIFFERENT call is the documented conflict, and it removes
    // nothing.
    let conflict = remove_group_member(
        &router,
        DIRECTORY,
        &json!({"GroupName": "engineering", "MemberName": "alice", "ClientToken": token}),
    )
    .await;
    assert_eq!(
        conflict.status, 409,
        "the same token with different parameters must conflict, got {} with {:?}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error_code(),
        CONFLICT,
        "expected {CONFLICT}, got {:?}",
        conflict.body
    );
    let members = members_of(&router, "engineering").await;
    assert!(
        holds(&members, "alice"),
        "a conflicting removal must remove nothing, got {members:?}"
    );

    // Without a token there is no record of an earlier call, so a membership already
    // gone is simply not there.
    let untokened = remove_group_member(
        &router,
        DIRECTORY,
        &json!({"GroupName": "engineering", "MemberName": "jdoe"}),
    )
    .await;
    assert_eq!(
        untokened.status, 404,
        "a membership already gone must be refused when no token names the call that \
         removed it, got {} with {:?}",
        untokened.status, untokened.body
    );
    assert_eq!(
        untokened.error_code(),
        NOT_FOUND,
        "expected {NOT_FOUND}, got {:?}",
        untokened.body
    );
}

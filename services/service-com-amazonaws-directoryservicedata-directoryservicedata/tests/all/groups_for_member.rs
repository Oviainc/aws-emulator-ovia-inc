// SPDX-License-Identifier: CC0-1.0

//! `ListGroupsForMember`: the member side of a Directory Service Data membership, end
//! to end over HTTP.
//!
//! The rule under test is the one a directory's membership relation states: one
//! membership is one record, so the groups reported for a member are exactly the groups
//! whose stored member set holds that member — projected from each group's own record,
//! in group-name order, one page of at most `MaxResults` of them, with a `NextToken`
//! only while groups remain. A member the directory holds no object for is
//! `ResourceNotFoundException`, and a member in no group is an empty list rather than a
//! refusal.
//!
//! Every case drives the router, so it exercises routing, deserialization, the handler,
//! the persisted records and serialization — the same path a client takes. The table
//! below spans what the rule generalises over: the member's kind (a user object and a
//! nested group object), the spelling the member is asked about in, how many groups hold
//! it (none, one, several, every one of them), and each `GroupType`/`GroupScope` pair a
//! reported group can carry.
//!
//! One group holds every user without anything adding it: the primary user group Active
//! Directory creates with the domain, so a user no call ever added to a group is still
//! reported as a member of that one.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservicedata_directoryservicedata::ops::types::{
    GROUP_SCOPE_VALUES, GROUP_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::validation_envelope::{call, emulator, Answer, DIRECTORY, VALIDATION_EXCEPTION};

/// A second `DirectoryId`, for the cases that are about one directory not answering for
/// another.
const OTHER_DIRECTORY: &str = "d-00abcdef99";

/// A domain name that is not the realm the emulator's directories carry, inside
/// `Realm`'s `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`.
const ANOTHER_REALM: &str = "elsewhere.example.org";

/// The largest page `MaxResults` admits (`@range(min: 1, max: 250)`), spelled out here
/// rather than imported so a wrong bound in the implementation fails.
const MAX_RESULTS_CEILING: i64 = 250;

/// The group Active Directory creates with a domain and makes every user account's
/// primary group, so every user of the directory is reported in it.
const PRIMARY_USER_GROUP: &str = "Domain Users";

/// The well-known RID Windows gives the domain's users group, which its SID must end
/// in — it is not minted from the sequence a created object's RID comes from.
const PRIMARY_USER_GROUP_RID: u64 = 513;

const CREATE_USER: &str = "/Users/CreateUser";
const CREATE_GROUP: &str = "/Groups/CreateGroup";
const ADD_GROUP_MEMBER: &str = "/GroupMemberships/AddGroupMember";
const LIST_GROUP_MEMBERS: &str = "/GroupMemberships/ListGroupMembers";
const LIST_GROUPS_FOR_MEMBER: &str = "/GroupMemberships/ListGroupsForMember";

async fn create_user(router: &MultiServiceRouter, directory: &str, name: &str) -> Answer {
    call(
        router,
        CREATE_USER,
        directory,
        &json!({"SAMAccountName": name}),
    )
    .await
}

async fn create_group(
    router: &MultiServiceRouter,
    directory: &str,
    name: &str,
    group_type: Option<&str>,
    group_scope: Option<&str>,
) -> Answer {
    let mut body = json!({"SAMAccountName": name});
    if let Some(group_type) = group_type {
        body["GroupType"] = json!(group_type);
    }
    if let Some(group_scope) = group_scope {
        body["GroupScope"] = json!(group_scope);
    }
    call(router, CREATE_GROUP, directory, &body).await
}

async fn add_group_member(
    router: &MultiServiceRouter,
    directory: &str,
    group: &str,
    member: &str,
) -> Answer {
    call(
        router,
        ADD_GROUP_MEMBER,
        directory,
        &json!({"GroupName": group, "MemberName": member}),
    )
    .await
}

async fn list_group_members(router: &MultiServiceRouter, directory: &str, body: &Value) -> Answer {
    call(router, LIST_GROUP_MEMBERS, directory, body).await
}

async fn list_groups_for_member(
    router: &MultiServiceRouter,
    directory: &str,
    body: &Value,
) -> Answer {
    call(router, LIST_GROUPS_FOR_MEMBER, directory, body).await
}

/// A string member of an answer, or the empty string when it carries none.
fn text(answer: &Answer, member: &str) -> String {
    answer.body[member].as_str().unwrap_or_default().to_string()
}

/// The groups of a `ListGroupsForMember` page, in the order it reported them.
fn listed_groups(answer: &Answer) -> Vec<Value> {
    answer.body["Groups"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListGroupsForMember must report a Groups list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `SAMAccountName`s of a `ListGroupsForMember` page, in the order it reported them.
fn listed_group_names(answer: &Answer) -> Vec<String> {
    listed_groups(answer)
        .iter()
        .map(|group| {
            group["SAMAccountName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `SAMAccountName`s of a `ListGroupMembers` page, in the order it reported them.
fn listed_member_names(answer: &Answer) -> Vec<String> {
    answer.body["Members"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListGroupMembers must report a Members list, got {:?}",
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

/// One group the fixture created: its name, the SID the create minted for it, and the
/// `GroupType`/`GroupScope` the create named — `None` for the group created naming
/// neither, whose values are the directory's own defaults.
#[derive(Clone)]
struct CreatedGroup {
    name: String,
    sid: String,
    group_type: Option<&'static str>,
    group_scope: Option<&'static str>,
}

/// One group per `GroupType`/`GroupScope` pair the model admits, plus one created naming
/// neither, so every projection a reported `GroupSummary` can carry is exercised.
///
/// The pairs come from the model's own value sets
/// (`smithy_query(query="shape", shape="GroupType")`,
/// `smithy_query(query="shape", shape="GroupScope")`), re-exported as
/// `GROUP_TYPE_VALUES` / `GROUP_SCOPE_VALUES`.
fn group_plan() -> Vec<(String, Option<&'static str>, Option<&'static str>)> {
    let mut plan = vec![("group-defaults".to_string(), None, None)];
    for group_type in GROUP_TYPE_VALUES {
        for group_scope in GROUP_SCOPE_VALUES {
            plan.push((
                format!(
                    "grp-{}-{}",
                    group_type.to_lowercase(),
                    group_scope.to_lowercase()
                ),
                Some(*group_type),
                Some(*group_scope),
            ));
        }
    }
    plan
}

/// Create every group of [`group_plan`] in `DIRECTORY` and report what the creates
/// stored.
async fn create_planned_groups(router: &MultiServiceRouter) -> Vec<CreatedGroup> {
    let mut created = Vec::new();
    for (name, group_type, group_scope) in group_plan() {
        let answer = create_group(router, DIRECTORY, &name, group_type, group_scope).await;
        assert_eq!(
            answer.status, 200,
            "group {name:?}: CreateGroup must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let sid = text(&answer, "SID");
        assert!(
            !sid.is_empty(),
            "group {name:?}: CreateGroup must mint a SID, got {:?}",
            answer.body
        );
        created.push(CreatedGroup {
            name,
            sid,
            group_type,
            group_scope,
        });
    }
    created
}

/// One member case: what the member is, which of the created groups hold it, and the
/// spelling the listing asks about it in.
struct MemberCase {
    what: &'static str,
    name: &'static str,
    /// `MemberType`: a user object or a group object.
    kind: &'static str,
    /// Positions in [`create_planned_groups`]' answer.
    in_groups: Vec<usize>,
    /// The case the member is asked about in — AD resolves an account name case
    /// insensitively.
    asked_as: &'static str,
}

/// Positions in [`create_planned_groups`]' answer, folded into the plan's actual size
/// so a table row stays in range whatever the model's value sets hold.
fn positions(group_count: usize, wanted: &[usize]) -> Vec<usize> {
    let mut positions: Vec<usize> = wanted
        .iter()
        .map(|position| position % group_count.max(1))
        .collect();
    positions.sort_unstable();
    positions.dedup();
    positions
}

fn member_cases(group_count: usize) -> Vec<MemberCase> {
    vec![
        MemberCase {
            what: "a user no call ever added to a group",
            name: "lonely",
            kind: "USER",
            in_groups: Vec::new(),
            asked_as: "lonely",
        },
        MemberCase {
            what: "a user in exactly one group",
            name: "jdoe",
            kind: "USER",
            in_groups: positions(group_count, &[0]),
            asked_as: "jdoe",
        },
        MemberCase {
            what: "a user in several groups",
            name: "asmith",
            kind: "USER",
            in_groups: positions(group_count, &[1, 3, 5]),
            asked_as: "asmith",
        },
        MemberCase {
            what: "a user in every group the directory holds",
            name: "ubiquitous",
            kind: "USER",
            in_groups: (0..group_count).collect(),
            asked_as: "ubiquitous",
        },
        MemberCase {
            what: "a mixed-case name asked about in another case",
            name: "MixedCase",
            kind: "USER",
            in_groups: positions(group_count, &[2, 4]),
            asked_as: "mIXEDcASE",
        },
        MemberCase {
            what: "a one-character name, the shortest MemberName admits",
            name: "a",
            kind: "USER",
            in_groups: positions(group_count, &[6]),
            asked_as: "A",
        },
        MemberCase {
            what: "a group object nested in other groups",
            name: "nested-group",
            kind: "GROUP",
            in_groups: positions(group_count, &[0, 7]),
            asked_as: "Nested-Group",
        },
    ]
}

/// Create each member of the table and record the memberships it names.
async fn create_members(router: &MultiServiceRouter, groups: &[CreatedGroup]) -> Vec<MemberCase> {
    let cases = member_cases(groups.len());
    for case in &cases {
        let created = if case.kind == "GROUP" {
            create_group(router, DIRECTORY, case.name, None, None).await
        } else {
            create_user(router, DIRECTORY, case.name).await
        };
        assert_eq!(
            created.status, 200,
            "case {:?}: creating the member must be accepted, got {} with {:?}",
            case.what, created.status, created.body
        );
        for position in &case.in_groups {
            let group = &groups[*position].name;
            let added = add_group_member(router, DIRECTORY, group, case.name).await;
            assert_eq!(
                added.status, 200,
                "case {:?}: AddGroupMember({group:?}) must be accepted, got {} with {:?}",
                case.what, added.status, added.body
            );
        }
    }
    cases
}

/// The group names a member case expects back, in the order the listing reports them:
/// a directory's objects are ordered by their case-folded account name.
///
/// A user is in the domain's primary user group besides the groups it was added to; a
/// group object is not.
fn expected_group_names(case: &MemberCase, groups: &[CreatedGroup]) -> Vec<String> {
    let mut names: Vec<String> = case
        .in_groups
        .iter()
        .map(|position| groups[*position].name.clone())
        .collect();
    if case.kind == "USER" {
        names.push(PRIMARY_USER_GROUP.to_string());
    }
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// `ListGroupsForMember` reports exactly the groups whose stored member set holds the
/// member, projected from each group's own record, and the same membership is reported
/// by `ListGroupMembers` from the group's side.
#[tokio::test]
async fn the_groups_reported_for_a_member_are_the_groups_that_hold_it() {
    let router = emulator();
    let groups = create_planned_groups(&router).await;
    let cases = create_members(&router, &groups).await;

    for case in &cases {
        let what = case.what;
        let answer =
            list_groups_for_member(&router, DIRECTORY, &json!({"SAMAccountName": case.name})).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: ListGroupsForMember must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let expected = expected_group_names(case, &groups);
        assert_eq!(
            listed_group_names(&answer),
            expected,
            "case {what:?}: expected the groups holding the member, in group-name order, \
             got {:?}",
            answer.body
        );
        assert_eq!(
            text(&answer, "DirectoryId"),
            DIRECTORY,
            "case {what:?}: expected DirectoryId {DIRECTORY}, got {:?}",
            answer.body
        );
        let realm = text(&answer, "Realm");
        let member_realm = text(&answer, "MemberRealm");
        assert_eq!(
            realm, member_realm,
            "case {what:?}: a member and its groups are in the one domain of this \
             directory, got Realm {realm:?} and MemberRealm {member_realm:?}"
        );
        assert!(
            answer.body.get("NextToken").is_none(),
            "case {what:?}: every group fits in one page, so no NextToken is due, got {:?}",
            answer.body
        );

        // Each reported summary is the group's own record, not a copy made at the
        // membership.
        for reported in listed_groups(&answer) {
            let name = reported["SAMAccountName"].as_str().unwrap_or_default();
            if name == PRIMARY_USER_GROUP {
                // The group the domain comes with carries its well-known RID rather
                // than one minted for a created object.
                let sid = reported["SID"].as_str().unwrap_or_default();
                assert!(
                    sid.ends_with(&format!("-{PRIMARY_USER_GROUP_RID}")),
                    "case {what:?}: expected the primary user group's SID to end in \
                     {PRIMARY_USER_GROUP_RID}, got {reported:?}"
                );
                continue;
            }
            let stored = groups
                .iter()
                .find(|group| group.name == name)
                .unwrap_or_else(|| panic!("case {what:?}: unknown group {name:?} reported"));
            assert_eq!(
                reported["SID"],
                json!(stored.sid),
                "case {what:?}, group {name:?}: expected the SID the group's record holds \
                 ({:?}), got {reported:?}",
                stored.sid
            );
            match stored.group_type {
                Some(group_type) => assert_eq!(
                    reported["GroupType"],
                    json!(group_type),
                    "case {what:?}, group {name:?}: expected GroupType {group_type:?}, \
                     got {reported:?}"
                ),
                // The create named none, so the directory's default answers — which is
                // one of the values the model admits.
                None => assert!(
                    GROUP_TYPE_VALUES
                        .contains(&reported["GroupType"].as_str().unwrap_or_default()),
                    "case {what:?}, group {name:?}: expected a modelled GroupType, \
                     got {reported:?}"
                ),
            }
            match stored.group_scope {
                Some(group_scope) => assert_eq!(
                    reported["GroupScope"],
                    json!(group_scope),
                    "case {what:?}, group {name:?}: expected GroupScope {group_scope:?}, \
                     got {reported:?}"
                ),
                None => assert!(
                    GROUP_SCOPE_VALUES
                        .contains(&reported["GroupScope"].as_str().unwrap_or_default()),
                    "case {what:?}, group {name:?}: expected a modelled GroupScope, \
                     got {reported:?}"
                ),
            }
        }

        // The same name in another case names the same member.
        let folded =
            list_groups_for_member(&router, DIRECTORY, &json!({"SAMAccountName": case.asked_as}))
                .await;
        assert_eq!(
            listed_group_names(&folded),
            expected,
            "case {what:?}: asking about {:?} must answer as asking about {:?} does, got {:?}",
            case.asked_as,
            case.name,
            folded.body
        );

        // The one membership record, read from the group's side: every group reported
        // for the member reports the member, and no other group does.
        for group in &groups {
            let members =
                list_group_members(&router, DIRECTORY, &json!({"SAMAccountName": group.name}))
                    .await;
            let holds = listed_member_names(&members)
                .iter()
                .any(|name| name.eq_ignore_ascii_case(case.name));
            assert_eq!(
                holds,
                expected.contains(&group.name),
                "case {what:?}, group {:?}: the two sides of a membership must agree, \
                 ListGroupMembers reported {:?}",
                group.name,
                members.body
            );
        }
    }

    // The primary user group reads the same from its own side: it holds every user of
    // the directory and no group object.
    let primary = list_group_members(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": PRIMARY_USER_GROUP, "MaxResults": MAX_RESULTS_CEILING}),
    )
    .await;
    let mut expected_users: Vec<String> = cases
        .iter()
        .filter(|case| case.kind == "USER")
        .map(|case| case.name.to_string())
        .collect();
    expected_users.sort_by_key(|name| name.to_lowercase());
    assert_eq!(
        listed_member_names(&primary),
        expected_users,
        "the primary user group holds every user of the directory and nothing else, \
         got {:?}",
        primary.body
    );
}

/// `ListGroupsForMember` honours `MaxResults` as a hard page size and resumes exactly
/// after the group the previous page ended on, so walking every page enumerates every
/// group holding the member once, in order.
#[tokio::test]
async fn list_groups_for_member_pages_through_the_groups_that_hold_it() {
    let router = emulator();
    let groups = create_planned_groups(&router).await;
    create_user(&router, DIRECTORY, "walker").await;
    for group in &groups {
        add_group_member(&router, DIRECTORY, &group.name, "walker").await;
    }
    let mut expected: Vec<String> = groups.iter().map(|group| group.name.clone()).collect();
    // Plus the group the user is in from the moment it was created.
    expected.push(PRIMARY_USER_GROUP.to_string());
    expected.sort_by_key(|name| name.to_lowercase());

    let last_page = i64::try_from(expected.len()).expect("the group count fits in i64");
    for page_size in [1_i64, 2, 3, last_page, last_page + 1, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"SAMAccountName": "walker", "MaxResults": page_size});
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = list_groups_for_member(&router, DIRECTORY, &request).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: ListGroupsForMember must be accepted, \
                 got {} with {:?}",
                page.status, page.body
            );
            let names = listed_group_names(&page);
            assert!(
                i64::try_from(names.len()).expect("a page fits in i64") <= page_size,
                "case maxResults={page_size}: a page must hold at most {page_size} groups, \
                 got {names:?}"
            );
            walked.extend(names);
            assert!(
                walked.len() <= expected.len(),
                "case maxResults={page_size}: paging must terminate, walked {walked:?}"
            );
            token = page.body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking every page must enumerate every group \
             holding the member once, in group-name order"
        );
    }
}

/// What `ListGroupsForMember` refuses, and what it serves as an empty list: a member no
/// object of the addressed directory answers to is not found, while a domain that holds
/// none of this directory's groups simply reports none.
#[tokio::test]
async fn a_member_the_directory_holds_no_object_for_is_not_found() {
    let router = emulator();
    let groups = create_planned_groups(&router).await;
    let group = groups.first().expect("a group was created").name.clone();
    create_user(&router, DIRECTORY, "jdoe").await;
    add_group_member(&router, DIRECTORY, &group, "jdoe").await;
    // The group the add recorded, plus the one the create put the user in.
    let mut holding = vec![group.clone(), PRIMARY_USER_GROUP.to_string()];
    holding.sort_by_key(|name| name.to_lowercase());
    let realm = text(
        &list_groups_for_member(&router, DIRECTORY, &json!({"SAMAccountName": "jdoe"})).await,
        "MemberRealm",
    );

    let not_found_cases: Vec<(&str, &str, Value)> = vec![
        (
            "a name no object of the directory answers to",
            DIRECTORY,
            json!({"SAMAccountName": "nobody"}),
        ),
        (
            "a member of another directory",
            OTHER_DIRECTORY,
            json!({"SAMAccountName": "jdoe"}),
        ),
        (
            "a MemberRealm naming a domain this directory holds no objects in",
            DIRECTORY,
            json!({"SAMAccountName": "jdoe", "MemberRealm": ANOTHER_REALM}),
        ),
    ];
    for (what, directory, body) in not_found_cases {
        let refused = list_groups_for_member(&router, directory, &body).await;
        assert_eq!(
            refused.status, 404,
            "case {what:?}: expected HTTP 404, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
    }

    // The member's own domain, spelled in any case, names the member the directory
    // holds — a realm is case insensitive.
    for spelling in [realm.clone(), realm.to_uppercase()] {
        let served = list_groups_for_member(
            &router,
            DIRECTORY,
            &json!({"SAMAccountName": "jdoe", "MemberRealm": spelling.clone()}),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case MemberRealm={spelling:?}: the directory's own domain must be served, \
             got {} with {:?}",
            served.status, served.body
        );
        assert_eq!(
            listed_group_names(&served),
            holding,
            "case MemberRealm={spelling:?}: expected the groups holding the member, got {:?}",
            served.body
        );
    }

    // `Realm` names the domain the GROUPS are in: another domain holds none of this
    // directory's groups, which is an empty page and not a refusal.
    let elsewhere = list_groups_for_member(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "jdoe", "Realm": ANOTHER_REALM}),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "a group domain this directory holds nothing in must still be served, got {} with {:?}",
        elsewhere.status, elsewhere.body
    );
    assert!(
        listed_group_names(&elsewhere).is_empty(),
        "no group of this directory is in another domain, got {:?}",
        elsewhere.body
    );
    assert_eq!(
        text(&elsewhere, "Realm"),
        ANOTHER_REALM,
        "the answer must report the domain the call asked about, got {:?}",
        elsewhere.body
    );
}

/// A `NextToken` this listing did not issue is refused: a token is a position in one
/// walk, so one built by a client, one issued for another member and one issued by
/// another listing are all `ValidationException`.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let router = emulator();
    let groups = create_planned_groups(&router).await;
    for member in ["jdoe", "asmith"] {
        create_user(&router, DIRECTORY, member).await;
        for group in &groups {
            add_group_member(&router, DIRECTORY, &group.name, member).await;
        }
    }

    let for_other_member = list_groups_for_member(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": "asmith", "MaxResults": 1}),
    )
    .await;
    let other_member_token = for_other_member
        .body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "a page of 1 of several groups must carry a NextToken, got {:?}",
                for_other_member.body
            )
        })
        .to_string();

    let group = groups.first().expect("a group was created").name.clone();
    let from_members = list_group_members(
        &router,
        DIRECTORY,
        &json!({"SAMAccountName": group, "MaxResults": 1}),
    )
    .await;
    let members_token = from_members.body["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "a page of 1 of two members must carry a NextToken, got {:?}",
                from_members.body
            )
        })
        .to_string();

    let cases: Vec<(&str, Value)> = vec![
        (
            "a token no listing issued",
            json!({"SAMAccountName": "jdoe", "NextToken": "not-a-token-this-listing-issued"}),
        ),
        (
            "a token issued for another member",
            json!({"SAMAccountName": "jdoe", "NextToken": other_member_token}),
        ),
        (
            "a token issued by another listing",
            json!({"SAMAccountName": "jdoe", "NextToken": members_token}),
        ),
    ];
    for (what, body) in cases {
        let refused = list_groups_for_member(&router, DIRECTORY, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            VALIDATION_EXCEPTION,
            "case {what:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body.get("Reason").and_then(Value::as_str),
            Some("INVALID_NEXT_TOKEN"),
            "case {what:?}: expected Reason INVALID_NEXT_TOKEN, got {:?}",
            refused.body
        );
    }
}

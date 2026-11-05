// SPDX-License-Identifier: CC0-1.0

//! `GetGroup`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rule under test: a read of ONE IAM entity addressed by name reports the record
//! the account holds under that name — names "are not distinguished by case"
//! (`CreateGroup$GroupName`) — together with the related collection that entity holds,
//! paged by `MaxItems` with an opaque `Marker` a follow-up call resumes strictly after,
//! and refuses a name the account holds no such entity for with `NoSuchEntity` (404).
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: the
//! group's name alphabet and path depth, the case the request spells the name in, how
//! many users the group holds, the page size (both published bounds included) and the
//! form of a `Marker` that names no position in this group's membership.
//!
//! The memberships these tests read back are seeded into the service's OWN store
//! (`state::group_members`) rather than into a second copy, so what they assert is the
//! READ and nothing else; `tests/all/group_membership_writes.rs` is the suite that
//! drives `AddUserToGroup`, the operation that writes one.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router_with_engine};

/// The fixture user set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="operation", operation="CreateUser")`), and paths spanning
/// the root, a one-segment path and a nested path, so a rule about what a member record
/// carries is exercised at more than one depth.
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

/// Every fixture user's name.
fn all_users() -> Vec<&'static str> {
    USERS.iter().map(|(name, _)| *name).collect()
}

/// One created group, as the account holds it.
#[derive(Debug)]
struct CreatedGroup {
    group_name: String,
    group_id: String,
    arn: String,
    create_date: String,
}

/// Create one group, answering the record its create reported.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) -> CreatedGroup {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
    );
    CreatedGroup {
        group_name: element(&body, "GroupName"),
        group_id: element(&body, "GroupId"),
        arn: element(&body, "Arn"),
        create_date: element(&body, "CreateDate"),
    }
}

/// Create every fixture user, answering the router and engine they were created in.
async fn with_fixture_users() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_engine();
    for (name, path) in USERS {
        let (status, body) = call(&router, "CreateUser", &[("UserName", name), ("Path", path)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the fixture CreateUser must succeed, got {status}: {body}"
        );
    }
    (router, engine)
}

/// Make `members` the membership of the group `group_name` names, answering the member
/// names in the order a read of the group must report them: ascending by the case-folded
/// user name, which is the key the store is ordered on.
///
/// This is the state an `AddUserToGroup` ("adds the specified user to the specified
/// group") writes, seeded into the service's own store here rather than into a second
/// copy, so these cases exercise the READ alone.
fn seed_members(
    engine: &engine::InMemoryEngine,
    group_name: &str,
    members: &[&str],
) -> Vec<String> {
    let store = state::service_state(engine).expect("the IAM store");
    let mut store = store.lock();
    for member in members {
        let member_key = state::name_key(member);
        assert!(
            store.users.contains_key(&member_key),
            "case {group_name:?}: the member {member:?} must be a user the account holds"
        );
        store
            .group_members
            .entry(state::name_key(group_name))
            .or_default()
            .insert(member_key);
    }
    let mut ordered: Vec<String> = members.iter().map(|name| name.to_string()).collect();
    ordered.sort_by_key(|name| name.to_ascii_lowercase());
    ordered
}

/// One group whose read must report the group and the users it holds.
struct MembershipCase {
    /// The name the group was created with.
    group_name: &'static str,
    path: &'static str,
    /// The users the group holds.
    members: Vec<&'static str>,
    /// The case the READ spells the group's name in.
    read_as: &'static str,
}

/// The rows: group names spanning the `^[\w+=,.@-]+$` alphabet and both cases, paths
/// spanning the root and two depths, memberships from none through every fixture user,
/// and a read spelling the name in a different case from the create.
fn membership_cases() -> Vec<MembershipCase> {
    vec![
        MembershipCase {
            group_name: "admins",
            path: "/eng/",
            members: vec![],
            read_as: "admins",
        },
        MembershipCase {
            group_name: "Billing",
            path: "/",
            members: vec!["Bruno"],
            read_as: "billing",
        },
        MembershipCase {
            group_name: "core_7",
            path: "/eng/tools/",
            members: vec!["gustav@3", "ada", "cleo_7"],
            read_as: "CORE_7",
        },
        MembershipCase {
            group_name: "data.x",
            path: "/sales/",
            members: vec!["Enzo+1", "fatima=2"],
            read_as: "Data.X",
        },
        MembershipCase {
            group_name: "eng+1",
            path: "/sales/west/",
            members: all_users(),
            read_as: "eng+1",
        },
    ]
}

/// RULE: the read reports the STORED record of the entity its name resolves to — names
/// are not distinguished by case — beside the users that entity holds, in one total
/// order, and a complete page is not truncated and carries no `Marker`.
#[tokio::test]
async fn get_group_reports_the_stored_group_and_the_users_it_holds() {
    for case in membership_cases() {
        let MembershipCase {
            group_name,
            path,
            members,
            read_as,
        } = case;
        let (router, engine) = with_fixture_users().await;
        let created = create_group(&router, group_name, path).await;
        let expected = seed_members(&engine, group_name, &members);

        let (status, body) = call(&router, "GetGroup", &[("GroupName", read_as)]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?} read as {read_as:?}: expected 200, got {status}: {body}"
        );

        // The group comes back as the account holds it, with the name, id, ARN and
        // creation instant its create minted — not a value recomputed from the read.
        assert_eq!(
            element(&body, "GroupName"),
            created.group_name,
            "case {group_name:?} read as {read_as:?}: expected the stored GroupName, got {:?}",
            element(&body, "GroupName")
        );
        assert_eq!(
            element(&body, "GroupId"),
            created.group_id,
            "case {group_name:?}: expected the GroupId the create minted, got {:?}",
            element(&body, "GroupId")
        );
        let arns = elements(&body, "Arn");
        assert!(
            arns.contains(&created.arn),
            "case {group_name:?}: expected the group's ARN {:?} among {arns:?}",
            created.arn
        );
        let dates = elements(&body, "CreateDate");
        assert!(
            dates.contains(&created.create_date),
            "case {group_name:?}: expected the group's CreateDate {:?} among {dates:?}",
            created.create_date
        );

        // The membership: every member the group holds, once, in the store's order.
        assert_eq!(
            elements(&body, "UserName"),
            expected,
            "case {group_name:?}: expected the users the group holds, got {body}"
        );
        for member in &members {
            let member_arn = state::user_arn(
                USERS
                    .iter()
                    .find(|(name, _)| name == member)
                    .map(|(_, path)| *path)
                    .expect("a fixture user"),
                member,
            );
            assert!(
                arns.contains(&member_arn),
                "case {group_name:?}: expected the member ARN {member_arn:?} among {arns:?}"
            );
        }
        assert_eq!(
            elements(&body, "UserId").len(),
            members.len(),
            "case {group_name:?}: every reported member must carry a UserId: {body}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {group_name:?}: a page holding every member is not truncated: {body}"
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {group_name:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );

        // A read cannot change what the next caller sees.
        let (status, again) = call(&router, "GetGroup", &[("GroupName", read_as)]).await;
        assert_eq!(
            status, 200,
            "case {group_name:?}: a second read was refused, got {status}: {again}"
        );
        assert_eq!(
            elements(&again, "UserName"),
            expected,
            "case {group_name:?}: a second read answered a different membership: {again}"
        );
        assert_eq!(
            element(&again, "GroupId"),
            created.group_id,
            "case {group_name:?}: a second read answered a different group: {again}"
        );
    }
}

/// RULE: `MaxItems` is a real page size: a page past it is truncated and carries a
/// `Marker` that resumes strictly after the last item reported, so draining the pages
/// enumerates every item exactly once; a `Marker` that names no position in THIS
/// collection is refused rather than read as the start of it.
#[tokio::test]
async fn get_group_pages_the_membership_with_a_resumable_marker() {
    let (router, engine) = with_fixture_users().await;
    create_group(&router, "paged", "/eng/").await;
    create_group(&router, "other", "/eng/").await;
    let all = seed_members(&engine, "paged", &all_users());
    seed_members(&engine, "other", &["ada"]);

    // Rows spanning page sizes from one at a time, through sizes that do and do not
    // divide the membership, to one more than it holds — and the published range's
    // bounds, 1 and 1000 (`maxItemsType`, range 1..1000).
    for page_size in [1usize, 2, 3, 5, 8, 9, 1000] {
        let mut drained: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let size = page_size.to_string();
            let mut params: Vec<(&str, &str)> =
                vec![("GroupName", "paged"), ("MaxItems", size.as_str())];
            if let Some(token) = marker.as_deref() {
                params.push(("Marker", token));
            }
            let (status, body) = call(&router, "GetGroup", &params).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status}: {body}"
            );
            let names = elements(&body, "UserName");
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} users a page, got {}: \
                 {body}",
                names.len()
            );
            // Every page reports the group itself, so a client paging the membership
            // never loses what it is reading.
            assert_eq!(
                element(&body, "GroupName"),
                "paged",
                "case page size {page_size}: every page must report the group: {body}"
            );
            drained.extend(names);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case page size {page_size}: paging did not terminate: {body}"
            );
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                assert!(
                    elements(&body, "Marker").is_empty(),
                    "case page size {page_size}: a complete page must carry no Marker: {body}"
                );
                break;
            }
        }
        assert_eq!(
            drained, all,
            "case page size {page_size}: expected the pages to enumerate every member exactly \
             once, in order"
        );
    }

    // A token this service never issued, one it issued for another collection, and one
    // it issued while paging ANOTHER group's membership are all refused: a marker is a
    // position in one collection.
    let (_, groups_page) = call(&router, "ListGroups", &[("MaxItems", "1")]).await;
    let (_, other_page) = call(
        &router,
        "GetGroup",
        &[("GroupName", "paged"), ("MaxItems", "1")],
    )
    .await;
    let foreign: Vec<String> = elements(&groups_page, "Marker");
    let paged_marker = element(&other_page, "Marker");
    for bogus in ["not-a-marker", "zz", "0g"]
        .iter()
        .map(|token| token.to_string())
        .chain(foreign)
        .chain([paged_marker])
    {
        let (status, body) = call(
            &router,
            "GetGroup",
            &[("GroupName", "other"), ("Marker", bogus.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected a foreign Marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
    }

    // The page size is refused outside its published range, at either bound.
    for (case, max_items) in [
        ("one below the minimum", "0"),
        ("one above the maximum", "1001"),
    ] {
        let (status, body) = call(
            &router,
            "GetGroup",
            &[("GroupName", "paged"), ("MaxItems", max_items)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a page size outside the range to be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the query protocol's bound refusal code, got {body}"
        );
    }
}

/// RULE: a name the account holds no entity of that TYPE under refers to no entity, so
/// the read is refused `NoSuchEntity` (404) with the entity type and the name the request
/// carried, and a name that names another type of entity is no exception.
#[tokio::test]
async fn get_group_refuses_a_name_the_account_holds_no_group_under() {
    let (router, engine) = with_fixture_users().await;
    create_group(&router, "admins", "/eng/").await;
    seed_members(&engine, "admins", &["ada"]);

    // Rows: a name nothing was ever created under, one differing from a held group by
    // more than case, one that names a USER rather than a group, and names spanning the
    // alphabet and both cases.
    for name in [
        "nobody",
        "admins2",
        "Admin",
        "ada",
        "Bruno",
        "ghost_7",
        "void.x",
        "NOPE+1",
    ] {
        let (status, body) = call(&router, "GetGroup", &[("GroupName", name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The group with name {name} cannot be found."),
            "case {name:?}: expected the refusal to name the group it could not find, got {:?}",
            element(&body, "Message")
        );
    }

    // The group the account DOES hold is still readable, so the refusals above are about
    // the name and not about the operation.
    let (status, body) = call(&router, "GetGroup", &[("GroupName", "admins")]).await;
    assert_eq!(status, 200, "expected the held group to be read: {body}");
    assert_eq!(
        elements(&body, "UserName"),
        vec!["ada".to_string()],
        "expected the held group's membership: {body}"
    );
}

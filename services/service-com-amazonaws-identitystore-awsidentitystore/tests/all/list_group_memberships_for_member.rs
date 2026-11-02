// SPDX-License-Identifier: CC0-1.0

//! `ListGroupMembershipsForMember`, driven end to end over HTTP.
//!
//! The rule under test: for the member a request names, the operation reports exactly
//! the memberships of that identity store whose member is that user — none of any other
//! member's, none of any other store's — in the order they were created, a page of at
//! most `MaxResults` items at a time, with an opaque `NextToken` when more are left.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: how
//! many groups the member belongs to, the page size asked for, which membership is
//! deleted, and the form the member's identifier takes. Nothing reaches into the state
//! struct — every case goes through the same `awsJson1_1` front door a client uses — so
//! a response that is not computed from persisted state cannot pass.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The wire harness is [`crate::harness`]'s and the resource-creation helpers are the
/// lifecycle suite's; this suite reuses them rather than carrying a second copy that
/// can drift.
use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{
    assert_resource_arn, create_group, create_user, user_name, walk_memberships, MAX_RESULTS_MAX,
    MAX_RESULTS_MIN,
};

/// The member of a request, as the modelled `MemberId` union writes it on the wire:
/// the union declares exactly one variant, `UserId`
/// (`smithy_query(query="shape",shape="MemberId")`).
fn member_id(user: &str) -> Value {
    json!({ "UserId": user })
}

/// The memberships `ListGroupMembershipsForMember` reports for `member`, in page order,
/// walking every page with `max_results`.
///
/// Asserts along the way what holds of every page: it carries at most `max_results`
/// items, nothing repeats, and every item is a membership of this store naming this
/// member, with a well-formed membership ARN.
async fn walk_member_memberships(
    router: &MultiServiceRouter,
    store: &str,
    member: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({ "IdentityStoreId": store, "MemberId": member_id(member) });
        if let Some(size) = max_results {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let page = ok(router, "ListGroupMembershipsForMember", request, case).await;
        let items = page["GroupMemberships"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a GroupMemberships list, got {page}"))
            .clone();
        if let Some(size) = max_results {
            assert!(
                items.len() as i64 <= size,
                "case {case:?}: expected at most {size} memberships in a page, got {}",
                items.len()
            );
        }
        for item in &items {
            let membership_id = string(item, "MembershipId", case);
            assert!(
                ids.insert(membership_id.clone()),
                "case {case:?}: expected every membership exactly once, saw {membership_id} twice"
            );
            assert_eq!(
                item["MemberId"],
                member_id(member),
                "case {case:?}: expected every reported membership to name member {member}, \
                 got {item}"
            );
            assert_eq!(
                item["IdentityStoreId"].as_str().unwrap_or_default(),
                store,
                "case {case:?}: expected the page's memberships to name store {store}, got {item}"
            );
            assert_resource_arn(
                item["MembershipArn"].as_str().unwrap_or_default(),
                "membership",
                &membership_id,
                case,
            );
            seen.push(item.clone());
        }
        pages += 1;
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
    }
    seen
}

/// The `MembershipId`s of `items`, in the order they were reported.
fn membership_ids(items: &[Value], case: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| string(item, "MembershipId", case))
        .collect()
}

/// Make `member` a member of `group` and return the new `MembershipId`.
async fn join(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    member: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": store,
            "GroupId": group,
            "MemberId": member_id(member),
        }),
        case,
    )
    .await;
    string(&created, "MembershipId", case)
}

#[tokio::test]
async fn a_members_listing_reports_its_memberships_in_creation_order() {
    let router = emulator();
    // Rows differ along how many groups the member belongs to and the page size asked
    // for — including an absent `MaxResults` (the modelled default) and both published
    // bounds of the modelled range.
    let rows: [(&str, usize, Option<i64>); 6] = [
        ("no memberships", 0, None),
        ("one membership, default page", 1, None),
        ("three memberships, one per page", 3, Some(MAX_RESULTS_MIN)),
        ("five memberships, two per page", 5, Some(2)),
        ("seven memberships, default page", 7, None),
        (
            "seven memberships, the largest page",
            7,
            Some(MAX_RESULTS_MAX),
        ),
    ];

    for (case, group_count, max_results) in rows {
        let member = create_user(&router, STORE, &user_name(case, 0), case).await;
        // A second member of every one of those groups, and a group the member under
        // test never joins: neither may show up in this member's listing.
        let bystander = create_user(&router, STORE, &user_name(case, 1), case).await;
        let mut expected: Vec<String> = Vec::new();
        let mut groups: Vec<String> = Vec::new();
        for index in 0..group_count {
            let group = create_group(&router, STORE, &format!("{case}-group-{index}"), case).await;
            expected.push(join(&router, STORE, &group, &member, case).await);
            join(&router, STORE, &group, &bystander, case).await;
            groups.push(group);
        }
        let unjoined = create_group(&router, STORE, &format!("{case}-group-unjoined"), case).await;
        join(&router, STORE, &unjoined, &bystander, case).await;

        let items = walk_member_memberships(&router, STORE, &member, max_results, case).await;
        assert_eq!(
            membership_ids(&items, case),
            expected,
            "case {case:?}: expected the member's {} membership(s) in creation order, got {items:?}",
            expected.len()
        );
        // Every reported membership names one of the groups the member joined, and the
        // group's own listing agrees it holds that membership: the two listings are two
        // views of one stored record.
        for (item, group) in items.iter().zip(&groups) {
            assert_eq!(
                item["GroupId"].as_str().unwrap_or_default(),
                group,
                "case {case:?}: expected the membership of {group}, got {item}"
            );
            let from_group = walk_memberships(&router, STORE, group, None, case).await;
            let membership_id = string(item, "MembershipId", case);
            assert!(
                from_group.contains(&membership_id),
                "case {case:?}: expected group {group} to list membership {membership_id}, \
                 got {from_group:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_member_id_is_also_accepted_as_the_users_arn() {
    let router = emulator();
    let case = "identifier forms";
    let member = create_user(&router, STORE, &user_name(case, 0), case).await;
    let group = create_group(&router, STORE, "engineering", case).await;
    let membership = join(&router, STORE, &group, &member, case).await;

    // Rows are the identifier forms the modelled `MemberId$UserId` (a `ResourceId`)
    // and `IdentityStoreId` patterns admit: the bare id, the resource ARN, and — for
    // the id, whose pattern accepts either letter case — the upper-cased id.
    let rows = [
        ("the bare ids", member.clone(), STORE.to_string()),
        (
            "the user ARN",
            format!("arn:aws:identitystore:::user/{member}"),
            STORE.to_string(),
        ),
        (
            "the identity store ARN",
            member.clone(),
            // The ARN form the modelled `IdentityStoreId` pattern admits, carrying the
            // emulator's canonical account id — the same form the lifecycle suite uses.
            format!("arn:aws:identitystore::000000000000:identitystore/{STORE}"),
        ),
        (
            "the upper-cased user id",
            member.to_ascii_uppercase(),
            STORE.to_string(),
        ),
    ];

    for (row_case, member_identifier, store_identifier) in rows {
        let page = ok(
            &router,
            "ListGroupMembershipsForMember",
            json!({
                "IdentityStoreId": store_identifier,
                "MemberId": member_id(&member_identifier),
            }),
            row_case,
        )
        .await;
        let items = page["GroupMemberships"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            membership_ids(&items, row_case),
            vec![membership.clone()],
            "case {row_case:?}: expected the one membership of {member}, got {page}"
        );
    }
}

#[tokio::test]
async fn deleting_a_membership_removes_it_from_the_members_listing() {
    let router = emulator();
    // Rows differ along WHICH of the member's memberships is deleted.
    let rows = [("the first", 0usize), ("a middle one", 1), ("the last", 2)];

    for (case, deleted) in rows {
        let member = create_user(&router, STORE, &user_name(case, 0), case).await;
        let mut memberships: Vec<String> = Vec::new();
        for index in 0..3 {
            let group = create_group(&router, STORE, &format!("{case}-group-{index}"), case).await;
            memberships.push(join(&router, STORE, &group, &member, case).await);
        }

        ok(
            &router,
            "DeleteGroupMembership",
            json!({ "IdentityStoreId": STORE, "MembershipId": memberships[deleted] }),
            case,
        )
        .await;

        let expected: Vec<String> = memberships
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != deleted)
            .map(|(_, id)| id.clone())
            .collect();
        let items = walk_member_memberships(&router, STORE, &member, None, case).await;
        assert_eq!(
            membership_ids(&items, case),
            expected,
            "case {case:?}: expected the surviving memberships in creation order, got {items:?}"
        );
    }
}

#[tokio::test]
async fn a_member_this_identity_store_does_not_hold_is_reported_as_not_found() {
    let router = emulator();
    let case = "unknown members";
    let member = create_user(&router, STORE, &user_name(case, 0), case).await;
    let group = create_group(&router, STORE, "engineering", case).await;
    join(&router, STORE, &group, &member, case).await;

    // Rows differ along WHY the member is not in the named store: no such id, and an
    // identifier naming no store this emulator serves — which is a different missing
    // resource.
    let rows = [
        (
            "an id no resource carries",
            STORE.to_string(),
            "a1b2c3d4-5678-90ab-cdef-example22222".to_string(),
            "USER",
        ),
        (
            "a store of another account",
            MISSING_STORE.to_string(),
            member.clone(),
            "IDENTITY_STORE",
        ),
        (
            "another account's other store",
            "arn:aws:identitystore::210987654321:identitystore/d-0000000000".to_string(),
            member.clone(),
            "IDENTITY_STORE",
        ),
    ];

    for (row_case, store, candidate, expected_type) in rows {
        let body = refused(
            &router,
            "ListGroupMembershipsForMember",
            json!({ "IdentityStoreId": store, "MemberId": member_id(&candidate) }),
            NOT_FOUND,
            "ResourceNotFoundException",
            row_case,
        )
        .await;
        assert_eq!(
            body.get("ResourceType").and_then(Value::as_str),
            Some(expected_type),
            "case {row_case:?}: expected the refusal to name the {expected_type} resource \
             type, got {body}"
        );
    }

    // The refusals above changed nothing: the member the store does hold still lists
    // its own membership.
    let items = walk_member_memberships(&router, STORE, &member, None, case).await;
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: expected the live member's one membership to survive, got {items:?}"
    );
}

#[tokio::test]
async fn a_page_request_outside_what_the_service_issued_or_models_is_refused() {
    let router = emulator();
    let case = "page requests";
    let member = create_user(&router, STORE, &user_name(case, 0), case).await;
    let other = create_user(&router, STORE, &user_name(case, 1), case).await;
    let mut groups: Vec<String> = Vec::new();
    for index in 0..2 {
        let group = create_group(&router, STORE, &format!("{case}-group-{index}"), case).await;
        join(&router, STORE, &group, &member, case).await;
        groups.push(group);
    }
    // A membership of a DIFFERENT member, whose cursor therefore names nothing in this
    // member's listing.
    let foreign = join(&router, STORE, &groups[0], &other, case).await;

    let first_page = ok(
        &router,
        "ListGroupMembershipsForMember",
        json!({
            "IdentityStoreId": STORE,
            "MemberId": member_id(&member),
            "MaxResults": MAX_RESULTS_MIN,
        }),
        case,
    )
    .await;
    let token = string(&first_page, "NextToken", case);

    // Rows differ along HOW the page request falls outside the contract: a token this
    // service never minted, a well-formed token naming a membership this member does
    // not hold, and a `MaxResults` on either side of the modelled 1..100 range.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a token this service did not mint",
            json!({ "NextToken": "not-a-cursor" }),
        ),
        (
            "a token for another member's membership",
            json!({ "NextToken": hex::encode(foreign.as_bytes()) }),
        ),
        (
            "a MaxResults below the modelled minimum",
            json!({ "MaxResults": MAX_RESULTS_MIN - 1 }),
        ),
        (
            "a MaxResults above the modelled maximum",
            json!({ "MaxResults": MAX_RESULTS_MAX + 1 }),
        ),
    ];

    for (row_case, extra) in rows {
        let mut request = json!({ "IdentityStoreId": STORE, "MemberId": member_id(&member) });
        for (key, value) in extra.as_object().expect("a table row is an object") {
            request[key.as_str()] = value.clone();
        }
        refused(
            &router,
            "ListGroupMembershipsForMember",
            request,
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }

    // The token this service DID issue walks the rest of the listing.
    let resumed = ok(
        &router,
        "ListGroupMembershipsForMember",
        json!({
            "IdentityStoreId": STORE,
            "MemberId": member_id(&member),
            "NextToken": token,
        }),
        case,
    )
    .await;
    let items = resumed["GroupMemberships"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: expected the second page to hold the remaining membership, got {resumed}"
    );
    assert!(
        resumed.get("NextToken").is_none_or(Value::is_null),
        "case {case:?}: expected no NextToken once the listing is exhausted, got {resumed}"
    );
}

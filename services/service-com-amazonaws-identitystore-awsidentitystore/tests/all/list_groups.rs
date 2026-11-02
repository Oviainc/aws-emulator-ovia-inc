// SPDX-License-Identifier: CC0-1.0

//! `ListGroups`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: AWSIdentityStore.ListGroups` and an `awsJson1_1` body), through the
//! harness the lifecycle suite already owns, so nothing reaches into the state struct:
//! a listing that is not computed from persisted state cannot pass.
//!
//! The suites are TABLES. Their rows differ along what the rules under test generalise
//! over — which optional members a group carries, how many groups the store holds, the
//! page size, the identity-store identifier form, the filter's attribute path — and the
//! recorded `group-without-description-field-omitted` /
//! `list-groups-pagination-next-token` cases are each one row.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The HTTP harness, shared with this crate's other suites rather than restated here.
use crate::harness::{call, emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{assert_resource_arn, assert_uuid};

/// A well-formed `IdentityStoreId` in the UUID form the modelled pattern also admits,
/// of a store of THIS account that nothing has written to: a second FORM of a store the
/// caller names, which this emulator serves.
const UUID_STORE: &str = "a1b2c3d4-5678-90ab-cdef-111122223333";

/// The account field of an ARN that names a store of ANOTHER account — not one this
/// emulator serves, whatever id it ends with.
const OTHER_ACCOUNT: &str = "210987654321";

/// The prefix of an identity store's ARN, the third identifier form the modelled
/// `IdentityStoreId` pattern admits
/// (`smithy_query(query="operation",operation="ListGroups")`).
const STORE_ARN_PREFIX: &str = "arn:aws:identitystore::";

/// The bounds of the modelled `MaxResults` range on `ListGroups` (range 1..100,
/// `@default: 100`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The members `CreateGroup` accepts besides `IdentityStoreId`
/// (`smithy_query(query="operation",operation="CreateGroup")`). A row that leaves one
/// out must be listed without it.
const GROUP_MEMBERS: &[&str] = &["DisplayName", "Description"];

/// The attribute path `ListGroups`'s `Filters` member searches
/// (`smithy_query(query="doc",shape="Filter",member="AttributePath")`: "`DisplayName`
/// is a valid attribute path for the `ListGroups` API").
const FILTER_PATH: &str = "DisplayName";

/// Create a group from `body` and return its `GroupId`.
async fn create_group(router: &MultiServiceRouter, body: &Value, case: &str) -> String {
    let created = ok(router, "CreateGroup", body.clone(), case).await;
    string(&created, "GroupId", case)
}

/// Create `count` groups in `store`, returning their `GroupId`s in creation order.
async fn create_groups(
    router: &MultiServiceRouter,
    store: &str,
    count: usize,
    case: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let body = json!({
            "IdentityStoreId": store,
            "DisplayName": format!("{}-{index}", case.replace(' ', "-")),
        });
        ids.push(create_group(router, &body, case).await);
    }
    ids
}

/// One `ListGroups` request body.
fn list_request(store: &str, max_results: Option<i64>, next_token: Option<&str>) -> Value {
    let mut request = json!({ "IdentityStoreId": store });
    if let Some(size) = max_results {
        request["MaxResults"] = json!(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    request
}

/// Walk every page of `ListGroups` for `store` with `max_results`, returning the
/// `Group` objects in the order they were reported.
///
/// Asserts the invariants that hold of any page of any size: a page never exceeds the
/// requested `MaxResults`, no group is reported twice, every group carries the identity
/// and ARN of a group of this store, and a `NextToken` is issued only when a further
/// page actually holds something.
async fn walk_groups(
    router: &MultiServiceRouter,
    store: &str,
    max_results: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let request = list_request(store, max_results, next.as_deref());
        let resumed = next.is_some();
        let page = ok(router, "ListGroups", request, case).await;
        let items = page["Groups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Groups list, got {page}"))
            .clone();
        if let Some(size) = max_results {
            assert!(
                items.len() as i64 <= size,
                "case {case:?}: expected at most {size} groups in a page, got {}",
                items.len()
            );
        }
        if resumed {
            assert!(
                !items.is_empty(),
                "case {case:?}: expected a NextToken to lead to a non-empty page, got {page}"
            );
        }
        for item in &items {
            let group_id = string(item, "GroupId", case);
            assert!(
                ids.insert(group_id.clone()),
                "case {case:?}: expected every group exactly once, saw {group_id} twice"
            );
            assert_uuid(&group_id, "GroupId", case);
            assert_resource_arn(
                item["GroupArn"].as_str().unwrap_or_default(),
                "group",
                &group_id,
                case,
            );
            assert_eq!(
                item["IdentityStoreId"].as_str().unwrap_or_default(),
                store.rsplit('/').next().unwrap_or(store),
                "case {case:?}: expected the listed groups to name store {store}, got {item}"
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

/// The `GroupId`s of `groups`, in the order they were listed.
fn group_ids(groups: &[Value]) -> Vec<String> {
    groups
        .iter()
        .map(|group| group["GroupId"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn every_stored_group_is_listed_exactly_as_it_was_stored() {
    let router = emulator();
    // Rows differ along WHICH optional members `CreateGroup` carried and the size and
    // form of their values. The modelled `GroupDisplayName` / `Description` lengths are
    // 1..1024 and their pattern admits letters, marks, symbols, numbers, punctuation
    // and whitespace.
    let rows: Vec<(&str, Value)> = vec![
        (
            "display name and description",
            json!({
                "IdentityStoreId": STORE,
                "DisplayName": "engineering",
                "Description": "The engineering group",
            }),
        ),
        (
            "display name only, so the listing omits Description",
            json!({ "IdentityStoreId": STORE, "DisplayName": "no-description-group" }),
        ),
        (
            "description only, so the listing omits DisplayName",
            json!({ "IdentityStoreId": STORE, "Description": "A group nobody named" }),
        ),
        (
            "neither optional member",
            json!({ "IdentityStoreId": STORE }),
        ),
        (
            "both members at their modelled 1024-character maximum",
            json!({
                "IdentityStoreId": STORE,
                "DisplayName": "d".repeat(1024),
                "Description": "e".repeat(1024),
            }),
        ),
        (
            "one-character values, the modelled minimum",
            json!({ "IdentityStoreId": STORE, "DisplayName": "x", "Description": "y" }),
        ),
        (
            "marks, symbols, punctuation and whitespace",
            json!({
                "IdentityStoreId": STORE,
                "DisplayName": "Zürich Ops — ①/② (prod)\tteam",
                "Description": "Ops & co. ✓ \"quoted\", <bracketed>\nsecond line",
            }),
        ),
    ];

    let mut expected: Vec<(String, Map<String, Value>, String)> = Vec::new();
    for (case, body) in &rows {
        let group_id = create_group(&router, body, case).await;
        expected.push((
            group_id,
            body.as_object().expect("an object body").clone(),
            (*case).to_string(),
        ));
    }

    let listed = walk_groups(&router, STORE, None, "every stored group").await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "case \"every stored group\": expected {} groups, got {}",
        rows.len(),
        listed.len()
    );

    for (group_id, sent, case) in &expected {
        let group = listed
            .iter()
            .find(|group| group["GroupId"].as_str() == Some(group_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected {group_id} in {listed:?}"));
        for member in GROUP_MEMBERS {
            let actual = group.get(*member).unwrap_or(&Value::Null);
            match sent.get(*member) {
                // A member the create carried comes back unchanged...
                Some(value) => assert_eq!(
                    actual, value,
                    "case {case:?}: expected {member} to be listed as {value}, got {actual}"
                ),
                // ...and one it did not carry is OMITTED, not invented or emptied.
                None => assert!(
                    actual.is_null(),
                    "case {case:?}: expected no {member} on a group created without one, \
                     got {actual}"
                ),
            }
        }
        assert_eq!(
            string(group, "Revision", case),
            "1",
            "case {case:?}: expected a created group's revision, got {group}"
        );
        for member in ["CreatedAt", "UpdatedAt"] {
            assert!(
                group.get(member).is_some_and(|value| value.is_number()),
                "case {case:?}: expected an epoch-seconds {member}, got {group}"
            );
        }
        // The same record, through the reader this crate already has: one group, not
        // two copies that can drift.
        let described = ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": group_id }),
            case,
        )
        .await;
        for member in GROUP_MEMBERS {
            assert_eq!(
                described.get(*member).unwrap_or(&Value::Null),
                group.get(*member).unwrap_or(&Value::Null),
                "case {case:?}: expected ListGroups and DescribeGroup to agree on {member}"
            );
        }
    }
}

#[tokio::test]
async fn groups_paginate_in_one_stable_order() {
    // Rows differ along how many groups the store holds and the page size the walk
    // asks for, including both published bounds of the modelled `MaxResults` range.
    let rows: &[(usize, Option<i64>)] = &[
        (0, None),
        (1, None),
        (1, Some(MAX_RESULTS_MIN)),
        (3, Some(MAX_RESULTS_MIN)),
        (5, Some(2)),
        (7, Some(3)),
        (9, Some(MAX_RESULTS_MAX)),
        (12, None),
    ];

    for (count, page_size) in rows {
        let case = format!("{count} groups in pages of {page_size:?}");
        let router = emulator();
        // A store with no groups still exists once something was created in it, so a
        // zero-group row is created through a user.
        let created = if *count == 0 {
            ok(
                &router,
                "CreateUser",
                json!({ "IdentityStoreId": STORE, "UserName": "only-user" }),
                &case,
            )
            .await;
            Vec::new()
        } else {
            create_groups(&router, STORE, *count, &case).await
        };

        let paged = group_ids(&walk_groups(&router, STORE, *page_size, &case).await);
        assert_eq!(
            paged.iter().collect::<BTreeSet<_>>(),
            created.iter().collect::<BTreeSet<_>>(),
            "case {case:?}: expected the walk to report exactly the created groups, \
             got {paged:?} for {created:?}"
        );
        // One order, whatever the page size: the sequence a caller pages through is the
        // sequence one unpaged call reports.
        let whole = group_ids(&walk_groups(&router, STORE, None, &case).await);
        assert_eq!(
            paged, whole,
            "case {case:?}: expected paging not to change the order, got {paged:?} vs {whole:?}"
        );
        // And repeating the walk reports the same order again: a read changes nothing.
        let again = group_ids(&walk_groups(&router, STORE, *page_size, &case).await);
        assert_eq!(
            paged, again,
            "case {case:?}: expected a second walk to report the same order, \
             got {again:?} vs {paged:?}"
        );
        // A `NextToken` only when a page is full and something is left.
        let last = ok(
            &router,
            "ListGroups",
            list_request(STORE, Some(MAX_RESULTS_MAX), None),
            &case,
        )
        .await;
        assert!(
            last.get("NextToken").is_none(),
            "case {case:?}: expected no NextToken when every group fits in one page, got {last}"
        );
    }
}

#[tokio::test]
async fn a_listing_covers_exactly_the_store_the_request_names() {
    let router = emulator();
    // The listing is the store's own groups, whichever FORM of its identifier named it:
    // the modelled `IdentityStoreId` admits the bare id and the store's ARN.
    let case = "per-store scope";
    let here = create_groups(&router, STORE, 3, "here").await;

    let listed = group_ids(&walk_groups(&router, STORE, None, case).await);
    assert_eq!(
        listed.iter().collect::<BTreeSet<_>>(),
        here.iter().collect::<BTreeSet<_>>(),
        "case {case:?}: expected store {STORE} to list its own groups, got {listed:?}"
    );

    // The ARN form of an identity store names that store, so it lists the same groups.
    let by_arn = group_ids(
        &walk_groups(
            &router,
            &format!(
                "{STORE_ARN_PREFIX}{}:identitystore/{STORE}",
                engine::emulator_account_id()
            ),
            None,
            case,
        )
        .await,
    );
    let by_id = group_ids(&walk_groups(&router, STORE, None, case).await);
    assert_eq!(
        by_arn, by_id,
        "case {case:?}: expected the store ARN to list the same groups as its id, \
         got {by_arn:?} vs {by_id:?}"
    );

    // And a store this emulator does not serve lists nothing at all — it is refused — so
    // one store's groups can never appear in another's listing.
    refused(
        &router,
        "ListGroups",
        list_request(MISSING_STORE, None, None),
        NOT_FOUND,
        "ResourceNotFoundException",
        case,
    )
    .await;

    // A store of this account that nothing has written to is served and lists no group,
    // which is also how one store's groups stay out of another's listing.
    let empty = ok(
        &router,
        "ListGroups",
        list_request("d-0c0d0e0f00", None, None),
        case,
    )
    .await;
    assert_eq!(
        empty["Groups"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected a store nothing has written to to list no groups, got {empty}"
    );
}

#[tokio::test]
async fn a_filter_selects_by_display_name() {
    let router = emulator();
    let case = "filter by DisplayName";
    let names = ["alpha", "beta", "gamma gamma", "Δelta"];
    let mut ids = Vec::new();
    for name in names {
        ids.push(
            create_group(
                &router,
                &json!({ "IdentityStoreId": STORE, "DisplayName": name, "Description": name }),
                case,
            )
            .await,
        );
    }

    // Rows: every created name selects exactly its own group, through each spelling of
    // the attribute path — SCIM attribute names are case-insensitive (RFC 7643 §2.1),
    // as `GetGroupId` already reads its `UniqueAttribute` path...
    for (name, group_id) in names.iter().zip(&ids) {
        for path in [FILTER_PATH, "displayName", "displayname", "DISPLAYNAME"] {
            let page = ok(
                &router,
                "ListGroups",
                json!({
                    "IdentityStoreId": STORE,
                    "Filters": [{ "AttributePath": path, "AttributeValue": name }],
                }),
                case,
            )
            .await;
            let listed =
                group_ids(page["Groups"].as_array().cloned().unwrap_or_default().as_slice());
            assert_eq!(
                listed,
                vec![group_id.clone()],
                "case {case:?}: expected the {path} filter on {name:?} to select {group_id}, \
                 got {listed:?}"
            );
            assert!(
                page.get("NextToken").is_none(),
                "case {case:?}: expected no NextToken on a single-group page, got {page}"
            );
        }
    }

    // ...a value no group carries selects nothing, and that is an empty list, not a
    // not-found.
    for missing in ["alph", "ALPHA", "gamma", "epsilon"] {
        let page = ok(
            &router,
            "ListGroups",
            json!({
                "IdentityStoreId": STORE,
                "Filters": [{ "AttributePath": FILTER_PATH, "AttributeValue": missing }],
            }),
            case,
        )
        .await;
        let listed = page["Groups"].as_array().cloned().unwrap_or_default();
        assert!(
            listed.is_empty(),
            "case {case:?}: expected the filter on {missing:?} to select nothing, got {listed:?}"
        );
    }

    // An empty `Filters` list (modelled `length 0..1`) filters nothing out.
    let unfiltered = ok(
        &router,
        "ListGroups",
        json!({ "IdentityStoreId": STORE, "Filters": [] }),
        case,
    )
    .await;
    let listed = group_ids(
        unfiltered["Groups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .as_slice(),
    );
    assert_eq!(
        listed.iter().collect::<BTreeSet<_>>(),
        ids.iter().collect::<BTreeSet<_>>(),
        "case {case:?}: expected an empty Filters list to select every group, got {listed:?}"
    );

    // An attribute path `ListGroups` does not search is a syntax error in the request.
    for path in ["Description", "UserName", "GroupId", "ExternalId.Id", "display.Name"] {
        refused(
            &router,
            "ListGroups",
            json!({
                "IdentityStoreId": STORE,
                "Filters": [{ "AttributePath": path, "AttributeValue": "alpha" }],
            }),
            400,
            "ValidationException",
            &format!("{case}: unsupported path {path}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let case = "cursor handling";
    create_groups(&router, STORE, 3, case).await;

    let first = ok(
        &router,
        "ListGroups",
        list_request(STORE, Some(MAX_RESULTS_MIN), None),
        case,
    )
    .await;
    let token = string(&first, "NextToken", case);
    // The cursor is opaque: it is not any id the response carries.
    assert!(
        !group_ids(first["Groups"].as_array().cloned().unwrap_or_default().as_slice())
            .contains(&token),
        "case {case:?}: expected an opaque NextToken, got the group id {token:?}"
    );

    // Rows: a token this service never minted, and ones that are well-formed ids no
    // group of this store carries, in either letter case.
    let stranger = "0123abcd-4567-89ab-cdef-0123456789ab";
    for bad in [
        "not-a-cursor".to_string(),
        hex::encode(b"00000000-0000-4000-8000-000000000000"),
        hex::encode(stranger.as_bytes()),
        hex::encode(stranger.to_ascii_uppercase().as_bytes()),
    ] {
        refused(
            &router,
            "ListGroups",
            list_request(STORE, Some(MAX_RESULTS_MIN), Some(&bad)),
            400,
            "ValidationException",
            &format!("{case}: token {bad}"),
        )
        .await;
    }

    // The token this service DID issue resumes the walk.
    let resumed = ok(
        &router,
        "ListGroups",
        list_request(STORE, Some(MAX_RESULTS_MIN), Some(&token)),
        case,
    )
    .await;
    assert_eq!(
        resumed["Groups"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: expected the resumed page to hold one group, got {resumed}"
    );
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let case = "MaxResults bounds";
    create_groups(&router, STORE, 2, case).await;

    // The published bounds are accepted...
    for size in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        ok(
            &router,
            "ListGroups",
            list_request(STORE, Some(size), None),
            &format!("{case}: {size}"),
        )
        .await;
    }
    // ...and one step past either end is refused.
    for size in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1, -1, 1_000] {
        refused(
            &router,
            "ListGroups",
            list_request(STORE, Some(size), None),
            400,
            "ValidationException",
            &format!("{case}: {size}"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_store_this_emulator_does_not_serve_is_not_found_and_an_unwritten_one_lists_nothing() {
    let router = emulator();
    // Rows are the identifier FORMS of a store this emulator does not serve: the ARN
    // form is the one that carries the OWNING ACCOUNT, so each of these names another
    // account's store — in the `d-` form and in the UUID form.
    let case = "another account's identity store";
    for store in [
        MISSING_STORE.to_string(),
        format!("{STORE_ARN_PREFIX}{OTHER_ACCOUNT}:identitystore/d-00ff00ff00"),
        format!("{STORE_ARN_PREFIX}{OTHER_ACCOUNT}:identitystore/{UUID_STORE}"),
    ] {
        let body = refused(
            &router,
            "ListGroups",
            list_request(&store, None, None),
            NOT_FOUND,
            "ResourceNotFoundException",
            &format!("{case}: {store}"),
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("IDENTITY_STORE"),
            "case {case:?}: expected the identity store to be the missing resource, got {body}"
        );
    }

    // A store of THIS account that nothing has written to is served and lists nothing, in
    // every form that names it.
    let case = "a store nothing has written to";
    for store in [
        "d-00ff00ff00".to_string(),
        UUID_STORE.to_string(),
        format!(
            "{STORE_ARN_PREFIX}{}:identitystore/d-00ff00ff00",
            engine::emulator_account_id()
        ),
    ] {
        let listed = ok(
            &router,
            "ListGroups",
            list_request(&store, None, None),
            &format!("{case}: {store}"),
        )
        .await;
        assert_eq!(
            listed["Groups"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected {store} to list no group, got {listed}"
        );
    }

    // The reads wrote nothing, and the store a group was created in still lists exactly
    // what was created in it — not what the other stores were read for.
    let group = create_group(
        &router,
        &json!({ "IdentityStoreId": STORE, "DisplayName": "first" }),
        case,
    )
    .await;
    let listed = group_ids(&walk_groups(&router, STORE, None, case).await);
    assert_eq!(
        listed,
        vec![group],
        "case {case:?}: expected the created group to be the whole listing, got {listed:?}"
    );
    let (status, code, body) = call(&router, "ListGroups", list_request(UUID_STORE, None, None)).await;
    assert_eq!(
        (status, code.as_str(), body["Groups"].as_array().map(Vec::len)),
        (200, "", Some(0)),
        "case {case:?}: expected a read to leave that store empty, got {status} {code} {body}"
    );
}

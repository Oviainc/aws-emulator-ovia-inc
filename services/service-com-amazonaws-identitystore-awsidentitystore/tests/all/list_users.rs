// SPDX-License-Identifier: CC0-1.0

//! `ListUsers`, driven end to end over HTTP.
//!
//! The rule under test: `ListUsers` reports the users PERSISTED in the identity store
//! the request names, in an order that does not depend on the page size and does not
//! change between calls, cut into pages of at most `MaxResults` with a `NextToken` that
//! resumes exactly where the previous page stopped and is absent on the last page; each
//! reported `User` carries what was stored for it and nothing else.
//!
//! Every suite is a TABLE whose rows differ along what that rule generalises over — how
//! many users the store holds, the page size, which profile members were stored, the
//! form of a `UserName`, which attribute path a filter names, how a `NextToken` is
//! wrong. The corpus's own two-user, two-page case is one row.
//!
//! The harness (router, `awsJson1_1` call, assertions on ids and ARNs) is the one
//! `identity_store_lifecycle` already owns, so there is a single front door in these
//! tests.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{
    assert_absent, assert_resource_arn, assert_round_tripped, assert_uuid, create_group,
    create_user, user_name, MAX_RESULTS_MAX, MAX_RESULTS_MIN,
};

/// The bounds of the modelled `UserName` length (1..128), from
/// `smithy_query(query="shape",shape="User",member="UserName")`.
const USER_NAME_MAX_LEN: usize = 128;

/// One page of `ListUsers`, as the parsed response body.
async fn list_page(
    router: &MultiServiceRouter,
    store: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
    filter: Option<(&str, &str)>,
    case: &str,
) -> Value {
    let mut request = json!({ "IdentityStoreId": store });
    if let Some(size) = max_results {
        request["MaxResults"] = json!(size);
    }
    if let Some(token) = next_token {
        request["NextToken"] = json!(token);
    }
    if let Some((path, value)) = filter {
        request["Filters"] = json!([{ "AttributePath": path, "AttributeValue": value }]);
    }
    ok(router, "ListUsers", request, case).await
}

/// The `User` objects `ListUsers` reports for `store`, in page order, walking every
/// page with `max_results`.
///
/// Asserts along the way that no page is larger than the page size asked for, that
/// every page but the last is FULL (a page short of `MaxResults` with more to come
/// would mean the cursor skipped work), that no user is reported twice, and that the
/// `NextToken` matches the modelled `^[-a-zA-Z0-9+=/:_]*$` pattern.
async fn walk_users(
    router: &MultiServiceRouter,
    store: &str,
    max_results: Option<i64>,
    filter: Option<(&str, &str)>,
    case: &str,
) -> Vec<Value> {
    let page_size = max_results.unwrap_or(MAX_RESULTS_MAX) as usize;
    let mut walked: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut next: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let page = list_page(router, store, max_results, next.as_deref(), filter, case).await;
        let items = page["Users"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Users list, got {page}"))
            .clone();
        pages += 1;
        assert!(
            items.len() <= page_size,
            "case {case:?}: expected at most {page_size} users on page {pages}, got {}",
            items.len()
        );
        let token = page.get("NextToken").and_then(Value::as_str);
        if token.is_some() {
            assert_eq!(
                items.len(),
                page_size,
                "case {case:?}: page {pages} carries a NextToken, so it must be full at \
                 {page_size} users, got {}",
                items.len()
            );
        }
        for item in &items {
            let user_id = string(item, "UserId", case);
            assert!(
                seen.insert(user_id.clone()),
                "case {case:?}: user {user_id} was reported twice while walking the pages"
            );
            walked.push(item.clone());
        }
        assert!(
            pages < 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match token {
            Some(token) => {
                assert!(
                    token
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-+=/:_".contains(&byte)),
                    "case {case:?}: expected a NextToken matching the modelled \
                     ^[-a-zA-Z0-9+=/:_]*$ pattern, got {token:?}"
                );
                next = Some(token.to_string());
            }
            None => break,
        }
    }
    walked
}

/// The `UserId`s of a walk, in page order.
fn ids(users: &[Value], case: &str) -> Vec<String> {
    users
        .iter()
        .map(|user| string(user, "UserId", case))
        .collect()
}

#[tokio::test]
async fn every_created_user_is_listed_once_in_an_order_the_page_size_does_not_change() {
    // Rows differ along how many users the store holds and the page size asked for,
    // including both published `MaxResults` bounds and an absent `MaxResults` (the
    // model's `@default: 100`). The corpus's two-users-two-pages case is the third row.
    let rows: [(&str, usize, Option<i64>); 7] = [
        ("an empty store", 0, None),
        ("one user, default page size", 1, None),
        ("two users, page of one", 2, Some(MAX_RESULTS_MIN)),
        ("three users, page of two", 3, Some(2)),
        ("five users, page of three", 5, Some(3)),
        ("four users, page of the upper bound", 4, Some(MAX_RESULTS_MAX)),
        ("seven users, default page size", 7, None),
    ];

    for (index, (case, count, max_results)) in rows.into_iter().enumerate() {
        // A fresh emulator per row, so rows cannot see each other's users: the identity
        // store is the one IAM Identity Center provisioned, and a caller cannot make a
        // second one.
        let router = emulator();
        let store = STORE.to_string();
        create_group(&router, &store, &format!("group-{index}"), case).await;
        let mut created: Vec<String> = Vec::new();
        for user in 0..count {
            created.push(create_user(&router, &store, &user_name(case, user), case).await);
        }

        let walked = walk_users(&router, &store, max_results, None, case).await;
        let walked_ids = ids(&walked, case);
        assert_eq!(
            walked_ids.len(),
            count,
            "case {case:?}: expected the {count} created users, got {walked_ids:?}"
        );
        assert_eq!(
            walked_ids.iter().cloned().collect::<BTreeSet<_>>(),
            created.iter().cloned().collect::<BTreeSet<_>>(),
            "case {case:?}: expected exactly the created users to be listed"
        );

        // The order is a property of the stored users, not of the page size: a walk one
        // user at a time reports the same sequence as a walk of the whole list.
        let one_at_a_time = ids(
            &walk_users(&router, &store, Some(MAX_RESULTS_MIN), None, case).await,
            case,
        );
        assert_eq!(
            one_at_a_time, walked_ids,
            "case {case:?}: the order must not depend on the page size"
        );
        // And it does not change between calls: a read reports, it does not reshuffle.
        let again = ids(&walk_users(&router, &store, max_results, None, case).await, case);
        assert_eq!(
            again, walked_ids,
            "case {case:?}: expected the same order on a second walk"
        );

        // Every listed user carries the identity the store minted for it.
        for user in &walked {
            let user_id = string(user, "UserId", case);
            assert_uuid(&user_id, "UserId", case);
            assert_resource_arn(&string(user, "UserArn", case), "user", &user_id, case);
            assert_eq!(
                string(user, "IdentityStoreId", case),
                store,
                "case {case:?}: expected the listed user to name its identity store"
            );
            assert_eq!(
                string(user, "Revision", case),
                "1",
                "case {case:?}: expected the revision CreateUser minted"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_user_is_reported_exactly_as_describe_user_reports_it() {
    let router = emulator();
    // Rows differ along WHICH profile members were stored, and the form and size of
    // their values. `Emails`, `Addresses`, `PhoneNumbers` and `Roles` are `length 1..1`
    // in the model and `Photos` is `1..3`
    // (`smithy_query(query="shape",shape="User")`).
    let rows: Vec<(&str, Value)> = vec![
        ("name only", json!({ "UserName": "alice" })),
        (
            "a structured name",
            json!({
                "UserName": "bob",
                "Name": {
                    "GivenName": "Bob",
                    "FamilyName": "Builder",
                    "Formatted": "Bob Builder",
                },
                "DisplayName": "Bob Builder",
            }),
        ),
        (
            "contact details",
            json!({
                "UserName": "carol",
                "Emails": [{ "Value": "carol@example.com", "Type": "work", "Primary": true }],
                "PhoneNumbers": [{ "Value": "+15555550100", "Type": "mobile" }],
                "Addresses": [{
                    "StreetAddress": "1 Example Way",
                    "Locality": "Seattle",
                    "Region": "WA",
                    "PostalCode": "98101",
                    "Country": "US",
                    "Type": "work",
                }],
            }),
        ),
        (
            "the whole profile",
            json!({
                "UserName": "dave",
                "DisplayName": "Dave",
                "NickName": "d",
                "ProfileUrl": "https://example.com/dave",
                "UserType": "Employee",
                "Title": "Engineer",
                "PreferredLanguage": "en-us",
                "Locale": "en-US",
                "Timezone": "America/Los_Angeles",
                "Website": "https://example.com/blog",
                "Birthdate": "1990-01-01",
                "Roles": [{ "Value": "admin", "Type": "primary", "Primary": true }],
                "Photos": [
                    { "Value": "https://example.com/a.png", "Type": "photo", "Primary": true },
                    { "Value": "https://example.com/b.png", "Type": "thumbnail" },
                ],
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "UserName": "u".repeat(USER_NAME_MAX_LEN) }),
        ),
    ];

    let mut expected: Vec<(String, String)> = Vec::new();
    for (case, profile) in &rows {
        let mut request = profile.clone();
        request["IdentityStoreId"] = json!(STORE);
        let created = ok(&router, "CreateUser", request, case).await;
        expected.push((case.to_string(), string(&created, "UserId", case)));
    }

    let listed = walk_users(&router, STORE, None, None, "every profile").await;
    for (case, user_id) in &expected {
        let item = listed
            .iter()
            .find(|user| string(user, "UserId", case) == *user_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected user {user_id} to be listed"));
        // The listing and the single-resource read are the SAME record: `DescribeUser`'s
        // response and the `User` object the list reports carry the same members, so a
        // listing that synthesised anything would differ here.
        let described = ok(
            &router,
            "DescribeUser",
            json!({ "IdentityStoreId": STORE, "UserId": user_id }),
            case,
        )
        .await;
        assert_eq!(
            item, &described,
            "case {case:?}: expected the listed user to match DescribeUser"
        );
        // And what was stored is what comes back: every member the create sent is on
        // the listed user, and no profile member it did not send appears. Containment
        // rather than equality, because a member the model gives a default (an
        // `Address`'s `Primary`, say) comes back on a response that did not send it.
        let sent = rows
            .iter()
            .find(|(row_case, _)| row_case == case)
            .map(|(_, profile)| profile.clone())
            .expect("the row is one of the table's");
        let sent = sent.as_object().expect("a row is an object").clone();
        assert_round_tripped(&sent, item, case);
        assert_absent(&sent, item, case);
        // Nothing stored an external id or an extension, so the listing reports
        // neither. `UserStatus`, `CreatedBy` and `UpdatedBy` are NOT in this list: every
        // stored user has a status (a created one is `ENABLED`) and records the principal
        // that created it — which is also the principal that last wrote it until
        // something updates it — so all three are reported; `assert_eq!` against
        // `DescribeUser` above already pins that the listing and the describe agree.
        for member in ["Extensions", "ExternalIds"] {
            assert!(
                item.get(member).is_none(),
                "case {case:?}: nothing stored {member}, so the listing must not report \
                 it, got {:?}",
                item.get(member)
            );
        }
        for member in ["UserStatus", "CreatedBy", "UpdatedBy"] {
            assert!(
                item.get(member).is_some_and(|value| value.is_string()),
                "case {case:?}: expected the listing to report {member}, got {:?}",
                item.get(member)
            );
        }
    }
}

#[tokio::test]
async fn a_filter_narrows_the_listing_to_one_user_name() {
    let router = emulator();
    // Rows differ along the FORM of the stored `UserName` a filter searches for.
    let rows = [
        ("ASCII", "alice".to_string()),
        ("accented", "zoë-münster".to_string()),
        ("punctuation and digits", "svc_user.01+test".to_string()),
        (
            "at the modelled length bound",
            "n".repeat(USER_NAME_MAX_LEN),
        ),
    ];

    let mut created: Vec<(&str, String, String)> = Vec::new();
    for (case, name) in &rows {
        let user_id = create_user(&router, STORE, name, case).await;
        created.push((case, name.clone(), user_id));
    }

    for (case, name, user_id) in &created {
        // The attribute path is matched case-insensitively, as SCIM attribute names are
        // (RFC 7643 §2.1) and as the sibling `ListGroups` / `GetGroupId` already read
        // theirs, so each spelling of the path names the same attribute.
        for path in ["UserName", "username", "USERNAME", "userName"] {
            let found = walk_users(&router, STORE, None, Some((path, name)), case).await;
            assert_eq!(
                ids(&found, case),
                vec![user_id.clone()],
                "case {case:?}: expected the filter on {path} to report only that user"
            );
        }
    }

    // A value no stored user carries matches nothing — an empty page, not an error and
    // not the whole list.
    let empty = list_page(
        &router,
        STORE,
        None,
        None,
        Some(("UserName", "nobody-by-this-name")),
        "no match",
    )
    .await;
    assert_eq!(
        empty["Users"].as_array().map(Vec::len),
        Some(0),
        "case \"no match\": expected an empty Users list, got {empty}"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case \"no match\": an empty listing has no next page, got {empty}"
    );

    // An empty `Filters` list (the modelled `length 0..1` lower bound) narrows nothing.
    let unfiltered = ok(
        &router,
        "ListUsers",
        json!({ "IdentityStoreId": STORE, "Filters": [] }),
        "no filter",
    )
    .await;
    assert_eq!(
        unfiltered["Users"].as_array().map(Vec::len),
        Some(rows.len()),
        "case \"no filter\": expected every user, got {unfiltered}"
    );
}

#[tokio::test]
async fn an_attribute_path_other_than_user_name_is_refused() {
    let router = emulator();
    let case = "unsearchable attribute paths";
    create_user(&router, STORE, "alice", case).await;
    // `UserName` is the attribute path `Filter$AttributePath` documents for `ListUsers`
    // (`smithy_query(query="shape",shape="Filter")`); every row below is a different
    // path the modelled `AttributePath` pattern admits but this operation cannot search,
    // including the one `ListGroups` searches and a nested one. Case alone does not make
    // a path unsupported (RFC 7643 §2.1), which the suite above pins.
    for (row_case, path) in [
        ("the ListGroups attribute", "DisplayName"),
        ("a nested attribute", "Name.GivenName"),
        ("a list member", "Emails.Value"),
        ("a different attribute entirely", "Title"),
    ] {
        refused(
            &router,
            "ListUsers",
            json!({
                "IdentityStoreId": STORE,
                "Filters": [{ "AttributePath": path, "AttributeValue": "alice" }],
            }),
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_store_this_emulator_does_not_serve_is_not_found_and_an_unwritten_one_lists_nothing() {
    let router = emulator();
    // Rows differ along the form of an identifier that names no store this emulator
    // serves: the ARN form carries the OWNING ACCOUNT, and each of these names a store of
    // another account, in both of the id forms the modelled pattern admits.
    for (row_case, store) in [
        ("another account's d- store", MISSING_STORE.to_string()),
        (
            "another account's other d- store",
            "arn:aws:identitystore::210987654321:identitystore/d-00ff00ff00".to_string(),
        ),
        (
            "another account's store in UUID form",
            "arn:aws-us-gov:identitystore::210987654321:identitystore/\
             11111111-2222-3333-4444-555555555555"
                .to_string(),
        ),
    ] {
        let body = refused(
            &router,
            "ListUsers",
            json!({ "IdentityStoreId": store }),
            NOT_FOUND,
            "ResourceNotFoundException",
            row_case,
        )
        .await;
        assert_eq!(
            body.get("ResourceType").and_then(Value::as_str),
            Some("IDENTITY_STORE"),
            "case {row_case:?}: expected the modelled IDENTITY_STORE resource type, got {body}"
        );
    }

    // A store of THIS account that nothing has written to is served: it lists the nothing
    // it holds rather than being refused, and listing it stores nothing — a create
    // addressed to it afterwards is the first user it holds.
    for (row_case, store) in [
        ("a d- id nothing has written to", "d-0a0b0c0d0e".to_string()),
        (
            "that id as an ARN",
            format!(
                "arn:aws:identitystore::{}:identitystore/d-0a0b0c0d0e",
                engine::emulator_account_id()
            ),
        ),
        (
            "an id in UUID form",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
    ] {
        for attempt in 0..2 {
            let case = format!("{row_case}, read {attempt}");
            let listed = ok(
                &router,
                "ListUsers",
                json!({ "IdentityStoreId": store.clone() }),
                &case,
            )
            .await;
            let users = listed["Users"].as_array().cloned().unwrap_or_default();
            assert!(
                users.is_empty(),
                "case {case:?}: expected a store nothing has written to to list no users, \
                 got {listed}"
            );
            assert!(
                listed.get("NextToken").is_none_or(Value::is_null),
                "case {case:?}: expected no NextToken on an empty listing, got {listed}"
            );
        }
    }

    // A listing is scoped to the store it names, and the store that holds a user lists
    // exactly its own.
    let here = create_user(&router, STORE, "alice", "per-store scope").await;
    let listed = ids(
        &walk_users(&router, STORE, None, None, "per-store scope").await,
        "per-store scope",
    );
    assert_eq!(
        listed,
        vec![here],
        "case \"per-store scope\": expected the provisioned store to list only its own user"
    );
    // And a user created in one store is in no other store's listing.
    let elsewhere = create_user(&router, "d-0a0b0c0d0e", "alice", "per-store scope").await;
    let other = ids(
        &walk_users(&router, "d-0a0b0c0d0e", None, None, "per-store scope").await,
        "per-store scope",
    );
    assert_eq!(
        other,
        vec![elsewhere],
        "case \"per-store scope\": expected the caller-named store to list only its own user"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    let case = "cursor handling";
    for index in 0..3 {
        create_user(&router, STORE, &user_name(case, index), case).await;
    }

    let first_page = list_page(&router, STORE, Some(MAX_RESULTS_MIN), None, None, case).await;
    let token = string(&first_page, "NextToken", case);

    // Rows differ along HOW the token is wrong. Each still satisfies the modelled
    // `NextToken` pattern, so the refusal is this handler's and not the framework's.
    for (row_case, bad_token) in [
        ("not a cursor this service mints", "not-a-cursor".to_string()),
        ("well-formed but naming nothing", "deadbeef".to_string()),
        (
            "a cursor naming a user this listing does not hold",
            hex::encode(b"0123abcd-4567-89ab-cdef-0123456789ab"),
        ),
        ("a truncated cursor", token[..token.len() - 2].to_string()),
    ] {
        refused(
            &router,
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "NextToken": bad_token }),
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }

    // The token this service DID issue walks the rest of the list.
    let resumed = list_page(&router, STORE, None, Some(&token), None, case).await;
    assert_eq!(
        resumed["Users"].as_array().map(Vec::len),
        Some(2),
        "case {case:?}: expected the issued cursor to resume at the second user, got {resumed}"
    );
    assert!(
        resumed.get("NextToken").is_none(),
        "case {case:?}: the last page carries no NextToken, got {resumed}"
    );
}

#[tokio::test]
async fn a_max_results_past_the_modelled_bound_is_refused() {
    let router = emulator();
    let case = "MaxResults past the bound";
    create_user(&router, STORE, "alice", case).await;
    // The published bounds of the modelled range (1..100) are accepted; one past either
    // end is refused.
    for size in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let page = list_page(&router, STORE, Some(size), None, None, case).await;
        assert_eq!(
            page["Users"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected MaxResults {size} to be accepted, got {page}"
        );
    }
    for (row_case, size) in [
        ("below the lower bound", MAX_RESULTS_MIN - 1),
        ("above the upper bound", MAX_RESULTS_MAX + 1),
    ] {
        refused(
            &router,
            "ListUsers",
            json!({ "IdentityStoreId": STORE, "MaxResults": size }),
            400,
            "ValidationException",
            row_case,
        )
        .await;
    }
}

#[tokio::test]
async fn only_the_extensions_the_request_asks_for_are_reported() {
    let router = emulator();
    let case = "requested extensions";
    // `aws:identitystore:enterprise` is the extension name the operation's own doc names
    // as the only supported one; the modelled `ExtensionName` pattern is
    // `^aws:identitystore:[a-z]{1,20}$`
    // (`smithy_query(query="shape",shape="ExtensionNames")`).
    let extension = "aws:identitystore:enterprise";
    let created = ok(
        &router,
        "CreateUser",
        json!({
            "IdentityStoreId": STORE,
            "UserName": "alice",
            "Extensions": { extension: { "CostCenter": "4100" } },
        }),
        case,
    )
    .await;
    let user_id = string(&created, "UserId", case);

    // Rows differ along which extension names the listing asks for.
    for (row_case, requested, expected) in [
        ("none asked for", None, None),
        (
            "the stored extension",
            Some(vec![extension]),
            Some(json!({ extension: { "CostCenter": "4100" } })),
        ),
        (
            "an extension the user does not carry",
            Some(vec!["aws:identitystore:other"]),
            None,
        ),
    ] {
        let mut request = json!({ "IdentityStoreId": STORE });
        if let Some(names) = &requested {
            request["Extensions"] = json!(names);
        }
        let page = ok(&router, "ListUsers", request, row_case).await;
        let user = page["Users"]
            .as_array()
            .and_then(|users| users.first())
            .unwrap_or_else(|| panic!("case {row_case:?}: expected the stored user, got {page}"));
        assert_eq!(
            string(user, "UserId", row_case),
            user_id,
            "case {row_case:?}: expected the stored user to be listed"
        );
        assert_eq!(
            user.get("Extensions"),
            expected.as_ref(),
            "case {row_case:?}: expected Extensions {expected:?}, got {:?}",
            user.get("Extensions")
        );
    }
}

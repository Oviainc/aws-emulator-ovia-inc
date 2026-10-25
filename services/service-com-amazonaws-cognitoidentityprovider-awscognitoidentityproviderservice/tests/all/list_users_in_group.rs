// SPDX-License-Identifier: CC0-1.0

//! `ListUsersInGroup` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListUsersInGroup` REPORTS, from stored state, the users
//! of the pool named by `UserPoolId` whose stored membership names the stored
//! group named by `GroupName` — each entry rendered from the STORED user, so it
//! is member-for-member what the pool's own reader of that user renders — in one
//! stable order, a page of at most `Limit` entries at a time, where the
//! `NextToken` of a non-final page resumes the listing strictly after the last
//! user that page served and the final page carries none. It writes nothing: a
//! repeated call answers the same, and so does every other reader of the same
//! state. An absent pool or group is a 404 `ResourceNotFoundException` and a
//! `NextToken` this listing never minted a 400 `InvalidParameterException`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// `QueryLimitType`'s modelled range is 0..60
/// (`smithy_query(query="operation",operation="ListUsersInGroup")`), so 60 is the
/// most one page can hold and what a request naming no usable `Limit` gets.
const MAX_LIMIT: usize = 60;

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `username` in `user_pool_id` and return the stored user as
/// `AdminCreateUser` rendered it.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Value {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    created["User"].clone()
}

/// Create the group `group_name` in `user_pool_id`.
async fn group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) {
    ok(
        router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
    )
    .await;
}

/// Add `username` to `group_name`.
async fn add(router: &MultiServiceRouter, user_pool_id: &str, username: &str, group_name: &str) {
    ok(
        router,
        "AdminAddUserToGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": group_name,
        }),
    )
    .await;
}

/// One `ListUsersInGroup` call's request body: the optional members a case
/// leaves unset are left OUT rather than sent as null.
fn list_request(
    user_pool_id: &str,
    group_name: &str,
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("GroupName".to_string(), json!(group_name));
    if let Some(limit) = limit {
        body.insert("Limit".to_string(), json!(limit));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// The `Users` entries of one response, requiring the member to be present.
fn users_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Users"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Users must be a list, got {payload}"))
        .clone()
}

/// The `Username`s of one response, in the order it served them.
fn names_of(payload: &Value, label: &str) -> Vec<String> {
    users_of(payload, label)
        .iter()
        .map(|entry| {
            entry["Username"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an entry must name a user, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `group_name`'s listing with `limit` and return the entries
/// in the order they were served, asserting on the way that no page holds more
/// than `limit` entries and that only a non-final page carries a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    group_name: &str,
    limit: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
    // A request naming no `Limit`, or naming `0` — a page that may hold no item
    // terminates no pagination loop — takes the modelled maximum.
    let page_size = match limit {
        Some(limit) if limit > 0 => (limit as usize).min(MAX_LIMIT),
        _ => MAX_LIMIT,
    };
    let mut served: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = ok(
            router,
            "ListUsersInGroup",
            &list_request(user_pool_id, group_name, limit, next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = users_of(&payload, label);
        assert!(
            page.len() <= page_size,
            "case {label:?}: page {pages} holds at most {page_size} entries, got {}",
            page.len()
        );
        served.extend(page);
        next_token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &next_token {
            None => break,
            Some(token) => assert!(
                !token.trim().is_empty() && !token.contains(char::is_whitespace),
                "case {label:?}: a PaginationKey matches ^[\\S]+$, got {token:?}"
            ),
        }
        assert!(
            pages <= 1 + MAX_LIMIT * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// One row of the membership table: a group and the users added to it. The rows
/// differ along the COUNT of members (none, one, several), the ORDER the members
/// were added in, and the FORM of the group name — every name satisfies
/// `GroupNameType` (length 1..128, pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="ListUsersInGroup")`.
struct MembersCase {
    case: &'static str,
    group_name: String,
    adds: Vec<&'static str>,
}

/// Every username any row names, so the pool holds them all.
const USERNAMES: [&str; 4] = ["alice", "bob", "carol", "zoe"];

fn members_cases() -> Vec<MembersCase> {
    vec![
        MembersCase {
            case: "a group with no member",
            group_name: "empty".to_string(),
            adds: vec![],
        },
        MembersCase {
            case: "a group with one member",
            group_name: "single".to_string(),
            adds: vec!["bob"],
        },
        MembersCase {
            case: "a group with three members, added out of username order",
            group_name: "several".to_string(),
            adds: vec!["zoe", "alice", "carol"],
        },
        MembersCase {
            case: "a non-ASCII group name at the modelled length maximum 128",
            group_name: format!("グループ①{}", "g".repeat(123)),
            adds: vec!["alice", "zoe"],
        },
        MembersCase {
            case: "the same member added twice: membership is a set",
            group_name: "repeated".to_string(),
            adds: vec!["carol", "carol"],
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_member_rendered_from_the_stored_user() {
    let router = router();
    let user_pool_id = pool(&router, "listing-pool").await;
    let mut stored: Vec<(String, Value)> = Vec::new();
    for username in USERNAMES {
        stored.push((
            username.to_string(),
            user(&router, &user_pool_id, username).await,
        ));
    }

    for case in members_cases() {
        let label = case.case;
        let group_name = &case.group_name;
        assert!(
            group_name.chars().count() <= 128,
            "case {label:?}: the group name must satisfy GroupNameType, got {} characters",
            group_name.chars().count()
        );
        group(&router, &user_pool_id, group_name).await;
        for username in &case.adds {
            add(&router, &user_pool_id, username, group_name).await;
        }

        let mut expected: Vec<String> = case.adds.iter().map(|name| name.to_string()).collect();
        expected.sort();
        expected.dedup();

        let payload = ok(
            &router,
            "ListUsersInGroup",
            &list_request(&user_pool_id, group_name, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored member exactly once: expected \
             {expected:?}, got {actual:?}"
        );
        assert!(
            payload.get("NextToken").is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the STORED user, member for member — the same thing the
        // pool's own reader of that one user renders.
        for entry in users_of(&payload, label) {
            let username = entry["Username"].as_str().expect("an entry names a user");
            let created = &stored
                .iter()
                .find(|(stored_name, _)| stored_name == username)
                .unwrap_or_else(|| panic!("case {label:?}: {username:?} was created above"))
                .1;
            assert_eq!(
                &entry, created,
                "case {label:?}: the entry for {username:?} must be the stored user, got {entry}"
            );
            let read = ok(
                &router,
                "AdminGetUser",
                &json!({ "UserPoolId": user_pool_id, "Username": username }),
            )
            .await;
            // `AdminGetUser` renders the same stored user under its own member
            // names: `UserType.Attributes` is `AdminGetUser`'s `UserAttributes`.
            for (entry_member, read_member) in [
                ("Username", "Username"),
                ("Attributes", "UserAttributes"),
                ("UserStatus", "UserStatus"),
                ("Enabled", "Enabled"),
                ("UserCreateDate", "UserCreateDate"),
                ("UserLastModifiedDate", "UserLastModifiedDate"),
            ] {
                assert_eq!(
                    entry[entry_member], read[read_member],
                    "case {label:?}: the entry's {entry_member} for {username:?} must be the \
                     {read_member} AdminGetUser renders, got {entry} and {read}"
                );
            }
        }

        // The listing is a READ: calling it again answers the same.
        let again = ok(
            &router,
            "ListUsersInGroup",
            &list_request(&user_pool_id, group_name, None, None),
        )
        .await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated listing answers the same, got {again}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_over_one_stable_order_under_every_limit() {
    let router = router();
    let user_pool_id = pool(&router, "paging-pool").await;
    group(&router, &user_pool_id, "paged").await;
    // Seven members, so the page sizes below split them evenly, unevenly and not
    // at all. They are added in an order that is neither ascending nor
    // descending, so the served order comes from stored state, not from arrival.
    let usernames = [
        "mallory", "alice", "zoe", "bob", "trent", "carol", "peggy",
    ];
    for username in usernames {
        user(&router, &user_pool_id, username).await;
        add(&router, &user_pool_id, username, "paged").await;
    }

    // Whatever order the implementation stores, paging must serve the SAME
    // sequence under every page size — the order an unpaged listing serves.
    let whole = ok(
        &router,
        "ListUsersInGroup",
        &list_request(&user_pool_id, "paged", None, None),
    )
    .await;
    let expected = names_of(&whole, "the whole listing in one page");
    let mut sorted = expected.clone();
    sorted.sort();
    let mut all: Vec<String> = usernames.iter().map(|name| name.to_string()).collect();
    all.sort();
    assert_eq!(
        sorted, all,
        "the listing serves every member exactly once: expected {all:?}, got {sorted:?}"
    );

    // The rows span the published bounds of `Limit` (0 and 60), the sizes that
    // divide and do not divide the seven members, the size that holds them all
    // exactly, and an absent `Limit`.
    for (label, limit, expected_pages) in [
        ("no Limit at all", None, 1),
        ("Limit 1, the smallest page that makes progress", Some(1), 7),
        ("Limit 2, which does not divide seven", Some(2), 4),
        ("Limit 7, exactly the member count", Some(7), 1),
        ("Limit 6, one short of the member count", Some(6), 2),
        ("Limit 60, the modelled maximum", Some(60), 1),
        ("Limit 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, "paged", limit, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["Username"].as_str().expect("a name").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every member exactly once in the \
             listing's order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_one_group_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;
    for user_pool_id in [&first_pool, &second_pool] {
        for group_name in ["admins", "readers"] {
            group(&router, user_pool_id, group_name).await;
        }
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
        }
    }
    // Only the first pool's `alice` is added, and only to `admins`.
    add(&router, &first_pool, "alice", "admins").await;

    for (label, user_pool_id, group_name, expected) in [
        (
            "the group the user was added to, in the pool it was added in",
            &first_pool,
            "admins",
            vec!["alice".to_string()],
        ),
        (
            "another group of the same pool",
            &first_pool,
            "readers",
            vec![],
        ),
        (
            "the same group name in another pool",
            &second_pool,
            "admins",
            vec![],
        ),
    ] {
        let payload = ok(
            &router,
            "ListUsersInGroup",
            &list_request(user_pool_id, group_name, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_removed_member_leaves_the_listing_and_a_deleted_group_is_gone() {
    let router = router();
    let user_pool_id = pool(&router, "roundtrip-pool").await;
    group(&router, &user_pool_id, "admins").await;
    for username in ["alice", "bob"] {
        user(&router, &user_pool_id, username).await;
        add(&router, &user_pool_id, username, "admins").await;
    }

    ok(
        &router,
        "AdminRemoveUserFromGroup",
        &json!({ "UserPoolId": user_pool_id, "Username": "alice", "GroupName": "admins" }),
    )
    .await;
    let payload = ok(
        &router,
        "ListUsersInGroup",
        &list_request(&user_pool_id, "admins", None, None),
    )
    .await;
    let actual = names_of(&payload, "after one member was removed");
    assert_eq!(
        actual,
        vec!["bob".to_string()],
        "case \"after one member was removed\": the removed user is gone from the listing, got \
         {actual:?}"
    );

    // A group the pool no longer holds has no listing to serve.
    ok(
        &router,
        "DeleteGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "ListUsersInGroup",
        &list_request(&user_pool_id, "admins", None, None),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (404, "ResourceNotFoundException"),
        "case \"a deleted group\": expected a 404 ResourceNotFoundException, got {status} with \
         {payload}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_group_and_an_unissued_token_are_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    for group_name in ["admins", "readers"] {
        group(&router, &user_pool_id, group_name).await;
    }
    for username in ["dave", "erin"] {
        user(&router, &user_pool_id, username).await;
        for group_name in ["admins", "readers"] {
            add(&router, &user_pool_id, username, group_name).await;
        }
    }

    // A token this listing DID mint, for `admins`: the row below echoes it back
    // for `readers`, whose listing it names no page of.
    let first_page = ok(
        &router,
        "ListUsersInGroup",
        &list_request(&user_pool_id, "admins", Some(1), None),
    )
    .await;
    let admins_token = first_page["NextToken"]
        .as_str()
        .expect("a non-final page carries a NextToken")
        .to_string();

    for (label, body, expected) in [
        (
            "an absent user pool",
            list_request("us-east-1_absent", "admins", None, None),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a group the pool does not hold",
            list_request(&user_pool_id, "absent-group", None, None),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a group name differing only in case, which GroupNameType matches exactly",
            list_request(&user_pool_id, "Admins", None, None),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a NextToken no listing of this service minted",
            list_request(&user_pool_id, "admins", None, Some("not-a-token")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken that is a username rather than a cursor",
            list_request(&user_pool_id, "admins", None, Some("dave")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken minted for another group's listing",
            list_request(&user_pool_id, "readers", None, Some(&admins_token)),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "ListUsersInGroup", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: both groups still hold both members.
    for group_name in ["admins", "readers"] {
        let payload = ok(
            &router,
            "ListUsersInGroup",
            &list_request(&user_pool_id, group_name, None, None),
        )
        .await;
        let actual = names_of(&payload, group_name);
        assert_eq!(
            actual,
            vec!["dave".to_string(), "erin".to_string()],
            "case {group_name:?}: a refused listing changes no stored membership, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    group(&router, &user_pool_id, "admins").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListUsersInGroup")`): `Limit`
    // range 0..60, `GroupName` length 1..128 and pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, `UserPoolId` pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`, and `NextToken` length 1..131072 with pattern
    // `^[\S]+$`.
    for (label, body) in [
        (
            "Limit 61, one past the range maximum 60",
            list_request(&user_pool_id, "admins", Some(61), None),
        ),
        (
            "Limit -1, one below the range minimum 0",
            list_request(&user_pool_id, "admins", Some(-1), None),
        ),
        (
            "an empty group name, below the length minimum 1",
            list_request(&user_pool_id, "", None, None),
        ),
        (
            "a group name of 129 characters, past the length maximum 128",
            list_request(&user_pool_id, &"g".repeat(129), None, None),
        ),
        (
            "a group name holding a space, which the pattern excludes",
            list_request(&user_pool_id, "two words", None, None),
        ),
        (
            "a user pool id the pattern excludes",
            list_request("no-underscore", "admins", None, None),
        ),
        (
            "an empty NextToken, below the length minimum 1",
            list_request(&user_pool_id, "admins", None, Some("")),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            list_request(&user_pool_id, "admins", None, Some("two words")),
        ),
        (
            "a request missing the required GroupName",
            json!({ "UserPoolId": user_pool_id }),
        ),
        ("a request missing the required UserPoolId", json!({})),
    ] {
        let (status, payload) = call(&router, "ListUsersInGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with \
             {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `AdminListGroupsForUser` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `AdminListGroupsForUser` REPORTS, from stored state, the
//! groups of the pool named by `UserPoolId` that the stored user named by
//! `Username` belongs to — each entry rendered from the STORED group, so it is
//! member-for-member what `GetGroup` renders — in ascending group-name order, a
//! page of at most `Limit` entries at a time, where the `NextToken` of a
//! non-final page resumes the listing strictly after the last group that page
//! served and the final page carries none. It writes nothing: a repeated call
//! answers the same, and so does every other reader of the same state. An
//! absent pool is a 404 `ResourceNotFoundException`, an absent user a 404
//! `UserNotFoundException`, and a `NextToken` this listing never minted a 400
//! `InvalidParameterException`.
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
/// (`smithy_query(query="operation",operation="AdminListGroupsForUser")`), so 60
/// is the most one page can hold and what a request naming no usable `Limit`
/// gets.
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

/// Create `username` in `user_pool_id` and return its generated `sub`.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string()
}

/// Create the group `group_name` in `user_pool_id`, with the optional members
/// the listing must then render, and return the stored group.
async fn group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) -> Value {
    let created = ok(
        router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": group_name,
            "Description": format!("the {group_name} group"),
            "Precedence": group_name.len() as i32,
        }),
    )
    .await;
    created["Group"].clone()
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

/// One `AdminListGroupsForUser` call's request body: the optional members a case
/// leaves unset are left OUT rather than sent as null.
fn list_request(
    user_pool_id: &str,
    username: &str,
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Username".to_string(), json!(username));
    if let Some(limit) = limit {
        body.insert("Limit".to_string(), json!(limit));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// The `Groups` entries of one response, requiring the member to be present.
fn groups_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Groups"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Groups must be a list, got {payload}"))
        .clone()
}

/// The `GroupName`s of one response, in the order it served them.
fn names_of(payload: &Value, label: &str) -> Vec<String> {
    groups_of(payload, label)
        .iter()
        .map(|entry| {
            entry["GroupName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an entry must name a group, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `username`'s listing with `limit` and return the entries
/// in the order they were served, asserting on the way that no page holds more
/// than `limit` entries and that only a non-final page carries a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
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
            "AdminListGroupsForUser",
            &list_request(user_pool_id, username, limit, next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = groups_of(&payload, label);
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
            Some(token) => {
                assert!(
                    served.len() < 1 + MAX_LIMIT * 4,
                    "case {label:?}: the listing must terminate, still paging at {token:?}"
                );
                assert!(
                    !token.trim().is_empty() && !token.contains(char::is_whitespace),
                    "case {label:?}: a PaginationKey matches ^[\\S]+$, got {token:?}"
                );
            }
        }
        assert!(
            pages <= 1 + MAX_LIMIT * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// One row of the membership table: the user and the groups it is added to.
/// The rows differ along the COUNT of memberships (none, one, several) and the
/// FORM of the group names — every name satisfies `GroupNameType` (length
/// 1..128, pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="AdminAddUserToGroup")`.
struct MembershipCase {
    case: &'static str,
    username: &'static str,
    adds: Vec<String>,
}

fn membership_cases() -> Vec<MembershipCase> {
    vec![
        MembershipCase {
            case: "a user in no group",
            username: "nobody",
            adds: vec![],
        },
        MembershipCase {
            case: "a user in one group",
            username: "single",
            adds: vec!["admins".to_string()],
        },
        MembershipCase {
            case: "a user in three groups, added out of name order",
            username: "several",
            adds: vec![
                "zebras".to_string(),
                "admins".to_string(),
                "ops.team-1_a".to_string(),
            ],
        },
        MembershipCase {
            case: "a non-ASCII group name and one at the modelled length maximum 128",
            username: "wide",
            adds: vec!["グループ①".to_string(), "g".repeat(128)],
        },
        MembershipCase {
            case: "the same group added twice: membership is a set",
            username: "repeated",
            adds: vec!["readers".to_string(), "readers".to_string()],
        },
    ]
}

/// Every group name any row names, so the pool holds them all.
fn all_group_names() -> Vec<String> {
    let mut names: Vec<String> = membership_cases()
        .iter()
        .flat_map(|case| case.adds.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[tokio::test]
async fn a_listing_reports_every_stored_membership_rendered_from_the_stored_group() {
    let router = router();
    let user_pool_id = pool(&router, "listing-pool").await;
    let mut stored: Vec<(String, Value)> = Vec::new();
    for name in all_group_names() {
        let group = group(&router, &user_pool_id, &name).await;
        stored.push((name, group));
    }

    for case in membership_cases() {
        let label = case.case;
        user(&router, &user_pool_id, case.username).await;
        for name in &case.adds {
            add(&router, &user_pool_id, case.username, name).await;
        }

        let mut expected: Vec<String> = case.adds.clone();
        expected.sort();
        expected.dedup();

        let payload = ok(
            &router,
            "AdminListGroupsForUser",
            &list_request(&user_pool_id, case.username, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored membership once, in name order: \
             expected {expected:?}, got {actual:?}"
        );
        assert!(
            payload.get("NextToken").is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the STORED group, member for member — the same thing
        // the implemented reader of one group renders.
        for entry in groups_of(&payload, label) {
            let name = entry["GroupName"].as_str().expect("an entry names a group");
            let created = &stored
                .iter()
                .find(|(stored_name, _)| stored_name == name)
                .unwrap_or_else(|| panic!("case {label:?}: {name:?} was created above"))
                .1;
            assert_eq!(
                &entry, created,
                "case {label:?}: the entry for {name:?} must be the stored group, got {entry}"
            );
            let read = ok(
                &router,
                "GetGroup",
                &json!({ "UserPoolId": user_pool_id, "GroupName": name }),
            )
            .await;
            assert_eq!(
                entry, read["Group"],
                "case {label:?}: the entry for {name:?} must match GetGroup, got {read}"
            );
        }

        // The listing is a READ: calling it again answers the same, and the
        // membership it reports is unchanged.
        let again = ok(
            &router,
            "AdminListGroupsForUser",
            &list_request(&user_pool_id, case.username, None, None),
        )
        .await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated listing answers the same, got {again}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_over_the_stored_order_under_every_limit() {
    let router = router();
    let user_pool_id = pool(&router, "paging-pool").await;
    // Seven groups, so the page sizes below split them evenly, unevenly and not
    // at all.
    let names = [
        "admins".to_string(),
        "b".to_string(),
        "ops.team-1_a".to_string(),
        "readers".to_string(),
        "zebras".to_string(),
        "グループ①".to_string(),
        "g".repeat(128),
    ];
    for name in &names {
        group(&router, &user_pool_id, name).await;
    }
    user(&router, &user_pool_id, "paged").await;
    for name in &names {
        add(&router, &user_pool_id, "paged", name).await;
    }
    let mut expected: Vec<String> = names.to_vec();
    expected.sort();

    // The rows span the published bounds of `Limit` (0 and 60), the sizes that
    // divide and do not divide the seven memberships, the size that holds them
    // all exactly, and an absent `Limit`.
    for (label, limit, expected_pages) in [
        ("no Limit at all", None, 1),
        ("Limit 1, the smallest page that makes progress", Some(1), 7),
        ("Limit 2, which does not divide seven", Some(2), 4),
        ("Limit 7, exactly the membership count", Some(7), 1),
        ("Limit 6, one short of the membership count", Some(6), 2),
        ("Limit 60, the modelled maximum", Some(60), 1),
        ("Limit 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, "paged", limit, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["GroupName"].as_str().expect("a name").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every group exactly once in name \
             order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_one_user_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;
    for user_pool_id in [&first_pool, &second_pool] {
        group(&router, user_pool_id, "admins").await;
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
        }
    }
    // Only the first pool's `alice` is added to the group.
    add(&router, &first_pool, "alice", "admins").await;

    for (label, user_pool_id, username, expected) in [
        (
            "the added user of the pool it was added in",
            &first_pool,
            "alice",
            vec!["admins".to_string()],
        ),
        ("another user of the same pool", &first_pool, "bob", vec![]),
        (
            "the same username in another pool",
            &second_pool,
            "alice",
            vec![],
        ),
    ] {
        let payload = ok(
            &router,
            "AdminListGroupsForUser",
            &list_request(user_pool_id, username, None, None),
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
async fn a_user_is_resolved_by_username_or_by_sub() {
    let router = router();
    let user_pool_id = pool(&router, "resolve-pool").await;
    group(&router, &user_pool_id, "admins").await;
    let sub = user(&router, &user_pool_id, "carol").await;
    add(&router, &user_pool_id, "carol", "admins").await;

    let by_username = ok(
        &router,
        "AdminListGroupsForUser",
        &list_request(&user_pool_id, "carol", None, None),
    )
    .await;
    let by_sub = ok(
        &router,
        "AdminListGroupsForUser",
        &list_request(&user_pool_id, &sub, None, None),
    )
    .await;
    assert_eq!(
        by_sub, by_username,
        "the username and the sub name ONE stored user: got {by_sub} and {by_username}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_user_and_an_unissued_token_are_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    for name in ["admins", "readers"] {
        group(&router, &user_pool_id, name).await;
    }
    for username in ["dave", "erin"] {
        user(&router, &user_pool_id, username).await;
    }
    for name in ["admins", "readers"] {
        add(&router, &user_pool_id, "dave", name).await;
        add(&router, &user_pool_id, "erin", name).await;
    }

    // A token this listing DID mint, for `dave`: the row below echoes it back
    // for `erin`, whose listing it names no page of.
    let first_page = ok(
        &router,
        "AdminListGroupsForUser",
        &list_request(&user_pool_id, "dave", Some(1), None),
    )
    .await;
    let daves_token = first_page["NextToken"]
        .as_str()
        .expect("a non-final page carries a NextToken")
        .to_string();

    for (label, body, expected) in [
        (
            "an absent user pool",
            list_request("us-east-1_absent", "dave", None, None),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a user the pool does not hold",
            list_request(&user_pool_id, "absent-user", None, None),
            (404, "UserNotFoundException"),
        ),
        (
            "a NextToken no listing of this service minted",
            list_request(&user_pool_id, "dave", None, Some("not-a-token")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken that is a group name rather than a cursor",
            list_request(&user_pool_id, "dave", None, Some("admins")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken minted for another user's listing",
            list_request(&user_pool_id, "erin", None, Some(&daves_token)),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "AdminListGroupsForUser", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: `dave` still belongs to both groups.
    let payload = ok(
        &router,
        "AdminListGroupsForUser",
        &list_request(&user_pool_id, "dave", None, None),
    )
    .await;
    let actual = names_of(&payload, "after the refusals");
    assert_eq!(
        actual,
        vec!["admins".to_string(), "readers".to_string()],
        "a refused listing changes no stored membership, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    user(&router, &user_pool_id, "dave").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="AdminListGroupsForUser")`):
    // `Limit` range 0..60, `Username` length 1..128 and pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, `UserPoolId` pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`, and `NextToken` length 1..131072 with pattern
    // `^[\S]+$`.
    for (label, body) in [
        (
            "Limit 61, one past the range maximum 60",
            list_request(&user_pool_id, "dave", Some(61), None),
        ),
        (
            "Limit -1, one below the range minimum 0",
            list_request(&user_pool_id, "dave", Some(-1), None),
        ),
        (
            "an empty username, below the length minimum 1",
            list_request(&user_pool_id, "", None, None),
        ),
        (
            "a username of 129 characters, past the length maximum 128",
            list_request(&user_pool_id, &"u".repeat(129), None, None),
        ),
        (
            "a username holding a space, which the pattern excludes",
            list_request(&user_pool_id, "two words", None, None),
        ),
        (
            "a user pool id the pattern excludes",
            list_request("no-underscore", "dave", None, None),
        ),
        (
            "an empty NextToken, below the length minimum 1",
            list_request(&user_pool_id, "dave", None, Some("")),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            list_request(&user_pool_id, "dave", None, Some("two words")),
        ),
        (
            "a request missing the required Username",
            json!({ "UserPoolId": user_pool_id }),
        ),
        ("a request missing the required UserPoolId", json!({})),
    ] {
        let (status, payload) = call(&router, "AdminListGroupsForUser", &body).await;
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

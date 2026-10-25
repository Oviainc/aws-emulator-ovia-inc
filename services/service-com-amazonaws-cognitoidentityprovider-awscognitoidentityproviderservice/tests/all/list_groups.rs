// SPDX-License-Identifier: CC0-1.0

//! `ListGroups` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListGroups` REPORTS, from stored state, every group of
//! the pool named by `UserPoolId` — each entry rendered from the STORED group, so
//! it is member-for-member what `GetGroup` renders — in ascending group-name
//! order, a page of at most `Limit` entries at a time, where the `NextToken` of a
//! non-final page resumes the listing strictly after the last group that page
//! served and the final page carries none. It writes nothing: a repeated call
//! answers the same, and so does every other reader of the same state. An absent
//! pool is a 404 `ResourceNotFoundException` and a `NextToken` this listing never
//! minted a 400 `InvalidParameterException`.
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
/// (`smithy_query(query="operation",operation="ListGroups")`), so 60 is the most
/// one page can hold and what a request naming no usable `Limit` gets.
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

/// Create the group `group_name` in `user_pool_id` and return the stored group.
async fn group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) -> Value {
    let created = ok(
        router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": group_name,
            "Description": format!("the {group_name} group"),
            "Precedence": group_name.chars().count() as i32,
        }),
    )
    .await;
    created["Group"].clone()
}

/// Create the group `group_name` in `user_pool_id` with no optional member set,
/// so the listing must render the absent members as absent.
async fn bare_group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) -> Value {
    let created = ok(
        router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
    )
    .await;
    created["Group"].clone()
}

/// One `ListGroups` request body: the optional members a case leaves unset are
/// left OUT rather than sent as null.
fn list_request(user_pool_id: &str, limit: Option<i32>, next_token: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
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

/// The `NextToken` of one response, absent on the final page.
fn token_of(payload: &Value) -> Option<String> {
    payload
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Walk every page of `user_pool_id`'s listing with `limit` and return the
/// entries in the order they were served, asserting on the way that no page
/// holds more than `limit` entries and that only a non-final page carries a
/// `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
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
            "ListGroups",
            &list_request(user_pool_id, limit, next_token.as_deref()),
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
        next_token = token_of(&payload);
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

/// One row of the stored-groups table. The rows differ along the COUNT of
/// groups (none, one, several) and the FORM of the group names — every name
/// satisfies `GroupNameType` (length 1..128, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="CreateGroup")`.
struct PoolCase {
    case: &'static str,
    pool_name: &'static str,
    created: Vec<String>,
}

fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            case: "a pool holding no group",
            pool_name: "empty-pool",
            created: vec![],
        },
        PoolCase {
            case: "a pool holding one group",
            pool_name: "single-pool",
            created: vec!["admins".to_string()],
        },
        PoolCase {
            case: "three groups created out of name order",
            pool_name: "unordered-pool",
            created: vec![
                "zebras".to_string(),
                "admins".to_string(),
                "ops.team-1_a".to_string(),
            ],
        },
        PoolCase {
            case: "a non-ASCII group name, a symbol name and one at the modelled length maximum 128",
            pool_name: "wide-pool",
            created: vec![
                "グループ①".to_string(),
                "+group".to_string(),
                "g".repeat(128),
            ],
        },
        PoolCase {
            case: "names whose ascending order is not their creation order",
            pool_name: "order-pool",
            created: vec![
                "b".to_string(),
                "B".to_string(),
                "a".to_string(),
                "a0".to_string(),
                "A".to_string(),
            ],
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_group_rendered_from_the_stored_group() {
    let router = router();
    for case in pool_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, case.pool_name).await;
        let mut stored: Vec<(String, Value)> = Vec::new();
        for name in &case.created {
            stored.push((name.clone(), group(&router, &user_pool_id, name).await));
        }

        let mut expected: Vec<String> = case.created.clone();
        expected.sort();

        let payload = ok(
            &router,
            "ListGroups",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored group once, in name order: \
             expected {expected:?}, got {actual:?}"
        );
        assert!(
            token_of(&payload).is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the STORED group, member for member — the same thing the
        // single-group reader renders.
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

        // The listing is a READ: calling it again answers the same.
        let again = ok(
            &router,
            "ListGroups",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated listing answers the same, got {again}"
        );
    }
}

#[tokio::test]
async fn an_entry_renders_the_optional_members_the_stored_group_has() {
    let router = router();
    let user_pool_id = pool(&router, "optional-pool").await;
    let bare = bare_group(&router, &user_pool_id, "bare").await;
    let full = group(&router, &user_pool_id, "full").await;

    let payload = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let entries = groups_of(&payload, "a bare group beside a described one");
    assert_eq!(
        entries,
        vec![bare.clone(), full.clone()],
        "case {:?}: expected {:?}, got {entries:?}",
        "a bare group beside a described one",
        vec![bare.clone(), full.clone()]
    );
    assert!(
        entries[0].get("Description").is_none() && entries[0].get("Precedence").is_none(),
        "case {:?}: a member the create left out stays absent, got {}",
        "a group created with no optional member",
        entries[0]
    );
    assert_eq!(
        entries[1].get("Precedence"),
        Some(&json!(4)),
        "case {:?}: a stored Precedence is reported, got {}",
        "a group created with a precedence",
        entries[1]
    );
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
    let mut expected: Vec<String> = names.to_vec();
    expected.sort();

    // The rows span the published bounds of `Limit` (0 and 60), the sizes that
    // divide and do not divide the seven groups, the size that holds them all
    // exactly, and an absent `Limit`.
    for (label, limit, expected_pages) in [
        ("no Limit at all", None, 1),
        ("Limit 1, the smallest page that makes progress", Some(1), 7),
        ("Limit 2, which does not divide seven", Some(2), 4),
        ("Limit 3, which divides neither", Some(3), 3),
        ("Limit 7, exactly the group count", Some(7), 1),
        ("Limit 6, one short of the group count", Some(6), 2),
        ("Limit 60, the modelled maximum", Some(60), 1),
        ("Limit 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, limit, label).await;
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
async fn a_page_resumes_strictly_after_the_last_group_it_served() {
    let router = router();
    let user_pool_id = pool(&router, "resume-pool").await;
    for name in ["c1", "c2", "c3", "c4"] {
        group(&router, &user_pool_id, name).await;
    }

    // One page at a time, each resuming at the previous page's NextToken: no
    // group is served twice and none is skipped.
    let first = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, Some(2), None),
    )
    .await;
    let label = "the first of two pages of two";
    assert_eq!(
        names_of(&first, label),
        vec!["c1".to_string(), "c2".to_string()],
        "case {label:?}: got {first}"
    );
    let token = token_of(&first).expect("a non-final page carries a NextToken");

    // A group created BETWEEN pages, whose name sorts before the cursor, is not
    // served by the page that resumes after it; one whose name sorts after it is.
    group(&router, &user_pool_id, "c0").await;
    group(&router, &user_pool_id, "c5").await;
    let second = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    let label = "a page resuming strictly after the cursor";
    assert_eq!(
        names_of(&second, label),
        vec!["c3".to_string(), "c4".to_string(), "c5".to_string()],
        "case {label:?}: the page serves only names after the cursor, got {second}"
    );
    assert!(
        token_of(&second).is_none(),
        "case {label:?}: the final page carries no NextToken, got {second}"
    );

    // The SAME token asked again answers the same page: a token is a cursor, not
    // a consumed resource.
    let repeated = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    assert_eq!(
        repeated, second,
        "a page token is stable across calls, got {repeated}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_groups_of_one_pool_only() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;
    group(&router, &first_pool, "admins").await;
    group(&router, &first_pool, "readers").await;
    group(&router, &second_pool, "admins").await;

    for (label, user_pool_id, expected) in [
        (
            "the pool holding two groups",
            &first_pool,
            vec!["admins".to_string(), "readers".to_string()],
        ),
        (
            "another pool holding a same-named group",
            &second_pool,
            vec!["admins".to_string()],
        ),
    ] {
        let payload = ok(&router, "ListGroups", &list_request(user_pool_id, None, None)).await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        for entry in groups_of(&payload, label) {
            assert_eq!(
                entry["UserPoolId"].as_str(),
                Some(user_pool_id.as_str()),
                "case {label:?}: an entry names the pool it is stored in, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_pool_and_an_unissued_token_are_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    let other_pool = pool(&router, "other-pool").await;
    for name in ["admins", "readers"] {
        group(&router, &user_pool_id, name).await;
        group(&router, &other_pool, name).await;
    }

    // A token this listing DID mint, for one pool: the row below echoes it back
    // for the other pool, whose listing it names no page of.
    let first_page = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, Some(1), None),
    )
    .await;
    let minted = first_page["NextToken"]
        .as_str()
        .expect("a non-final page carries a NextToken")
        .to_string();

    for (label, body, expected) in [
        (
            "an absent user pool",
            list_request("us-east-1_absent", None, None),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a NextToken no listing of this service minted",
            list_request(&user_pool_id, None, Some("not-a-token")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken that is a group name rather than a cursor",
            list_request(&user_pool_id, None, Some("admins")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken minted for another pool's listing",
            list_request(&other_pool, None, Some(&minted)),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "ListGroups", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: the pool still holds both groups.
    let payload = ok(
        &router,
        "ListGroups",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let actual = names_of(&payload, "after the refusals");
    assert_eq!(
        actual,
        vec!["admins".to_string(), "readers".to_string()],
        "a refused listing changes no stored group, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListGroups")`): `Limit` range
    // 0..60, `UserPoolId` length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`, and
    // `NextToken` length 1..131072 with pattern `^[\S]+$`.
    for (label, body) in [
        (
            "Limit 61, one past the range maximum 60",
            list_request(&user_pool_id, Some(61), None),
        ),
        (
            "Limit -1, one below the range minimum 0",
            list_request(&user_pool_id, Some(-1), None),
        ),
        (
            "an empty user pool id, below the length minimum 1",
            list_request("", None, None),
        ),
        (
            "a user pool id the pattern excludes",
            list_request("no-underscore", None, None),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            list_request(&format!("us-east-1_{}", "a".repeat(46)), None, None),
        ),
        (
            "an empty NextToken, below the length minimum 1",
            list_request(&user_pool_id, None, Some("")),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            list_request(&user_pool_id, None, Some("two words")),
        ),
        ("a request missing the required UserPoolId", json!({})),
    ] {
        let (status, payload) = call(&router, "ListGroups", &body).await;
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

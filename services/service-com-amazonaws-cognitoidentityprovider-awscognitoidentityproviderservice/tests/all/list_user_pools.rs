// SPDX-License-Identifier: CC0-1.0

//! `ListUserPools` over the wire, driven through the router exactly as an SDK
//! drives it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListUserPools` REPORTS, from stored state, every user
//! pool this account holds as a short description rendered from the STORED pool
//! — so every member it shares with `DescribeUserPool` carries the same value —
//! in one stable ascending order, a page of at most `MaxResults` entries at a
//! time, where the `NextToken` of a non-final page resumes the listing strictly
//! after the last pool that page served and the final page carries none. It
//! writes nothing: a repeated call answers the same, and so does every other
//! reader of the same state. A `NextToken` this listing never minted is a 400
//! `InvalidParameterException`.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// `PoolQueryLimitType`'s modelled range is 1..60
/// (`smithy_query(query="operation",operation="ListUserPools")`), so 60 is the
/// most one page can hold and 1 the least.
const MAX_RESULTS_MAXIMUM: i32 = 60;

/// The members `UserPoolDescriptionType` shares with `UserPoolType`
/// (`smithy_query(query="shape",shape="UserPoolDescriptionType")`): a listing
/// entry must carry each of them exactly as the reader of that one pool does.
const SHARED_MEMBERS: [&str; 5] = [
    "Id",
    "Name",
    "LambdaConfig",
    "CreationDate",
    "LastModifiedDate",
];

/// A `PreSignUp` trigger ARN, so a row can carry a `LambdaConfig` the listing
/// has to render.
const TRIGGER_PRE_SIGN_UP: &str = "arn:aws:lambda:us-east-1:000000000000:function:pre-sign-up";

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

/// Create the pool `request` describes and return its minted `UserPoolId`.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateUserPool", request).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint a pool id, got {created}"))
        .to_string()
}

/// One `ListUserPools` request body. `MaxResults` is a REQUIRED member, so every
/// request carries it; `NextToken` is left OUT rather than sent as null when a
/// case names none.
fn list_request(max_results: Value, next_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("MaxResults".to_string(), max_results);
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// The `UserPools` entries of one response, requiring the member to be present.
fn pools_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["UserPools"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: UserPools must be a list, got {payload}"))
        .clone()
}

/// The `Id`s of one response, in the order it served them.
fn ids_of(payload: &Value, label: &str) -> Vec<String> {
    pools_of(payload, label)
        .iter()
        .map(|entry| {
            entry["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: an entry must name a pool, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of the listing with `max_results`, returning the entries in
/// the order they were served and the page count, asserting on the way that no
/// page holds more than `max_results` entries and that only a non-final page
/// carries a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    max_results: i32,
    label: &str,
) -> (Vec<Value>, usize) {
    let mut served: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = ok(
            router,
            "ListUserPools",
            &list_request(json!(max_results), next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = pools_of(&payload, label);
        assert!(
            page.len() <= max_results as usize,
            "case {label:?}: page {pages} holds at most {max_results} entries, got {}",
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
            pages <= 1 + MAX_RESULTS_MAXIMUM as usize,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// The short description `DescribeUserPool` implies for `user_pool_id`: the
/// members a listing entry shares with the full pool, taken from the STORED pool
/// the reader of that one pool renders.
async fn expected_entry(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    let described = ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let pool = described["UserPool"]
        .as_object()
        .unwrap_or_else(|| panic!("DescribeUserPool must render a pool, got {described}"));
    let mut expected = Map::new();
    for member in SHARED_MEMBERS {
        if let Some(value) = pool.get(member) {
            expected.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(expected)
}

/// One row of the pool table. The rows differ along the FORM of the pool name
/// (every one satisfies `UserPoolNameType`: length 1..128, pattern
/// `^[\w\s+=,.@-]+$`, from
/// `smithy_query(query="operation",operation="CreateUserPool")`) and along the
/// configuration the created pool carries, so a listing entry is checked for a
/// pool with a `LambdaConfig` and for one without.
struct PoolCase {
    case: &'static str,
    request: Value,
}

fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            case: "a pool with nothing but the required name",
            request: json!({ "PoolName": "plain-pool" }),
        },
        PoolCase {
            case: "a pool name at the modelled length maximum 128",
            request: json!({ "PoolName": "n".repeat(128) }),
        },
        PoolCase {
            case: "a one-character pool name, the modelled length minimum",
            request: json!({ "PoolName": "p" }),
        },
        PoolCase {
            case: "a pool name holding the spaces and symbols the pattern allows",
            request: json!({ "PoolName": "pool one +=,.@-" }),
        },
        PoolCase {
            case: "a pool carrying a LambdaConfig the listing must render",
            request: json!({
                "PoolName": "lambda-pool",
                "LambdaConfig": { "PreSignUp": TRIGGER_PRE_SIGN_UP },
            }),
        },
        PoolCase {
            case: "a pool configured well past its defaults",
            request: json!({
                "PoolName": "configured-pool",
                "MfaConfiguration": "OPTIONAL",
                "DeletionProtection": "ACTIVE",
                "AutoVerifiedAttributes": ["email"],
                "UserPoolTags": { "owner": "listing" },
            }),
        },
        PoolCase {
            case: "a second pool of the same name as the first: a name is not an identity",
            request: json!({ "PoolName": "plain-pool" }),
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_pool_rendered_from_the_stored_pool() {
    let router = router();

    // An account that holds no pool: the result member is PRESENT and empty,
    // and the listing is final.
    let empty = ok(
        &router,
        "ListUserPools",
        &list_request(json!(MAX_RESULTS_MAXIMUM), None),
    )
    .await;
    assert_eq!(
        pools_of(&empty, "an account holding no pool"),
        Vec::<Value>::new(),
        "case {:?}: expected an empty list, got {empty}",
        "an account holding no pool"
    );
    assert!(
        empty.get("NextToken").is_none(),
        "case {:?}: a listing that served everything carries no NextToken, got {empty}",
        "an account holding no pool"
    );

    // Each created pool joins the listing, and every pool created before it
    // stays in it.
    let mut created: Vec<String> = Vec::new();
    for case in pool_cases() {
        let label = case.case;
        let user_pool_id = create(&router, &case.request).await;
        assert!(
            !created.contains(&user_pool_id),
            "case {label:?}: each pool gets its own minted id, got {user_pool_id:?} twice"
        );
        created.push(user_pool_id);

        let (served, _) = walk(&router, MAX_RESULTS_MAXIMUM, label).await;
        let mut expected_ids = created.clone();
        expected_ids.sort();
        let actual_ids: Vec<String> = served
            .iter()
            .map(|entry| entry["Id"].as_str().expect("an entry names a pool").to_string())
            .collect();
        assert_eq!(
            actual_ids, expected_ids,
            "case {label:?}: the listing reports every stored pool exactly once, in ascending id \
             order: expected {expected_ids:?}, got {actual_ids:?}"
        );

        // Every entry is the STORED pool's short description, member for
        // member — the same values the reader of that one pool renders, and
        // nothing else (the `@deprecated` `Status` and a `ReplicaRegions` no
        // stored replica backs are absent).
        for entry in &served {
            let id = entry["Id"].as_str().expect("an entry names a pool");
            let expected = expected_entry(&router, id).await;
            assert_eq!(
                entry, &expected,
                "case {label:?}: the entry for {id:?} must be the stored pool's short \
                 description, expected {expected} got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_over_the_stored_order_under_every_max_results() {
    let router = router();
    // Seven pools, so the page sizes below split them evenly, unevenly and not
    // at all.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        expected.push(create(&router, &json!({ "PoolName": format!("paged-{index}") })).await);
    }
    expected.sort();

    // The rows span the published bounds of `MaxResults` (1 and 60), the sizes
    // that divide and do not divide the seven pools, and the size that holds
    // them all exactly.
    for (label, max_results, expected_pages) in [
        ("MaxResults 1, the modelled minimum", 1, 7),
        ("MaxResults 2, which does not divide seven", 2, 4),
        ("MaxResults 6, one short of the pool count", 6, 2),
        ("MaxResults 7, exactly the pool count", 7, 1),
        ("MaxResults 8, one more than the pool count", 8, 1),
        ("MaxResults 60, the modelled maximum", MAX_RESULTS_MAXIMUM, 1),
    ] {
        let (served, pages) = walk(&router, max_results, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["Id"].as_str().expect("an entry names a pool").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every pool exactly once in ascending \
             id order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_listing_follows_the_stored_state_through_an_update_and_a_delete() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for index in 0..4 {
        created.push(create(&router, &json!({ "PoolName": format!("mutated-{index}") })).await);
    }

    // An update of one pool shows through the listing, because the entry is
    // rendered from the stored pool rather than from the request that created
    // it.
    let updated = created[1].clone();
    ok(
        &router,
        "UpdateUserPool",
        &json!({ "UserPoolId": updated, "DeletionProtection": "INACTIVE" }),
    )
    .await;
    let (served, _) = walk(&router, MAX_RESULTS_MAXIMUM, "after an update").await;
    let entry = served
        .iter()
        .find(|entry| entry["Id"] == json!(updated))
        .unwrap_or_else(|| panic!("case {:?}: the updated pool is still listed", "after an update"))
        .clone();
    let expected = expected_entry(&router, &updated).await;
    assert_eq!(
        entry, expected,
        "case {:?}: the entry must follow the stored pool, expected {expected} got {entry}",
        "after an update"
    );

    // A deleted pool leaves the listing; every other pool stays.
    let deleted = created.remove(0);
    ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": deleted }),
    )
    .await;
    let (served, _) = walk(&router, MAX_RESULTS_MAXIMUM, "after a delete").await;
    let actual = ids_of(
        &json!({ "UserPools": served }),
        "after a delete",
    );
    let mut expected_ids = created.clone();
    expected_ids.sort();
    assert_eq!(
        actual, expected_ids,
        "case {:?}: a deleted pool is no longer listed: expected {expected_ids:?}, got {actual:?}",
        "after a delete"
    );
}

#[tokio::test]
async fn a_listing_is_a_read_that_changes_nothing() {
    let router = router();
    for index in 0..3 {
        create(&router, &json!({ "PoolName": format!("read-{index}") })).await;
    }

    let first = ok(
        &router,
        "ListUserPools",
        &list_request(json!(MAX_RESULTS_MAXIMUM), None),
    )
    .await;
    let described_before: Vec<Value> = {
        let mut described = Vec::new();
        for id in ids_of(&first, "before the repeat") {
            described.push(ok(&router, "DescribeUserPool", &json!({ "UserPoolId": id })).await);
        }
        described
    };

    // Paging through the listing, and listing it again, answers the same.
    walk(&router, 1, "a full walk between two reads").await;
    let again = ok(
        &router,
        "ListUserPools",
        &list_request(json!(MAX_RESULTS_MAXIMUM), None),
    )
    .await;
    assert_eq!(
        again, first,
        "case {:?}: a repeated listing answers the same, got {again}",
        "a repeated listing"
    );

    // And so does every other reader of the same state.
    for (index, id) in ids_of(&first, "after the repeat").iter().enumerate() {
        let described = ok(&router, "DescribeUserPool", &json!({ "UserPoolId": id })).await;
        assert_eq!(
            described, described_before[index],
            "case {:?}: listing a pool changes nothing a later read of it sees, got {described}",
            "a read after the listing"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_listing_never_minted_is_refused() {
    let router = router();
    for index in 0..3 {
        create(&router, &json!({ "PoolName": format!("refusal-{index}") })).await;
    }
    // A token this listing DID mint, so the rows below can be compared against
    // one that works.
    let first_page = ok(&router, "ListUserPools", &list_request(json!(1), None)).await;
    let minted = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a non-final page carries a NextToken, got {first_page}"))
        .to_string();
    let listed_id = ids_of(&first_page, "the first page")
        .first()
        .expect("the first page serves one pool")
        .clone();

    // A token minted by ANOTHER listing of this service names no page of this
    // one.
    let pool_with_groups = create(&router, &json!({ "PoolName": "other-listing" })).await;
    for name in ["admins", "readers"] {
        ok(
            &router,
            "CreateGroup",
            &json!({ "UserPoolId": pool_with_groups, "GroupName": name }),
        )
        .await;
    }
    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": pool_with_groups, "Username": "alice" }),
    )
    .await;
    for name in ["admins", "readers"] {
        ok(
            &router,
            "AdminAddUserToGroup",
            &json!({ "UserPoolId": pool_with_groups, "Username": "alice", "GroupName": name }),
        )
        .await;
    }
    let other_listing = ok(
        &router,
        "AdminListGroupsForUser",
        &json!({ "UserPoolId": pool_with_groups, "Username": "alice", "Limit": 1 }),
    )
    .await;
    let other_token = other_listing["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the other listing's first page carries a token, got {other_listing}"))
        .to_string();

    for (label, body) in [
        (
            "a NextToken no listing of this service minted",
            list_request(json!(MAX_RESULTS_MAXIMUM), Some("not-a-token")),
        ),
        (
            "a NextToken that is a pool id rather than a cursor",
            list_request(json!(MAX_RESULTS_MAXIMUM), Some(&listed_id)),
        ),
        (
            "a NextToken minted for another listing of this service",
            list_request(json!(MAX_RESULTS_MAXIMUM), Some(&other_token)),
        ),
    ] {
        let (status, payload) = call(&router, "ListUserPools", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
    }

    // The token this listing minted still works, and the refusals persisted
    // nothing.
    let second_page = ok(
        &router,
        "ListUserPools",
        &list_request(json!(1), Some(&minted)),
    )
    .await;
    let ids = ids_of(&second_page, "the page after the minted token");
    assert_eq!(
        ids.len(),
        1,
        "case {:?}: the minted token serves the next page, got {second_page}",
        "the page after the minted token"
    );
    assert_ne!(
        ids[0], listed_id,
        "case {:?}: the next page resumes strictly after the last pool served, got {ids:?}",
        "the page after the minted token"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    create(&router, &json!({ "PoolName": "constraint-pool" })).await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListUserPools")`):
    // `MaxResults` is required with range 1..60, and `NextToken` has length 1..
    // with pattern `^[\S]+$`.
    for (label, body) in [
        (
            "MaxResults 61, one past the range maximum 60",
            list_request(json!(61), None),
        ),
        (
            "MaxResults 0, one below the range minimum 1",
            list_request(json!(0), None),
        ),
        (
            "MaxResults -1, below the range minimum 1",
            list_request(json!(-1), None),
        ),
        ("a request missing the required MaxResults", json!({})),
        (
            "an empty NextToken, below the length minimum 1",
            list_request(json!(MAX_RESULTS_MAXIMUM), Some("")),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            list_request(json!(MAX_RESULTS_MAXIMUM), Some("two words")),
        ),
    ] {
        let (status, payload) = call(&router, "ListUserPools", &body).await;
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

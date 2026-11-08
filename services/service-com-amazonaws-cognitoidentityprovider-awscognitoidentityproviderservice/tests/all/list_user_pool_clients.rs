// SPDX-License-Identifier: CC0-1.0

//! `ListUserPoolClients` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListUserPoolClients` REPORTS, from stored state, every
//! app client of the pool named by `UserPoolId` — each entry the short
//! description of the STORED client, so its `ClientId`, `UserPoolId` and
//! `ClientName` are what the single-client reader renders and nothing else of
//! the client's configuration is in it — in ascending `ClientId` order, a page
//! of at most `MaxResults` entries at a time, where the `NextToken` of a
//! non-final page resumes the listing strictly after the last client that page
//! served and the final page carries none. It writes nothing: a repeated call
//! answers the same, and so does every other reader of the same state. An
//! absent pool is a 400 `ResourceNotFoundException` and a `NextToken` this
//! listing never minted a 400 `InvalidParameterException`.
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

/// `QueryLimit`'s modelled range for this operation is 1..60
/// (`smithy_query(query="operation",operation="ListUserPoolClients")`), so 60
/// is the most one page can hold and what a request naming no `MaxResults` gets.
const MAX_RESULTS: usize = 60;

/// The members `UserPoolClientDescription` models, and the only ones a listing
/// entry may carry
/// (`smithy_query(query="shape",shape="UserPoolClientDescription")`).
const DESCRIPTION_MEMBERS: [&str; 3] = ["ClientId", "UserPoolId", "ClientName"];

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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client from `request` and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateUserPoolClient", request).await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create the app client `client_name` in `user_pool_id` and return its minted
/// `ClientId`.
async fn named_client(router: &MultiServiceRouter, user_pool_id: &str, client_name: &str) -> String {
    client(
        router,
        &json!({ "UserPoolId": user_pool_id, "ClientName": client_name }),
    )
    .await
}

/// One `ListUserPoolClients` request body: the optional members a case leaves
/// unset are left OUT rather than sent as null.
fn list_request(user_pool_id: &str, max_results: Option<i32>, next_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// The `UserPoolClients` entries of one response, requiring the member to be
/// present.
fn clients_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["UserPoolClients"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: UserPoolClients must be a list, got {payload}"))
        .clone()
}

/// The `ClientId`s of one response, in the order it served them.
fn ids_of(payload: &Value, label: &str) -> Vec<String> {
    clients_of(payload, label)
        .iter()
        .map(|entry| {
            entry["ClientId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: an entry must name a client, got {entry}")
                })
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

/// Walk every page of `user_pool_id`'s listing with `max_results` and return
/// the entries in the order they were served, asserting on the way that no page
/// holds more than the page size and that only a non-final page carries a
/// `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    max_results: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
    // A request naming no `MaxResults` takes the modelled maximum; the modelled
    // range starts at 1, so every value that reaches the handler is a real page
    // size.
    let page_size = match max_results {
        Some(max_results) if max_results > 0 => (max_results as usize).min(MAX_RESULTS),
        _ => MAX_RESULTS,
    };
    let mut served: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = ok(
            router,
            "ListUserPoolClients",
            &list_request(user_pool_id, max_results, next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = clients_of(&payload, label);
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
            pages <= 1 + MAX_RESULTS * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// One row of the stored-clients table: the app clients one pool holds when the
/// listing arrives, each named and configured by its own create request.
struct PoolCase {
    case: &'static str,
    pool_name: &'static str,
    /// (`ClientName`, the members beyond `ClientName` the create carries).
    created: Vec<(String, Vec<(&'static str, Value)>)>,
}

/// The table. The rows differ along what the rule generalises over: the COUNT of
/// app clients a pool holds (none, one, several), the FORM and length of the
/// client names across both ends of `ClientNameType`'s modelled length 1..128
/// with the punctuation its pattern `^[\w\s+=,.@-]+$` admits, and how much of
/// the client's optional configuration is set — including a confidential client,
/// whose secret a short description never carries. Members and constraints from
/// `smithy_query(query="operation",operation="CreateUserPoolClient,ListUserPoolClients")`.
fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            case: "a pool holding no app client",
            pool_name: "list-clients-empty-pool",
            created: Vec::new(),
        },
        PoolCase {
            case: "a pool holding one app client",
            pool_name: "list-clients-single-pool",
            created: vec![("web-app".to_string(), Vec::new())],
        },
        PoolCase {
            case: "three app clients, one of them confidential",
            pool_name: "list-clients-trio-pool",
            created: vec![
                ("public-app".to_string(), Vec::new()),
                (
                    "confidential-app".to_string(),
                    vec![("GenerateSecret", json!(true))],
                ),
                (
                    "configured-app".to_string(),
                    vec![
                        ("AccessTokenValidity", json!(5)),
                        (
                            "TokenValidityUnits",
                            json!({ "AccessToken": "minutes", "IdToken": "minutes" }),
                        ),
                        ("CallbackURLs", json!(["https://example.com/callback"])),
                        ("AllowedOAuthFlows", json!(["code"])),
                        ("AllowedOAuthScopes", json!(["openid"])),
                        ("AllowedOAuthFlowsUserPoolClient", json!(true)),
                        ("SupportedIdentityProviders", json!(["COGNITO"])),
                    ],
                ),
            ],
        },
        PoolCase {
            case: "client names at both ends of the modelled length 1..128 and one of \
                   punctuation the pattern admits",
            pool_name: "list-clients-wide-pool",
            created: vec![
                ("c".to_string(), Vec::new()),
                ("n".repeat(128), vec![("GenerateSecret", json!(true))]),
                ("a name +=,.@- 1".to_string(), Vec::new()),
            ],
        },
        PoolCase {
            case: "two app clients sharing one name, which the service does not make unique",
            pool_name: "list-clients-duplicate-name-pool",
            created: vec![
                ("same-name".to_string(), Vec::new()),
                ("same-name".to_string(), Vec::new()),
            ],
        },
    ]
}

impl PoolCase {
    /// The `CreateUserPoolClient` request for the row's `index`th client.
    fn create_request(&self, user_pool_id: &str, index: usize) -> Value {
        let (client_name, extra) = &self.created[index];
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientName".to_string(), json!(client_name));
        for (member, value) in extra {
            body.insert((*member).to_string(), value.clone());
        }
        Value::Object(body)
    }
}

#[tokio::test]
async fn a_listing_reports_every_stored_app_client_as_its_short_description() {
    let router = router();
    for case in pool_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, case.pool_name).await;

        // (the minted ClientId, the ClientName it was created with)
        let mut stored: Vec<(String, String)> = Vec::new();
        for index in 0..case.created.len() {
            let request = case.create_request(&user_pool_id, index);
            let client_id = client(&router, &request).await;
            stored.push((client_id, case.created[index].0.clone()));
        }

        let mut expected: Vec<String> = stored.iter().map(|(id, _)| id.clone()).collect();
        expected.sort();

        let payload = ok(
            &router,
            "ListUserPoolClients",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        let actual = ids_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored app client once, in ClientId \
             order: expected {expected:?}, got {actual:?}"
        );
        assert!(
            token_of(&payload).is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the short description of the STORED client: the three
        // modelled members, each one what the single-client reader renders, and
        // nothing else — a confidential client's secret included.
        for entry in clients_of(&payload, label) {
            let client_id = entry["ClientId"].as_str().expect("an entry names a client");
            let client_name = &stored
                .iter()
                .find(|(stored_id, _)| stored_id == client_id)
                .unwrap_or_else(|| panic!("case {label:?}: {client_id:?} was created above"))
                .1;
            let expected_entry = json!({
                "ClientId": client_id,
                "UserPoolId": user_pool_id,
                "ClientName": client_name,
            });
            assert_eq!(
                entry, expected_entry,
                "case {label:?}: the entry for {client_id:?} must be the stored client's short \
                 description {expected_entry}, got {entry}"
            );
            let members: Vec<String> = entry
                .as_object()
                .expect("an entry is a structure")
                .keys()
                .cloned()
                .collect();
            assert!(
                members.iter().all(|member| DESCRIPTION_MEMBERS
                    .contains(&member.as_str())),
                "case {label:?}: an entry carries only {DESCRIPTION_MEMBERS:?}, got {members:?}"
            );
            let read = ok(
                &router,
                "DescribeUserPoolClient",
                &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
            )
            .await;
            for member in DESCRIPTION_MEMBERS {
                assert_eq!(
                    entry[member], read["UserPoolClient"][member],
                    "case {label:?}: the entry's {member} for {client_id:?} must be what \
                     DescribeUserPoolClient renders, got {entry} against {read}"
                );
            }
        }

        // The listing is a READ: calling it again answers the same.
        let again = ok(
            &router,
            "ListUserPoolClients",
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
async fn a_listing_pages_over_the_stored_order_under_every_max_results() {
    let router = router();
    let user_pool_id = pool(&router, "list-clients-paging-pool").await;
    // Seven app clients, so the page sizes below split them evenly, unevenly and
    // not at all.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        expected.push(named_client(&router, &user_pool_id, &format!("app-{index}")).await);
    }
    expected.sort();

    // The rows span the published bounds of `MaxResults` (1 and 60), the sizes
    // that divide and do not divide the seven clients, the size that holds them
    // all exactly, and an absent `MaxResults`.
    for (label, max_results, expected_pages) in [
        ("no MaxResults at all", None, 1),
        (
            "MaxResults 1, the modelled range minimum",
            Some(1),
            7,
        ),
        ("MaxResults 2, which does not divide seven", Some(2), 4),
        ("MaxResults 3, which divides neither", Some(3), 3),
        ("MaxResults 6, one short of the client count", Some(6), 2),
        ("MaxResults 7, exactly the client count", Some(7), 1),
        ("MaxResults 60, the modelled range maximum", Some(60), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, max_results, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["ClientId"].as_str().expect("an id").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every app client exactly once in \
             ClientId order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_strictly_after_the_last_app_client_it_served() {
    let router = router();
    let user_pool_id = pool(&router, "list-clients-resume-pool").await;
    let mut first_four: Vec<String> = Vec::new();
    for index in 0..4 {
        first_four.push(named_client(&router, &user_pool_id, &format!("resume-{index}")).await);
    }
    first_four.sort();

    let label = "the first of two pages of two";
    let first = ok(
        &router,
        "ListUserPoolClients",
        &list_request(&user_pool_id, Some(2), None),
    )
    .await;
    assert_eq!(
        ids_of(&first, label),
        first_four[..2].to_vec(),
        "case {label:?}: the first page serves the two lowest ClientIds, got {first}"
    );
    let cursor = first_four[1].clone();
    let token = token_of(&first).expect("a non-final page carries a NextToken");

    // Two app clients created BETWEEN pages: the page that resumes after the
    // cursor serves exactly the stored ids that sort after it, so no client is
    // served twice and none is skipped.
    let mut all = first_four.clone();
    for index in 4..6 {
        all.push(named_client(&router, &user_pool_id, &format!("resume-{index}")).await);
    }
    all.sort();
    let expected: Vec<String> = all
        .iter()
        .filter(|id| *id > &cursor)
        .cloned()
        .collect();

    let label = "a page resuming strictly after the cursor";
    let second = ok(
        &router,
        "ListUserPoolClients",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    assert_eq!(
        ids_of(&second, label),
        expected,
        "case {label:?}: the page serves only the ids after the cursor {cursor:?}: expected \
         {expected:?}, got {second}"
    );
    assert!(
        token_of(&second).is_none(),
        "case {label:?}: the final page carries no NextToken, got {second}"
    );

    // The SAME token asked again answers the same page: a token is a cursor, not
    // a consumed resource.
    let repeated = ok(
        &router,
        "ListUserPoolClients",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    assert_eq!(
        repeated, second,
        "a page token is stable across calls, got {repeated}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_app_clients_of_one_pool_only() {
    let router = router();
    let first_pool = pool(&router, "list-clients-first-pool").await;
    let second_pool = pool(&router, "list-clients-second-pool").await;
    let mut first_ids = vec![
        named_client(&router, &first_pool, "shared-name").await,
        named_client(&router, &first_pool, "only-here").await,
    ];
    first_ids.sort();
    let second_ids = vec![named_client(&router, &second_pool, "shared-name").await];

    for (label, user_pool_id, expected) in [
        (
            "the pool holding two app clients",
            &first_pool,
            first_ids.clone(),
        ),
        (
            "another pool holding a same-named app client",
            &second_pool,
            second_ids.clone(),
        ),
    ] {
        let payload = ok(
            &router,
            "ListUserPoolClients",
            &list_request(user_pool_id, None, None),
        )
        .await;
        let actual = ids_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        for entry in clients_of(&payload, label) {
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
    let user_pool_id = pool(&router, "list-clients-refusal-pool").await;
    let other_pool = pool(&router, "list-clients-other-pool").await;
    let mut stored: Vec<String> = Vec::new();
    for name in ["first-app", "second-app"] {
        stored.push(named_client(&router, &user_pool_id, name).await);
        named_client(&router, &other_pool, name).await;
    }
    stored.sort();

    // A token this listing DID mint, for one pool: a row below echoes it back
    // for the other pool, whose listing it names no page of.
    let first_page = ok(
        &router,
        "ListUserPoolClients",
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
            (
                400,
                "ResourceNotFoundException",
                "User pool us-east-1_absent does not exist.".to_string(),
            ),
        ),
        (
            "a NextToken no listing of this service minted",
            list_request(&user_pool_id, None, Some("not-a-token")),
            (
                400,
                "InvalidParameterException",
                "Invalid pagination token given.".to_string(),
            ),
        ),
        (
            "a NextToken that is a stored ClientId rather than a cursor",
            list_request(&user_pool_id, None, Some(&stored[0])),
            (
                400,
                "InvalidParameterException",
                "Invalid pagination token given.".to_string(),
            ),
        ),
        (
            "a NextToken minted for another pool's listing",
            list_request(&other_pool, None, Some(&minted)),
            (
                400,
                "InvalidParameterException",
                "Invalid pagination token given.".to_string(),
            ),
        ),
    ] {
        let (status, payload) = call(&router, "ListUserPoolClients", &body).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload).to_string()),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: the pool still holds both app clients.
    let payload = ok(
        &router,
        "ListUserPoolClients",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let actual = ids_of(&payload, "after the refusals");
    assert_eq!(
        actual, stored,
        "a refused listing changes no stored app client, expected {stored:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "list-clients-constraint-pool").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListUserPoolClients")`):
    // `MaxResults` range 1..60, `UserPoolId` length 1..55 with pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`, and `NextToken` length 1..131072 with pattern
    // `^[\S]+$`. A `MaxResults` below the range MINIMUM is not here: Amazon
    // Cognito serves such a request with a page rather than refusing it, which
    // `tests/all/user_pool_request_preconditions.rs` covers.
    for (label, body) in [
        (
            "MaxResults 61, one past the range maximum 60",
            list_request(&user_pool_id, Some(61), None),
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
        let (status, payload) = call(&router, "ListUserPoolClients", &body).await;
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

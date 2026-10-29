// SPDX-License-Identifier: CC0-1.0

//! `ListResourceServers` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a paginated listing reports exactly the resource servers
//! STORED in the user pool it names, each rendered member for member as the
//! single-resource read renders it, in one stable order; a page serves at most
//! the page size the request asked for and carries a `NextToken` only while
//! items remain; a token belongs to the listing that issued it; and an absent
//! pool is a 400. Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// Create `identifier` in `user_pool_id` with one custom scope named after it,
/// and return the `ResourceServer` the create rendered. The friendly name is the
/// identifier with every character `NameType`'s pattern `^[\w\s+=,.@-]+$`
/// excludes replaced, so an identifier of any admitted form gets a valid name.
async fn resource_server(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    identifier: &str,
) -> Value {
    let name: String = identifier
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '+' | '=' | ',' | '.' | '@' | '-' => {
                character
            }
            _ => '-',
        })
        .collect();
    let created = ok(
        router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": identifier,
            "Name": format!("resource server {name}"),
            "Scopes": [{
                "ScopeName": "read",
                "ScopeDescription": format!("read {name}"),
            }],
        }),
    )
    .await;
    created["ResourceServer"].clone()
}

/// The `Identifier` of every entry of a listing's `ResourceServers`, in the order
/// the listing served them.
fn identifiers(payload: &Value) -> Vec<String> {
    payload["ResourceServers"]
        .as_array()
        .unwrap_or_else(|| panic!("ResourceServers must be a present list, got {payload}"))
        .iter()
        .map(|entry| {
            entry["Identifier"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries an Identifier, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Page through the whole listing of `user_pool_id` with `max_results` per
/// request, returning every identifier served, in order, and how many requests
/// it took. Loops are bounded so a listing that never terminates fails the test
/// instead of hanging.
async fn page_through(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    max_results: Option<i64>,
    label: &str,
) -> (Vec<String>, usize) {
    let mut served: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut requests = 0usize;
    loop {
        let mut body = json!({ "UserPoolId": user_pool_id });
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let payload = ok(router, "ListResourceServers", &body).await;
        requests += 1;
        let page = identifiers(&payload);
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case {label:?}: a page serves at most MaxResults items, got {} for a limit of {limit}",
                page.len()
            );
        }
        served.extend(page);
        next_token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return (served, requests);
        }
        assert!(
            requests < 64,
            "case {label:?}: the listing kept issuing a NextToken after {requests} requests, serving {served:?}"
        );
    }
}

/// One row of the listing table: how many resource servers the pool holds and
/// the page size each listing of it asks for. The rows differ along what the
/// rule generalises over — the stored count (none, one, several, more than one
/// page) and the requested page size (unset, the range minimum 1, an interior
/// value, the range maximum 50).
struct ListingCase {
    case: &'static str,
    stored: usize,
    max_results: Option<i64>,
    /// How many requests paging the whole listing must take.
    expected_requests: usize,
}

/// The table. `MaxResults`' modelled range is 1..50 and `NextToken` is the
/// listing's own token
/// (`smithy_query(query="operation",operation="ListResourceServers")`), so the
/// page sizes below span that range and the stored counts span "fits in one
/// page" and "needs several".
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            case: "a pool holding no resource server, no MaxResults",
            stored: 0,
            max_results: None,
            expected_requests: 1,
        },
        ListingCase {
            case: "a pool holding one resource server, no MaxResults",
            stored: 1,
            max_results: None,
            expected_requests: 1,
        },
        ListingCase {
            case: "one resource server at the MaxResults minimum 1",
            stored: 1,
            max_results: Some(1),
            expected_requests: 1,
        },
        ListingCase {
            case: "seven resource servers one page at a time, MaxResults 1",
            stored: 7,
            max_results: Some(1),
            expected_requests: 7,
        },
        ListingCase {
            case: "seven resource servers three at a time",
            stored: 7,
            max_results: Some(3),
            expected_requests: 3,
        },
        ListingCase {
            case: "seven resource servers at the MaxResults maximum 50",
            stored: 7,
            max_results: Some(50),
            expected_requests: 1,
        },
        ListingCase {
            case: "fifty-two resource servers, more than one page holds, no MaxResults",
            stored: 52,
            max_results: None,
            expected_requests: 2,
        },
        ListingCase {
            case: "fifty-two resource servers at the MaxResults maximum 50",
            stored: 52,
            max_results: Some(50),
            expected_requests: 2,
        },
    ]
}

#[tokio::test]
async fn a_listing_serves_every_stored_resource_server_exactly_once() {
    for case in listing_cases() {
        let label = case.case;
        let router = router();
        let user_pool_id = pool(&router, "listing-pool").await;

        // Identifiers created OUT of ascending order, so a listing that happens
        // to echo creation order is caught.
        let mut created: Vec<String> = (0..case.stored)
            .map(|index| format!("rs-{index:02}"))
            .collect();
        created.reverse();
        for identifier in &created {
            resource_server(&router, &user_pool_id, identifier).await;
        }
        let mut expected = created.clone();
        expected.sort();

        let (served, requests) = page_through(&router, &user_pool_id, case.max_results, label).await;
        assert_eq!(
            served, expected,
            "case {label:?}: the listing must serve every stored resource server exactly once, in one stable order, got {served:?}"
        );
        assert_eq!(
            requests, case.expected_requests,
            "case {label:?}: paging the whole listing must take {} request(s), took {requests}",
            case.expected_requests
        );
    }
}

#[tokio::test]
async fn a_listed_resource_server_is_rendered_as_the_single_resource_read_renders_it() {
    let router = router();
    let user_pool_id = pool(&router, "listing-render-pool").await;

    // Identifiers spanning the forms `ResourceServerIdentifierType` admits: an
    // API friendly name, an API URL and punctuation (length 1..256, pattern
    // `^[\x21\x23-\x5B\x5D-\x7E]+$`, from
    // `smithy_query(query="operation",operation="CreateResourceServer")`).
    let identifiers_created = [
        "solar-system-data",
        "https://solar-system-data-api.example.com",
        "tele!metry#1[2]~",
    ];
    for identifier in identifiers_created {
        resource_server(&router, &user_pool_id, identifier).await;
    }

    let listed = ok(
        &router,
        "ListResourceServers",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let entries = listed["ResourceServers"]
        .as_array()
        .unwrap_or_else(|| panic!("ResourceServers must be a present list, got {listed}"))
        .clone();

    for identifier in identifiers_created {
        let read = ok(
            &router,
            "DescribeResourceServer",
            &json!({ "UserPoolId": user_pool_id, "Identifier": identifier }),
        )
        .await;
        let entry = entries
            .iter()
            .find(|entry| entry["Identifier"] == json!(identifier))
            .unwrap_or_else(|| {
                panic!("case {identifier:?}: the listing must carry this resource server, got {listed}")
            });
        assert_eq!(
            entry, &read["ResourceServer"],
            "case {identifier:?}: the listing must render what the single-resource read renders, got {entry}"
        );
    }

    // A read changes nothing a later caller sees.
    let again = ok(
        &router,
        "ListResourceServers",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        again, listed,
        "a listing must not change what the next listing sees, got {again}"
    );
}

#[tokio::test]
async fn a_listing_reports_only_the_pool_it_names() {
    let router = router();
    let first_pool = pool(&router, "first-listing-pool").await;
    let second_pool = pool(&router, "second-listing-pool").await;
    resource_server(&router, &first_pool, "shared-identifier").await;
    resource_server(&router, &first_pool, "only-in-the-first-pool").await;
    resource_server(&router, &second_pool, "shared-identifier").await;

    for (label, user_pool_id, expected) in [
        (
            "the first pool",
            &first_pool,
            vec![
                "only-in-the-first-pool".to_string(),
                "shared-identifier".to_string(),
            ],
        ),
        (
            "the second pool",
            &second_pool,
            vec!["shared-identifier".to_string()],
        ),
    ] {
        let listed = ok(
            &router,
            "ListResourceServers",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            identifiers(&listed),
            expected,
            "case {label:?}: a listing reports only the resource servers of the pool it names, got {listed}"
        );
        for entry in listed["ResourceServers"]
            .as_array()
            .unwrap_or_else(|| panic!("ResourceServers must be a present list, got {listed}"))
        {
            assert_eq!(
                entry["UserPoolId"],
                json!(user_pool_id),
                "case {label:?}: every listed resource server names the pool that was listed, got {entry}"
            );
        }
    }

    // A token the FIRST pool's listing issued names no page of the second
    // pool's listing.
    let first_page = ok(
        &router,
        "ListResourceServers",
        &json!({ "UserPoolId": first_pool, "MaxResults": 1 }),
    )
    .await;
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of two must carry a NextToken, got {first_page}"))
        .to_string();
    let (status, payload) = call(
        &router,
        "ListResourceServers",
        &json!({ "UserPoolId": second_pool, "NextToken": token }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "a token from another pool's listing is a 400 InvalidParameterException, got {payload}"
    );
}

#[tokio::test]
async fn a_bad_request_is_refused_and_an_absent_pool_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "listing-refusal-pool").await;
    resource_server(&router, &user_pool_id, "solar-system-data").await;

    // An absent pool is a 400, whatever else the request carries.
    for (label, body) in [
        (
            "an absent pool",
            json!({ "UserPoolId": "us-east-1_absent" }),
        ),
        (
            "an absent pool with a page size",
            json!({ "UserPoolId": "us-east-1_absent", "MaxResults": 10 }),
        ),
    ] {
        let (status, payload) = call(&router, "ListResourceServers", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
    }

    // A `NextToken` this listing never issued names no page of it.
    for (label, token) in [
        ("a token that is not base64 at all", "not-a-token"),
        (
            "base64 of something this service never minted",
            "YWJjZGVmZ2g=",
        ),
    ] {
        let (status, payload) = call(
            &router,
            "ListResourceServers",
            &json!({ "UserPoolId": user_pool_id, "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: a token this listing never issued is a 400 InvalidParameterException, got {payload}"
        );
    }

    // One value past each published bound of this operation's input: the page
    // size's range 1..50 and the token's length minimum 1 and pattern `^[\S]+$`
    // (`smithy_query(query="operation",operation="ListResourceServers")`).
    for (label, body) in [
        (
            "MaxResults 0, below the range minimum 1",
            json!({ "UserPoolId": user_pool_id, "MaxResults": 0 }),
        ),
        (
            "MaxResults 51, past the range maximum 50",
            json!({ "UserPoolId": user_pool_id, "MaxResults": 51 }),
        ),
        (
            "an empty NextToken, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "NextToken": "" }),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "NextToken": "two words" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore" }),
        ),
    ] {
        let (status, payload) = call(&router, "ListResourceServers", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // None of the refused requests changed the listing.
    let listed = ok(
        &router,
        "ListResourceServers",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        identifiers(&listed),
        vec!["solar-system-data".to_string()],
        "a refused request changes nothing a later listing sees, got {listed}"
    );
}

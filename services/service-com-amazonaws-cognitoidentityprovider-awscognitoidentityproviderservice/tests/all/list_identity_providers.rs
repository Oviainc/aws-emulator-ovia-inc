// SPDX-License-Identifier: CC0-1.0

//! `ListIdentityProviders` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListIdentityProviders` REPORTS, from stored state, the
//! identity providers of the pool named by `UserPoolId` — each entry rendered
//! from the STORED provider, so its name, type and date members are what the
//! single-provider reader renders — in ascending provider-name order, a page of
//! at most `MaxResults` entries at a time and never more than the result list's
//! modelled maximum, where the `NextToken` of a non-final page resumes the
//! listing strictly after the last provider that page served and the final page
//! carries none. It writes nothing: a repeated call answers the same, and so
//! does every other reader of the same state. An absent pool is a 404
//! `ResourceNotFoundException` and a `NextToken` this listing never minted a 400
//! `InvalidParameterException`.
//!
//! Each table row names the case it asserts.

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

/// `ProvidersListType`, the list `Providers` is modelled as, has a maximum length
/// of 50 while `MaxResults`' range reaches 60
/// (`smithy_query(query="operation",operation="ListIdentityProviders")`), so one
/// page serves at most 50 providers however many the caller asks for.
const MAX_PROVIDERS_PER_PAGE: usize = 50;

/// The largest `MaxResults` this operation accepts, from the same query.
const MAX_RESULTS_MAXIMUM: i32 = 60;

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

/// Create the identity provider `provider_name` of `provider_type` in
/// `user_pool_id`, and return the stored provider the create rendered.
async fn provider(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    provider_name: &str,
    provider_type: &str,
) -> Value {
    let mut details = Map::new();
    match provider_type {
        "SAML" => {
            details.insert(
                "MetadataFile".to_string(),
                json!(format!("<EntityDescriptor entityID=\"{provider_name}\"/>")),
            );
        }
        "OIDC" => {
            details.insert(
                "oidc_issuer".to_string(),
                json!(format!("https://{provider_name}.example.com")),
            );
        }
        _ => {
            details.insert("client_id".to_string(), json!(provider_name));
            details.insert("client_secret".to_string(), json!("a-client-secret"));
            details.insert("authorize_scopes".to_string(), json!("openid"));
        }
    }
    let created = ok(
        router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": provider_name,
            "ProviderType": provider_type,
            "ProviderDetails": Value::Object(details),
        }),
    )
    .await;
    created["IdentityProvider"].clone()
}

/// One `ListIdentityProviders` request body: the optional members a case leaves
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

/// The `Providers` entries of one response, requiring the member to be present:
/// it is modelled `required`, so it is there even when the pool holds nothing.
fn providers_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Providers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Providers must be a list, got {payload}"))
        .clone()
}

/// The `ProviderName`s of one response, in the order it served them.
fn names_of(payload: &Value, label: &str) -> Vec<String> {
    providers_of(payload, label)
        .iter()
        .map(|entry| {
            entry["ProviderName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: an entry must name a provider, got {entry}")
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

/// Walk every page of `user_pool_id`'s listing with `max_results` and return the
/// entries in the order they were served, asserting on the way that no page holds
/// more than the page size the request asks for and that only a non-final page
/// carries a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    max_results: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
    // A request naming no `MaxResults`, or naming `0` — a page that may hold no
    // item terminates no pagination loop — takes the maximum, and no request can
    // make a page longer than the result list's modelled maximum.
    let page_size = match max_results {
        Some(requested) if requested > 0 => (requested as usize).min(MAX_PROVIDERS_PER_PAGE),
        _ => MAX_PROVIDERS_PER_PAGE,
    };
    let mut served: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let payload = ok(
            router,
            "ListIdentityProviders",
            &list_request(user_pool_id, max_results, next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = providers_of(&payload, label);
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
            pages <= 1 + MAX_PROVIDERS_PER_PAGE * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// One row of the stored-providers table: the pool to create and the providers to
/// store in it, each as (`ProviderName`, `ProviderType`). The rows differ along
/// the COUNT of providers (none, one, several), the provider TYPE across the
/// modelled catalog, and the FORM of the provider names.
struct PoolCase {
    case: &'static str,
    pool_name: &'static str,
    created: Vec<(String, &'static str)>,
}

/// The table. `ProviderType` spans the whole modelled catalog — SAML, Facebook,
/// Google, LoginWithAmazon, SignInWithApple, OIDC, from
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")` — and the names
/// span `ProviderNameType`'s length 1..32 with its pattern
/// `^[^_\p{Z}][\p{L}\p{M}\p{S}\p{N}\p{P}][^_\p{Z}]+$`, from
/// `smithy_query(query="operation",operation="CreateIdentityProvider")`.
fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            case: "a pool holding no identity provider",
            pool_name: "empty-idp-pool",
            created: vec![],
        },
        PoolCase {
            case: "a pool holding one identity provider",
            pool_name: "single-idp-pool",
            created: vec![("CorpSAML".to_string(), "SAML")],
        },
        PoolCase {
            case: "three providers created out of name order",
            pool_name: "unordered-idp-pool",
            created: vec![
                ("Zendesk-OIDC".to_string(), "OIDC"),
                ("Apple".to_string(), "SignInWithApple"),
                ("Google".to_string(), "Google"),
            ],
        },
        PoolCase {
            case: "the whole modelled provider-type catalog in one pool",
            pool_name: "catalog-idp-pool",
            created: vec![
                ("Facebook".to_string(), "Facebook"),
                ("Google".to_string(), "Google"),
                ("LoginWithAmazon".to_string(), "LoginWithAmazon"),
                ("AuthExampleOIDC".to_string(), "OIDC"),
                ("CorpSAML".to_string(), "SAML"),
                ("Apple".to_string(), "SignInWithApple"),
            ],
        },
        PoolCase {
            case: "names whose ascending order is not their creation order, one at the \
                   modelled length maximum 32",
            pool_name: "order-idp-pool",
            created: vec![
                ("bbb".to_string(), "OIDC"),
                ("BBB".to_string(), "SAML"),
                ("aaa".to_string(), "Google"),
                ("aaa0".to_string(), "Facebook"),
                ("p".repeat(32), "OIDC"),
            ],
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_provider_rendered_from_the_stored_provider() {
    let router = router();
    for case in pool_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, case.pool_name).await;
        let mut stored: Vec<(String, Value)> = Vec::new();
        for (provider_name, provider_type) in &case.created {
            stored.push((
                provider_name.clone(),
                provider(&router, &user_pool_id, provider_name, provider_type).await,
            ));
        }

        let mut expected: Vec<String> = case
            .created
            .iter()
            .map(|(provider_name, _)| provider_name.clone())
            .collect();
        expected.sort();

        let payload = ok(
            &router,
            "ListIdentityProviders",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored provider once, in name order: \
             expected {expected:?}, got {actual:?}"
        );
        assert!(
            token_of(&payload).is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the STORED provider: its name, type and date members are
        // what the single-provider reader renders.
        for entry in providers_of(&payload, label) {
            let provider_name = entry["ProviderName"]
                .as_str()
                .expect("an entry names a provider");
            let created = &stored
                .iter()
                .find(|(stored_name, _)| stored_name == provider_name)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: {provider_name:?} was created above")
                })
                .1;
            let read = ok(
                &router,
                "DescribeIdentityProvider",
                &json!({ "UserPoolId": user_pool_id, "ProviderName": provider_name }),
            )
            .await;
            let described = &read["IdentityProvider"];
            for member in [
                "ProviderName",
                "ProviderType",
                "CreationDate",
                "LastModifiedDate",
            ] {
                assert_eq!(
                    entry.get(member),
                    described.get(member),
                    "case {label:?}: the entry for {provider_name:?} must render {member} from \
                     the stored provider, got {entry} against {described}"
                );
                assert_eq!(
                    entry.get(member),
                    created.get(member),
                    "case {label:?}: the entry for {provider_name:?} must render {member} as the \
                     create stored it, got {entry} against {created}"
                );
            }
            // A `ProviderDescription` is name, type and dates only: the trust
            // details a single-provider read renders are not in a listing entry.
            for member in [
                "UserPoolId",
                "ProviderDetails",
                "AttributeMapping",
                "IdpIdentifiers",
            ] {
                assert!(
                    entry.get(member).is_none(),
                    "case {label:?}: a listing entry carries no {member}, got {entry}"
                );
            }
        }

        // The listing is a READ: calling it again answers the same, and so does
        // the single-provider reader.
        let again = ok(
            &router,
            "ListIdentityProviders",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        assert_eq!(
            again, payload,
            "case {label:?}: a repeated listing answers the same, got {again}"
        );
        for (provider_name, created) in &stored {
            let read = ok(
                &router,
                "DescribeIdentityProvider",
                &json!({ "UserPoolId": user_pool_id, "ProviderName": provider_name }),
            )
            .await;
            assert_eq!(
                &read["IdentityProvider"], created,
                "case {label:?}: the listing changed nothing a later reader of \
                 {provider_name:?} sees, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_over_the_stored_order_under_every_max_results() {
    let router = router();
    let user_pool_id = pool(&router, "idp-paging-pool").await;
    // Seven providers, so the page sizes below split them evenly, unevenly and
    // not at all.
    let created = [
        ("AuthExampleOIDC", "OIDC"),
        ("Apple", "SignInWithApple"),
        ("CorpSAML", "SAML"),
        ("Facebook", "Facebook"),
        ("Google", "Google"),
        ("LoginWithAmazon", "LoginWithAmazon"),
        ("Zendesk-OIDC", "OIDC"),
    ];
    for (provider_name, provider_type) in created {
        provider(&router, &user_pool_id, provider_name, provider_type).await;
    }
    let mut expected: Vec<String> = created
        .iter()
        .map(|(provider_name, _)| (*provider_name).to_string())
        .collect();
    expected.sort();

    // The rows span the published bounds of `MaxResults` (0 and 60), the sizes
    // that divide and do not divide the seven providers, the size that holds them
    // all exactly, and an absent `MaxResults`.
    for (label, max_results, expected_pages) in [
        ("no MaxResults at all", None, 1),
        (
            "MaxResults 1, the smallest page that makes progress",
            Some(1),
            7,
        ),
        ("MaxResults 2, which does not divide seven", Some(2), 4),
        ("MaxResults 3, which divides neither", Some(3), 3),
        ("MaxResults 7, exactly the provider count", Some(7), 1),
        ("MaxResults 6, one short of the provider count", Some(6), 2),
        (
            "MaxResults 60, the modelled maximum",
            Some(MAX_RESULTS_MAXIMUM),
            1,
        ),
        ("MaxResults 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, max_results, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| {
                entry["ProviderName"]
                    .as_str()
                    .expect("a name")
                    .to_string()
            })
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every provider exactly once in name \
             order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn no_page_is_longer_than_the_result_list_allows() {
    let router = router();
    let user_pool_id = pool(&router, "idp-page-bound-pool").await;
    // One provider more than a single page can hold, so a request asking for more
    // than the result list's maximum still gets a page inside it.
    let total = MAX_PROVIDERS_PER_PAGE + 1;
    for index in 0..total {
        provider(
            &router,
            &user_pool_id,
            &format!("Provider{index:03}"),
            "OIDC",
        )
        .await;
    }
    let mut expected: Vec<String> = (0..total).map(|index| format!("Provider{index:03}")).collect();
    expected.sort();

    for (label, max_results) in [
        ("MaxResults 50, exactly the result list maximum", Some(50)),
        (
            "MaxResults 51, one past the result list maximum",
            Some(51),
        ),
        (
            "MaxResults 60, the largest the operation accepts",
            Some(MAX_RESULTS_MAXIMUM),
        ),
        ("no MaxResults at all", None),
    ] {
        let payload = ok(
            &router,
            "ListIdentityProviders",
            &list_request(&user_pool_id, max_results, None),
        )
        .await;
        let page = names_of(&payload, label);
        assert_eq!(
            page.len(),
            MAX_PROVIDERS_PER_PAGE,
            "case {label:?}: a page holds at most {MAX_PROVIDERS_PER_PAGE} providers, got {}",
            page.len()
        );
        let token = token_of(&payload)
            .unwrap_or_else(|| panic!("case {label:?}: a non-final page carries a NextToken"));
        let rest = ok(
            &router,
            "ListIdentityProviders",
            &list_request(&user_pool_id, max_results, Some(&token)),
        )
        .await;
        let tail = names_of(&rest, label);
        let mut walked = page.clone();
        walked.extend(tail);
        assert_eq!(
            walked, expected,
            "case {label:?}: the two pages together serve every provider once in name order, \
             got {walked:?}"
        );
        assert!(
            token_of(&rest).is_none(),
            "case {label:?}: the final page carries no NextToken, got {rest}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_strictly_after_the_last_provider_it_served() {
    let router = router();
    let user_pool_id = pool(&router, "idp-resume-pool").await;
    for provider_name in ["Idp1", "Idp2", "Idp3", "Idp4"] {
        provider(&router, &user_pool_id, provider_name, "OIDC").await;
    }

    let first = ok(
        &router,
        "ListIdentityProviders",
        &list_request(&user_pool_id, Some(2), None),
    )
    .await;
    let label = "the first of two pages of two";
    assert_eq!(
        names_of(&first, label),
        vec!["Idp1".to_string(), "Idp2".to_string()],
        "case {label:?}: got {first}"
    );
    let token = token_of(&first).expect("a non-final page carries a NextToken");

    // A provider created BETWEEN pages, whose name sorts before the cursor, is
    // not served by the page that resumes after it; one whose name sorts after it
    // is.
    provider(&router, &user_pool_id, "Idp0", "SAML").await;
    provider(&router, &user_pool_id, "Idp5", "Google").await;
    let second = ok(
        &router,
        "ListIdentityProviders",
        &list_request(&user_pool_id, Some(MAX_RESULTS_MAXIMUM), Some(&token)),
    )
    .await;
    let label = "a page resuming strictly after the cursor";
    assert_eq!(
        names_of(&second, label),
        vec!["Idp3".to_string(), "Idp4".to_string(), "Idp5".to_string()],
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
        "ListIdentityProviders",
        &list_request(&user_pool_id, Some(MAX_RESULTS_MAXIMUM), Some(&token)),
    )
    .await;
    assert_eq!(
        repeated, second,
        "a page token is stable across calls, got {repeated}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_providers_of_one_pool_only() {
    let router = router();
    let first_pool = pool(&router, "first-idp-listing-pool").await;
    let second_pool = pool(&router, "second-idp-listing-pool").await;
    provider(&router, &first_pool, "CorpSAML", "SAML").await;
    provider(&router, &first_pool, "Google", "Google").await;
    provider(&router, &second_pool, "CorpSAML", "OIDC").await;

    for (label, user_pool_id, expected, shared_type) in [
        (
            "the pool holding two providers",
            &first_pool,
            vec!["CorpSAML".to_string(), "Google".to_string()],
            "SAML",
        ),
        (
            "another pool holding a same-named provider of another type",
            &second_pool,
            vec!["CorpSAML".to_string()],
            "OIDC",
        ),
    ] {
        let payload = ok(
            &router,
            "ListIdentityProviders",
            &list_request(user_pool_id, None, None),
        )
        .await;
        let actual = names_of(&payload, label);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        let entry = providers_of(&payload, label)
            .into_iter()
            .find(|entry| entry["ProviderName"] == json!("CorpSAML"))
            .unwrap_or_else(|| panic!("case {label:?}: the shared name is listed"));
        assert_eq!(
            entry["ProviderType"],
            json!(shared_type),
            "case {label:?}: each pool's provider keeps its own type, got {entry}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_and_an_unissued_token_are_refused() {
    let router = router();
    let user_pool_id = pool(&router, "idp-refusal-pool").await;
    let other_pool = pool(&router, "idp-other-pool").await;
    for provider_name in ["CorpSAML", "Google"] {
        provider(&router, &user_pool_id, provider_name, "OIDC").await;
        provider(&router, &other_pool, provider_name, "OIDC").await;
    }

    // A token this listing DID mint, for one pool: the row below echoes it back
    // for the other pool, whose listing it names no page of.
    let first_page = ok(
        &router,
        "ListIdentityProviders",
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
            "a NextToken that is a provider name rather than a cursor",
            list_request(&user_pool_id, None, Some("CorpSAML")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken minted for another pool's listing",
            list_request(&other_pool, None, Some(&minted)),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "ListIdentityProviders", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: the pool still holds both providers.
    let payload = ok(
        &router,
        "ListIdentityProviders",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let actual = names_of(&payload, "after the refusals");
    assert_eq!(
        actual,
        vec!["CorpSAML".to_string(), "Google".to_string()],
        "a refused listing changes no stored provider, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "idp-constraint-pool").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListIdentityProviders")`):
    // `MaxResults` range 0..60, `UserPoolId` length 1..55 and pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`, and `NextToken` length 1.. with pattern `^[\S]+$`.
    for (label, body) in [
        (
            "MaxResults 61, one past the range maximum 60",
            list_request(&user_pool_id, Some(MAX_RESULTS_MAXIMUM + 1), None),
        ),
        (
            "MaxResults -1, one below the range minimum 0",
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
        let (status, payload) = call(&router, "ListIdentityProviders", &body).await;
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

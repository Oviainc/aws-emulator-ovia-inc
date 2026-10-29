// SPDX-License-Identifier: CC0-1.0

//! `ListTerms` over the wire, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `ListTerms` REPORTS, from stored state, every set of
//! terms documents of the pool named by `UserPoolId` — every app client's
//! together — each entry rendered from the STORED record as the summary view
//! `TermsDescriptionType` declares, so every member it carries is what
//! `DescribeTerms` renders for the same `TermsId`. It serves them in one stable
//! order, a page of at most `MaxResults` entries at a time, where the
//! `NextToken` of a non-final page resumes the listing strictly after the last
//! entry that page served and the final page carries none. It writes nothing: a
//! repeated call answers the same. An absent pool is a 400
//! `ResourceNotFoundException` and a `NextToken` this listing never minted a 400
//! `InvalidParameterException`.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeSet;
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

/// `ListTermsRequestMaxResultsInteger`'s modelled range is 1..60
/// (`smithy_query(query="operation",operation="ListTerms")`), so 60 is the most
/// one page can hold and what a request naming no `MaxResults` gets.
const MAX_RESULTS: usize = 60;

/// The two document types `TermsNameType`'s pattern admits
/// (`^(terms-of-use|privacy-policy)$`, from
/// `smithy_query(query="shape",shape="TermsDescriptionType")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

/// Every member `TermsDescriptionType` declares, all of them required
/// (`smithy_query(query="shape",shape="TermsDescriptionType")`) — the summary
/// view a listing renders, which carries neither `UserPoolId`, `ClientId`,
/// `TermsSource` nor `Links`.
const SUMMARY_MEMBERS: [&str; 5] = [
    "TermsId",
    "TermsName",
    "Enforcement",
    "CreationDate",
    "LastModifiedDate",
];

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

/// Create an app client in `user_pool_id` and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create one set of terms documents named `terms_name` for `client_id` and
/// return its minted `TermsId`.
async fn terms(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    terms_name: &str,
) -> String {
    let created = ok(
        router,
        "CreateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": terms_name,
            // The only values `TermsSourceType` and `TermsEnforcementType`
            // admit (`smithy_query(query="shape",shape="TermsSourceType")`).
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": { "cognito:default": format!("https://example.com/{terms_name}") },
        }),
    )
    .await;
    created["Terms"]["TermsId"]
        .as_str()
        .expect("a minted TermsId")
        .to_string()
}

/// One `ListTerms` request body: the optional members a case leaves unset are
/// left OUT rather than sent as null.
fn list_request(user_pool_id: &str, max_results: Option<i32>, next_token: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// The `Terms` entries of one response, requiring the member to be present —
/// `ListTermsResponse.Terms` is REQUIRED, so it is a list even when empty.
fn entries_of(payload: &Value, label: &str) -> Vec<Value> {
    payload["Terms"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: Terms must be a list, got {payload}"))
        .clone()
}

/// The `TermsId`s of one response, in the order it served them.
fn ids_of(payload: &Value, label: &str) -> Vec<String> {
    entries_of(payload, label)
        .iter()
        .map(|entry| {
            entry["TermsId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {label:?}: an entry must name its documents, got {entry}")
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
/// entries in the order they were served plus the page count, asserting on the
/// way that no page holds more than the asked-for size and that only a non-final
/// page carries a `NextToken`.
async fn walk(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    max_results: Option<i32>,
    label: &str,
) -> (Vec<Value>, usize) {
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
            "ListTerms",
            &list_request(user_pool_id, max_results, next_token.as_deref()),
        )
        .await;
        pages += 1;
        let page = entries_of(&payload, label);
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
                "case {label:?}: a pagination token carries no whitespace, got {token:?}"
            ),
        }
        assert!(
            pages <= 1 + MAX_RESULTS * 4,
            "case {label:?}: the listing must terminate, served {pages} pages"
        );
    }
    (served, pages)
}

/// Store `count` sets of terms documents in `user_pool_id`, alternating the two
/// document types, and return the minted `TermsId`s. Each set gets its own app
/// client: a document's name is unique to its app client and only two names
/// exist, so a pool holding more than two sets spreads them over several app
/// clients — which is also what the listing must gather back together.
async fn stored_terms(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    count: usize,
    client_prefix: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let client_id = client(router, user_pool_id, &format!("{client_prefix}-{index}")).await;
        let terms_name = TERMS_NAMES[index % TERMS_NAMES.len()];
        ids.push(terms(router, user_pool_id, &client_id, terms_name).await);
    }
    ids
}

/// One row of the stored-documents table: how many sets of terms documents the
/// pool holds. The rows differ along the COUNT, which is what the listing rule
/// generalises over — none, one, both document types, and several spread over
/// several app clients.
struct PoolCase {
    case: &'static str,
    pool_name: &'static str,
    count: usize,
}

fn pool_cases() -> Vec<PoolCase> {
    vec![
        PoolCase {
            case: "a pool holding no terms documents",
            pool_name: "empty-terms-pool",
            count: 0,
        },
        PoolCase {
            case: "a pool holding one set of terms documents",
            pool_name: "single-terms-pool",
            count: 1,
        },
        PoolCase {
            case: "both document types, one on each of two app clients",
            pool_name: "both-terms-pool",
            count: 2,
        },
        PoolCase {
            case: "five sets spread over five app clients",
            pool_name: "many-terms-pool",
            count: 5,
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_every_stored_set_as_the_summary_view() {
    let router = router();
    for case in pool_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, case.pool_name).await;
        let created = stored_terms(&router, &user_pool_id, case.count, "report").await;

        let payload = ok(
            &router,
            "ListTerms",
            &list_request(&user_pool_id, None, None),
        )
        .await;
        let served = ids_of(&payload, label);
        let expected: BTreeSet<String> = created.iter().cloned().collect();
        let actual: BTreeSet<String> = served.iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: the listing reports every stored set exactly once: expected \
             {expected:?}, got {served:?}"
        );
        assert_eq!(
            served.len(),
            case.count,
            "case {label:?}: expected {} entries, got {served:?}",
            case.count
        );
        let mut ascending = served.clone();
        ascending.sort();
        assert_eq!(
            served, ascending,
            "case {label:?}: the listing serves one stable ascending order, got {served:?}"
        );
        assert!(
            token_of(&payload).is_none(),
            "case {label:?}: a listing that served everything carries no NextToken, got {payload}"
        );

        // Each entry is the summary view of the STORED record: exactly the
        // members `TermsDescriptionType` declares, each equal to what the
        // single-document reader renders.
        for entry in entries_of(&payload, label) {
            let terms_id = entry["TermsId"].as_str().expect("an entry names its id");
            let members: BTreeSet<String> = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {label:?}: an entry is a structure, got {entry}"))
                .keys()
                .cloned()
                .collect();
            let declared: BTreeSet<String> =
                SUMMARY_MEMBERS.iter().map(|m| (*m).to_string()).collect();
            assert_eq!(
                members, declared,
                "case {label:?}: an entry carries exactly the summary members: expected \
                 {declared:?}, got {members:?}"
            );
            let read = ok(
                &router,
                "DescribeTerms",
                &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
            )
            .await;
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    entry[member], read["Terms"][member],
                    "case {label:?}: the entry's {member} for {terms_id:?} must be the stored \
                     one, got {entry} against {read}"
                );
            }
        }

        // The listing is a READ: calling it again answers the same.
        let again = ok(
            &router,
            "ListTerms",
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
    let user_pool_id = pool(&router, "paging-terms-pool").await;
    // Seven sets of documents, so the page sizes below split them evenly,
    // unevenly and not at all.
    let created = stored_terms(&router, &user_pool_id, 7, "paging").await;
    let mut expected: Vec<String> = created.clone();
    expected.sort();

    // The rows span the published bounds of `MaxResults` (1 and 60), the sizes
    // that divide and do not divide the seven sets, the size that holds them all
    // exactly, and an absent `MaxResults`.
    for (label, max_results, expected_pages) in [
        ("no MaxResults at all", None, 1),
        (
            "MaxResults 1, the modelled range minimum",
            Some(1),
            created.len(),
        ),
        ("MaxResults 2, which does not divide seven", Some(2), 4),
        ("MaxResults 3, which divides neither", Some(3), 3),
        ("MaxResults 7, exactly the stored count", Some(7), 1),
        ("MaxResults 6, one short of the stored count", Some(6), 2),
        ("MaxResults 60, the modelled range maximum", Some(60), 1),
    ] {
        let (served, pages) = walk(&router, &user_pool_id, max_results, label).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| entry["TermsId"].as_str().expect("an id").to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: paging the whole listing serves every stored set exactly once in one \
             order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {label:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

#[tokio::test]
async fn a_page_resumes_strictly_after_the_last_entry_it_served() {
    let router = router();
    let user_pool_id = pool(&router, "resume-terms-pool").await;
    let created = stored_terms(&router, &user_pool_id, 4, "resume").await;
    let mut expected: Vec<String> = created.clone();
    expected.sort();

    let label = "the first of two pages of two";
    let first = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, Some(2), None),
    )
    .await;
    assert_eq!(
        ids_of(&first, label),
        expected[..2].to_vec(),
        "case {label:?}: got {first}"
    );
    let token = token_of(&first).expect("a non-final page carries a NextToken");

    let label = "the page resuming strictly after the cursor";
    let second = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    assert_eq!(
        ids_of(&second, label),
        expected[2..].to_vec(),
        "case {label:?}: the page serves only the entries after the cursor, got {second}"
    );
    assert!(
        token_of(&second).is_none(),
        "case {label:?}: the final page carries no NextToken, got {second}"
    );

    // The SAME token asked again answers the same page: a token is a cursor, not
    // a consumed resource.
    let repeated = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, Some(60), Some(&token)),
    )
    .await;
    assert_eq!(
        repeated, second,
        "a page token is stable across calls, got {repeated}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_terms_documents_of_one_pool_only() {
    let router = router();
    let first_pool = pool(&router, "first-terms-pool").await;
    let second_pool = pool(&router, "second-terms-pool").await;
    let first_ids = stored_terms(&router, &first_pool, 2, "first").await;
    let second_ids = stored_terms(&router, &second_pool, 1, "second").await;

    for (label, user_pool_id, expected) in [
        (
            "the pool holding two sets of documents",
            &first_pool,
            &first_ids,
        ),
        (
            "another pool holding a single set",
            &second_pool,
            &second_ids,
        ),
    ] {
        let payload = ok(&router, "ListTerms", &list_request(user_pool_id, None, None)).await;
        let actual: BTreeSet<String> = ids_of(&payload, label).into_iter().collect();
        let expected: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_an_update_a_later_call_made() {
    let router = router();
    let user_pool_id = pool(&router, "updated-terms-pool").await;
    let client_id = client(&router, &user_pool_id, "updated-terms-client").await;
    let terms_id = terms(&router, &user_pool_id, &client_id, TERMS_NAMES[0]).await;

    let label = "a listing before the rename";
    let before = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let first = entries_of(&before, label);
    assert_eq!(
        first[0]["TermsName"],
        json!(TERMS_NAMES[0]),
        "case {label:?}: the listing reports the created name, got {before}"
    );

    // `UpdateTerms` renames the STORED documents, so the next listing reports
    // the new name — the listing renders stored state, not the create's request.
    ok(
        &router,
        "UpdateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "TermsId": terms_id,
            "TermsName": TERMS_NAMES[1],
        }),
    )
    .await;

    let label = "a listing after the rename";
    let after = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let second = entries_of(&after, label);
    assert_eq!(
        second[0]["TermsName"],
        json!(TERMS_NAMES[1]),
        "case {label:?}: the listing reports the updated name, got {after}"
    );
    assert_eq!(
        second[0]["CreationDate"], first[0]["CreationDate"],
        "case {label:?}: an update leaves the creation instant alone, got {after}"
    );
    assert_eq!(
        second[0]["TermsId"], first[0]["TermsId"],
        "case {label:?}: an update keeps the same resource, got {after}"
    );
}

#[tokio::test]
async fn an_absent_pool_and_an_unissued_token_are_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-terms-pool").await;
    let other_pool = pool(&router, "other-terms-pool").await;
    let created = stored_terms(&router, &user_pool_id, 2, "refusal").await;
    stored_terms(&router, &other_pool, 2, "refusal-other").await;

    // A token this listing DID mint, for one pool: the row below echoes it back
    // for the other pool, whose listing it names no page of.
    let first_page = ok(
        &router,
        "ListTerms",
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
            (400, "ResourceNotFoundException"),
        ),
        (
            "a NextToken no listing of this service minted",
            list_request(&user_pool_id, None, Some("not-a-token")),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken that is a TermsId rather than a cursor",
            list_request(&user_pool_id, None, Some(&created[0])),
            (400, "InvalidParameterException"),
        ),
        (
            "a NextToken minted for another pool's listing",
            list_request(&other_pool, None, Some(&minted)),
            (400, "InvalidParameterException"),
        ),
        (
            "an empty NextToken, which the modelled length 0..131072 admits",
            list_request(&user_pool_id, None, Some("")),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "ListTerms", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // The refusals persisted nothing: the pool still holds both sets.
    let payload = ok(
        &router,
        "ListTerms",
        &list_request(&user_pool_id, None, None),
    )
    .await;
    let actual: BTreeSet<String> = ids_of(&payload, "after the refusals").into_iter().collect();
    let expected: BTreeSet<String> = created.iter().cloned().collect();
    assert_eq!(
        actual, expected,
        "a refused listing changes no stored terms documents: expected {expected:?}, got \
         {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-terms-pool").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListTerms")`): `MaxResults`
    // range 1..60, `UserPoolId` length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`,
    // and `NextToken` length 0..131072.
    let over_long_token = "t".repeat(131_073);
    for (label, body) in [
        (
            "MaxResults 61, one past the range maximum 60",
            list_request(&user_pool_id, Some(61), None),
        ),
        (
            "MaxResults 0, one below the range minimum 1",
            list_request(&user_pool_id, Some(0), None),
        ),
        (
            "MaxResults -1, below the range minimum 1",
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
            "a NextToken one character past the length maximum 131072",
            list_request(&user_pool_id, None, Some(&over_long_token)),
        ),
        ("a request missing the required UserPoolId", json!({})),
    ] {
        let (status, payload) = call(&router, "ListTerms", &body).await;
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

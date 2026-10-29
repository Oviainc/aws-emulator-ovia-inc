// SPDX-License-Identifier: CC0-1.0

//! `DeleteTerms` over the wire, driven exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: deleting a resource named by its minted id removes that
//! one stored record of the pool the request names, so every later read of it —
//! by id, through its app client, or in the pool's listing — no longer finds it,
//! the name it held is free for its app client again, and every other stored
//! record is untouched; a request naming a pool that does not exist, or an id
//! the named pool does not hold (never minted, already deleted, or stored in
//! another pool), is refused with a 400 `ResourceNotFoundException` and deletes
//! nothing.
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

/// The two document types `TermsNameType`'s pattern admits
/// (`^(terms-of-use|privacy-policy)$`, from
/// `smithy_query(query="operation",operation="DescribeTermsByClient")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

/// A canonical v4 UUID `TermsIdType`'s pattern admits that no create in this
/// suite ever mints (pattern from
/// `smithy_query(query="operation",operation="DeleteTerms")`).
const UNMINTED_TERMS_ID: &str = "00000000-0000-4000-8000-000000000000";

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
    links: &Value,
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
            "Links": links,
        }),
    )
    .await;
    created["Terms"]["TermsId"]
        .as_str()
        .expect("a minted TermsId")
        .to_string()
}

/// The `TermsId`s the pool's listing reports, gathered over every page.
async fn listed_ids(router: &MultiServiceRouter, user_pool_id: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = serde_json::Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        if let Some(token) = &next_token {
            body.insert("NextToken".to_string(), json!(token));
        }
        let payload = ok(router, "ListTerms", &Value::Object(body)).await;
        for entry in payload["Terms"].as_array().expect("Terms is a list") {
            ids.insert(
                entry["TermsId"]
                    .as_str()
                    .expect("an entry names its documents")
                    .to_string(),
            );
        }
        next_token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return ids;
        }
    }
}

/// One row of the deletion table: which document type is deleted, how many
/// other sets the pool holds beside it, and the link map the deleted set was
/// created with. The rows differ along what the rule generalises over — the
/// document type, the position of the deleted record among the pool's stored
/// ones, how many app clients the pool spreads them over, and the size of the
/// deleted record.
struct DeleteCase {
    case: &'static str,
    pool_name: &'static str,
    terms_name: &'static str,
    /// How many further sets of documents the pool holds, each on its own app
    /// client, created BEFORE the one this row deletes.
    before: usize,
    /// How many further sets the pool holds, created AFTER it.
    after: usize,
    links: Value,
}

fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "the only set of documents in the pool",
            pool_name: "delete-only-pool",
            terms_name: TERMS_NAMES[0],
            before: 0,
            after: 0,
            links: json!({ "cognito:default": "https://terms.example.com" }),
        },
        DeleteCase {
            case: "the other document type, the first of three stored sets",
            pool_name: "delete-first-pool",
            terms_name: TERMS_NAMES[1],
            before: 0,
            after: 2,
            links: json!({
                "cognito:spanish": "https://privacy.example.com/es",
                "cognito:default": "https://privacy.example.com",
            }),
        },
        DeleteCase {
            case: "a set stored between two others, on the middle app client",
            pool_name: "delete-middle-pool",
            terms_name: TERMS_NAMES[0],
            before: 1,
            after: 1,
            links: json!({ "cognito:french": "https://terms.example.com/fr" }),
        },
        DeleteCase {
            case: "the last of four stored sets, carrying several languages",
            pool_name: "delete-last-pool",
            terms_name: TERMS_NAMES[1],
            before: 3,
            after: 0,
            links: json!({
                "cognito:english": "https://privacy.example.com/en",
                "cognito:japanese": "https://privacy.example.com/ja",
                "cognito:korean": "https://privacy.example.com/ko",
                "cognito:chinese-traditional": "https://privacy.example.com/zh",
            }),
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_the_named_set_and_leaves_every_other_stored_one() {
    let router = router();
    for case in delete_cases() {
        let label = case.case;
        let user_pool_id = pool(&router, case.pool_name).await;

        // The sets this row does NOT delete, each on its own app client: a
        // document's name is unique to its app client, so several sets of one
        // pool spread over several clients.
        let mut survivors = Vec::new();
        for index in 0..case.before {
            let other = client(&router, &user_pool_id, &format!("before-{index}")).await;
            survivors.push(
                terms(
                    &router,
                    &user_pool_id,
                    &other,
                    TERMS_NAMES[index % TERMS_NAMES.len()],
                    &json!({ "cognito:default": "https://other.example.com" }),
                )
                .await,
            );
        }

        let client_id = client(&router, &user_pool_id, "target-client").await;
        let terms_id = terms(
            &router,
            &user_pool_id,
            &client_id,
            case.terms_name,
            &case.links,
        )
        .await;

        for index in 0..case.after {
            let other = client(&router, &user_pool_id, &format!("after-{index}")).await;
            survivors.push(
                terms(
                    &router,
                    &user_pool_id,
                    &other,
                    TERMS_NAMES[index % TERMS_NAMES.len()],
                    &json!({ "cognito:default": "https://other.example.com" }),
                )
                .await,
            );
        }

        // The documents are there to begin with, by id and through their app
        // client.
        ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;

        let (status, payload) = call(
            &router,
            "DeleteTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: a delete of stored documents succeeds, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            json!({}),
            "case {label:?}: this operation models no output, so its body carries no member, got \
             {payload}"
        );

        // Gone from every read: by id, through its app client, and from the
        // pool's listing.
        let (status, payload) = call(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: the deleted documents are no longer readable by id, got {status} \
             with {payload}"
        );
        let (status, payload) = call(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": case.terms_name,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: the deleted documents are no longer readable through their app \
             client, got {status} with {payload}"
        );

        let listed = listed_ids(&router, &user_pool_id).await;
        let expected: BTreeSet<String> = survivors.iter().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {label:?}: the pool's listing reports exactly the surviving sets: expected \
             {expected:?}, got {listed:?}"
        );
        for survivor in &survivors {
            ok(
                &router,
                "DescribeTerms",
                &json!({ "UserPoolId": user_pool_id, "TermsId": survivor }),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn the_deleted_name_is_free_for_its_app_client_again() {
    let router = router();
    let user_pool_id = pool(&router, "recreate-terms-pool").await;

    for (index, terms_name) in TERMS_NAMES.into_iter().enumerate() {
        let label = terms_name;
        let client_id = client(&router, &user_pool_id, &format!("recreate-{index}")).await;
        let first = terms(
            &router,
            &user_pool_id,
            &client_id,
            terms_name,
            &json!({ "cognito:default": "https://first.example.com" }),
        )
        .await;

        // While it is stored, the name is taken for this app client.
        let (status, payload) = call(
            &router,
            "CreateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://second.example.com" },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "TermsExistsException"),
            "case {label:?}: the stored name is taken for its app client, got {status} with \
             {payload}"
        );

        ok(
            &router,
            "DeleteTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": first }),
        )
        .await;

        // Deleted, so the name is free again — and the recreated documents are a
        // NEW resource with their own id and their own links.
        let second = terms(
            &router,
            &user_pool_id,
            &client_id,
            terms_name,
            &json!({ "cognito:default": "https://second.example.com" }),
        )
        .await;
        assert_ne!(
            second, first,
            "case {label:?}: documents recreated after a delete are a new resource, got \
             {second:?} twice"
        );
        let read = ok(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
            }),
        )
        .await;
        assert_eq!(
            read["Terms"]["TermsId"],
            json!(second),
            "case {label:?}: the app client now holds the recreated documents, got {read}"
        );
        assert_eq!(
            read["Terms"]["Links"],
            json!({ "cognito:default": "https://second.example.com" }),
            "case {label:?}: the recreated documents carry their own links, got {read}"
        );

        // Leave the pool as it was found, so each row starts from a free name.
        ok(
            &router,
            "DeleteTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": second }),
        )
        .await;
    }
}

#[tokio::test]
async fn an_id_the_named_pool_does_not_hold_is_a_400_and_deletes_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-delete-pool").await;
    let other_pool = pool(&router, "other-delete-pool").await;
    let client_id = client(&router, &user_pool_id, "refusal-client").await;
    let other_client = client(&router, &other_pool, "other-client").await;
    let terms_id = terms(
        &router,
        &user_pool_id,
        &client_id,
        TERMS_NAMES[0],
        &json!({ "cognito:default": "https://terms.example.com" }),
    )
    .await;
    let other_terms_id = terms(
        &router,
        &other_pool,
        &other_client,
        TERMS_NAMES[0],
        &json!({ "cognito:default": "https://other.example.com" }),
    )
    .await;

    // A set of documents deleted once: a second delete of the same id names
    // nothing.
    ok(
        &router,
        "DeleteTerms",
        &json!({ "UserPoolId": other_pool, "TermsId": other_terms_id }),
    )
    .await;

    for (label, body) in [
        (
            "a user pool that does not exist",
            json!({ "UserPoolId": "us-east-1_absent", "TermsId": terms_id }),
        ),
        (
            "an id no create ever minted",
            json!({ "UserPoolId": user_pool_id, "TermsId": UNMINTED_TERMS_ID }),
        ),
        (
            "an id stored in another pool",
            json!({ "UserPoolId": other_pool, "TermsId": terms_id }),
        ),
        (
            "an id this pool's documents were already deleted under",
            json!({ "UserPoolId": other_pool, "TermsId": other_terms_id }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteTerms", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: expected a 400 ResourceNotFoundException, got {status} with {payload}"
        );
    }

    // None of the refusals deleted anything: the one stored set is still there.
    let read = ok(
        &router,
        "DescribeTerms",
        &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
    )
    .await;
    assert_eq!(
        read["Terms"]["TermsId"],
        json!(terms_id),
        "a refused delete removes no stored terms documents, got {read}"
    );
    let listed = listed_ids(&router, &user_pool_id).await;
    assert_eq!(
        listed,
        BTreeSet::from([terms_id.clone()]),
        "a refused delete leaves the pool's listing alone, got {listed:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_deletes_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-delete-pool").await;
    let client_id = client(&router, &user_pool_id, "constraint-client").await;
    let terms_id = terms(
        &router,
        &user_pool_id,
        &client_id,
        TERMS_NAMES[0],
        &json!({ "cognito:default": "https://terms.example.com" }),
    )
    .await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="DeleteTerms")`): `UserPoolId`
    // length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`, `TermsId` the canonical
    // v4 UUID pattern.
    for (label, body) in [
        (
            "an empty user pool id, below the length minimum 1",
            json!({ "UserPoolId": "", "TermsId": terms_id }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "TermsId": terms_id }),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            json!({
                "UserPoolId": format!("us-east-1_{}", "a".repeat(46)),
                "TermsId": terms_id,
            }),
        ),
        (
            "a terms id that is not a UUID at all",
            json!({ "UserPoolId": user_pool_id, "TermsId": "not-a-uuid" }),
        ),
        (
            "a UUID of another version, which the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": "00000000-0000-1000-8000-000000000000",
            }),
        ),
        (
            "a request missing the required TermsId",
            json!({ "UserPoolId": user_pool_id }),
        ),
        (
            "a request missing the required UserPoolId",
            json!({ "TermsId": terms_id }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteTerms", &body).await;
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

    // The refused requests deleted nothing.
    let read = ok(
        &router,
        "DescribeTerms",
        &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
    )
    .await;
    assert_eq!(
        read["Terms"]["TermsId"],
        json!(terms_id),
        "a refused delete removes no stored terms documents, got {read}"
    );
}

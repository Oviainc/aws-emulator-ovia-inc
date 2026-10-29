// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal a terms-document lookup that resolves nothing
//! answers, over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: every operation that resolves a set of terms documents
//! and finds none refuses with ONE fixed sentence — the same text whether the
//! documents were looked up by their `TermsId` in the named pool or by the app
//! client and document-type name that address them together, whether they were
//! deleted, never created, or belong to another pool, and whether the caller is
//! reading, updating or deleting. The sentence quotes nothing the request asked
//! for: not the id, not the name, not the app client id (which the model marks
//! `@sensitive`) and not the pool, so a caller that classifies refusals by their
//! message sees one not-found class rather than one error per request.
//!
//! Each table row names the case it asserts, spans a different way the lookup
//! can resolve nothing, and every assertion reports the actual value it saw.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

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

/// The one sentence a lookup that resolves no terms documents answers.
const TERMS_NOT_FOUND_MESSAGE: &str = "Terms does not exist.";

/// Two ids no `CreateTerms` of this suite minted, both in the form
/// `TermsIdType`'s pattern demands
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`,
/// from `smithy_query(query="operation",operation="DescribeTerms")`): one at the
/// bottom of the hex range in lower case, one at the top in upper case.
const UNMINTED_TERMS_IDS: [&str; 2] = [
    "00000000-0000-4000-8000-000000000000",
    "FFFFFFFF-FFFF-4FFF-BFFF-FFFFFFFFFFFF",
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

/// Store one set of terms documents of type `terms_name` for `client_id` and
/// return its minted `TermsId`.
async fn create_terms(
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
            "Links": { "cognito:default": "https://terms.example.com" },
        }),
    )
    .await;
    created["Terms"]["TermsId"]
        .as_str()
        .expect("a minted TermsId")
        .to_string()
}

/// One row of the table: the operation to call, its request body, and the
/// request values the refusal must not quote.
struct NotFoundCase {
    case: String,
    operation: &'static str,
    body: Value,
    quoted_by_nothing: Vec<String>,
}

#[tokio::test]
async fn a_lookup_that_resolves_no_terms_documents_answers_one_fixed_sentence() {
    let router = router();
    let user_pool_id = pool(&router, "terms-not-found-pool").await;
    let client_id = client(&router, &user_pool_id, "holds-terms").await;
    let bare_client = client(&router, &user_pool_id, "holds-nothing").await;
    let other_pool = pool(&router, "terms-not-found-other-pool").await;
    let other_client = client(&router, &other_pool, "other-pool-client").await;

    // The set that survives every refusal below, and one that is deleted so the
    // reads of it resolve nothing the way the delete left them.
    let kept = create_terms(&router, &user_pool_id, &client_id, TERMS_NAMES[0]).await;
    let deleted = create_terms(&router, &user_pool_id, &client_id, TERMS_NAMES[1]).await;
    let other_pool_terms = create_terms(&router, &other_pool, &other_client, TERMS_NAMES[0]).await;
    ok(
        &router,
        "DeleteTerms",
        &json!({ "UserPoolId": user_pool_id, "TermsId": deleted }),
    )
    .await;

    let mut cases = vec![
        NotFoundCase {
            case: "a read by the id of documents DeleteTerms removed".to_string(),
            operation: "DescribeTerms",
            body: json!({ "UserPoolId": user_pool_id, "TermsId": deleted }),
            quoted_by_nothing: vec![deleted.clone(), user_pool_id.clone()],
        },
        NotFoundCase {
            case: "a read by app client and name after DeleteTerms removed the documents"
                .to_string(),
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": TERMS_NAMES[1],
            }),
            quoted_by_nothing: vec![
                client_id.clone(),
                TERMS_NAMES[1].to_string(),
                user_pool_id.clone(),
            ],
        },
        NotFoundCase {
            case: "a read by the id of documents ANOTHER pool holds".to_string(),
            operation: "DescribeTerms",
            body: json!({ "UserPoolId": user_pool_id, "TermsId": other_pool_terms }),
            quoted_by_nothing: vec![other_pool_terms.clone(), user_pool_id.clone()],
        },
        NotFoundCase {
            case: "a delete of documents already deleted".to_string(),
            operation: "DeleteTerms",
            body: json!({ "UserPoolId": user_pool_id, "TermsId": deleted }),
            quoted_by_nothing: vec![deleted.clone(), user_pool_id.clone()],
        },
        NotFoundCase {
            case: "an update of documents already deleted".to_string(),
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": deleted,
                "Enforcement": "NONE",
            }),
            quoted_by_nothing: vec![deleted.clone(), user_pool_id.clone()],
        },
        NotFoundCase {
            case: "an update naming the id of documents ANOTHER pool holds".to_string(),
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": user_pool_id,
                "TermsId": other_pool_terms,
                "TermsName": TERMS_NAMES[1],
            }),
            quoted_by_nothing: vec![
                other_pool_terms.clone(),
                TERMS_NAMES[1].to_string(),
                user_pool_id.clone(),
            ],
        },
    ];

    // The same refusal for an id nothing was ever minted for, in both hex cases
    // the id's pattern admits, across every operation that reaches documents by
    // id.
    for terms_id in UNMINTED_TERMS_IDS {
        for operation in ["DescribeTerms", "DeleteTerms", "UpdateTerms"] {
            cases.push(NotFoundCase {
                case: format!("{operation} on the never-minted id {terms_id}"),
                operation,
                body: json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
                quoted_by_nothing: vec![terms_id.to_string(), user_pool_id.clone()],
            });
        }
    }

    // And for an app client of this pool that holds no documents at all, under
    // each document type a name may ask for.
    for terms_name in TERMS_NAMES {
        cases.push(NotFoundCase {
            case: format!("a read by an app client that holds nothing, for {terms_name}"),
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": user_pool_id,
                "ClientId": bare_client,
                "TermsName": terms_name,
            }),
            quoted_by_nothing: vec![
                bare_client.clone(),
                terms_name.to_string(),
                user_pool_id.clone(),
            ],
        });
    }

    for NotFoundCase {
        case,
        operation,
        body,
        quoted_by_nothing,
    } in cases
    {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: must be a 400 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, TERMS_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the fixed not-found sentence, got {message:?}"
        );
        for value in &quoted_by_nothing {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the refusal must quote nothing the request asked for, \
                 but {message:?} carries {value:?}"
            );
        }
        assert!(
            payload.get("Terms").is_none(),
            "case {case:?}: a refusal carries no terms documents, got {payload}"
        );
    }

    // The deleted documents are gone from every view, and the surviving set is
    // untouched by the refusals: none of them wrote anything.
    let listed = ok(&router, "ListTerms", &json!({ "UserPoolId": user_pool_id })).await;
    let listed_ids: Vec<String> = listed["Terms"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTerms must answer a Terms list, got {listed}"))
        .iter()
        .map(|entry| entry["TermsId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids,
        vec![kept.clone()],
        "the deleted documents must be gone from the listing and the kept ones still in it, got {listed}"
    );
    let read = ok(
        &router,
        "DescribeTerms",
        &json!({ "UserPoolId": user_pool_id, "TermsId": kept }),
    )
    .await;
    assert_eq!(
        read["Terms"]["TermsName"],
        json!(TERMS_NAMES[0]),
        "the surviving documents must still read back as stored, got {read}"
    );
    let read_by_client = ok(
        &router,
        "DescribeTermsByClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": TERMS_NAMES[0],
        }),
    )
    .await;
    assert_eq!(
        read_by_client["Terms"], read["Terms"],
        "a read by app client and name must still answer the stored documents, got {read_by_client}"
    );
}

/// The refusal for an app client the named pool does not hold is a DIFFERENT
/// class: there the app client, not the terms documents, is what fails to
/// resolve, so that refusal keeps its own wording and must not be flattened into
/// the terms-documents sentence.
#[tokio::test]
async fn an_unresolved_app_client_keeps_its_own_wording() {
    let router = router();
    let user_pool_id = pool(&router, "terms-not-found-client-pool").await;
    let other_pool = pool(&router, "terms-not-found-client-other-pool").await;
    let client_id = client(&router, &user_pool_id, "this-pools-client").await;

    for (case, pool_id, asked_client) in [
        (
            "an app client id no pool ever minted",
            user_pool_id.clone(),
            "neverminted1234".to_string(),
        ),
        (
            "this pool's own app client, asked against another pool",
            other_pool.clone(),
            client_id.clone(),
        ),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": pool_id,
                "ClientId": asked_client,
                "TermsName": TERMS_NAMES[0],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: must be a 400 ResourceNotFoundException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message,
            format!("User pool client {asked_client} does not exist in user pool {pool_id}."),
            "case {case:?}: an unresolved app client keeps its own wording, got {message:?}"
        );
    }
}

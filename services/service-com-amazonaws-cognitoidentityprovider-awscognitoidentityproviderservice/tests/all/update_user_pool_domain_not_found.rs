// SPDX-License-Identifier: CC0-1.0

//! How `UpdateUserPoolDomain` refuses a `UserPoolId` + `Domain` pair that holds
//! no stored domain. Driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: this operation reconfigures an EXISTING domain record
//! ("This operation doesn't change the name of your user pool domain. To change
//! your domain, delete it with DeleteUserPoolDomain and create a new domain
//! with CreateUserPoolDomain"), so a pair that resolves no stored domain names a
//! resource the service cannot find — `ResourceNotFoundException`, which the
//! model gives HTTP 400
//! (`smithy_query(query="operation",operation="UpdateUserPoolDomain")`) — and
//! not a malformed parameter, whichever half of the pair is wrong and whatever
//! else the request carries. The sentence it carries is fixed text in full: the
//! pair is resolved as a unit, so the refusal names neither half.
//!
//! A domain name is unique across pools, so ONE pairing resolves after all: a
//! stored pool paired with a domain name ANOTHER stored pool holds. Nothing is
//! missing there — the pool named is simply not the one configured with that
//! domain — so both pair-addressed operations answer
//! `InvalidParameterException` with the sentence for that mismatch.
//!
//! The control that the two pair-addressed operations are NOT one refusal:
//! `DeleteUserPoolDomain`, asked to remove the very same unresolvable pair,
//! keeps answering `400 InvalidParameterException` with its own sentence.
//!
//! Note on the domain strings below: `DomainType`'s modelled pattern
//! `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$` admits no dots, so even the
//! custom-domain rows name dotless domain strings.

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

/// An ACM certificate ARN, which is what makes a create request a CUSTOM domain
/// (`CreateUserPoolDomain.CustomDomainConfig`).
const CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// The sentence `UpdateUserPoolDomain` answers when the pair it is addressed by
/// holds no stored domain.
const DOMAIN_NOT_FOUND: &str = "User pool domain was not found.";

/// The sentence `DeleteUserPoolDomain` answers for the same unresolvable pair,
/// which is deliberately a different refusal.
const NO_SUCH_DOMAIN: &str = "No such domain or user pool exists.";

/// The sentence both pair-addressed operations answer when the pair resolves — a
/// stored pool and a domain name another stored pool holds — but names a pool
/// that is not the one configured with that domain.
const DOMAIN_OF_OTHER_POOL: &str = "The domain is not configured for this user pool.";

/// A well-formed `UserPoolId` (pattern `^[\w-]+_[0-9a-zA-Z]+$`) that no
/// `CreateUserPool` call minted.
const ABSENT_POOL: &str = "us-east-1_absent";

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

/// The `message` of an error body.
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

/// Create `domain` on `user_pool_id`, as a custom domain when `custom`.
async fn create_domain(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    domain: &str,
    custom: bool,
) {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Domain".to_string(), json!(domain));
    if custom {
        body.insert(
            "CustomDomainConfig".to_string(),
            json!({ "CertificateArn": CERTIFICATE }),
        );
    }
    ok(router, "CreateUserPoolDomain", &Value::Object(body)).await;
}

/// One row of the unresolvable-pair table.
struct PairCase {
    case: &'static str,
    user_pool_id: String,
    domain: String,
    /// The members the `UpdateUserPoolDomain` request carries beside the pair,
    /// so the refusal is measured against an empty request, a request asking for
    /// a legal change, and a request whose other members are themselves illegal.
    members: Value,
    /// Whether the pair RESOLVES to a stored pool and a domain name another
    /// stored pool holds, which is the mismatch both operations refuse as a bad
    /// parameter rather than an absent resource.
    of_other_pool: bool,
}

#[tokio::test]
async fn an_unresolvable_pool_and_domain_pair_is_the_resource_the_service_cannot_find() {
    let router = router();

    let holder = pool(&router, "not-found-holder").await;
    let other = pool(&router, "not-found-other").await;
    // A pool that takes a prefix and a custom domain and then gives both up:
    // the recorded shape of this defect is an update that follows a delete.
    create_domain(&router, &holder, "deleted-prefix", false).await;
    create_domain(&router, &holder, "deleted-custom", true).await;
    for domain in ["deleted-prefix", "deleted-custom"] {
        ok(
            &router,
            "DeleteUserPoolDomain",
            &json!({ "UserPoolId": holder, "Domain": domain }),
        )
        .await;
    }
    // One name the holder keeps, so a pair can fail by naming the WRONG pool for
    // a domain that does exist.
    create_domain(&router, &holder, "held-prefix", false).await;
    // A pool that is deleted outright, domain and all: the pair's pool half is
    // gone rather than never minted.
    let dropped = pool(&router, "not-found-dropped").await;
    create_domain(&router, &dropped, "dropped-prefix", false).await;
    ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": dropped }),
    )
    .await;

    // The rows differ along the way the pair fails to resolve (a prefix domain a
    // delete took away, a custom domain a delete took away, a name nothing ever
    // created, a name another pool holds, a pool never minted, a pool deleted
    // outright) and along what the request carries beside the pair.
    let cases = vec![
        PairCase {
            case: "a prefix domain a delete took away",
            user_pool_id: holder.clone(),
            domain: "deleted-prefix".to_string(),
            members: json!({ "ManagedLoginVersion": 1 }),
            of_other_pool: false,
        },
        PairCase {
            case: "a custom domain a delete took away",
            user_pool_id: holder.clone(),
            domain: "deleted-custom".to_string(),
            members: json!({ "CustomDomainConfig": { "CertificateArn": CERTIFICATE } }),
            of_other_pool: false,
        },
        PairCase {
            case: "a name nothing ever created, with no updatable member",
            user_pool_id: holder.clone(),
            domain: "never-created".to_string(),
            members: json!({}),
            of_other_pool: false,
        },
        PairCase {
            case: "a name nothing ever created, asking for managed login",
            user_pool_id: holder.clone(),
            domain: "never-created".to_string(),
            members: json!({ "ManagedLoginVersion": 2 }),
            of_other_pool: false,
        },
        PairCase {
            case: "a name ANOTHER pool holds",
            user_pool_id: other.clone(),
            domain: "held-prefix".to_string(),
            members: json!({ "ManagedLoginVersion": 2 }),
            of_other_pool: true,
        },
        PairCase {
            case: "a pool nothing ever minted, naming a domain a pool does hold",
            user_pool_id: ABSENT_POOL.to_string(),
            domain: "held-prefix".to_string(),
            members: json!({ "ManagedLoginVersion": 1 }),
            of_other_pool: false,
        },
        PairCase {
            case: "a pool nothing ever minted, naming a name nothing created",
            user_pool_id: ABSENT_POOL.to_string(),
            domain: "never-created".to_string(),
            members: json!({}),
            of_other_pool: false,
        },
        PairCase {
            case: "a pool a delete took away, naming the domain it used to hold",
            user_pool_id: dropped.clone(),
            domain: "dropped-prefix".to_string(),
            members: json!({ "ManagedLoginVersion": 2 }),
            of_other_pool: false,
        },
        PairCase {
            case: "an unresolvable pair whose branding version is illegal too",
            user_pool_id: holder.clone(),
            domain: "never-created".to_string(),
            members: json!({ "ManagedLoginVersion": 3 }),
            of_other_pool: false,
        },
        PairCase {
            case: "an unresolvable pair carrying routing, which a prefix domain forbids",
            user_pool_id: holder.clone(),
            domain: "never-created".to_string(),
            members: json!({
                "Routing": {
                    "Failover": {
                        "SecondaryRegion": "us-west-2",
                        "PrimaryRoute53HealthCheckId": "abcdef01-2345-6789-abcd-ef0123456789",
                    },
                },
            }),
            of_other_pool: false,
        },
    ];

    for case in &cases {
        let label = case.case;
        let mut body = case
            .members
            .as_object()
            .expect("the update members are an object")
            .clone();
        body.insert("UserPoolId".to_string(), json!(case.user_pool_id));
        body.insert("Domain".to_string(), json!(case.domain));
        let body = Value::Object(body);

        // What this pairing owes: the mismatch refusal when it resolves to a
        // stored pool and another stored pool's domain, and the not-found
        // refusal when it resolves nothing.
        let (expected_code, expected_message) = if case.of_other_pool {
            ("InvalidParameterException", DOMAIN_OF_OTHER_POOL)
        } else {
            ("ResourceNotFoundException", DOMAIN_NOT_FOUND)
        };
        let (status, payload) = call(&router, "UpdateUserPoolDomain", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, expected_code),
            "case {label:?}: expected a 400 {expected_code}, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {label:?}: expected {expected_message:?}, got {payload}"
        );
        // Fixed text in full: the sentence reports the pair and echoes neither
        // half of it, so no row's own values can appear in it.
        let message = error_message(&payload).to_string();
        assert!(
            !message.contains(&case.user_pool_id) && !message.contains(&case.domain),
            "case {label:?}: the sentence names neither half of the pair, got {message:?}"
        );

        // The control: removing the very same pair is a different refusal when
        // the pair resolves nothing, so the two operations do not share one.
        let removal = json!({ "UserPoolId": case.user_pool_id, "Domain": case.domain });
        let (status, payload) = call(&router, "DeleteUserPoolDomain", &removal).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: removing the same pair keeps its own refusal, got {status} \
             with {payload}"
        );
        let expected_removal = if case.of_other_pool {
            DOMAIN_OF_OTHER_POOL
        } else {
            NO_SUCH_DOMAIN
        };
        assert_eq!(
            error_message(&payload),
            expected_removal,
            "case {label:?}: removing the same pair owes {expected_removal:?}, got {payload}"
        );

        // A refused update creates nothing: the name is still one the reader
        // binds no member for.
        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        let described_pool = read["DomainDescription"]["UserPoolId"].clone();
        let expected_pool = if case.domain == "held-prefix" {
            json!(holder)
        } else {
            Value::Null
        };
        assert_eq!(
            described_pool, expected_pool,
            "case {label:?}: a refused update neither stores nor moves a domain, got {read}"
        );
    }

    // The positive control: the pair the holder DOES hold resolves, and the
    // update it asks for round-trips through the reader.
    let updated = ok(
        &router,
        "UpdateUserPoolDomain",
        &json!({ "UserPoolId": holder, "Domain": "held-prefix", "ManagedLoginVersion": 2 }),
    )
    .await;
    assert_eq!(
        updated["ManagedLoginVersion"],
        json!(2),
        "a resolvable pair is updated rather than refused, got {updated}"
    );
    let read = ok(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "held-prefix" }),
    )
    .await;
    assert_eq!(
        read["DomainDescription"]["ManagedLoginVersion"],
        json!(2),
        "the stored domain carries the updated branding version, got {read}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! Deleting a user pool domain over the wire, driven through the router exactly
//! as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `DeleteUserPoolDomain` removes the domain record the
//! NAMED pool holds under the given name — whichever of the pool's two domain
//! slots (`UserPoolType.Domain`, `UserPoolType.CustomDomain`) holds it, and
//! whatever status the clock has that domain in — so afterwards neither the pool
//! nor a read of that domain string reports it, the name is free again, and the
//! pool's other domain is untouched; a request naming a pool that does not
//! exist, or a name that pool does not hold (including one another pool holds),
//! deletes nothing and is refused as a bad parameter, since the pair
//! `UserPoolId` + `Domain` is what addresses the domain.
//!
//! Note on the domain strings below: `DomainType`'s modelled pattern
//! `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$` admits no dots, and the framework
//! refuses a modelled-constraint violation before the handler runs, so even the
//! custom-domain rows name dotless domain strings.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state::DOMAIN_ACTIVATION_SPAN;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Long enough that the clock has made a created domain's activation due, so a
/// row can delete a domain that has reached `ACTIVE`.
const PAST_THE_ACTIVATION: Duration =
    DOMAIN_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// An ACM certificate ARN, which is what makes a request a CUSTOM domain
/// (`CreateUserPoolDomain.CustomDomainConfig`).
const CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

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

/// Assert that no read of `domain` reports a stored domain any more.
///
/// `DescribeUserPoolDomain` is a different operation with its own report of what
/// an unknown domain answers, so this pins only what deleting the domain is
/// about: whatever that reader answers, it no longer names this domain.
async fn assert_domain_is_gone(router: &MultiServiceRouter, label: &str, domain: &str) {
    let (status, payload) = call(
        router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": domain }),
    )
    .await;
    let reported = payload
        .get("DomainDescription")
        .and_then(|description| description.get("Domain"));
    assert!(
        reported.is_none(),
        "case {label:?}: a deleted domain is no longer reported, got {status} with {payload}"
    );
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// One row of the delete table. The rows differ along what the rule generalises
/// over: the kind of domain (prefix or custom, which is also which of the pool's
/// two slots holds it), the optional members the create set, the length and form
/// of the domain string, and whether the clock has moved the domain out of its
/// transient status by the time the delete arrives.
struct DeleteCase {
    case: &'static str,
    domain: String,
    managed_login_version: Option<i32>,
    custom_domain_config: Option<Value>,
    routing: Option<Value>,
    /// Whether to wait past [`DOMAIN_ACTIVATION_SPAN`] before deleting, so the
    /// row deletes a domain the clock has made `ACTIVE` rather than one still in
    /// the status its create left it in.
    wait_for_active: bool,
}

impl DeleteCase {
    /// This case's `CreateUserPoolDomain` request for `user_pool_id`: an
    /// optional member the case leaves unset is left OUT of the request.
    fn create_request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Domain".to_string(), json!(self.domain));
        if let Some(version) = self.managed_login_version {
            body.insert("ManagedLoginVersion".to_string(), json!(version));
        }
        if let Some(config) = &self.custom_domain_config {
            body.insert("CustomDomainConfig".to_string(), config.clone());
        }
        if let Some(routing) = &self.routing {
            body.insert("Routing".to_string(), routing.clone());
        }
        Value::Object(body)
    }

    /// Whether this row's domain is a CUSTOM one, which is exactly "the create
    /// carried a `CustomDomainConfig`", and so which of the pool's two domain
    /// members reported it.
    fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }
}

/// The table. `Domain` spans the digits, letters and hyphens its pattern admits
/// and both ends of its modelled length 1..63; `ManagedLoginVersion` spans both
/// branding versions the model names (`1` hosted UI classic, `2` managed login)
/// and its absence; `CustomDomainConfig` spans a certificate alone and a
/// certificate with a `SecurityPolicy`; `Routing` appears on a custom domain,
/// the only kind the model supports it for. Members and constraints from
/// `smithy_query(query="operation",operation="CreateUserPoolDomain,DeleteUserPoolDomain")`.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a prefix domain deleted straight after the create",
            domain: "delete-prefix".to_string(),
            managed_login_version: None,
            custom_domain_config: None,
            routing: None,
            wait_for_active: false,
        },
        DeleteCase {
            case: "an ACTIVE prefix domain on managed login branding",
            domain: "delete-active-prefix".to_string(),
            managed_login_version: Some(2),
            custom_domain_config: None,
            routing: None,
            wait_for_active: true,
        },
        DeleteCase {
            case: "a prefix domain of one character, the length minimum",
            domain: "z".to_string(),
            managed_login_version: Some(1),
            custom_domain_config: None,
            routing: None,
            wait_for_active: false,
        },
        DeleteCase {
            case: "a prefix domain of 63 characters, the length maximum",
            domain: format!("d{}7", "y".repeat(61)),
            managed_login_version: None,
            custom_domain_config: None,
            routing: None,
            wait_for_active: false,
        },
        DeleteCase {
            case: "a CREATING custom domain with the certificate alone",
            domain: "delete-custom".to_string(),
            managed_login_version: None,
            custom_domain_config: Some(json!({ "CertificateArn": CERTIFICATE })),
            routing: None,
            wait_for_active: false,
        },
        DeleteCase {
            case: "an ACTIVE custom domain with a security policy and a failover routing",
            domain: "delete-active-custom".to_string(),
            managed_login_version: Some(2),
            custom_domain_config: Some(json!({
                "CertificateArn": CERTIFICATE,
                "SecurityPolicy": "TLS_V1_2_2021",
            })),
            routing: Some(json!({
                "Failover": {
                    "SecondaryRegion": "us-west-2",
                    "PrimaryRoute53HealthCheckId": "abcdef01-2345-6789-abcd-ef0123456789",
                },
            })),
            wait_for_active: true,
        },
    ]
}

#[tokio::test]
async fn a_deleted_domain_is_gone_from_the_pool_and_from_the_store() {
    let router = router();

    for case in delete_cases() {
        let label = case.case;
        // Each row gets its own pool: a pool holds at most one prefix domain and
        // at most one custom domain.
        let user_pool_id = pool(&router, &format!("delete-pool-{}", case.domain)).await;
        ok(
            &router,
            "CreateUserPoolDomain",
            &case.create_request(&user_pool_id),
        )
        .await;
        // The domain really was stored before the delete, so what follows
        // measures the delete and not a create that never took.
        ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        if case.wait_for_active {
            tokio::time::sleep(PAST_THE_ACTIVATION).await;
        }

        let deleted = ok(
            &router,
            "DeleteUserPoolDomain",
            &json!({ "UserPoolId": user_pool_id, "Domain": case.domain }),
        )
        .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {label:?}: DeleteUserPoolDomain answers an empty body, got {deleted}"
        );

        // The store no longer holds the domain under that name.
        assert_domain_is_gone(&router, label, &case.domain).await;

        // And the pool reports neither kind of domain any more.
        let pool_read = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let user_pool = &pool_read["UserPool"];
        for member in ["Domain", "CustomDomain"] {
            assert!(
                user_pool.get(member).is_none(),
                "case {label:?}: the pool reports no {member} once its domain is deleted, got {pool_read}"
            );
        }

        // The name is free again: re-creating it stores a fresh domain of the
        // same kind, which the reader then renders.
        let recreated = ok(
            &router,
            "CreateUserPoolDomain",
            &case.create_request(&user_pool_id),
        )
        .await;
        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        assert_eq!(
            read["DomainDescription"]["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: a deleted name is free to create again, got {read}"
        );
        assert_eq!(
            recreated.get("CloudFrontDomain").is_some(),
            case.is_custom(),
            "case {label:?}: the re-created domain keeps its kind, got {recreated}"
        );
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_domain_it_names() {
    let router = router();
    let user_pool_id = pool(&router, "two-slot-pool").await;
    let other_pool_id = pool(&router, "bystander-pool").await;

    // The pool holds one domain in each slot, and another pool holds a third.
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "slots-prefix" }),
    )
    .await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({
            "UserPoolId": user_pool_id,
            "Domain": "slots-custom",
            "CustomDomainConfig": { "CertificateArn": CERTIFICATE },
        }),
    )
    .await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": other_pool_id, "Domain": "bystander-prefix" }),
    )
    .await;

    // (case, the domain deleted, the pool member that must go, the domain that
    // must survive the delete unchanged)
    for (label, domain, gone, survivor) in [
        (
            "deleting the prefix domain leaves the custom one",
            "slots-prefix",
            "Domain",
            Some("slots-custom"),
        ),
        (
            "deleting the custom domain leaves nothing else on the pool",
            "slots-custom",
            "CustomDomain",
            None,
        ),
    ] {
        let before = match survivor {
            Some(survivor) => Some(
                ok(
                    &router,
                    "DescribeUserPoolDomain",
                    &json!({ "Domain": survivor }),
                )
                .await,
            ),
            None => None,
        };

        ok(
            &router,
            "DeleteUserPoolDomain",
            &json!({ "UserPoolId": user_pool_id, "Domain": domain }),
        )
        .await;

        let pool_read = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert!(
            pool_read["UserPool"].get(gone).is_none(),
            "case {label:?}: the deleted domain's {gone} member is gone, got {pool_read}"
        );

        // The pool's other domain, where it has one, is the same record the
        // read before the delete rendered.
        if let (Some(survivor), Some(before)) = (survivor, before) {
            let after = ok(
                &router,
                "DescribeUserPoolDomain",
                &json!({ "Domain": survivor }),
            )
            .await;
            assert_eq!(
                after["DomainDescription"], before["DomainDescription"],
                "case {label:?}: the pool's other domain is untouched, got {after}"
            );
        }
    }

    // The other pool's domain survived both deletes.
    let bystander = ok(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "bystander-prefix" }),
    )
    .await;
    assert_eq!(
        bystander["DomainDescription"]["UserPoolId"],
        json!(other_pool_id),
        "a delete on one pool leaves another pool's domain in place, got {bystander}"
    );
}

#[tokio::test]
async fn a_delete_that_names_no_stored_domain_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-delete-pool").await;
    let other_pool_id = pool(&router, "refusal-other-pool").await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "held-domain" }),
    )
    .await;

    // (case, the request that must be refused, the sentence it owes)
    for (label, body, expected) in [
        (
            "a domain on a user pool that does not exist",
            json!({ "UserPoolId": "us-east-1_absent", "Domain": "held-domain" }),
            "No such domain or user pool exists.",
        ),
        (
            "a name the named pool does not hold",
            json!({ "UserPoolId": user_pool_id, "Domain": "never-created" }),
            "No such domain or user pool exists.",
        ),
        (
            "a name another pool holds",
            json!({ "UserPoolId": other_pool_id, "Domain": "held-domain" }),
            "The domain is not configured for this user pool.",
        ),
    ] {
        let (status, payload) = call(&router, "DeleteUserPoolDomain", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        assert_eq!(
            payload["message"],
            json!(expected),
            "case {label:?}: expected {expected:?}, got {payload}"
        );
    }

    // Every refusal left the stored domain where it was.
    let read = ok(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "held-domain" }),
    )
    .await;
    assert_eq!(
        read["DomainDescription"]["UserPoolId"],
        json!(user_pool_id),
        "a refused delete must not remove the stored domain, got {read}"
    );

    // A delete is not idempotent: the second one names a domain the pool no
    // longer holds.
    ok(
        &router,
        "DeleteUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "held-domain" }),
    )
    .await;
    let (status, payload) = call(
        &router,
        "DeleteUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "held-domain" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "deleting a domain twice is refused the second time, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_delete_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-delete-pool").await;

    // One value past each published bound of `DeleteUserPoolDomain`'s input
    // (`smithy_query(query="operation",operation="DeleteUserPoolDomain")`): the
    // domain's length 1..63 and its pattern
    // `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, and the user pool id's pattern
    // `^[\w-]+_[0-9a-zA-Z]+$`.
    for (label, body) in [
        (
            "an empty domain, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Domain": "" }),
        ),
        (
            "a domain of 64 characters, past the length maximum 63",
            json!({ "UserPoolId": user_pool_id, "Domain": "d".repeat(64) }),
        ),
        (
            "a domain in upper case, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Domain": "HeldDomain" }),
        ),
        (
            "a domain carrying dots, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Domain": "auth.example.com" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Domain": "held-domain" }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteUserPoolDomain", &body).await;
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
}

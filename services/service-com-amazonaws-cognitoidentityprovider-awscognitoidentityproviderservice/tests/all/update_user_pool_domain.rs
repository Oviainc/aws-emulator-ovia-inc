// SPDX-License-Identifier: CC0-1.0

//! `UpdateUserPoolDomain` over the wire: it reconfigures the domain the request
//! names on the pool it names, and every later read — `DescribeUserPoolDomain` —
//! renders the STORED domain. Driven through the router exactly as an SDK drives
//! it (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: this operation updates an EXISTING domain in place —
//! "This operation doesn't change the name of your user pool domain" — so each
//! member the request carries (`ManagedLoginVersion`, `CustomDomainConfig`,
//! `Routing`) replaces the stored value, a member it leaves unset leaves the
//! stored value alone, the domain's name, pool and CloudFront endpoint never
//! move, and a changed CUSTOM domain is `UPDATING` until the clock makes it
//! `ACTIVE` again while a prefix domain, which provisions nothing of its own,
//! stays `ACTIVE`. Members that configure a custom domain name nothing on a prefix
//! domain, a branding version the model does not name is refused, and a domain
//! the named pool does not hold names no resource the service can find. Each
//! table row names the case it asserts.
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

/// Long enough that the clock has made a domain's pending activation due.
const PAST_THE_ACTIVATION: Duration =
    DOMAIN_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// An ACM certificate ARN in the Region a custom domain needs one from ("ACM
/// certificates for custom domains must be in the US East (N. Virginia) Amazon
/// Web Services Region").
const US_EAST_1_CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// A second ACM certificate ARN in the same Region: "if you generate a new
/// certificate for your custom domain name, replace the original configuration
/// with the new ARN in this request" (`UpdateUserPoolDomain`).
const RENEWED_CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// An ACM certificate ARN in another Region, which pins that the submitted
/// configuration is STORED and rendered back rather than rewritten.
const US_WEST_2_CERTIFICATE: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/66666666-7777-8888-9999-aaaaaaaaaaaa";

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

/// Create a pool on `tier` (its `UserPoolTier`, one of the modelled
/// `UserPoolTierType` values) and return its minted `UserPoolId`.
async fn pool_in_tier(router: &MultiServiceRouter, name: &str, tier: Option<&str>) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(tier) = tier {
        body.insert("UserPoolTier".to_string(), json!(tier));
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    pool_in_tier(router, name, None).await
}

/// `DescribeUserPoolDomain`'s description of the stored domain, which is keyed by
/// the domain string alone.
async fn describe(router: &MultiServiceRouter, domain: &str) -> Value {
    let read = ok(
        router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": domain }),
    )
    .await;
    read["DomainDescription"].clone()
}

/// The request `members` plus the pool and domain every `UpdateUserPoolDomain`
/// request names.
fn update_body(user_pool_id: &str, domain: &str, members: &Value) -> Value {
    let mut body = members
        .as_object()
        .expect("the update members are an object")
        .clone();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("Domain".to_string(), json!(domain));
    Value::Object(body)
}

/// One row of the update table. The rows differ along what the rule generalises
/// over: the kind of domain the update lands on (prefix or custom), which of the
/// three updatable members the request carries, the direction the branding
/// version moves, and whether the request changes anything at all.
struct UpdateCase {
    case: &'static str,
    domain: &'static str,
    /// The `CustomDomainConfig` the domain is CREATED with; `None` creates an
    /// Amazon Cognito prefix domain.
    created_config: Option<Value>,
    /// The `ManagedLoginVersion` the domain is created with, or `None` to take
    /// the default.
    created_version: Option<i64>,
    /// The members the `UpdateUserPoolDomain` request carries.
    update: Value,
    /// The `ManagedLoginVersion` the stored domain must carry afterwards.
    expected_version: i64,
    /// The `CustomDomainConfig` the stored domain must carry afterwards.
    expected_config: Option<Value>,
    /// The `Routing` the stored domain must carry afterwards.
    expected_routing: Option<Value>,
    /// Whether the request changes the stored configuration, which is what puts
    /// the domain back into `UPDATING`.
    changed: bool,
}

impl UpdateCase {
    /// This row's `CreateUserPoolDomain` request for `user_pool_id`.
    fn create_body(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("Domain".to_string(), json!(self.domain));
        if let Some(version) = self.created_version {
            body.insert("ManagedLoginVersion".to_string(), json!(version));
        }
        if let Some(config) = &self.created_config {
            body.insert("CustomDomainConfig".to_string(), config.clone());
        }
        Value::Object(body)
    }
}

/// A failover routing configuration. Its members are the two `FailoverType`
/// requires (`smithy_query(query="shape",shape="FailoverType")`).
fn failover(secondary_region: &str, health_check_id: &str) -> Value {
    json!({
        "Failover": {
            "SecondaryRegion": secondary_region,
            "PrimaryRoute53HealthCheckId": health_check_id,
        },
    })
}

/// The table. The branding versions are the two `ManagedLoginVersion` names ("A
/// value of `1` indicates hosted UI (classic) and a version of `2` indicates
/// managed login") and the security policies come from the modelled
/// `SecurityPolicyType` values
/// (`smithy_query(query="shape",shape="CustomDomainConfigType,SecurityPolicyType")`).
fn update_cases() -> Vec<UpdateCase> {
    let classic_cert = json!({ "CertificateArn": US_EAST_1_CERTIFICATE });
    let policy_cert = json!({
        "CertificateArn": US_EAST_1_CERTIFICATE,
        "SecurityPolicy": "TLS_V1_2_2021",
    });
    vec![
        UpdateCase {
            case: "a prefix domain moved from hosted UI (classic) to managed login",
            domain: "prefix-to-managed",
            created_config: None,
            created_version: None,
            update: json!({ "ManagedLoginVersion": 2 }),
            expected_version: 2,
            expected_config: None,
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a prefix domain moved from managed login back to hosted UI (classic)",
            domain: "prefix-to-classic",
            created_config: None,
            created_version: Some(2),
            update: json!({ "ManagedLoginVersion": 1 }),
            expected_version: 1,
            expected_config: None,
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a prefix domain set to the branding version it already carries",
            domain: "prefix-unchanged-version",
            created_config: None,
            created_version: Some(2),
            update: json!({ "ManagedLoginVersion": 2 }),
            expected_version: 2,
            expected_config: None,
            expected_routing: None,
            changed: false,
        },
        UpdateCase {
            case: "a prefix domain with no updatable member in the request",
            domain: "prefix-no-members",
            created_config: None,
            created_version: None,
            update: json!({}),
            expected_version: 1,
            expected_config: None,
            expected_routing: None,
            changed: false,
        },
        UpdateCase {
            case: "a custom domain given a renewed certificate",
            domain: "custom-renewed-cert",
            created_config: Some(classic_cert.clone()),
            created_version: None,
            update: json!({ "CustomDomainConfig": { "CertificateArn": RENEWED_CERTIFICATE } }),
            expected_version: 1,
            expected_config: Some(json!({ "CertificateArn": RENEWED_CERTIFICATE })),
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a custom domain moved to the strictest security policy",
            domain: "custom-strict-policy",
            created_config: Some(json!({
                "CertificateArn": US_EAST_1_CERTIFICATE,
                "SecurityPolicy": "TLS_V1",
            })),
            created_version: None,
            update: json!({
                "CustomDomainConfig": {
                    "CertificateArn": RENEWED_CERTIFICATE,
                    "SecurityPolicy": "TLS_V1_3_2025",
                },
            }),
            expected_version: 1,
            expected_config: Some(json!({
                "CertificateArn": RENEWED_CERTIFICATE,
                "SecurityPolicy": "TLS_V1_3_2025",
            })),
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a custom domain given a failover routing configuration alone",
            domain: "custom-routing-only",
            created_config: Some(classic_cert.clone()),
            created_version: None,
            update: json!({
                "Routing": failover("us-west-2", "abcdef01-2345-6789-abcd-ef0123456789"),
            }),
            expected_version: 1,
            // The certificate the request did not name is left in place.
            expected_config: Some(classic_cert.clone()),
            expected_routing: Some(failover(
                "us-west-2",
                "abcdef01-2345-6789-abcd-ef0123456789",
            )),
            changed: true,
        },
        UpdateCase {
            case: "a custom domain whose branding alone changes keeps its certificate",
            domain: "custom-branding-only",
            created_config: Some(policy_cert.clone()),
            created_version: None,
            update: json!({ "ManagedLoginVersion": 2 }),
            expected_version: 2,
            expected_config: Some(policy_cert.clone()),
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a custom domain given every updatable member at once",
            domain: "custom-every-member",
            created_config: Some(policy_cert.clone()),
            created_version: Some(1),
            update: json!({
                "ManagedLoginVersion": 2,
                "CustomDomainConfig": {
                    "CertificateArn": RENEWED_CERTIFICATE,
                    "SecurityPolicy": "TLS_V1_3_2025",
                },
                "Routing": failover("eu-west-1", "11111111-2222-3333-4444-555555555555"),
            }),
            expected_version: 2,
            expected_config: Some(json!({
                "CertificateArn": RENEWED_CERTIFICATE,
                "SecurityPolicy": "TLS_V1_3_2025",
            })),
            expected_routing: Some(failover(
                "eu-west-1",
                "11111111-2222-3333-4444-555555555555",
            )),
            changed: true,
        },
        UpdateCase {
            case: "a custom domain given a certificate ARN in another Region, stored as submitted",
            domain: "custom-other-region",
            created_config: Some(classic_cert.clone()),
            created_version: None,
            update: json!({ "CustomDomainConfig": { "CertificateArn": US_WEST_2_CERTIFICATE } }),
            expected_version: 1,
            expected_config: Some(json!({ "CertificateArn": US_WEST_2_CERTIFICATE })),
            expected_routing: None,
            changed: true,
        },
        UpdateCase {
            case: "a custom domain whose routing is replaced with another secondary Region",
            domain: "custom-replaced-routing",
            created_config: Some(classic_cert.clone()),
            created_version: None,
            update: json!({
                "Routing": failover("ap-southeast-2", ""),
            }),
            expected_version: 1,
            expected_config: Some(classic_cert),
            // `HealthCheckIdType`'s modelled length starts at 0, so the empty
            // id is inside the constraint and is stored as submitted.
            expected_routing: Some(failover("ap-southeast-2", "")),
            changed: true,
        },
    ]
}

#[tokio::test]
async fn an_updated_domain_renders_the_members_the_request_set_and_keeps_the_rest() {
    let router = router();

    for case in update_cases() {
        let label = case.case;
        // Each row gets its own pool: a pool holds at most one prefix domain and
        // at most one custom domain.
        let user_pool_id = pool(&router, &format!("update-pool-{}", case.domain)).await;
        let created = ok(
            &router,
            "CreateUserPoolDomain",
            &case.create_body(&user_pool_id),
        )
        .await;
        let created_cloud_front = created.get("CloudFrontDomain").and_then(Value::as_str);
        let created_cloud_front = created_cloud_front.map(str::to_string);

        // The domain is left to reach `ACTIVE` first, so what the update does to
        // the status is what this row observes.
        tokio::time::sleep(PAST_THE_ACTIVATION).await;
        let before = describe(&router, case.domain).await;
        assert_eq!(
            before["Status"],
            json!("ACTIVE"),
            "case {label:?}: the created domain must be ACTIVE before the update, got {before}"
        );

        let body = update_body(&user_pool_id, case.domain, &case.update);
        let updated = ok(&router, "UpdateUserPoolDomain", &body).await;
        assert_eq!(
            updated["ManagedLoginVersion"],
            json!(case.expected_version),
            "case {label:?}: the update must report the branding version in force, got {updated}"
        );
        match &case.expected_routing {
            Some(routing) => assert_eq!(
                updated["Routing"], *routing,
                "case {label:?}: the update must report the routing in force, got {updated}"
            ),
            None => assert!(
                updated.get("Routing").is_none(),
                "case {label:?}: a domain with no routing reports none, got {updated}"
            ),
        }
        // "Amazon Cognito returns this value if you set a custom domain with
        // CustomDomainConfig": the endpoint is the domain's, and an update never
        // moves it.
        assert_eq!(
            updated.get("CloudFrontDomain").and_then(Value::as_str),
            created_cloud_front.as_deref(),
            "case {label:?}: the update must report the domain's own CloudFront FQDN, got {updated}"
        );

        // The round trip: the reader renders the STORED domain.
        let after = describe(&router, case.domain).await;
        assert_eq!(
            after["Domain"],
            json!(case.domain),
            "case {label:?}: an update never changes the domain's name, got {after}"
        );
        assert_eq!(
            after["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: an update never moves the domain to another pool, got {after}"
        );
        assert_eq!(
            after["ManagedLoginVersion"],
            json!(case.expected_version),
            "case {label:?}: the stored branding version must be the updated one, got {after}"
        );
        match &case.expected_config {
            Some(config) => assert_eq!(
                after["CustomDomainConfig"], *config,
                "case {label:?}: the stored custom domain configuration must be the expected one, got {after}"
            ),
            None => assert!(
                after.get("CustomDomainConfig").is_none(),
                "case {label:?}: a prefix domain stores no custom domain configuration, got {after}"
            ),
        }
        match &case.expected_routing {
            Some(routing) => assert_eq!(
                after["Routing"], *routing,
                "case {label:?}: the stored routing must be the expected one, got {after}"
            ),
            None => assert!(
                after.get("Routing").is_none(),
                "case {label:?}: a domain never given routing stores none, got {after}"
            ),
        }
        assert_eq!(
            after.get("CloudFrontDistribution"),
            before.get("CloudFrontDistribution"),
            "case {label:?}: an update never moves the CloudFront endpoint, got {after}"
        );
        let expected_status = if case.changed && case.created_config.is_some() {
            "UPDATING"
        } else {
            "ACTIVE"
        };
        assert_eq!(
            after["Status"],
            json!(expected_status),
            "case {label:?}: a changed custom domain is UPDATING while a prefix domain stays ACTIVE, got {after}"
        );

        // Idempotence: the same request again asks for the state the domain is
        // already in, so it answers the same thing.
        let again = ok(&router, "UpdateUserPoolDomain", &body).await;
        assert_eq!(
            again, updated,
            "case {label:?}: repeating the update must answer the same response, got {again}"
        );
    }
}

#[tokio::test]
async fn an_updated_domain_reports_the_status_its_own_provisioning_is_in() {
    let router = router();
    let user_pool_id = pool(&router, "status-pool").await;

    // Both kinds of domain: a custom domain's own CloudFront distribution has to
    // carry the change before it serves it, while a prefix domain provisions
    // nothing of its own and stays `ACTIVE` through the change.
    for (label, domain, create_extra, after_update) in [
        ("a prefix domain", "status-prefix", json!({}), "ACTIVE"),
        (
            "a custom domain",
            "status-custom",
            json!({ "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE } }),
            "UPDATING",
        ),
    ] {
        let create = update_body(&user_pool_id, domain, &create_extra);
        ok(&router, "CreateUserPoolDomain", &create).await;
        tokio::time::sleep(PAST_THE_ACTIVATION).await;

        let body = update_body(&user_pool_id, domain, &json!({ "ManagedLoginVersion": 2 }));
        ok(&router, "UpdateUserPoolDomain", &body).await;

        let updating = describe(&router, domain).await;
        assert_eq!(
            updating["Status"],
            json!(after_update),
            "case {label:?}: a changed domain reports the status its own provisioning is in, got {updating}"
        );
        // A read never moves the status: two reads at the same instant agree.
        let polled = describe(&router, domain).await;
        assert_eq!(
            polled["Status"], updating["Status"],
            "case {label:?}: reading the domain must not move its status, got {polled}"
        );

        tokio::time::sleep(PAST_THE_ACTIVATION).await;
        let active = describe(&router, domain).await;
        assert_eq!(
            active["Status"],
            json!("ACTIVE"),
            "case {label:?}: a domain is ACTIVE once the clock owes it, got {active}"
        );
        assert_eq!(
            active["ManagedLoginVersion"],
            json!(2),
            "case {label:?}: the updated branding version survives the transition, got {active}"
        );
    }
}

#[tokio::test]
async fn a_request_the_documented_rules_forbid_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    let other_pool = pool(&router, "other-pool").await;
    let lite_pool = pool_in_tier(&router, "lite-pool", Some("LITE")).await;

    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "refusal-prefix" }),
    )
    .await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({
            "UserPoolId": user_pool_id,
            "Domain": "refusal-custom",
            "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE },
        }),
    )
    .await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": lite_pool, "Domain": "lite-prefix" }),
    )
    .await;
    // Taken once every domain has reached `ACTIVE`, so the snapshot a refused
    // update is compared against is not one the clock itself moves.
    tokio::time::sleep(PAST_THE_ACTIVATION).await;
    let before_prefix = describe(&router, "refusal-prefix").await;
    let before_custom = describe(&router, "refusal-custom").await;

    let routing = failover("us-west-2", "abcdef01-2345-6789-abcd-ef0123456789");
    // (case, the request, the status and error code it must answer)
    for (label, body, expected) in [
        (
            "a branding version below the versions the model names",
            json!({ "UserPoolId": user_pool_id, "Domain": "refusal-prefix", "ManagedLoginVersion": 0 }),
            (400, "InvalidParameterException"),
        ),
        (
            "a branding version past the versions the model names",
            json!({ "UserPoolId": user_pool_id, "Domain": "refusal-prefix", "ManagedLoginVersion": 3 }),
            (400, "InvalidParameterException"),
        ),
        (
            "a negative branding version",
            json!({ "UserPoolId": user_pool_id, "Domain": "refusal-custom", "ManagedLoginVersion": -1 }),
            (400, "InvalidParameterException"),
        ),
        (
            "managed login branding on a pool in the Lite feature plan",
            json!({ "UserPoolId": lite_pool, "Domain": "lite-prefix", "ManagedLoginVersion": 2 }),
            (400, "FeatureUnavailableInTierException"),
        ),
        (
            "a custom domain configuration on a prefix domain",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "refusal-prefix",
                "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE },
            }),
            (400, "InvalidParameterException"),
        ),
        (
            "routing on a prefix domain",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "refusal-prefix",
                "Routing": routing.clone(),
            }),
            (400, "InvalidParameterException"),
        ),
        // The pair `UserPoolId` + `Domain` addresses an EXISTING domain record,
        // so a pair that holds no stored domain — whichever half of it is wrong
        // — names a resource the service cannot find, not a bad parameter. A
        // stored pool paired with a domain ANOTHER pool holds is the one case
        // that resolves: the parameters are mismatched rather than unresolvable.
        // The whole class is covered by `update_user_pool_domain_not_found.rs`.
        (
            "a domain the named pool does not hold",
            json!({ "UserPoolId": user_pool_id, "Domain": "never-created", "ManagedLoginVersion": 2 }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a domain another pool holds",
            json!({ "UserPoolId": other_pool, "Domain": "refusal-prefix", "ManagedLoginVersion": 2 }),
            (400, "InvalidParameterException"),
        ),
        (
            "a user pool that does not exist",
            json!({ "UserPoolId": "us-east-1_absent", "Domain": "refusal-prefix" }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "an absent pool whose other members are invalid too",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Domain": "refusal-prefix",
                "ManagedLoginVersion": 3,
            }),
            (400, "ResourceNotFoundException"),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateUserPoolDomain", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Every refusal left the stored domains exactly as they were, status
    // included: a refused update changes nothing.
    assert_eq!(
        describe(&router, "refusal-prefix").await,
        before_prefix,
        "a refused update must not change the stored prefix domain"
    );
    assert_eq!(
        describe(&router, "refusal-custom").await,
        before_custom,
        "a refused update must not change the stored custom domain"
    );
    // The domain a refused update named was not created by it either.
    let (status, payload) = call(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "never-created" }),
    )
    .await;
    assert_eq!(
        (status, payload["DomainDescription"].clone()),
        (200, json!({})),
        "a refused update stores no domain, got {payload}"
    );

    // Hosted UI (classic) branding IS available in the Lite feature plan.
    let updated = ok(
        &router,
        "UpdateUserPoolDomain",
        &json!({ "UserPoolId": lite_pool, "Domain": "lite-prefix", "ManagedLoginVersion": 1 }),
    )
    .await;
    assert_eq!(
        updated["ManagedLoginVersion"],
        json!(1),
        "hosted UI (classic) branding needs no feature plan, got {updated}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": user_pool_id, "Domain": "constraint-prefix" }),
    )
    .await;

    // One value past each published bound of `UpdateUserPoolDomain`'s input
    // (`smithy_query(query="operation",operation="UpdateUserPoolDomain")`): the
    // domain's length 1..63 and its pattern
    // `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, the user pool id's length 1..55
    // and pattern `^[\w-]+_[0-9a-zA-Z]+$`, and the certificate ARN's length
    // 20..2048.
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
            json!({ "UserPoolId": user_pool_id, "Domain": "ConstraintPrefix" }),
        ),
        (
            "a domain carrying dots, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Domain": "auth.example.com" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Domain": "constraint-prefix" }),
        ),
        (
            "a certificate ARN below the length minimum 20",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "constraint-prefix",
                "CustomDomainConfig": { "CertificateArn": "arn:aws:acm:us-e" },
            }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateUserPoolDomain", &body).await;
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

// SPDX-License-Identifier: CC0-1.0

//! User pool domains over the wire: `CreateUserPoolDomain` stores the domain on
//! the pool it names, and every later read — `DescribeUserPoolDomain` and
//! `DescribeUserPool` — renders the STORED domain. Driven through the router
//! exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created domain is keyed by its `Domain` string alone
//! (`DescribeUserPoolDomain` models no `UserPoolId`), it carries the
//! configuration the request set and nothing it did not, a `CustomDomainConfig`
//! makes it a custom domain whose CloudFront endpoint the create hands to the
//! caller while a prefix domain's is the service's own, a custom domain comes up
//! `CREATING` and reaches `ACTIVE` only once the clock makes that transition due
//! while a prefix domain is `ACTIVE` from the create, and a domain string any
//! pool already holds —
//! or a second domain of the same kind on one pool — is refused with the stored
//! domain untouched. Each table row names the case it asserts.
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

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use cognito_idp::state::DOMAIN_ACTIVATION_SPAN;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Long enough that the clock has made a created domain's activation due.
const PAST_THE_ACTIVATION: Duration = DOMAIN_ACTIVATION_SPAN.saturating_add(Duration::from_millis(
    250,
));

/// An ACM certificate ARN in the Region `CreateUserPoolDomain` names for a
/// custom domain ("you must provide an SSL certificate in the US East (N.
/// Virginia) Amazon Web Services Region").
const US_EAST_1_CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// An ACM certificate ARN in another Region, which pins that the submitted
/// configuration is STORED and rendered back rather than rewritten.
const US_WEST_2_CERTIFICATE: &str =
    "arn:aws:acm:us-west-2:000000000000:certificate/66666666-7777-8888-9999-aaaaaaaaaaaa";

/// The canonical account id every emulated resource reports.
const ACCOUNT_ID: &str = "000000000000";

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
    pool_in_tier(router, name, None).await
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

/// One row of the `CreateUserPoolDomain` table. The rows differ along what the
/// rule generalises over: the kind of domain (prefix or custom), which optional
/// members the request sets, the branding version, and the length and form of
/// the domain string.
struct DomainCase {
    case: &'static str,
    domain: String,
    managed_login_version: Option<i32>,
    custom_domain_config: Option<Value>,
    routing: Option<Value>,
}

impl DomainCase {
    /// This case's `CreateUserPoolDomain` request for `user_pool_id`: an
    /// optional member the case leaves unset is left OUT of the request.
    fn request(&self, user_pool_id: &str) -> Value {
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

    /// Whether this row creates a CUSTOM domain, which is exactly "the request
    /// carries a `CustomDomainConfig`".
    fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }

    /// The branding version the stored domain must carry: the one the request
    /// set, else the hosted UI (classic) version `1`.
    fn expected_version(&self) -> i32 {
        self.managed_login_version.unwrap_or(1)
    }
}

/// The table. `Domain` spans its modelled length 1..63 and the digits, letters
/// and hyphens its pattern admits; `ManagedLoginVersion` spans both branding
/// versions the model names (`1` hosted UI classic, `2` managed login);
/// `CustomDomainConfig` spans a certificate alone, a certificate with each end
/// of the modelled `SecurityPolicyType` list, and a certificate ARN naming
/// another Region (which is stored as submitted); `Routing` appears on a custom
/// domain, the only kind the model supports it for. Every constraint from
/// `smithy_query(query="operation",operation="CreateUserPoolDomain")` and
/// `smithy_query(query="shape",shape="CustomDomainConfigType,SecurityPolicyType")`.
fn domain_cases() -> Vec<DomainCase> {
    vec![
        DomainCase {
            case: "a prefix domain with every optional member left unset",
            domain: "auth-prefix".to_string(),
            managed_login_version: None,
            custom_domain_config: None,
            routing: None,
        },
        DomainCase {
            case: "a prefix domain on hosted UI (classic) branding",
            domain: "classic-branding".to_string(),
            managed_login_version: Some(1),
            custom_domain_config: None,
            routing: None,
        },
        DomainCase {
            case: "a prefix domain on managed login branding",
            domain: "managed-login".to_string(),
            managed_login_version: Some(2),
            custom_domain_config: None,
            routing: None,
        },
        DomainCase {
            case: "a prefix domain of one character, the length minimum",
            domain: "a".to_string(),
            managed_login_version: None,
            custom_domain_config: None,
            routing: None,
        },
        DomainCase {
            case: "a prefix domain of 63 characters, the length maximum",
            domain: format!("d{}9", "x".repeat(61)),
            managed_login_version: Some(2),
            custom_domain_config: None,
            routing: None,
        },
        DomainCase {
            case: "a custom domain with the certificate alone",
            domain: "custom-cert-only".to_string(),
            managed_login_version: None,
            custom_domain_config: Some(json!({ "CertificateArn": US_EAST_1_CERTIFICATE })),
            routing: None,
        },
        DomainCase {
            case: "a custom domain with the oldest security policy",
            domain: "custom-tls-v1".to_string(),
            managed_login_version: Some(1),
            custom_domain_config: Some(json!({
                "CertificateArn": US_EAST_1_CERTIFICATE,
                "SecurityPolicy": "TLS_V1",
            })),
            routing: None,
        },
        DomainCase {
            case: "a custom domain with the newest security policy and a failover routing",
            domain: "custom-tls-v1-3".to_string(),
            managed_login_version: Some(2),
            custom_domain_config: Some(json!({
                "CertificateArn": US_EAST_1_CERTIFICATE,
                "SecurityPolicy": "TLS_V1_3_2025",
            })),
            routing: Some(json!({
                "Failover": {
                    "SecondaryRegion": "us-west-2",
                    "PrimaryRoute53HealthCheckId": "abcdef01-2345-6789-abcd-ef0123456789",
                },
            })),
        },
        DomainCase {
            case: "a custom domain whose certificate ARN names another Region, stored as submitted",
            domain: "custom-other-region".to_string(),
            managed_login_version: None,
            custom_domain_config: Some(json!({
                "CertificateArn": US_WEST_2_CERTIFICATE,
                "SecurityPolicy": "TLS_V1_2_2021",
            })),
            routing: None,
        },
    ]
}

#[tokio::test]
async fn a_created_domain_round_trips_through_stored_state() {
    let router = router();

    for case in domain_cases() {
        let label = case.case;
        // Each row gets its own pool: a pool holds at most one prefix domain
        // and at most one custom domain.
        let user_pool_id = pool(&router, &format!("domain-pool-{}", case.domain)).await;
        let created = ok(
            &router,
            "CreateUserPoolDomain",
            &case.request(&user_pool_id),
        )
        .await;

        assert_eq!(
            created["ManagedLoginVersion"],
            json!(case.expected_version()),
            "case {label:?}: the created domain must report its branding version, got {created}"
        );
        match &case.routing {
            Some(routing) => assert_eq!(
                created["Routing"], *routing,
                "case {label:?}: the created domain must report the routing it was given, got {created}"
            ),
            None => assert!(
                created.get("Routing").is_none(),
                "case {label:?}: a domain created without routing reports none, got {created}"
            ),
        }
        let cloud_front = created.get("CloudFrontDomain").and_then(Value::as_str);
        if case.is_custom() {
            let endpoint = cloud_front.unwrap_or_else(|| {
                panic!("case {label:?}: a custom domain reports its CloudFront FQDN, got {created}")
            });
            assert!(
                endpoint.ends_with(".cloudfront.net"),
                "case {label:?}: expected a CloudFront FQDN, got {endpoint:?}"
            );
        } else {
            assert!(
                cloud_front.is_none(),
                "case {label:?}: a prefix domain reports no CloudFrontDomain, got {created}"
            );
        }

        // The round trip: the reader renders the STORED domain, keyed by the
        // domain string alone.
        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": case.domain }),
        )
        .await;
        let description = &read["DomainDescription"];
        assert_eq!(
            description["Domain"],
            json!(case.domain),
            "case {label:?}: the stored domain string must be the created one, got {read}"
        );
        assert_eq!(
            description["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the domain must name the pool it was created on, got {read}"
        );
        assert_eq!(
            description["AWSAccountId"],
            json!(ACCOUNT_ID),
            "case {label:?}: the domain must report the emulator's account, got {read}"
        );
        assert_eq!(
            description["ManagedLoginVersion"],
            json!(case.expected_version()),
            "case {label:?}: the stored branding version must be the created one, got {read}"
        );
        match &case.custom_domain_config {
            Some(config) => assert_eq!(
                description["CustomDomainConfig"], *config,
                "case {label:?}: the stored custom domain configuration must be the created one, got {read}"
            ),
            None => assert!(
                description.get("CustomDomainConfig").is_none(),
                "case {label:?}: a prefix domain stores no custom domain configuration, got {read}"
            ),
        }
        match &case.routing {
            Some(routing) => assert_eq!(
                description["Routing"], *routing,
                "case {label:?}: the stored routing must be the created one, got {read}"
            ),
            None => assert!(
                description.get("Routing").is_none(),
                "case {label:?}: a domain created without routing stores none, got {read}"
            ),
        }
        // Every hosted domain is served through a CloudFront endpoint, which the
        // description reports; only a CUSTOM domain's is handed to the caller by
        // the create, because only that caller has an alias record to point at
        // it.
        let distribution = description
            .get("CloudFrontDistribution")
            .and_then(Value::as_str);
        assert!(
            distribution.is_some_and(|endpoint| endpoint.ends_with(".cloudfront.net")),
            "case {label:?}: the stored domain must report its CloudFront endpoint, got {read}"
        );
        if case.is_custom() {
            assert_eq!(
                distribution, cloud_front,
                "case {label:?}: the stored CloudFront endpoint must be the one the create reported, got {read}"
            );
        }

        // `DescribeUserPool` renders the same stored domain in the member for
        // its kind, and leaves the other kind's member absent.
        let pool_read = ok(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let user_pool = &pool_read["UserPool"];
        let (set, unset) = if case.is_custom() {
            ("CustomDomain", "Domain")
        } else {
            ("Domain", "CustomDomain")
        };
        assert_eq!(
            user_pool[set],
            json!(case.domain),
            "case {label:?}: the pool must render its {set} as the stored domain, got {pool_read}"
        );
        assert!(
            user_pool.get(unset).is_none(),
            "case {label:?}: the pool holds no {unset}, got {pool_read}"
        );
    }
}

#[tokio::test]
async fn a_created_domain_reports_the_status_its_provisioning_is_in() {
    let router = router();
    let user_pool_id = pool(&router, "status-pool").await;

    // A prefix domain is served by the managed login infrastructure that is
    // already running, so it is usable when the create returns; a custom domain
    // has a CloudFront distribution of its own to provision first.
    for (label, domain, body, created_status) in [
        (
            "a prefix domain",
            "status-prefix",
            json!({ "UserPoolId": user_pool_id, "Domain": "status-prefix" }),
            "ACTIVE",
        ),
        (
            "a custom domain",
            "status-custom",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "status-custom",
                "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE },
            }),
            "CREATING",
        ),
    ] {
        ok(&router, "CreateUserPoolDomain", &body).await;

        // The span of a domain that has provisioning to do is never zero, so a
        // caller reading straight after the create still sees the transient
        // status.
        let read = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            read["DomainDescription"]["Status"],
            json!(created_status),
            "case {label:?}: a created domain reports the status its provisioning is in, got {read}"
        );

        tokio::time::sleep(PAST_THE_ACTIVATION).await;

        let active = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            active["DomainDescription"]["Status"],
            json!("ACTIVE"),
            "case {label:?}: a domain is ACTIVE once the clock owes it, got {active}"
        );

        // A read changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            again["DomainDescription"], active["DomainDescription"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_domain_string_names_at_most_one_pool() {
    let router = router();
    let first = pool(&router, "taken-first").await;
    let second = pool(&router, "taken-second").await;
    ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": first, "Domain": "contested" }),
    )
    .await;

    // (case, the request that must be refused)
    for (label, body) in [
        (
            "the same domain string on another pool",
            json!({ "UserPoolId": second, "Domain": "contested" }),
        ),
        (
            "the same domain string on the pool that holds it",
            json!({ "UserPoolId": first, "Domain": "contested" }),
        ),
        (
            "the same domain string as a custom domain",
            json!({
                "UserPoolId": second,
                "Domain": "contested",
                "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE },
            }),
        ),
        (
            "a second prefix domain on a pool that already has one",
            json!({ "UserPoolId": first, "Domain": "second-prefix" }),
        ),
    ] {
        let (status, payload) = call(&router, "CreateUserPoolDomain", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: must be a 400 InvalidParameterException, got {payload}"
        );
    }

    // The stored domain survived every refusal, and still names its own pool.
    let read = ok(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "contested" }),
    )
    .await;
    assert_eq!(
        read["DomainDescription"]["UserPoolId"],
        json!(first),
        "a refused create must not move the stored domain, got {read}"
    );
    let (status, payload) = call(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "second-prefix" }),
    )
    .await;
    assert_eq!(
        (status, payload["DomainDescription"].clone()),
        (200, json!({})),
        "a refused create stores no domain, got {payload}"
    );

    // A pool that holds a prefix domain can still take its custom domain: the
    // two members are separate.
    let created = ok(
        &router,
        "CreateUserPoolDomain",
        &json!({
            "UserPoolId": first,
            "Domain": "contested-custom",
            "CustomDomainConfig": { "CertificateArn": US_EAST_1_CERTIFICATE },
        }),
    )
    .await;
    assert!(
        created["CloudFrontDomain"].is_string(),
        "a custom domain beside a prefix domain is accepted, got {created}"
    );
}

#[tokio::test]
async fn a_request_the_documented_rules_forbid_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    let lite_pool = pool_in_tier(&router, "lite-pool", Some("LITE")).await;

    // (case, the request, the status and error code it must answer)
    for (label, body, expected) in [
        (
            "a branding version below the versions the model names",
            json!({ "UserPoolId": user_pool_id, "Domain": "version-zero", "ManagedLoginVersion": 0 }),
            (400, "InvalidParameterException"),
        ),
        (
            "a branding version past the versions the model names",
            json!({ "UserPoolId": user_pool_id, "Domain": "version-three", "ManagedLoginVersion": 3 }),
            (400, "InvalidParameterException"),
        ),
        (
            "managed login branding on a pool in the Lite feature plan",
            json!({ "UserPoolId": lite_pool, "Domain": "lite-managed", "ManagedLoginVersion": 2 }),
            (400, "FeatureUnavailableInTierException"),
        ),
        (
            "routing without a custom domain configuration",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "prefix-routing",
                "Routing": {
                    "Failover": {
                        "SecondaryRegion": "us-west-2",
                        "PrimaryRoute53HealthCheckId": "abcdef01-2345-6789-abcd-ef0123456789",
                    },
                },
            }),
            (400, "InvalidParameterException"),
        ),
        (
            "a domain on a user pool that does not exist",
            json!({ "UserPoolId": "us-east-1_absent", "Domain": "orphan-domain" }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a domain on an absent pool whose other members are invalid too",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Domain": "orphan-bad-version",
                "ManagedLoginVersion": 3,
            }),
            (400, "ResourceNotFoundException"),
        ),
    ] {
        let (status, payload) = call(&router, "CreateUserPoolDomain", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // None of the refused requests stored a domain, and the lite pool is
    // unchanged.
    for (label, domain) in [
        ("the version-zero request", "version-zero"),
        ("the version-three request", "version-three"),
        ("the lite-tier request", "lite-managed"),
        ("the prefix-routing request", "prefix-routing"),
        ("the absent-pool request", "orphan-domain"),
        ("the absent-pool request with a bad version", "orphan-bad-version"),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            (status, payload["DomainDescription"].clone()),
            (200, json!({})),
            "case {label:?}: a refused create stores no domain, got {payload}"
        );
    }

    // Hosted UI (classic) branding IS available in the Lite feature plan.
    let created = ok(
        &router,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": lite_pool, "Domain": "lite-classic", "ManagedLoginVersion": 1 }),
    )
    .await;
    assert_eq!(
        created["ManagedLoginVersion"],
        json!(1),
        "hosted UI (classic) branding needs no feature plan, got {created}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;

    // One value past each published bound of `CreateUserPoolDomain`'s input
    // (`smithy_query(query="operation",operation="CreateUserPoolDomain")`): the
    // domain's length 1..63 and its pattern
    // `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, the user pool id's pattern, and
    // the certificate ARN's length 20..2048.
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
            json!({ "UserPoolId": user_pool_id, "Domain": "AuthPrefix" }),
        ),
        (
            "a domain ending in a hyphen, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Domain": "auth-" }),
        ),
        (
            "a domain carrying dots, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Domain": "auth.example.com" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Domain": "patternless-pool" }),
        ),
        (
            "a certificate ARN below the length minimum 20",
            json!({
                "UserPoolId": user_pool_id,
                "Domain": "short-cert",
                "CustomDomainConfig": { "CertificateArn": "arn:aws:acm:us-e" },
            }),
        ),
    ] {
        let (status, payload) = call(&router, "CreateUserPoolDomain", &body).await;
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

    // The reader, which names a domain alone, refuses a domain outside the
    // modelled LENGTH the same way.
    for (label, domain) in [
        ("an empty domain", String::new()),
        ("a domain of 64 characters", "d".repeat(64)),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: DescribeUserPoolDomain refuses it too, got {status} with {payload}"
        );
    }

    // The FORM of the name is a different matter for the reader: a name no
    // create could take holds no domain, so the reader reports the domain it
    // found (none) rather than refusing the name.
    // `input_validation_refusal_wording.rs` asserts that over a set of such
    // names, beside the prose the operations that ask FOR a name answer.
    let (status, payload) = call(
        &router,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "auth.example.com" }),
    )
    .await;
    assert_eq!(
        (status, &payload["DomainDescription"]),
        (200, &json!({})),
        "case \"a domain carrying dots\": the reader reports no domain for it, got {status} \
         with {payload}"
    );
}

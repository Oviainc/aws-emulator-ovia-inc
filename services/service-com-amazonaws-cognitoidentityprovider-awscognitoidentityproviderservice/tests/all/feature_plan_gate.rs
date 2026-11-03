// SPDX-License-Identifier: CC0-1.0

//! The user pool FEATURE PLAN boundary over the wire, driven through the router
//! exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: an operation that configures a user pool refuses any
//! request member whose feature the pool's stored `UserPoolTier` does not
//! include — HTTP 400, error code `FeatureUnavailableInTierException`, message
//! "The following feature is not available for the <tier> pricing tier
//! configured: <feature>" with the pool's stored tier and that feature's name —
//! and persists NOTHING; a pool on the cheapest plan that includes the feature,
//! or on any plan above it, is served.
//!
//! The plans are ordered by inclusion (`LITE`, `ESSENTIALS`, `PLUS` —
//! `smithy_query(query="shape",shape="UserPoolTierType")`), so each gated
//! feature has one floor and every plan from the floor upwards serves it. Each
//! table row names its case and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service, and
/// its Region is the one a created pool is primary in. The signature is never
/// checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A Region other than the one above, so a replica has somewhere to live.
const SECONDARY_REGION: &str = "us-west-2";

/// The CloudWatch Logs group an activity-log entry is delivered to.
const LOG_GROUP_ARN: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/plan-gate";

/// The modelled feature plans in INCLUSION order, cheapest first
/// (`smithy_query(query="shape",shape="UserPoolTierType")` names the three, and
/// `EmailMfaConfigType`'s "the Essentials tier or higher" is what orders them).
const TIERS_IN_ORDER: [&str; 3] = ["LITE", "ESSENTIALS", "PLUS"];

/// The plan a `CreateUserPool` request that names none carries: "Defaults to
/// `ESSENTIALS`" (`CreateUserPool.UserPoolTier`).
const DEFAULT_TIER: &str = "ESSENTIALS";

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

/// A pool named `name` on `tier`, or on the service default when `tier` is
/// `None`, and its minted id.
///
/// Every pool of this suite is encrypted under a customer-managed key, so that
/// what a gated feature's request is refused for is the pool's PLAN alone:
/// replication additionally needs that key type — "you must use a multi-region
/// key to enable multi-region replication for a user pool"
/// (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
async fn pool_in_tier(router: &MultiServiceRouter, name: &str, tier: Option<&str>) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    body.insert(
        "KeyConfiguration".to_string(),
        json!({ "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": MULTI_REGION_KEY_ARN }),
    );
    if let Some(tier) = tier {
        body.insert("UserPoolTier".to_string(), json!(tier));
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string()
}

/// A `KeyConfiguration.KmsKeyArn`-shaped ARN (`EncryptionKeyArnType`, len
/// 20..2048) standing for the multi-Region customer-managed key a replicated
/// pool is encrypted under.
const MULTI_REGION_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/multi-region-example";

/// One plan-gated feature: the request that asks for it, the cheapest plan that
/// includes it, the name a refusal gives it, and the read that shows whether
/// the pool kept anything.
struct GatedFeature {
    /// The case this row is named by.
    case: &'static str,
    /// The cheapest `UserPoolTierType` value whose plan includes this feature.
    floor: &'static str,
    /// The feature's display name, in the tail of the refusal message.
    feature: &'static str,
    /// The operation that configures the feature.
    operation: &'static str,
    /// Its request, built for the pool under test and a per-row unique slug.
    request: fn(&str, &str) -> Value,
    /// The reporting operation whose answer shows what the pool holds.
    reader: &'static str,
    /// The reader's request for the pool under test.
    read: fn(&str) -> Value,
}

/// The table: every feature this service gates on the pool's plan, with the
/// floor each one's own documentation names.
const GATED_FEATURES: [GatedFeature; 4] = [
    GatedFeature {
        // `EmailMfaConfigType`: "To activate this setting, your user pool must
        // be in the Essentials tier or higher."
        case: "email message MFA through SetUserPoolMfaConfig",
        floor: "ESSENTIALS",
        feature: "Email MFA",
        operation: "SetUserPoolMfaConfig",
        request: |user_pool_id, _slug| {
            json!({
                "UserPoolId": user_pool_id,
                "MfaConfiguration": "OPTIONAL",
                "EmailMfaConfiguration": {
                    "Message": "Your verification code is {####}",
                    "Subject": "Your verification code",
                },
            })
        },
        reader: "GetUserPoolMfaConfig",
        read: |user_pool_id| json!({ "UserPoolId": user_pool_id }),
    },
    GatedFeature {
        // `CreateUserPoolDomain.ManagedLoginVersion`: "Managed login requires
        // that your user pool be configured for any feature plan other than
        // `Lite`."
        case: "managed login branding through CreateUserPoolDomain",
        floor: "ESSENTIALS",
        feature: "Managed Login Version 2",
        operation: "CreateUserPoolDomain",
        request: |user_pool_id, slug| {
            json!({
                "UserPoolId": user_pool_id,
                "Domain": format!("plan-gate-{slug}"),
                "ManagedLoginVersion": 2,
            })
        },
        reader: "DescribeUserPool",
        read: |user_pool_id| json!({ "UserPoolId": user_pool_id }),
    },
    GatedFeature {
        // `LogConfigurationType.EventSource`: "To send info-level logs about
        // threat-protection user activity in user pools with the Plus feature
        // plan, set to `userAuthEvents`."
        case: "activity-log streaming through SetLogDeliveryConfiguration",
        floor: "PLUS",
        feature: "Log Streaming",
        operation: "SetLogDeliveryConfiguration",
        request: |user_pool_id, _slug| {
            json!({
                "UserPoolId": user_pool_id,
                "LogConfigurations": [{
                    "LogLevel": "INFO",
                    "EventSource": "userAuthEvents",
                    "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP_ARN },
                }],
            })
        },
        reader: "GetLogDeliveryConfiguration",
        read: |user_pool_id| json!({ "UserPoolId": user_pool_id }),
    },
    GatedFeature {
        // `CreateUserPoolReplica` adds a Region to a user pool, which
        // `UserPoolReplicaType` reports; a pool on the cheapest plan is not
        // replicated to a second Region.
        case: "multi-Region replication through CreateUserPoolReplica",
        floor: "ESSENTIALS",
        feature: "Multi-Region Replication",
        operation: "CreateUserPoolReplica",
        request: |user_pool_id, _slug| {
            json!({ "UserPoolId": user_pool_id, "RegionName": SECONDARY_REGION })
        },
        reader: "ListUserPoolReplicas",
        read: |user_pool_id| json!({ "UserPoolId": user_pool_id }),
    },
];

/// Whether `tier` includes a feature whose floor is `floor`: the plans are
/// ordered by inclusion, so every plan from the floor upwards includes it.
fn plan_includes(tier: &str, floor: &str) -> bool {
    let rank = |plan: &str| {
        TIERS_IN_ORDER
            .iter()
            .position(|known| *known == plan)
            .unwrap_or_else(|| panic!("{plan} is one of the modelled plans"))
    };
    rank(tier) >= rank(floor)
}

/// The refusal a pool on `tier` owes a request for `feature`.
fn tier_refusal_message(tier: &str, feature: &str) -> String {
    format!("The following feature is not available for the {tier} pricing tier configured: {feature}")
}

#[tokio::test]
async fn a_feature_the_pools_plan_excludes_is_refused_and_persists_nothing() {
    let router = router();

    // Every modelled plan, plus the pool that names none and so carries the
    // documented default — the rows differ along the plan the pool is on and
    // along which feature the request asks for.
    let plans: Vec<(String, Option<&str>, &str)> = TIERS_IN_ORDER
        .iter()
        .map(|tier| (format!("the {tier} plan"), Some(*tier), *tier))
        .chain(std::iter::once((
            "the default plan".to_string(),
            None,
            DEFAULT_TIER,
        )))
        .collect();

    for (index, row) in GATED_FEATURES.iter().enumerate() {
        for (plan_case, requested, effective) in &plans {
            let case = format!("{} on a pool in {plan_case}", row.case);
            let slug = format!("{index}-{}", requested.unwrap_or("default").to_lowercase());
            let user_pool_id =
                pool_in_tier(&router, &format!("plan-gate-pool-{slug}"), *requested).await;

            // What the pool holds BEFORE the call, read through the reporting
            // operation that serves the same configuration afterwards.
            let before = ok(&router, row.reader, &(row.read)(&user_pool_id)).await;

            let (status, payload) =
                call(&router, row.operation, &(row.request)(&user_pool_id, &slug)).await;
            let after = ok(&router, row.reader, &(row.read)(&user_pool_id)).await;

            if plan_includes(effective, row.floor) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the plan includes this feature, got {status} with {payload}"
                );
                // The accepted path is a write: the pool reports something it
                // did not report before the call.
                assert_ne!(
                    after, before,
                    "case {case:?}: an accepted configuration must change what the pool \
                     reports, got {after}"
                );
                continue;
            }

            assert_eq!(
                (status, error_code(&payload)),
                (400, "FeatureUnavailableInTierException"),
                "case {case:?}: expected a 400 FeatureUnavailableInTierException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            let expected = tier_refusal_message(effective, row.feature);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The refusal reports the plan the POOL is on, which is the stored
            // one rather than anything the request carried.
            assert!(
                message.contains(effective),
                "case {case:?}: the refusal must name the pool's stored plan {effective:?}, \
                 got {message:?}"
            );
            // A refused configuration is not a configuration: the pool reports
            // exactly what it reported before the call.
            assert_eq!(
                after, before,
                "case {case:?}: a refused configuration must leave the pool unchanged, got \
                 {after}"
            );
        }
    }
}

#[tokio::test]
async fn email_mfa_is_served_from_the_floor_plan_upwards_and_refused_below_it() {
    let router = router();

    // The one feature the recorded divergence was filed about, asserted end to
    // end: the configuration lands on a plan that includes it and is readable
    // back, and the pool below the floor neither stores it nor reports it.
    for (plan_case, requested, effective) in [
        ("LITE", Some("LITE"), "LITE"),
        ("ESSENTIALS", Some("ESSENTIALS"), "ESSENTIALS"),
        ("PLUS", Some("PLUS"), "PLUS"),
        ("the default plan", None, DEFAULT_TIER),
    ] {
        let slug = requested.unwrap_or("default").to_lowercase();
        let user_pool_id = pool_in_tier(&router, &format!("email-mfa-{slug}"), requested).await;

        // Switching MFA off configures no factor at all, so it is served on
        // every plan — the control that keeps the gate on the member rather
        // than on the operation.
        let case = format!("MfaConfiguration OFF on a pool in {plan_case}");
        let answered = ok(
            &router,
            "SetUserPoolMfaConfig",
            &json!({ "UserPoolId": user_pool_id, "MfaConfiguration": "OFF" }),
        )
        .await;
        assert_eq!(
            answered["MfaConfiguration"],
            json!("OFF"),
            "case {case:?}: configuring no factor is served on every plan, got {answered}"
        );

        let case = format!("EmailMfaConfiguration on a pool in {plan_case}");
        let configuration = json!({
            "Message": "Your one-time code is {####}",
            "Subject": "Your one-time code",
        });
        let (status, payload) = call(
            &router,
            "SetUserPoolMfaConfig",
            &json!({
                "UserPoolId": user_pool_id,
                "MfaConfiguration": "ON",
                "EmailMfaConfiguration": configuration,
            }),
        )
        .await;
        let read_back = ok(
            &router,
            "GetUserPoolMfaConfig",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;

        if plan_includes(effective, "ESSENTIALS") {
            assert_eq!(
                status, 200,
                "case {case:?}: the plan includes email MFA, got {status} with {payload}"
            );
            assert_eq!(
                read_back["EmailMfaConfiguration"], configuration,
                "case {case:?}: the stored configuration must read back, got {read_back}"
            );
            assert_eq!(
                read_back["MfaConfiguration"],
                json!("ON"),
                "case {case:?}: the mode the call set must read back, got {read_back}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&payload)),
            (400, "FeatureUnavailableInTierException"),
            "case {case:?}: expected a 400 FeatureUnavailableInTierException, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            tier_refusal_message(effective, "Email MFA"),
            "case {case:?}: expected the tier template naming Email MFA, got {payload}"
        );
        assert_eq!(
            read_back.get("EmailMfaConfiguration"),
            None,
            "case {case:?}: a refused factor must not be configured on the pool, got {read_back}"
        );
        // The mode the earlier accepted call left the pool in is untouched too:
        // a refusal writes nothing, not even the members its plan allows.
        assert_eq!(
            read_back["MfaConfiguration"],
            json!("OFF"),
            "case {case:?}: a refusal must leave the stored mode alone, got {read_back}"
        );
    }
}

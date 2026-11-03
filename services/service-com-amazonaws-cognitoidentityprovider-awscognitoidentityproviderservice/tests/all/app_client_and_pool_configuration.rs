// SPDX-License-Identifier: CC0-1.0

//! App-client and user-pool CONFIGURATION validation over the wire, driven
//! through the router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`), plus the
//! non-modelled CSV upload target a user import job publishes.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded request:
//!
//! * a configuration member whose value set this service enumerates is refused
//!   when the request asks for a combination outside it:
//!   `AllowedOAuthFlows` may not combine the machine-to-machine grant with a
//!   user-facing one (`InvalidOAuthFlowException`), `AllowedOAuthScopes` names
//!   only scopes the pool has a definition of (`ScopeDoesNotExistException`),
//!   and a `RegionName` names an AWS Region (`InvalidParameterException`);
//! * a member whose documentation names a feature plan is refused on a pool
//!   whose CONFIGURED plan is below it (`FeatureUnavailableInTierException`),
//!   and served on every plan from that floor upwards — and a create that
//!   configures no plan at all is served on the cheapest plan that includes it;
//! * a refusal persists NOTHING: the resource it was about reads back exactly as
//!   it was;
//! * a user import job starts only once the CSV file has been uploaded to the
//!   `PreSignedUrl` it published, and `GetCSVHeader` reports one column per
//!   attribute of the pool's schema.
//!
//! Each table row names its case and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service, and its
/// Region is the one a created pool is primary in. The signature is never checked
/// here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The endpoint host the suite's requests address, which a locator this service
/// hands out has to name.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The Region the requests are signed for, so the Region every created pool is
/// primary in.
const PRIMARY_REGION: &str = "us-east-1";

/// A CloudWatch Logs role a user import job logs its results to.
const IMPORT_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

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
        .header("host", ENDPOINT_HOST)
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

/// A pool named `name` on `tier` — or on the service default when `tier` is
/// `None` — and its minted id.
async fn pool_in_tier(router: &MultiServiceRouter, name: &str, tier: Option<&str>) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(tier) = tier {
        body.insert("UserPoolTier".to_string(), json!(tier));
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string()
}

/// A pool on the service default plan, and its minted id.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    pool_in_tier(router, name, None).await
}

/// A `KeyConfiguration.KmsKeyArn`-shaped ARN (`EncryptionKeyArnType`, len
/// 20..2048) standing for the multi-Region customer-managed key a replicated
/// pool is encrypted under.
const MULTI_REGION_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/multi-region-example";

/// A pool on `tier` that can be REPLICATED, and its minted id: it is encrypted
/// under a customer-managed key, which is what a pool has to hold to be
/// replicated at all — "you must use a multi-region key to enable multi-region
/// replication for a user pool"
/// (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
async fn replicable_pool_in_tier(router: &MultiServiceRouter, name: &str, tier: &str) -> String {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": name,
            "UserPoolTier": tier,
            "KeyConfiguration": {
                "KeyType": "CUSTOMER_MANAGED_KEY",
                "KmsKeyArn": MULTI_REGION_KEY_ARN,
            },
        }),
    )
    .await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string()
}

/// An app client of `user_pool_id` carrying `members` beside its name, and the
/// create's answer.
async fn create_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    members: &Value,
) -> (u16, Value) {
    let mut body = members
        .as_object()
        .cloned()
        .expect("the members are a JSON object");
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientName".to_string(), json!(name));
    call(router, "CreateUserPoolClient", &Value::Object(body)).await
}

/// The app clients of `user_pool_id`, as `ListUserPoolClients` reports them.
async fn clients(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<Value> {
    let listed = ok(
        router,
        "ListUserPoolClients",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    listed["UserPoolClients"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The stored app client, as `DescribeUserPoolClient` renders it.
async fn described_client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
) -> Value {
    ok(
        router,
        "DescribeUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await["UserPoolClient"]
        .clone()
}

/// The stored pool, as `DescribeUserPool` renders it.
async fn described_pool(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

// ---------------------------------------------------------------------------
// AllowedOAuthFlows combinations
// ---------------------------------------------------------------------------

/// Every `OAuthFlowType` value (`smithy_query(query="shape",shape="OAuthFlowType")`:
/// code | implicit | client_credentials) and the machine-to-machine one among
/// them.
const OAUTH_FLOWS: [&str; 3] = ["code", "implicit", "client_credentials"];

/// The message a refused `AllowedOAuthFlows` combination carries.
const INVALID_OAUTH_FLOW_MESSAGE: &str =
    "client_credentials flow can not be selected along with code flow or implicit flow.";

/// Whether `flows` is a combination an app client can hold: the
/// machine-to-machine grant has to be "the only allowed OAuth flow"
/// (`CreateUserPoolClient.AllowedOAuthFlows`), so it may not appear beside a
/// user-facing grant. A list that merely repeats one flow names no other.
fn flows_are_acceptable(flows: &[&str]) -> bool {
    let names_client_credentials = flows.contains(&"client_credentials");
    let names_a_user_grant = flows.iter().any(|flow| *flow != "client_credentials");
    !(names_client_credentials && names_a_user_grant)
}

/// Every subset of [`OAUTH_FLOWS`] the modelled list length 0..3 admits, beside
/// a list that repeats one flow — the rows differ along which grants are asked
/// for, how many, and in which order.
fn oauth_flow_rows() -> Vec<(String, Vec<&'static str>)> {
    let mut rows: Vec<(String, Vec<&'static str>)> = Vec::new();
    for mask in 0u8..8 {
        let flows: Vec<&'static str> = OAUTH_FLOWS
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, flow)| *flow)
            .collect();
        rows.push((format!("the flows {flows:?}"), flows));
    }
    // Order is the caller's, and a repeat is not a second grant.
    rows.push((
        "the machine-to-machine grant before a user grant".to_string(),
        vec!["client_credentials", "code"],
    ));
    rows.push((
        "the machine-to-machine grant twice".to_string(),
        vec!["client_credentials", "client_credentials"],
    ));
    rows
}

#[tokio::test]
async fn an_oauth_flow_combination_no_app_client_can_hold_is_refused_by_create_and_update() {
    let router = router();
    let user_pool_id = pool(&router, "oauth-flow-pool").await;

    // The app client every update row is applied to, created with a combination
    // the service accepts.
    let standing = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "standing",
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthFlows": ["code"],
        }),
    )
    .await["UserPoolClient"]
        .clone();
    let standing_id = standing["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    for (case, flows) in oauth_flow_rows() {
        let before = clients(&router, &user_pool_id).await.len();
        let (status, payload) = create_client(
            &router,
            &user_pool_id,
            "oauth-flow-client",
            &json!({
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthFlows": flows,
            }),
        )
        .await;
        let after = clients(&router, &user_pool_id).await.len();

        if flows_are_acceptable(&flows) {
            assert_eq!(
                status, 200,
                "case {case:?}: the combination is one an app client can hold, got {status} with \
                 {payload}"
            );
            let client_id = payload["UserPoolClient"]["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted client id: {payload}"))
                .to_string();
            // The STORED client carries the flows as submitted, repeats
            // included: the answer is what a later read renders.
            let read = described_client(&router, &user_pool_id, &client_id).await;
            assert_eq!(
                read["AllowedOAuthFlows"],
                json!(flows),
                "case {case:?}: the stored client must carry the flows the create set, got {read}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: an accepted create adds one app client, got {after}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidOAuthFlowException"),
            "case {case:?}: expected a 400 InvalidOAuthFlowException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, INVALID_OAUTH_FLOW_MESSAGE,
            "case {case:?}: expected {INVALID_OAUTH_FLOW_MESSAGE:?}, got {message:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no app client, got {after}"
        );

        // The same combination is refused through the update, and the standing
        // app client keeps the configuration it had.
        let was = described_client(&router, &user_pool_id, &standing_id).await;
        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": standing_id,
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthFlows": flows,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidOAuthFlowException", INVALID_OAUTH_FLOW_MESSAGE),
            "case {case:?}: the update owes the same refusal, got {status} with {payload}"
        );
        let now = described_client(&router, &user_pool_id, &standing_id).await;
        assert_eq!(
            now, was,
            "case {case:?}: a refused update must leave the app client alone, got {now}"
        );
    }
}

// ---------------------------------------------------------------------------
// AllowedOAuthScopes
// ---------------------------------------------------------------------------

/// The identifier of the resource server every scope row resolves against.
const RESOURCE_SERVER: &str = "https://api.example.com";

/// A custom scope that resource server declares.
const DECLARED_SCOPE: &str = "widgets.read";

/// The scopes every pool authorizes without a resource server of its own:
/// "Scope values include `phone`, `email`, `openid`, and `profile`. The
/// `aws.cognito.signin.user.admin` scope authorizes user self-service
/// operations" (`CreateUserPoolClient.AllowedOAuthScopes` in this service's
/// model).
const STANDARD_SCOPES: [&str; 5] = [
    "phone",
    "email",
    "openid",
    "profile",
    "aws.cognito.signin.user.admin",
];

/// The message a refused scope carries, which names the scope asked for.
fn invalid_scope_message(scope: &str) -> String {
    format!("Invalid scope requested: {scope}")
}

/// One scope row: the scopes a request asks for and the first one of them the
/// pool has no definition of, if any.
struct ScopeCase {
    case: String,
    scopes: Vec<String>,
    /// The scope the refusal has to name, or `None` for a row the pool serves.
    undefined: Option<String>,
}

/// The rows: every standard scope on its own, the declared custom scope, the two
/// kinds together, and scopes no resource server of the pool defines — a scope
/// name the declared resource server does not carry, a scope of a resource
/// server that does not exist, and a bad scope hidden among good ones.
fn scope_cases() -> Vec<ScopeCase> {
    let mut rows: Vec<ScopeCase> = STANDARD_SCOPES
        .iter()
        .map(|scope| ScopeCase {
            case: format!("the standard scope {scope:?}"),
            scopes: vec![(*scope).to_string()],
            undefined: None,
        })
        .collect();
    let declared = format!("{RESOURCE_SERVER}/{DECLARED_SCOPE}");
    rows.push(ScopeCase {
        case: "a custom scope the pool's resource server declares".to_string(),
        scopes: vec![declared.clone()],
        undefined: None,
    });
    rows.push(ScopeCase {
        case: "every standard scope beside the declared custom one".to_string(),
        scopes: STANDARD_SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .chain(std::iter::once(declared.clone()))
            .collect(),
        undefined: None,
    });
    rows.push(ScopeCase {
        case: "the empty scope list".to_string(),
        scopes: Vec::new(),
        undefined: None,
    });
    let unknown_name = format!("{RESOURCE_SERVER}/nonexistent.scope");
    rows.push(ScopeCase {
        case: "a scope name the declared resource server does not carry".to_string(),
        scopes: vec![unknown_name.clone()],
        undefined: Some(unknown_name),
    });
    let unknown_server = "https://no-such-rs.example.com/no-such-scope".to_string();
    rows.push(ScopeCase {
        case: "a scope of a resource server the pool does not hold".to_string(),
        scopes: vec![unknown_server.clone()],
        undefined: Some(unknown_server),
    });
    let misspelled = "openid.profile".to_string();
    rows.push(ScopeCase {
        case: "a value that is neither a standard scope nor a custom one".to_string(),
        scopes: vec![misspelled.clone()],
        undefined: Some(misspelled),
    });
    let hidden = format!("{RESOURCE_SERVER}/widgets.write");
    rows.push(ScopeCase {
        case: "an undefined scope among scopes the pool does define".to_string(),
        scopes: vec!["openid".to_string(), hidden.clone(), declared],
        undefined: Some(hidden),
    });
    rows
}

#[tokio::test]
async fn a_scope_the_pool_has_no_definition_of_is_refused_by_create_and_update() {
    let router = router();
    let user_pool_id = pool(&router, "scope-pool").await;
    ok(
        &router,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": RESOURCE_SERVER,
            "Name": "widgets",
            "Scopes": [{ "ScopeName": DECLARED_SCOPE, "ScopeDescription": "read widgets" }],
        }),
    )
    .await;

    // The app client every update row is applied to.
    let standing_id = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "standing",
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthScopes": ["openid"],
        }),
    )
    .await["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    for ScopeCase {
        case,
        scopes,
        undefined,
    } in scope_cases()
    {
        let before = clients(&router, &user_pool_id).await.len();
        let (status, payload) = create_client(
            &router,
            &user_pool_id,
            "scope-client",
            &json!({
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthScopes": scopes,
            }),
        )
        .await;
        let after = clients(&router, &user_pool_id).await.len();

        let Some(undefined) = undefined else {
            assert_eq!(
                status, 200,
                "case {case:?}: the pool defines every scope asked for, got {status} with {payload}"
            );
            let client_id = payload["UserPoolClient"]["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted client id: {payload}"))
                .to_string();
            let read = described_client(&router, &user_pool_id, &client_id).await;
            assert_eq!(
                read["AllowedOAuthScopes"],
                json!(scopes),
                "case {case:?}: the stored client must carry the scopes the create set, got {read}"
            );
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: an accepted create adds one app client, got {after}"
            );
            continue;
        };

        let expected = invalid_scope_message(&undefined);
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ScopeDoesNotExistException"),
            "case {case:?}: expected a 400 ScopeDoesNotExistException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no app client, got {after}"
        );

        let was = described_client(&router, &user_pool_id, &standing_id).await;
        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": standing_id,
                "AllowedOAuthFlowsUserPoolClient": true,
                "AllowedOAuthScopes": scopes,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "ScopeDoesNotExistException", expected.as_str()),
            "case {case:?}: the update owes the same refusal, got {status} with {payload}"
        );
        let now = described_client(&router, &user_pool_id, &standing_id).await;
        assert_eq!(
            now, was,
            "case {case:?}: a refused update must leave the app client alone, got {now}"
        );
    }
}

// ---------------------------------------------------------------------------
// RegionName
// ---------------------------------------------------------------------------

/// The message a refused `RegionName` carries.
const INVALID_REGION_MESSAGE: &str = "Invalid region name";

/// Values that look like a Region name but name none: the modelled
/// `RegionNameType` bounds only the length (5..32), so each of these reaches the
/// handler and is judged against the Region catalog.
const NOT_REGIONS: [&str; 5] = [
    "not-a-real-region",
    "us-west-9",
    "eu-west-42",
    "US-WEST-2",
    "us-west-2-extra",
];

/// `operation`'s request for `region` on the pool `user_pool_id`: the Region is
/// what every one of them is addressed by, and `UpdateUserPoolReplica` carries
/// the status it would set beside it.
fn replica_request(operation: &str, user_pool_id: &str, region: &str) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("RegionName".to_string(), json!(region));
    if operation == "UpdateUserPoolReplica" {
        body.insert("Status".to_string(), json!("ACTIVE"));
    }
    Value::Object(body)
}

#[tokio::test]
async fn a_region_name_that_names_no_aws_region_is_refused_by_every_replica_operation() {
    let router = router();
    let user_pool_id = replicable_pool_in_tier(&router, "region-pool", "PLUS").await;

    // Each operation that takes a `RegionName`, with the rest of its request.
    for operation in [
        "CreateUserPoolReplica",
        "UpdateUserPoolReplica",
        "DeleteUserPoolReplica",
    ] {
        for region in NOT_REGIONS {
            let case = format!("{operation} with RegionName {region:?}");
            let before = ok(
                &router,
                "ListUserPoolReplicas",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await;
            let (status, payload) = call(
                &router,
                operation,
                &replica_request(operation, &user_pool_id, region),
            )
            .await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: expected a 400 InvalidParameterException, got {status} with \
                 {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, INVALID_REGION_MESSAGE,
                "case {case:?}: expected {INVALID_REGION_MESSAGE:?}, got {message:?}"
            );
            let after = ok(
                &router,
                "ListUserPoolReplicas",
                &json!({ "UserPoolId": user_pool_id }),
            )
            .await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused call must leave the pool's replicas alone, got {after}"
            );
        }
    }

    // Every Region of the catalog other than the pool's own IS replicated to,
    // including entries the issue did not cite, and the replica reads back.
    for region in cognito_idp::state::AWS_REGIONS {
        if *region == PRIMARY_REGION {
            continue;
        }
        let case = format!("CreateUserPoolReplica in {region:?}");
        let created = ok(
            &router,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
        )
        .await;
        assert_eq!(
            created["UserPoolReplica"]["RegionName"],
            json!(region),
            "case {case:?}: the create answers the replica it stored, got {created}"
        );
        let listed = ok(
            &router,
            "ListUserPoolReplicas",
            &json!({ "UserPoolId": user_pool_id, "MaxResults": 50 }),
        )
        .await;
        assert!(
            listed["UserPoolReplicas"]
                .as_array()
                .is_some_and(|entries| entries
                    .iter()
                    .any(|entry| entry["RegionName"] == json!(region))),
            "case {case:?}: the stored replica must be listed, got {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// The feature-plan gate on a pool's own configuration and on an app client's
// ---------------------------------------------------------------------------

/// Every modelled feature plan, cheapest first
/// (`smithy_query(query="shape",shape="UserPoolTierType")`), beside the pool that
/// names none and carries the documented `ESSENTIALS` default.
const PLANS: [(&str, Option<&str>, &str); 4] = [
    ("the LITE plan", Some("LITE"), "LITE"),
    ("the ESSENTIALS plan", Some("ESSENTIALS"), "ESSENTIALS"),
    ("the PLUS plan", Some("PLUS"), "PLUS"),
    ("the default plan", None, "ESSENTIALS"),
];

/// Whether `tier` includes a feature whose floor is `floor`: the plans are
/// ordered by inclusion, so every plan from the floor upwards includes it.
fn plan_includes(tier: &str, floor: &str) -> bool {
    let rank = |plan: &str| {
        PLANS
            .iter()
            .position(|(_, _, effective)| *effective == plan)
            .unwrap_or_else(|| panic!("{plan} is one of the modelled plans"))
    };
    rank(tier) >= rank(floor)
}

/// The plan a `CreateUserPool` that names none is created on when it configures
/// a feature whose floor is `floor`: the cheapest plan that includes that
/// feature, which is the documented default when the default already includes
/// it.
fn plan_a_create_without_a_plan_lands_on(floor: &'static str) -> &'static str {
    if plan_includes("ESSENTIALS", floor) {
        "ESSENTIALS"
    } else {
        floor
    }
}

/// What a refusal says about a pool configuration whose plan excludes a feature:
/// the request sets the plan and the feature together, so the features that have
/// to be off for that plan are what it names.
fn features_need_disabling_message(tier: &str, feature: &str) -> String {
    format!("The following features need to be disabled for the {tier} pricing tier configured: {feature}")
}

/// What a refusal says about a feature configured on a pool whose plan is
/// already set.
fn feature_unavailable_message(tier: &str, feature: &str) -> String {
    format!("The following feature is not available for the {tier} pricing tier configured: {feature}")
}

/// One plan-gated member of a POOL's own configuration: what a `CreateUserPool`
/// request carries for it, the cheapest plan that includes it, and the feature a
/// refusal names.
struct GatedPoolMember {
    case: &'static str,
    floor: &'static str,
    feature: &'static str,
    members: fn() -> Value,
}

/// The table: each pool configuration member whose own documentation names a
/// feature plan, plus the values of those members that activate nothing and are
/// therefore served on every plan.
const GATED_POOL_MEMBERS: [GatedPoolMember; 3] = [
    GatedPoolMember {
        // `SignInPolicyType`: "To activate this setting, your user pool must be
        // in the Essentials tier or higher."
        case: "a sign-in policy with a passwordless first factor",
        floor: "ESSENTIALS",
        feature: "Choice Based Authentication",
        members: || {
            json!({ "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD", "EMAIL_OTP"] } } })
        },
    },
    GatedPoolMember {
        // The same member asking for passkeys instead, so the rule is not read
        // off one factor.
        case: "a sign-in policy that admits passkeys",
        floor: "ESSENTIALS",
        feature: "Choice Based Authentication",
        members: || {
            json!({ "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["WEB_AUTHN"] } } })
        },
    },
    GatedPoolMember {
        // `UserPoolAddOnsType`: "To activate this setting, your user pool must
        // be on the Plus tier."
        case: "threat protection in the enforced mode",
        floor: "PLUS",
        feature: "Threat Protection",
        members: || json!({ "UserPoolAddOns": { "AdvancedSecurityMode": "ENFORCED" } }),
    },
];

#[tokio::test]
async fn a_pool_configuration_its_plan_excludes_is_refused_at_create_and_update() {
    let router = router();

    for (index, row) in GATED_POOL_MEMBERS.iter().enumerate() {
        for (plan_case, requested, effective) in PLANS {
            let case = format!("{} on a pool in {plan_case}", row.case);
            let mut body = (row.members)()
                .as_object()
                .cloned()
                .expect("the members are a JSON object");
            body.insert(
                "PoolName".to_string(),
                json!(format!("plan-pool-{index}-{effective}")),
            );
            if let Some(requested) = requested {
                body.insert("UserPoolTier".to_string(), json!(requested));
            }
            let pools_before = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            let (status, payload) =
                call(&router, "CreateUserPool", &Value::Object(body.clone())).await;

            // A create that names NO plan configures none, so it is served
            // whatever feature it asks for and the pool is created on the
            // cheapest plan that includes that feature.
            if requested.is_none() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a create that names no plan is served, got {status} with \
                     {payload}"
                );
                let expected = plan_a_create_without_a_plan_lands_on(row.floor);
                assert_eq!(
                    payload["UserPool"]["UserPoolTier"],
                    json!(expected),
                    "case {case:?}: the pool must be created on {expected:?}, got {payload}"
                );
                continue;
            }

            if plan_includes(effective, row.floor) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the plan includes this configuration, got {status} with \
                     {payload}"
                );
                continue;
            }

            let expected = features_need_disabling_message(effective, row.feature);
            assert_eq!(
                (status, error_code(&payload)),
                (400, "FeatureUnavailableInTierException"),
                "case {case:?}: expected a 400 FeatureUnavailableInTierException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            let pools_after = ok(&router, "ListUserPools", &json!({ "MaxResults": 60 })).await;
            assert_eq!(
                pools_after, pools_before,
                "case {case:?}: a refused create stores no pool, got {pools_after}"
            );

            // The same configuration applied through an update is refused too,
            // and leaves the pool exactly as it was.
            let existing = pool_in_tier(
                &router,
                &format!("plan-update-{index}-{effective}"),
                requested,
            )
            .await;
            let was = described_pool(&router, &existing).await;
            let mut update = (row.members)()
                .as_object()
                .cloned()
                .expect("the members are a JSON object");
            update.insert("UserPoolId".to_string(), json!(existing));
            if let Some(requested) = requested {
                update.insert("UserPoolTier".to_string(), json!(requested));
            }
            let (status, payload) = call(&router, "UpdateUserPool", &Value::Object(update)).await;
            assert_eq!(
                (status, error_code(&payload), error_message(&payload)),
                (
                    400,
                    "FeatureUnavailableInTierException",
                    expected.as_str()
                ),
                "case {case:?}: the update owes the same refusal, got {status} with {payload}"
            );
            let now = described_pool(&router, &existing).await;
            assert_eq!(
                now, was,
                "case {case:?}: a refused update must leave the pool alone, got {now}"
            );
        }
    }
}

#[tokio::test]
async fn a_member_that_activates_nothing_is_served_on_every_plan() {
    let router = router();

    // The control that keeps the gate on what the configuration ASKS FOR rather
    // than on the member being present: threat protection switched off or
    // logging only ("to log user security information but take no action, set
    // to AUDIT", `UserPoolAddOnsType`), and a sign-in policy that admits
    // username and password alone, are served on the cheapest plan.
    for (case, members) in [
        (
            "threat protection switched off",
            json!({ "UserPoolAddOns": { "AdvancedSecurityMode": "OFF" } }),
        ),
        (
            "threat protection logging only, taking no action",
            json!({ "UserPoolAddOns": { "AdvancedSecurityMode": "AUDIT" } }),
        ),
        (
            "a sign-in policy of username and password alone",
            json!({ "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD"] } } }),
        ),
    ] {
        let mut body = members
            .as_object()
            .cloned()
            .expect("the members are a JSON object");
        body.insert("PoolName".to_string(), json!("plan-neutral"));
        body.insert("UserPoolTier".to_string(), json!("LITE"));
        let (status, payload) = call(&router, "CreateUserPool", &Value::Object(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a configuration that activates nothing is served on the cheapest \
             plan, got {status} with {payload}"
        );
        assert_eq!(
            payload["UserPool"]["UserPoolTier"],
            json!("LITE"),
            "case {case:?}: the pool is stored on the plan it asked for, got {payload}"
        );
    }
}

#[tokio::test]
async fn an_app_client_flow_the_pools_plan_excludes_is_refused_and_stores_nothing() {
    let router = router();

    for (plan_case, requested, effective) in PLANS {
        let case = format!("ALLOW_USER_AUTH on a pool in {plan_case}");
        let user_pool_id =
            pool_in_tier(&router, &format!("client-plan-{effective}"), requested).await;
        let before = clients(&router, &user_pool_id).await.len();
        let (status, payload) = create_client(
            &router,
            &user_pool_id,
            "choice-based",
            &json!({ "ExplicitAuthFlows": ["ALLOW_USER_AUTH"] }),
        )
        .await;
        let after = clients(&router, &user_pool_id).await.len();

        if plan_includes(effective, "ESSENTIALS") {
            assert_eq!(
                status, 200,
                "case {case:?}: the plan includes choice-based sign-in, got {status} with {payload}"
            );
            let client_id = payload["UserPoolClient"]["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted client id: {payload}"))
                .to_string();
            let read = described_client(&router, &user_pool_id, &client_id).await;
            assert_eq!(
                read["ExplicitAuthFlows"],
                json!(["ALLOW_USER_AUTH"]),
                "case {case:?}: the stored client must carry the flow, got {read}"
            );
            continue;
        }

        let expected = feature_unavailable_message(effective, "Choice Based Authentication");
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "FeatureUnavailableInTierException",
                expected.as_str()
            ),
            "case {case:?}: expected the tier refusal naming the feature, got {status} with \
             {payload}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no app client, got {after}"
        );

        // A flow every plan includes is served on the same pool, so the gate is
        // on the flow rather than on the operation.
        let (status, payload) = create_client(
            &router,
            &user_pool_id,
            "password-based",
            &json!({ "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a flow no plan gates is served, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn propagating_user_context_data_needs_the_app_clients_secret() {
    let router = router();
    let user_pool_id = pool(&router, "context-data-pool").await;

    // The rows differ along how the app client comes by a secret: generated,
    // supplied by the request, or none at all.
    for (case, secret_members, accepted) in [
        ("a client that generates a secret", json!({ "GenerateSecret": true }), true),
        (
            "a client whose secret the request supplies",
            json!({ "ClientSecret": "abcdefghijklmnopqrstuvwx" }),
            true,
        ),
        ("a public client with no secret", json!({}), false),
        (
            "a client that explicitly generates none",
            json!({ "GenerateSecret": false }),
            false,
        ),
    ] {
        let mut members = secret_members
            .as_object()
            .cloned()
            .expect("the members are a JSON object");
        members.insert(
            "EnablePropagateAdditionalUserContextData".to_string(),
            json!(true),
        );
        let before = clients(&router, &user_pool_id).await.len();
        let (status, payload) = create_client(
            &router,
            &user_pool_id,
            "context-data",
            &Value::Object(members),
        )
        .await;
        let after = clients(&router, &user_pool_id).await.len();

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a client with a secret may propagate the data, got {status} with \
                 {payload}"
            );
            let client_id = payload["UserPoolClient"]["ClientId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a minted client id: {payload}"))
                .to_string();
            let read = described_client(&router, &user_pool_id, &client_id).await;
            assert_eq!(
                read["EnablePropagateAdditionalUserContextData"],
                json!(true),
                "case {case:?}: the stored client must carry the setting, got {read}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (
                400,
                "InvalidParameterException",
                "Client Secret is required to set EnablePropagateAdditionalUserContextData as true"
            ),
            "case {case:?}: expected the refusal naming the missing secret, got {status} with \
             {payload}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: a refused create stores no app client, got {after}"
        );
    }

    // And the same rule through the update: a public app client cannot be given
    // the setting afterwards, while one holding a secret can.
    for (case, generate_secret, accepted) in [
        ("an app client holding a secret", true, true),
        ("a public app client", false, false),
    ] {
        let client_id = ok(
            &router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientName": "updated-context-data",
                "GenerateSecret": generate_secret,
            }),
        )
        .await["UserPoolClient"]["ClientId"]
            .as_str()
            .expect("a minted client id")
            .to_string();
        let was = described_client(&router, &user_pool_id, &client_id).await;
        let (status, payload) = call(
            &router,
            "UpdateUserPoolClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "EnablePropagateAdditionalUserContextData": true,
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: the update is served, got {status} with {payload}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected the refusal naming the missing secret, got {status} with \
             {payload}"
        );
        let now = described_client(&router, &user_pool_id, &client_id).await;
        assert_eq!(
            now, was,
            "case {case:?}: a refused update must leave the app client alone, got {now}"
        );
    }
}

// ---------------------------------------------------------------------------
// The user-import CSV: the upload target, and the header a client builds it from
// ---------------------------------------------------------------------------

/// Upload `csv` to `url` — the ANONYMOUS `PUT` a client writes a pre-signed
/// target with, carrying no credential and no `X-Amz-Target`, only the path of
/// the locator this service handed out — and answer its HTTP status.
async fn upload(router: &MultiServiceRouter, url: &str, csv: &[u8]) -> u16 {
    let path = url
        .strip_prefix(&format!("https://{ENDPOINT_HOST}"))
        .unwrap_or_else(|| panic!("{url} must be a locator on {ENDPOINT_HOST}"));
    let request = http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("host", ENDPOINT_HOST)
        .header("content-type", "text/csv")
        .body(Bytes::from(csv.to_vec()))
        .expect("the request is well formed");
    router.dispatch(request).await.status().as_u16()
}

/// Create a user import job on `user_pool_id` and answer `(JobId, PreSignedUrl)`.
async fn import_job(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": name,
            "CloudWatchLogsRoleArn": IMPORT_ROLE,
        }),
    )
    .await["UserImportJob"]
        .clone();
    let job_id = created["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted job id: {created}"))
        .to_string();
    let url = created["PreSignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("a created job carries its upload locator: {created}"))
        .to_string();
    (job_id, url)
}

#[tokio::test]
async fn a_job_starts_only_once_its_csv_has_been_uploaded() {
    let router = router();
    let user_pool_id = pool(&router, "import-precondition-pool").await;

    // The rows differ along what reached the upload target: nothing at all, an
    // empty body (a file with neither a header row nor a user), a header row
    // alone, and a header row with users.
    for (case, uploaded, starts) in [
        ("no upload at all", None, false),
        ("an empty body", Some(&b""[..]), false),
        (
            "a header row alone",
            Some(&b"cognito:username,email\n"[..]),
            true,
        ),
        (
            "a header row and two users",
            Some(
                &b"cognito:username,email\nalice,alice@example.com\nbob,bob@example.com\n"[..],
            ),
            true,
        ),
    ] {
        let (job_id, url) = import_job(&router, &user_pool_id, "precondition-job").await;
        if let Some(csv) = uploaded {
            let status = upload(&router, &url, csv).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the upload target must accept the PUT, got {status}"
            );
        }

        let (status, payload) = call(
            &router,
            "StartUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        let read = ok(
            &router,
            "DescribeUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await["UserImportJob"]
            .clone();

        if starts {
            assert_eq!(
                status, 200,
                "case {case:?}: a job whose CSV was uploaded starts, got {status} with {payload}"
            );
            assert_eq!(
                read["Status"],
                json!("Pending"),
                "case {case:?}: the started job is Pending, got {read}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&payload)),
            (400, "PreconditionNotMetException"),
            "case {case:?}: expected a 400 PreconditionNotMetException, got {status} with {payload}"
        );
        // The refusal moved nothing: the job is still waiting to be started, and
        // starts as soon as its CSV arrives.
        assert_eq!(
            read["Status"],
            json!("Created"),
            "case {case:?}: a refused start must leave the job Created, got {read}"
        );
        assert_eq!(
            read.get("StartDate"),
            None,
            "case {case:?}: a refused start must not date the job, got {read}"
        );
        let status = upload(&router, &url, b"cognito:username\nalice\n").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the upload target must accept a later PUT, got {status}"
        );
        let started = ok(
            &router,
            "StartUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        assert_eq!(
            started["UserImportJob"]["Status"],
            json!("Pending"),
            "case {case:?}: the same job starts once its CSV is uploaded, got {started}"
        );
    }
}

#[tokio::test]
async fn the_csv_header_reports_one_column_per_attribute_of_the_pools_schema() {
    let router = router();

    // The rows differ along the pool's schema: the standard attributes alone,
    // and the same pool once custom attributes have been added.
    let user_pool_id = pool(&router, "csv-header-pool").await;
    let header = ok(
        &router,
        "GetCSVHeader",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    assert_eq!(
        header["UserPoolId"],
        json!(user_pool_id),
        "the answer names the pool it was asked about, got {header}"
    );
    let columns: Vec<String> = header["CSVHeader"]
        .as_array()
        .unwrap_or_else(|| panic!("the answer carries its header columns: {header}"))
        .iter()
        .map(|column| column.as_str().unwrap_or_default().to_string())
        .collect();

    // Every column is an attribute of the stored schema or one of the two the
    // import format adds; the minted `sub` is not importable, and neither is the
    // `identities` attribute the service maintains itself.
    let described = described_pool(&router, &user_pool_id).await;
    let schema: Vec<String> = described["SchemaAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("a pool reports its schema: {described}"))
        .iter()
        .map(|attribute| attribute["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    for attribute in &schema {
        let expected = attribute != "sub" && attribute != "identities";
        assert_eq!(
            columns.contains(attribute),
            expected,
            "case {attribute:?}: a column is reported for every attribute but the minted sub, got \
             {columns:?}"
        );
    }
    for added in ["cognito:mfa_enabled", "cognito:username"] {
        assert!(
            columns.contains(&added.to_string()),
            "case {added:?}: the import format's own column is reported, got {columns:?}"
        );
    }

    // A custom attribute added to the pool's schema is reported as a column of
    // its own, so the header is computed from the stored schema rather than
    // fixed.
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [
                { "Name": "department", "AttributeDataType": "String" },
                { "Name": "seats", "AttributeDataType": "Number" },
            ],
        }),
    )
    .await;
    let widened: Vec<String> = ok(
        &router,
        "GetCSVHeader",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["CSVHeader"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|column| column.as_str().unwrap_or_default().to_string())
        .collect();
    for added in ["custom:department", "custom:seats"] {
        assert!(
            widened.contains(&added.to_string()),
            "case {added:?}: a custom attribute is a column of the header, got {widened:?}"
        );
    }
    assert_eq!(
        widened.len(),
        columns.len() + 2,
        "two custom attributes add two columns, got {widened:?}"
    );

    // A pool nothing created has no header to report.
    let (status, payload) = call(
        &router,
        "GetCSVHeader",
        &json!({ "UserPoolId": "us-east-1_absentpool" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "case \"an absent pool\": expected a ResourceNotFoundException, got {status} with {payload}"
    );
}

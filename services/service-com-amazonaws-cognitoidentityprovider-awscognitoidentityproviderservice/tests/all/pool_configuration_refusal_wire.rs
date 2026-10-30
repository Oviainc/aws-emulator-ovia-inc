// SPDX-License-Identifier: CC0-1.0

//! How this endpoint answers a refusal raised by one of its POOL-LEVEL CONFIGURATION
//! operations, over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test is the one [`super::error_wire`] states for this endpoint, asserted
//! here over the configuration surface of a user pool: a refusal of the CALLER is answered
//! with HTTP 400 and names its error shape in the `x-amzn-errortype` response header, as
//! the shape's simple name followed by a colon, whatever `@httpError` the model annotates
//! that shape with. The model gives `ResourceNotFoundException` 404 and
//! `FeatureUnavailableInTierException` 403
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,FeatureUnavailableInTierException")`),
//! and this endpoint applies neither: the error class travels in the body's `__type` and in
//! the header, and the status says only that the request was refused. That matters for
//! reconciling tooling, which treats a modelled 400 "the resource is gone" as drift it can
//! repair and a 404 as a routing failure of the endpoint itself.
//!
//! The rows span what the rule generalises over, so no row is the rule: the pool-level
//! configuration SURFACE the call addresses (the pool's own configuration, its tags, its
//! branding, its log delivery, its threat-protection configuration, its user-import jobs,
//! its domains and its Region replicas), the way a request ADDRESSES the resource (pool id,
//! pool ARN, pool + app client, pool + domain name, pool + Region), the LOOKUP that
//! resolves nothing (a pool nothing minted, a pool a delete removed, an app client, a
//! domain name, a replica), and the error CLASS — and so the modelled status — the refusal
//! is raised with (404, 403 and 400). The control is the same operations against a pool
//! that IS stored: they are served, keep their own status and name no error shape, so the
//! 400 is computed from state rather than answered to every call.
//!
//! Every assertion names its case and reports the value it saw.

use serde_json::{json, Value};

use super::error_wire::{
    call, error_code, ok, router, Answered, ERRORTYPE, REFUSED, UNMINTED_CLIENT, UNMINTED_POOL,
};

/// A Region of this emulator's shared Region catalog that no pool below is replicated to,
/// so it names no replica. `RegionNameType` bounds only the length (5..32,
/// `smithy_query(query="operation",operation="UpdateUserPoolReplica")`), so which values
/// name a Region at all is the service's own catalog.
const UNREPLICATED_REGION: &str = "eu-west-1";

/// A role ARN `CreateUserImportJob` requires, in the form `ArnType`'s pattern demands
/// (`smithy_query(query="operation",operation="CreateUserImportJob")`). Whether the role
/// exists is decided where the import logs, so a well-formed ARN is enough to reach the
/// pool lookup.
const IMPORT_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// An ARN that satisfies `ArnType`'s modelled length and pattern but names a resource of
/// another service, so it names no user pool to configure.
const OTHER_SERVICE_ARN: &str = "arn:aws:dynamodb:us-east-1:000000000000:table/not-a-pool";

/// A domain name no pool below created, in the form `DomainType`'s pattern demands
/// (`^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, from
/// `smithy_query(query="operation",operation="UpdateUserPoolDomain")`).
const UNCREATED_DOMAIN: &str = "auth-nothing-created";

/// One row: a pool-level configuration call this service refuses, and the error class it
/// refuses it with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: Value,
    class: &'static str,
}

/// The id of a pool created with `request`.
async fn pool_id(router: &smithy_server_runtime::MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateUserPool", request).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string()
}

#[tokio::test]
async fn every_pool_configuration_refusal_is_a_400_naming_its_error_shape() {
    let router = router();

    // One stored pool with one app client, so a row addressing a configuration level of a
    // pool that IS there is refused for the level it names rather than for the pool.
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "pool-configuration-refusal" }),
    )
    .await;
    let stored_pool = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();
    let stored_arn = created["UserPool"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool ARN: {created}"))
        .to_string();
    // The ARN of a pool nothing minted: the stored pool's, with the pool id replaced, so
    // the ARN is well formed and only its lookup resolves nothing.
    let unminted_arn = stored_arn.replace(&stored_pool, UNMINTED_POOL);

    // A pool a DELETE removed, which is the state a teardown path re-runs against.
    let removed_pool = pool_id(&router, &json!({ "PoolName": "pool-configuration-removed" })).await;
    ok(
        &router,
        "DeleteUserPool",
        &json!({ "UserPoolId": &removed_pool }),
    )
    .await;

    // A pool on the cheapest feature plan, so one row is refused with the class the model
    // annotates 403 rather than 404.
    let lite_pool = pool_id(
        &router,
        &json!({ "PoolName": "pool-configuration-lite", "UserPoolTier": "LITE" }),
    )
    .await;

    let rows = vec![
        // The pool's own configuration, addressed by pool id.
        Refusal {
            case: "UpdateUserPool on a pool a delete removed",
            operation: "UpdateUserPool",
            body: json!({ "UserPoolId": &removed_pool }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateUserPool on a pool nothing created",
            operation: "UpdateUserPool",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        // The pool's log delivery configuration.
        Refusal {
            case: "GetLogDeliveryConfiguration for a pool nothing created",
            operation: "GetLogDeliveryConfiguration",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        // The pool's tags, addressed by ARN rather than by pool id.
        Refusal {
            case: "TagResource on the ARN of a pool nothing created",
            operation: "TagResource",
            body: json!({ "ResourceArn": &unminted_arn, "Tags": { "Team": "wire" } }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "ListTagsForResource on the ARN of a pool nothing created",
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": &unminted_arn }),
            class: "ResourceNotFoundException",
        },
        // The pool's threat-protection configuration, at either level.
        Refusal {
            case: "DescribeRiskConfiguration for a pool nothing created",
            operation: "DescribeRiskConfiguration",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeRiskConfiguration for an app client the pool does not hold",
            operation: "DescribeRiskConfiguration",
            body: json!({ "UserPoolId": &stored_pool, "ClientId": UNMINTED_CLIENT }),
            class: "ResourceNotFoundException",
        },
        // The pool's branding, at either level.
        Refusal {
            case: "GetUICustomization for a pool nothing created",
            operation: "GetUICustomization",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "GetUICustomization for an app client the pool does not hold",
            operation: "GetUICustomization",
            body: json!({ "UserPoolId": &stored_pool, "ClientId": UNMINTED_CLIENT }),
            class: "ResourceNotFoundException",
        },
        // The pool's user-import jobs.
        Refusal {
            case: "CreateUserImportJob in a pool nothing created",
            operation: "CreateUserImportJob",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "JobName": "import-nowhere",
                "CloudWatchLogsRoleArn": IMPORT_ROLE_ARN,
            }),
            class: "ResourceNotFoundException",
        },
        // The pool's domains, addressed by the pair pool id + domain name.
        Refusal {
            case: "UpdateUserPoolDomain for a domain name the pool does not hold",
            operation: "UpdateUserPoolDomain",
            body: json!({
                "UserPoolId": &stored_pool,
                "Domain": UNCREATED_DOMAIN,
                "ManagedLoginVersion": 1,
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateUserPoolDomain for a pool nothing created",
            operation: "UpdateUserPoolDomain",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "Domain": UNCREATED_DOMAIN,
                "ManagedLoginVersion": 1,
            }),
            class: "ResourceNotFoundException",
        },
        // The pool's Region replicas, addressed by pool id and by the pair pool id +
        // Region.
        Refusal {
            case: "ListUserPoolReplicas for a pool nothing created",
            operation: "ListUserPoolReplicas",
            body: json!({ "UserPoolId": UNMINTED_POOL }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateUserPoolReplica for a pool nothing created",
            operation: "UpdateUserPoolReplica",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "RegionName": UNREPLICATED_REGION,
                "Status": "ACTIVE",
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateUserPoolReplica for a Region the pool holds no replica in",
            operation: "UpdateUserPoolReplica",
            body: json!({
                "UserPoolId": &stored_pool,
                "RegionName": UNREPLICATED_REGION,
                "Status": "ACTIVE",
            }),
            class: "ResourceNotFoundException",
        },
        // A refusal of the same surface raised with the class the model annotates 403: the
        // status is the request's outcome, not the class's.
        Refusal {
            case: "UpdateUserPool asking for a feature the pool's plan excludes",
            operation: "UpdateUserPool",
            body: json!({
                "UserPoolId": &lite_pool,
                "UserPoolTier": "LITE",
                "UserPoolAddOns": { "AdvancedSecurityMode": "ENFORCED" },
            }),
            class: "FeatureUnavailableInTierException",
        },
        // And one the model annotates 400 already, so only the header is new: an ARN that
        // names no user pool is a bad parameter rather than a missing resource.
        Refusal {
            case: "TagResource on an ARN naming a resource of another service",
            operation: "TagResource",
            body: json!({ "ResourceArn": OTHER_SERVICE_ARN, "Tags": { "Team": "wire" } }),
            class: "InvalidParameterException",
        },
    ];

    for Refusal {
        case,
        operation,
        body,
        class,
    } in rows
    {
        let answered = call(&router, operation, &body).await;
        assert_eq!(
            answered.status, REFUSED,
            "case {case:?}: a refusal of the caller is HTTP {REFUSED}, got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            error_code(&answered.payload),
            class,
            "case {case:?}: expected the body to name {class}, got {}",
            answered.payload
        );
        let expected = format!("{class}:");
        assert_eq!(
            answered.errortype.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {ERRORTYPE} {expected:?}, got {:?}",
            answered.errortype
        );
    }
}

/// The control: every operation of the table above, against a pool this service HOLDS, is
/// served — it keeps its own status and names no error shape — so the refusals above are
/// decided by the lookup rather than answered to every configuration call.
#[tokio::test]
async fn the_same_configuration_calls_are_served_for_a_pool_this_service_holds() {
    let router = router();
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({ "PoolName": "pool-configuration-served" }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();
    let pool_arn = created["UserPool"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool ARN: {created}"))
        .to_string();
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": &user_pool_id, "ClientName": "served" }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted app client id: {client}"))
        .to_string();

    // (case, operation, body) — one call per configuration surface the table refuses on.
    let rows = vec![
        (
            "the pool's own configuration updated",
            "UpdateUserPool",
            json!({ "UserPoolId": &user_pool_id }),
        ),
        (
            "the pool's log delivery configuration read",
            "GetLogDeliveryConfiguration",
            json!({ "UserPoolId": &user_pool_id }),
        ),
        (
            "the pool's tags assigned through its ARN",
            "TagResource",
            json!({ "ResourceArn": &pool_arn, "Tags": { "Team": "wire" } }),
        ),
        (
            "the pool's tags listed through its ARN",
            "ListTagsForResource",
            json!({ "ResourceArn": &pool_arn }),
        ),
        (
            "the pool's threat-protection configuration read",
            "DescribeRiskConfiguration",
            json!({ "UserPoolId": &user_pool_id }),
        ),
        (
            "an app client's branding read",
            "GetUICustomization",
            json!({ "UserPoolId": &user_pool_id, "ClientId": &client_id }),
        ),
        (
            "a user-import job created in the pool",
            "CreateUserImportJob",
            json!({
                "UserPoolId": &user_pool_id,
                "JobName": "import-served",
                "CloudWatchLogsRoleArn": IMPORT_ROLE_ARN,
            }),
        ),
        (
            "the pool's replicas listed",
            "ListUserPoolReplicas",
            json!({ "UserPoolId": &user_pool_id }),
        ),
    ];

    for (case, operation, body) in rows {
        let Answered {
            status,
            errortype,
            payload,
        } = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {payload}"
        );
        assert_eq!(
            errortype, None,
            "case {case:?}: a served call names no error shape, got {errortype:?}"
        );
    }
}

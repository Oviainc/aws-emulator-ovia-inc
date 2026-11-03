// SPDX-License-Identifier: CC0-1.0

//! Retiring a multi-Region user pool replica over the wire:
//! `DeleteUserPoolReplica` "Deletes a secondary replica user pool. You can only
//! delete replicas that are in the INACTIVE status. This operation must be
//! called from the primary Region." Driven through the router exactly as an SDK
//! drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a delete is served only for a stored SECONDARY replica
//! whose status at that instant is `INACTIVE`, and only when the request is made
//! in the Region the pool was created in; a replica a delete has started reports
//! `DELETING` until the clock makes its removal due and is gone from every read
//! from then on, which frees its Region for another replica. Beside it: the
//! operations that change WHICH replicas a pool has are the primary Region's
//! alone, while `UpdateUserPoolReplica` — the one the model says "can be made in
//! both primary and secondary regions of the user pool" — is served from any
//! Region, and an update may only name a status `UpdateReplicaStatusType`
//! declares. Each table row names the case it asserts.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state::{
    REPLICA_ACTIVATION_SPAN, REPLICA_DELETION_SPAN, REPLICA_STATUS_ACTIVE,
    REPLICA_STATUS_CREATING, REPLICA_STATUS_DELETING, REPLICA_STATUS_INACTIVE,
};
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// The Region every request is signed for unless it names another one: a
/// request's SigV4 credential scope is what routes it to this service and what
/// names the Region it was made in, so it is the Region a created pool — the
/// PRIMARY replica — lives in. The signature is never checked here: the suite
/// turns request authentication off.
const PRIMARY_REGION: &str = "us-east-1";

/// The Region the replica under test is created in.
const REPLICA_REGION: &str = "us-west-2";

/// A third Region, so a request can be made somewhere that is neither the
/// pool's own Region nor the replica's.
const UNRELATED_REGION: &str = "eu-central-1";

/// A `KeyConfiguration.KmsKeyArn`-shaped ARN (`EncryptionKeyArnType`, len
/// 20..2048) standing for the multi-Region customer-managed key a replicated
/// pool is encrypted under.
const MULTI_REGION_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/multi-region-example";

/// Long enough that the clock has made a created replica's activation due.
const PAST_THE_ACTIVATION: Duration =
    REPLICA_ACTIVATION_SPAN.saturating_add(Duration::from_millis(250));

/// Long enough that the clock has made a deleted replica's removal due.
const PAST_THE_REMOVAL: Duration =
    REPLICA_DELETION_SPAN.saturating_add(Duration::from_millis(250));

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` in `region` with `body` and return its status and JSON
/// response body.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cognito-idp/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cognito-idp.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", authorization)
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

/// Call `operation` in the primary Region.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, PRIMARY_REGION, operation, body).await
}

/// Call `operation` in the primary Region, requiring success.
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
///
/// The pool is encrypted under a customer-managed key, which is what a pool has
/// to hold to be replicated at all: "you must use a multi-region key to enable
/// multi-region replication for a user pool"
/// (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    body.insert(
        "KeyConfiguration".to_string(),
        json!({ "KeyType": "CUSTOMER_MANAGED_KEY", "KmsKeyArn": MULTI_REGION_KEY_ARN }),
    );
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The entry `ListUserPoolReplicas` reports for `region`, or `None` when the
/// listing holds none.
async fn replica(router: &MultiServiceRouter, user_pool_id: &str, region: &str) -> Option<Value> {
    let listed = ok(
        router,
        "ListUserPoolReplicas",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    listed["UserPoolReplicas"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUserPoolReplicas answers a list, got {listed}"))
        .iter()
        .find(|entry| entry["RegionName"] == json!(region))
        .cloned()
}

/// The status `ListUserPoolReplicas` reports for `region`.
async fn status_of(router: &MultiServiceRouter, user_pool_id: &str, region: &str) -> String {
    let entry = replica(router, user_pool_id, region)
        .await
        .unwrap_or_else(|| panic!("the replica in {region} must be listed"));
    entry["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("a listed replica carries a status, got {entry}"))
        .to_string()
}

/// How far a row drives the replica's lifecycle before it attempts the delete,
/// which is what decides the status the replica is in at that instant.
#[derive(Debug, Clone, Copy)]
enum Prepared {
    /// Created and read straight away, so the clock still owes its activation.
    StillCreating,
    /// Created and left until the clock settled it.
    Settled,
    /// Created, settled, then activated by an update.
    Activated,
    /// Created, settled, activated, then deactivated by a second update: the
    /// same status as `Settled`, reached by a different route.
    Deactivated,
}

/// One row of the delete table: the lifecycle point the delete is attempted
/// from, the status the replica is in there, and the outcome.
struct DeleteCase {
    case: &'static str,
    prepared: Prepared,
    status_at_delete: &'static str,
    deletable: bool,
}

/// The table. The rows span every `ReplicaStatusType` a secondary replica can
/// be in when a delete arrives — `CREATING`, `INACTIVE` and `ACTIVE`
/// (`smithy_query(query="shape",shape="ReplicaStatusType")`) — and reach
/// `INACTIVE` by both routes: the status a create settles into and the status an
/// update writes back.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a replica the clock still owes its activation",
            prepared: Prepared::StillCreating,
            status_at_delete: REPLICA_STATUS_CREATING,
            deletable: false,
        },
        DeleteCase {
            case: "a replica that settled after its creation",
            prepared: Prepared::Settled,
            status_at_delete: REPLICA_STATUS_INACTIVE,
            deletable: true,
        },
        DeleteCase {
            case: "a replica an update activated",
            prepared: Prepared::Activated,
            status_at_delete: REPLICA_STATUS_ACTIVE,
            deletable: false,
        },
        DeleteCase {
            case: "a replica an update deactivated again",
            prepared: Prepared::Deactivated,
            status_at_delete: REPLICA_STATUS_INACTIVE,
            deletable: true,
        },
    ]
}

/// Drive `region`'s replica of `user_pool_id` to the lifecycle point `prepared`
/// names.
async fn prepare(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    region: &str,
    prepared: Prepared,
) {
    ok(
        router,
        "CreateUserPoolReplica",
        &json!({ "UserPoolId": user_pool_id, "RegionName": region }),
    )
    .await;
    if matches!(prepared, Prepared::StillCreating) {
        return;
    }
    tokio::time::sleep(PAST_THE_ACTIVATION).await;
    if matches!(prepared, Prepared::Activated | Prepared::Deactivated) {
        ok(
            router,
            "UpdateUserPoolReplica",
            &json!({
                "UserPoolId": user_pool_id,
                "RegionName": region,
                "Status": REPLICA_STATUS_ACTIVE,
            }),
        )
        .await;
    }
    if matches!(prepared, Prepared::Deactivated) {
        ok(
            router,
            "UpdateUserPoolReplica",
            &json!({
                "UserPoolId": user_pool_id,
                "RegionName": region,
                "Status": REPLICA_STATUS_INACTIVE,
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn only_an_inactive_replica_is_deleted() {
    for DeleteCase {
        case,
        prepared,
        status_at_delete,
        deletable,
    } in delete_cases()
    {
        let router = router();
        let user_pool_id = pool(&router, "retired").await;
        prepare(&router, &user_pool_id, REPLICA_REGION, prepared).await;

        assert_eq!(
            status_of(&router, &user_pool_id, REPLICA_REGION).await,
            status_at_delete,
            "case {case:?}: the row drives the replica to the status it names"
        );

        let (answered, payload) = call(
            &router,
            "DeleteUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
        )
        .await;

        if deletable {
            assert_eq!(
                (answered, &payload["UserPoolReplica"]["Status"]),
                (200, &json!(REPLICA_STATUS_DELETING)),
                "case {case:?}: an INACTIVE replica is deleted and reports DELETING, got \
                 {answered} with {payload}"
            );
            assert_eq!(
                status_of(&router, &user_pool_id, REPLICA_REGION).await,
                REPLICA_STATUS_DELETING,
                "case {case:?}: the listing reports the deletion the delete stored"
            );
        } else {
            assert_eq!(
                (answered, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: a replica that is not INACTIVE is not deletable, got {answered} \
                 with {payload}"
            );
            assert!(
                error_message(&payload).contains(REPLICA_STATUS_INACTIVE)
                    && error_message(&payload).contains(status_at_delete),
                "case {case:?}: the refusal names the status the replica is in and the one a \
                 deletable replica must be in, got {payload}"
            );
            assert_eq!(
                status_of(&router, &user_pool_id, REPLICA_REGION).await,
                status_at_delete,
                "case {case:?}: the refused delete left the stored replica alone"
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_replica_is_gone_once_the_clock_owes_its_removal() {
    let router = router();
    let user_pool_id = pool(&router, "removed").await;
    prepare(&router, &user_pool_id, REPLICA_REGION, Prepared::Settled).await;
    ok(
        &router,
        "DeleteUserPoolReplica",
        &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
    )
    .await;

    // Still reported, as DELETING, until the removal the delete started is due:
    // repeated reads report that same status and move nothing on.
    for read in 0..3 {
        assert_eq!(
            status_of(&router, &user_pool_id, REPLICA_REGION).await,
            REPLICA_STATUS_DELETING,
            "read {read}: a replica being deleted is listed as DELETING"
        );
    }
    // A replica on its way out no longer takes a status, and is not deleted a
    // second time: each call is refused with the error that states why.
    let (answered, payload) = call(
        &router,
        "UpdateUserPoolReplica",
        &json!({
            "UserPoolId": user_pool_id,
            "RegionName": REPLICA_REGION,
            "Status": REPLICA_STATUS_ACTIVE,
        }),
    )
    .await;
    assert_eq!(
        (answered, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a replica a delete has started is no longer there to configure, got {answered} with \
         {payload}"
    );
    let (answered, payload) = call(
        &router,
        "DeleteUserPoolReplica",
        &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
    )
    .await;
    assert_eq!(
        (answered, error_code(&payload)),
        (400, "InvalidParameterException"),
        "a replica already being deleted is not INACTIVE, so a second delete is refused, got \
         {answered} with {payload}"
    );
    assert!(
        error_message(&payload).contains(REPLICA_STATUS_DELETING)
            && error_message(&payload).contains(REPLICA_STATUS_INACTIVE),
        "the refusal names the status the replica is in and the one a deletable replica must be \
         in, got {payload}"
    );
    assert_eq!(
        status_of(&router, &user_pool_id, REPLICA_REGION).await,
        REPLICA_STATUS_DELETING,
        "neither refused call changed the deletion already under way"
    );

    tokio::time::sleep(PAST_THE_REMOVAL).await;
    assert!(
        replica(&router, &user_pool_id, REPLICA_REGION)
            .await
            .is_none(),
        "the replica is gone from the listing once the clock has made its removal due"
    );
    let listed = ok(&router, "ListUserPools", &json!({ "MaxResults": 20 })).await;
    let entry = listed["UserPools"]
        .as_array()
        .and_then(|pools| {
            pools
                .iter()
                .find(|pool| pool["Id"] == json!(user_pool_id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("the pool must be listed, got {listed}"));
    assert_eq!(
        entry["ReplicaRegions"],
        json!([PRIMARY_REGION]),
        "a pool whose only secondary replica the clock has removed reports the Region its \
         PRIMARY replica is in, exactly as one nothing was ever replicated to does, got {entry}"
    );

    // The Region a removed replica held takes another replica, which comes up
    // CREATING like any other.
    let recreated = ok(
        &router,
        "CreateUserPoolReplica",
        &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
    )
    .await;
    assert_eq!(
        recreated["UserPoolReplica"]["Status"],
        json!(REPLICA_STATUS_CREATING),
        "the Region a removed replica held is free for another replica, got {recreated}"
    );
}

#[tokio::test]
async fn the_calls_that_change_which_replicas_exist_are_served_in_the_primary_region_alone() {
    let router = router();
    let user_pool_id = pool(&router, "regional").await;

    // A create made outside the pool's own Region names an operation that is
    // not available there, and stores nothing.
    for region in [REPLICA_REGION, UNRELATED_REGION] {
        let (answered, payload) = call_in(
            &router,
            region,
            "CreateUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
        )
        .await;
        assert_eq!(
            (answered, error_code(&payload)),
            (400, "OperationNotEnabledException"),
            "case {region:?}: a create made outside the primary Region is refused, got \
             {answered} with {payload}"
        );
        assert!(
            replica(&router, &user_pool_id, REPLICA_REGION)
                .await
                .is_none(),
            "case {region:?}: the refused create stored no replica"
        );
    }

    prepare(&router, &user_pool_id, REPLICA_REGION, Prepared::Settled).await;

    // "This request can be made in both primary and secondary regions of the
    // user pool" (`UpdateUserPoolReplica`), so the Region a request is made in
    // does not decide whether it is served.
    for (case, caller_region, status) in [
        ("the primary Region", PRIMARY_REGION, REPLICA_STATUS_ACTIVE),
        (
            "the replica's own Region",
            REPLICA_REGION,
            REPLICA_STATUS_INACTIVE,
        ),
        (
            "a Region neither the pool nor the replica is in",
            UNRELATED_REGION,
            REPLICA_STATUS_ACTIVE,
        ),
    ] {
        let (answered, payload) = call_in(
            &router,
            caller_region,
            "UpdateUserPoolReplica",
            &json!({
                "UserPoolId": user_pool_id,
                "RegionName": REPLICA_REGION,
                "Status": status,
            }),
        )
        .await;
        assert_eq!(
            (answered, &payload["UserPoolReplica"]["Status"]),
            (200, &json!(status)),
            "case {case:?}: an update made in {caller_region} stores the status it names, got \
             {answered} with {payload}"
        );
    }

    // "This operation must be called from the primary Region"
    // (`DeleteUserPoolReplica`).
    ok(
        &router,
        "UpdateUserPoolReplica",
        &json!({
            "UserPoolId": user_pool_id,
            "RegionName": REPLICA_REGION,
            "Status": REPLICA_STATUS_INACTIVE,
        }),
    )
    .await;
    for region in [REPLICA_REGION, UNRELATED_REGION] {
        let (answered, payload) = call_in(
            &router,
            region,
            "DeleteUserPoolReplica",
            &json!({ "UserPoolId": user_pool_id, "RegionName": REPLICA_REGION }),
        )
        .await;
        assert_eq!(
            (answered, error_code(&payload)),
            (400, "OperationNotEnabledException"),
            "case {region:?}: a delete made outside the primary Region is refused, got \
             {answered} with {payload}"
        );
        assert_eq!(
            status_of(&router, &user_pool_id, REPLICA_REGION).await,
            REPLICA_STATUS_INACTIVE,
            "case {region:?}: the refused delete left the stored replica alone"
        );
    }
}

#[tokio::test]
async fn an_update_names_a_status_the_model_declares() {
    let router = router();
    let user_pool_id = pool(&router, "statuses").await;
    prepare(&router, &user_pool_id, REPLICA_REGION, Prepared::Settled).await;

    // Every row is a value `UpdateReplicaStatusType` does not declare: the two
    // `ReplicaStatusType` values that are the service's to set rather than the
    // caller's, a value of the right shape in the wrong case, a value from
    // another enum of this service, the empty string, and one longer than the
    // framework's own echo bound, which a clause of this service quotes whole.
    let over_cap = "x".repeat(300);
    for status in [
        REPLICA_STATUS_CREATING,
        REPLICA_STATUS_DELETING,
        "active",
        "ENABLED",
        "",
        &over_cap,
    ] {
        let (answered, payload) = call(
            &router,
            "UpdateUserPoolReplica",
            &json!({
                "UserPoolId": user_pool_id,
                "RegionName": REPLICA_REGION,
                "Status": status,
            }),
        )
        .await;
        assert_eq!(
            (answered, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {status:?}: a status outside UpdateReplicaStatusType is refused, got \
             {answered} with {payload}"
        );
        let message = error_message(&payload);
        assert!(
            message.contains("1 validation error detected")
                && message.contains("at 'status'")
                && message.contains("Member must satisfy enum value set: [ACTIVE, INACTIVE]"),
            "case {status:?}: the refusal states the member, the value set it broke and the \
             value it submitted, got {payload}"
        );
        // The submitted value is quoted WHOLE, however long it was: an Amazon
        // Cognito constraint clause quotes the value the request carried, which
        // `input_validation_refusal_wording.rs` asserts over the class of such
        // values. Bounded all the same, by the ceiling this service states from
        // its own model rather than by the framework's own 256-byte elision.
        assert!(
            message.contains(&format!("Value '{status}' at 'status'")),
            "case {status:?}: the submitted value is quoted whole, got {payload}"
        );
        assert_eq!(
            status_of(&router, &user_pool_id, REPLICA_REGION).await,
            REPLICA_STATUS_INACTIVE,
            "case {status:?}: the refused update stored nothing"
        );
    }

    // The two values the model does declare are served, and each one sticks.
    for status in [REPLICA_STATUS_ACTIVE, REPLICA_STATUS_INACTIVE] {
        ok(
            &router,
            "UpdateUserPoolReplica",
            &json!({
                "UserPoolId": user_pool_id,
                "RegionName": REPLICA_REGION,
                "Status": status,
            }),
        )
        .await;
        assert_eq!(
            status_of(&router, &user_pool_id, REPLICA_REGION).await,
            status,
            "case {status:?}: a status the model declares is stored and read back"
        );
    }
}

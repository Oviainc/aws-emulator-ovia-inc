// SPDX-License-Identifier: CC0-1.0

//! The Amazon Keyspaces table a `cassandra` scalable target names, resolved in the service
//! that owns it.
//!
//! RULE: a scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
//! that resource where it lives — a `cassandra:table:*` target scales the provisioned
//! throughput of one Amazon Keyspaces table, so a table that does not exist has no throughput
//! to scale and neither has one whose capacity mode provisions none, and either is refused
//! with `ValidationException` rather than minting a target against capacity that does not
//! exist.
//!
//! The rows differ along what the rule generalises over: both capacity dimensions of the
//! namespace, a table that exists in PROVISIONED mode (accepted), one in on-demand mode, one
//! that does not exist inside a keyspace that does, and a keyspace that does not exist at all.
//!
//! This suite builds its OWN emulator rather than using the shared one
//! ([`super::emulator::Emulator`]), because the rule is about a service the other suites do
//! not mount: Amazon Keyspaces, which is where the table lives.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::emulator::{error_code, error_message, REGION};

/// Application Auto Scaling, Amazon Keyspaces and the two services a registration also
/// touches, over ONE engine — the shape `core/router` assembles them in.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites drive unsigned requests; the `Authorization` header is still what routes
    // each request and names its region, as a signed client's would.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// Dispatch one awsJson request through the router, as a signing client's SDK sends it.
async fn dispatch(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    target: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body().to_vec();
    let document: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
    (status, document)
}

/// Call one Application Auto Scaling operation.
async fn application_auto_scaling(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    dispatch(
        router,
        "application-autoscaling",
        "application/x-amz-json-1.1",
        &format!("AnyScaleFrontendService.{operation}"),
        body,
    )
    .await
}

/// Call one Amazon Keyspaces operation (that model declares `awsJson1_0`).
async fn keyspaces(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    dispatch(
        router,
        "cassandra",
        "application/x-amz-json-1.0",
        &format!("KeyspacesService.{operation}"),
        body,
    )
    .await
}

/// Create a keyspace, and in it a table in the capacity mode `throughput_mode` names.
async fn given_table(
    router: &MultiServiceRouter,
    keyspace: &str,
    table: &str,
    throughput_mode: &str,
) {
    let (status, body) = keyspaces(router, "CreateKeyspace", json!({"keyspaceName": keyspace})).await;
    assert!(
        status == 200 || error_code(&body) == "ConflictException",
        "case {keyspace:?}: expected CreateKeyspace to answer 200, got {status} {body:?}"
    );
    let mut capacity = json!({"throughputMode": throughput_mode});
    if throughput_mode == "PROVISIONED" {
        capacity["readCapacityUnits"] = json!(5);
        capacity["writeCapacityUnits"] = json!(5);
    }
    let (status, body) = keyspaces(
        router,
        "CreateTable",
        json!({
            "keyspaceName": keyspace,
            "tableName": table,
            "schemaDefinition": {
                "allColumns": [{"name": "id", "type": "text"}],
                "partitionKeys": [{"name": "id"}],
            },
            "capacitySpecification": capacity,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {table:?}: expected CreateTable to answer 200, got {status} {body:?}"
    );
}

/// What Amazon Keyspaces holds before the registration under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// The keyspace and the table, in PROVISIONED capacity mode.
    Provisioned,
    /// The keyspace and the table, in on-demand capacity mode.
    OnDemand,
    /// The keyspace alone: the table the registration names was never created.
    KeyspaceOnly,
    /// Nothing at all: not even the keyspace exists.
    Nothing,
}

/// One registration: what the owning service holds, the table the target names, and whether
/// the registration is accepted.
struct Row {
    case: &'static str,
    setup: Setup,
    keyspace: &'static str,
    table: &'static str,
    dimension: &'static str,
    /// `None` for a registration that is accepted; otherwise the sentence it is refused with.
    refusal: Option<String>,
}

/// The rows, over both capacity dimensions the `cassandra` namespace publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScalableDimension")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a provisioned table, on the write dimension",
            setup: Setup::Provisioned,
            keyspace: "orders_ks",
            table: "orders",
            dimension: "cassandra:table:WriteCapacityUnits",
            refusal: None,
        },
        Row {
            case: "a provisioned table, on the read dimension",
            setup: Setup::Provisioned,
            keyspace: "orders_ks",
            table: "orders",
            dimension: "cassandra:table:ReadCapacityUnits",
            refusal: None,
        },
        Row {
            case: "an on-demand table, on the write dimension",
            setup: Setup::OnDemand,
            keyspace: "events_ks",
            table: "events",
            dimension: "cassandra:table:WriteCapacityUnits",
            refusal: Some(
                "Validation failed for scalable target. Reason: Table events must be in \
                 PROVISIONED throughput mode"
                    .to_string(),
            ),
        },
        Row {
            case: "an on-demand table, on the read dimension",
            setup: Setup::OnDemand,
            keyspace: "events_ks",
            table: "events",
            dimension: "cassandra:table:ReadCapacityUnits",
            refusal: Some(
                "Validation failed for scalable target. Reason: Table events must be in \
                 PROVISIONED throughput mode"
                    .to_string(),
            ),
        },
        Row {
            case: "a table that does not exist in a keyspace that does",
            setup: Setup::KeyspaceOnly,
            keyspace: "reports_ks",
            table: "never_created",
            dimension: "cassandra:table:WriteCapacityUnits",
            refusal: Some(
                "Validation failed for resource: keyspace/reports_ks/table/never_created, \
                 scalable dimension: cassandra:table:WriteCapacityUnits. Reason: Table \
                 never_created does not exist in keyspace reports_ks"
                    .to_string(),
            ),
        },
        Row {
            case: "a keyspace that does not exist at all",
            setup: Setup::Nothing,
            keyspace: "missing_ks",
            table: "orders",
            dimension: "cassandra:table:ReadCapacityUnits",
            refusal: Some(
                "Validation failed for resource: keyspace/missing_ks/table/orders, scalable \
                 dimension: cassandra:table:ReadCapacityUnits. Reason: Table orders does not \
                 exist in keyspace missing_ks"
                    .to_string(),
            ),
        },
    ]
}

#[tokio::test]
async fn a_cassandra_target_is_registered_only_on_a_provisioned_table_that_exists() {
    for row in rows() {
        let router = router();
        let case = row.case;
        match row.setup {
            Setup::Provisioned => {
                given_table(&router, row.keyspace, row.table, "PROVISIONED").await;
            }
            Setup::OnDemand => {
                given_table(&router, row.keyspace, row.table, "PAY_PER_REQUEST").await;
            }
            Setup::KeyspaceOnly => {
                let (status, body) = keyspaces(
                    &router,
                    "CreateKeyspace",
                    json!({"keyspaceName": row.keyspace}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected CreateKeyspace to answer 200, got {status} {body:?}"
                );
            }
            Setup::Nothing => {}
        }

        let resource_id = format!("keyspace/{}/table/{}", row.keyspace, row.table);
        let (status, body) = application_auto_scaling(
            &router,
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "cassandra",
                "ResourceId": resource_id,
                "ScalableDimension": row.dimension,
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;

        match &row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the registration to be accepted, got {status} \
                     {body:?}"
                );
                // Backed by real state: the target is reported by the service that holds it.
                let (status, described) = application_auto_scaling(
                    &router,
                    "DescribeScalableTargets",
                    json!({
                        "ServiceNamespace": "cassandra",
                        "ResourceIds": [resource_id],
                        "ScalableDimension": row.dimension,
                    }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DescribeScalableTargets to answer 200, got \
                     {status} {described:?}"
                );
                let targets = described["ScalableTargets"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    targets.len(),
                    1,
                    "case {case:?}: expected the registered target to be reported, got \
                     {targets:?}"
                );
                assert_eq!(
                    (
                        targets[0]["MinCapacity"].clone(),
                        targets[0]["MaxCapacity"].clone()
                    ),
                    (json!(1), json!(10)),
                    "case {case:?}: expected the registered window, got {:?}",
                    targets[0]
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body:?}"
                );
                let message = error_message(&body);
                assert_eq!(
                    &message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                // A refused registration mints nothing: no target is reported for it.
                let (_, described) = application_auto_scaling(
                    &router,
                    "DescribeScalableTargets",
                    json!({"ServiceNamespace": "cassandra", "ResourceIds": [resource_id]}),
                )
                .await;
                let targets = described["ScalableTargets"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    targets.is_empty(),
                    "case {case:?}: expected a refused registration to register nothing, got \
                     {targets:?}"
                );
            }
        }
    }
}

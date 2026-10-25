// SPDX-License-Identifier: CC0-1.0

//! `RestartConnector`, end to end over HTTP: what a restart changes, and what it leaves
//! exactly as it was.
//!
//! RULE. A restart runs against a STORED connector and touches nothing else: it records
//! exactly one `RESTART_CONNECTOR` connector operation against the connector its
//! `connectorArn` names and moves that one connector's `connectorState` for as long as
//! the operation runs, while every other member `DescribeConnector` reports for it — and
//! every other connector in the store — reads exactly as it did before the call. The
//! connector is returned to its earlier state by the CLOCK alone. A `connectorArn` that
//! names no stored connector is the operation's modelled 404 and records nothing at all.
//!
//! That is the restart half of the asynchronous-work rule the suite in
//! `connector_operations.rs` pins from the operation's side: there, the record a restart
//! stores and the two operations that report it; here, the scope of the write a restart
//! performs.
//!
//! Every case drives the emulator the way a client does — a buffered `restJson1` request
//! through the same router the production binary assembles — so a handler that answered
//! from the request, or that wrote more of the store than a restart may, would fail here.
//!
//! The table spans what the rule generalises over: the two capacity forms a connector can
//! be created in, a connector with and without a description, and each form
//! `RestartConnector$onlyFailedTasks` takes (absent, false, true). The 404 table spans
//! every form of a `connectorArn` this emulator holds no connector of, including one it
//! held until a delete finished.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    RESTART_PROGRESSION, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN must
/// carry. No authenticator is registered, so the signature is never checked; the scope is
/// what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Long enough that one change the clock owes after a call is due. The span is the
/// service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Long enough that a restart's whole progression is due: one span per state it passes
/// through, both of them the service's own constants.
async fn past_the_whole_restart() {
    let steps = u32::try_from(RESTART_PROGRESSION.len()).expect("a small progression");
    tokio::time::sleep(TRANSITION_SPAN * steps + Duration::from_millis(400)).await;
}

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS
/// SDK encodes a label: every character a path segment reserves (including the ARN's own
/// `:` and `/`) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `body` without its `connectorState`: everything a restart may NOT change.
fn without_state(body: &Value) -> Value {
    let mut stripped = body.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("connectorState");
    }
    stripped
}

/// Create one ACTIVE custom plugin and return its ARN.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/custom-plugins",
        json!({
            "name": name,
            "contentType": "ZIP",
            "location": {"s3Location": {
                "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
                "fileKey": format!("plugins/{name}.zip"),
            }},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    past_one_transition().await;
    text(&created, "customPluginArn")
}

/// One row of the restart table: the connector's form and the `onlyFailedTasks` the
/// restart asks for.
///
/// `RestartConnector$onlyFailedTasks` is modelled `__boolean` with `default false`, so it
/// is spanned by the three forms it takes on the wire — absent, explicitly false, and
/// true — and the connector it runs against is spanned by the two forms
/// `CreateConnectorRequest$capacity` takes (`provisionedCapacity` and `autoScaling`,
/// members of the `Capacity` union) with its optional `connectorDescription` present and
/// absent.
#[derive(Debug)]
struct Row {
    name: &'static str,
    capacity: Value,
    description: Option<&'static str>,
    only_failed_tasks: Option<bool>,
}

/// The table, built here rather than as a `const` because each row carries a JSON value.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "scope-provisioned",
            // `mcuCount` takes one of the documented values 1, 2, 4, 8.
            capacity: json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}}),
            description: None,
            only_failed_tasks: None,
        },
        Row {
            name: "scope-autoscaled",
            capacity: json!({"autoScaling": {
                "mcuCount": 2,
                "minWorkerCount": 1,
                "maxWorkerCount": 4,
            }}),
            description: Some("the autoscaled connector"),
            only_failed_tasks: Some(false),
        },
        Row {
            name: "scope-provisioned-wide",
            capacity: json!({"provisionedCapacity": {"mcuCount": 4, "workerCount": 3}}),
            description: Some("the wide connector"),
            only_failed_tasks: Some(true),
        },
    ]
}

/// The `CreateConnector` body for `row`, referencing `plugin_arn`.
fn connector_request(row: &Row, plugin_arn: &str) -> Value {
    let mut body = json!({
        "connectorName": row.name,
        "capacity": row.capacity,
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{}.kafka.{REGION}.amazonaws.com:9092", row.name),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{}-role", row.name),
    });
    if let Some(description) = row.description {
        body["connectorDescription"] = json!(description);
    }
    body
}

/// Create the connector `row` describes and return its ARN.
async fn create_connector(
    emulator: &MultiServiceRouter,
    row: &Row,
    plugin_arn: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/connectors",
        connector_request(row, plugin_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateConnector must succeed, got {status} {created}",
        row.name
    );
    text(&created, "connectorArn")
}

/// `DescribeConnector`'s whole answer for `connector_arn`.
async fn describe(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnector must succeed, got {status} {described}"
    );
    described
}

/// `RestartConnector` against `connector_arn`, with `only_failed_tasks` as the query
/// parameter when the row names one.
async fn restart(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    only_failed_tasks: Option<bool>,
) -> (u16, Value) {
    let mut path = format!("/v1/connectors/{}/restart", label(connector_arn));
    if let Some(only_failed_tasks) = only_failed_tasks {
        path.push_str(&format!("?onlyFailedTasks={only_failed_tasks}"));
    }
    call(emulator, "POST", &path, Value::Null).await
}

/// The summaries `ListConnectorOperations` reports for `connector_arn`.
async fn operations(emulator: &MultiServiceRouter, connector_arn: &str) -> Vec<Value> {
    let (status, listed) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}/operations", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListConnectorOperations must succeed, got {status} {listed}"
    );
    listed["connectorOperations"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
        .clone()
}

#[tokio::test]
async fn a_restart_moves_only_the_named_connectors_state_and_records_one_operation() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "restart-scope-plugin").await;
    let rows = rows();

    let mut arns = Vec::new();
    for row in &rows {
        arns.push(create_connector(&emulator, row, &plugin_arn).await);
    }
    past_one_transition().await;

    // How every connector reads before any restart: the answer each one must go back to.
    let mut before = Vec::new();
    for (row, arn) in rows.iter().zip(&arns) {
        let described = describe(&emulator, arn).await;
        let state = text(&described, "connectorState");
        assert_eq!(
            state, "RUNNING",
            "case {:?}: the clock must bring a created connector to RUNNING, got {state:?}",
            row.name
        );
        before.push(described);
    }

    for (index, (row, arn)) in rows.iter().zip(&arns).enumerate() {
        let (status, restarted) = restart(&emulator, arn, row.only_failed_tasks).await;
        assert_eq!(
            status, 200,
            "case {:?}: RestartConnector must succeed, got {status} {restarted}",
            row.name
        );
        assert_eq!(
            text(&restarted, "connectorArn"),
            *arn,
            "case {:?}: the answer must name the connector that was restarted, got {restarted}",
            row.name
        );
        let operation_arn = text(&restarted, "connectorOperationArn");
        assert!(
            !operation_arn.is_empty(),
            "case {:?}: the answer must name the operation it recorded, got {restarted}",
            row.name
        );

        // While the restart runs: the state moved, and NOTHING else about the connector
        // did — not its capacity, its configuration, its version or its creation time.
        let during = describe(&emulator, arn).await;
        let state = text(&during, "connectorState");
        assert_eq!(
            state, "RESTARTING",
            "case {:?}: a restarting connector must read RESTARTING, got {state:?}",
            row.name
        );
        assert_eq!(
            without_state(&during),
            without_state(&before[index]),
            "case {:?}: a restart must leave every other reported member of the connector \
             unchanged, got {during}",
            row.name
        );

        // No other connector is touched by it: one not restarted yet still reads as it
        // did and holds no operation, and one already restarted holds exactly the one
        // operation its own restart recorded.
        for (other, (other_row, other_arn)) in rows.iter().zip(&arns).enumerate() {
            if other == index {
                continue;
            }
            let described = describe(&emulator, other_arn).await;
            assert_eq!(
                described, before[other],
                "case {:?}: restarting {:?} must not change another connector, got {described}",
                other_row.name, row.name
            );
            let listed = operations(&emulator, other_arn).await;
            let expected = usize::from(other < index);
            assert_eq!(
                listed.len(),
                expected,
                "case {:?}: restarting {:?} must record nothing against it, so it must hold \
                 {expected} operations, got {listed:?}",
                other_row.name,
                row.name
            );
        }

        past_the_whole_restart().await;

        // The clock alone brings it back, and it comes back as it was.
        let after = describe(&emulator, arn).await;
        assert_eq!(
            after, before[index],
            "case {:?}: a finished restart must leave the connector reading exactly as it \
             did, got {after}",
            row.name
        );

        let listed = operations(&emulator, arn).await;
        assert_eq!(
            listed.len(),
            1,
            "case {:?}: a restart must record exactly one operation, got {listed:?}",
            row.name
        );
        assert_eq!(
            text(&listed[0], "connectorOperationArn"),
            operation_arn,
            "case {:?}: the stored operation must be the one the answer named, got {listed:?}",
            row.name
        );
        assert_eq!(
            text(&listed[0], "connectorOperationType"),
            "RESTART_CONNECTOR",
            "case {:?}: a restart records a RESTART_CONNECTOR operation, got {listed:?}",
            row.name
        );
        assert_eq!(
            text(&listed[0], "connectorOperationState"),
            "RESTART_COMPLETE",
            "case {:?}: the clock must finish the restart, got {listed:?}",
            row.name
        );
    }
}

#[tokio::test]
async fn a_restart_of_a_connector_this_emulator_does_not_hold_is_the_modelled_not_found() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "restart-absent-plugin").await;

    let bystander_row = Row {
        name: "scope-bystander",
        capacity: json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 1}}),
        description: None,
        only_failed_tasks: None,
    };
    let gone_row = Row {
        name: "scope-gone",
        capacity: json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 1}}),
        description: None,
        only_failed_tasks: None,
    };
    let bystander = create_connector(&emulator, &bystander_row, &plugin_arn).await;
    let gone = create_connector(&emulator, &gone_row, &plugin_arn).await;
    past_one_transition().await;

    // A connector the store held until its delete finished is a connector the store does
    // not hold.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/connectors/{}", label(&gone)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteConnector must succeed, got {status} {deleted}"
    );
    past_one_transition().await;

    let absent: [(&str, String); 4] = [
        (
            "a connector ARN of the right shape that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/never-made/{}/1",
                "0".repeat(8)
            ),
        ),
        (
            "another service's ARN",
            format!("arn:aws:iam::{ACCOUNT}:role/not-a-connector"),
        ),
        ("a value that is no ARN at all", "not-an-arn".to_string()),
        ("a connector whose delete has finished", gone.clone()),
    ];

    for (case, connector_arn) in &absent {
        let (status, answered) = restart(&emulator, connector_arn, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: restarting a connector this emulator does not hold is a 404, got \
             {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        assert!(
            text(&answered, "message").contains("connectorArn"),
            "case {case:?}: the message must name the parameter it refused, got {answered}"
        );
    }

    // A refused restart writes nothing: the connector that IS stored is untouched, and
    // still takes a restart of its own.
    let state = text(&describe(&emulator, &bystander).await, "connectorState");
    assert_eq!(
        state, "RUNNING",
        "a refused restart must not move another connector, got {state:?}"
    );
    let listed = operations(&emulator, &bystander).await;
    assert!(
        listed.is_empty(),
        "a refused restart must record no operation anywhere, got {listed:?}"
    );

    let (status, restarted) = restart(&emulator, &bystander, None).await;
    assert_eq!(
        status, 200,
        "the stored connector must still restart, got {status} {restarted}"
    );
    let listed = operations(&emulator, &bystander).await;
    assert_eq!(
        listed.len(),
        1,
        "the accepted restart must record exactly one operation, got {listed:?}"
    );
}

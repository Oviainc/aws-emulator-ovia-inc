// SPDX-License-Identifier: CC0-1.0

//! The window a call against a connector keeps that connector out of `RUNNING`, end to
//! end over HTTP.
//!
//! RULE. The transient connector state a call against a connector moves it into and the
//! connector operation that same call records are two reports of ONE piece of
//! asynchronous work, so they run on ONE clock: the connector holds that transient state
//! for exactly as long as the operation is non-terminal, the clock returns it to
//! `RUNNING` at the same instant the operation reaches its terminal state and never
//! earlier, and a further call against that connector — which only a `RUNNING` connector
//! takes — is refused for as long as the operation runs and accepted once it has ended.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with the response body read
//! back as JSON. Nothing reaches into the state struct, so a handler answering from the
//! request instead of from state would fail here.
//!
//! The table spans what the rule generalises over — every call this service has that
//! starts a connector operation, each with its own progression length and its own
//! transient connector state: a restart (both `onlyFailedTasks` forms) and an update of
//! either of a connector's two updatable settings — and the whole life of each operation
//! is sampled on a grid finer than one transition, rather than only at the two ends of it.
//!
//! Each observation is ordered so that it holds whatever instant it happens to land on:
//! the state a read reports only ever moves forwards, so an operation read as ENDED
//! BEFORE a connector read means that connector must already be back to `RUNNING`, and a
//! connector read as `RUNNING` BEFORE an operation read means that operation must already
//! have ended.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    RESTART_PROGRESSION, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN must
/// carry. No authenticator is registered, so the signature is never checked; the scope
/// is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `ConnectorState` a connector that is serving its workers is in; one of the six
/// values `ConnectorState` lists (`smithy_query query="shape" shape="ConnectorState"`).
const RUNNING: &str = "RUNNING";

/// The most states any connector operation this suite starts passes through.
///
/// An update walks one state per step and `ConnectorOperationStepType` lists four steps
/// for an update of either kind — `INITIALIZE_UPDATE`, `VALIDATE_UPDATE`, the step
/// spelled the same as the operation's own type, and `FINALIZE_UPDATE`
/// (`smithy_query query="shape" shape="ConnectorOperationStepType"`) — while a restart
/// walks the two states of `RESTART_PROGRESSION`, which the test asserts against this
/// bound rather than trusting it.
const LONGEST_PROGRESSION: u32 = 4;

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

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{REGION}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// Create one ACTIVE custom plugin and return its ARN.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) =
        call(emulator, "POST", "/v1/custom-plugins", plugin_request(name)).await;
    assert_eq!(
        status, 200,
        "the plugin {name} must be created, got {status} {created}"
    );
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    text(&created, "customPluginArn")
}

/// Create the named connectors against `plugin_arn` and return their ARNs, in the same
/// order, once the clock has moved every one of them to `RUNNING`.
async fn running_connectors(
    emulator: &MultiServiceRouter,
    plugin_arn: &str,
    names: &[&str],
) -> Vec<String> {
    let mut arns = Vec::new();
    for name in names {
        let (status, created) = call(
            emulator,
            "POST",
            "/v1/connectors",
            connector_request(name, plugin_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnector must succeed, got {status} {created}"
        );
        arns.push(text(&created, "connectorArn"));
    }
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    arns
}

/// `DescribeConnector` for `connector_arn`.
async fn describe_connector(emulator: &MultiServiceRouter, connector_arn: &str) -> Value {
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

/// `DescribeConnectorOperation` for `operation_arn`.
async fn describe_operation(emulator: &MultiServiceRouter, operation_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectorOperations/{}", label(operation_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeConnectorOperation must succeed, got {status} {described}"
    );
    described
}

/// `RestartConnector` against `connector_arn`, with `only_failed_tasks` as the query
/// parameter when the case names one.
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

/// `UpdateConnector` against `connector_arn` at `current_version`, with `body` as the
/// request body.
async fn update(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    current_version: &str,
    body: Value,
) -> (u16, Value) {
    let path = format!(
        "/v1/connectors/{}?currentVersion={}",
        label(connector_arn),
        label(current_version)
    );
    call(emulator, "PUT", &path, body).await
}

/// How many operations `ListConnectorOperations` reports against `connector_arn`.
async fn operation_count(emulator: &MultiServiceRouter, connector_arn: &str) -> usize {
    let path = format!("/v1/connectors/{}/operations", label(connector_arn));
    let (status, listed) = call(emulator, "GET", &path, Value::Null).await;
    assert_eq!(
        status, 200,
        "ListConnectorOperations must succeed, got {status} {listed}"
    );
    listed["connectorOperations"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectorOperations array in {listed}"))
        .len()
}

/// Whether a described operation has reached its terminal state, which is exactly when
/// it reports an `endTime`: the member is modelled optional because an operation that
/// has not finished has no end.
fn has_ended(described: &Value) -> bool {
    described.get("endTime").is_some()
}

/// The call one row of the table makes against its connector to start an operation.
enum Start {
    /// `RestartConnector`, with `onlyFailedTasks` as the query parameter when the row
    /// names one.
    Restart(Option<bool>),
    /// `UpdateConnector` naming a new capacity (the connector's worker setting).
    Capacity,
    /// `UpdateConnector` naming the whole configuration the connector takes on.
    Configuration,
}

impl Start {
    /// Make this call against the connector `connector_arn` names, which is at
    /// `current_version`.
    async fn make(
        &self,
        emulator: &MultiServiceRouter,
        connector_arn: &str,
        current_version: &str,
    ) -> (u16, Value) {
        match self {
            Self::Restart(only_failed_tasks) => {
                restart(emulator, connector_arn, *only_failed_tasks).await
            }
            // An `mcuCount` of 2 is one of the values that member documents, and both
            // counts differ from the ones the connector was created with, so the update
            // is a real change of capacity.
            Self::Capacity => {
                update(
                    emulator,
                    connector_arn,
                    current_version,
                    json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}}),
                )
                .await
            }
            Self::Configuration => {
                update(
                    emulator,
                    connector_arn,
                    current_version,
                    json!({"connectorConfiguration": {
                        "connector.class": "io.confluent.connect.s3.S3SinkConnector",
                        "tasks.max": "4",
                    }}),
                )
                .await
            }
        }
    }
}

/// One row of the table: a call that starts a connector operation, the transient
/// `ConnectorState` it leaves the connector in, and the `ConnectorOperationState` that
/// operation ends in.
struct Case {
    connector: &'static str,
    start: Start,
    /// The `ConnectorState` the connector holds while this operation runs; one of the
    /// values `ConnectorState` lists.
    transient_state: &'static str,
    /// The `ConnectorOperationState` this kind of operation ends in: `RESTART_COMPLETE`
    /// for a restart and `UPDATE_COMPLETE` for an update, both from the value list of
    /// `ConnectorOperationState` (`smithy_query query="shape"
    /// shape="ConnectorOperationState"`).
    terminal_state: &'static str,
}

/// Every call this service has that starts a connector operation: a restart with
/// `onlyFailedTasks` absent and with it set, and an update of each of a connector's two
/// updatable settings. The two kinds differ in the length of their progression, which is
/// what the rule must hold across.
const CASES: &[Case] = &[
    Case {
        connector: "window-restart-default",
        start: Start::Restart(None),
        transient_state: "RESTARTING",
        terminal_state: "RESTART_COMPLETE",
    },
    Case {
        connector: "window-restart-failed-tasks",
        start: Start::Restart(Some(true)),
        transient_state: "RESTARTING",
        terminal_state: "RESTART_COMPLETE",
    },
    Case {
        connector: "window-update-capacity",
        start: Start::Capacity,
        transient_state: "UPDATING",
        terminal_state: "UPDATE_COMPLETE",
    },
    Case {
        connector: "window-update-configuration",
        start: Start::Configuration,
        transient_state: "UPDATING",
        terminal_state: "UPDATE_COMPLETE",
    },
];

/// One row of the table, with the connector and the operation its call started.
struct Started {
    case: &'static Case,
    connector_arn: String,
    operation_arn: String,
    /// Whether a further call against this connector has been accepted yet, which is
    /// what closes the row: it happens only once the operation has ended.
    served_again: bool,
}

impl Started {
    /// One observation of this row, at whatever instant it lands on.
    async fn sample(&mut self, emulator: &MultiServiceRouter, sample: u32) {
        let case = format!("{} sample {sample}", self.case.connector);

        // The operation first, then the connector: an operation that has ended stays
        // ended, so a connector read afterwards must already be back to RUNNING.
        let before = describe_operation(emulator, &self.operation_arn).await;
        let ended_before = has_ended(&before);
        let connector = describe_connector(emulator, &self.connector_arn).await;
        let connector_state = text(&connector, "connectorState");
        if ended_before {
            assert_eq!(
                connector_state, RUNNING,
                "case {case:?}: the clock returns the connector to RUNNING at the instant \
                 the operation reporting its work ends, and that operation had already \
                 ended ({before}), got {connector_state:?}"
            );
        }

        if connector_state == RUNNING {
            // The connector read RUNNING first, so the work that kept it out of RUNNING
            // must already be over by the time this read lands.
            let after = describe_operation(emulator, &self.operation_arn).await;
            assert!(
                has_ended(&after),
                "case {case:?}: the connector read RUNNING, so the operation reporting the \
                 work that kept it out of RUNNING must have ended, got {after}"
            );
            assert_eq!(
                text(&after, "connectorOperationState"),
                self.case.terminal_state,
                "case {case:?}: an ended operation reports its modelled terminal state, \
                 got {after}"
            );
        } else {
            assert_eq!(
                connector_state, self.case.transient_state,
                "case {case:?}: a connector whose operation is still running holds the \
                 state its call put it in, got {connector_state:?}"
            );
        }

        // Work against one connector is serialized: a further call is refused for as
        // long as this operation runs, and accepted once it has ended. The probe is a
        // restart for every row, because it is the call that needs nothing but the
        // connector's ARN.
        let (status, answered) = restart(emulator, &self.connector_arn, None).await;
        match status {
            200 => {
                // The probe was served, so the connector was RUNNING when it landed:
                // the operation it waited on must be over by this later read.
                let after = describe_operation(emulator, &self.operation_arn).await;
                assert!(
                    has_ended(&after),
                    "case {case:?}: a further restart was accepted, so the operation that \
                     was running must have ended, got {after}"
                );
                self.served_again = true;
            }
            400 => {
                assert!(
                    error_code(&answered).contains("BadRequestException"),
                    "case {case:?}: a call against a connector that is not RUNNING is a \
                     BadRequestException, got {answered}"
                );
                assert!(
                    text(&answered, "message").contains("connectorArn"),
                    "case {case:?}: the refusal must name the parameter it blamed, got \
                     {answered}"
                );
                let count = operation_count(emulator, &self.connector_arn).await;
                assert_eq!(
                    count, 1,
                    "case {case:?}: a refused call records no second operation, got {count}"
                );
            }
            other => panic!(
                "case {case:?}: a further call is either served or refused as a bad \
                 request, got {other} {answered}"
            ),
        }
    }
}

#[tokio::test]
async fn a_connector_leaves_its_transient_state_exactly_when_its_operation_ends() {
    let restart_states = u32::try_from(RESTART_PROGRESSION.len()).expect("a small progression");
    assert!(
        restart_states <= LONGEST_PROGRESSION,
        "the sampling grid must cover a restart's whole progression: {restart_states} \
         states against a bound of {LONGEST_PROGRESSION}"
    );

    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "window-plugin").await;
    let names: Vec<&str> = CASES.iter().map(|case| case.connector).collect();
    let connector_arns = running_connectors(&emulator, &plugin_arn, &names).await;

    // Every row's call is made first, so the whole table is sampled on one grid.
    let mut started: Vec<Started> = Vec::new();
    for (case, connector_arn) in CASES.iter().zip(&connector_arns) {
        let connector = describe_connector(&emulator, connector_arn).await;
        assert_eq!(
            text(&connector, "connectorState"),
            RUNNING,
            "case {:?}: the connector must be RUNNING before the call, got {connector}",
            case.connector
        );
        let (status, answered) = case
            .start
            .make(&emulator, connector_arn, &text(&connector, "currentVersion"))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: the call against a RUNNING connector must be served, got {status} \
             {answered}",
            case.connector
        );
        let operation_arn = text(&answered, "connectorOperationArn");
        assert!(
            !operation_arn.is_empty(),
            "case {:?}: the answer must name the operation it recorded, got {answered}",
            case.connector
        );
        started.push(Started {
            case,
            connector_arn: connector_arn.clone(),
            operation_arn,
            served_again: false,
        });
    }

    // Twice per transition, so a sample lands inside every state an operation passes
    // through — the window the two clocks used to disagree in is half this wide — and on
    // past the last state's due instant.
    let interval = TRANSITION_SPAN / 2;
    let samples = 2 * LONGEST_PROGRESSION + 4;
    for sample in 0..samples {
        for row in started.iter_mut().filter(|row| !row.served_again) {
            row.sample(&emulator, sample).await;
        }
        tokio::time::sleep(interval).await;
    }

    for row in &started {
        assert!(
            row.served_again,
            "case {:?}: the clock must end the operation and return the connector to \
             RUNNING within {samples} samples, after which a further call is served",
            row.case.connector
        );
    }
}

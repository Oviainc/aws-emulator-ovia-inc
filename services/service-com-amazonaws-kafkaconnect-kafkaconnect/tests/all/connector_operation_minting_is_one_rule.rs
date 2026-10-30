// SPDX-License-Identifier: CC0-1.0

//! The rule that mints a connector operation, driven end to end through BOTH
//! operations that start one: `RestartConnector` and `UpdateConnector`.
//!
//! RULE. An operation started against a connector is reported under an ARN
//! derived from that connector's OWN ARN — the connector ARN's scope (partition,
//! region and account) with its `connector/<name>/<id>` resource re-typed
//! `connector-operation/<name>/<id>` and the operation's own minted id appended
//! — whichever call started it, and each call mints a fresh id, so two
//! operations of one connector are two distinct ARNs that both read back through
//! `DescribeConnectorOperation`.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, with the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler minting an ARN of its own shape would fail here.
//!
//! The table spans what the rule generalises over: which call starts the
//! operation (a restart, an update of the capacity, an update of the
//! configuration), and, for a restart, each form `onlyFailedTasks` takes. The
//! last case starts one of each KIND against a single connector, which is what
//! pins that the two paths mint for one store alike.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs. The ARN the rule states is
//! the exception: it comes from this crate's one grammar-derived statement of it
//! (`super::connector_operation_arn_rule`), shared with the suite that spans the
//! connector names a name may carry an ARN separator in.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{
    CONNECTOR_OPERATION_RESTART, CONNECTOR_OPERATION_UPDATE_CONFIGURATION,
    CONNECTOR_OPERATION_UPDATE_WORKER_SETTING, RESTART_PROGRESSION, TRANSITION_SPAN,
};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

use super::connector_operation_arn_rule::{derived_operation_arn, last_segment};

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// How many steps the longest progression either call starts walks, so one wait
/// covers a restart as well as an update. Taken from the service's own
/// progression rather than a literal.
fn longest_progression_steps() -> u32 {
    let restart = u32::try_from(RESTART_PROGRESSION.len()).expect("a small progression");
    // An update walks `INITIALIZE_UPDATE`, `VALIDATE_UPDATE`, its own step and
    // `FINALIZE_UPDATE`: four steps, as `update_step_types` composes them.
    restart.max(4)
}

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Long enough that whatever either call started has reached its terminal state
/// and the connector is running again.
async fn past_the_whole_operation() {
    tokio::time::sleep(TRANSITION_SPAN * longest_progression_steps() + Duration::from_millis(500))
        .await;
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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A member of a JSON object as a string, or `""`.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `value` is the UUID AWS gives the random part of a kafkaconnect
/// resource ARN: the 8-4-4-4-12 lowercase-hex form, with RFC 4122's version (4)
/// and variant (8..b) nibbles.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(want, group)| group.len() == *want)
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        && groups[2].starts_with('4')
        && matches!(groups[3].as_bytes().first(), Some(b'8' | b'9' | b'a' | b'b'))
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
    past_one_transition().await;
    text(&created, "customPluginArn")
}

/// Create the named connectors against `plugin_arn` and return their ARNs, in
/// the same order, once the clock has moved every one of them to `RUNNING`.
async fn running_connectors(
    emulator: &MultiServiceRouter,
    plugin_arn: &str,
    names: &[String],
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
    past_one_transition().await;
    arns
}

/// `DescribeConnector` for `connector_arn`.
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

/// `DescribeConnectorOperation` for `operation_arn`.
async fn describe_operation(emulator: &MultiServiceRouter, operation_arn: &str) -> (u16, Value) {
    let path = format!("/v1/connectorOperations/{}", label(operation_arn));
    call(emulator, "GET", &path, Value::Null).await
}

/// Which call starts the operation a case drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// `RestartConnector`, with `onlyFailedTasks` as the case names it.
    Restart(Option<bool>),
    /// `UpdateConnector` naming a new `capacity`.
    UpdateCapacity,
    /// `UpdateConnector` naming a new `connectorConfiguration`.
    UpdateConfiguration,
}

impl Start {
    /// The operation type the started operation is reported as, from the
    /// service's own `ConnectorOperationType` constants.
    fn operation_type(self) -> &'static str {
        match self {
            Self::Restart(_) => CONNECTOR_OPERATION_RESTART,
            Self::UpdateCapacity => CONNECTOR_OPERATION_UPDATE_WORKER_SETTING,
            Self::UpdateConfiguration => CONNECTOR_OPERATION_UPDATE_CONFIGURATION,
        }
    }
}

/// Start one operation against `connector_arn` the way `start` names, and return
/// the `(status, body)` the call answered with. An update reads the connector's
/// current version first, since that is what the operation requires.
async fn start_operation(
    emulator: &MultiServiceRouter,
    connector_arn: &str,
    start: Start,
) -> (u16, Value) {
    match start {
        Start::Restart(only_failed_tasks) => {
            let mut path = format!("/v1/connectors/{}/restart", label(connector_arn));
            if let Some(only_failed_tasks) = only_failed_tasks {
                path.push_str(&format!("?onlyFailedTasks={only_failed_tasks}"));
            }
            call(emulator, "POST", &path, Value::Null).await
        }
        Start::UpdateCapacity | Start::UpdateConfiguration => {
            let version = text(&describe(emulator, connector_arn).await, "currentVersion");
            let body = if start == Start::UpdateCapacity {
                json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}})
            } else {
                json!({"connectorConfiguration": {
                    "connector.class": "io.confluent.connect.s3.S3SinkConnector",
                    "tasks.max": "4",
                }})
            };
            let path = format!(
                "/v1/connectors/{}?currentVersion={}",
                label(connector_arn),
                label(&version)
            );
            call(emulator, "PUT", &path, body).await
        }
    }
}

/// Start one operation and check the rule on the ARN it answers with: it is the
/// one derived from the connector's own ARN, its appended id is a freshly minted
/// UUID, and the stored operation reads back under it. Returns that ARN.
async fn minted_operation_arn(
    emulator: &MultiServiceRouter,
    case: &str,
    connector_arn: &str,
    start: Start,
) -> String {
    let (status, answer) = start_operation(emulator, connector_arn, start).await;
    assert_eq!(
        status, 200,
        "case {case:?}: starting the operation must succeed, got {status} {answer}"
    );
    assert_eq!(
        text(&answer, "connectorArn"),
        connector_arn,
        "case {case:?}: the answer must name the connector it acted on, got {answer}"
    );

    let operation_arn = text(&answer, "connectorOperationArn");
    let id = last_segment(&operation_arn).to_string();
    assert!(
        is_uuid(&id),
        "case {case:?}: expected a minted UUID as the operation's id, got {id:?} in \
         {operation_arn:?}"
    );
    assert_eq!(
        operation_arn,
        derived_operation_arn(case, connector_arn, &id),
        "case {case:?}: expected the operation ARN derived from {connector_arn:?}, got \
         {operation_arn:?}"
    );

    let (status, described) = describe_operation(emulator, &operation_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the minted ARN must read back, got {status} {described}"
    );
    assert_eq!(
        text(&described, "connectorArn"),
        connector_arn,
        "case {case:?}: the stored operation must name its connector, got {described}"
    );
    assert_eq!(
        text(&described, "connectorOperationType"),
        start.operation_type(),
        "case {case:?}: expected a {} operation, got {described}",
        start.operation_type()
    );
    operation_arn
}

/// Every way an operation is started, each on its own connector.
const CASES: &[(&str, Start)] = &[
    ("a restart naming no onlyFailedTasks", Start::Restart(None)),
    ("a restart of all tasks", Start::Restart(Some(false))),
    ("a restart of the failed tasks", Start::Restart(Some(true))),
    ("an update of the capacity", Start::UpdateCapacity),
    ("an update of the configuration", Start::UpdateConfiguration),
];

#[tokio::test]
async fn every_call_that_starts_a_connector_operation_mints_one_arn_shape() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "operation-minting-plugin").await;
    let names: Vec<String> = (0..CASES.len())
        .map(|index| format!("operation-minting-{index}"))
        .collect();
    let connector_arns = running_connectors(&emulator, &plugin_arn, &names).await;

    let mut minted = BTreeSet::new();
    for ((case, start), connector_arn) in CASES.iter().zip(&connector_arns) {
        let operation_arn = minted_operation_arn(&emulator, case, connector_arn, *start).await;
        // The ARN also carries the scope and the account the connector lives in,
        // which is what deriving it from the connector's ARN guarantees.
        let prefix = format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector-operation/");
        assert!(
            operation_arn.starts_with(&prefix),
            "case {case:?}: expected a connectorOperationArn starting {prefix}, got \
             {operation_arn:?}"
        );
        assert!(
            minted.insert(operation_arn.clone()),
            "case {case:?}: every call must mint its own operation ARN, got {operation_arn:?} \
             twice"
        );
    }
}

#[tokio::test]
async fn a_restart_and_an_update_of_one_connector_mint_under_one_prefix() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "one-store-plugin").await;
    let names = vec!["one-store-connector".to_string()];
    let connector_arn = running_connectors(&emulator, &plugin_arn, &names)
        .await
        .pop()
        .expect("the connector was created");

    // One operation from each of the two paths, against the SAME connector in
    // the SAME store: the restart first, then the update once the clock has
    // brought the connector back to RUNNING.
    let restarted = minted_operation_arn(
        &emulator,
        "the restart path",
        &connector_arn,
        Start::Restart(None),
    )
    .await;
    past_the_whole_operation().await;
    let updated = minted_operation_arn(
        &emulator,
        "the update path",
        &connector_arn,
        Start::UpdateCapacity,
    )
    .await;

    let restart_prefix = restarted
        .strip_suffix(last_segment(&restarted))
        .expect("the ARN ends with its id");
    let update_prefix = updated
        .strip_suffix(last_segment(&updated))
        .expect("the ARN ends with its id");
    assert_eq!(
        restart_prefix, update_prefix,
        "both paths must name one connector's operations alike, got {restarted:?} and {updated:?}"
    );
    assert_ne!(
        restarted, updated,
        "each call must mint its own id, got {restarted:?} twice"
    );

    // Both are stored side by side: the second did not replace the first.
    for (case, operation_arn) in [("the restart", &restarted), ("the update", &updated)] {
        let (status, described) = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation_arn:?} must still read back, got {status} {described}"
        );
    }
}

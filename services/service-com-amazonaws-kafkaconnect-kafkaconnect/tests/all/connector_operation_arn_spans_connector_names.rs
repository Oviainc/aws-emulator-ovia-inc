// SPDX-License-Identifier: CC0-1.0

//! The ARN a started connector operation is reported under, over the whole set
//! of connector names the model admits.
//!
//! RULE. An operation started against a connector is reported under an ARN that
//! keeps the connector ARN's own five leading fields (`arn`, partition, vendor,
//! region, account) and carries, as its whole resource field, that connector's
//! resource field with its resource TYPE replaced by `connector-operation` and
//! the operation's own minted id appended. A resource NAME is caller-supplied
//! text — `CreateConnectorRequest$connectorName` is `__stringMin1Max128`, a
//! `@length` of 1..128 with no `@pattern` (`smithy_query` query="operation"
//! operation="CreateConnector"), so a name may carry `:` and `/` and even the
//! `:connector/` sequence the type boundary is written with — and no text
//! inside the name is a field or type boundary.
//!
//! The table spans what the rule generalises over: where the ARN's own
//! separators appear INSIDE the name (neither, a `/`, a `:`, a whole
//! `:connector/`, that sequence twice, a `:connector-operation/`), and the
//! name's size at both published bounds of its `@length` (1 and 128). Each row
//! is driven through BOTH calls that start an operation, `RestartConnector` and
//! `UpdateConnector`, and the minted ARN is read back through
//! `DescribeConnectorOperation`.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles. Nothing
//! reaches into the state struct, and the expectation comes from this crate's
//! one grammar-derived statement of the rule
//! (`super::connector_operation_arn_rule`) rather than from the service's own
//! derivation, so a handler splitting an ARN on text that may appear in a name
//! fails here.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

use super::connector_operation_arn_rule::{
    derived_operation_arn, last_segment, scope_and_resource, CONNECTOR_TYPE, OPERATION_TYPE,
};

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The published bounds of `connectorName`'s `@length`, from
/// `__stringMin1Max128` (`smithy_query` query="operation"
/// operation="CreateConnector"): a name outside them is not a name the model
/// admits.
const NAME_LENGTH: (usize, usize) = (1, 128);

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
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

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": "arn:aws:s3:::plugin-bucket",
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`. Only the
/// NAME varies per case: every other member is the same valid value, so a case
/// differs from its neighbours in exactly what the rule generalises over.
fn connector_request(name: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.cluster.kafka.{REGION}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/connector-role"),
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

/// `DescribeConnector` for `connector_arn`.
async fn describe(emulator: &MultiServiceRouter, case: &str, connector_arn: &str) -> Value {
    let (status, described) = call(
        emulator,
        "GET",
        &format!("/v1/connectors/{}", label(connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeConnector must succeed, got {status} {described}"
    );
    described
}

/// Which call starts the operation a row is driven through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// `RestartConnector`.
    Restart,
    /// `UpdateConnector` naming a new `capacity`.
    UpdateCapacity,
}

/// Start one operation against `connector_arn` and return `(status, body)`. An
/// update reads the connector's current version first, since the operation
/// requires it.
async fn start_operation(
    emulator: &MultiServiceRouter,
    case: &str,
    connector_arn: &str,
    start: Start,
) -> (u16, Value) {
    match start {
        Start::Restart => {
            let path = format!("/v1/connectors/{}/restart", label(connector_arn));
            call(emulator, "POST", &path, Value::Null).await
        }
        Start::UpdateCapacity => {
            let version = text(&describe(emulator, case, connector_arn).await, "currentVersion");
            let path = format!(
                "/v1/connectors/{}?currentVersion={}",
                label(connector_arn),
                label(&version)
            );
            let body =
                json!({"capacity": {"provisionedCapacity": {"mcuCount": 2, "workerCount": 3}}});
            call(emulator, "PUT", &path, body).await
        }
    }
}

/// The names the table spans: where the ARN's own separators fall inside the
/// name, and the name's size at each published bound of `connectorName`'s
/// `@length`.
fn case_names() -> Vec<(String, String)> {
    let (shortest, longest) = NAME_LENGTH;
    // A longest admitted name that also carries the type boundary: the
    // separator sequence with `l`s before it and `z`s after it, padded to the
    // published maximum.
    let padding = longest - ":connector/".len();
    let longest_name = format!(
        "{}:connector/{}",
        "l".repeat(padding / 2),
        "z".repeat(padding - padding / 2)
    );
    assert_eq!(
        longest_name.chars().count(),
        longest,
        "the longest case must sit on the published bound"
    );
    vec![
        ("no separator in the name", "plain-connector".to_string()),
        ("a slash in the name", "slash/in-name".to_string()),
        ("a colon in the name", "colon:in-name".to_string()),
        ("the type boundary in the name", "a:connector/b".to_string()),
        (
            "the type boundary twice in the name",
            "a:connector/b:connector/c".to_string(),
        ),
        (
            "the operation type in the name",
            "lead:connector-operation/tail".to_string(),
        ),
        ("the shortest admitted name", ":".repeat(shortest)),
        ("the longest admitted name", longest_name),
    ]
    .into_iter()
    .map(|(case, name)| (case.to_string(), name))
    .collect()
}

/// Create every case's connector, wait once for them all to reach `RUNNING`,
/// then start one operation of `start` per connector and check the rule on the
/// ARN it is reported under.
async fn every_name_mints_the_derived_arn(start: Start) {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "operation-arn-plugin").await;

    let cases = case_names();
    let mut connectors = Vec::new();
    for (case, name) in &cases {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/connectors",
            connector_request(name, &plugin_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConnector must admit the name {name:?}, got {status} {created}"
        );
        let connector_arn = text(&created, "connectorArn");
        // The name reaches the connector's ARN verbatim, which is what makes a
        // separator inside it indistinguishable from the ARN's own.
        let (_, resource) = scope_and_resource(case, &connector_arn);
        let prefix = format!("{CONNECTOR_TYPE}/{name}/");
        assert!(
            resource.starts_with(&prefix),
            "case {case:?}: expected a connector resource starting {prefix:?}, got {resource:?}"
        );
        connectors.push(connector_arn);
    }
    past_one_transition().await;

    for ((case, name), connector_arn) in cases.iter().zip(&connectors) {
        let (status, answer) = start_operation(&emulator, case, connector_arn, start).await;
        assert_eq!(
            status, 200,
            "case {case:?}: starting the operation must succeed, got {status} {answer}"
        );
        let operation_arn = text(&answer, "connectorOperationArn");
        let operation_id = last_segment(&operation_arn).to_string();

        assert_eq!(
            operation_arn,
            derived_operation_arn(case, connector_arn, &operation_id),
            "case {case:?}: expected the operation ARN derived from {connector_arn:?}, got \
             {operation_arn:?}"
        );

        // The ARN's own fields, read under the grammar: the connector's scope
        // unchanged, the resource re-typed, and the name still whole inside it.
        let (connector_scope, _) = scope_and_resource(case, connector_arn);
        let (operation_scope, operation_resource) = scope_and_resource(case, &operation_arn);
        assert_eq!(
            operation_scope, connector_scope,
            "case {case:?}: expected the connector's own scope, got {operation_arn:?}"
        );
        let expected_prefix = format!("{OPERATION_TYPE}/{name}/");
        assert!(
            operation_resource.starts_with(&expected_prefix),
            "case {case:?}: expected a resource starting {expected_prefix:?}, got \
             {operation_resource:?}"
        );
        assert_eq!(
            operation_resource.split(['/', ':']).next(),
            Some(OPERATION_TYPE),
            "case {case:?}: expected the resource type {OPERATION_TYPE}, got \
             {operation_resource:?}"
        );

        // The minted ARN is the operation's identity, so it reads back.
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/connectorOperations/{}", label(&operation_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation_arn:?} must read back, got {status} {described}"
        );
        assert_eq!(
            text(&described, "connectorOperationArn"),
            operation_arn,
            "case {case:?}: the stored operation must report its own ARN, got {described}"
        );
        assert_eq!(
            text(&described, "connectorArn"),
            *connector_arn,
            "case {case:?}: the stored operation must name its connector, got {described}"
        );
    }
}

#[tokio::test]
async fn a_restart_derives_the_operation_arn_for_every_admitted_connector_name() {
    every_name_mints_the_derived_arn(Start::Restart).await;
}

#[tokio::test]
async fn an_update_derives_the_operation_arn_for_every_admitted_connector_name() {
    every_name_mints_the_derived_arn(Start::UpdateCapacity).await;
}

#[tokio::test]
async fn a_name_past_the_published_length_bound_is_refused_and_mints_nothing() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "operation-arn-bound-plugin").await;
    let (_, longest) = NAME_LENGTH;
    // One character past the published maximum, carrying the type boundary the
    // admitted cases carry, so the only difference is the length.
    let too_long = format!(
        "a:connector/{}",
        "z".repeat(longest + 1 - "a:connector/".len())
    );
    let (status, refused) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_request(&too_long, &plugin_arn),
    )
    .await;
    assert_eq!(
        status, 400,
        "a name of {} characters is past the bound of {longest} and must be refused, got {status} \
         {refused}",
        too_long.chars().count()
    );
}

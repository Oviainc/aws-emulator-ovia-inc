// SPDX-License-Identifier: CC0-1.0

//! Every kafkaconnect timestamp member on the wire, and in the schema the codec reads it
//! from.
//!
//! RULE. Every member of a kafkaconnect response whose target carries
//! `@timestampFormat("date-time")` — in this model the `timestamp` shape
//! `com.amazonaws.kafkaconnect#__timestampIso8601`, which every timestamp member of every
//! shape targets — is rendered as an RFC 3339 / ISO-8601 STRING, never as a JSON number.
//! The INSTANT is environment-specific and is not part of the contract; only the form is.
//!
//! The first suite observes the rule the way a client does: a buffered `restJson1` request
//! through the same router the production binary assembles, with every response body walked
//! RECURSIVELY so a timestamp nested under `latestRevision` or inside a list element is
//! checked as well as a top-level one. Its table spans the class the rule generalises over
//! — three resource kinds (worker configuration, custom plugin, connector), the create,
//! describe and list paths of each, and rows differing in which optional members they carry
//! — rather than the single `CreateWorkerConfiguration` call the issue cited.
//!
//! The second suite pins the same property one level down, at the member schema the JSON
//! codec actually looks the format up on (`vendor/smithy-schema-server/src/json.rs::
//! write_timestamp` -> `Schema::timestamp_format()`). That covers the shapes whose
//! operations are still 501 stubs (`ListWorkerConfigurations`) and so cannot be reached
//! over HTTP yet, and it fails loudly if a shape ever goes back to a macro-built schema
//! that drops the trait.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::ops::types::*;
use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The member names this service renders as a `date-time`: every member of every shape in
/// the model whose target is `com.amazonaws.kafkaconnect#__timestampIso8601`
/// (`smithy_query(query="shape", shape="com.amazonaws.kafkaconnect#__timestampIso8601")`
/// reports `"timestampFormat": "date-time"`). No other member of this model carries either
/// name, so a body key of one of these names is always a modelled timestamp.
const DATE_TIME_MEMBERS: &[&str] = &["creationTime", "endTime"];

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

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every modelled timestamp in `body`, keyed by its JSON path, with the instant each one
/// names — after asserting that each is a `date-time` STRING and not a number.
///
/// The walk is recursive, so `$.latestRevision.creationTime` and
/// `$.customPlugins[0].latestRevision.creationTime` are checked exactly like
/// `$.creationTime`.
fn timestamps_of(case: &str, body: &Value) -> BTreeMap<String, f64> {
    let mut found = BTreeMap::new();
    walk(case, "$", body, &mut found);
    found
}

fn walk(case: &str, path: &str, value: &Value, found: &mut BTreeMap<String, f64>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                let child = format!("{path}.{name}");
                if DATE_TIME_MEMBERS.contains(&name.as_str()) {
                    found.insert(child.clone(), date_time_at(case, &child, member));
                } else {
                    walk(case, &child, member, found);
                }
            }
        }
        Value::Array(elements) => {
            for (index, element) in elements.iter().enumerate() {
                walk(case, &format!("{path}[{index}]"), element, found);
            }
        }
        _ => {}
    }
}

/// The instant `value` names, after asserting it is the RFC 3339 / ISO-8601 string the
/// model requires. A JSON number — what the protocol's epoch-seconds default renders, and
/// what the defect produced — fails here.
fn date_time_at(case: &str, path: &str, value: &Value) -> f64 {
    let rendered = match value {
        Value::String(rendered) => rendered.clone(),
        other => panic!(
            "case {case:?}: {path} must be an ISO-8601 date-time STRING, got the JSON \
             {} {other}",
            match other {
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
                Value::String(_) => "string",
            }
        ),
    };
    // RFC 3339's own shape: a date, the `T` separator and a UTC `Z` offset. Checked before
    // the parse so a value the parser happens to accept in another form still fails.
    assert!(
        rendered.contains('T') && rendered.ends_with('Z') && rendered.len() >= 20,
        "case {case:?}: {path} must be an RFC 3339 UTC date-time, got {rendered:?}"
    );
    let parsed = DateTime::from_str(&rendered, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: {path} must parse as a date-time, got {rendered:?}: {e}")
    });
    // Round trip: the rendering the service chose is the one the modelled format produces.
    let reparsed = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {path} must re-render: {e}"));
    assert_eq!(
        reparsed, rendered,
        "case {case:?}: {path} must already be in the modelled date-time form, got \
         {rendered:?} which re-renders as {reparsed:?}"
    );
    parsed.as_secs_f64()
}

/// Assert that `body` carries exactly the timestamp paths `expected` names, each a
/// `date-time` string, and hand them back.
fn checked_timestamps(case: &str, body: &Value, expected: &[&str]) -> BTreeMap<String, f64> {
    let found = timestamps_of(case, body);
    let paths: Vec<&str> = found.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        expected.to_vec(),
        "case {case:?}: expected the timestamp members {expected:?}, got {paths:?} in {body}"
    );
    found
}

/// One row of the worker-configuration table.
struct WorkerCase {
    name: &'static str,
    description: Option<&'static str>,
    /// `propertiesFileContent`: base64, so every size from empty upwards is legal.
    properties: &'static str,
}

/// The rows differ in what they carry beside the name — a description present, empty or
/// absent, and a properties file empty or not — because the rule is about the RENDERING of
/// a timestamp and so must hold whatever else the request says.
const WORKER_CASES: &[WorkerCase] = &[
    WorkerCase {
        name: "wc-described",
        description: Some("the described configuration"),
        properties: "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
    },
    WorkerCase {
        name: "wc-undescribed",
        description: None,
        properties: "QUJD",
    },
    WorkerCase {
        name: "w",
        description: Some(""),
        properties: "",
    },
];

/// One row of the custom-plugin table: both `CustomPluginContentType` values.
struct PluginCase {
    name: &'static str,
    content_type: &'static str,
    description: Option<&'static str>,
}

const PLUGIN_CASES: &[PluginCase] = &[
    PluginCase {
        name: "zip-plugin",
        content_type: "ZIP",
        description: Some("a zipped plugin"),
    },
    PluginCase {
        name: "jar-plugin",
        content_type: "JAR",
        description: None,
    },
];

/// The `CreateCustomPlugin` body for one row.
fn plugin_request(case: &PluginCase) -> Value {
    let mut body = json!({
        "name": case.name,
        "contentType": case.content_type,
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{}", case.name),
            "fileKey": format!("plugins/{}.zip", case.name),
        }},
    });
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    body
}

/// The `CreateConnector` body for one connector name, referencing `plugin_arn`.
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

#[tokio::test]
async fn every_worker_configuration_timestamp_is_an_iso8601_string_on_the_create_and_read_paths() {
    let emulator = emulator();

    for case in WORKER_CASES {
        let mut request = json!({"name": case.name, "propertiesFileContent": case.properties});
        if let Some(description) = case.description {
            request["description"] = json!(description);
        }
        let (status, created) = call(&emulator, "POST", "/v1/worker-configurations", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateWorkerConfiguration must succeed, got {status} {created}",
            case.name
        );
        // The two members the issue cited, and nothing else carries a timestamp here.
        let created_times = checked_timestamps(
            case.name,
            &created,
            &["$.creationTime", "$.latestRevision.creationTime"],
        );

        let arn = text(&created, "workerConfigurationArn");
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeWorkerConfiguration must succeed, got {status} {described}",
            case.name
        );
        let described_times = checked_timestamps(
            case.name,
            &described,
            &["$.creationTime", "$.latestRevision.creationTime"],
        );

        // The instant is not part of the contract, but it is the resource's own: the read
        // path reports the same instant the create path did, so the rendering is of stored
        // state rather than of the clock at serialize time.
        assert_eq!(
            created_times["$.creationTime"], described_times["$.creationTime"],
            "case {:?}: the described creationTime must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            created_times["$.latestRevision.creationTime"],
            described_times["$.latestRevision.creationTime"],
            "case {:?}: the described revision creationTime must be the created one, got \
             {described}",
            case.name
        );
    }
}

#[tokio::test]
async fn every_custom_plugin_timestamp_is_an_iso8601_string_on_the_read_and_list_paths() {
    let emulator = emulator();
    let mut arns = Vec::new();

    for case in PLUGIN_CASES {
        let (status, created) =
            call(&emulator, "POST", "/v1/custom-plugins", plugin_request(case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCustomPlugin must succeed, got {status} {created}",
            case.name
        );
        // `CreateCustomPluginResponse` models no timestamp member at all
        // (`smithy_query(query="shape", shape="CreateCustomPluginResponse")`), so the
        // create response is expected to carry none.
        checked_timestamps(case.name, &created, &[]);
        arns.push((case.name, text(&created, "customPluginArn")));
    }

    for (name, arn) in &arns {
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeCustomPlugin must succeed, got {status} {described}"
        );
        checked_timestamps(
            name,
            &described,
            &["$.creationTime", "$.latestRevision.creationTime"],
        );
    }

    let (status, listed) = call(&emulator, "GET", "/v1/custom-plugins", Value::Null).await;
    assert_eq!(
        status, 200,
        "ListCustomPlugins must succeed, got {status} {listed}"
    );
    let summaries = listed["customPlugins"]
        .as_array()
        .unwrap_or_else(|| panic!("a customPlugins array in {listed}"));
    assert_eq!(
        summaries.len(),
        PLUGIN_CASES.len(),
        "ListCustomPlugins must report every stored plugin, got {listed}"
    );
    let expected: Vec<String> = (0..PLUGIN_CASES.len())
        .flat_map(|index| {
            [
                format!("$.customPlugins[{index}].creationTime"),
                format!("$.customPlugins[{index}].latestRevision.creationTime"),
            ]
        })
        .collect();
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    checked_timestamps("ListCustomPlugins", &listed, &expected);
}

#[tokio::test]
async fn every_connector_timestamp_is_an_iso8601_string_on_the_read_and_list_paths() {
    let emulator = emulator();
    let (status, plugin) = call(
        &emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_request(&PLUGIN_CASES[0]),
    )
    .await;
    assert_eq!(status, 200, "the plugin must be created, got {plugin}");
    let plugin_arn = text(&plugin, "customPluginArn");
    // A connector may only reference an ACTIVE plugin, and the move out of CREATING is
    // owed to the clock: wait out the service's own span rather than guessing one.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;

    let names = ["connector-one", "connector-two"];
    let mut arns = Vec::new();
    for name in names {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/connectors",
            connector_request(name, &plugin_arn),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnector must succeed, got {status} {created}"
        );
        // `CreateConnectorResponse` models no timestamp member
        // (`smithy_query(query="shape", shape="CreateConnectorResponse")`).
        checked_timestamps(name, &created, &[]);
        arns.push((name, text(&created, "connectorArn")));
    }

    for (name, arn) in &arns {
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors/{}", label(arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeConnector must succeed, got {status} {described}"
        );
        checked_timestamps(name, &described, &["$.creationTime"]);
    }

    let (status, listed) = call(&emulator, "GET", "/v1/connectors", Value::Null).await;
    assert_eq!(
        status, 200,
        "ListConnectors must succeed, got {status} {listed}"
    );
    let summaries = listed["connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("a connectors array in {listed}"));
    assert_eq!(
        summaries.len(),
        names.len(),
        "ListConnectors must report every stored connector, got {listed}"
    );
    let expected: Vec<String> = (0..names.len())
        .map(|index| format!("$.connectors[{index}].creationTime"))
        .collect();
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    checked_timestamps("ListConnectors", &listed, &expected);
}

/// Every shape of this service that declares a `__timestampIso8601` member, with that
/// shape's timestamp members in the model's member order.
///
/// The catalog is the complete set: it is every structure of
/// `smithy-model/com-amazonaws-kafkaconnect-kafkaconnect.json` with a member targeting
/// `com.amazonaws.kafkaconnect#__timestampIso8601`. It therefore also covers
/// `WorkerConfigurationSummary`, whose operation (`ListWorkerConfigurations`) is still a
/// 501 stub and so cannot be reached by the wire suites above.
fn timestamp_shapes() -> Vec<(&'static str, &'static Schema, &'static [&'static str])> {
    vec![
        (
            "CreateWorkerConfigurationResponse",
            &CreateWorkerConfigurationOutput::SCHEMA,
            &["creationTime"],
        ),
        (
            "DescribeWorkerConfigurationResponse",
            &DescribeWorkerConfigurationOutput::SCHEMA,
            &["creationTime"],
        ),
        (
            "WorkerConfigurationSummary",
            &WorkerConfigurationSummary::SCHEMA,
            &["creationTime"],
        ),
        (
            "WorkerConfigurationRevisionSummary",
            &WorkerConfigurationRevisionSummary::SCHEMA,
            &["creationTime"],
        ),
        (
            "WorkerConfigurationRevisionDescription",
            &WorkerConfigurationRevisionDescription::SCHEMA,
            &["creationTime"],
        ),
        (
            "DescribeCustomPluginResponse",
            &DescribeCustomPluginOutput::SCHEMA,
            &["creationTime"],
        ),
        (
            "CustomPluginSummary",
            &CustomPluginSummary::SCHEMA,
            &["creationTime"],
        ),
        (
            "CustomPluginRevisionSummary",
            &CustomPluginRevisionSummary::SCHEMA,
            &["creationTime"],
        ),
        (
            "DescribeConnectorResponse",
            &DescribeConnectorOutput::SCHEMA,
            &["creationTime"],
        ),
        (
            "ConnectorSummary",
            &ConnectorSummary::SCHEMA,
            &["creationTime"],
        ),
        (
            "DescribeConnectorOperationResponse",
            &DescribeConnectorOperationOutput::SCHEMA,
            &["creationTime", "endTime"],
        ),
        (
            "ConnectorOperationSummary",
            &ConnectorOperationSummary::SCHEMA,
            &["creationTime", "endTime"],
        ),
    ]
}

/// RULE, at the schema the codec reads: every timestamp member of every shape in the
/// catalog reports `date-time`, and no other member reports a timestamp format at all.
/// This is the property the wire suites observe, pinned where the JSON codec looks it up.
#[test]
fn every_modelled_timestamp_member_reports_the_date_time_format() {
    for (shape, schema, modelled) in timestamp_shapes() {
        let members = schema.members();
        let timestamps: Vec<&str> = members
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .map(|member| member.member_name().unwrap_or("<unnamed>"))
            .collect();
        assert_eq!(
            timestamps,
            modelled.to_vec(),
            "case {shape:?}: expected the modelled timestamp members {modelled:?}, got \
             {timestamps:?}"
        );

        for (position, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            // The codec dispatches on the index, so it has to be the member's position.
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {shape:?}.{name}: member_index must equal the member's position, got {:?}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|carried| carried.format());
            if matches!(member.shape_type(), ShapeType::Timestamp) {
                assert_eq!(
                    format,
                    Some(TimestampFormat::DateTime),
                    "case {shape:?}.{name}: a `__timestampIso8601` member must report \
                     date-time, got {format:?}"
                );
            } else {
                assert_eq!(
                    format, None,
                    "case {shape:?}.{name}: only a timestamp member carries a timestamp \
                     format, got {format:?}"
                );
            }
        }
    }
}

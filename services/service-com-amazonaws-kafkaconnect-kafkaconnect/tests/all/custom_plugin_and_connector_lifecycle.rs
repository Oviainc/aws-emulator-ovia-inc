// SPDX-License-Identifier: CC0-1.0

//! The custom-plugin + connector lifecycle, end to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request instead of from state would fail here.
//!
//! The tables below span the classes the rules generalise over — both
//! `contentType` values, both capacity forms, every `networkType`, each
//! published length bound and one past it — rather than the single case the
//! bootstrap issue cited.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// must carry. No authenticator is registered, so the signature is never
/// checked; the scope is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// Long enough that every change the clock owes after one call is due. The span
/// itself is the service's own constant, so this cannot drift from it.
async fn past_the_transition() {
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
/// an AWS SDK encodes a label: every character a path segment reserves
/// (including the ARN's own `:` and `/`) is escaped. Encoded with the same
/// `percent-encoding` crate the server side uses, rather than a second encoder.
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

/// Whether `suffix` is the UUID AWS gives the random part of a kafkaconnect
/// resource ARN: the 8-4-4-4-12 lowercase-hex form, with RFC 4122's version
/// (4) and variant (8..b) nibbles.
fn is_uuid(suffix: &str) -> bool {
    let groups: Vec<&str> = suffix.split('-').collect();
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

/// One row of the custom-plugin table.
struct PluginCase {
    name: &'static str,
    /// `CustomPluginContentType`: JAR | ZIP.
    content_type: &'static str,
    description: Option<&'static str>,
    object_version: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The `CreateCustomPlugin` body for one row.
fn plugin_request(case: &PluginCase) -> Value {
    let mut location = json!({
        "s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{}", case.name),
            "fileKey": format!("plugins/{}.zip", case.name),
        }
    });
    if let Some(version) = case.object_version {
        location["s3Location"]["objectVersion"] = json!(version);
    }
    let mut body = json!({
        "name": case.name,
        "contentType": case.content_type,
        "location": location,
    });
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// Every `contentType` the model accepts, with and without the optional members
/// — the class `CreateCustomPlugin`/`DescribeCustomPlugin` must round-trip.
const PLUGIN_CASES: &[PluginCase] = &[
    PluginCase {
        name: "demo-plugin",
        content_type: "ZIP",
        description: Some("the plugin the bootstrap issue cited"),
        object_version: None,
        tags: &[("team", "data")],
    },
    PluginCase {
        name: "jar-plugin",
        content_type: "JAR",
        description: None,
        object_version: Some("3HL4kqtJlcpXroDTDmjVBH40Nrjfkd"),
        tags: &[],
    },
    PluginCase {
        // The low end of `__stringMin1Max128` (len 1..128).
        name: "a",
        content_type: "JAR",
        description: Some(""),
        object_version: None,
        tags: &[("one", "1"), ("two", "2")],
    },
];

#[tokio::test]
async fn a_created_custom_plugin_reads_back_from_state_and_becomes_active_on_the_clock() {
    let emulator = emulator();
    let mut arns = Vec::new();

    for case in PLUGIN_CASES {
        let (status, created) =
            call(&emulator, "POST", "/v1/custom-plugins", plugin_request(case)).await;
        assert_eq!(
            status, 200,
            "case {}: CreateCustomPlugin must succeed, got {status} {created}",
            case.name
        );

        let arn = text(&created, "customPluginArn");
        let expected_prefix =
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:custom-plugin/{}/", case.name);
        assert!(
            arn.starts_with(&expected_prefix),
            "case {}: expected an ARN starting {expected_prefix}, got {arn:?}",
            case.name
        );
        assert!(
            is_uuid(&arn[expected_prefix.len()..]),
            "case {}: expected a uuid ARN suffix, got {:?}",
            case.name,
            &arn[expected_prefix.len()..]
        );
        assert_eq!(
            text(&created, "name"),
            case.name,
            "case {}: CreateCustomPlugin must echo the stored name, got {created}",
            case.name
        );
        assert_eq!(
            text(&created, "customPluginState"),
            "CREATING",
            "case {}: a plugin is not active the instant it is created, got {created}",
            case.name
        );
        assert_eq!(
            created.get("revision").and_then(Value::as_i64),
            Some(1),
            "case {}: a new plugin is at revision 1, got {created}",
            case.name
        );

        // Read back through DescribeCustomPlugin: every value must come from
        // state, and the plugin must still be CREATING because the span the
        // clock owes is never zero.
        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: DescribeCustomPlugin must succeed, got {status} {described}",
            case.name
        );
        assert_eq!(
            text(&described, "customPluginArn"),
            arn,
            "case {}: the described ARN must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "name"),
            case.name,
            "case {}: the described name must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "customPluginState"),
            "CREATING",
            "case {}: a plugin read straight after creation is still CREATING, got {described}",
            case.name
        );
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            case.description,
            "case {}: the described description must be the created one, got {described}",
            case.name
        );
        assert!(
            described.get("creationTime").is_some(),
            "case {}: a described plugin carries its creationTime, got {described}",
            case.name
        );

        let revision = described
            .get("latestRevision")
            .unwrap_or_else(|| panic!("case {}: latestRevision in {described}", case.name));
        assert_eq!(
            revision.get("revision").and_then(Value::as_i64),
            Some(1),
            "case {}: latestRevision must be the stored revision, got {revision}",
            case.name
        );
        assert_eq!(
            text(revision, "contentType"),
            case.content_type,
            "case {}: latestRevision must carry the stored contentType, got {revision}",
            case.name
        );
        let s3 = &revision["location"]["s3Location"];
        assert_eq!(
            text(s3, "bucketArn"),
            format!("arn:aws:s3:::plugin-bucket-{}", case.name),
            "case {}: latestRevision must carry the stored bucketArn, got {s3}",
            case.name
        );
        assert_eq!(
            text(s3, "fileKey"),
            format!("plugins/{}.zip", case.name),
            "case {}: latestRevision must carry the stored fileKey, got {s3}",
            case.name
        );
        assert_eq!(
            s3.get("objectVersion").and_then(Value::as_str),
            case.object_version,
            "case {}: latestRevision must carry the stored objectVersion, got {s3}",
            case.name
        );

        arns.push((case.name, arn));
    }

    // The move out of CREATING is owed to the clock, not to any call.
    past_the_transition().await;
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
            "case {name}: DescribeCustomPlugin must still succeed, got {status} {described}"
        );
        assert_eq!(
            text(&described, "customPluginState"),
            "ACTIVE",
            "case {name}: the plugin must be ACTIVE once the transition is due, got {described}"
        );
    }

    // Every stored plugin, and only those, are listed.
    let (status, listed) = call(&emulator, "GET", "/v1/custom-plugins", Value::Null).await;
    assert_eq!(status, 200, "ListCustomPlugins must succeed, got {listed}");
    let names: BTreeSet<String> = listed["customPlugins"]
        .as_array()
        .unwrap_or_else(|| panic!("customPlugins array in {listed}"))
        .iter()
        .map(|summary| text(summary, "name"))
        .collect();
    let expected: BTreeSet<String> = PLUGIN_CASES
        .iter()
        .map(|case| case.name.to_string())
        .collect();
    assert_eq!(
        names, expected,
        "ListCustomPlugins must report exactly the stored plugins, got {listed}"
    );
}

#[tokio::test]
async fn a_repeated_custom_plugin_name_is_a_conflict_whatever_the_rest_of_the_request_says() {
    let emulator = emulator();
    for case in PLUGIN_CASES {
        let (status, _) = call(&emulator, "POST", "/v1/custom-plugins", plugin_request(case)).await;
        assert_eq!(status, 200, "case {}: the first create succeeds", case.name);

        // Same name, everything else different: the name alone decides.
        let mut repeat = plugin_request(case);
        repeat["contentType"] = json!(if case.content_type == "ZIP" { "JAR" } else { "ZIP" });
        repeat["location"]["s3Location"]["fileKey"] = json!("plugins/other.zip");
        let (status, body) = call(&emulator, "POST", "/v1/custom-plugins", repeat).await;
        assert_eq!(
            status, 409,
            "case {}: a repeated name is a 409, got {status} {body}",
            case.name
        );
        assert!(
            error_code(&body).contains("ConflictException"),
            "case {}: expected ConflictException, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_modelled_constraints_on_a_create_custom_plugin_request_are_refused_as_bad_requests() {
    // The bounds come from the model, not from this test:
    // `name` is `__stringMin1Max128` (len 1..128), `description` is
    // `__stringMax1024` (len 0..1024) and `contentType` is
    // `CustomPluginContentType` (JAR | ZIP) — each published bound is exercised
    // here together with one value past it.
    let emulator = emulator();
    let name_at_min = "n".to_string();
    let name_at_max = "n".repeat(128);
    let name_past_max = "n".repeat(129);
    let description_at_max = "d".repeat(1024);
    let description_past_max = "d".repeat(1025);

    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "name at the minimum length",
            json!({"name": name_at_min, "contentType": "ZIP", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            200,
        ),
        (
            "name at the maximum length",
            json!({"name": name_at_max, "contentType": "ZIP", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            200,
        ),
        (
            "name one past the maximum length",
            json!({"name": name_past_max, "contentType": "ZIP", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            400,
        ),
        (
            "empty name, one below the minimum length",
            json!({"name": "", "contentType": "ZIP", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            400,
        ),
        (
            "description at the maximum length",
            json!({"name": "description-at-max", "contentType": "ZIP", "description": description_at_max, "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            200,
        ),
        (
            "description one past the maximum length",
            json!({"name": "description-past-max", "contentType": "ZIP", "description": description_past_max, "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            400,
        ),
        (
            "contentType outside the enumerated set",
            json!({"name": "bad-content-type", "contentType": "TARBALL", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            400,
        ),
        (
            "the required location is absent",
            json!({"name": "no-location", "contentType": "ZIP"}),
            400,
        ),
        (
            "the required name is absent",
            json!({"contentType": "ZIP", "location": {"s3Location": {"bucketArn": "arn:aws:s3:::b", "fileKey": "k"}}}),
            400,
        ),
    ];

    for (case, body, want) in cases {
        let (status, answered) = call(&emulator, "POST", "/v1/custom-plugins", body).await;
        assert_eq!(
            status, want,
            "case {case:?}: expected HTTP {want}, got {status} {answered}"
        );
        if want == 400 {
            assert!(
                error_code(&answered).contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {answered}"
            );
        }
    }
}

/// One row of the connector table.
struct ConnectorCase {
    name: &'static str,
    /// `"autoScaling"` or `"provisionedCapacity"` — the two forms of `Capacity`.
    capacity: &'static str,
    /// `NetworkType` (IPV4 | DUAL), or `None` to leave it out: the member
    /// documents "Defaults to IPV4".
    network_type: Option<&'static str>,
    /// `KafkaClusterClientAuthenticationType`: IAM | NONE.
    authentication: &'static str,
    /// `KafkaClusterEncryptionInTransitType`: PLAINTEXT | TLS.
    encryption: &'static str,
    kafka_connect_version: &'static str,
    description: Option<&'static str>,
    subnets: &'static [&'static str],
    with_log_delivery: bool,
}

const CONNECTOR_CASES: &[ConnectorCase] = &[
    ConnectorCase {
        name: "demo-connector",
        capacity: "provisionedCapacity",
        network_type: None,
        authentication: "NONE",
        encryption: "PLAINTEXT",
        kafka_connect_version: "2.7.1",
        description: Some("the connector the bootstrap issue cited"),
        subnets: &["subnet-0a1b2c3d"],
        with_log_delivery: false,
    },
    ConnectorCase {
        name: "scaled-connector",
        capacity: "autoScaling",
        network_type: Some("DUAL"),
        authentication: "IAM",
        encryption: "TLS",
        kafka_connect_version: "3.7.x",
        description: None,
        subnets: &["subnet-0a1b2c3d", "subnet-4e5f6a7b", "subnet-8c9d0e1f"],
        with_log_delivery: true,
    },
    ConnectorCase {
        name: "ipv4-connector",
        capacity: "provisionedCapacity",
        network_type: Some("IPV4"),
        authentication: "NONE",
        encryption: "TLS",
        kafka_connect_version: "2.7.1",
        description: None,
        subnets: &["subnet-0a1b2c3d", "subnet-4e5f6a7b"],
        with_log_delivery: false,
    },
];

/// The capacity body for one row, in whichever of the two forms it names.
fn capacity_of(case: &ConnectorCase) -> Value {
    if case.capacity == "autoScaling" {
        json!({"autoScaling": {
            "maxWorkerCount": 4,
            "mcuCount": 2,
            "minWorkerCount": 1,
            "scaleInPolicy": {"cpuUtilizationPercentage": 20},
            "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
        }})
    } else {
        json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}})
    }
}

/// The `CreateConnector` body for one row, referencing `plugin_arn`.
fn connector_request(case: &ConnectorCase, plugin_arn: &str) -> Value {
    let mut body = json!({
        "connectorName": case.name,
        "capacity": capacity_of(case),
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
            "topics": format!("{}-topic", case.name),
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{}.kafka.{REGION}.amazonaws.com:9092", case.name),
            "vpc": {"subnets": case.subnets, "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": case.authentication},
        "kafkaClusterEncryptionInTransit": {"encryptionType": case.encryption},
        "kafkaConnectVersion": case.kafka_connect_version,
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{}-role", case.name),
    });
    if let Some(network_type) = case.network_type {
        body["networkType"] = json!(network_type);
    }
    if let Some(description) = case.description {
        body["connectorDescription"] = json!(description);
    }
    if case.with_log_delivery {
        body["logDelivery"] = json!({"workerLogDelivery": {
            "cloudWatchLogs": {"enabled": true, "logGroup": "/aws/kafkaconnect/demo"},
            "s3": {"enabled": false},
        }});
    }
    body
}

/// Create one plugin and return its ARN, waiting until it is ACTIVE.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let case = PluginCase {
        name: "",
        content_type: "ZIP",
        description: None,
        object_version: None,
        tags: &[],
    };
    let mut body = plugin_request(&case);
    body["name"] = json!(name);
    let (status, created) = call(emulator, "POST", "/v1/custom-plugins", body).await;
    assert_eq!(status, 200, "the plugin {name} must be created, got {created}");
    past_the_transition().await;
    text(&created, "customPluginArn")
}

#[tokio::test]
async fn a_created_connector_reads_back_from_state_and_becomes_running_on_the_clock() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "connector-plugin").await;
    let mut arns = Vec::new();

    for case in CONNECTOR_CASES {
        let request = connector_request(case, &plugin_arn);
        let (status, created) = call(&emulator, "POST", "/v1/connectors", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {}: CreateConnector must succeed, got {status} {created}",
            case.name
        );

        let arn = text(&created, "connectorArn");
        let expected_prefix =
            format!("arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/{}/", case.name);
        assert!(
            arn.starts_with(&expected_prefix),
            "case {}: expected an ARN starting {expected_prefix}, got {arn:?}",
            case.name
        );
        assert!(
            is_uuid(&arn[expected_prefix.len()..]),
            "case {}: expected a uuid ARN suffix, got {:?}",
            case.name,
            &arn[expected_prefix.len()..]
        );
        assert_eq!(
            text(&created, "connectorName"),
            case.name,
            "case {}: CreateConnector must echo the stored name, got {created}",
            case.name
        );
        assert_eq!(
            text(&created, "connectorState"),
            "CREATING",
            "case {}: a connector is not running the instant it is created, got {created}",
            case.name
        );

        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: DescribeConnector must succeed, got {status} {described}",
            case.name
        );
        assert_eq!(
            text(&described, "connectorArn"),
            arn,
            "case {}: the described ARN must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "connectorName"),
            case.name,
            "case {}: the described name must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "connectorState"),
            "CREATING",
            "case {}: a connector read straight after creation is still CREATING, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "currentVersion"),
            "1",
            "case {}: a new connector is at version 1, got {described}",
            case.name
        );
        assert!(
            described.get("creationTime").is_some(),
            "case {}: a described connector carries its creationTime, got {described}",
            case.name
        );
        assert_eq!(
            described.get("connectorDescription").and_then(Value::as_str),
            case.description,
            "case {}: the described description must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            described["connectorConfiguration"], request["connectorConfiguration"],
            "case {}: the whole stored configuration must come back, got {described}",
            case.name
        );
        assert!(
            !described["capacity"][case.capacity].is_null(),
            "case {}: the described capacity must be the {} form, got {described}",
            case.name,
            case.capacity
        );
        assert_eq!(
            text(&described, "kafkaConnectVersion"),
            case.kafka_connect_version,
            "case {}: the described kafkaConnectVersion must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "networkType"),
            case.network_type.unwrap_or("IPV4"),
            "case {}: networkType defaults to IPV4 when the request names none, got {described}",
            case.name
        );
        assert_eq!(
            text(&described["kafkaClusterClientAuthentication"], "authenticationType"),
            case.authentication,
            "case {}: the described authenticationType must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described["kafkaClusterEncryptionInTransit"], "encryptionType"),
            case.encryption,
            "case {}: the described encryptionType must be the created one, got {described}",
            case.name
        );
        let cluster = &described["kafkaCluster"]["apacheKafkaCluster"];
        assert_eq!(
            cluster["vpc"]["subnets"], json!(case.subnets),
            "case {}: every stored subnet must come back, got {cluster}",
            case.name
        );
        assert_eq!(
            text(cluster, "bootstrapServers"),
            format!("b-1.{}.kafka.{REGION}.amazonaws.com:9092", case.name),
            "case {}: the described bootstrapServers must be the created one, got {cluster}",
            case.name
        );
        assert_eq!(
            text(&described, "serviceExecutionRoleArn"),
            format!("arn:aws:iam::{ACCOUNT}:role/{}-role", case.name),
            "case {}: the described role must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            described["plugins"],
            json!([{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}]),
            "case {}: the described plugins must be the created ones, got {described}",
            case.name
        );
        assert_eq!(
            described["logDelivery"]["workerLogDelivery"]["cloudWatchLogs"]["enabled"]
                .as_bool()
                .unwrap_or(false),
            case.with_log_delivery,
            "case {}: log delivery must come back exactly as created, got {described}",
            case.name
        );

        arns.push((case.name, arn));
    }

    past_the_transition().await;
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
            "case {name}: DescribeConnector must still succeed, got {status} {described}"
        );
        assert_eq!(
            text(&described, "connectorState"),
            "RUNNING",
            "case {name}: the connector must be RUNNING once the transition is due, got {described}"
        );
    }
}

#[tokio::test]
async fn a_create_connector_request_is_refused_on_its_name_its_plugins_and_its_capacity() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "refusal-plugin").await;
    let case = &CONNECTOR_CASES[0];

    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_request(case, &plugin_arn),
    )
    .await;
    assert_eq!(status, 200, "the first create succeeds, got {created}");

    // A repeated name conflicts, whatever the rest of the request says.
    let mut repeat = connector_request(case, &plugin_arn);
    repeat["kafkaConnectVersion"] = json!("3.7.x");
    let (status, body) = call(&emulator, "POST", "/v1/connectors", repeat).await;
    assert_eq!(status, 409, "a repeated connector name is a 409, got {body}");
    assert!(
        error_code(&body).contains("ConflictException"),
        "expected ConflictException, got {body}"
    );

    let unknown_plugin = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:custom-plugin/missing/00000000-0000-0000-0000-000000000000"
    );
    let refusals: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a plugin ARN no stored plugin carries",
            {
                let mut body = connector_request(case, &unknown_plugin);
                body["connectorName"] = json!("unknown-plugin-connector");
                body
            },
            404,
            "NotFoundException",
        ),
        (
            "a revision the stored plugin does not have",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("unknown-revision-connector");
                body["plugins"] =
                    json!([{"customPlugin": {"customPluginArn": plugin_arn, "revision": 7}}]);
                body
            },
            404,
            "NotFoundException",
        ),
        (
            "an empty plugins list",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("no-plugin-connector");
                body["plugins"] = json!([]);
                body
            },
            400,
            "BadRequestException",
        ),
        (
            "more than one plugin",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("two-plugin-connector");
                body["plugins"] = json!([
                    {"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}},
                    {"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}},
                ]);
                body
            },
            400,
            "BadRequestException",
        ),
        (
            "a capacity naming neither form",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("no-capacity-connector");
                body["capacity"] = json!({});
                body
            },
            400,
            "BadRequestException",
        ),
        (
            "a capacity naming both forms",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("both-capacity-connector");
                body["capacity"] = json!({
                    "autoScaling": {"maxWorkerCount": 4, "mcuCount": 2, "minWorkerCount": 1},
                    "provisionedCapacity": {"mcuCount": 1, "workerCount": 2},
                });
                body
            },
            400,
            "BadRequestException",
        ),
        (
            "a plugin revision below the modelled minimum of 1",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("zero-revision-connector");
                body["plugins"] =
                    json!([{"customPlugin": {"customPluginArn": plugin_arn, "revision": 0}}]);
                body
            },
            400,
            "BadRequestException",
        ),
        (
            "the required serviceExecutionRoleArn is absent",
            {
                let mut body = connector_request(case, &plugin_arn);
                body["connectorName"] = json!("no-role-connector");
                body.as_object_mut()
                    .expect("an object body")
                    .remove("serviceExecutionRoleArn");
                body
            },
            400,
            "BadRequestException",
        ),
    ];

    for (name, body, want_status, want_code) in refusals {
        // The name THIS row asked for, so the "stores nothing" check below looks
        // for the very connector the refused request would have stored rather
        // than for a prefix no row uses.
        let refused_name = text(&body, "connectorName");
        assert!(
            !refused_name.is_empty(),
            "case {name:?}: every refusal row names the connector it tries to create"
        );
        let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(
            status, want_status,
            "case {name:?}: expected HTTP {want_status}, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains(want_code),
            "case {name:?}: expected {want_code}, got {answered}"
        );

        // A refused create stores nothing: the name it asked for is still free.
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors?connectorNamePrefix={refused_name}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: ListConnectors must succeed");
        assert_eq!(
            listed["connectors"].as_array().map(Vec::len),
            Some(0),
            "case {name:?}: a refused create must store no connector named {refused_name:?}, got {listed}"
        );
    }

    // A plugin the service has not finished taking on cannot back a connector:
    // this one is still CREATING, because the clock owes it its move to ACTIVE.
    let mut plugin = plugin_request(&PluginCase {
        name: "",
        content_type: "ZIP",
        description: None,
        object_version: None,
        tags: &[],
    });
    plugin["name"] = json!("still-creating-plugin");
    let (status, created) = call(&emulator, "POST", "/v1/custom-plugins", plugin).await;
    assert_eq!(status, 200, "the plugin must be created, got {created}");
    assert_eq!(
        text(&created, "customPluginState"),
        "CREATING",
        "the plugin must still be CREATING for this case, got {created}"
    );
    let mut body = connector_request(case, &text(&created, "customPluginArn"));
    body["connectorName"] = json!("creating-plugin-connector");
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 400,
        "a connector on a CREATING plugin is a 400, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("BadRequestException"),
        "expected BadRequestException, got {answered}"
    );
}

#[tokio::test]
async fn listing_connectors_filters_by_prefix_and_pages_each_connector_exactly_once() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "paging-plugin").await;
    let case = &CONNECTOR_CASES[0];

    // Five connectors under one prefix, plus one outside it.
    let mut expected = BTreeSet::new();
    for index in 0..5 {
        let name = format!("page-{index}");
        let mut body = connector_request(case, &plugin_arn);
        body["connectorName"] = json!(name);
        let (status, created) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(status, 200, "creating {name} must succeed, got {created}");
        expected.insert(text(&created, "connectorArn"));
    }
    let mut body = connector_request(case, &plugin_arn);
    body["connectorName"] = json!("other-connector");
    let (status, _) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(status, 200, "creating the connector outside the prefix must succeed");

    // Every page size spanning the published `maxResults` range pages the same
    // five connectors, each exactly once.
    for page_size in [1_i32, 2, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path =
                format!("/v1/connectors?connectorNamePrefix=page-&maxResults={page_size}");
            if let Some(token) = &token {
                path.push_str(&format!("&nextToken={token}"));
            }
            let (status, listed) = call(&emulator, "GET", &path, Value::Null).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListConnectors must succeed, got {listed}"
            );
            let items = listed["connectors"]
                .as_array()
                .unwrap_or_else(|| panic!("connectors array in {listed}"));
            assert!(
                items.len() <= usize::try_from(page_size).expect("a positive page size"),
                "page size {page_size}: a page must not exceed maxResults, got {} items",
                items.len()
            );
            for summary in items {
                assert!(
                    text(summary, "connectorName").starts_with("page-"),
                    "page size {page_size}: the prefix filter must hold, got {summary}"
                );
                assert_eq!(
                    text(summary, "currentVersion"),
                    "1",
                    "page size {page_size}: a summary carries the stored version, got {summary}"
                );
                assert!(
                    summary.get("creationTime").is_some(),
                    "page size {page_size}: a summary carries its creationTime, got {summary}"
                );
                seen.push(text(summary, "connectorArn"));
            }
            pages += 1;
            assert!(pages <= 10, "page size {page_size}: paging must terminate");
            match listed.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "page size {page_size}: paging must not repeat a connector, saw {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "page size {page_size}: paging must return every connector under the prefix"
        );
    }

    // The published `maxResults` range is 1..100; a value outside it is refused.
    for (case_name, max_results) in [("below the minimum", 0), ("above the maximum", 101)] {
        let (status, answered) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors?maxResults={max_results}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case_name:?}: maxResults={max_results} is outside the modelled range, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "case {case_name:?}: expected BadRequestException, got {answered}"
        );
    }

    // A token this service never issued is a bad request, not an empty page and
    // not a silently honoured resume point. Both forms are refused: one that is
    // not even the token's encoding, and one that is — hex that decodes to a
    // plausible key — but carries none of this service's marker.
    for forged in [
        "not-a-token",
        "6161",
        &hex::encode("arn:aws:kafkaconnect:eu-west-1:000000000000:connector/page-0/x"),
    ] {
        let (status, answered) = call(
            &emulator,
            "GET",
            &format!("/v1/connectors?nextToken={}", label(forged)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 400,
            "token {forged:?}: an unissued nextToken is a 400, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("BadRequestException"),
            "token {forged:?}: expected BadRequestException, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_create_is_refused_once_the_resource_quota_is_reached() {
    // The cap is the service's own constant, so this cannot drift from it.
    use service_com_amazonaws_kafkaconnect_kafkaconnect::state::RESOURCE_QUOTA;

    let emulator = emulator();
    let mut plugin_arn = String::new();
    for index in 0..RESOURCE_QUOTA {
        let case = PluginCase {
            name: "",
            content_type: "ZIP",
            description: None,
            object_version: None,
            tags: &[],
        };
        let mut body = plugin_request(&case);
        body["name"] = json!(format!("quota-plugin-{index}"));
        let (status, created) = call(&emulator, "POST", "/v1/custom-plugins", body).await;
        assert_eq!(
            status, 200,
            "plugin {index} of the quota must be created, got {status} {created}"
        );
        if index == 0 {
            plugin_arn = text(&created, "customPluginArn");
        }
    }

    let case = PluginCase {
        name: "",
        content_type: "ZIP",
        description: None,
        object_version: None,
        tags: &[],
    };
    let mut body = plugin_request(&case);
    body["name"] = json!("quota-plugin-over");
    let (status, answered) = call(&emulator, "POST", "/v1/custom-plugins", body).await;
    assert_eq!(
        status, 429,
        "the plugin past the quota is refused with 429, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("TooManyRequestsException"),
        "expected TooManyRequestsException, got {answered}"
    );

    // The connector map is capped on its own count, not the plugins'.
    past_the_transition().await;
    let connector_case = &CONNECTOR_CASES[0];
    for index in 0..RESOURCE_QUOTA {
        let mut body = connector_request(connector_case, &plugin_arn);
        body["connectorName"] = json!(format!("quota-connector-{index}"));
        let (status, created) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(
            status, 200,
            "connector {index} of the quota must be created, got {status} {created}"
        );
    }
    let mut body = connector_request(connector_case, &plugin_arn);
    body["connectorName"] = json!("quota-connector-over");
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 429,
        "the connector past the quota is refused with 429, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("TooManyRequestsException"),
        "expected TooManyRequestsException, got {answered}"
    );
}

#[tokio::test]
async fn deleting_a_connector_and_a_plugin_moves_them_to_deleting_and_then_drops_them() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "deletable-plugin").await;
    let case = &CONNECTOR_CASES[0];
    let (status, created) = call(
        &emulator,
        "POST",
        "/v1/connectors",
        connector_request(case, &plugin_arn),
    )
    .await;
    assert_eq!(status, 200, "the connector must be created, got {created}");
    let connector_arn = text(&created, "connectorArn");

    // A currentVersion that is not the connector's own is refused, and the
    // connector is untouched.
    let (status, answered) = call(
        &emulator,
        "DELETE",
        &format!("/v1/connectors/{}?currentVersion=9", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 400, "a stale currentVersion is a 400, got {answered}");
    assert!(
        error_code(&answered).contains("BadRequestException"),
        "expected BadRequestException, got {answered}"
    );

    // The real version is accepted, and the delete is idempotent.
    for attempt in 1..=2 {
        let (status, deleted) = call(
            &emulator,
            "DELETE",
            &format!("/v1/connectors/{}?currentVersion=1", label(&connector_arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: DeleteConnector must succeed, got {status} {deleted}"
        );
        assert_eq!(
            text(&deleted, "connectorArn"),
            connector_arn,
            "attempt {attempt}: DeleteConnector names the connector it deleted, got {deleted}"
        );
        assert_eq!(
            text(&deleted, "connectorState"),
            "DELETING",
            "attempt {attempt}: a deleted connector is DELETING, got {deleted}"
        );
    }

    // The next read sees the change the delete made.
    let (status, described) = call(
        &emulator,
        "GET",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "a deleting connector is still readable, got {described}");
    assert_eq!(
        text(&described, "connectorState"),
        "DELETING",
        "the read must show the state the delete moved it to, got {described}"
    );

    // Once the clock has dropped it, it is gone from both reads.
    past_the_transition().await;
    let (status, answered) = call(
        &emulator,
        "GET",
        &format!("/v1/connectors/{}", label(&connector_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 404, "a dropped connector is a 404, got {answered}");
    assert!(
        error_code(&answered).contains("NotFoundException"),
        "expected NotFoundException, got {answered}"
    );
    let (status, listed) = call(&emulator, "GET", "/v1/connectors", Value::Null).await;
    assert_eq!(status, 200, "ListConnectors must succeed, got {listed}");
    assert_eq!(
        listed["connectors"].as_array().map(Vec::len),
        Some(0),
        "a dropped connector must not be listed, got {listed}"
    );

    // The same two steps for the plugin.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/custom-plugins/{}", label(&plugin_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "DeleteCustomPlugin must succeed, got {deleted}");
    assert_eq!(
        text(&deleted, "customPluginState"),
        "DELETING",
        "a deleted plugin is DELETING, got {deleted}"
    );
    past_the_transition().await;
    let (status, answered) = call(
        &emulator,
        "GET",
        &format!("/v1/custom-plugins/{}", label(&plugin_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(status, 404, "a dropped plugin is a 404, got {answered}");
    assert!(
        error_code(&answered).contains("NotFoundException"),
        "expected NotFoundException, got {answered}"
    );

    // An ARN that never existed is a 404 on every one of the four by-ARN
    // operations, not a 500 and not a success.
    let missing_connector = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/00000000-0000-0000-0000-000000000000"
    );
    let missing_plugin = format!(
        "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:custom-plugin/ghost/00000000-0000-0000-0000-000000000000"
    );
    let unknown: Vec<(&str, &str, String)> = vec![
        ("GET", "/v1/connectors", missing_connector.clone()),
        ("DELETE", "/v1/connectors", missing_connector),
        ("GET", "/v1/custom-plugins", missing_plugin.clone()),
        ("DELETE", "/v1/custom-plugins", missing_plugin),
    ];
    for (method, collection, arn) in unknown {
        let (status, answered) = call(
            &emulator,
            method,
            &format!("{collection}/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {method} {collection}: an unknown ARN is a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {method} {collection}: expected NotFoundException, got {answered}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The capacity rule, driven end to end through BOTH operations that take a
//! capacity: `CreateConnector` and `UpdateConnector`.
//!
//! RULE. A connector's capacity is "whether it is auto scaled or provisioned",
//! so a capacity names exactly one of the two optional forms `Capacity` /
//! `CapacityUpdate` give it, and the counts inside the named form must hold the
//! relations their own members document. An update's `CapacityUpdate` mirrors a
//! create's `Capacity` member for member, so a capacity either operation accepts
//! is one the other accepts, and a capacity either refuses is refused by the
//! other with the same HTTP status, the same error code and the same message.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, with the
//! response body read back as JSON. Nothing reaches into the state struct.
//!
//! The table spans what the rule generalises over: which forms a capacity names
//! (neither, both, each one alone), and, inside each form, a count the service
//! refuses — an `mcuCount` the modelled range `__integerMin1Max8` admits but the
//! member's documented value set does not, a worker window that is no window, and
//! a task ceiling below the worker ceiling. The accepted rows sit at the published
//! bounds of that value set.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{MCU_COUNT_VALUES, TRANSITION_SPAN};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// carries. No authenticator is registered, so the signature is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

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

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Every number `value` carries, behind the path it sits at, so a comparison
/// names the capacity member that differs instead of printing two blobs.
fn numbers(value: &Value) -> BTreeMap<String, i64> {
    fn walk(at: &str, value: &Value, into: &mut BTreeMap<String, i64>) {
        match value {
            Value::Object(members) => {
                for (member, value) in members {
                    walk(&format!("{at}/{member}"), value, into);
                }
            }
            Value::Number(number) => {
                into.insert(at.to_string(), number.as_i64().unwrap_or_default());
            }
            _ => {}
        }
    }
    let mut collected = BTreeMap::new();
    walk("", value, &mut collected);
    collected
}

/// How one operation answered a capacity: the parts of the answer the two
/// operations must agree on.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    code: String,
    message: String,
}

impl Answer {
    /// The answer a response carries. A refusal's `__type` is read without its
    /// namespace prefix, and its `message` verbatim.
    fn of(status: u16, body: &Value) -> Self {
        let code = text(body, "__type");
        let code = code
            .rsplit(['#', ':'])
            .next()
            .unwrap_or_default()
            .to_string();
        Self {
            status,
            code,
            message: text(body, "message"),
        }
    }
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

/// A capacity every connector in this suite is created with: the provisioned
/// form, so a row naming the auto-scaled form changes the FORM and not only a
/// value.
fn created_capacity() -> Value {
    json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}})
}

/// The `CreateConnector` body for `name`, referencing `plugin_arn`, carrying
/// `capacity` as its `capacity` member.
fn connector_request(name: &str, plugin_arn: &str, capacity: &Value) -> Value {
    json!({
        "connectorName": name,
        "capacity": capacity,
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

/// An auto-scaled capacity naming the counts given, with both scaling policies,
/// which `AutoScalingUpdate` requires and `AutoScaling` accepts.
fn auto_scaling(mcu_count: i32, minimum: i32, maximum: i32, task_ceiling: i32) -> Value {
    json!({"autoScaling": {
        "mcuCount": mcu_count,
        "minWorkerCount": minimum,
        "maxWorkerCount": maximum,
        "scaleInPolicy": {"cpuUtilizationPercentage": 20},
        "scaleOutPolicy": {"cpuUtilizationPercentage": 80},
        "maxAutoscalingTaskCount": task_ceiling,
    }})
}

/// One row: the capacity, why it is in the class, and whether the rule accepts
/// it.
struct Row {
    why: &'static str,
    capacity: Value,
    accepted: bool,
}

/// The capacities both operations are driven with.
///
/// The accepted rows sit at the published bounds of the `mcuCount` value set
/// (`AutoScaling$mcuCount` / `ProvisionedCapacity$mcuCount`: "The valid values
/// are 1,2,4,8"), in either form; the refused rows name neither form, both
/// forms, and — inside each form — a count the service does not allocate.
fn rows() -> Vec<Row> {
    let smallest_mcu_count = MCU_COUNT_VALUES[0];
    let largest_mcu_count = *MCU_COUNT_VALUES
        .iter()
        .max()
        .expect("the documented value set is not empty");
    // An integer the modelled range `__integerMin1Max8` admits and the
    // documented value set does not, so the framework passes it to the service.
    let unallocatable_mcu_count = (1..=8)
        .find(|count| !MCU_COUNT_VALUES.contains(count))
        .expect("the range admits a count outside the value set");
    // One capacity carrying both forms, each of them a form the rule accepts on
    // its own, so what the refusal answers to is naming two of them.
    let mut both_forms = auto_scaling(smallest_mcu_count, 1, 2, 2);
    both_forms["provisionedCapacity"] =
        json!({"mcuCount": smallest_mcu_count, "workerCount": 2});
    vec![
        Row {
            why: "neither form",
            capacity: json!({}),
            accepted: false,
        },
        Row {
            why: "both forms at once",
            capacity: both_forms,
            accepted: false,
        },
        Row {
            why: "the provisioned form at the smallest documented mcuCount",
            capacity: json!({
                "provisionedCapacity": {"mcuCount": smallest_mcu_count, "workerCount": 1},
            }),
            accepted: true,
        },
        Row {
            why: "the provisioned form at the largest documented mcuCount",
            capacity: json!({
                "provisionedCapacity": {"mcuCount": largest_mcu_count, "workerCount": 3},
            }),
            accepted: true,
        },
        Row {
            why: "the auto-scaled form at the largest documented mcuCount",
            capacity: auto_scaling(largest_mcu_count, 1, 4, 4),
            accepted: true,
        },
        Row {
            why: "the provisioned form at an mcuCount the service does not allocate",
            capacity: json!({
                "provisionedCapacity": {"mcuCount": unallocatable_mcu_count, "workerCount": 2},
            }),
            accepted: false,
        },
        Row {
            why: "the auto-scaled form at an mcuCount the service does not allocate",
            capacity: auto_scaling(unallocatable_mcu_count, 1, 4, 4),
            accepted: false,
        },
        Row {
            why: "an auto-scaled worker window that is no window",
            capacity: auto_scaling(smallest_mcu_count, 4, 2, 4),
            accepted: false,
        },
        Row {
            why: "an auto-scaled task ceiling below the worker ceiling",
            capacity: auto_scaling(smallest_mcu_count, 1, 4, 2),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn a_create_and_an_update_answer_every_capacity_identically() {
    let emulator = emulator();
    let plugin_arn = active_plugin(&emulator, "capacity-rule-plugin").await;
    let rows = rows();

    // One running connector per row, so each update acts on its own connector
    // at its own version: created with a capacity every row can be compared
    // against, and left to the clock once.
    let mut connectors = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("capacity-rule-target-{index}");
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/connectors",
            connector_request(&name, &plugin_arn, &created_capacity()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the connector to update must be created, got {status} {created}",
            row.why
        );
        connectors.push(text(&created, "connectorArn"));
    }
    past_one_transition().await;

    for (index, row) in rows.iter().enumerate() {
        let connector_arn = &connectors[index];
        let before = describe(&emulator, connector_arn).await;
        let version = text(&before, "currentVersion");

        let (create_status, created) = call(
            &emulator,
            "POST",
            "/v1/connectors",
            connector_request(
                &format!("capacity-rule-created-{index}"),
                &plugin_arn,
                &row.capacity,
            ),
        )
        .await;
        let create_answer = Answer::of(create_status, &created);

        let (update_status, updated) = call(
            &emulator,
            "PUT",
            &format!(
                "/v1/connectors/{}?currentVersion={}",
                label(connector_arn),
                label(&version)
            ),
            json!({"capacity": row.capacity}),
        )
        .await;
        let update_answer = Answer::of(update_status, &updated);

        if row.accepted {
            assert_eq!(
                create_status, 200,
                "case {:?}: CreateConnector must accept this capacity, got {create_status} \
                 {created}",
                row.why
            );
            assert_eq!(
                update_status, 200,
                "case {:?}: UpdateConnector must accept this capacity, got {update_status} \
                 {updated}",
                row.why
            );
            continue;
        }

        assert_eq!(
            create_status, 400,
            "case {:?}: CreateConnector must refuse this capacity with HTTP 400, got \
             {create_status} {created}",
            row.why
        );
        assert_eq!(
            create_answer, update_answer,
            "case {:?}: the create and the update must answer one capacity alike, got \
             {create_answer:?} and {update_answer:?}",
            row.why
        );
        assert!(
            create_answer.code.contains("BadRequestException"),
            "case {:?}: expected BadRequestException, got {created}",
            row.why
        );
        assert!(
            create_answer.message.contains("Invalid parameter"),
            "case {:?}: the refusal must name the parameter it blames, got {:?}",
            row.why,
            create_answer.message
        );

        // A refused capacity stores nothing: no connector for the create, and
        // the updated connector still reports the capacity and the version it
        // was created with.
        assert_eq!(
            text(&created, "connectorArn"),
            "",
            "case {:?}: a refused create must mint no connector, got {created}",
            row.why
        );
        let after = describe(&emulator, connector_arn).await;
        assert_eq!(
            numbers(&after["capacity"]),
            numbers(&before["capacity"]),
            "case {:?}: a refused update must leave the capacity alone, got {after}",
            row.why
        );
        assert_eq!(
            text(&after, "currentVersion"),
            version,
            "case {:?}: a refused update must leave the version alone, got {after}",
            row.why
        );
    }
}

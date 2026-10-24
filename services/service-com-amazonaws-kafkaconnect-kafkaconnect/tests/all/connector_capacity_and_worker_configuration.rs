// SPDX-License-Identifier: CC0-1.0

//! What `CreateConnector` accepts for a capacity and for a worker-configuration
//! reference, end to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request rather than from state would fail here.
//!
//! The rules under test, each asserted over a SET of inputs rather than the one
//! request the issue cited:
//!
//! 1. A capacity's `mcuCount` is accepted exactly for the values the member
//!    documents ("The valid values are 1,2,4,8"), in either capacity form, and
//!    refused for every other integer — including the ones the modelled range
//!    `__integerMin1Max8` admits.
//! 2. An auto-scaled capacity's counts must bound each other the way their
//!    documentation says: `minWorkerCount` must not exceed `maxWorkerCount`, and a
//!    `maxAutoscalingTaskCount` the request names must be at least
//!    `maxWorkerCount`.
//! 3. A `workerConfiguration` is accepted only when this account holds an ACTIVE
//!    worker configuration at that ARN and revision; the accepted reference reads
//!    back through `DescribeConnector`.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

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

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `message` a failure response carries, or `""`.
fn message(body: &Value) -> String {
    body.get("message")
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

/// A well-formed IAM role ARN, as every accepted `CreateConnector` carries.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/connector-role")
}

/// A `CreateConnector` body on `plugin_arn` with the given capacity.
fn connector_body(name: &str, plugin_arn: &str, capacity: Value) -> Value {
    json!({
        "connectorName": name,
        "capacity": capacity,
        "connectorConfiguration": {"tasks.max": "1"},
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": "b-1.example:9098",
            "vpc": {"subnets": ["subnet-1", "subnet-2"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "IAM"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": role_arn(),
    })
}

/// A plugin created and settled into ACTIVE, with its ARN: a connector can only
/// be created against an ACTIVE plugin, so every row below needs one.
async fn active_plugin(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/custom-plugins",
        json!({
            "name": name,
            "contentType": "ZIP",
            "location": {"s3Location": {
                "bucketArn": "arn:aws:s3:::plugin-bucket",
                "fileKey": "plugins/p.zip",
            }},
        }),
    )
    .await;
    assert_eq!(status, 200, "creating {name} must succeed, got {created}");
    let arn = text(&created, "customPluginArn");
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    arn
}

/// A worker configuration created (ACTIVE at once), with its ARN.
async fn worker_configuration(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v1/worker-configurations",
        // `QUJD` is base64 of `ABC`: the body is stored verbatim, so any legal
        // base64 will do here.
        json!({"name": name, "propertiesFileContent": "QUJD"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the worker configuration {name} must succeed, got {created}"
    );
    text(&created, "workerConfigurationArn")
}

/// Assert that `body`'s message names `member` and goes on to give a reason: the
/// `Invalid parameter <member>: <reason>` form every kafkaconnect rejection uses.
/// Only the part the rule fixes is asserted, since the reason sentence for these
/// refusals is this emulator's own wording.
fn assert_names_member(case: &str, body: &Value, member: &str) {
    let actual = message(body);
    let prefix = format!("Invalid parameter {member}: ");
    assert!(
        actual.starts_with(&prefix) && actual.len() > prefix.len(),
        "case {case:?}: expected a message starting {prefix:?} and carrying a reason, got \
         {actual:?}"
    );
}

#[tokio::test]
async fn only_the_documented_mcu_counts_are_accepted_in_either_capacity_form() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, "mcu-plugin").await;

    // The whole neighbourhood of the documented set, so the rule under test is
    // the SET and not a range: zero and nine sit outside the modelled
    // `__integerMin1Max8` range, and 3, 5, 6, 7 sit inside it while still not
    // being MCU counts the member admits ("The valid values are 1,2,4,8" —
    // `AutoScaling$mcuCount` / `ProvisionedCapacity$mcuCount`).
    let candidates: [i32; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

    for mcu in candidates {
        // Both capacity forms carry the same member with the same documented
        // value set, so each candidate is tried in each form.
        let forms: [(&str, Value); 2] = [
            (
                "provisionedCapacity",
                json!({"provisionedCapacity": {"mcuCount": mcu, "workerCount": 2}}),
            ),
            (
                "autoScaling",
                json!({"autoScaling": {
                    "mcuCount": mcu,
                    "minWorkerCount": 1,
                    "maxWorkerCount": 4,
                }}),
            ),
        ];
        for (form, capacity) in forms {
            let case = format!("{form} with mcuCount {mcu}");
            let name = format!("mcu-{form}-{mcu}");
            let (status, answered) = call(
                &emulator,
                "POST",
                "/v1/connectors",
                connector_body(&name, &plugin, capacity),
            )
            .await;
            if MCU_COUNT_VALUES.contains(&mcu) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a documented MCU count must be accepted, got {status} \
                     {answered}"
                );
                assert_eq!(
                    text(&answered, "connectorName"),
                    name,
                    "case {case:?}: the created connector must be the one asked for, got \
                     {answered}"
                );
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?}: an MCU count outside the documented set must be refused, \
                     got {status} {answered}"
                );
                assert!(
                    error_code(&answered).contains("BadRequestException"),
                    "case {case:?}: expected BadRequestException, got {answered}"
                );
                // Inside the modelled range the refusal is the service's own, so
                // it names the member it refused; outside it the framework
                // refuses the modelled range before the handler runs.
                if (1..=8).contains(&mcu) {
                    assert_names_member(&case, &answered, "mcuCount");
                }
            }
        }
    }
}

/// One row of the auto-scaling relation table: the counts asked for, and the
/// member the refusal must name (`None` for a request that is accepted).
struct ScalingCase {
    min_worker_count: i32,
    max_worker_count: i32,
    /// `maxAutoscalingTaskCount`, or `None` to leave the optional member out.
    max_autoscaling_task_count: Option<i32>,
    refused_member: Option<&'static str>,
}

#[tokio::test]
async fn an_auto_scaled_capacity_s_counts_must_bound_each_other() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, "scaling-plugin").await;

    // The rows differ along the relations the members document: a window that is
    // a window (min below, equal to, or above max), and a task ceiling absent,
    // at `maxWorkerCount`, above it, or below it ("maxAutoscalingTaskCount …
    // Must be at least equal to maxWorkerCount").
    let rows: &[ScalingCase] = &[
        ScalingCase {
            min_worker_count: 1,
            max_worker_count: 4,
            max_autoscaling_task_count: None,
            refused_member: None,
        },
        ScalingCase {
            min_worker_count: 2,
            max_worker_count: 2,
            max_autoscaling_task_count: None,
            refused_member: None,
        },
        ScalingCase {
            min_worker_count: 1,
            max_worker_count: 4,
            max_autoscaling_task_count: Some(4),
            refused_member: None,
        },
        ScalingCase {
            min_worker_count: 1,
            max_worker_count: 4,
            max_autoscaling_task_count: Some(40),
            refused_member: None,
        },
        ScalingCase {
            min_worker_count: 8,
            max_worker_count: 2,
            max_autoscaling_task_count: None,
            refused_member: Some("minWorkerCount"),
        },
        ScalingCase {
            min_worker_count: 5,
            max_worker_count: 4,
            max_autoscaling_task_count: Some(10),
            refused_member: Some("minWorkerCount"),
        },
        ScalingCase {
            min_worker_count: 1,
            max_worker_count: 4,
            max_autoscaling_task_count: Some(3),
            refused_member: Some("maxAutoscalingTaskCount"),
        },
        ScalingCase {
            min_worker_count: 2,
            max_worker_count: 2,
            max_autoscaling_task_count: Some(1),
            refused_member: Some("maxAutoscalingTaskCount"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = format!(
            "min {} max {} tasks {:?}",
            row.min_worker_count, row.max_worker_count, row.max_autoscaling_task_count
        );
        let mut scaling = json!({
            "mcuCount": 2,
            "minWorkerCount": row.min_worker_count,
            "maxWorkerCount": row.max_worker_count,
        });
        if let Some(tasks) = row.max_autoscaling_task_count {
            scaling["maxAutoscalingTaskCount"] = json!(tasks);
        }
        let (status, answered) = call(
            &emulator,
            "POST",
            "/v1/connectors",
            connector_body(
                &format!("scaling-{index}"),
                &plugin,
                json!({"autoScaling": scaling}),
            ),
        )
        .await;

        match row.refused_member {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: counts that bound each other must be accepted, got {status} \
                     {answered}"
                );
                // And the accepted counts are what the connector reads back
                // with, from state.
                let arn = text(&answered, "connectorArn");
                let (status, described) = call(
                    &emulator,
                    "GET",
                    &format!("/v1/connectors/{}", label(&arn)),
                    Value::Null,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the created connector must read back, got {status} \
                     {described}"
                );
                let reported = &described["capacity"]["autoScaling"];
                assert_eq!(
                    reported["minWorkerCount"].as_i64(),
                    Some(i64::from(row.min_worker_count)),
                    "case {case:?}: the stored minWorkerCount must read back, got {described}"
                );
                assert_eq!(
                    reported["maxWorkerCount"].as_i64(),
                    Some(i64::from(row.max_worker_count)),
                    "case {case:?}: the stored maxWorkerCount must read back, got {described}"
                );
            }
            Some(member) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: counts that do not bound each other must be refused, got \
                     {status} {answered}"
                );
                assert!(
                    error_code(&answered).contains("BadRequestException"),
                    "case {case:?}: expected BadRequestException, got {answered}"
                );
                assert_names_member(&case, &answered, member);
            }
        }
    }
}

#[tokio::test]
async fn a_connector_is_only_created_against_a_worker_configuration_this_account_holds() {
    let emulator = emulator();
    let plugin = active_plugin(&emulator, "worker-config-plugin").await;
    let held = worker_configuration(&emulator, "held-configuration").await;

    // The reference the account holds, at the revision it holds: accepted, and
    // read back from state through DescribeConnector.
    let mut body = connector_body(
        "with-worker-configuration",
        &plugin,
        json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}}),
    );
    body["workerConfiguration"] = json!({"workerConfigurationArn": held, "revision": 1});
    let (status, created) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 200,
        "a held worker configuration must be accepted, got {status} {created}"
    );
    let connector = text(&created, "connectorArn");
    let (status, described) = call(
        &emulator,
        "GET",
        &format!("/v1/connectors/{}", label(&connector)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the created connector must read back, got {status} {described}"
    );
    let reported = &described["workerConfiguration"];
    assert_eq!(
        reported["workerConfigurationArn"].as_str(),
        Some(held.as_str()),
        "the stored workerConfigurationArn must read back, got {described}"
    );
    assert_eq!(
        reported["revision"].as_i64(),
        Some(1),
        "the stored revision must read back, got {described}"
    );

    // Every row below is a reference this account does not hold at that ARN and
    // revision: a name never created, the same name in another region, the held
    // ARN with its uuid altered, a value that is no ARN at all, and the held ARN
    // at revisions the configuration does not have.
    let (head, last) = held.split_at(held.len() - 1);
    let altered = format!("{head}{}", if last == "0" { "1" } else { "0" });
    let unheld: Vec<(&str, String, i64, &str)> = vec![
        (
            "a name that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/ghost/\
                 00000000-0000-4000-8000-000000000000"
            ),
            1,
            "workerConfigurationArn",
        ),
        (
            "the held name in another region",
            held.replace(REGION, "us-east-2"),
            1,
            "workerConfigurationArn",
        ),
        (
            "the held ARN with a different uuid",
            altered,
            1,
            "workerConfigurationArn",
        ),
        (
            "not an ARN at all",
            "worker-configuration".to_string(),
            1,
            "workerConfigurationArn",
        ),
        (
            "the held ARN at the next revision",
            held.clone(),
            2,
            "revision",
        ),
        (
            "the held ARN at a far revision",
            held.clone(),
            99,
            "revision",
        ),
    ];

    for (index, (case, arn, revision, member)) in unheld.iter().enumerate() {
        let mut body = connector_body(
            &format!("dangling-{index}"),
            &plugin,
            json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}}),
        );
        body["workerConfiguration"] =
            json!({"workerConfigurationArn": arn, "revision": revision});
        let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a reference this account does not hold is a 404, got {status} \
             {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
        assert_names_member(case, &answered, member);

        // And nothing was stored: the refused name is still free.
        let (status, listed) = call(&emulator, "GET", "/v1/connectors", Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConnectors must succeed, got {status} {listed}"
        );
        let names: Vec<String> = listed["connectors"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|connector| text(connector, "connectorName"))
            .collect();
        assert!(
            !names.contains(&format!("dangling-{index}")),
            "case {case:?}: a refused create must store nothing, got {names:?}"
        );
    }

    // A configuration on its way out of the account cannot back a connector
    // either: between the delete and the removal the clock owes, the reference
    // resolves to a DELETING resource, which is refused as a bad request.
    let (status, deleted) = call(
        &emulator,
        "DELETE",
        &format!("/v1/worker-configurations/{}", label(&held)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete must be accepted, got {status} {deleted}"
    );
    assert_eq!(
        text(&deleted, "workerConfigurationState"),
        "DELETING",
        "the deleted configuration must be DELETING, got {deleted}"
    );
    let mut body = connector_body(
        "on-deleting-configuration",
        &plugin,
        json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}}),
    );
    body["workerConfiguration"] = json!({"workerConfigurationArn": held, "revision": 1});
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 400,
        "a connector on a DELETING worker configuration is a 400, got {status} {answered}"
    );
    assert!(
        error_code(&answered).contains("BadRequestException"),
        "expected BadRequestException, got {answered}"
    );
    assert_names_member(
        "a configuration that is DELETING",
        &answered,
        "workerConfigurationArn",
    );

    // Once the clock has dropped it, the same reference names nothing at all.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    let mut body = connector_body(
        "on-dropped-configuration",
        &plugin,
        json!({"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}}),
    );
    body["workerConfiguration"] = json!({"workerConfigurationArn": held, "revision": 1});
    let (status, answered) = call(&emulator, "POST", "/v1/connectors", body).await;
    assert_eq!(
        status, 404,
        "a connector on a dropped worker configuration is a 404, got {status} {answered}"
    );
    assert_names_member(
        "a configuration the clock has dropped",
        &answered,
        "workerConfigurationArn",
    );
}

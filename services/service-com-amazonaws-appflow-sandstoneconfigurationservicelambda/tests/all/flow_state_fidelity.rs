// SPDX-License-Identifier: CC0-1.0

//! The STATE a flow read reports, end to end over HTTP: the status a flow is stored
//! with, the configuration containers AppFlow materialises on it, and the key that
//! encrypts it.
//!
//! The rules under test:
//!
//! 1. A flow's stored `flowStatus` follows the trigger that runs it: a flow AppFlow runs
//!    only when asked is finished when it is stored and is therefore `Active`; a flow
//!    that runs itself — on a schedule or on a source event — is stored as a `Draft`
//!    whose trigger `StartFlow` arms, and a read of the flow reports that same status.
//! 2. A flow's stored definition carries the configuration containers AppFlow
//!    materialises — the trigger's properties, each task's properties, the prefix
//!    configuration of an S3 output format, and the metadata catalog configuration — so
//!    a read answers with the container (empty when nothing in it was configured) rather
//!    than omitting it, and with the values the request put inside it when it did.
//! 3. A flow reports the key its data is encrypted with: the one its definition named,
//!    or the account's AppFlow-managed key when it named none
//!    (`smithy_query(query="doc", shape="CreateFlowRequest", member="kmsArn")`).
//! 4. Both read paths project the SAME stored record: `DescribeFlow` reports it in full
//!    and `ListFlows` enumerates every stored flow as a `FlowDefinition` carrying that
//!    flow's own status, connector types and trigger type.
//!
//! The harness (`call`, `Answer`, the shape builders) is the one `flow_lifecycle` drives
//! the service with, so every case reaches the service exactly the way a client does.

use serde_json::{json, Value};

use super::flow_lifecycle::{
    appflow, call, event_bridge_destination, event_trigger, map_task, on_demand_trigger,
    s3_destination, s3_source, salesforce_source, scheduled_trigger, Answer, ACCOUNT,
};

/// The key a case that names its own KMS key uses.
fn named_kms_arn() -> String {
    format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/9fd1f0a4-7b1a-4a7f-9a3a-1d2c3b4a5e6f")
}

/// The AppFlow-managed key every flow that names no key of its own reports:
/// `arn:aws:kms:<region>:<account>:key/<key-id>`, with a key id this emulator mints once
/// for the account.
fn is_managed_key_arn(arn: &str) -> bool {
    arn.starts_with(&format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/"))
        && arn.len() > format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/").len()
}

/// An S3 destination whose output format is configured but whose PREFIX configuration is
/// not: the container AppFlow materialises inside `s3OutputFormatConfig`.
fn s3_destination_with_output_format(bucket: &str, file_type: &str) -> Value {
    json!({
        "connectorType": "S3",
        "destinationConnectorProperties": {
            "S3": {
                "bucketName": bucket,
                "bucketPrefix": "outbound",
                "s3OutputFormatConfig": { "fileType": file_type }
            }
        }
    })
}

/// An S3 destination whose prefix configuration IS given: the values inside a
/// materialised container must come back exactly as they arrived. The member values are
/// the model's (`smithy_query(query="shape", shape="PrefixType,PrefixFormat")`).
fn s3_destination_with_prefix_config(bucket: &str) -> Value {
    json!({
        "connectorType": "S3",
        "destinationConnectorProperties": {
            "S3": {
                "bucketName": bucket,
                "s3OutputFormatConfig": {
                    "fileType": "PARQUET",
                    "prefixConfig": {
                        "prefixType": "PATH",
                        "prefixFormat": "DAY",
                        "pathPrefixHierarchy": ["EXECUTION_ID"]
                    }
                }
            }
        }
    })
}

/// A `Map` task that carries NO `taskProperties`: the container AppFlow materialises on
/// every task.
fn map_task_without_properties(source_field: &str, destination_field: &str) -> Value {
    json!({
        "taskType": "Map",
        "sourceFields": [source_field],
        "destinationField": destination_field,
        "connectorOperator": { "S3": "PROJECTION" }
    })
}

/// A `Map` task whose `taskProperties` carry a value, with the property key the model
/// lists (`smithy_query(query="shape", shape="OperatorPropertiesKeys")`).
fn map_task_with_properties(source_field: &str, destination_field: &str) -> Value {
    json!({
        "taskType": "Map",
        "sourceFields": [source_field],
        "destinationField": destination_field,
        "connectorOperator": { "S3": "PROJECTION" },
        "taskProperties": { "SOURCE_DATA_TYPE": "string" }
    })
}

/// One row of the table: a `CreateFlow` body, the status the trigger type implies, and
/// whether the row names its own encryption key.
struct Case {
    name: &'static str,
    request: Value,
    expected_status: &'static str,
    names_own_key: bool,
}

/// The table. The rows differ along what the rules generalise over: the trigger type
/// (every value of the model's `TriggerType` — see
/// `smithy_query(query="shape", shape="TriggerType")`), the destination connector (one
/// with an S3 output format, one with a prefix configuration of its own, one with no
/// output format at all), the number of tasks and whether they carry properties, whether
/// the request names an encryption key, and whether it names a metadata catalog.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "on-demand, sparse: no container given anywhere",
            request: json!({
                "flowName": "sparse-ondemand-flow",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("sparse-in"),
                "destinationFlowConfigList": [s3_destination_with_output_format("sparse-out", "JSON")],
                "tasks": [map_task_without_properties("order_id", "id")]
            }),
            expected_status: "Active",
            names_own_key: false,
        },
        Case {
            name: "scheduled, two tasks, one carrying properties",
            request: json!({
                "flowName": "scheduled-fidelity-flow",
                "triggerConfig": scheduled_trigger("rate(1hours)"),
                "sourceFlowConfig": salesforce_source("Account"),
                "destinationFlowConfigList": [s3_destination_with_output_format("sf-archive", "CSV")],
                "tasks": [
                    map_task_without_properties("Id", "id"),
                    map_task_with_properties("Name", "name")
                ]
            }),
            expected_status: "Draft",
            names_own_key: false,
        },
        Case {
            name: "event, destination without an output format, own kms key",
            request: json!({
                "flowName": "event-fidelity-flow",
                "triggerConfig": event_trigger(),
                "kmsArn": named_kms_arn(),
                "sourceFlowConfig": s3_source("events-in"),
                "destinationFlowConfigList": [event_bridge_destination("appflow-events")],
                "tasks": [map_task("event_id", "id")]
            }),
            expected_status: "Draft",
            names_own_key: true,
        },
        Case {
            name: "on-demand, every container given with values",
            request: json!({
                "flowName": "populated-ondemand-flow",
                "triggerConfig": on_demand_trigger(),
                "kmsArn": named_kms_arn(),
                "sourceFlowConfig": s3_source("populated-in"),
                "destinationFlowConfigList": [s3_destination_with_prefix_config("populated-out")],
                "tasks": [map_task_with_properties("ts", "timestamp")],
                "metadataCatalogConfig": {
                    "glueDataCatalog": {
                        "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/appflow-catalog"),
                        "databaseName": "appflow_db",
                        "tablePrefix": "events"
                    }
                }
            }),
            expected_status: "Active",
            names_own_key: true,
        },
        Case {
            name: "on-demand, two destinations, the second without an output format",
            request: json!({
                "flowName": "two-destination-flow",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("fanout-in"),
                "destinationFlowConfigList": [
                    s3_destination_with_output_format("fanout-out", "JSON"),
                    s3_destination("fanout-plain")
                ],
                "tasks": [map_task_without_properties("id", "id")]
            }),
            expected_status: "Active",
            names_own_key: false,
        },
    ]
}

/// The S3 output format of the destination at `index`, or `Value::Null`.
fn s3_output_format(described: &Answer, index: usize) -> &Value {
    described
        .get("destinationFlowConfigList")
        .get(index)
        .and_then(|destination| destination.get("destinationConnectorProperties"))
        .and_then(|properties| properties.get("S3"))
        .and_then(|s3| s3.get("s3OutputFormatConfig"))
        .unwrap_or(&Value::Null)
}

#[tokio::test]
async fn describe_flow_reports_the_whole_stored_flow_state() {
    let router = appflow();

    for case in cases() {
        let flow_name = case.request["flowName"].as_str().expect("a flow name");
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );
        assert_eq!(
            created.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected the status the trigger type implies, got {:?}",
            case.name,
            created.get("flowStatus")
        );

        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert!(
            described.is_ok(),
            "case {:?}: expected DescribeFlow to succeed, got {} {:?}",
            case.name,
            described.status,
            described.body
        );

        // Rule 1: the read reports the status the flow was stored with.
        assert_eq!(
            described.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected DescribeFlow to report the stored status, got {:?}",
            case.name,
            described.get("flowStatus")
        );

        // Rule 2: every materialised container is PRESENT, whether or not the request
        // carried it.
        assert!(
            described.get("triggerConfig").get("triggerProperties").is_some(),
            "case {:?}: expected triggerConfig.triggerProperties to be present, got {:?}",
            case.name,
            described.get("triggerConfig")
        );
        assert!(
            described.get("metadataCatalogConfig").is_object(),
            "case {:?}: expected metadataCatalogConfig to be present, got {:?}",
            case.name,
            described.get("metadataCatalogConfig")
        );
        let tasks = described.get("tasks").as_array().cloned().unwrap_or_default();
        assert_eq!(
            tasks.len(),
            case.request["tasks"].as_array().map_or(0, Vec::len),
            "case {:?}: expected every stored task, got {:?}",
            case.name,
            described.get("tasks")
        );
        for (index, task) in tasks.iter().enumerate() {
            assert!(
                task.get("taskProperties").is_some_and(Value::is_object),
                "case {:?}: expected tasks[{index}].taskProperties to be present, got {task:?}",
                case.name
            );
        }
        let destinations = case.request["destinationFlowConfigList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (index, destination) in destinations.iter().enumerate() {
            let sent_output_format = destination
                .get("destinationConnectorProperties")
                .and_then(|properties| properties.get("S3"))
                .and_then(|s3| s3.get("s3OutputFormatConfig"));
            let reported = s3_output_format(&described, index);
            match sent_output_format {
                // An output format the request gave carries a prefix configuration
                // afterwards, even when the request named none.
                Some(sent) => {
                    assert!(
                        reported.get("prefixConfig").is_some_and(Value::is_object),
                        "case {:?}: expected destination {index} to report a prefixConfig, got {reported:?}",
                        case.name
                    );
                    if let Some(sent_prefix) = sent.get("prefixConfig") {
                        assert_eq!(
                            reported.get("prefixConfig"),
                            Some(sent_prefix),
                            "case {:?}: expected destination {index} to report the stored prefixConfig, got {reported:?}",
                            case.name
                        );
                    }
                    assert_eq!(
                        reported.get("fileType"),
                        sent.get("fileType"),
                        "case {:?}: expected destination {index} to report the stored fileType, got {reported:?}",
                        case.name
                    );
                }
                // Nothing is invented for a destination that named no output format.
                None => assert!(
                    reported.is_null(),
                    "case {:?}: expected destination {index} to report no s3OutputFormatConfig, got {reported:?}",
                    case.name
                ),
            }
        }
        // A container the request filled keeps every value it carried.
        if let Some(sent_catalog) = case.request.get("metadataCatalogConfig") {
            assert_eq!(
                described.get("metadataCatalogConfig"),
                sent_catalog,
                "case {:?}: expected the stored metadataCatalogConfig, got {:?}",
                case.name,
                described.get("metadataCatalogConfig")
            );
        }
        for (index, sent_task) in case.request["tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if let Some(sent_properties) = sent_task.get("taskProperties") {
                assert_eq!(
                    tasks[index].get("taskProperties"),
                    Some(sent_properties),
                    "case {:?}: expected tasks[{index}] to report the stored taskProperties, got {:?}",
                    case.name,
                    tasks[index]
                );
            }
        }

        // Rule 3: the flow reports the key that encrypts it.
        let kms_arn = described.string("kmsArn");
        if case.names_own_key {
            assert_eq!(
                kms_arn,
                named_kms_arn(),
                "case {:?}: expected the key the definition named, got {kms_arn:?}",
                case.name
            );
        } else {
            assert!(
                is_managed_key_arn(kms_arn),
                "case {:?}: expected the AppFlow-managed key ARN, got {kms_arn:?}",
                case.name
            );
        }

        // A read changes nothing a later caller sees: the same request answers the same.
        let described_again =
            call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert_eq!(
            described_again.body, described.body,
            "case {:?}: expected a second DescribeFlow to answer identically, got {:?}",
            case.name, described_again.body
        );
    }
}

#[tokio::test]
async fn every_flow_without_its_own_key_reports_the_same_managed_key() {
    let router = appflow();
    let mut managed_keys: Vec<String> = Vec::new();

    for case in cases() {
        let flow_name = case.request["flowName"].as_str().expect("a flow name");
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );
        if case.names_own_key {
            continue;
        }
        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        managed_keys.push(described.string("kmsArn").to_string());
    }

    assert!(
        managed_keys.len() > 1,
        "the table must hold more than one flow without its own key, got {managed_keys:?}"
    );
    assert!(
        managed_keys.windows(2).all(|pair| pair[0] == pair[1]),
        "expected one managed key for the account, got {managed_keys:?}"
    );
}

#[tokio::test]
async fn list_flows_enumerates_every_stored_flow_with_its_own_state() {
    let router = appflow();
    let cases = cases();
    for case in &cases {
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );
    }

    let listed = call(&router, "/list-flows", &json!({})).await;
    assert!(
        listed.is_ok(),
        "expected ListFlows to succeed, got {} {:?}",
        listed.status,
        listed.body
    );
    let flows = listed.get("flows").as_array().cloned().unwrap_or_default();
    assert_eq!(
        flows.len(),
        cases.len(),
        "expected every created flow to be enumerated, got {flows:?}"
    );

    for case in &cases {
        let flow_name = case.request["flowName"].as_str().expect("a flow name");
        let entry = flows
            .iter()
            .find(|flow| flow.get("flowName").and_then(Value::as_str) == Some(flow_name))
            .unwrap_or_else(|| {
                panic!("case {:?}: expected {flow_name} to be enumerated, got {flows:?}", case.name)
            });
        assert_eq!(
            entry.get("flowStatus").and_then(Value::as_str),
            Some(case.expected_status),
            "case {:?}: expected the summary to carry the stored status, got {entry:?}",
            case.name
        );
        assert_eq!(
            entry.get("triggerType"),
            case.request["triggerConfig"].get("triggerType"),
            "case {:?}: expected the summary to carry the stored trigger type, got {entry:?}",
            case.name
        );
        assert_eq!(
            entry.get("sourceConnectorType"),
            case.request["sourceFlowConfig"].get("connectorType"),
            "case {:?}: expected the summary to carry the stored source connector, got {entry:?}",
            case.name
        );
        assert_eq!(
            entry.get("destinationConnectorType"),
            case.request["destinationFlowConfigList"][0].get("connectorType"),
            "case {:?}: expected the summary to carry the stored destination connector, got {entry:?}",
            case.name
        );
        assert_eq!(
            entry.get("flowArn").and_then(Value::as_str),
            Some(format!("arn:aws:appflow:us-east-1:{ACCOUNT}:flow/{flow_name}").as_str()),
            "case {:?}: expected the summary to carry the minted ARN, got {entry:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn start_flow_runs_an_on_demand_flow_and_arms_a_self_triggered_one() {
    let router = appflow();

    for case in cases() {
        let flow_name = case.request["flowName"].as_str().expect("a flow name");
        let created = call(&router, "/create-flow", &case.request).await;
        assert!(
            created.is_ok(),
            "case {:?}: expected CreateFlow to succeed, got {} {:?}",
            case.name,
            created.status,
            created.body
        );

        let started = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
        assert!(
            started.is_ok(),
            "case {:?}: expected StartFlow to succeed, got {} {:?}",
            case.name,
            started.status,
            started.body
        );
        let on_demand = case.request["triggerConfig"]["triggerType"] == json!("OnDemand");
        if on_demand {
            // An on-demand flow is RUN: it reports the run it started, and stays active.
            assert!(
                !started.string("executionId").is_empty(),
                "case {:?}: expected a run id for an on-demand flow, got {:?}",
                case.name,
                started.get("executionId")
            );
        } else {
            // A flow that runs itself is ARMED: no run starts, so there is no run id.
            assert!(
                started.get("executionId").is_null(),
                "case {:?}: expected no run id for a self-triggered flow, got {:?}",
                case.name,
                started.get("executionId")
            );
        }
        assert_eq!(
            started.string("flowStatus"),
            "Active",
            "case {:?}: expected the flow to be active once started, got {:?}",
            case.name,
            started.get("flowStatus")
        );

        // The status the start left behind is what a later read reports.
        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert_eq!(
            described.string("flowStatus"),
            "Active",
            "case {:?}: expected DescribeFlow to report the activated flow, got {:?}",
            case.name,
            described.get("flowStatus")
        );

        if !on_demand {
            // Its trigger is armed now, so there is nothing left to start.
            let again = call(&router, "/start-flow", &json!({ "flowName": flow_name })).await;
            assert_eq!(
                (again.status, again.error.as_str()),
                (409, "ConflictException"),
                "case {:?}: expected a second StartFlow to conflict, got {} {:?}",
                case.name,
                again.status,
                again.body
            );
        }
    }
}

#[tokio::test]
async fn update_flow_stores_the_same_materialised_containers_create_flow_does() {
    let router = appflow();
    let flow_name = "updated-fidelity-flow";
    let created = call(
        &router,
        "/create-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("update-in"),
            "destinationFlowConfigList": [s3_destination_with_prefix_config("update-out")],
            "tasks": [map_task_with_properties("id", "id")]
        }),
    )
    .await;
    assert!(
        created.is_ok(),
        "expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );

    // The update carries NONE of the containers: the flow must still report them.
    let updated = call(
        &router,
        "/update-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("update-in"),
            "destinationFlowConfigList": [s3_destination_with_output_format("update-out", "JSON")],
            "tasks": [map_task_without_properties("id", "id")]
        }),
    )
    .await;
    assert!(
        updated.is_ok(),
        "expected UpdateFlow to succeed, got {} {:?}",
        updated.status,
        updated.body
    );

    let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    assert!(
        described.get("triggerConfig").get("triggerProperties").is_some(),
        "expected triggerConfig.triggerProperties after an update, got {:?}",
        described.get("triggerConfig")
    );
    assert!(
        described.get("metadataCatalogConfig").is_object(),
        "expected metadataCatalogConfig after an update, got {:?}",
        described.get("metadataCatalogConfig")
    );
    assert!(
        s3_output_format(&described, 0)
            .get("prefixConfig")
            .is_some_and(Value::is_object),
        "expected a materialised prefixConfig after an update, got {:?}",
        s3_output_format(&described, 0)
    );
    let tasks = described.get("tasks").as_array().cloned().unwrap_or_default();
    assert!(
        tasks
            .iter()
            .all(|task| task.get("taskProperties").is_some_and(Value::is_object)),
        "expected every task to report its properties after an update, got {tasks:?}"
    );
    // The encryption key is not an `UpdateFlow` member, so the key the create resolved
    // is still the flow's.
    assert!(
        is_managed_key_arn(described.string("kmsArn")),
        "expected the managed key to survive an update, got {:?}",
        described.get("kmsArn")
    );
}

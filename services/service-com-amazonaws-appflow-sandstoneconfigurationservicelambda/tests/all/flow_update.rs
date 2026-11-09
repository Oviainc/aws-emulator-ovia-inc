// SPDX-License-Identifier: CC0-1.0

//! `UpdateFlow`, end to end over HTTP: a stored flow is RE-DEFINED, and every later
//! read answers from the definition the update left behind.
//!
//! The rule under test: `UpdateFlow` replaces the mutable definition of an existing
//! flow (its description, trigger, source and destination configuration, tasks and
//! catalog configuration), leaves its identity alone (ARN, name, tags, encryption key,
//! creation record, run history), records who updated it and when, finishes a flow that
//! was still a draft by activating it, and raises
//! `schemaVersion` by exactly one when the data schema changed — the field mappings,
//! the field data types and the partition keys, which all live in `tasks`
//! (`smithy_query(query="doc", shape="DescribeFlowResponse", member="schemaVersion")`
//! and `smithy_query(query="shape", shape="Task,TaskType,OperatorPropertiesKeys")`).
//!
//! The harness (`call`, `Answer`, the shape builders) is the one `flow_lifecycle`
//! already drives the service with, so these cases reach the service exactly the way a
//! client does.

use serde_json::{json, Value};

use super::flow_lifecycle::{
    appflow, call, contains, event_bridge_destination, event_trigger, filter_task, map_task,
    on_demand_trigger, s3_destination, s3_source, salesforce_source, scheduled_trigger, Answer,
    ACCOUNT,
};

/// The KMS key the base flow is created with: `UpdateFlow` carries no `kmsArn`, so it
/// must still be there afterwards.
fn base_kms_arn() -> String {
    format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555")
}

/// A `Partition` task naming its partition keys, the task type and property the model
/// carries them in (`smithy_query(query="shape", shape="TaskType")` lists `Partition`;
/// `smithy_query(query="shape", shape="OperatorPropertiesKeys")` lists
/// `ORDERED_PARTITION_KEYS_LIST`).
fn partition_task(partition_keys: &str) -> Value {
    json!({
        "taskType": "Partition",
        "sourceFields": [],
        "connectorOperator": { "S3": "NO_OP" },
        "taskProperties": { "ORDERED_PARTITION_KEYS_LIST": partition_keys }
    })
}

/// A `Map` task that also states the data type of its destination field
/// (`DESTINATION_DATA_TYPE`, from the same `OperatorPropertiesKeys` enum).
fn typed_map_task(source_field: &str, destination_field: &str, data_type: &str) -> Value {
    json!({
        "taskType": "Map",
        "sourceFields": [source_field],
        "destinationField": destination_field,
        "connectorOperator": { "S3": "PROJECTION" },
        "taskProperties": { "DESTINATION_DATA_TYPE": data_type }
    })
}

/// The flow every case starts from: on-demand, S3 to S3, one field mapping, a
/// description, tags and a customer key — so an update can be seen to replace what it
/// carries and to leave the rest where it is.
fn base_flow(flow_name: &str) -> Value {
    json!({
        "flowName": flow_name,
        "description": "Nightly extract of orders",
        "triggerConfig": on_demand_trigger(),
        "kmsArn": base_kms_arn(),
        "sourceFlowConfig": s3_source("orders-landing"),
        "destinationFlowConfigList": [s3_destination("orders-curated")],
        "tasks": [map_task("order_id", "id")],
        "tags": { "env": "test", "team": "data" }
    })
}

/// The definition members `UpdateFlow` carries, and therefore replaces.
const UPDATABLE_MEMBERS: [&str; 6] = [
    "description",
    "triggerConfig",
    "sourceFlowConfig",
    "destinationFlowConfigList",
    "tasks",
    "metadataCatalogConfig",
];

/// One row of the update table: the definition the update carries, the status the flow
/// then has (ACTIVE for every row — the status of an updated flow does not depend on
/// what the new definition says, which is why the column is carried per row rather
/// than assumed), and the `schemaVersion` it then carries (the base flow is version 1).
struct UpdateCase {
    name: &'static str,
    update: Value,
    expected_status: &'static str,
    expected_schema_version: i64,
}

/// The update table. The rows differ along what the rule generalises over: WHICH
/// members the new definition changes, how many tasks it carries, whether the data
/// schema moved (a mapping, a data type, a partition key) and whether the kind of
/// trigger moved.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            name: "only the description changes",
            update: json!({
                "description": "Hourly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "the description is omitted, so it is cleared",
            update: json!({
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "a field mapping changes",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "order_identifier")]
            }),
            expected_status: "Active",
            expected_schema_version: 2,
        },
        UpdateCase {
            name: "a filter task is added, which declares no part of the data schema",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id"), filter_task(&["order_id", "total"])]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "a field data type is declared",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [typed_map_task("order_id", "id", "string")]
            }),
            expected_status: "Active",
            expected_schema_version: 2,
        },
        UpdateCase {
            name: "partition keys are added",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id"), partition_task("order_date")]
            }),
            expected_status: "Active",
            expected_schema_version: 2,
        },
        UpdateCase {
            name: "the source and the destination are replaced, the tasks are not",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": salesforce_source("Order"),
                "destinationFlowConfigList": [s3_destination("orders-archive")],
                "tasks": [map_task("order_id", "id")]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "the trigger becomes scheduled, which arms the flow",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": scheduled_trigger("rate(1hours)"),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "the trigger becomes event-driven, which also arms the flow",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": event_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [event_bridge_destination("appflow-orders")],
                "tasks": [map_task("order_id", "id")]
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
        UpdateCase {
            name: "a catalog configuration is added",
            update: json!({
                "description": "Nightly extract of orders",
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")],
                "metadataCatalogConfig": {
                    "glueDataCatalog": {
                        "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/appflow-catalog"),
                        "databaseName": "appflow_db",
                        "tablePrefix": "orders"
                    }
                }
            }),
            expected_status: "Active",
            expected_schema_version: 1,
        },
    ]
}

/// Create the base flow and answer what `DescribeFlow` reports for it.
async fn create_base_flow(
    router: &smithy_server_runtime::MultiServiceRouter,
    case_name: &str,
    flow_name: &str,
) -> Answer {
    let created = call(router, "/create-flow", &base_flow(flow_name)).await;
    assert!(
        created.is_ok(),
        "case {case_name:?}: expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );
    let described = call(router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    assert!(
        described.is_ok(),
        "case {case_name:?}: expected DescribeFlow to succeed, got {} {:?}",
        described.status,
        described.body
    );
    described
}

#[tokio::test]
async fn update_flow_replaces_the_definition_describe_flow_returns() {
    let router = appflow();

    for (index, case) in update_cases().into_iter().enumerate() {
        let flow_name = format!("update-case-{index}");
        let before = create_base_flow(&router, case.name, &flow_name).await;
        // The base flow runs on demand, so it was stored ready to run; an update does not
        // move it out of that status.
        assert_eq!(
            before.string("flowStatus"),
            "Active",
            "case {:?}: expected the created on-demand flow to be active, got {:?}",
            case.name,
            before.get("flowStatus")
        );

        let mut request = case.update.clone();
        request["flowName"] = json!(flow_name);
        let updated = call(&router, "/update-flow", &request).await;
        assert!(
            updated.is_ok(),
            "case {:?}: expected UpdateFlow to succeed, got {} {:?}",
            case.name,
            updated.status,
            updated.body
        );
        assert_eq!(
            updated.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected UpdateFlow to answer with the flow's status, got {:?}",
            case.name,
            updated.get("flowStatus")
        );

        let after = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert!(
            after.is_ok(),
            "case {:?}: expected DescribeFlow to succeed after the update, got {} {:?}",
            case.name,
            after.status,
            after.body
        );

        // Every member the update carried is what a later read returns; a member it
        // omitted is gone, because an update re-defines the flow rather than patching it
        // — except for a container AppFlow materialises on every flow, which is left
        // empty rather than absent.
        for member in UPDATABLE_MEMBERS {
            match request.get(member) {
                Some(sent) => assert!(
                    contains(after.get(member), sent),
                    "case {:?}: expected DescribeFlow to return the updated {member}\n  sent: {sent}\n  got:  {}",
                    case.name,
                    after.get(member)
                ),
                None if member == "metadataCatalogConfig" => assert_eq!(
                    after.get(member),
                    &json!({}),
                    "case {:?}: expected the omitted {member} to be cleared to its empty container, got {:?}",
                    case.name,
                    after.get(member)
                ),
                None => assert!(
                    after.get(member).is_null(),
                    "case {:?}: expected the omitted {member} to be cleared, got {:?}",
                    case.name,
                    after.get(member)
                ),
            }
        }

        // The flow's IDENTITY is not part of a re-definition.
        for member in ["flowArn", "flowName", "kmsArn", "createdAt", "createdBy"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {:?}: expected {member} to survive the update unchanged, got {:?}",
                case.name,
                after.get(member)
            );
        }
        assert_eq!(
            after.get("tags"),
            before.get("tags"),
            "case {:?}: expected the tags to survive the update, got {:?}",
            case.name,
            after.get("tags")
        );
        assert!(
            after.get("lastUpdatedAt").as_f64().unwrap_or_default()
                > before.get("lastUpdatedAt").as_f64().unwrap_or_default(),
            "case {:?}: expected lastUpdatedAt to carry the instant of the update, got {:?} (was {:?})",
            case.name,
            after.get("lastUpdatedAt"),
            before.get("lastUpdatedAt")
        );
        let who = after.string("lastUpdatedBy");
        assert!(
            who.contains(ACCOUNT) && !who.contains(char::is_whitespace),
            "case {:?}: expected lastUpdatedBy to name the caller, got {who:?}",
            case.name
        );
        assert_eq!(
            after.get("schemaVersion").as_i64(),
            Some(case.expected_schema_version),
            "case {:?}: expected schemaVersion {}, got {:?}",
            case.name,
            case.expected_schema_version,
            after.get("schemaVersion")
        );
        assert_eq!(
            after.string("flowStatus"),
            case.expected_status,
            "case {:?}: expected the stored status, got {:?}",
            case.name,
            after.get("flowStatus")
        );
    }
}

/// `schemaVersion` increases by ONE per data-schema change and stands still otherwise,
/// however many updates are applied. The rows are the successive updates, each with the
/// version the flow must carry after it.
#[tokio::test]
async fn schema_version_counts_only_data_schema_changes() {
    let router = appflow();
    let flow_name = "schema-version-flow";
    let before = create_base_flow(&router, "schemaVersion walk", flow_name).await;
    assert_eq!(
        before.get("schemaVersion").as_i64(),
        Some(1),
        "expected the created flow to carry the first schema version, got {:?}",
        before.get("schemaVersion")
    );

    let steps: Vec<(&str, Value, i64)> = vec![
        (
            "the same tasks again",
            json!([map_task("order_id", "id")]),
            1,
        ),
        (
            "a new destination field",
            json!([map_task("order_id", "order_key")]),
            2,
        ),
        (
            "that same mapping once more",
            json!([map_task("order_id", "order_key")]),
            2,
        ),
        (
            "a data type on the mapping",
            json!([typed_map_task("order_id", "order_key", "long")]),
            3,
        ),
        (
            "partition keys beside it",
            json!([
                typed_map_task("order_id", "order_key", "long"),
                partition_task("order_month")
            ]),
            4,
        ),
        (
            "different partition keys",
            json!([
                typed_map_task("order_id", "order_key", "long"),
                partition_task("order_day")
            ]),
            5,
        ),
        (
            "the tasks in the other order, which declare the same schema",
            json!([
                partition_task("order_day"),
                typed_map_task("order_id", "order_key", "long")
            ]),
            5,
        ),
        (
            "a filter beside them, which declares no part of the schema",
            json!([
                partition_task("order_day"),
                typed_map_task("order_id", "order_key", "long"),
                filter_task(&["order_id"])
            ]),
            5,
        ),
    ];

    for (case, tasks, expected_version) in steps {
        let updated = call(
            &router,
            "/update-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": tasks
            }),
        )
        .await;
        assert!(
            updated.is_ok(),
            "case {case:?}: expected UpdateFlow to succeed, got {} {:?}",
            updated.status,
            updated.body
        );
        let described = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
        assert_eq!(
            described.get("schemaVersion").as_i64(),
            Some(expected_version),
            "case {case:?}: expected schemaVersion {expected_version}, got {:?}",
            described.get("schemaVersion")
        );
        assert!(
            contains(described.get("tasks"), &tasks),
            "case {case:?}: expected the updated tasks to be stored, got {:?}",
            described.get("tasks")
        );
    }
}

#[tokio::test]
async fn updating_a_flow_that_does_not_exist_is_not_found() {
    let router = appflow();
    // The flow names differ along what the rule generalises over: a name nothing ever
    // created, and one that WAS created and then deleted.
    let created = call(&router, "/create-flow", &base_flow("deleted-flow")).await;
    assert!(
        created.is_ok(),
        "expected CreateFlow to succeed, got {} {:?}",
        created.status,
        created.body
    );
    // An on-demand flow is active from the moment it is created, so deleting it is the
    // forced delete AppFlow requires for a flow that is in use.
    let deleted = call(
        &router,
        "/delete-flow",
        &json!({ "flowName": "deleted-flow", "forceDelete": true }),
    )
    .await;
    assert!(
        deleted.is_ok(),
        "expected DeleteFlow to succeed, got {} {:?}",
        deleted.status,
        deleted.body
    );

    for flow_name in ["never-created-flow", "deleted-flow"] {
        let answer = call(
            &router,
            "/update-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": on_demand_trigger(),
                "sourceFlowConfig": s3_source("orders-in"),
                "destinationFlowConfigList": [s3_destination("orders-out")],
                "tasks": [map_task("id", "id")]
            }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {flow_name:?}: expected 404 for a flow that does not exist, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error, "ResourceNotFoundException",
            "case {flow_name:?}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.string("message"),
            format!("There is no flow with the name {flow_name}"),
            "case {flow_name:?}: expected the message to name the flow, got {:?}",
            answer.get("message")
        );
    }
}

#[tokio::test]
async fn a_retried_update_carrying_the_same_token_completes_once() {
    let router = appflow();
    let flow_name = "idempotent-update-flow";
    create_base_flow(&router, "idempotent update", flow_name).await;

    let definition = |tasks: Value, token: &str| {
        json!({
            "flowName": flow_name,
            "clientToken": token,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("orders-landing"),
            "destinationFlowConfigList": [s3_destination("orders-curated")],
            "tasks": tasks
        })
    };
    let first_tasks = json!([map_task("order_id", "order_key")]);
    let other_tasks = json!([map_task("order_id", "something_else")]);

    let first = call(&router, "/update-flow", &definition(first_tasks.clone(), "update-token-1")).await;
    assert!(
        first.is_ok(),
        "expected the first UpdateFlow to succeed, got {} {:?}",
        first.status,
        first.body
    );
    let after_first = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    assert_eq!(
        after_first.get("schemaVersion").as_i64(),
        Some(2),
        "expected the data-schema change to raise schemaVersion once, got {:?}",
        after_first.get("schemaVersion")
    );

    // The same token with the same input: answered again, applied once — not even
    // lastUpdatedAt moves.
    let retried = call(&router, "/update-flow", &definition(first_tasks.clone(), "update-token-1")).await;
    assert!(
        retried.is_ok(),
        "expected the retry to be answered, got {} {:?}",
        retried.status,
        retried.body
    );
    assert_eq!(
        retried.string("flowStatus"),
        first.string("flowStatus"),
        "expected the retry to answer with the first result, got {:?}",
        retried.get("flowStatus")
    );
    let after_retry = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    for member in ["schemaVersion", "lastUpdatedAt", "tasks"] {
        assert_eq!(
            after_retry.get(member),
            after_first.get(member),
            "expected the retry to leave {member} where the first update left it, got {:?}",
            after_retry.get(member)
        );
    }

    // The same token with DIFFERENT input is an error, and changes nothing.
    let conflicting = call(&router, "/update-flow", &definition(other_tasks.clone(), "update-token-1")).await;
    assert_eq!(
        conflicting.status, 400,
        "expected a reused token with different input to be refused, got {} {:?}",
        conflicting.status,
        conflicting.body
    );
    assert_eq!(
        conflicting.error, "ValidationException",
        "expected ValidationException for a reused token, got {:?}",
        conflicting.error
    );
    let after_conflict = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    assert_eq!(
        after_conflict.get("tasks"),
        after_first.get("tasks"),
        "expected the refused update to store nothing, got {:?}",
        after_conflict.get("tasks")
    );

    // A DIFFERENT token is a new call, so the same input is applied.
    let fresh = call(&router, "/update-flow", &definition(other_tasks.clone(), "update-token-2")).await;
    assert!(
        fresh.is_ok(),
        "expected a new token to be a new call, got {} {:?}",
        fresh.status,
        fresh.body
    );
    let after_fresh = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    assert_eq!(
        after_fresh.get("schemaVersion").as_i64(),
        Some(3),
        "expected the new data schema to raise schemaVersion again, got {:?}",
        after_fresh.get("schemaVersion")
    );
    assert!(
        contains(after_fresh.get("tasks"), &other_tasks),
        "expected the new tasks to be stored, got {:?}",
        after_fresh.get("tasks")
    );
}

#[tokio::test]
async fn updating_to_a_scheduled_trigger_without_a_schedule_is_refused() {
    let router = appflow();
    let flow_name = "unscheduled-update-flow";
    let before = create_base_flow(&router, "scheduled without a schedule", flow_name).await;

    // The rows differ along the one thing that matters: a Scheduled trigger with no
    // schedule at all, and one whose triggerProperties carry no `Scheduled` entry.
    for (case, trigger) in [
        ("no triggerProperties", json!({ "triggerType": "Scheduled" })),
        (
            "empty triggerProperties",
            json!({ "triggerType": "Scheduled", "triggerProperties": {} }),
        ),
    ] {
        let answer = call(
            &router,
            "/update-flow",
            &json!({
                "flowName": flow_name,
                "triggerConfig": trigger,
                "sourceFlowConfig": s3_source("orders-landing"),
                "destinationFlowConfigList": [s3_destination("orders-curated")],
                "tasks": [map_task("order_id", "id")]
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a scheduled trigger with no schedule to be refused, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error, "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.error
        );
    }

    // Nothing was stored by either refusal.
    let after = call(&router, "/describe-flow", &json!({ "flowName": flow_name })).await;
    for member in ["triggerConfig", "flowStatus", "schemaVersion", "lastUpdatedAt"] {
        assert_eq!(
            after.get(member),
            before.get(member),
            "expected the refused update to leave {member} alone, got {:?}",
            after.get(member)
        );
    }
}

#[tokio::test]
async fn an_update_keeps_the_flow_s_run_history() {
    let router = appflow();
    let flow_name = "updated-after-a-run-flow";
    create_base_flow(&router, "run history", flow_name).await;

    let started = call(
        &router,
        "/start-flow",
        &json!({ "flowName": flow_name, "clientToken": "run-before-update" }),
    )
    .await;
    assert!(
        started.is_ok(),
        "expected StartFlow to succeed, got {} {:?}",
        started.status,
        started.body
    );
    let execution_id = started.string("executionId").to_string();

    let updated = call(
        &router,
        "/update-flow",
        &json!({
            "flowName": flow_name,
            "triggerConfig": on_demand_trigger(),
            "sourceFlowConfig": s3_source("orders-landing"),
            "destinationFlowConfigList": [s3_destination("orders-curated")],
            "tasks": [map_task("order_id", "order_key")]
        }),
    )
    .await;
    assert!(
        updated.is_ok(),
        "expected UpdateFlow to succeed after a run, got {} {:?}",
        updated.status,
        updated.body
    );

    let records = call(
        &router,
        "/describe-flow-execution-records",
        &json!({ "flowName": flow_name }),
    )
    .await;
    let runs = records
        .get("flowExecutions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        runs.len(),
        1,
        "expected the run recorded before the update to still be there, got {runs:?}"
    );
    assert_eq!(
        runs[0].get("executionId").and_then(Value::as_str),
        Some(execution_id.as_str()),
        "expected the same run StartFlow recorded, got {:?}",
        runs[0].get("executionId")
    );
}

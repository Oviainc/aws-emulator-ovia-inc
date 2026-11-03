// SPDX-License-Identifier: CC0-1.0

//! The STATUS LINE this service puts on a modelled error, over the wire.
//!
//! Amazon Personalize is an `awsJson1_1` service: the error shape travels in the body's
//! `__type` and every client error shares the protocol's one client status, 400. The
//! per-shape `@httpError` values the model records (403, 404, 409 beside 400) are not the
//! numbers the service emits, so a client that classifies by status class must still read
//! "bad request" for each of them.
//!
//! The suite is a table spanning what the rule generalises over: the error SHAPE (not
//! found, already exists, in use, invalid input), the OPERATION that raises it (describe,
//! delete, create), and the RESOURCE KIND it is raised about (schema, dataset group,
//! dataset). The recorded case — a `DescribeSchema` for a schema that is gone — is one row
//! among them.

use serde_json::{json, Value};

use crate::dataset_group_lifecycle::{
    avro_schema, create_active_group, create_dataset, create_schema, error_code, Emulator, ACCOUNT,
    REGION,
};

/// The one status an `awsJson1_1` service puts on a client error.
const CLIENT_ERROR_STATUS: u16 = 400;

/// Every distinct `@httpError` the model declares for this service's error shapes.
/// Source: `smithy_query(query="shape", shape="InvalidInputException,
/// ResourceAlreadyExistsException,ResourceNotFoundException,ResourceInUseException,
/// LimitExceededException,TooManyTagsException,TooManyTagKeysException")` — 400 on
/// `InvalidInputException`, `TooManyTagsException` and `TooManyTagKeysException`, 403 on
/// `ResourceAlreadyExistsException`, 404 on `ResourceNotFoundException`, 409 on
/// `ResourceInUseException` and `LimitExceededException`. Each is a client error, so each
/// goes out as [`CLIENT_ERROR_STATUS`].
const MODELLED_CLIENT_ERROR_STATUSES: &[u16] = &[400, 403, 404, 409];

/// Statuses that are NOT client errors, so the mapping leaves them where they are: a
/// success, a redirect, and the server faults an SDK retries on.
const UNFOLDED_STATUSES: &[u16] = &[200, 301, 500, 501, 503];

/// One row of the wire table: the request, and the error shape it must answer with.
struct Row {
    case: String,
    operation: &'static str,
    body: Value,
    code: &'static str,
}

/// Assert `operation` with `body` is refused with the client status and `code`.
async fn assert_client_error(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: Value,
    code: &str,
) {
    let (status, answered) = emulator.call(operation, body).await;
    assert_eq!(
        status, CLIENT_ERROR_STATUS,
        "case {case:?}: expected {operation} to answer HTTP {CLIENT_ERROR_STATUS}, got \
         {status} {answered:?}"
    );
    let answered_code = error_code(&answered);
    assert!(
        answered_code.contains(code),
        "case {case:?}: expected {operation} to answer {code}, got {answered_code:?} in \
         {answered:?}"
    );
}

#[tokio::test]
async fn every_modelled_client_error_is_served_with_the_json_client_status() {
    let emulator = Emulator::new();
    let setup = "client-error status setup";

    let group = create_active_group(&emulator, "status-dsg", setup).await;
    let schema =
        create_schema(&emulator, "status-schema", &avro_schema("Interactions"), setup).await;
    let dataset = create_dataset(&emulator, &group, &schema, "Interactions", setup).await;

    let absent_schema = format!("arn:aws:personalize:{REGION}:{ACCOUNT}:schema/absent-status");
    let absent_group =
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset-group/absent-status");
    let absent_dataset =
        format!("arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/absent-status/INTERACTIONS");

    // A group whose create has not completed yet: a dataset created against it is refused
    // as in-use. Asserted here, on the call right after the create, rather than as a table
    // row, so the assertion never races the create span.
    let (status, body) = emulator
        .call("CreateDatasetGroup", json!({"name": "status-pending-dsg"}))
        .await;
    assert_eq!(
        status, 200,
        "case {setup:?}: expected CreateDatasetGroup to answer 200, got {status} {body:?}"
    );
    let pending_group = body["datasetGroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {setup:?}: expected a datasetGroupArn, got {body:?}"))
        .to_string();
    assert_client_error(
        &emulator,
        "a dataset against a group whose create has not completed",
        "CreateDataset",
        json!({
            "name": "too-early", "datasetType": "Items",
            "schemaArn": schema, "datasetGroupArn": pending_group,
        }),
        "ResourceInUseException",
    )
    .await;

    let rows: Vec<Row> = vec![
        // Not found, over all three resource kinds and both readers and deletes. The
        // recorded unit is the first row.
        Row {
            case: "DescribeSchema for an ARN that holds no schema".to_string(),
            operation: "DescribeSchema",
            body: json!({"schemaArn": absent_schema}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "DeleteSchema for an ARN that holds no schema".to_string(),
            operation: "DeleteSchema",
            body: json!({"schemaArn": absent_schema}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "DescribeDatasetGroup for an ARN that holds no group".to_string(),
            operation: "DescribeDatasetGroup",
            body: json!({"datasetGroupArn": absent_group}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "DeleteDatasetGroup for an ARN that holds no group".to_string(),
            operation: "DeleteDatasetGroup",
            body: json!({"datasetGroupArn": absent_group}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "DescribeDataset for an ARN that holds no dataset".to_string(),
            operation: "DescribeDataset",
            body: json!({"datasetArn": absent_dataset}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "DeleteDataset for an ARN that holds no dataset".to_string(),
            operation: "DeleteDataset",
            body: json!({"datasetArn": absent_dataset}),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "CreateDataset naming a group that does not exist".to_string(),
            operation: "CreateDataset",
            body: json!({
                "name": "orphan", "datasetType": "Items",
                "schemaArn": schema, "datasetGroupArn": absent_group,
            }),
            code: "ResourceNotFoundException",
        },
        Row {
            case: "CreateDataset naming a schema that does not exist".to_string(),
            operation: "CreateDataset",
            body: json!({
                "name": "orphan", "datasetType": "Items",
                "schemaArn": absent_schema, "datasetGroupArn": group,
            }),
            code: "ResourceNotFoundException",
        },
        // Already exists, on each creating operation.
        Row {
            case: "CreateSchema under a name already taken".to_string(),
            operation: "CreateSchema",
            body: json!({"name": "status-schema", "schema": avro_schema("Interactions")}),
            code: "ResourceAlreadyExistsException",
        },
        Row {
            case: "CreateDatasetGroup under a name already taken".to_string(),
            operation: "CreateDatasetGroup",
            body: json!({"name": "status-dsg"}),
            code: "ResourceAlreadyExistsException",
        },
        Row {
            case: "a second dataset of a type the group already holds".to_string(),
            operation: "CreateDataset",
            body: json!({
                "name": "a-different-name", "datasetType": "Interactions",
                "schemaArn": schema, "datasetGroupArn": group,
            }),
            code: "ResourceAlreadyExistsException",
        },
        // In use, on each delete a live reference blocks.
        Row {
            case: "DeleteSchema while a dataset references it".to_string(),
            operation: "DeleteSchema",
            body: json!({"schemaArn": schema}),
            code: "ResourceInUseException",
        },
        Row {
            case: "DeleteDatasetGroup while it still holds a dataset".to_string(),
            operation: "DeleteDatasetGroup",
            body: json!({"datasetGroupArn": group}),
            code: "ResourceInUseException",
        },
        // Invalid input: modelled 400 already, so these rows show the mapping moves no
        // status that was right to begin with.
        Row {
            case: "CreateSchema with a document that is not an Avro record".to_string(),
            operation: "CreateSchema",
            body: json!({"name": "not-avro", "schema": "USER_ID string"}),
            code: "InvalidInputException",
        },
        Row {
            case: "CreateDataset with a datasetType outside the documented set".to_string(),
            operation: "CreateDataset",
            body: json!({
                "name": "bad-type", "datasetType": "Clicks",
                "schemaArn": schema, "datasetGroupArn": group,
            }),
            code: "InvalidInputException",
        },
    ];

    for row in rows {
        let Row {
            case,
            operation,
            body,
            code,
        } = row;
        assert_client_error(&emulator, &case, operation, body, code).await;
    }

    // The refusals above changed nothing: the resources they were raised about are all
    // still readable, so the status under test belongs to the refusal and not to the call.
    for (case, operation, member, arn) in [
        ("the schema survives", "DescribeSchema", "schemaArn", &schema),
        (
            "the group survives",
            "DescribeDatasetGroup",
            "datasetGroupArn",
            &group,
        ),
        (
            "the dataset survives",
            "DescribeDataset",
            "datasetArn",
            &dataset,
        ),
    ] {
        let (status, body) = emulator.call_with_arn(operation, member, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn the_status_mapping_folds_client_statuses_only() {
    for modelled in MODELLED_CLIENT_ERROR_STATUSES {
        let served =
            service_com_amazonaws_personalize_amazonpersonalize::scaffolded::wire_status(*modelled);
        assert_eq!(
            served, CLIENT_ERROR_STATUS,
            "case \"modelled {modelled}\": expected HTTP {CLIENT_ERROR_STATUS} on the wire, got \
             {served}"
        );
    }

    // The bounds of the client-error class, and one status either side of it.
    for modelled in [400_u16, 499] {
        let served =
            service_com_amazonaws_personalize_amazonpersonalize::scaffolded::wire_status(modelled);
        assert_eq!(
            served, CLIENT_ERROR_STATUS,
            "case \"modelled {modelled}\": expected HTTP {CLIENT_ERROR_STATUS} on the wire, got \
             {served}"
        );
    }

    for modelled in UNFOLDED_STATUSES {
        let served =
            service_com_amazonaws_personalize_amazonpersonalize::scaffolded::wire_status(*modelled);
        assert_eq!(
            served, *modelled,
            "case \"modelled {modelled}\": expected the status to stand, got {served}"
        );
    }
}

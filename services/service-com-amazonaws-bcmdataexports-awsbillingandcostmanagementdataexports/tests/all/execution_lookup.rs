// SPDX-License-Identifier: CC0-1.0

//! `GetExecution` over the awsJson1_1 wire.
//!
//! The rule under test: `GetExecution` reports the recorded execution its two ids name —
//! the `ExecutionId`, that execution's `ExecutionStatus`, and the snapshot of the export
//! definition the execution was generated from ("This export data is a snapshot from when
//! the execution was generated… The data could be different from the current export data
//! if the export was updated since" — smithy_query shape `GetExecutionResponse`, member
//! `Export`) — and otherwise answers the modelled `ResourceNotFoundException` (HTTP 404):
//! for an `ExportArn` that names no export, and for an `ExecutionId` that names no
//! execution OF THAT export. It never answers an error the operation does not model, and
//! reading an execution changes nothing a later caller sees.
//!
//! Nothing in this emulator CREATES an execution — an execution records a data-plane
//! delivery (a query run and a file written to the caller's bucket), which this emulator
//! does not perform — so the executions below are written into the store the handler reads
//! (`crate::state`), exactly as a delivery would record them. That is also what makes the
//! assertions about the reported values meaningful: each one is a value this test put in
//! the store, not a value the handler invented.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: several
//! executions of several exports, each with a different `ExecutionStatusCode` (walked from
//! the model's own published list), ids of several forms, and lookups that cross the
//! (export, execution) pair in both directions.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::{
    ExecutionStatus, EXECUTION_STATUS_CODE_VALUES,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    StoredExecution, EXPORT_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState as State;

use crate::export_lifecycle::{cases, error_code, Client};

/// The ids a bogus-execution lookup is tried with: the recorded shape (a UUID), and the
/// other forms `ExecutionId` admits (`@length 0..1024`, `@pattern ^[\S\s]*$` —
/// smithy_query operation `GetExecution`, member `ExecutionId`).
fn bogus_execution_ids() -> Vec<String> {
    vec![
        "00000000-0000-0000-0000-000000000000".to_string(),
        "not-an-execution".to_string(),
        String::new(),
        " ".to_string(),
        "x".repeat(1024),
    ]
}

/// An engine and a client over it, so this suite can write the executions the handler
/// reads.
///
/// `pub(crate)` so the `execution_listing` suite seeds the SAME store through the SAME
/// harness instead of growing a second copy of it.
pub(crate) fn client_and_engine() -> (Client, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (Client::with_engine(Arc::clone(&engine)), engine)
}

/// Create the export named by case `index` and return its ARN.
async fn create_export(client: &Client, index: usize) -> String {
    let case = cases().remove(index);
    let created = client.ok("CreateExport", &case.create_request()).await;
    created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string()
}

/// Record one execution of `export_arn` in the store, with the status code
/// `EXECUTION_STATUS_CODE_VALUES[status]`, and return the `(id, status)` it recorded.
///
/// `pub(crate)` for the reason [`client_and_engine`] is: `ListExecutions`'s suite pages
/// over the executions this records.
pub(crate) fn record_execution(
    engine: &Arc<engine::InMemoryEngine>,
    export_arn: &str,
    execution_id: &str,
    status: usize,
) -> ExecutionStatus {
    let state = engine.state::<State>();
    let mut store = state.lock();
    let export = store
        .exports
        .get(export_arn)
        .expect("the export was created through CreateExport")
        .export
        .clone();
    let recorded = ExecutionStatus {
        status_code: Some(EXECUTION_STATUS_CODE_VALUES[status].to_string()),
        status_reason: None,
        created_at: Some(
            ::aws_smithy_types::DateTime::from_secs(1_767_225_600 + status as i64),
        ),
        completed_at: None,
        last_updated_at: Some(
            ::aws_smithy_types::DateTime::from_secs(1_767_225_600 + status as i64),
        ),
    };
    store.insert_execution(StoredExecution {
        execution_id: execution_id.to_string(),
        export_arn: export_arn.to_string(),
        status: recorded.clone(),
        export,
    });
    recorded
}

/// The `ExecutionStatus` a response reports, as JSON.
fn reported_status(body: &Value) -> &Value {
    &body["ExecutionStatus"]
}

#[tokio::test]
async fn an_execution_id_that_names_nothing_is_a_modelled_not_found() {
    let (client, _engine) = client_and_engine();

    // The recorded precondition: a valid export, so the refusal is about the execution.
    let arn = create_export(&client, 0).await;

    for id in bogus_execution_ids() {
        let label = format!("bogus execution id {id:?}");
        let (status, body) = client
            .call(
                "GetExecution",
                &json!({ "ExportArn": arn, "ExecutionId": id }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected HTTP 404 ResourceNotFoundException, got {status} with {body}"
        );
        assert_eq!(
            (
                body["Message"].as_str(),
                body["ResourceId"].as_str(),
                body["ResourceType"].as_str(),
            ),
            (
                Some(EXPORT_NOT_FOUND_MESSAGE),
                Some(id.as_str()),
                Some("Execution"),
            ),
            "case {label:?}: the 404 must report the execution it could not find, got {body}"
        );
    }

    // An ARN that names no export is a not-found about the EXPORT, reported as such.
    let absent = ::engine::emulator_arn("bcm-data-exports", "us-east-1", "export/never_created");
    let (status, body) = client
        .call(
            "GetExecution",
            &json!({ "ExportArn": absent, "ExecutionId": "any" }),
        )
        .await;
    assert_eq!(
        (
            status,
            error_code(&body).as_str(),
            body["ResourceId"].as_str(),
            body["ResourceType"].as_str(),
        ),
        (
            404,
            "ResourceNotFoundException",
            Some(absent.as_str()),
            Some("Export"),
        ),
        "an ARN naming no export must be reported as the absent export, got {body}"
    );
}

#[tokio::test]
async fn a_recorded_execution_is_reported_from_the_store() {
    let (client, engine) = client_and_engine();

    let first = create_export(&client, 0).await;
    let second = create_export(&client, 1).await;
    let first_definition = client
        .ok("GetExport", &json!({ "ExportArn": first }))
        .await["Export"]
        .clone();
    let second_definition = client
        .ok("GetExport", &json!({ "ExportArn": second }))
        .await["Export"]
        .clone();
    let definition_of = |export_arn: &str| {
        if export_arn == first {
            first_definition.clone()
        } else {
            second_definition.clone()
        }
    };

    // One execution per published status code, spread over the two exports, each with its
    // own id form.
    let mut recorded: Vec<(String, String, ExecutionStatus)> = Vec::new();
    for (index, _) in EXECUTION_STATUS_CODE_VALUES.iter().enumerate() {
        let export_arn = if index % 2 == 0 { &first } else { &second };
        let execution_id = format!("exec-{index:04}-0000-0000-0000-000000000000");
        let status = record_execution(&engine, export_arn, &execution_id, index);
        recorded.push((export_arn.clone(), execution_id, status));
    }

    for (export_arn, execution_id, status) in &recorded {
        let label = format!("{execution_id} of {export_arn}");
        let body = client
            .ok(
                "GetExecution",
                &json!({ "ExportArn": export_arn, "ExecutionId": execution_id }),
            )
            .await;
        assert_eq!(
            body["ExecutionId"].as_str(),
            Some(execution_id.as_str()),
            "case {label:?}: the response must report the execution that was asked for, got {body}"
        );
        assert_eq!(
            reported_status(&body)["StatusCode"].as_str(),
            status.status_code.as_deref(),
            "case {label:?}: the reported status must be the recorded one, got {body}"
        );
        assert_eq!(
            body["Export"], definition_of(export_arn),
            "case {label:?}: the response must report the snapshot of its OWN export, got {body}"
        );
        assert_eq!(
            body["Export"]["ExportArn"].as_str(),
            Some(export_arn.as_str()),
            "case {label:?}: the snapshot must be the export that generated it, got {body}"
        );

        // A read changes nothing: the same request answers the same thing.
        let again = client
            .ok(
                "GetExecution",
                &json!({ "ExportArn": export_arn, "ExecutionId": execution_id }),
            )
            .await;
        assert_eq!(
            again, body,
            "case {label:?}: reading an execution must not change what a later caller reads"
        );
    }

    // An execution of one export is not an execution of another: the pair is the key.
    for (export_arn, execution_id, _) in &recorded {
        let other = if export_arn == &first { &second } else { &first };
        let (status, body) = client
            .call(
                "GetExecution",
                &json!({ "ExportArn": other, "ExecutionId": execution_id }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {execution_id:?} under {other}: an execution belongs to ONE export, got {status} with {body}"
        );
    }

    // The snapshot is the definition as it stood: updating the export afterwards does not
    // rewrite what the execution reports.
    let (export_arn, execution_id, _) = recorded
        .iter()
        .find(|(arn, _, _)| arn == &first)
        .expect("the first export recorded an execution");
    let before = client
        .ok(
            "GetExecution",
            &json!({ "ExportArn": export_arn, "ExecutionId": execution_id }),
        )
        .await;
    let mut updated = first_definition.clone();
    let object = updated.as_object_mut().expect("Export is a JSON object");
    object.insert("Description".to_string(), json!("updated after the run"));
    object.insert(
        "DataQuery".to_string(),
        json!({
            "QueryStatement": "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT",
            "TableConfigurations": { "COST_AND_USAGE_REPORT": { "TIME_GRANULARITY": "DAILY" } },
        }),
    );
    client
        .ok(
            "UpdateExport",
            &json!({ "ExportArn": export_arn, "Export": updated }),
        )
        .await;
    let after = client
        .ok(
            "GetExecution",
            &json!({ "ExportArn": export_arn, "ExecutionId": execution_id }),
        )
        .await;
    assert_eq!(
        after, before,
        "an execution reports the snapshot it was generated from, not the current definition"
    );
}

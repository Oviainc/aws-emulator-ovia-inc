// SPDX-License-Identifier: CC0-1.0

//! `GetExecution` and `ListExecutions` over the awsJson1_1 wire.
//!
//! The rule under test: `GetExecution` reports the recorded execution its two ids name —
//! the `ExecutionId`, that execution's `ExecutionStatus`, and the snapshot of the export
//! definition the execution was generated from ("This export data is a snapshot from when
//! the execution was generated… The data could be different from the current export data
//! if the export was updated since" — smithy_query shape `GetExecutionResponse`, member
//! `Export`) — and otherwise answers the modelled `ResourceNotFoundException` (at the
//! status this service answers the not-found condition with,
//! `export_lifecycle::NOT_FOUND_STATUS`):
//! for an `ExportArn` that names no export, and for an `ExecutionId` that names no
//! execution OF THAT export. It never answers an error the operation does not model, and
//! reading an execution changes nothing a later caller sees. `ListExecutions` reports the
//! executions of the export its ARN names — all of them and no other export's, in one
//! stable order, paged — and answers 200 with an EMPTY list for an ARN that names no
//! export, which is recorded AWS behaviour for this one listing.
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

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

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
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {label:?}: expected the modelled ResourceNotFoundException, got {status} with {body}"
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
            "case {label:?}: the refusal must report the execution it could not find, got {body}"
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
            NOT_FOUND_STATUS,
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
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
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

/// RULE: `ListExecutions` reports the executions OF THE EXPORT its ARN names — every one
/// of them, no other export's, in one stable order — and an export with none reports an
/// empty list rather than a refusal.
///
/// Asserted over a table whose rows differ along the number of recorded executions (none,
/// one, several) and over two exports, so an execution of one is never listed for the
/// other.
#[tokio::test]
async fn list_executions_reports_the_executions_of_its_own_export() {
    let (client, engine) = client_and_engine();

    let first = create_export(&client, 0).await;
    let second = create_export(&client, 1).await;

    // An export that has recorded none lists none: nothing in this emulator creates an
    // execution, so this is the ordinary answer.
    for (label, arn) in [("first", &first), ("second", &second)] {
        let body = client
            .ok("ListExecutions", &json!({ "ExportArn": arn }))
            .await;
        assert_eq!(
            body["Executions"],
            json!([]),
            "case {label:?}: an export with no recorded execution must list none, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {label:?}: a last page must omit NextToken, got {body}"
        );
    }

    // Record several executions of the first export and one of the second.
    let mut expected_first: Vec<String> = Vec::new();
    for (index, _) in EXECUTION_STATUS_CODE_VALUES.iter().enumerate() {
        let execution_id = format!("list-{index:04}-0000-0000-0000-000000000000");
        record_execution(&engine, &first, &execution_id, index);
        expected_first.push(execution_id);
    }
    expected_first.sort();
    let only_second = "second-0000-0000-0000-000000000000".to_string();
    record_execution(&engine, &second, &only_second, 0);

    for (label, arn, expected) in [
        ("the first export", &first, expected_first.clone()),
        ("the second export", &second, vec![only_second.clone()]),
    ] {
        let body = client
            .ok("ListExecutions", &json!({ "ExportArn": arn }))
            .await;
        let listed: Vec<String> = body["Executions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: ListExecutions must report a list: {body}"))
            .iter()
            .map(|reference| {
                reference["ExecutionId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {label:?}: a reference carries its ExecutionId: {body}")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed, expected,
            "case {label:?}: expected exactly this export's executions {expected:?}, got {listed:?}"
        );
        // Each reference carries that execution's own recorded status.
        for reference in body["Executions"].as_array().unwrap_or(&Vec::new()) {
            let execution_id = reference["ExecutionId"].as_str().unwrap_or_default();
            let reported = client
                .ok(
                    "GetExecution",
                    &json!({ "ExportArn": arn, "ExecutionId": execution_id }),
                )
                .await;
            assert_eq!(
                reference["ExecutionStatus"],
                reported["ExecutionStatus"],
                "case {label:?}: the listed status of {execution_id} must be the one GetExecution reports, got {reference}"
            );
        }

        // A listing changes nothing a later caller sees.
        let again = client
            .ok("ListExecutions", &json!({ "ExportArn": arn }))
            .await;
        assert_eq!(
            again, body,
            "case {label:?}: ListExecutions is a reporting operation and must answer identically"
        );
    }
}

/// RULE: the listing pages — each page carries at most `MaxResults` references, the token
/// resumes immediately after it, every execution appears exactly once over the pages, and
/// the last page carries no token. A token this listing never issued is refused.
#[tokio::test]
async fn list_executions_pages_over_every_recorded_execution() {
    let (client, engine) = client_and_engine();
    let arn = create_export(&client, 0).await;

    let total = EXECUTION_STATUS_CODE_VALUES.len();
    let mut expected: Vec<String> = Vec::new();
    for index in 0..total {
        let execution_id = format!("page-{index:04}-0000-0000-0000-000000000000");
        record_execution(&engine, &arn, &execution_id, index);
        expected.push(execution_id);
    }
    expected.sort();

    for page_size in 1..=total + 1 {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "ExportArn": arn, "MaxResults": page_size });
            if let Some(token) = &token {
                request
                    .as_object_mut()
                    .expect("the request is a JSON object")
                    .insert("NextToken".to_string(), json!(token));
            }
            let body = client.ok("ListExecutions", &request).await;
            let references = body["Executions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a page must carry Executions: {body}"))
                .clone();
            assert!(
                references.len() <= page_size,
                "case {case:?}: a page must carry at most {page_size} references, got {body}"
            );
            for reference in &references {
                seen.push(
                    reference["ExecutionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= total + 2,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: every execution must appear exactly once, in order, got {seen:?}"
        );
    }

    // A token this listing never issued is refused rather than resumed somewhere.
    for bogus in ["not-a-token", "00", "6e6f7420612074616720617420616c6c"] {
        let (status, body) = client
            .call(
                "ListExecutions",
                &json!({ "ExportArn": arn, "NextToken": bogus }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {bogus:?}: a token this listing never issued must be refused, got {status} with {body}"
        );
    }
}

/// RULE: `ListExecutions` on an ARN that names no export answers 200 with an EMPTY list
/// — recorded AWS behaviour for this listing, and unlike every other ARN-addressed
/// operation of this service, which answers the not-found refusal. The answer is the
/// executions recorded for that ARN, and an unknown ARN has none.
#[tokio::test]
async fn list_executions_on_an_absent_export_lists_none() {
    let (client, engine) = client_and_engine();
    let live = create_export(&client, 0).await;
    // A recorded execution of a LIVE export, so an empty answer below is about the ARN
    // asked for and not about an empty store.
    record_execution(&engine, &live, "live-0000-0000-0000-000000000000", 0);
    let other = create_export(&client, 1).await;
    client
        .ok("DeleteExport", &json!({ "ExportArn": other }))
        .await;

    let absent = [
        (
            "never created",
            ::engine::emulator_arn("bcm-data-exports", "us-east-1", "export/never_created"),
        ),
        ("deleted", other.clone()),
        (
            "another region",
            ::engine::emulator_arn("bcm-data-exports", "eu-west-1", "export/never_created"),
        ),
        (
            "another account",
            "arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated".to_string(),
        ),
    ];
    for (label, arn) in absent {
        let body = client
            .ok("ListExecutions", &json!({ "ExportArn": arn }))
            .await;
        assert_eq!(
            body["Executions"],
            json!([]),
            "case {label:?}: an ARN that names no export lists no execution, got {body}"
        );
        assert!(
            body.get("NextToken").is_none(),
            "case {label:?}: a last page must omit NextToken, got {body}"
        );
    }

    // And the live export's own execution is still listed, so the empty answers above are
    // not an empty store.
    let body = client
        .ok("ListExecutions", &json!({ "ExportArn": live }))
        .await;
    assert_eq!(
        body["Executions"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        1,
        "the live export's recorded execution must still be listed, got {body}"
    );
}

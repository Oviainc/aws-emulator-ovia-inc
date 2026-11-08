// SPDX-License-Identifier: CC0-1.0

//! The deliveries the SERVICE owes an export, over the awsJson1_1 wire: `CreateExport` /
//! `UpdateExport` -> `ListExecutions` / `GetExecution` / `GetExport`.
//!
//! The rule under test: a data export is delivered BY THE SERVICE and not by the caller —
//! "The cadence for Amazon Web Services to update the data export in your S3 bucket"
//! (smithy_query shape `RefreshCadence`) — so the service owes every export a delivery for
//! each definition it persists (one when `CreateExport` stores it, one for each
//! `UpdateExport` that replaces it), and performs and records that delivery as an execution
//! ON THE CLOCK with no caller call: once the delivery's instant has passed,
//! `ListExecutions` reports an `ExecutionReference` for it and `GetExecution` reports it by
//! id, with an `ExecutionStatus` whose `StatusCode` is one of the modelled
//! `ExecutionStatusCode` values and with the snapshot of the definition that delivery runs.
//!
//! The destinations in this suite are buckets the object store does not hold, so the write
//! a delivery performs cannot be made: each delivery here therefore ends in the modelled
//! `DELIVERY_FAILURE` with the modelled reason for a destination this service is not
//! permitted to write (`INSUFFICIENT_PERMISSION`), and — because no export file was
//! generated — `ExportStatus.LastRefreshedAt` ("The timestamp of when the export was last
//! generated") stays absent. The delivery that DOES land a file in a bucket, with every
//! encoding and both `Overwrite` dispositions, is asserted in
//! `tests/all/export_file_delivery.rs`.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: every
//! export definition `export_lifecycle::cases` publishes (lean and fully populated
//! definitions, queries on three catalog tables, with and without table configurations and
//! tags), the number of definitions an export has had (one, two, and one superseded before
//! its delivery ran), both ends of the delivery's own lifecycle (initiated, then terminal),
//! and the executions of two exports side by side. One test walks the clock with no state
//! poking at all, so the accrual is the service's and not the test's.
//!
//! Nothing in this emulator sleeps for a due instant: the instant is STORED (on the export
//! while its delivery is pending, on the execution while its terminal status is pending)
//! and materialised by whichever request next reaches the store, so a test moves a stored
//! instant to put a transition in the past or hold it in the future — exactly the state the
//! service reads — instead of waiting for one.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::{
    EXECUTION_STATUS_CODE_VALUES, EXECUTION_STATUS_REASON_VALUES, FREQUENCY_OPTION_VALUES,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    DELIVERY_INITIATION_DELAY, DELIVERY_SETTLES_AFTER, EXECUTION_STATUS_DELIVERY_FAILURE,
    EXECUTION_STATUS_INITIATION_IN_PROCESS, EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION,
    EXPORT_STATUS_HEALTHY, MAX_EXECUTIONS_PER_EXPORT,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState as State;

use crate::execution_lookup::client_and_engine;
use crate::export_lifecycle::{cases, error_code, timestamp, Client, NOT_FOUND_STATUS};
use crate::table_listing::minimal_export_request;

/// How far into the future a HELD delivery is moved: long enough that no call of a suite
/// that is not about deliveries can reach it.
const HOLD_FOR: Duration = Duration::from_secs(3_600);

/// The margin a timing test waits past a published span, so a scheduler that runs the
/// transition a moment late is still observed.
const CLOCK_MARGIN: Duration = Duration::from_millis(250);

/// Re-date the delivery the service owes `export_arn`: it is initiated at `initiate_at` and
/// the execution it records reaches its terminal status at `complete_at`.
///
/// Both instants are state the service itself reads, so moving them is how a test puts a
/// clock-owed transition in the past (or holds it in the future) without sleeping — the
/// service applies it on the next request exactly as it would have applied it on time.
///
/// `pub(crate)` so the suite that asserts on the delivery's own file
/// (`tests/all/export_file_delivery.rs`) moves a due instant the same ONE way.
pub(crate) fn reschedule_delivery(
    engine: &Arc<engine::InMemoryEngine>,
    export_arn: &str,
    initiate_at: SystemTime,
    complete_at: SystemTime,
) {
    let state = engine.state::<State>();
    let mut store = state.lock();
    let export = store
        .exports
        .get_mut(export_arn)
        .unwrap_or_else(|| panic!("case {export_arn:?}: the export was created through CreateExport"));
    let delivery = export.pending_delivery.as_mut().unwrap_or_else(|| {
        panic!("case {export_arn:?}: the service must owe this export a delivery")
    });
    delivery.initiate_at = initiate_at;
    delivery.complete_at = complete_at;
}

/// Hold the delivery the service owes `export_arn` out of the way of a suite that asserts
/// on executions IT recorded.
///
/// `pub(crate)` because the two suites that seed executions directly
/// (`execution_lookup`, `execution_listing`) need the service's own delivery held while
/// they assert on theirs.
pub(crate) fn hold_delivery(engine: &Arc<engine::InMemoryEngine>, export_arn: &str) {
    let held = SystemTime::now() + HOLD_FOR;
    reschedule_delivery(engine, export_arn, held, held + DELIVERY_SETTLES_AFTER);
}

/// Make the INITIATION of the delivery the service owes `export_arn` due `ago` in the past
/// while its terminal status stays held, answering the instant it is now initiated at — so
/// the next request records a delivery that has started and not finished.
fn settle_initiation(
    engine: &Arc<engine::InMemoryEngine>,
    export_arn: &str,
    ago: Duration,
) -> SystemTime {
    let now = SystemTime::now();
    let initiate_at = now - ago;
    reschedule_delivery(engine, export_arn, initiate_at, now + HOLD_FOR);
    initiate_at
}

/// Make the terminal status of every execution of `export_arn` that has not reached one yet
/// due `ago` in the past, answering the instant they are now dated at.
fn settle_completions(
    engine: &Arc<engine::InMemoryEngine>,
    export_arn: &str,
    ago: Duration,
) -> SystemTime {
    let at = SystemTime::now() - ago;
    let state = engine.state::<State>();
    let mut store = state.lock();
    let mut pending = 0usize;
    for (key, execution) in store.executions.iter_mut() {
        if key.0.as_str() == export_arn && execution.completion_due_at.is_some() {
            execution.completion_due_at = Some(at);
            pending += 1;
        }
    }
    assert!(
        pending > 0,
        "case {export_arn:?}: an initiated delivery must still owe its terminal status"
    );
    at
}

/// `at` as the whole epoch second this service reports every instant at.
fn epoch_second(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs() as i64
}

/// The `Executions` of one `ListExecutions` answer. An empty page may elide the member, so
/// an absent one reads as the empty list it stands for.
async fn listed(client: &Client, export_arn: &str) -> Vec<Value> {
    let body = client
        .ok("ListExecutions", &json!({ "ExportArn": export_arn }))
        .await;
    match body.get("Executions") {
        Some(Value::Array(references)) => references.clone(),
        None => Vec::new(),
        other => panic!("case {export_arn:?}: Executions must be a list, got {other:?}"),
    }
}

/// The one execution `export_arn` has accrued, as `(id, ExecutionStatus)`.
async fn only_execution(case: &str, client: &Client, export_arn: &str) -> (String, Value) {
    let references = listed(client, export_arn).await;
    assert_eq!(
        references.len(),
        1,
        "case {case:?}: expected exactly the one delivery the service owed, got {references:?}"
    );
    let reference = references[0].clone();
    let id = reference["ExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a reference carries its ExecutionId: {reference}"))
        .to_string();
    (id, reference["ExecutionStatus"].clone())
}

/// Assert the shape every reported `ExecutionStatus` has to have, whatever status it is in:
/// a `StatusCode` the model publishes, a `StatusReason` that is either absent or one the
/// model publishes, and `CreatedAt`/`LastUpdatedAt` present.
fn assert_modelled_status(case: &str, status: &Value) {
    let code = status["StatusCode"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an execution reports a StatusCode: {status}"));
    assert!(
        EXECUTION_STATUS_CODE_VALUES.contains(&code),
        "case {case:?}: {code:?} must be one of the published {EXECUTION_STATUS_CODE_VALUES:?}"
    );
    if let Some(reason) = status.get("StatusReason") {
        let reason = reason.as_str().unwrap_or_else(|| {
            panic!("case {case:?}: StatusReason is an enum string, got {status}")
        });
        assert!(
            EXECUTION_STATUS_REASON_VALUES.contains(&reason),
            "case {case:?}: {reason:?} must be one of the published {EXECUTION_STATUS_REASON_VALUES:?}"
        );
    }
    for member in ["CreatedAt", "LastUpdatedAt"] {
        assert_eq!(
            timestamp(case, status, member).subsec_nanos(),
            0,
            "case {case:?}: expected a whole-second ExecutionStatus.{member}, got {status}"
        );
    }
}

/// The export's own status members a delivery that wrote nothing must not invent: no export
/// file was generated, so `LastRefreshedAt` is absent, and a destination this service could
/// not write to is not a verdict on the definition, so the export stays healthy.
async fn assert_export_reports_no_generated_file(case: &str, client: &Client, export_arn: &str) {
    let status = client
        .ok("GetExport", &json!({ "ExportArn": export_arn }))
        .await["ExportStatus"]
        .clone();
    assert!(
        status.get("LastRefreshedAt").is_none(),
        "case {case:?}: no export file was generated, so LastRefreshedAt must be absent, got {status}"
    );
    assert_eq!(
        status["StatusCode"].as_str(),
        Some(EXPORT_STATUS_HEALTHY),
        "case {case:?}: a delivery the destination would not take is not a verdict on the definition, got {status}"
    );
}

/// RULE: the service owes every export it stores a delivery, and records it as an execution
/// when its instant falls due — reported by `ListExecutions` and by `GetExecution`, in the
/// non-terminal status a started delivery carries and then in the terminal status this
/// emulator's missing data plane owes it, each dated from the instant the transition fell
/// due rather than from the instant some request materialised it.
#[tokio::test]
async fn an_export_accrues_the_delivery_the_service_owes_it() {
    for (index, mut case) in cases().into_iter().enumerate() {
        let (client, engine) = client_and_engine();
        case.set_name(&format!("delivery_{index:02}"));
        let label = format!("delivery of {index:02}");

        let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateExport returns an ExportArn"))
            .to_string();
        let definition = client.ok("GetExport", &json!({ "ExportArn": &arn })).await["Export"]
            .clone();

        // BEFORE the delivery falls due there is nothing to report: the span is never
        // zero, so a caller that lists executions straight after the create sees the
        // export with no delivery yet.
        hold_delivery(&engine, &arn);
        assert_eq!(
            listed(&client, &arn).await,
            Vec::<Value>::new(),
            "case {label:?}: a delivery that is not due yet must not be reported"
        );
        assert_export_reports_no_generated_file(&label, &client, &arn).await;

        // ONCE IT FALLS DUE the service has recorded it, with no caller call of its own.
        let due_at = settle_initiation(&engine, &arn, Duration::from_secs(2));
        let (execution_id, status) = only_execution(&label, &client, &arn).await;
        assert!(
            !execution_id.is_empty() && execution_id.chars().count() <= 1024,
            "case {label:?}: the ExecutionId must be a GenericString (len 0..1024), got {execution_id:?}"
        );
        assert_modelled_status(&label, &status);
        assert_eq!(
            status["StatusCode"].as_str(),
            Some(EXECUTION_STATUS_INITIATION_IN_PROCESS),
            "case {label:?}: a delivery that has started and not finished is in its non-terminal status, got {status}"
        );
        assert_eq!(
            status.get("StatusReason"),
            None,
            "case {label:?}: a delivery that has not finished has no reason to report, got {status}"
        );
        assert_eq!(
            status.get("CompletedAt"),
            None,
            "case {label:?}: a delivery that has not finished reports no CompletedAt, got {status}"
        );
        // The execution is dated from the instant the delivery fell DUE, not from the
        // instant the request that materialised it ran.
        assert_eq!(
            timestamp(&label, &status, "CreatedAt").secs(),
            epoch_second(due_at),
            "case {label:?}: expected CreatedAt at the instant the delivery fell due, got {status}"
        );
        assert_eq!(
            timestamp(&label, &status, "LastUpdatedAt").secs(),
            epoch_second(due_at),
            "case {label:?}: expected LastUpdatedAt at the instant the delivery fell due, got {status}"
        );

        // `GetExecution` answers out of the same store, and reports the snapshot of the
        // definition this delivery runs.
        let described = client
            .ok(
                "GetExecution",
                &json!({ "ExportArn": &arn, "ExecutionId": &execution_id }),
            )
            .await;
        assert_eq!(
            (
                described["ExecutionId"].as_str(),
                &described["ExecutionStatus"],
            ),
            (Some(execution_id.as_str()), &status),
            "case {label:?}: GetExecution must report the execution the listing did, got {described}"
        );
        assert_eq!(
            described["Export"], definition,
            "case {label:?}: the execution reports the definition it was generated from, got {described}"
        );

        // THE TERMINAL STATUS the clock owes it: the destination of this definition is a
        // bucket the object store does not hold, so the write the delivery performs cannot
        // be made and the delivery fails with the modelled reason for a destination this
        // service may not write.
        let completed_at = settle_completions(&engine, &arn, Duration::from_secs(1));
        let (terminal_id, terminal) = only_execution(&label, &client, &arn).await;
        assert_eq!(
            terminal_id, execution_id,
            "case {label:?}: a delivery is ONE execution, not a second one"
        );
        assert_modelled_status(&label, &terminal);
        assert_eq!(
            (
                terminal["StatusCode"].as_str(),
                terminal["StatusReason"].as_str(),
            ),
            (
                Some(EXECUTION_STATUS_DELIVERY_FAILURE),
                Some(EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION),
            ),
            "case {label:?}: a delivery the destination would not take ends in the modelled failure state, got {terminal}"
        );
        assert_eq!(
            timestamp(&label, &terminal, "CompletedAt").secs(),
            epoch_second(completed_at),
            "case {label:?}: expected CompletedAt at the instant the delivery ended, got {terminal}"
        );
        assert_eq!(
            timestamp(&label, &terminal, "LastUpdatedAt").secs(),
            epoch_second(completed_at),
            "case {label:?}: an execution that moved must report when it last moved, got {terminal}"
        );
        assert_eq!(
            timestamp(&label, &terminal, "CreatedAt").secs(),
            epoch_second(due_at),
            "case {label:?}: a terminal status must not move CreatedAt, got {terminal}"
        );
        assert!(
            timestamp(&label, &terminal, "CompletedAt").secs()
                >= timestamp(&label, &terminal, "CreatedAt").secs(),
            "case {label:?}: expected CompletedAt at or after CreatedAt, got {terminal}"
        );

        // Reading is still a read: the next caller sees the same answers, and the export
        // reports no file as generated.
        assert_eq!(
            only_execution(&label, &client, &arn).await,
            (execution_id.clone(), terminal.clone()),
            "case {label:?}: listing executions must answer identically on every call"
        );
        assert_eq!(
            client
                .ok(
                    "GetExecution",
                    &json!({ "ExportArn": &arn, "ExecutionId": &execution_id }),
                )
                .await["ExecutionStatus"],
            terminal,
            "case {label:?}: reading an execution must not change what a later caller reads"
        );
        assert_export_reports_no_generated_file(&label, &client, &arn).await;
    }
}

/// RULE over the published cadences: whatever `RefreshCadence.Frequency` an export
/// declares, the service owes it a delivery and records it when it falls due.
///
/// The rows walk every value the model publishes for that member, read from the service's
/// own generated catalog (`ops::types::FREQUENCY_OPTION_VALUES`, from smithy_query shape
/// `FrequencyOption`), so a cadence AWS adds later is covered by the same rows.
#[tokio::test]
async fn every_published_cadence_accrues_a_delivery() {
    let (client, engine) = client_and_engine();

    for (index, frequency) in FREQUENCY_OPTION_VALUES.iter().enumerate() {
        let mut request = minimal_export_request(&format!("cadence_{index:02}"));
        request["Export"]["RefreshCadence"] = json!({ "Frequency": frequency });
        let arn = client.ok("CreateExport", &request).await["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {frequency:?}: CreateExport returns an ExportArn"))
            .to_string();

        hold_delivery(&engine, &arn);
        assert_eq!(
            listed(&client, &arn).await,
            Vec::<Value>::new(),
            "case {frequency:?}: a delivery that is not due yet must not be reported"
        );

        settle_initiation(&engine, &arn, Duration::from_secs(1));
        let (execution_id, status) = only_execution(frequency, &client, &arn).await;
        assert_modelled_status(frequency, &status);
        assert_eq!(
            status["StatusCode"].as_str(),
            Some(EXECUTION_STATUS_INITIATION_IN_PROCESS),
            "case {frequency:?}: the delivery the clock owed must have started, got {status}"
        );
        assert!(
            !execution_id.is_empty(),
            "case {frequency:?}: the delivery must be addressable by an id"
        );
    }
}

/// RULE: each definition the service persists is owed its OWN delivery, and the execution
/// reports the definition that delivery runs — "This export data is a snapshot from when
/// the execution was generated… The data could be different from the current export data if
/// the export was updated since" (smithy_query shape `GetExecutionResponse`, member
/// `Export`). A definition replaced before its delivery ran is superseded by the one
/// replacing it, because a delivery initiated after the update delivers the updated
/// definition.
#[tokio::test]
async fn each_persisted_definition_is_owed_a_delivery_of_itself() {
    // Rows: an update AFTER the first delivery ran (two deliveries, two snapshots), and an
    // update BEFORE it ran (one delivery, of the newer definition).
    for (label, run_first) in [("update after the first delivery", true), ("update before it", false)]
    {
        let (client, engine) = client_and_engine();
        let mut case = cases().remove(0);
        case.set_name("snapshot_export");
        let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
            .as_str()
            .expect("CreateExport returns an ExportArn")
            .to_string();
        let first_definition = client.ok("GetExport", &json!({ "ExportArn": &arn })).await
            ["Export"]
            .clone();
        hold_delivery(&engine, &arn);

        let mut expected: Vec<Value> = Vec::new();
        if run_first {
            // The initiation falls due, a request records it, and then its terminal status
            // falls due: a transition is materialised by the request AFTER it, so each one
            // needs a call of its own.
            settle_initiation(&engine, &arn, Duration::from_secs(4));
            let _ = listed(&client, &arn).await;
            settle_completions(&engine, &arn, Duration::from_secs(3));
            let _ = listed(&client, &arn).await;
            expected.push(first_definition.clone());
        }

        // The update overwrites every parameter, so the stored definition — and the
        // snapshot of the delivery it owes — is the new one.
        let mut updated = first_definition.clone();
        let object = updated.as_object_mut().expect("Export is a JSON object");
        object.insert("Description".to_string(), json!("redefined for the next delivery"));
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
                &json!({ "ExportArn": &arn, "Export": updated }),
            )
            .await;
        let second_definition = client.ok("GetExport", &json!({ "ExportArn": &arn })).await
            ["Export"]
            .clone();
        assert_ne!(
            second_definition, first_definition,
            "case {label:?}: the update must have changed the stored definition"
        );
        expected.push(second_definition.clone());

        settle_initiation(&engine, &arn, Duration::from_secs(2));
        let _ = listed(&client, &arn).await;
        settle_completions(&engine, &arn, Duration::from_secs(1));
        let references = listed(&client, &arn).await;
        assert_eq!(
            references.len(),
            expected.len(),
            "case {label:?}: expected {} delivery(ies), got {references:?}",
            expected.len()
        );

        // Every delivery's snapshot is the definition it was owed for, and each one is
        // terminal with the modelled reason.
        let mut snapshots: Vec<Value> = Vec::new();
        for reference in &references {
            let id = reference["ExecutionId"].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: a reference carries its ExecutionId: {reference}")
            });
            let described = client
                .ok(
                    "GetExecution",
                    &json!({ "ExportArn": &arn, "ExecutionId": id }),
                )
                .await;
            assert_modelled_status(label, &described["ExecutionStatus"]);
            assert_eq!(
                (
                    described["ExecutionStatus"]["StatusCode"].as_str(),
                    described["ExecutionStatus"]["StatusReason"].as_str(),
                ),
                (
                    Some(EXECUTION_STATUS_DELIVERY_FAILURE),
                    Some(EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION),
                ),
                "case {label:?}: every delivery ends in the modelled failure state, got {described}"
            );
            snapshots.push(described["Export"].clone());
        }
        for definition in &expected {
            assert!(
                snapshots.contains(definition),
                "case {label:?}: a delivery of this definition must report it as its snapshot, got {snapshots:?}"
            );
        }
        if !run_first {
            assert!(
                !snapshots.contains(&first_definition),
                "case {label:?}: a definition replaced before its delivery ran is not delivered, got {snapshots:?}"
            );
        }
    }
}

/// RULE, with the CLOCK and nothing else: an export created through the wire accrues its
/// delivery on its own — no state is written by this test, and no operation is called to
/// make the delivery happen.
#[tokio::test]
async fn the_clock_alone_records_the_delivery() {
    let client = Client::new();
    let label = "clock alone";
    let arn = client
        .ok("CreateExport", &minimal_export_request("clock_export"))
        .await["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // A blocking sleep rather than `tokio::time::sleep`: the workspace's tokio carries
    // only the `macros`/`rt` features, so there is no time driver to await on.
    std::thread::sleep(DELIVERY_INITIATION_DELAY + CLOCK_MARGIN);
    let (execution_id, initiated) = only_execution(label, &client, &arn).await;
    assert_modelled_status(label, &initiated);

    std::thread::sleep(DELIVERY_SETTLES_AFTER + CLOCK_MARGIN);
    let (terminal_id, terminal) = only_execution(label, &client, &arn).await;
    assert_eq!(
        terminal_id, execution_id,
        "case {label:?}: the delivery that was initiated is the one that ends, not a second one"
    );
    assert_eq!(
        (
            terminal["StatusCode"].as_str(),
            terminal["StatusReason"].as_str(),
        ),
        (
            Some(EXECUTION_STATUS_DELIVERY_FAILURE),
            Some(EXECUTION_STATUS_REASON_INSUFFICIENT_PERMISSION),
        ),
        "case {label:?}: a delivery the destination would not take ends in the modelled failure state, got {terminal}"
    );
    assert!(
        terminal.get("CompletedAt").is_some(),
        "case {label:?}: a delivery that ended reports when it ended, got {terminal}"
    );
    assert_export_reports_no_generated_file(label, &client, &arn).await;
}

/// RULE: a delivery belongs to the export that owed it — it is listed under that export
/// only, addressed by the `(ExportArn, ExecutionId)` pair only, and removed with it.
#[tokio::test]
async fn a_delivery_belongs_to_the_export_that_owed_it() {
    let (client, engine) = client_and_engine();
    let mut arns: Vec<String> = Vec::new();
    for (index, mut case) in cases().into_iter().take(2).enumerate() {
        case.set_name(&format!("owner_{index:02}"));
        let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
            .as_str()
            .expect("CreateExport returns an ExportArn")
            .to_string();
        hold_delivery(&engine, &arn);
        arns.push(arn);
    }
    let mut ids: Vec<(String, String)> = Vec::new();
    for arn in &arns {
        settle_initiation(&engine, arn, Duration::from_secs(2));
        let (id, _) = only_execution(arn, &client, arn).await;
        ids.push((arn.clone(), id));
    }

    for (arn, id) in &ids {
        for other in arns.iter().filter(|other| *other != arn) {
            let (status, body) = client
                .call(
                    "GetExecution",
                    &json!({ "ExportArn": other, "ExecutionId": id }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {id:?} under {other}: a delivery belongs to ONE export, got {status} with {body}"
            );
        }
    }

    // Deleting an export takes its deliveries with it, and leaves the other export's own.
    let (doomed, doomed_id) = ids[0].clone();
    client
        .ok("DeleteExport", &json!({ "ExportArn": &doomed }))
        .await;
    assert_eq!(
        listed(&client, &doomed).await,
        Vec::<Value>::new(),
        "case {doomed:?}: a deleted export's deliveries are gone with it"
    );
    let (status, body) = client
        .call(
            "GetExecution",
            &json!({ "ExportArn": &doomed, "ExecutionId": &doomed_id }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {doomed:?}: an execution of a deleted export is addressable by nobody, got {body}"
    );
    let (survivor, survivor_id) = ids[1].clone();
    let (still_there, _) = only_execution(&survivor, &client, &survivor).await;
    assert_eq!(
        still_there, survivor_id,
        "case {survivor:?}: deleting one export must not touch another's deliveries"
    );
}

/// An EMULATOR resource guard rather than an AWS rule: an export keeps at most
/// `MAX_EXECUTIONS_PER_EXPORT` executions, dropping the oldest first, so a process that
/// takes many deliveries does not grow without bound.
///
/// Each delivery here is dated a second apart, so which executions are the oldest is
/// decided by the instants the deliveries fell due and not by the order a map happens to
/// hold them in.
#[tokio::test]
async fn an_export_keeps_a_bounded_number_of_deliveries() {
    let (client, engine) = client_and_engine();
    let mut case = cases().remove(0);
    case.set_name("bounded_export");
    let arn = client.ok("CreateExport", &case.create_request()).await["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // Two deliveries more than the bound: the create owes the first, each update owes
    // another.
    let total = MAX_EXECUTIONS_PER_EXPORT + 2;
    let base = SystemTime::now();
    let mut due: Vec<i64> = Vec::new();
    for index in 0..total {
        let at = base - Duration::from_secs((total - index) as u64);
        reschedule_delivery(&engine, &arn, at, at + DELIVERY_SETTLES_AFTER);
        due.push(epoch_second(at));
        if index + 1 < total {
            // The update materialises the delivery that just fell due and owes another.
            client
                .ok(
                    "UpdateExport",
                    &json!({ "ExportArn": &arn, "Export": case.update_export() }),
                )
                .await;
        }
    }

    let references = listed(&client, &arn).await;
    assert_eq!(
        references.len(),
        MAX_EXECUTIONS_PER_EXPORT,
        "case \"bounded\": an export keeps at most {MAX_EXECUTIONS_PER_EXPORT} executions, got {}",
        references.len()
    );
    let mut reported: Vec<i64> = Vec::new();
    for reference in &references {
        let status = &reference["ExecutionStatus"];
        assert_modelled_status("bounded", status);
        reported.push(timestamp("bounded", status, "CreatedAt").secs());
    }
    reported.sort_unstable();
    let mut kept = due[due.len() - MAX_EXECUTIONS_PER_EXPORT..].to_vec();
    kept.sort_unstable();
    assert_eq!(
        reported, kept,
        "case \"bounded\": the executions kept must be the newest {MAX_EXECUTIONS_PER_EXPORT}"
    );
}

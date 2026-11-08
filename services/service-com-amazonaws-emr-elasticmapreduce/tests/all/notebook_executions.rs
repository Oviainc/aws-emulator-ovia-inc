// SPDX-License-Identifier: CC0-1.0

//! The EMR notebook-execution family, end to end over the wire.
//!
//! THE RULE: `StartNotebookExecution` records a notebook execution against the cluster
//! its `ExecutionEngine` names — "the unique identifier of the execution engine. For an
//! Amazon EMR cluster, this is the cluster ID" (`ExecutionEngineConfig$Id` doc) — under
//! an id EMR mints, and `ListNotebookExecutions` reports the recorded executions that
//! match EVERY filter the request names ("you can filter the list based on multiple
//! criteria such as status, time range, and editor id"), newest first, "a maximum of 50
//! notebook executions and a marker to track the paging of a longer notebook execution
//! list" (`ListNotebookExecutions` doc). An engine id naming no cluster is refused with
//! the operation's modelled `InvalidRequestException` and records nothing, and so is a
//! marker that names no execution of the filtered list.
//!
//! Nothing here reaches into the service's state struct: an execution is only ever read
//! back through `DescribeNotebookExecution` and `ListNotebookExecutions`, which is what
//! makes these tests evidence that the responses are computed from persisted state.
//!
//! The filter rows span what the rule generalises over: WHICH filter is named (editor,
//! execution engine, status, each end of the time range), how many executions it matches
//! (all, several, one, none), filters combined, and — through the status catalog — every
//! value the status enum declares. The paging rows span the count: a page exactly at the
//! documented maximum, the remainder behind its marker, and a marker that names nothing.
//!
//! The status a recorded execution carries is the other half: an execution is
//! `START_PENDING` when the request has just been received, and the clock then owes it
//! each status of the modelled progression in turn — or the stop progression when a caller
//! asks it to stop. This emulator runs no notebook, so the progression ends in the
//! modelled failure statuses carrying the reason that says exactly that, rather than in
//! `FINISHED`, which would be a success for work nothing performed.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::NOTEBOOK_PHASE_SPAN;

use crate::job_flow_lifecycle::{
    assert_minted_id, assert_refused, at, epoch_secs, kept_alive_request, Emr,
};
use crate::refusal_messages::message;

/// Longer than the whole status progression a notebook execution walks (one
/// [`NOTEBOOK_PHASE_SPAN`] per phase), so an execution that was `START_PENDING` or
/// `STOP_PENDING` has reached the status it ends in by the time we look.
const PAST_TRANSITION: Duration = NOTEBOOK_PHASE_SPAN
    .saturating_mul(5)
    .saturating_add(Duration::from_millis(400));

/// `ListNotebookExecutions` page size, documented on the operation: "Returns a maximum
/// of 50 notebook executions and a marker to track the paging of a longer notebook
/// execution list across multiple `ListNotebookExecutions` calls."
const PAGE_SIZE: usize = 50;

/// The error code `ListNotebookExecutions` and `StartNotebookExecution` both model for
/// bad user input (`smithy_query(query="operation",
/// operation="ListNotebookExecutions,StartNotebookExecution")`).
const MODELED_CLIENT_ERROR: &str = "InvalidRequestException";

/// The error code the request-validation envelope carries, for a request Amazon EMR
/// judges on its own input before any resource is resolved.
///
/// `pub(crate)` so a sibling suite asserts the same code through one constant.
pub(crate) const REQUEST_VALIDATION: &str = "ValidationException";

/// The sentence `StartNotebookExecution` answers an `ExecutionEngine` that resolves no
/// usable Amazon EMR cluster with: it names the cluster RESOLUTION produced and states
/// that it is not available, so `resolved_cluster_id` fills the slot and the
/// absent-value rendering stands there when resolution produced none — which is the case
/// on every refusal of this class.
///
/// `pub(crate)` so a sibling suite builds the same sentence through one helper rather than
/// its own copy.
pub(crate) fn engine_sentence(resolved_cluster_id: Option<&str>) -> String {
    format!(
        "The EMR cluster Id {} is not available",
        resolved_cluster_id.unwrap_or("null")
    )
}

/// Every value `com.amazonaws.emr#NotebookExecutionStatus` declares
/// (`smithy_query(query="shape", shape="NotebookExecutionStatus")`).
const NOTEBOOK_EXECUTION_STATUSES: [&str; 10] = [
    "FAILED",
    "FAILING",
    "FINISHED",
    "FINISHING",
    "RUNNING",
    "STARTING",
    "START_PENDING",
    "STOPPED",
    "STOPPING",
    "STOP_PENDING",
];

/// The status a just-recorded execution carries: "`START_PENDING` indicates that the
/// cluster has received the execution request but execution has not begun."
const START_PENDING: &str = "START_PENDING";

/// The status the clock owes an execution whose notebook this service never ran: "`FAILED`
/// indicates that the execution failed", reached with the reason that says the failure is
/// this emulator's own.
const NOT_EXECUTED: &str = "FAILED";

/// The smallest `StartNotebookExecution` request that satisfies the operation's required
/// members, running `name` on the cluster `engine_id` for `editor_id`.
fn start_request(name: &str, engine_id: &str, editor_id: Option<&str>) -> Value {
    let mut request = json!({
        "NotebookExecutionName": name,
        "RelativePath": "notebooks/demo.ipynb",
        "ExecutionEngine": { "Id": engine_id, "Type": "EMR" },
        "ServiceRole": "EMR_Notebooks_DefaultRole",
    });
    if let Some(editor) = editor_id {
        request["EditorId"] = json!(editor);
    }
    request
}

/// `StartNotebookExecution`, answering the minted execution id.
async fn start(emr: &Emr, request: Value) -> String {
    let out = emr.ok("StartNotebookExecution", request).await;
    at(&out, &["NotebookExecutionId"])
        .as_str()
        .unwrap_or_else(|| {
            panic!("StartNotebookExecution must answer a NotebookExecutionId, got {out}")
        })
        .to_string()
}

/// `DescribeNotebookExecution`, answering the `NotebookExecution` it reports.
async fn describe(emr: &Emr, execution_id: &str) -> Value {
    emr.ok(
        "DescribeNotebookExecution",
        json!({ "NotebookExecutionId": execution_id }),
    )
    .await["NotebookExecution"]
        .clone()
}

/// The execution NAMES `ListNotebookExecutions` reports for `filter`, in the order it
/// reports them, with the marker it hands back for the rest.
async fn listed(emr: &Emr, filter: Value) -> (Vec<String>, Option<String>) {
    let page = emr.ok("ListNotebookExecutions", filter).await;
    let names = at(&page, &["NotebookExecutions"])
        .as_array()
        .map(|executions| {
            executions
                .iter()
                .filter_map(|execution| {
                    at(execution, &["NotebookExecutionName"])
                        .as_str()
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let marker = at(&page, &["Marker"]).as_str().map(str::to_string);
    (names, marker)
}

// ---------------------------------------------------------------------------
// StartNotebookExecution -> DescribeNotebookExecution / ListNotebookExecutions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_started_notebook_execution_round_trips_and_then_ends_without_running() {
    let case = "round trip";
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("notebook-host")).await;
    let started_at = epoch_secs(SystemTime::now());

    let id = start(
        &emr,
        json!({
            "EditorId": "e-ROUNDTRIP",
            "RelativePath": "notebooks/demo.ipynb",
            "NotebookExecutionName": "round-trip",
            "NotebookParams": "{\"threshold\":3}",
            "ExecutionEngine": { "Id": cluster, "Type": "EMR" },
            "ServiceRole": "EMR_Notebooks_DefaultRole",
            "NotebookInstanceSecurityGroupId": "sg-0123456789abcdef0",
            "NotebookS3Location": { "Bucket": "notebooks", "Key": "demo.ipynb" },
            "OutputNotebookS3Location": { "Bucket": "notebook-output", "Key": "demo.html" },
            "OutputNotebookFormat": "HTML",
            "Tags": [{ "Key": "team", "Value": "data" }],
            "EnvironmentVariables": { "LOG_LEVEL": "INFO" },
        }),
    )
    .await;
    assert_minted_id(case, "ex-", &id);

    // Every member below is read back out of the store, not out of the request that is
    // still in this test's hands.
    let execution = describe(&emr, &id).await;
    assert_eq!(
        at(&execution, &["NotebookExecutionId"]).as_str(),
        Some(id.as_str()),
        "case {case:?}: the execution reports the id it was minted under, got {execution}"
    );
    assert_eq!(
        at(&execution, &["EditorId"]).as_str(),
        Some("e-ROUNDTRIP"),
        "case {case:?}: expected the recorded EditorId, got {execution}"
    );
    assert_eq!(
        at(&execution, &["ExecutionEngine", "Id"]).as_str(),
        Some(cluster.as_str()),
        "case {case:?}: expected the execution engine to be the cluster, got {execution}"
    );
    assert_eq!(
        at(&execution, &["NotebookParams"]).as_str(),
        Some("{\"threshold\":3}"),
        "case {case:?}: expected the recorded NotebookParams, got {execution}"
    );
    assert_eq!(
        at(&execution, &["NotebookS3Location", "Key"]).as_str(),
        Some("demo.ipynb"),
        "case {case:?}: expected the recorded notebook location, got {execution}"
    );
    assert_eq!(
        at(&execution, &["OutputNotebookS3Location", "Bucket"]).as_str(),
        Some("notebook-output"),
        "case {case:?}: expected the recorded output location, got {execution}"
    );
    assert_eq!(
        at(&execution, &["OutputNotebookFormat"]).as_str(),
        Some("HTML"),
        "case {case:?}: expected the recorded output format, got {execution}"
    );
    assert_eq!(
        at(&execution, &["EnvironmentVariables", "LOG_LEVEL"]).as_str(),
        Some("INFO"),
        "case {case:?}: expected the recorded environment variables, got {execution}"
    );
    assert_eq!(
        at(&execution, &["Tags"]).as_array().map(Vec::len),
        Some(1),
        "case {case:?}: expected the recorded tag, got {execution}"
    );
    let arn = at(&execution, &["Arn"]).as_str().unwrap_or_default();
    assert!(
        arn.starts_with("arn:aws:elasticmapreduce:")
            && arn.ends_with(&format!("notebook-execution/{id}")),
        "case {case:?}: expected the execution's own ARN, got {arn:?}"
    );
    assert_eq!(
        at(&execution, &["Status"]).as_str(),
        Some(START_PENDING),
        "case {case:?}: a just-recorded execution has not begun, got {execution}"
    );
    let start_time = at(&execution, &["StartTime"]).as_f64().unwrap_or_default();
    assert!(
        start_time >= started_at - 1.0,
        "case {case:?}: expected a StartTime at the call, got {start_time} against \
         {started_at}"
    );
    assert!(
        at(&execution, &["EndTime"]).is_null(),
        "case {case:?}: an execution that has not ended reports no EndTime, got {execution}"
    );
    // No cell has run yet, so there is no output notebook in Amazon S3 to point at.
    assert!(
        at(&execution, &["OutputNotebookURI"]).is_null(),
        "case {case:?}: expected no OutputNotebookURI, got {execution}"
    );

    // The listing projects the SAME record.
    let page = emr.ok("ListNotebookExecutions", json!({})).await;
    let summary = at(&page, &["NotebookExecutions"])
        .as_array()
        .and_then(|executions| executions.first().cloned())
        .unwrap_or(Value::Null);
    assert_eq!(
        at(&summary, &["NotebookExecutionId"]).as_str(),
        Some(id.as_str()),
        "case {case:?}: expected the started execution in the listing, got {page}"
    );
    assert_eq!(
        at(&summary, &["ExecutionEngineId"]).as_str(),
        Some(cluster.as_str()),
        "case {case:?}: the summary names the execution engine, got {summary}"
    );
    assert_eq!(
        at(&summary, &["EditorId"]).as_str(),
        at(&execution, &["EditorId"]).as_str(),
        "case {case:?}: the summary and the description agree, got {summary}"
    );
    assert_eq!(
        at(&summary, &["NotebookS3Location", "Bucket"]).as_str(),
        Some("notebooks"),
        "case {case:?}: the summary carries the notebook location, got {summary}"
    );

    // Past the span the clock owes the execution each status of the progression in turn,
    // up to the terminal one, with the instant it ended and the reason it ended for.
    tokio::time::sleep(PAST_TRANSITION).await;
    let ended = describe(&emr, &id).await;
    assert_eq!(
        at(&ended, &["Status"]).as_str(),
        Some(NOT_EXECUTED),
        "case {case:?}: expected the execution to end without running, got {ended}"
    );
    assert!(
        at(&ended, &["LastStateChangeReason"])
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "case {case:?}: an ended execution says why, got {ended}"
    );
    let end_time = at(&ended, &["EndTime"]).as_f64().unwrap_or_default();
    assert!(
        end_time >= start_time,
        "case {case:?}: expected an EndTime at or after the StartTime, got {end_time} \
         against {start_time}"
    );
    // A read reports; it does not move the execution on. The second description is the
    // first one.
    let again = describe(&emr, &id).await;
    assert_eq!(
        again, ended,
        "case {case:?}: a second read reports the same execution, got {again}"
    );
}

// ---------------------------------------------------------------------------
// ListNotebookExecutions: the filters
// ---------------------------------------------------------------------------

/// One filter row: the request `ListNotebookExecutions` is given and the execution
/// names it must report, newest first.
struct Filter {
    case: &'static str,
    request: Value,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn the_listing_reports_the_executions_matching_every_filter_it_is_given() {
    let emr = Emr::new();
    let first_cluster = emr.run_job_flow(kept_alive_request("engine-one")).await;
    let second_cluster = emr.run_job_flow(kept_alive_request("engine-two")).await;

    let before = epoch_secs(SystemTime::now()) - 60.0;
    // Four executions, differing in the editor they belong to and the engine they name,
    // started in this order — so the listing's newest-first order is the reverse.
    for (name, engine, editor) in [
        ("alpha", &first_cluster, Some("e-FIRST")),
        ("beta", &first_cluster, Some("e-SECOND")),
        ("gamma", &second_cluster, Some("e-FIRST")),
        ("delta", &second_cluster, None),
    ] {
        start(&emr, start_request(name, engine, editor)).await;
    }
    // Past the transition span every execution has reached the status the clock owed it,
    // so the status rows below are stable however long the rest of this test takes.
    tokio::time::sleep(PAST_TRANSITION).await;
    let after = epoch_secs(SystemTime::now()) + 60.0;

    let mut rows = vec![
        Filter {
            case: "no filter at all",
            request: json!({}),
            expected: vec!["delta", "gamma", "beta", "alpha"],
        },
        Filter {
            case: "an editor two executions belong to",
            request: json!({ "EditorId": "e-FIRST" }),
            expected: vec!["gamma", "alpha"],
        },
        Filter {
            case: "an editor one execution belongs to",
            request: json!({ "EditorId": "e-SECOND" }),
            expected: vec!["beta"],
        },
        Filter {
            case: "an editor no execution belongs to",
            request: json!({ "EditorId": "e-NOBODY" }),
            expected: vec![],
        },
        Filter {
            case: "the first execution engine",
            request: json!({ "ExecutionEngineId": first_cluster }),
            expected: vec!["beta", "alpha"],
        },
        Filter {
            case: "the second execution engine",
            request: json!({ "ExecutionEngineId": second_cluster }),
            expected: vec!["delta", "gamma"],
        },
        Filter {
            case: "an execution engine no execution names",
            request: json!({ "ExecutionEngineId": "j-NOSUCHCLUSTER" }),
            expected: vec![],
        },
        Filter {
            case: "an editor and an engine together",
            request: json!({ "EditorId": "e-FIRST", "ExecutionEngineId": second_cluster }),
            expected: vec!["gamma"],
        },
        Filter {
            case: "an editor and an engine that no one execution satisfies at once",
            request: json!({ "EditorId": "e-SECOND", "ExecutionEngineId": second_cluster }),
            expected: vec![],
        },
        Filter {
            case: "a time range around every execution",
            request: json!({ "From": before, "To": after }),
            expected: vec!["delta", "gamma", "beta", "alpha"],
        },
        Filter {
            case: "a time range that begins after every execution started",
            request: json!({ "From": after }),
            expected: vec![],
        },
        Filter {
            case: "a time range that ends before every execution started",
            request: json!({ "To": before }),
            expected: vec![],
        },
        Filter {
            case: "a status filter beside a time range and an editor",
            request: json!({
                "EditorId": "e-FIRST",
                "From": before,
                "To": after,
                "Status": NOT_EXECUTED,
            }),
            expected: vec!["gamma", "alpha"],
        },
    ];
    // Every status the enum declares is a row: the one the clock owed these executions
    // matches all four, and no other status matches any.
    for status in NOTEBOOK_EXECUTION_STATUSES {
        rows.push(Filter {
            case: "one declared status",
            request: json!({ "Status": status }),
            expected: if status == NOT_EXECUTED {
                vec!["delta", "gamma", "beta", "alpha"]
            } else {
                vec![]
            },
        });
    }

    for row in rows {
        let label = format!("{} ({})", row.case, row.request);
        let (names, marker) = listed(&emr, row.request.clone()).await;
        assert_eq!(
            names, row.expected,
            "case {label:?}: expected {:?}, got {names:?}",
            row.expected
        );
        assert!(
            marker.is_none(),
            "case {label:?}: a page that fits needs no marker, got {marker:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListNotebookExecutions: paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_pages_at_the_documented_maximum_and_refuses_an_unknown_marker() {
    let case = "paging";
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("paging-host")).await;

    // One execution past two pages' worth of the documented maximum, so the first page
    // is full, the second carries the remainder and nothing is reported twice.
    let overflow = 3;
    let total = PAGE_SIZE + overflow;
    let mut started = Vec::new();
    for index in 0..total {
        let name = format!("execution-{index:03}");
        started.push(start(&emr, start_request(&name, &cluster, None)).await);
    }

    let page = emr.ok("ListNotebookExecutions", json!({})).await;
    let first: Vec<String> = at(&page, &["NotebookExecutions"])
        .as_array()
        .map(|executions| {
            executions
                .iter()
                .filter_map(|execution| {
                    at(execution, &["NotebookExecutionId"])
                        .as_str()
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        first.len(),
        PAGE_SIZE,
        "case {case:?}: expected a full page of {PAGE_SIZE}, got {}",
        first.len()
    );
    let marker = at(&page, &["Marker"])
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !marker.is_empty(),
        "case {case:?}: a truncated listing hands back a marker, got {page}"
    );

    let next = emr
        .ok("ListNotebookExecutions", json!({ "Marker": marker.clone() }))
        .await;
    let second: Vec<String> = at(&next, &["NotebookExecutions"])
        .as_array()
        .map(|executions| {
            executions
                .iter()
                .filter_map(|execution| {
                    at(execution, &["NotebookExecutionId"])
                        .as_str()
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        second.len(),
        overflow,
        "case {case:?}: expected the remaining {overflow}, got {}",
        second.len()
    );
    assert_eq!(
        second.first().map(String::as_str),
        Some(marker.as_str()),
        "case {case:?}: the next page begins at the marker, got {next}"
    );
    assert!(
        at(&next, &["Marker"]).is_null(),
        "case {case:?}: the last page hands back no marker, got {next}"
    );

    // Both pages together are every execution that was started, each exactly once.
    let mut reported: Vec<String> = first.into_iter().chain(second).collect();
    let mut expected = started;
    reported.sort();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: the pages together are every execution, got {reported:?}"
    );

    // A marker naming no execution of the filtered list is refused rather than silently
    // treated as the start of the list.
    for marker in ["ex-NOTAMARKER", ""] {
        let (status, payload) = emr
            .call("ListNotebookExecutions", json!({ "Marker": marker }))
            .await;
        assert_refused(marker, status, &payload, MODELED_CLIENT_ERROR);
    }
}

// ---------------------------------------------------------------------------
// StartNotebookExecution: the execution engine has to resolve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_execution_engine_that_names_no_cluster_is_refused_atomically() {
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("the-only-cluster")).await;
    let second = emr.run_job_flow(kept_alive_request("other-cluster")).await;

    // The rows differ in the FORM of the engine id: a cluster id that was never minted,
    // a name that is no cluster id at all, the empty id the member's `@length(0..256)`
    // admits, and — the admitted rows — a cluster this service holds.
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a cluster id nothing minted",
            "j-NOSUCHCLUSTER".to_string(),
            false,
        ),
        ("a value that is no cluster id", "my-cluster".to_string(), false),
        ("the empty engine id", String::new(), false),
        ("a cluster this service holds", cluster.clone(), true),
        ("a second cluster this service holds", second.clone(), true),
    ];

    for (label, engine_id, admitted) in cases {
        let (status, payload) = emr
            .call(
                "StartNotebookExecution",
                start_request("probe", &engine_id, Some("e-PROBE")),
            )
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the start to stand, got {status} with {payload}"
            );
            continue;
        }
        // An engine that resolves no cluster is a bad REQUEST: the request-validation
        // envelope, with the class's sentence naming the cluster resolution produced —
        // none — rather than the id the request carried.
        let expected = engine_sentence(None);
        assert_refused(label, status, &payload, REQUEST_VALIDATION);
        assert_eq!(
            message(label, &payload),
            expected,
            "case {label:?}: expected {expected:?}, got {payload}"
        );
        assert!(
            at(&payload, &["NotebookExecutionId"]).is_null(),
            "case {label:?}: a refusal mints no execution id, got {payload}"
        );
    }

    // The refused starts wrote nothing: only the two admitted ones are reported, and
    // both name a cluster that exists.
    let (names, _) = listed(&emr, json!({})).await;
    assert_eq!(
        names.len(),
        2,
        "case \"atomicity\": expected only the admitted executions, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// StopNotebookExecution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_an_execution_moves_it_through_stop_pending_to_stopped() {
    let case = "stop";
    let emr = Emr::new();
    let cluster = emr.run_job_flow(kept_alive_request("stop-host")).await;
    let id = start(&emr, start_request("stopped", &cluster, Some("e-STOP"))).await;

    emr.ok(
        "StopNotebookExecution",
        json!({ "NotebookExecutionId": id.clone() }),
    )
    .await;
    let stopping = describe(&emr, &id).await;
    assert_eq!(
        at(&stopping, &["Status"]).as_str(),
        Some("STOP_PENDING"),
        "case {case:?}: a stop is pending before it is done, got {stopping}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let stopped = describe(&emr, &id).await;
    assert_eq!(
        at(&stopped, &["Status"]).as_str(),
        Some("STOPPED"),
        "case {case:?}: expected the stop to complete, got {stopped}"
    );
    assert!(
        at(&stopped, &["EndTime"]).as_f64().is_some(),
        "case {case:?}: a stopped execution reports when it ended, got {stopped}"
    );
    // The status filter sees the stopped execution, and no longer sees it as pending.
    let (stopped_now, _) = listed(&emr, json!({ "Status": "STOPPED" })).await;
    assert_eq!(
        stopped_now,
        vec!["stopped".to_string()],
        "case {case:?}: expected the stopped execution under its status, got {stopped_now:?}"
    );
    let (still_pending, _) = listed(&emr, json!({ "Status": START_PENDING })).await;
    assert!(
        still_pending.is_empty(),
        "case {case:?}: the execution is no longer pending, got {still_pending:?}"
    );

    // An execution in a status it never leaves has nothing left to stop, so a second
    // stop reports success and changes nothing.
    emr.ok(
        "StopNotebookExecution",
        json!({ "NotebookExecutionId": id.clone() }),
    )
    .await;
    let unchanged = describe(&emr, &id).await;
    assert_eq!(
        unchanged, stopped,
        "case {case:?}: stopping a stopped execution changes nothing, got {unchanged}"
    );
}

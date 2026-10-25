// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListTaskInstances`.
//!
//! The rule under test: `ListTaskInstances` reports one `TaskInstanceSummary` per task
//! instance the STORE holds under the run its `RunId` names inside the workflow its
//! `WorkflowArn` names, in the order the run keys those instances (their
//! `TaskInstanceId`) — the same set and order `GetWorkflowRun` reports as
//! `RunDetail.TaskInstances` — at most `MaxResults` per page, continued through the
//! opaque `NextToken` this service itself issued for that one run's walk. Each summary
//! carries the stored instance's `TaskInstanceId`, `Status` and `OperatorName`, the
//! `WorkflowArn`/`RunId`/`WorkflowVersion` of the run that owns it, and a
//! `DurationInSeconds` derived from the instance's own `StartedAt`/`EndedAt` — "null if
//! the task is not complete" (`smithy_query(query="shape",
//! shape="TaskInstanceSummary")`, `DurationInSeconds`). A parent this endpoint does not
//! hold (an absent workflow, an absent run, a workflow of another region) contributes no
//! instance, so the answer is the empty page the output models rather than an error shape
//! the operation's own set does not declare. The listing persists nothing.
//!
//! The tables span what the rule generalises over: the number of instances a run holds
//! (none, one, several, more than the default page size), the page size (both published
//! `MaxResults` bounds, sizes between them, and the modelled default), each instance's
//! status, which optional members it carries, whether it has started or completed, the
//! length of a completed span, the form and length of the `TaskInstanceId`, and where a
//! presented cursor came from.
//!
//! No modelled operation of this service CREATES a task instance: one exists only once
//! something executes a workflow definition, and this emulator has no Apache Airflow
//! engine to execute one with, so every run it starts holds none. These tests therefore
//! seed the records into the store the handler reads — through
//! `task_instance_reads::seed`, the same seam the `GetTaskInstance` suite uses — and
//! drive the listing over the real wire path, which is what this operation owns.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    TaskInstance, RUN_STATUS_STARTING, TASK_INSTANCE_STATUS_NONE,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::child_listing_error_set::{declared_errors, shape_name};
use crate::missing_resource_lookups::ghost_arns;
use crate::run_lifecycle_progression::with_run;
use crate::task_instance_reads::{at, instance, seed};
use crate::validation_errors::smithy_model;
use crate::workflow_lifecycle::Harness;
use crate::workflow_run_listing::created_workflow;

/// The `@paginated` `items` member this listing reports a page in
/// (`smithy_query(query="operation", operation="ListTaskInstances")`).
const ITEMS: &str = "TaskInstances";

/// The page size the model applies when a request names none: `MaxResults` carries
/// `@default: 20` (`smithy_query(query="operation", operation="ListTaskInstances")`).
const DEFAULT_PAGE_SIZE: usize = 20;

/// The error shape this listing's own set does NOT declare, unlike the single-resource
/// reads that resolve the same references.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Create a workflow and start a run under it, pinned so no clock-owed status change
/// falls due for the rest of the test; report `(WorkflowArn, RunId, WorkflowVersion)`.
async fn pinned_run(harness: &Harness, name: &str) -> (String, String, String) {
    let arn = created_workflow(harness, name).await;
    let (status, created) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected GetWorkflow to succeed, got {status} {created:?}"
    );
    let version = created["WorkflowVersion"]
        .as_str()
        .expect("setup: a created workflow has a version")
        .to_string();

    let (status, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string();

    // Far enough out that no transition is owed while the test runs, so what the store
    // holds for this run cannot move between two calls of one assertion.
    with_run(harness, &arn, &run_id, |run| {
        run.next_status_at = SystemTime::now() + Duration::from_secs(3_600);
    });

    (arn, run_id, version)
}

/// Call `ListTaskInstances` over the wire and report its status and body.
async fn list(
    harness: &Harness,
    workflow_arn: &str,
    run_id: &str,
    max_results: Option<i64>,
    token: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({"WorkflowArn": workflow_arn, "RunId": run_id});
    if let Some(size) = max_results {
        body["MaxResults"] = serde_json::Value::from(size);
    }
    if let Some(token) = token {
        body["NextToken"] = serde_json::Value::String(token.to_string());
    }
    harness.call("ListTaskInstances", body).await
}

/// The `TaskInstanceId`s one page reports, in the order it reported them.
fn page_ids(page: &serde_json::Value) -> Vec<String> {
    page[ITEMS]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["TaskInstanceId"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Walk the whole listing through the cursors it issues, asserting at each step that no
/// page exceeds the page size and no instance is reported twice; report the ids seen, in
/// order.
async fn walk(
    case: &str,
    harness: &Harness,
    workflow_arn: &str,
    run_id: &str,
    max_results: Option<i64>,
    expected_total: usize,
) -> Vec<String> {
    let size = max_results.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per item plus the first and a slack page: a walk needing more than that
    // is not making progress.
    let page_budget = expected_total + 2;

    for step in 0..=page_budget {
        assert!(
            step < page_budget,
            "case {case:?}: the walk must finish within {page_budget} pages, got {seen:?}"
        );
        let (status, page) = list(harness, workflow_arn, run_id, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}/page {step}: expected HTTP 200, got {status} {page:?}"
        );
        let ids = page_ids(&page);
        assert!(
            page[ITEMS].is_array(),
            "case {case:?}/page {step}: expected a {ITEMS} list, got {:?}",
            page[ITEMS]
        );
        assert!(
            ids.len() <= size,
            "case {case:?}/page {step}: expected at most {size} items, got {ids:?}"
        );
        assert!(
            ids.iter().all(|id| !seen.contains(id)),
            "case {case:?}/page {step}: the walk must not repeat an instance, got {ids:?} \
             after {seen:?}"
        );
        seen.extend(ids);
        match page["NextToken"].as_str() {
            Some(cursor) => token = Some(cursor.to_string()),
            None => break,
        }
    }
    seen
}

/// One row of the summary table: the record to seed and the `DurationInSeconds` the
/// listing must derive for it.
struct Row {
    task: TaskInstance,
    expected_duration: Option<i32>,
}

/// The rows. They differ along status, which optional members are set, whether the task
/// started or completed, the length of the span and the form and length of the
/// `TaskInstanceId` — and they are seeded in an order that is NOT their key order, so a
/// listing that reported insertion order would fail.
fn summary_rows() -> Vec<Row> {
    // 255 characters, the maximum `IdString` allows
    // (`smithy_query(query="shape", shape="IdString")`: length 1..255).
    let longest_id = format!("z{}", "t".repeat(254));

    let mut running = instance("extract.data_task-01", "RUNNING");
    running.operator_name = Some("PythonOperator".to_string());
    running.started_at = Some(at(1_000));

    let mut succeeded = instance("transform-02", "SUCCESS");
    succeeded.operator_name = Some("BashOperator".to_string());
    succeeded.started_at = Some(at(2_000));
    succeeded.ended_at = Some(at(3_000));

    let mut failed = instance("load-03", "FAILED");
    failed.operator_name = Some("SqlOperator".to_string());
    failed.started_at = Some(at(4_000));
    failed.ended_at = Some(at(4_001));

    let mut upstream_failed = instance(&longest_id, "UPSTREAM_FAILED");
    upstream_failed.started_at = Some(at(5_000));
    upstream_failed.ended_at = Some(at(3_605_000));

    let mut zero_span = instance("deferred.task_04", "DEFERRED");
    zero_span.started_at = Some(at(6_000));
    zero_span.ended_at = Some(at(6_000));

    vec![
        // The longest id, and an hour-long span.
        Row {
            task: upstream_failed,
            expected_duration: Some(3_600),
        },
        // Started, not complete: DurationInSeconds stays null.
        Row {
            task: running,
            expected_duration: None,
        },
        // Complete in exactly one second.
        Row {
            task: succeeded,
            expected_duration: Some(1),
        },
        // Ended at the very instant it started: a zero span is still "complete".
        Row {
            task: zero_span,
            expected_duration: Some(0),
        },
        // A fractional span still reports the second it occupied (rounded up).
        Row {
            task: failed,
            expected_duration: Some(1),
        },
        // Not started, no optional member at all, and the shortest id.
        Row {
            task: instance("a", TASK_INSTANCE_STATUS_NONE),
            expected_duration: None,
        },
    ]
}

#[tokio::test]
async fn every_stored_instance_is_reported_member_for_member_in_key_order() {
    let harness = Harness::new("us-west-2");
    let (arn, run_id, version) = pinned_run(&harness, "taskInstanceListing").await;

    let rows = summary_rows();
    for row in &rows {
        seed(&harness, &arn, &run_id, row.task.clone());
    }

    let (status, page) = list(&harness, &arn, &run_id, Some(100), None).await;
    assert_eq!(
        status, 200,
        "case whole run: expected HTTP 200, got {status} {page:?}"
    );
    assert!(
        page["NextToken"].is_null(),
        "case whole run: one page holding every instance ends the walk, got {:?}",
        page["NextToken"]
    );

    // The order is the run's key order, not the order the records were seeded in.
    let mut expected_ids: Vec<String> = rows
        .iter()
        .map(|row| row.task.task_instance_id.clone())
        .collect();
    expected_ids.sort();
    assert_eq!(
        page_ids(&page),
        expected_ids,
        "case whole run: expected the instances in TaskInstanceId order, got {:?}",
        page[ITEMS]
    );

    let items = page[ITEMS]
        .as_array()
        .unwrap_or_else(|| panic!("case whole run: expected a {ITEMS} list, got {page:?}"))
        .clone();

    for row in &rows {
        let id = row.task.task_instance_id.as_str();
        let case = format!("{} ({} character id)", row.task.status, id.len());
        let summary = items
            .iter()
            .find(|item| item["TaskInstanceId"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {case:?}: expected a summary for {id:?}, got {items:?}"));

        // The three members the OWNING RUN decides, read from the run and not echoed
        // from the request.
        assert_eq!(
            summary["WorkflowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the run's WorkflowArn, got {:?}",
            summary["WorkflowArn"]
        );
        assert_eq!(
            summary["RunId"].as_str(),
            Some(run_id.as_str()),
            "case {case:?}: expected the run's RunId, got {:?}",
            summary["RunId"]
        );
        assert_eq!(
            summary["WorkflowVersion"].as_str(),
            Some(version.as_str()),
            "case {case:?}: expected the version the run executes, got {:?}",
            summary["WorkflowVersion"]
        );

        assert_eq!(
            summary["Status"].as_str(),
            Some(row.task.status.as_str()),
            "case {case:?}: expected the stored Status, got {:?}",
            summary["Status"]
        );
        assert_eq!(
            summary["OperatorName"].as_str(),
            row.task.operator_name.as_deref(),
            "case {case:?}: expected OperatorName {:?}, got {:?}",
            row.task.operator_name,
            summary["OperatorName"]
        );
        assert_eq!(
            summary["DurationInSeconds"].as_i64(),
            row.expected_duration.map(i64::from),
            "case {case:?}: expected DurationInSeconds {:?}, got {:?}",
            row.expected_duration,
            summary["DurationInSeconds"]
        );

        // The other reader of the same record agrees member for member.
        let (status, read) = harness
            .call(
                "GetTaskInstance",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "RunId": run_id,
                    "TaskInstanceId": id,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetTaskInstance to succeed, got {status} {read:?}"
        );
        for member in [
            "WorkflowArn",
            "RunId",
            "WorkflowVersion",
            "TaskInstanceId",
            "Status",
            "OperatorName",
            "DurationInSeconds",
        ] {
            assert_eq!(
                summary[member], read[member],
                "case {case:?}: the listing and GetTaskInstance must agree on {member}, got \
                 {:?} and {:?}",
                summary[member], read[member]
            );
        }
    }

    // And `GetWorkflowRun` reports the very same set, in the very same order.
    let (status, run) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case cross-reader: expected GetWorkflowRun to succeed, got {status} {run:?}"
    );
    let listed: Vec<String> = run["RunDetail"]["TaskInstances"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed, expected_ids,
        "case cross-reader: GetWorkflowRun must list the instances this listing reports, \
         got {listed:?}"
    );
}

/// How many instances the paging rows seed, spanning none, one, a couple, several and
/// more than the modelled default page size.
const COUNTS: &[usize] = &[0, 1, 2, 5, 25];

/// The page sizes the paging rows ask for: both published bounds of `MaxResults`
/// (`@range(min: 1, max: 100)`), two sizes between them, and none at all — which is the
/// modelled default.
const PAGE_SIZES: &[Option<i64>] = &[Some(1), Some(2), Some(3), Some(100), None];

#[tokio::test]
async fn a_page_holds_at_most_max_results_and_the_cursor_covers_the_whole_run() {
    let harness = Harness::new("eu-west-1");

    for count in COUNTS {
        let (arn, run_id, _) = pinned_run(&harness, &format!("taskInstancePaging{count}")).await;
        // Seeded in DESCENDING key order, so the ascending order the walk must report is
        // not the order the records arrived in.
        let mut expected_ids: Vec<String> = Vec::new();
        for index in (0..*count).rev() {
            let id = format!("task.{index:02}");
            seed(&harness, &arn, &run_id, instance(&id, "SCHEDULED"));
            expected_ids.push(id);
        }
        expected_ids.sort();

        for size in PAGE_SIZES {
            let case = format!("{count} instances/MaxResults {size:?}");
            let seen = walk(&case, &harness, &arn, &run_id, *size, *count).await;
            assert_eq!(
                seen, expected_ids,
                "case {case:?}: the walk must cover every instance exactly once, in key \
                 order, got {seen:?}"
            );
        }

        // The page the listing hands back is bounded by the page size itself, and the
        // cursor is issued exactly when instances remain.
        for size in PAGE_SIZES {
            let case = format!("{count} instances/first page at MaxResults {size:?}");
            let effective = size.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
            let (status, page) = list(&harness, &arn, &run_id, *size, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {page:?}"
            );
            assert_eq!(
                page_ids(&page).len(),
                effective.min(*count),
                "case {case:?}: expected min({effective}, {count}) items, got {:?}",
                page[ITEMS]
            );
            assert_eq!(
                page["NextToken"].is_string(),
                *count > effective,
                "case {case:?}: a cursor is issued exactly when instances remain, got {:?}",
                page["NextToken"]
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_is_a_position_in_the_one_walk_it_was_issued_for() {
    let harness = Harness::new("us-east-1");
    let (arn, first_run, _) = pinned_run(&harness, "taskInstanceCursorFirst").await;
    let (other_arn, other_run, _) = pinned_run(&harness, "taskInstanceCursorOther").await;

    // A second run under the SAME workflow, so the refusal is about the run the cursor
    // was issued on and not merely about the workflow.
    let (status, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected a second StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let sibling_run = started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string();

    let ids = ["task.a", "task.b", "task.c"];
    for (workflow, run) in [
        (&arn, &first_run),
        (&arn, &sibling_run),
        (&other_arn, &other_run),
    ] {
        for id in ids {
            seed(&harness, workflow, run, instance(id, "QUEUED"));
        }
    }

    // The cursor under test: issued on the first run's walk, with instances still to come.
    let (status, first_page) = list(&harness, &arn, &first_run, Some(1), None).await;
    assert_eq!(
        status, 200,
        "setup: expected the first page to succeed, got {status} {first_page:?}"
    );
    let token = first_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("setup: {} instances at MaxResults 1 must leave a cursor, got {first_page:?}", ids.len())
        })
        .to_string();

    // A cursor issued by ANOTHER of this service's lists, over the very same workflow.
    let (status, runs_page) = harness
        .call(
            "ListWorkflowRuns",
            serde_json::json!({"WorkflowArn": arn, "MaxResults": 1}),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected ListWorkflowRuns to succeed, got {status} {runs_page:?}"
    );
    let run_listing_token = runs_page["NextToken"]
        .as_str()
        .expect("setup: two runs at MaxResults 1 must leave a cursor")
        .to_string();

    // Rows: where the presented cursor came from, and which walk it is presented to.
    let ghost = ghost_arns()
        .into_iter()
        .next()
        .expect("a workflow ARN no store ever held");
    let mut rows: Vec<(String, String, String, String)> = vec![
        (
            "another run of the same workflow".to_string(),
            arn.clone(),
            sibling_run.clone(),
            token.clone(),
        ),
        (
            "another workflow's run".to_string(),
            other_arn.clone(),
            other_run.clone(),
            token.clone(),
        ),
        (
            "a workflow this endpoint does not hold".to_string(),
            ghost,
            first_run.clone(),
            token.clone(),
        ),
        (
            "a cursor issued by ListWorkflowRuns".to_string(),
            arn.clone(),
            first_run.clone(),
            run_listing_token,
        ),
    ];
    // Tokens this service never issued at all, in several forms.
    for (label, forged) in [
        ("not a token at all", "next-page".to_string()),
        ("an empty token", String::new()),
        // `7061676520322e2e` is the hex of `page 2..`: well-formed hex carrying something
        // this service never minted.
        (
            "hex that decodes to something else",
            "7061676520322e2e".to_string(),
        ),
        (
            "this service's own cursor with a byte appended",
            format!("{token}00"),
        ),
    ] {
        rows.push((
            label.to_string(),
            arn.clone(),
            first_run.clone(),
            forged,
        ));
    }

    for (label, workflow, run, presented) in &rows {
        for size in [Some(1), Some(100)] {
            let case = format!("{label}/MaxResults {size:?}");
            let (status, answered) =
                list(&harness, workflow, run, size, Some(presented.as_str())).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a cursor that is not this walk's, got \
                 {status} {answered:?}"
            );
            assert!(
                answered["__type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains("ValidationException")),
                "case {case:?}: expected a ValidationException, got {:?}",
                answered["__type"]
            );
            assert!(
                answered["Message"]
                    .as_str()
                    .is_some_and(|message| message.contains("NextToken")),
                "case {case:?}: the refusal must name the offending member, got {:?}",
                answered["Message"]
            );
            assert!(
                answered[ITEMS].is_null(),
                "case {case:?}: a refusal reports no page, got {:?}",
                answered[ITEMS]
            );
        }
    }

    // The control: the same cursor presented back on the walk that issued it continues
    // that walk rather than restarting it.
    let (status, second_page) = list(&harness, &arn, &first_run, Some(1), Some(&token)).await;
    assert_eq!(
        status, 200,
        "case own cursor: expected the issuing walk to accept it, got {status} {second_page:?}"
    );
    assert_eq!(
        page_ids(&second_page),
        vec![ids[1].to_string()],
        "case own cursor: expected the walk to continue after the first instance, got {:?}",
        second_page[ITEMS]
    );

    // Every run still holds exactly the instances it was seeded with: the refusals
    // changed nothing.
    for (workflow, run) in [
        (&arn, &first_run),
        (&arn, &sibling_run),
        (&other_arn, &other_run),
    ] {
        let stored = stored_ids(&harness, workflow, run);
        assert_eq!(
            stored,
            ids.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
            "case {run}: a refused cursor must not change what the store holds, got {stored:?}"
        );
    }
}

/// The `TaskInstanceId`s the STORE holds for a run, read directly rather than through the
/// listing under test.
fn stored_ids(harness: &Harness, workflow_arn: &str, run_id: &str) -> Vec<String> {
    harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
        .lock()
        .workflows
        .get(workflow_arn)
        .and_then(|workflow| workflow.runs.get(run_id))
        .map(|run| run.task_instance_ids())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_parent_this_endpoint_does_not_hold_is_an_empty_page() {
    let harness = Harness::new("ap-south-1");
    let (arn, run_id, _) = pinned_run(&harness, "taskInstanceAbsentParent").await;
    seed(&harness, &arn, &run_id, instance("present.task-01", "SUCCESS"));

    // The fact the expectation rests on, read from the model rather than written out
    // here: this listing's own error set declares no not-found error, so it has no
    // modelled shape to refuse an absent parent with.
    let declared = declared_errors(&smithy_model(), "ListTaskInstances");
    assert!(
        !declared.iter().any(|name| name == NOT_FOUND),
        "case error set: these rows assume ListTaskInstances declares no {NOT_FOUND}; the \
         model now declares {declared:?}"
    );

    // Rows: why the parent is absent, over the endpoint each call is made at.
    let ghost = ghost_arns()
        .into_iter()
        .next()
        .expect("a workflow ARN no store ever held");
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "a workflow no store ever held",
            harness.region(),
            ghost,
            run_id.clone(),
        ),
        (
            "a run the workflow does not hold",
            harness.region(),
            arn.clone(),
            "run-absent0000000".to_string(),
        ),
        (
            "a workflow another region's endpoint holds",
            "eu-central-1",
            arn.clone(),
            run_id.clone(),
        ),
    ];

    for (label, endpoint, workflow, run) in &rows {
        for size in [None, Some(1), Some(100)] {
            let case = format!("{label}/MaxResults {size:?}");
            let mut body = serde_json::json!({"WorkflowArn": workflow, "RunId": run});
            if let Some(size) = size {
                body["MaxResults"] = serde_json::Value::from(size);
            }
            let (status, answered) = harness
                .call_in(endpoint, "ListTaskInstances", body)
                .await;
            if let Some(kind) = answered["__type"].as_str() {
                assert!(
                    declared.iter().any(|name| name == shape_name(kind)),
                    "case {case:?}: {kind:?} is outside the error set this operation declares \
                     ({declared:?}), got {status} {answered:?}"
                );
            }
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {answered:?}"
            );
            assert_eq!(
                answered[ITEMS].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected an empty {ITEMS} list, got {:?}",
                answered[ITEMS]
            );
            assert!(
                answered["NextToken"].is_null(),
                "case {case:?}: an empty page ends the walk, got {:?}",
                answered["NextToken"]
            );
        }
    }

    // The control: the run this endpoint DOES hold still reports its instance, so an
    // empty page is computed from what is stored rather than always answered.
    let (status, page) = list(&harness, &arn, &run_id, None, None).await;
    assert_eq!(
        status, 200,
        "case held run: expected HTTP 200, got {status} {page:?}"
    );
    assert_eq!(
        page_ids(&page),
        vec!["present.task-01".to_string()],
        "case held run: expected the instance the store holds, got {:?}",
        page[ITEMS]
    );
}

#[tokio::test]
async fn listing_task_instances_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new("us-east-2");
    let (arn, run_id, _) = pinned_run(&harness, "taskInstanceReportingList").await;
    for id in ["b.second-task", "a.first-task"] {
        seed(&harness, &arn, &run_id, instance(id, "SCHEDULED"));
    }

    let (first_status, first) = list(&harness, &arn, &run_id, Some(100), None).await;
    let (second_status, second) = list(&harness, &arn, &run_id, Some(100), None).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case repeated listing: expected both calls to succeed, got {first_status} {first:?} \
         then {second_status} {second:?}"
    );
    assert_eq!(
        first, second,
        "case repeated listing: a reporting operation must answer identically, got {first:?} \
         then {second:?}"
    );

    // The listing moved no run status, failed no run and consumed no instance.
    let (status, error_message) = with_run(&harness, &arn, &run_id, |run| {
        (run.status.clone(), run.error_message.clone())
    });
    assert_eq!(
        status, RUN_STATUS_STARTING,
        "case reporting listing: the listing must not move the run's status, got {status:?}"
    );
    assert_eq!(
        error_message, None,
        "case reporting listing: the listing must not fail the run, got {error_message:?}"
    );
    assert_eq!(
        stored_ids(&harness, &arn, &run_id),
        vec!["a.first-task".to_string(), "b.second-task".to_string()],
        "case reporting listing: the run must still hold exactly the seeded instances, got \
         {:?}",
        stored_ids(&harness, &arn, &run_id)
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `GetWorkflowRuns` when there is no run to report.
//!
//! RULE: a read whose answer carries a list the model gives a minimum length of one has no
//! 2xx shape that reports nothing, so a read of a resource the store DOES hold that has no
//! element to report on this page is refused with the operation's own modelled not-found
//! error at HTTP 400 — never answered 200 with the list member left out — and every answered
//! page carries a non-empty list.
//!
//! For `GetWorkflowRuns` the list is `Runs` (`WorkflowRuns`, len 1..1000, from
//! `smithy_query(query="operation", operation="GetWorkflowRuns")`) and the modelled refusal
//! is `EntityNotFoundException` (HTTP 400), so a client reading a run history either gets a
//! non-empty `Runs` or an error code it can branch on: "no runs yet" is representable, and a
//! direct member access on a success can never miss.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many runs the workflow holds
//! (none, one, several), whether it even has an entry point a run could be started through,
//! the page size asked for (including both published bounds of `MaxResults`), whether the
//! reader asked for the workflow graph, and whether the request carries a cursor that names
//! a place past the end of the history.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::history_page_cursor;
use service_com_amazonaws_glue_awsglue::state::messages::no_workflow_runs_found;
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::emulator_with_engine;
use crate::wire::{call, error_code, error_message, ok};
use crate::workflow_run_listing::given_workflow_with_runs;

/// What a row expects of the read: a refusal, or a page of that many runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 400 with the operation's modelled `EntityNotFoundException`.
    Refused,
    /// HTTP 200 whose `Runs` holds exactly this many runs — never zero.
    Served(usize),
}

/// A cursor naming a place past the END of a newest-first run history: the shared history
/// cursor ([`history_page_cursor`]) over the very beginning of the clock, which every run's
/// own key sorts above, so the page it resumes selects nothing.
fn cursor_past_the_end() -> String {
    history_page_cursor(SystemTime::UNIX_EPOCH, "wr-none")
}

/// Read the run history of `workflow` with whatever extra members `extra` carries.
async fn read_history(
    router: &MultiServiceRouter,
    workflow: &str,
    extra: &Value,
) -> (u16, Value) {
    let mut request = json!({ "Name": workflow });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    call(router, "GetWorkflowRuns", &request).await
}

/// Assert one read against what the rule expects of it.
fn assert_read(case: &str, expected: Expect, status: u16, body: &Value) {
    match expected {
        Expect::Refused => {
            assert_eq!(
                status, 400,
                "case {case:?}: a read with no run to report is refused, got {status} with \
                 {body}"
            );
            assert!(
                error_code(body).ends_with("EntityNotFoundException"),
                "case {case:?}: expected an EntityNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(body),
                no_workflow_runs_found(),
                "case {case:?}: expected the wording a read with no run history carries, got \
                 {body}"
            );
            assert!(
                body.get("Runs").is_none(),
                "case {case:?}: a refusal reports no runs at all, got {body}"
            );
        }
        Expect::Served(count) => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            let runs = body
                .get("Runs")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an answered page carries Runs as a list, got {body}")
                });
            assert!(
                !runs.is_empty(),
                "case {case:?}: an answered page never carries an empty Runs, got {body}"
            );
            assert_eq!(
                runs.len(),
                count,
                "case {case:?}: expected {count} run(s) on the page, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_with_no_run_to_report_is_refused_and_an_answered_page_is_never_empty() {
    // (case, the start instant of each run the workflow holds, whether it has an entry
    //  point a run can be started through, the extra request members, what the rule expects)
    let rows: Vec<(&str, &[u64], bool, Value, Expect)> = vec![
        (
            "a workflow with no entry point, so no run was ever possible",
            &[],
            false,
            json!({}),
            Expect::Refused,
        ),
        (
            "a workflow with an entry point that has never run",
            &[],
            true,
            json!({}),
            Expect::Refused,
        ),
        (
            // The published lower bound of `MaxResults` (`PageSize`, range 1..1000).
            "never run, with the smallest page the model accepts",
            &[],
            true,
            json!({ "MaxResults": 1 }),
            Expect::Refused,
        ),
        (
            // The published upper bound.
            "never run, with the largest page the model accepts",
            &[],
            true,
            json!({ "MaxResults": 1000 }),
            Expect::Refused,
        ),
        (
            "never run, with the graph asked for",
            &[],
            true,
            json!({ "IncludeGraph": true }),
            Expect::Refused,
        ),
        (
            "never run, with a cursor",
            &[],
            true,
            json!({ "NextToken": cursor_past_the_end() }),
            Expect::Refused,
        ),
        ("one run", &[10], true, json!({}), Expect::Served(1)),
        (
            "one run, with the graph asked for",
            &[10],
            true,
            json!({ "IncludeGraph": true }),
            Expect::Served(1),
        ),
        (
            "one run, read past the end of the history",
            &[10],
            true,
            json!({ "NextToken": cursor_past_the_end() }),
            Expect::Refused,
        ),
        (
            "four runs",
            &[40, 10, 30, 20],
            true,
            json!({}),
            Expect::Served(4),
        ),
        (
            "four runs, a page smaller than the history",
            &[40, 10, 30, 20],
            true,
            json!({ "MaxResults": 3 }),
            Expect::Served(3),
        ),
        (
            "four runs, read past the end of the history",
            &[40, 10, 30, 20],
            true,
            json!({ "NextToken": cursor_past_the_end() }),
            Expect::Refused,
        ),
    ];

    for (case, offsets, entry_point, extra, expected) in &rows {
        let (router, engine) = emulator_with_engine();
        let name = "history-workflow";
        if *entry_point {
            given_workflow_with_runs(&router, &engine, name, offsets).await;
        } else {
            ok(&router, "CreateWorkflow", &json!({ "Name": name })).await;
        }

        let (status, body) = read_history(&router, name, extra).await;
        assert_read(case, *expected, status, &body);
    }
}

#[tokio::test]
async fn the_refusal_is_about_the_history_and_lifts_once_a_run_exists() {
    let (router, engine) = emulator_with_engine();
    let name = "settling-history-workflow";
    given_workflow_with_runs(&router, &engine, name, &[]).await;

    // Reading a history that holds nothing changes nothing: the same refusal, twice.
    let (first_status, first) = read_history(&router, name, &json!({})).await;
    assert_read("before any run", Expect::Refused, first_status, &first);
    let (second_status, second) = read_history(&router, name, &json!({})).await;
    assert_read("read again before any run", Expect::Refused, second_status, &second);
    assert_eq!(
        first, second,
        "case \"read twice before any run\": two refusals must agree, got {first} and {second}"
    );

    // The refusal is about the run history, not about the workflow: the workflow itself is
    // still readable, and still names the trigger attached to it.
    let read_back = ok(&router, "GetWorkflow", &json!({ "Name": name })).await;
    assert_eq!(
        read_back["Workflow"]["Name"].as_str(),
        Some(name),
        "case \"the workflow behind the empty history\": GetWorkflow must still report it, got \
         {read_back}"
    );

    // Once a run exists the same read is served, and it reports exactly that run — the
    // round trip through this operation's own reader.
    let started = ok(&router, "StartWorkflowRun", &json!({ "Name": name })).await;
    let run_id = started["RunId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {started}"))
        .to_string();
    let (status, page) = read_history(&router, name, &json!({})).await;
    assert_read("after one run was started", Expect::Served(1), status, &page);
    assert_eq!(
        page["Runs"][0]["WorkflowRunId"].as_str(),
        Some(run_id.as_str()),
        "case \"after one run was started\": expected the run just started, got {page}"
    );
    assert!(
        page.get("NextToken").is_none(),
        "case \"after one run was started\": a page that reached the end carries no token, got \
         {page}"
    );
}

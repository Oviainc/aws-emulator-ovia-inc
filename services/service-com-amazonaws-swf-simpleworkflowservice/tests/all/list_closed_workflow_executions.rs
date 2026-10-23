// SPDX-License-Identifier: CC0-1.0

//! `ListClosedWorkflowExecutions`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: a visibility listing of closed executions answers
//! from the addressed domain's STORED executions — exactly the runs whose status is
//! CLOSED, whose instant for the ONE time filter the request names (`startTimeFilter`
//! or `closeTimeFilter`, one of them but not both) lies inside that filter's window,
//! and which satisfy whichever single one of `closeStatusFilter` / `executionFilter` /
//! `typeFilter` / `tagFilter` the request names — each reported as the same
//! `WorkflowExecutionInfo` the per-execution reader reports, ordered by that same
//! instant (descending by default, ascending under `reverseOrder`) and paged by
//! `maximumPageSize` / `nextPageToken`.
//!
//! Each table row varies what that rule generalises over: which filter is named and
//! with what value (the close-status rows walk the model's whole `CloseStatus`
//! catalog), which time filter the request names, which end of its window is bounded
//! and where, the page size, the ordering direction, and how many runs match. No row
//! is privileged — in particular, the runs below are closed in the opposite order to
//! the one they were started in, so a listing ordered by close time is not the same
//! listing ordered by start time.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::CLOSE_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{full_defaults, merge, seeded, string_at, Emulator};

/// `oldestDate` is required, so every window needs a lower bound; this is the oldest
/// instant the Unix-time form the filter documents can name ("The parameter values are
/// in the Unix Time format"), and therefore a bound that excludes nothing.
const EPOCH_SECONDS: f64 = 0.0;

/// `(workflowId, (type name, type version), tags)` for every run [`seed`] starts. Two
/// type names, two versions of one of them, and tag lists of size 0, 1 and 2.
const SEEDED_RUNS: &[(&str, (&str, &str), &[&str])] = &[
    ("wf-orders-a", ("orders", "1.0"), &["shared", "red"]),
    ("wf-orders-b", ("orders", "2.0"), &["shared"]),
    ("wf-reports", ("reports", "1.0"), &[]),
    ("wf-open", ("orders", "1.0"), &["shared"]),
];

/// The seeded runs that are terminated, in the order [`seed`] terminates them — the
/// REVERSE of the order they were started in, so the close ordering of this domain is
/// not its start ordering.
const CLOSED_RUNS: &[&str] = &["wf-reports", "wf-orders-b", "wf-orders-a"];

/// The seeded run left running, so no listing of CLOSED executions may report it.
const OPEN_RUN: &str = "wf-open";

/// The only `CloseStatus` a run can reach through this emulator's implemented
/// operations: `TerminateWorkflowExecution` closes a run as TERMINATED. Every other
/// entry of [`CLOSE_STATUS_VALUES`] therefore matches no stored run here.
const REACHABLE_CLOSE_STATUS: &str = "TERMINATED";

/// The task list and child policy the seeded workflow types are registered with, so a
/// start needs no overrides.
const TASK_LIST: &str = "primary";
const CHILD_POLICY: &str = "TERMINATE";

/// Now, in the epoch-seconds form the filter's members take.
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now is after the Unix epoch")
        .as_secs_f64()
}

/// A request listing every closed execution of `domain` on `time_filter`: the widest
/// window the filter can express at its oldest end, and no other filter.
fn list_all(domain: &str, time_filter: &str) -> Value {
    json!({ "domain": domain, time_filter: { "oldestDate": EPOCH_SECONDS } })
}

/// The two time filters a closed-execution listing may name, with the
/// `WorkflowExecutionInfo` member each one orders the results by.
const TIME_FILTERS: &[(&str, &str)] = &[
    ("startTimeFilter", "/startTimestamp"),
    ("closeTimeFilter", "/closeTimestamp"),
];

/// The `executionInfos` of a listing response.
fn infos(case: &str, body: &Value) -> Vec<Value> {
    body.pointer("/executionInfos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected executionInfos in {body:?}"))
}

/// The `workflowId`s a listing reported, in the order it reported them.
fn workflow_ids(case: &str, body: &Value) -> Vec<String> {
    infos(case, body)
        .iter()
        .map(|info| string_at(info, "/execution/workflowId"))
        .collect()
}

/// The timestamps a listing reported at `pointer`, in the order it reported them.
fn timestamps(case: &str, body: &Value, pointer: &str) -> Vec<f64> {
    infos(case, body)
        .iter()
        .map(|info| {
            info.pointer(pointer).and_then(Value::as_f64).unwrap_or_else(|| {
                panic!("case {case:?}: expected a timestamp at {pointer} of {info:?}")
            })
        })
        .collect()
}

/// Sorted, so a set of expected matches can be compared without fixing an order.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// Register `(name, version)` in `domain` with every default a start could need.
async fn register_type(emulator: &Emulator, case: &str, domain: &str, name: &str, version: &str) {
    let mut body = json!({ "domain": domain, "name": name, "version": version });
    merge(&mut body, full_defaults(TASK_LIST, CHILD_POLICY));
    emulator.ok(case, "RegisterWorkflowType", &body).await;
}

/// Start one run and return the `runId` the service minted for it.
async fn start_run(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    workflow_type: (&str, &str),
    tags: &[&str],
) -> String {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": workflow_type.0, "version": workflow_type.1 },
    });
    if !tags.is_empty() {
        merge(&mut body, json!({ "tagList": tags }));
    }
    let started = emulator.ok(case, "StartWorkflowExecution", &body).await;
    string_at(&started, "/runId")
}

/// Close one run, which is what gives it a close instant and a close status.
async fn terminate_run(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str) {
    emulator
        .ok(
            case,
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": workflow_id }),
        )
        .await;
}

/// The stored record of one run, as the per-execution reader reports it.
async fn describe(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> Value {
    emulator
        .ok(
            case,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await
}

/// The domain every filter, ordering and paging case lists over: three workflow types
/// and the four runs of [`SEEDED_RUNS`], three of which are then terminated in reverse
/// start order. Returns `workflowId -> runId` for all four.
async fn seed(emulator: &Emulator, domain: &str) -> BTreeMap<String, String> {
    seeded(
        emulator,
        domain,
        domain,
        ("orders", "1.0"),
        full_defaults(TASK_LIST, CHILD_POLICY),
    )
    .await;
    register_type(emulator, domain, domain, "orders", "2.0").await;
    register_type(emulator, domain, domain, "reports", "1.0").await;

    let mut run_ids = BTreeMap::new();
    for (workflow_id, workflow_type, tags) in SEEDED_RUNS {
        let run_id = start_run(emulator, domain, domain, workflow_id, *workflow_type, tags).await;
        run_ids.insert((*workflow_id).to_string(), run_id);
    }
    for workflow_id in CLOSED_RUNS {
        terminate_run(emulator, domain, domain, workflow_id).await;
    }
    run_ids
}

// ---------------------------------------------------------------------------
// Which stored executions a listing selects
// ---------------------------------------------------------------------------

/// One listing, varying which of the mutually exclusive filters it names and with what
/// value — what "the filter the request names selects from the stored closed runs"
/// generalises over.
struct FilterCase {
    name: String,
    filter: Value,
    expected: Vec<String>,
}

#[tokio::test]
async fn each_visibility_filter_selects_from_the_stored_closed_executions() {
    let domain = "filters";
    let mut cases = vec![
        FilterCase {
            name: "no filter at all".to_string(),
            filter: json!({}),
            expected: owned(CLOSED_RUNS),
        },
        FilterCase {
            name: "executionFilter on a closed run".to_string(),
            filter: json!({ "executionFilter": { "workflowId": "wf-orders-a" } }),
            expected: owned(&["wf-orders-a"]),
        },
        FilterCase {
            name: "executionFilter on the open run".to_string(),
            filter: json!({ "executionFilter": { "workflowId": OPEN_RUN } }),
            expected: Vec::new(),
        },
        FilterCase {
            name: "executionFilter on a workflowId nothing started".to_string(),
            filter: json!({ "executionFilter": { "workflowId": "wf-never-started" } }),
            expected: Vec::new(),
        },
        FilterCase {
            // `WorkflowTypeFilter.version` is optional (`VersionOptional`, length
            // 0..64), so a name-only filter keeps every version of that name.
            name: "typeFilter naming only the type".to_string(),
            filter: json!({ "typeFilter": { "name": "orders" } }),
            expected: owned(&["wf-orders-a", "wf-orders-b"]),
        },
        FilterCase {
            name: "typeFilter naming one version".to_string(),
            filter: json!({ "typeFilter": { "name": "orders", "version": "2.0" } }),
            expected: owned(&["wf-orders-b"]),
        },
        FilterCase {
            // The only run of `orders` 1.0 that is closed; the open one is excluded by
            // the status, not by the filter.
            name: "typeFilter naming the other version".to_string(),
            filter: json!({ "typeFilter": { "name": "orders", "version": "1.0" } }),
            expected: owned(&["wf-orders-a"]),
        },
        FilterCase {
            // The empty string is inside `VersionOptional`'s 0..64 length, so it is on
            // the wire as "no version given".
            name: "typeFilter with an empty version".to_string(),
            filter: json!({ "typeFilter": { "name": "orders", "version": "" } }),
            expected: owned(&["wf-orders-a", "wf-orders-b"]),
        },
        FilterCase {
            name: "typeFilter naming a version nothing runs".to_string(),
            filter: json!({ "typeFilter": { "name": "orders", "version": "9.9" } }),
            expected: Vec::new(),
        },
        FilterCase {
            name: "typeFilter naming the other type".to_string(),
            filter: json!({ "typeFilter": { "name": "reports" } }),
            expected: owned(&["wf-reports"]),
        },
        FilterCase {
            name: "typeFilter naming a type nothing is registered under".to_string(),
            filter: json!({ "typeFilter": { "name": "invoices" } }),
            expected: Vec::new(),
        },
        FilterCase {
            name: "tagFilter on a tag two closed runs carry".to_string(),
            filter: json!({ "tagFilter": { "tag": "shared" } }),
            expected: owned(&["wf-orders-a", "wf-orders-b"]),
        },
        FilterCase {
            name: "tagFilter on a tag one run carries".to_string(),
            filter: json!({ "tagFilter": { "tag": "red" } }),
            expected: owned(&["wf-orders-a"]),
        },
        FilterCase {
            name: "tagFilter on a tag no run carries".to_string(),
            filter: json!({ "tagFilter": { "tag": "green" } }),
            expected: Vec::new(),
        },
    ];

    // One row per entry of the model's `CloseStatus` enum (the catalog
    // `CLOSE_STATUS_VALUES` mirrors, from `smithy_query(query="shape",
    // shape="CloseStatus")`), so every documented status is exercised and not just the
    // one the seeded runs reached.
    for status in CLOSE_STATUS_VALUES {
        cases.push(FilterCase {
            name: format!("closeStatusFilter on {status}"),
            filter: json!({ "closeStatusFilter": { "status": status } }),
            expected: if *status == REACHABLE_CLOSE_STATUS {
                owned(CLOSED_RUNS)
            } else {
                Vec::new()
            },
        });
    }

    let emulator = Emulator::new();
    let run_ids = seed(&emulator, domain).await;

    for case in &cases {
        for (time_filter, _) in TIME_FILTERS {
            let case_name = format!("{} via {time_filter}", case.name);
            let mut body = list_all(domain, time_filter);
            merge(&mut body, case.filter.clone());
            let listed = emulator
                .ok(&case_name, "ListClosedWorkflowExecutions", &body)
                .await;

            assert_eq!(
                sorted(&workflow_ids(&case_name, &listed)),
                sorted(&case.expected),
                "case {case_name:?}: the listing must report exactly the stored CLOSED runs \
                 the filter selects, got {listed:?}"
            );
            assert!(
                listed.get("nextPageToken").is_none(),
                "case {case_name:?}: every match fits in one page here, so no nextPageToken \
                 may be issued, got {listed:?}"
            );

            for info in infos(&case_name, &listed) {
                let workflow_id = string_at(&info, "/execution/workflowId");
                let run_id = run_ids.get(&workflow_id).unwrap_or_else(|| {
                    panic!(
                        "case {case_name:?}: the listing reported {workflow_id:?}, which no \
                         start created"
                    )
                });
                assert_eq!(
                    string_at(&info, "/executionStatus"),
                    "CLOSED",
                    "case {case_name:?}: only CLOSED runs may be listed, got {info:?}"
                );
                assert_eq!(
                    string_at(&info, "/closeStatus"),
                    REACHABLE_CLOSE_STATUS,
                    "case {case_name:?}: a terminated run is reported with the close status \
                     the terminate stored, got {info:?}"
                );
                let described = describe(&emulator, &case_name, domain, &workflow_id, run_id).await;
                assert_eq!(
                    described.pointer("/executionInfo"),
                    Some(&info),
                    "case {case_name:?}: the listing must report the one stored record the \
                     per-execution reader reports, got {info:?} against {described:?}"
                );
            }
        }
    }

    // The listings above changed nothing: the open run is still OPEN and the closed
    // ones are still CLOSED, so reading is not what moves a status.
    for (workflow_id, run_id) in &run_ids {
        let described = describe(&emulator, workflow_id, domain, workflow_id, run_id).await;
        let expected_status = if workflow_id == OPEN_RUN { "OPEN" } else { "CLOSED" };
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            expected_status,
            "case {workflow_id:?}: listing closed executions must not change a stored \
             status, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The time window, on either instant
// ---------------------------------------------------------------------------

/// One listing, varying which time filter it names, which end of that window is
/// bounded and where it sits relative to the stored instants.
struct WindowCase {
    name: &'static str,
    time_filter: &'static str,
    /// Built from a stored instant's whole second, which the harness only knows at
    /// runtime.
    window: Value,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn the_named_time_window_selects_which_stored_runs_are_listed() {
    let domain = "windows";
    let emulator = Emulator::new();
    seeded(
        &emulator,
        domain,
        domain,
        ("orders", "1.0"),
        full_defaults(TASK_LIST, CHILD_POLICY),
    )
    .await;

    // Three runs started together, one closed straight away and two closed more than a
    // second later: the start instants then share a second while the close instants do
    // not, so a window that selects on close time selects a DIFFERENT set from the same
    // window on start time.
    let mut run_ids = BTreeMap::new();
    for workflow_id in ["wf-closed-early", "wf-closed-late-a", "wf-closed-late-b"] {
        let run_id = start_run(&emulator, domain, domain, workflow_id, ("orders", "1.0"), &[]).await;
        run_ids.insert(workflow_id.to_string(), run_id);
    }
    terminate_run(&emulator, domain, domain, "wf-closed-early").await;
    // The bounds below are whole seconds, so the late closes happen at least 1.1 s
    // after the early one: their own second is then strictly later than the early close
    // instant, which is what makes each case's expectation exact rather than racy.
    std::thread::sleep(Duration::from_millis(1_100));
    terminate_run(&emulator, domain, domain, "wf-closed-late-a").await;
    terminate_run(&emulator, domain, domain, "wf-closed-late-b").await;

    let described = describe(
        &emulator,
        "late close",
        domain,
        "wf-closed-late-a",
        &run_ids["wf-closed-late-a"],
    )
    .await;
    let late_close = described
        .pointer("/executionInfo/closeTimestamp")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case \"late close\": expected a closeTimestamp in {described:?}"));
    // `late_second <= late_close`, and every start and the early close happened more
    // than 0.1 s before `late_second`, so `late_second` includes only the late closes
    // and `late_second - 0.1` includes everything else.
    let late_second = late_close.floor();
    let future = now_seconds() + 3_600.0;
    let every_run = vec!["wf-closed-early", "wf-closed-late-a", "wf-closed-late-b"];

    let cases = [
        WindowCase {
            name: "start window, oldest bound only",
            time_filter: "startTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS }),
            expected: every_run.clone(),
        },
        WindowCase {
            name: "close window, oldest bound only",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS }),
            expected: every_run.clone(),
        },
        WindowCase {
            name: "start window, both bounds spanning every run",
            time_filter: "startTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": future }),
            expected: every_run.clone(),
        },
        WindowCase {
            name: "close window, both bounds spanning every run",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": future }),
            expected: every_run.clone(),
        },
        WindowCase {
            // Every run STARTED before the late closes, so the same bound that keeps
            // two runs on close time keeps none on start time.
            name: "start window, oldest bound at the late close's second",
            time_filter: "startTimeFilter",
            window: json!({ "oldestDate": late_second }),
            expected: vec![],
        },
        WindowCase {
            name: "close window, oldest bound at the late close's second",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": late_second }),
            expected: vec!["wf-closed-late-a", "wf-closed-late-b"],
        },
        WindowCase {
            name: "close window, oldest bound at the late close's second, latest unbounded upward",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": late_second, "latestDate": future }),
            expected: vec!["wf-closed-late-a", "wf-closed-late-b"],
        },
        WindowCase {
            name: "close window, latest bound before the late closes",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": late_second - 0.1 }),
            expected: vec!["wf-closed-early"],
        },
        WindowCase {
            name: "start window, latest bound before the late closes",
            time_filter: "startTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": late_second - 0.1 }),
            expected: every_run.clone(),
        },
        WindowCase {
            name: "start window entirely in the future",
            time_filter: "startTimeFilter",
            window: json!({ "oldestDate": future }),
            expected: vec![],
        },
        WindowCase {
            name: "close window entirely in the future",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": future }),
            expected: vec![],
        },
        WindowCase {
            name: "close window entirely in the past",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": 1.0 }),
            expected: vec![],
        },
        WindowCase {
            name: "empty close window: latest before oldest",
            time_filter: "closeTimeFilter",
            window: json!({ "oldestDate": future, "latestDate": EPOCH_SECONDS }),
            expected: vec![],
        },
    ];

    for case in &cases {
        let body = json!({ "domain": domain, case.time_filter: case.window });
        let listed = emulator
            .ok(case.name, "ListClosedWorkflowExecutions", &body)
            .await;
        assert_eq!(
            sorted(&workflow_ids(case.name, &listed)),
            sorted(&owned(&case.expected)),
            "case {:?}: only runs whose stored instant for the named filter is inside the \
             window may be listed, got {listed:?}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// Ordering and paging, on either instant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ordering_and_paging_walk_the_same_stored_closed_executions() {
    let domain = "paging";
    let emulator = Emulator::new();
    let mut run_ids = seed(&emulator, domain).await;
    // A fourth closed run, so a page size of 2 or 3 leaves a remainder of a different
    // size in each ordering.
    let extra = start_run(
        &emulator,
        domain,
        domain,
        "wf-extra",
        ("reports", "1.0"),
        &["shared"],
    )
    .await;
    terminate_run(&emulator, domain, domain, "wf-extra").await;
    run_ids.insert("wf-extra".to_string(), extra);
    let mut closed = owned(CLOSED_RUNS);
    closed.push("wf-extra".to_string());

    for (time_filter, ordered_by) in TIME_FILTERS {
        let mut whole_listing: BTreeMap<bool, Vec<String>> = BTreeMap::new();
        for reverse_order in [false, true] {
            let case = format!("{time_filter}, reverseOrder={reverse_order}");
            let mut body = list_all(domain, time_filter);
            merge(&mut body, json!({ "reverseOrder": reverse_order }));
            let listed = emulator
                .ok(&case, "ListClosedWorkflowExecutions", &body)
                .await;

            assert_eq!(
                sorted(&workflow_ids(&case, &listed)),
                sorted(&closed),
                "case {case:?}: an unpaged listing must report every stored CLOSED run, \
                 got {listed:?}"
            );

            // "By default the results are returned in descending order of the start or
            // the close time of the executions"; `reverseOrder` returns them ascending.
            // Equal instants are allowed, so the check is monotonicity, not strict
            // ordering.
            let instants = timestamps(&case, &listed, ordered_by);
            for pair in instants.windows(2) {
                let ordered = if reverse_order {
                    pair[0] <= pair[1]
                } else {
                    pair[0] >= pair[1]
                };
                assert!(
                    ordered,
                    "case {case:?}: the {ordered_by} values must be ordered by the requested \
                     direction, got {instants:?}"
                );
            }
            whole_listing.insert(reverse_order, workflow_ids(&case, &listed));
        }

        let descending = whole_listing[&false].clone();
        let ascending = whole_listing[&true].clone();
        let mut reversed = ascending.clone();
        reversed.reverse();
        assert_eq!(
            descending, reversed,
            "case {time_filter:?}: the two orderings must walk one total order in opposite \
             directions, got {descending:?} against {ascending:?}"
        );

        // Paging walks exactly that order, whatever the page size: no run repeated,
        // none skipped, and the token absent only on the last page.
        for reverse_order in [false, true] {
            for page_size in [1_usize, 2, 3, 4, 1_000] {
                let case = format!(
                    "{time_filter}, reverseOrder={reverse_order}, maximumPageSize={page_size}"
                );
                let mut walked: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;
                loop {
                    let mut body = list_all(domain, time_filter);
                    merge(
                        &mut body,
                        json!({ "reverseOrder": reverse_order, "maximumPageSize": page_size }),
                    );
                    if let Some(token) = &token {
                        merge(&mut body, json!({ "nextPageToken": token }));
                    }
                    let listed = emulator
                        .ok(&case, "ListClosedWorkflowExecutions", &body)
                        .await;
                    let ids = workflow_ids(&case, &listed);
                    assert!(
                        ids.len() <= page_size,
                        "case {case:?}: a page may not exceed maximumPageSize, got {ids:?}"
                    );
                    assert!(
                        !ids.is_empty(),
                        "case {case:?}: a token the service issued must address at least one \
                         remaining run, got {listed:?}"
                    );
                    walked.extend(ids);
                    token = listed
                        .get("nextPageToken")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    pages += 1;
                    assert!(
                        pages <= closed.len() + 1,
                        "case {case:?}: paging must terminate, got {pages} pages for {} runs",
                        closed.len()
                    );
                    if token.is_none() {
                        break;
                    }
                }
                let expected = if reverse_order { &ascending } else { &descending };
                assert_eq!(
                    &walked, expected,
                    "case {case:?}: paging must walk the unpaged order exactly once, got \
                     {walked:?}"
                );
            }
        }
    }

    // The two instants give two different orders here — the runs were closed in the
    // reverse of the order they were started in — so a listing that ignored the named
    // filter when ordering would report one of them for both.
    let by_start = workflow_ids(
        "startTimeFilter order",
        &emulator
            .ok(
                "startTimeFilter order",
                "ListClosedWorkflowExecutions",
                &list_all(domain, "startTimeFilter"),
            )
            .await,
    );
    let by_close = workflow_ids(
        "closeTimeFilter order",
        &emulator
            .ok(
                "closeTimeFilter order",
                "ListClosedWorkflowExecutions",
                &list_all(domain, "closeTimeFilter"),
            )
            .await,
    );
    assert_ne!(
        by_start, by_close,
        "case \"two instants\": these runs were closed in the reverse of their start order, \
         so ordering by close time must differ from ordering by start time, got {by_start:?} \
         against {by_close:?}"
    );

    // A token is issued for ONE ordering — one direction on one instant — so replayed
    // against another it is refused rather than silently skipping or repeating runs.
    for (time_filter, _) in TIME_FILTERS {
        let mut first_page = list_all(domain, time_filter);
        merge(&mut first_page, json!({ "maximumPageSize": 1 }));
        let listed = emulator
            .ok(time_filter, "ListClosedWorkflowExecutions", &first_page)
            .await;
        let token = string_at(&listed, "/nextPageToken");

        let mut other_direction = list_all(domain, time_filter);
        merge(
            &mut other_direction,
            json!({ "maximumPageSize": 1, "reverseOrder": true, "nextPageToken": &token }),
        );
        emulator
            .refused(
                &format!("{time_filter} token under the other direction"),
                "ListClosedWorkflowExecutions",
                &other_direction,
                "ValidationException",
            )
            .await;

        let other_filter = if *time_filter == "startTimeFilter" {
            "closeTimeFilter"
        } else {
            "startTimeFilter"
        };
        let mut other_instant = list_all(domain, other_filter);
        merge(
            &mut other_instant,
            json!({ "maximumPageSize": 1, "nextPageToken": &token }),
        );
        emulator
            .refused(
                &format!("{time_filter} token under {other_filter}"),
                "ListClosedWorkflowExecutions",
                &other_instant,
                "ValidationException",
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_is_refused_for_an_unknown_domain_or_a_request_it_cannot_satisfy() {
    let domain = "refusals";
    let emulator = Emulator::new();
    seed(&emulator, domain).await;

    // A domain that was never registered is out of this operation's scope.
    emulator
        .refused(
            "unknown domain",
            "ListClosedWorkflowExecutions",
            &list_all("never-registered", "startTimeFilter"),
            "UnknownResourceFault",
        )
        .await;

    // "startTimeFilter and closeTimeFilter are mutually exclusive. You must specify one
    // of these in a request but not both." Both, and neither.
    emulator
        .refused(
            "both time filters",
            "ListClosedWorkflowExecutions",
            &json!({
                "domain": domain,
                "startTimeFilter": { "oldestDate": EPOCH_SECONDS },
                "closeTimeFilter": { "oldestDate": EPOCH_SECONDS },
            }),
            "ValidationException",
        )
        .await;
    emulator
        .refused(
            "neither time filter",
            "ListClosedWorkflowExecutions",
            &json!({ "domain": domain }),
            "ValidationException",
        )
        .await;

    // "closeStatusFilter, executionFilter, typeFilter and tagFilter are mutually
    // exclusive. You can specify at most one of these in a request." Every pair, and
    // all four.
    let exclusive: &[(&str, Value)] = &[
        (
            "closeStatusFilter with executionFilter",
            json!({
                "closeStatusFilter": { "status": REACHABLE_CLOSE_STATUS },
                "executionFilter": { "workflowId": "wf-orders-a" },
            }),
        ),
        (
            "closeStatusFilter with typeFilter",
            json!({
                "closeStatusFilter": { "status": REACHABLE_CLOSE_STATUS },
                "typeFilter": { "name": "orders" },
            }),
        ),
        (
            "closeStatusFilter with tagFilter",
            json!({
                "closeStatusFilter": { "status": REACHABLE_CLOSE_STATUS },
                "tagFilter": { "tag": "shared" },
            }),
        ),
        (
            "executionFilter with typeFilter",
            json!({
                "executionFilter": { "workflowId": "wf-orders-a" },
                "typeFilter": { "name": "orders" },
            }),
        ),
        (
            "executionFilter with tagFilter",
            json!({
                "executionFilter": { "workflowId": "wf-orders-a" },
                "tagFilter": { "tag": "shared" },
            }),
        ),
        (
            "typeFilter with tagFilter",
            json!({
                "typeFilter": { "name": "orders" },
                "tagFilter": { "tag": "shared" },
            }),
        ),
        (
            "all four filters",
            json!({
                "closeStatusFilter": { "status": REACHABLE_CLOSE_STATUS },
                "executionFilter": { "workflowId": "wf-orders-a" },
                "typeFilter": { "name": "orders" },
                "tagFilter": { "tag": "shared" },
            }),
        ),
    ];
    for (case, filters) in exclusive {
        let mut body = list_all(domain, "closeTimeFilter");
        merge(&mut body, filters.clone());
        emulator
            .refused(
                case,
                "ListClosedWorkflowExecutions",
                &body,
                "ValidationException",
            )
            .await;
    }

    // A `nextPageToken` this service never issued addresses no position in the order.
    for (case, token) in [
        ("token the service never issued", "not-a-token"),
        ("token with an unknown ordering tag", "xs6162"),
        ("token that is not hex", "dszzzz"),
    ] {
        let mut body = list_all(domain, "startTimeFilter");
        merge(&mut body, json!({ "nextPageToken": token }));
        emulator
            .refused(
                case,
                "ListClosedWorkflowExecutions",
                &body,
                "ValidationException",
            )
            .await;
    }

    // An empty `nextPageToken` is inside the modelled `PageToken` length (0..2048) and
    // means "no token", so it lists the first page rather than being refused.
    let mut body = list_all(domain, "startTimeFilter");
    merge(&mut body, json!({ "nextPageToken": "" }));
    let listed = emulator
        .ok("empty token", "ListClosedWorkflowExecutions", &body)
        .await;
    assert_eq!(
        workflow_ids("empty token", &listed).len(),
        CLOSED_RUNS.len(),
        "case \"empty token\": an empty nextPageToken means no token, so every closed run is \
         listed, got {listed:?}"
    );
}

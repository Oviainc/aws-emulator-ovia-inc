// SPDX-License-Identifier: CC0-1.0

//! `ListOpenWorkflowExecutions`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same router the emulator mounts in production.
//!
//! The rule every case here asserts: a visibility listing of open executions answers
//! from the addressed domain's STORED executions — exactly the runs whose status is
//! OPEN, whose start instant lies inside the request's `startTimeFilter` window and
//! which satisfy whichever single one of `executionFilter` / `typeFilter` /
//! `tagFilter` the request names — each reported as the same `WorkflowExecutionInfo`
//! the per-execution reader reports, ordered by start time (descending by default,
//! ascending under `reverseOrder`) and paged by `maximumPageSize` / `nextPageToken`.
//!
//! Each table row varies what that rule generalises over: which filter is named and
//! with what value, which end of the time window is bounded and where, the page size,
//! the ordering, and how many executions match. No row is privileged.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

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
    ("wf-closed", ("orders", "1.0"), &["shared"]),
];

/// The seeded run that is terminated, so no listing of OPEN executions may report it.
const CLOSED_RUN: &str = "wf-closed";

/// The seeded runs that are still OPEN once [`seed`] returns.
const OPEN_RUNS: &[&str] = &["wf-orders-a", "wf-orders-b", "wf-reports"];

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

/// A request listing every open execution of `domain`: the widest window the filter
/// can express at its oldest end, and no execution/type/tag filter.
fn list_all(domain: &str) -> Value {
    json!({ "domain": domain, "startTimeFilter": { "oldestDate": EPOCH_SECONDS } })
}

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

/// The `startTimestamp`s a listing reported, in the order it reported them.
fn start_timestamps(case: &str, body: &Value) -> Vec<f64> {
    infos(case, body)
        .iter()
        .map(|info| {
            info.pointer("/startTimestamp")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected a startTimestamp in {info:?}"))
        })
        .collect()
}

/// Sorted, so a set of expected matches can be compared without fixing an order.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
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

/// The domain every filter and paging case lists over: three workflow types and the
/// four runs of [`SEEDED_RUNS`], the last of which is then terminated. Returns
/// `workflowId -> runId` for all four.
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

    emulator
        .ok(
            domain,
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": CLOSED_RUN }),
        )
        .await;
    run_ids
}

// ---------------------------------------------------------------------------
// Which stored executions a listing selects
// ---------------------------------------------------------------------------

/// One listing, varying which of the mutually exclusive filters it names and with what
/// value — what "the filter the request names selects from the stored open runs"
/// generalises over.
struct FilterCase {
    name: &'static str,
    filter: Value,
    expected: &'static [&'static str],
}

#[tokio::test]
async fn each_visibility_filter_selects_from_the_stored_open_executions() {
    let domain = "filters";
    let cases = [
        FilterCase {
            name: "no filter at all",
            filter: json!({}),
            expected: OPEN_RUNS,
        },
        FilterCase {
            name: "executionFilter on an open run",
            filter: json!({ "executionFilter": { "workflowId": "wf-orders-a" } }),
            expected: &["wf-orders-a"],
        },
        FilterCase {
            name: "executionFilter on the closed run",
            filter: json!({ "executionFilter": { "workflowId": CLOSED_RUN } }),
            expected: &[],
        },
        FilterCase {
            name: "executionFilter on a workflowId nothing started",
            filter: json!({ "executionFilter": { "workflowId": "wf-never-started" } }),
            expected: &[],
        },
        FilterCase {
            // `WorkflowTypeFilter.version` is optional (`VersionOptional`, length
            // 0..64), so a name-only filter keeps every version of that name.
            name: "typeFilter naming only the type",
            filter: json!({ "typeFilter": { "name": "orders" } }),
            expected: &["wf-orders-a", "wf-orders-b"],
        },
        FilterCase {
            name: "typeFilter naming one version",
            filter: json!({ "typeFilter": { "name": "orders", "version": "2.0" } }),
            expected: &["wf-orders-b"],
        },
        FilterCase {
            name: "typeFilter naming the other version",
            filter: json!({ "typeFilter": { "name": "orders", "version": "1.0" } }),
            expected: &["wf-orders-a"],
        },
        FilterCase {
            // The empty string is inside `VersionOptional`'s 0..64 length, so it is on
            // the wire as "no version given".
            name: "typeFilter with an empty version",
            filter: json!({ "typeFilter": { "name": "orders", "version": "" } }),
            expected: &["wf-orders-a", "wf-orders-b"],
        },
        FilterCase {
            name: "typeFilter naming a version nothing runs",
            filter: json!({ "typeFilter": { "name": "orders", "version": "9.9" } }),
            expected: &[],
        },
        FilterCase {
            name: "typeFilter naming the other type",
            filter: json!({ "typeFilter": { "name": "reports" } }),
            expected: &["wf-reports"],
        },
        FilterCase {
            name: "typeFilter naming a type nothing is registered under",
            filter: json!({ "typeFilter": { "name": "invoices" } }),
            expected: &[],
        },
        FilterCase {
            name: "tagFilter on a tag two runs carry",
            filter: json!({ "tagFilter": { "tag": "shared" } }),
            expected: &["wf-orders-a", "wf-orders-b"],
        },
        FilterCase {
            name: "tagFilter on a tag one run carries",
            filter: json!({ "tagFilter": { "tag": "red" } }),
            expected: &["wf-orders-a"],
        },
        FilterCase {
            name: "tagFilter on a tag no run carries",
            filter: json!({ "tagFilter": { "tag": "green" } }),
            expected: &[],
        },
    ];

    let emulator = Emulator::new();
    let run_ids = seed(&emulator, domain).await;

    for case in &cases {
        let mut body = list_all(domain);
        merge(&mut body, case.filter.clone());
        let listed = emulator
            .ok(case.name, "ListOpenWorkflowExecutions", &body)
            .await;

        let listed_ids = workflow_ids(case.name, &listed);
        let expected: Vec<String> = case.expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            sorted(&listed_ids),
            sorted(&expected),
            "case {:?}: the listing must report exactly the stored OPEN runs the filter \
             selects, got {listed:?}",
            case.name
        );
        assert!(
            listed.get("nextPageToken").is_none(),
            "case {:?}: every match fits in one page here, so no nextPageToken may be \
             issued, got {listed:?}",
            case.name
        );

        for info in infos(case.name, &listed) {
            let workflow_id = string_at(&info, "/execution/workflowId");
            let run_id = run_ids.get(&workflow_id).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing reported {workflow_id:?}, which no start created",
                    case.name
                )
            });
            assert_eq!(
                string_at(&info, "/executionStatus"),
                "OPEN",
                "case {:?}: only OPEN runs may be listed, got {info:?}",
                case.name
            );
            let described = emulator
                .ok(
                    case.name,
                    "DescribeWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "execution": { "workflowId": workflow_id, "runId": run_id },
                    }),
                )
                .await;
            assert_eq!(
                described.pointer("/executionInfo"),
                Some(&info),
                "case {:?}: the listing must report the one stored record the \
                 per-execution reader reports, got {info:?} against {described:?}",
                case.name
            );
        }
    }

    // The listings above changed nothing: the closed run is still CLOSED and the open
    // ones are still OPEN, so reading is not what moves a status.
    for (workflow_id, run_id) in &run_ids {
        let described = emulator
            .ok(
                workflow_id,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": workflow_id, "runId": run_id },
                }),
            )
            .await;
        let expected_status = if workflow_id == CLOSED_RUN {
            "CLOSED"
        } else {
            "OPEN"
        };
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            expected_status,
            "case {workflow_id:?}: listing open executions must not change a stored \
             status, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The start-time window
// ---------------------------------------------------------------------------

/// One listing, varying which end of the `startTimeFilter` window is bounded and
/// where it sits relative to the stored start instants.
struct WindowCase {
    name: &'static str,
    /// Built from the late run's whole second, which the harness only knows at runtime.
    window: Value,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn the_start_time_window_selects_which_stored_runs_are_listed() {
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

    for workflow_id in ["wf-early-a", "wf-early-b"] {
        start_run(&emulator, domain, domain, workflow_id, ("orders", "1.0"), &[]).await;
    }
    // The bounds below are whole seconds, so the late run is started at least 1.1 s
    // after the early ones: its own second is then strictly later than their start
    // instants, which is what makes each case's expectation exact rather than racy.
    std::thread::sleep(Duration::from_millis(1_100));
    let late_run = start_run(&emulator, domain, domain, "wf-late", ("orders", "1.0"), &[]).await;

    let described = emulator
        .ok(
            "late run",
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": "wf-late", "runId": late_run },
            }),
        )
        .await;
    let late_start = described
        .pointer("/executionInfo/startTimestamp")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case \"late run\": expected a startTimestamp in {described:?}"));
    // `late_second <= late_start`, and every early run started more than 0.1 s before
    // `late_second`, so `late_second` includes only the late run and
    // `late_second - 0.1` includes only the early ones.
    let late_second = late_start.floor();
    let future = now_seconds() + 3_600.0;

    let cases = [
        WindowCase {
            name: "oldest bound only",
            window: json!({ "oldestDate": EPOCH_SECONDS }),
            expected: vec!["wf-early-a", "wf-early-b", "wf-late"],
        },
        WindowCase {
            name: "both bounds, spanning every run",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": future }),
            expected: vec!["wf-early-a", "wf-early-b", "wf-late"],
        },
        WindowCase {
            name: "oldest bound at the late run's own second",
            window: json!({ "oldestDate": late_second }),
            expected: vec!["wf-late"],
        },
        WindowCase {
            name: "oldest bound at the late run's second, latest unbounded upward",
            window: json!({ "oldestDate": late_second, "latestDate": future }),
            expected: vec!["wf-late"],
        },
        WindowCase {
            name: "latest bound before the late run",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": late_second - 0.1 }),
            expected: vec!["wf-early-a", "wf-early-b"],
        },
        WindowCase {
            name: "window entirely in the future",
            window: json!({ "oldestDate": future }),
            expected: vec![],
        },
        WindowCase {
            name: "window entirely in the past",
            window: json!({ "oldestDate": EPOCH_SECONDS, "latestDate": 1.0 }),
            expected: vec![],
        },
        WindowCase {
            name: "empty window: latest before oldest",
            window: json!({ "oldestDate": future, "latestDate": EPOCH_SECONDS }),
            expected: vec![],
        },
    ];

    for case in &cases {
        let body = json!({ "domain": domain, "startTimeFilter": case.window });
        let listed = emulator
            .ok(case.name, "ListOpenWorkflowExecutions", &body)
            .await;
        let expected: Vec<String> = case.expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            sorted(&workflow_ids(case.name, &listed)),
            sorted(&expected),
            "case {:?}: only runs whose stored start instant is inside the window may be \
             listed, got {listed:?}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// Ordering and paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ordering_and_paging_walk_the_same_stored_open_executions() {
    let domain = "paging";
    let emulator = Emulator::new();
    let mut run_ids = seed(&emulator, domain).await;
    // A fourth open run, so a page size of 2 or 3 leaves a remainder of a different
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
    run_ids.insert("wf-extra".to_string(), extra);
    let mut open: Vec<String> = OPEN_RUNS.iter().map(|id| (*id).to_string()).collect();
    open.push("wf-extra".to_string());

    let mut whole_listing: BTreeMap<bool, Vec<String>> = BTreeMap::new();
    for reverse_order in [false, true] {
        let case = format!("reverseOrder={reverse_order}");
        let mut body = list_all(domain);
        merge(&mut body, json!({ "reverseOrder": reverse_order }));
        let listed = emulator
            .ok(&case, "ListOpenWorkflowExecutions", &body)
            .await;

        let ids = workflow_ids(&case, &listed);
        assert_eq!(
            sorted(&ids),
            sorted(&open),
            "case {case:?}: an unpaged listing must report every stored OPEN run, got {listed:?}"
        );

        // "By default the results are returned in descending order of the start time of
        // the executions"; `reverseOrder` returns them ascending. Equal instants are
        // allowed, so the check is monotonicity, not strict ordering.
        let timestamps = start_timestamps(&case, &listed);
        for pair in timestamps.windows(2) {
            let ordered = if reverse_order {
                pair[0] <= pair[1]
            } else {
                pair[0] >= pair[1]
            };
            assert!(
                ordered,
                "case {case:?}: start timestamps must be ordered by the requested \
                 direction, got {timestamps:?}"
            );
        }
        whole_listing.insert(reverse_order, ids);
    }

    let descending = whole_listing[&false].clone();
    let ascending = whole_listing[&true].clone();
    let mut reversed = ascending.clone();
    reversed.reverse();
    assert_eq!(
        descending, reversed,
        "the two orderings must walk one total order in opposite directions, got \
         {descending:?} against {ascending:?}"
    );

    // Paging walks exactly that order, whatever the page size: no run repeated, none
    // skipped, and the token absent only on the last page.
    for reverse_order in [false, true] {
        for page_size in [1_usize, 2, 3, 4, 1_000] {
            let case = format!("reverseOrder={reverse_order}, maximumPageSize={page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut body = list_all(domain);
                merge(
                    &mut body,
                    json!({ "reverseOrder": reverse_order, "maximumPageSize": page_size }),
                );
                if let Some(token) = &token {
                    merge(&mut body, json!({ "nextPageToken": token }));
                }
                let listed = emulator
                    .ok(&case, "ListOpenWorkflowExecutions", &body)
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
                    pages <= open.len() + 1,
                    "case {case:?}: paging must terminate, got {pages} pages for {} runs",
                    open.len()
                );
                if token.is_none() {
                    break;
                }
            }
            let expected = if reverse_order { &ascending } else { &descending };
            assert_eq!(
                &walked, expected,
                "case {case:?}: paging must walk the unpaged order exactly once, got {walked:?}"
            );
        }
    }

    // A token is issued for ONE ordering: replayed against the other it is refused,
    // rather than silently skipping or repeating runs.
    let mut first_page = list_all(domain);
    merge(&mut first_page, json!({ "maximumPageSize": 1 }));
    let listed = emulator
        .ok("cross-ordering token", "ListOpenWorkflowExecutions", &first_page)
        .await;
    let token = string_at(&listed, "/nextPageToken");
    let mut replayed = list_all(domain);
    merge(
        &mut replayed,
        json!({ "maximumPageSize": 1, "reverseOrder": true, "nextPageToken": token }),
    );
    emulator
        .refused(
            "cross-ordering token",
            "ListOpenWorkflowExecutions",
            &replayed,
            "ValidationException",
        )
        .await;
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
            "ListOpenWorkflowExecutions",
            &list_all("never-registered"),
            "UnknownResourceFault",
        )
        .await;

    // "executionFilter, typeFilter and tagFilter are mutually exclusive. You can
    // specify at most one of these in a request." Every pair, and all three.
    let exclusive = [
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
            "all three filters",
            json!({
                "executionFilter": { "workflowId": "wf-orders-a" },
                "typeFilter": { "name": "orders" },
                "tagFilter": { "tag": "shared" },
            }),
        ),
    ];
    for (case, filters) in exclusive {
        let mut body = list_all(domain);
        merge(&mut body, filters);
        emulator
            .refused(case, "ListOpenWorkflowExecutions", &body, "ValidationException")
            .await;
    }

    // A `nextPageToken` this service never issued addresses no position in the order.
    for (case, token) in [
        ("token the service never issued", "not-a-token"),
        ("token with an unknown ordering tag", "x6162"),
        ("token that is not hex", "dzzzz"),
    ] {
        let mut body = list_all(domain);
        merge(&mut body, json!({ "nextPageToken": token }));
        emulator
            .refused(case, "ListOpenWorkflowExecutions", &body, "ValidationException")
            .await;
    }

    // An empty `nextPageToken` is inside the modelled `PageToken` length (0..2048) and
    // means "no token", so it lists the first page rather than being refused.
    let mut body = list_all(domain);
    merge(&mut body, json!({ "nextPageToken": "" }));
    let listed = emulator
        .ok("empty token", "ListOpenWorkflowExecutions", &body)
        .await;
    assert_eq!(
        workflow_ids("empty token", &listed).len(),
        OPEN_RUNS.len(),
        "case \"empty token\": an empty nextPageToken means no token, so every open run \
         is listed, got {listed:?}"
    );
}

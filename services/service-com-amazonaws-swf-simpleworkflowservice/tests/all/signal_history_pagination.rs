// SPDX-License-Identifier: CC0-1.0

//! `SignalWorkflowExecution` feeding `GetWorkflowExecutionHistory`, end to end over the
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: a signal to an open run APPENDS to that run's
//! persisted event log — one `WorkflowExecutionSignaled` event per call, plus a
//! `DecisionTaskScheduled` only while the run has no decision task outstanding — and the
//! log that grows is read back by `GetWorkflowExecutionHistory` as a stable cursor over
//! EVENTS: whatever its length, every `maximumPageSize` and both orderings concatenate
//! to exactly the stored log (reversed under `reverseOrder`, which is the forward reading
//! reversed event for event), and re-presenting a `nextPageToken` returns the same page
//! again rather than advancing or shifting.
//!
//! Rows vary what the rule generalises over: how many signals grew the log (so the log
//! length varies, including the ungrown log), the page size relative to that length
//! (one, a few, one short of it, exactly it, one past it) and the reading direction.
//! `maximumPageSize` is modelled `PageSize` with range 0..1000 and `nextPageToken` is
//! modelled `PageToken` with length 0..2048 — both from `smithy_query(query="operation",
//! operation="GetWorkflowExecutionHistory")` — so the sizes below stay inside the range
//! and every issued token is checked against the token length.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    full_defaults, i64_at, merge, seeded, string_at, Emulator,
};

/// The workflow type every run here is started from.
const TYPE: (&str, &str) = ("demo-wf", "1.0");

/// The task list that type registers as its `defaultTaskList`.
const TASK_LIST: &str = "demo-tasks";

/// The modelled `PageToken` maximum length.
const MAX_PAGE_TOKEN_LEN: usize = 2048;

/// How many events `StartWorkflowExecution` leaves in a fresh run's history
/// (`WorkflowExecutionStarted`, then the first `DecisionTaskScheduled`).
const EVENTS_AT_START: usize = 2;

/// One history, identified by how many signals grew it.
struct HistoryCase {
    name: &'static str,
    signals: usize,
}

/// Read one page of a run's history.
async fn page(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    run_id: &str,
    reverse_order: bool,
    page_size: usize,
    token: Option<&str>,
) -> Value {
    let mut body = json!({
        "domain": domain,
        "execution": { "workflowId": "wf-1", "runId": run_id },
        "maximumPageSize": page_size,
        "reverseOrder": reverse_order,
    });
    if let Some(token) = token {
        merge(&mut body, json!({ "nextPageToken": token }));
    }
    emulator
        .ok(case, "GetWorkflowExecutionHistory", &body)
        .await
}

/// The `events` array of one answer.
fn events_of(case: &str, answer: &Value) -> Vec<Value> {
    answer
        .pointer("/events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected events in {answer:?}"))
}

/// Walk every page of one reading and return the events in the order they arrived.
async fn walk(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    run_id: &str,
    reverse_order: bool,
    page_size: usize,
    expected_len: usize,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let answer = page(
            emulator,
            case,
            domain,
            run_id,
            reverse_order,
            page_size,
            token.as_deref(),
        )
        .await;
        let events = events_of(case, &answer);
        assert!(
            events.len() <= page_size,
            "case {case:?}: a page must not exceed maximumPageSize {page_size}, got {answer:?}"
        );
        seen.extend(events);
        token = answer
            .pointer("/nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &token {
            assert!(
                !token.is_empty() && token.len() <= MAX_PAGE_TOKEN_LEN,
                "case {case:?}: an issued nextPageToken stays inside the modelled \
                 PageToken length 0..{MAX_PAGE_TOKEN_LEN}, got {} characters",
                token.len()
            );
        }
        pages += 1;
        assert!(
            pages <= expected_len + 2,
            "case {case:?}: paging must terminate, but it produced {pages} pages"
        );
        if token.is_none() {
            break;
        }
    }
    seen
}

/// Start one run of the seeded type and return its `runId`.
async fn start_run(emulator: &Emulator, case: &str, domain: &str) -> String {
    let started = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-1",
                "workflowType": { "name": TYPE.0, "version": TYPE.1 },
            }),
        )
        .await;
    string_at(&started, "/runId")
}

/// The event ids of `events`.
fn ids(events: &[Value]) -> Vec<i64> {
    events
        .iter()
        .map(|event| i64_at(event, "/eventId"))
        .collect()
}

/// How many events of `events` carry `event_type`.
fn count_of_type(events: &[Value], event_type: &str) -> usize {
    events
        .iter()
        .filter(|event| event.get("eventType").and_then(Value::as_str) == Some(event_type))
        .count()
}

#[tokio::test]
async fn signals_grow_a_history_that_pages_identically_in_either_order() {
    let cases = [
        HistoryCase {
            name: "no signal at all",
            signals: 0,
        },
        HistoryCase {
            name: "one signal",
            signals: 1,
        },
        HistoryCase {
            name: "two signals",
            signals: 2,
        },
        HistoryCase {
            name: "eleven signals",
            signals: 11,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("signal-paging-{index}");
        seeded(
            &emulator,
            case.name,
            &domain,
            TYPE,
            full_defaults(TASK_LIST, "TERMINATE"),
        )
        .await;
        let run_id = start_run(&emulator, case.name, &domain).await;

        for signal in 0..case.signals {
            emulator
                .ok(
                    case.name,
                    "SignalWorkflowExecution",
                    &json!({
                        "domain": domain,
                        "workflowId": "wf-1",
                        "signalName": format!("signal-{signal}"),
                        "input": format!("payload {signal}"),
                    }),
                )
                .await;
        }

        // The whole stored log, read in one page: the reference every paged reading
        // below is compared against. `maximumPageSize` 0 is the member's own default
        // ("not specified"), which reads the history up to the modelled maximum.
        let whole = events_of(
            case.name,
            &page(&emulator, case.name, &domain, &run_id, false, 0, None).await,
        );
        let expected_len = EVENTS_AT_START + case.signals;
        assert_eq!(
            whole.len(),
            expected_len,
            "case {:?}: each of the {} signals appends exactly one event to the log the \
             start left, got {:?}",
            case.name,
            case.signals,
            ids(&whole)
        );
        assert_eq!(
            count_of_type(&whole, "WorkflowExecutionSignaled"),
            case.signals,
            "case {:?}: every signal is recorded once, got {whole:?}",
            case.name
        );
        // A run with a decision task already outstanding does not get a second one,
        // however many signals arrive while it is in flight.
        assert_eq!(
            count_of_type(&whole, "DecisionTaskScheduled"),
            1,
            "case {:?}: signals arriving while a decision task is outstanding schedule \
             no further task, got {whole:?}",
            case.name
        );
        let forward_ids = ids(&whole);
        assert!(
            forward_ids.windows(2).all(|pair| pair[0] < pair[1]),
            "case {:?}: the stored log reads oldest-first with strictly increasing \
             eventIds, got {forward_ids:?}",
            case.name
        );

        // The reverse reading is the forward one reversed, event for event — not merely
        // the same ids in the other order.
        let reversed: Vec<Value> = whole.iter().rev().cloned().collect();
        let reverse_whole = events_of(
            case.name,
            &page(&emulator, case.name, &domain, &run_id, true, 0, None).await,
        );
        assert_eq!(
            reverse_whole, reversed,
            "case {:?}: reverseOrder returns the forward reading reversed, got {:?} \
             against {:?}",
            case.name,
            ids(&reverse_whole),
            ids(&reversed)
        );

        // Page sizes spanning the log: one event at a time, a few at a time, one short
        // of the log, exactly the log, and one past it.
        let mut sizes = vec![
            1,
            2,
            3,
            expected_len.saturating_sub(1),
            expected_len,
            expected_len + 1,
        ];
        sizes.retain(|size| *size >= 1);
        sizes.sort_unstable();
        sizes.dedup();
        for reverse_order in [false, true] {
            let expected = if reverse_order { &reversed } else { &whole };
            for size in &sizes {
                let label = format!(
                    "{} / reverseOrder={reverse_order}, maximumPageSize={size}",
                    case.name
                );
                let seen = walk(
                    &emulator,
                    &label,
                    &domain,
                    &run_id,
                    reverse_order,
                    *size,
                    expected_len,
                )
                .await;
                assert_eq!(
                    &seen,
                    expected,
                    "case {label:?}: the pages concatenate to the whole ordering, got {:?} \
                     against {:?}",
                    ids(&seen),
                    ids(expected)
                );
            }
        }
    }
}

#[tokio::test]
async fn re_presenting_a_history_page_token_returns_the_same_page_again() {
    // A token names the EVENT the previous page ended on, so it is a stable address:
    // reading it twice is the same page twice. The rows vary the direction and the page
    // size the token was issued at.
    let emulator = Emulator::new();
    let domain = "signal-token-stability";
    seeded(
        &emulator,
        "token stability",
        domain,
        TYPE,
        full_defaults(TASK_LIST, "TERMINATE"),
    )
    .await;
    let run_id = start_run(&emulator, "token stability", domain).await;
    for signal in 0..4 {
        emulator
            .ok(
                "token stability",
                "SignalWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": "wf-1",
                    "signalName": format!("signal-{signal}"),
                }),
            )
            .await;
    }

    for reverse_order in [false, true] {
        for size in [1usize, 2, 3] {
            let label = format!("reverseOrder={reverse_order}, maximumPageSize={size}");
            let first =
                page(&emulator, &label, domain, &run_id, reverse_order, size, None).await;
            let token = string_at(&first, "/nextPageToken");

            // The same token, read three times over: every read is the same page, with
            // the same follow-on token.
            let mut answers: Vec<Value> = Vec::new();
            for _ in 0..3 {
                answers.push(
                    page(
                        &emulator,
                        &label,
                        domain,
                        &run_id,
                        reverse_order,
                        size,
                        Some(&token),
                    )
                    .await,
                );
            }
            for (attempt, answer) in answers.iter().enumerate() {
                assert_eq!(
                    answer, &answers[0],
                    "case {label:?}: read {attempt} of the same token differs from the \
                     first, got {answer:?} against {:?}",
                    answers[0]
                );
            }
            let resumed = events_of(&label, &answers[0]);
            let first_page = events_of(&label, &first);
            assert!(
                !resumed.is_empty(),
                "case {label:?}: a token was issued, so the page it addresses holds \
                 events, got {:?}",
                answers[0]
            );
            assert!(
                resumed.iter().all(|event| !first_page.contains(event)),
                "case {label:?}: resuming hands back no event the first page already \
                 held, got {:?} after {:?}",
                ids(&resumed),
                ids(&first_page)
            );
        }
    }
}

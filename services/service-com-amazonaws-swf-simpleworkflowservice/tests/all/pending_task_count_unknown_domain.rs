// SPDX-License-Identifier: CC0-1.0

//! The pending-task counters, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: `CountPendingActivityTasks` and
//! `CountPendingDecisionTasks` report how many tasks are WAITING in the named task list
//! and answer the documented zero whenever none is ("If you specify a task list that no
//! activity task was ever scheduled in then `0` is returned") — a task list is not a
//! registered resource, so a name nothing was ever scheduled under holds no task
//! whether the enclosing domain holds no such list or no record of that domain exists at
//! all. Counting is a read: it creates no domain and no task list.
//!
//! Rows vary what the rule generalises over: which of the two counters is asked, the
//! shape and size of the domain name (plain, case-differing from a stored one, a prefix
//! of a stored one, punctuated, non-ASCII, at the modelled 256-character maximum) and of
//! the task-list name, and whether the list is empty because its domain holds nothing or
//! because the domain is not there at all. A non-zero control keeps the zero from being
//! a constant.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{full_defaults, seeded, Emulator};

/// The two counters the rule covers: both resolve a `(domain, taskList)` pair the same
/// way, so both must answer a list that holds nothing the same way.
const COUNTERS: [&str; 2] = ["CountPendingActivityTasks", "CountPendingDecisionTasks"];

/// The workflow type the control run is started with.
const WORKFLOW_TYPE: (&str, &str) = ("counted-wf", "1.0");

/// Count one `(domain, taskList)` pair with `operation`, requiring a 200, and return
/// `(count, truncated)`.
async fn counted(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    task_list: &str,
) -> (i64, bool) {
    let body = emulator
        .ok(
            case,
            operation,
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await;
    (
        body.get("count")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a count in {body:?}")),
        body.get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

#[tokio::test]
async fn a_task_list_that_holds_nothing_counts_zero_whatever_domain_it_is_named_in() {
    let emulator = Emulator::new();
    // One registered domain, so a row whose name only DIFFERS from a stored one (case,
    // suffix, prefix) is checked against a store that is not empty.
    let stored = "counted-domain";
    seeded(
        &emulator,
        "stored",
        stored,
        WORKFLOW_TYPE,
        full_defaults("stored-list", "TERMINATE"),
    )
    .await;

    let long_domain = "d".repeat(256);
    let long_list = "l".repeat(256);
    // `(case, domain, taskList)`: each pair names a list nothing was ever scheduled in.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a domain nothing registered", "never-registered", "any-list"),
        ("the stored name in another case", "COUNTED-DOMAIN", "any-list"),
        ("a suffix of the stored name", "counted-domain-2", "any-list"),
        ("a prefix of the stored name", "counted", "any-list"),
        ("a punctuated unknown domain", "home.domain_1", "queue.1-a"),
        ("a non-ASCII unknown domain", "注文ドメイン", "タスク一覧"),
        (
            "an unknown domain at the modelled name maximum",
            long_domain.as_str(),
            long_list.as_str(),
        ),
        // The same answer from the other side of the rule: the domain IS stored and the
        // list is simply one nothing was scheduled in.
        ("a stored domain with an unused list", stored, "unused-list"),
        ("a stored domain with a non-ASCII list", stored, "未使用の一覧"),
    ];

    for (case, domain, task_list) in &rows {
        for operation in COUNTERS {
            let label = format!("{case} / {operation}");
            let (count, truncated) = counted(&emulator, &label, operation, domain, task_list).await;
            assert_eq!(
                (count, truncated),
                (0, false),
                "case {label:?}: a list nothing was ever scheduled in holds no task, got \
                 count {count} truncated {truncated}"
            );
        }
    }

    // Counting created nothing: every unknown domain above is still unknown to the
    // readers that DO resolve a domain.
    for (case, domain, _) in rows.iter().filter(|(_, domain, _)| *domain != stored) {
        emulator
            .refused(
                case,
                "DescribeDomain",
                &json!({ "name": domain }),
                "UnknownResourceFault",
            )
            .await;
        emulator
            .refused(
                case,
                "DeprecateDomain",
                &json!({ "name": domain }),
                "UnknownResourceFault",
            )
            .await;
    }
}

#[tokio::test]
async fn the_counters_still_report_what_is_waiting() {
    // The control: the zero above is the count of what is waiting, not a constant. One
    // started run leaves exactly one decision task waiting in its list, and the activity
    // counter reports that same list as empty of activity tasks.
    let emulator = Emulator::new();
    let domain = "counted-live";
    let list = "live-deciders";
    seeded(
        &emulator,
        "live",
        domain,
        WORKFLOW_TYPE,
        full_defaults(list, "TERMINATE"),
    )
    .await;
    emulator
        .ok(
            "live",
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": "wf-live",
                "workflowType": { "name": WORKFLOW_TYPE.0, "version": WORKFLOW_TYPE.1 },
            }),
        )
        .await;

    for (operation, expected) in [
        ("CountPendingDecisionTasks", 1),
        ("CountPendingActivityTasks", 0),
    ] {
        let (count, truncated) = counted(&emulator, operation, operation, domain, list).await;
        assert_eq!(
            (count, truncated),
            (expected, false),
            "case {operation:?}: the counter reports the tasks of that kind waiting in \
             {list:?}, got count {count}"
        );
    }
}

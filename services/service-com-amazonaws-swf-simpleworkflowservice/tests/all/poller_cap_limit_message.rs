// SPDX-License-Identifier: CC0-1.0

//! The wording of the `LimitExceededFault` a poll offered past the per-task-list poller
//! cap is refused with, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `decision_task_poller_cap` already builds.
//!
//! The rule every case here asserts: the concurrent-poller quota's refusal is reported
//! with ONE fixed sentence — `Limit on maximum concurrent poll requests per task list
//! reached` — so the message is the same text for every such refusal and carries nothing
//! of the request or of the store: no task list name, no domain name, no identity, and no
//! number at all (least of all the count of pollers a list admits, which the service never
//! reports to a caller).
//!
//! Rows vary what the rule generalises over: which polling operation is refused (both
//! kinds of task are counted and refused by the same rule), the shape of the task list
//! name (plain, punctuated, non-ASCII, the longest the model admits), the shape of the
//! domain name, whether an `identity` is sent and how long it is, and the paging members
//! a decision poll may carry. `decision_task_poller_cap` fills the cap with a thousand
//! genuine concurrent long polls and asserts the same message on the refusal they
//! produce; here the admission record is seeded in the store so one suite can cover the
//! whole class without holding thousands of connections open.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::state::{TaskKind, MAX_POLLERS_PER_TASK_LIST};

use crate::decision_task_poller_cap::{seed, Harness};
use crate::workflow_execution_lifecycle::{error_code, merge};

/// The message SWF reports this condition with, as recorded from the service. Fixed text:
/// it has no request-derived or state-derived part.
const EXPECTED_MESSAGE: &str = "Limit on maximum concurrent poll requests per task list reached";

/// Both polling operations, each with the kind of poller its open calls are counted as.
/// The cap is per `(task list, kind)`, and either operation is refused by it.
const POLL_OPERATIONS: [(&str, TaskKind); 2] = [
    ("PollForDecisionTask", TaskKind::Decision),
    ("PollForActivityTask", TaskKind::Activity),
];

/// How long a poll admitted below the cap is given to prove it is NOT being refused. A
/// poll of an empty task list holds the connection open for the documented 60 seconds.
const HOLD_OPEN_PROBE: Duration = Duration::from_millis(250);

/// How long a refusal that must come promptly is given before the test fails.
const PROMPT: Duration = Duration::from_secs(5);

/// One shape of task list a poll can name. `TaskList$name` is modelled `length 1..256`
/// and must obey SWF's naming rule (no leading or trailing whitespace, no `:`, `/`, `|`
/// or control character, never the literal `arn`), so every shape below is a name the
/// service accepts.
struct ListShape {
    case: &'static str,
    task_list: String,
}

fn list_shapes() -> Vec<ListShape> {
    vec![
        ListShape {
            case: "a plain name",
            task_list: "tasks".to_string(),
        },
        ListShape {
            case: "a punctuated name",
            task_list: "orders.tasks-2_v1#a".to_string(),
        },
        ListShape {
            case: "a non-ASCII name",
            task_list: "注文タスク一覧".to_string(),
        },
        ListShape {
            // The longest name the modelled length admits.
            case: "the longest name the model admits",
            task_list: "l".repeat(256),
        },
        ListShape {
            // A name carrying the digits of the cap, so "the message reports no number"
            // cannot pass by the name being absent for another reason.
            case: "a name carrying the cap's digits",
            task_list: format!("list-{MAX_POLLERS_PER_TASK_LIST}-pollers"),
        },
    ]
}

/// One optional `identity`, the only other member a poll of either kind carries that
/// comes from the caller. `Identity` is modelled `length 0..256`.
fn identities() -> Vec<(&'static str, Option<String>)> {
    vec![
        ("no identity", None),
        ("a named identity", Some("poller-7".to_string())),
        ("the longest identity the model admits", Some("i".repeat(256))),
    ]
}

/// Count `count` open pollers of `kind` on `(domain, task_list)` in the store — exactly
/// the admission record an open long poll makes — so a row reaches the cap without
/// holding that many connections open.
fn fill_pollers(harness: &Harness, domain: &str, kind: TaskKind, task_list: &str, count: usize) {
    let state = harness.state();
    let mut guard = state.lock();
    let record = guard
        .domains
        .get_mut(domain)
        .unwrap_or_else(|| panic!("domain {domain:?} was registered"));
    for index in 0..count {
        assert!(
            record.admit_poller(kind, task_list),
            "case {domain:?}/{task_list:?}: poller {index} of {count} is admitted below the cap"
        );
    }
}

/// How many pollers of `kind` the store counts on `(domain, task_list)`.
fn open_pollers(harness: &Harness, domain: &str, kind: TaskKind, task_list: &str) -> usize {
    let state = harness.state();
    let guard = state.lock();
    guard
        .domains
        .get(domain)
        .map_or(0, |record| record.open_pollers(kind, task_list))
}

/// The poll request for `operation` on one task list, with the row's optional identity
/// and whatever extra members the row sends.
fn poll_body(domain: &str, task_list: &str, identity: Option<&str>, extra: Value) -> Value {
    let mut body = json!({ "domain": domain, "taskList": { "name": task_list } });
    if let Some(identity) = identity {
        merge(&mut body, json!({ "identity": identity }));
    }
    merge(&mut body, extra);
    body
}

/// Call `operation`, require the over-quota refusal, and return its message.
async fn refused_message(harness: &Harness, case: &str, operation: &str, body: &Value) -> String {
    let (status, parsed) = tokio::time::timeout(PROMPT, harness.call(operation, body))
        .await
        .unwrap_or_else(|_| {
            panic!("case {case:?}: an over-cap poll is refused promptly, not held open")
        });
    let code = error_code(&parsed);
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededFault"),
        "case {case:?}: expected {operation} to answer 400 LimitExceededFault (never \
         OperationNotPermittedFault), got {status} {code} with {parsed:?}"
    );
    parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed:?}"))
        .to_string()
}

#[tokio::test]
async fn every_over_cap_poll_refusal_carries_the_same_fixed_message() {
    let harness = Harness::new();
    // Two domain-name shapes, so a message that leaked the domain would differ between
    // them; neither name shares a substring with any task list name below.
    let domains = ["quota.domain_1", "second-domain"];
    for domain in domains {
        seed(&harness, domain, domain).await;
    }

    let shapes = list_shapes();
    let identities = identities();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (domain_index, domain) in domains.iter().enumerate() {
        for (operation_index, (operation, kind)) in POLL_OPERATIONS.iter().enumerate() {
            for (shape_index, shape) in shapes.iter().enumerate() {
                let (identity_case, identity) =
                    &identities[(shape_index + operation_index + domain_index) % identities.len()];
                // A decision poll may also carry paging members; an activity poll takes
                // neither, so only the decision rows vary them.
                let extra = if *operation == "PollForDecisionTask" && shape_index % 2 == 0 {
                    json!({ "maximumPageSize": 10, "reverseOrder": true })
                } else {
                    json!({})
                };

                fill_pollers(
                    &harness,
                    domain,
                    *kind,
                    &shape.task_list,
                    MAX_POLLERS_PER_TASK_LIST,
                );
                let case = format!("{operation} / {domain} / {} / {identity_case}", shape.case);
                let body = poll_body(domain, &shape.task_list, identity.as_deref(), extra);
                let message = refused_message(&harness, &case, operation, &body).await;

                assert_eq!(
                    message, EXPECTED_MESSAGE,
                    "case {case:?}: the over-quota refusal carries this condition's fixed \
                     message, got {message:?}"
                );
                assert!(
                    !message.contains(&shape.task_list),
                    "case {case:?}: the task list is not part of the message, got {message:?}"
                );
                assert!(
                    !message.contains(domain),
                    "case {case:?}: the domain is not part of the message, got {message:?}"
                );
                if let Some(identity) = identity {
                    assert!(
                        !message.contains(identity.as_str()),
                        "case {case:?}: the identity is not part of the message, got {message:?}"
                    );
                }
                assert!(
                    !message.chars().any(|c| c.is_ascii_digit()),
                    "case {case:?}: the message reports no number, least of all how many \
                     pollers a list admits, got {message:?}"
                );
                seen.insert(message);
            }
        }
    }

    assert_eq!(
        seen.iter().map(String::as_str).collect::<Vec<_>>(),
        vec![EXPECTED_MESSAGE],
        "every over-quota refusal carries the identical text, got {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cap_itself_is_what_produces_the_message() {
    // The message belongs to the admission refusal and to nothing else: one slot below
    // the cap the same request is admitted and held open, and it is the poll that fills
    // the list which turns the next one into the refusal.
    let harness = Arc::new(Harness::new());
    let domain = "cap-boundary";
    seed(&harness, domain, domain).await;

    for (operation, kind) in POLL_OPERATIONS {
        let task_list = format!("{operation}-boundary-list");
        fill_pollers(
            &harness,
            domain,
            kind,
            &task_list,
            MAX_POLLERS_PER_TASK_LIST - 1,
        );

        let body = poll_body(domain, &task_list, Some("poller-last"), json!({}));
        let spawned = {
            let harness = Arc::clone(&harness);
            tokio::spawn(async move { harness.call(operation, &body).await })
        };
        tokio::time::sleep(HOLD_OPEN_PROBE).await;
        assert!(
            !spawned.is_finished(),
            "case {operation:?}: the last slot below the cap is admitted and held open, \
             not refused; it answered inside {HOLD_OPEN_PROBE:?}"
        );
        assert_eq!(
            open_pollers(&harness, domain, kind, &task_list),
            MAX_POLLERS_PER_TASK_LIST,
            "case {operation:?}: the admitted poll fills the list to its cap"
        );

        // Now the list is full, so the very same request is refused — with the fixed text.
        let case = format!("{operation} one past the cap");
        let body = poll_body(domain, &task_list, Some("poller-last"), json!({}));
        let message = refused_message(&harness, &case, operation, &body).await;
        assert_eq!(
            message, EXPECTED_MESSAGE,
            "case {case:?}: the refusal the full list produces carries the fixed message, \
             got {message:?}"
        );

        spawned.abort();
    }
}

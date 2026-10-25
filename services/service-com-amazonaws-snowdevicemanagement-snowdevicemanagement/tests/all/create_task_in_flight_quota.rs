// SPDX-License-Identifier: CC0-1.0

//! The task quota `CreateTask` refuses with, end to end over HTTP.
//!
//! The rule under test: a service quota bounds what the Region a request is signed for
//! currently HOLDS, so `CreateTask` owes HTTP 402 `ServiceQuotaExceededException` only
//! while that Region has as many tasks IN FLIGHT as the bound allows. A task in a
//! terminal state consumes none of it — whichever way it got there, the clock carrying
//! it to `COMPLETED` or `CancelTask` moving it to `CANCELED` — and neither does a task
//! of another Region, so the refusal lifts of its own accord and the record of a
//! finished task is still readable.
//!
//! The rows span what the rule generalises over: the terminal state the earlier tasks
//! reached and how they reached it, the Region the create is signed for, and the
//! consumption at the bound, one past it and once it has been released. Every row goes
//! through the router, so an answer that is not computed from the persisted records
//! cannot pass.
//!
//! Filling the bound takes many calls, so this suite does not race the dispatch and run
//! spans while it fills: it pushes the instant the store owes a queued execution its
//! next state out of reach while the tasks have to stay in flight, and brings it back
//! within reach when they have to finish. Those spans elapsing in real time is what
//! `cancel_task_dispatch_window.rs` pins; here they are only the mechanism that makes a
//! task terminal.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    service_state, EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN, IN_FLIGHT_TASK_QUOTA,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::device_task_lifecycle::{assert_not_found, ok, string, Answer, ACCOUNT};

/// The wire error code and status the modelled refusal carries:
/// `ServiceQuotaExceededException`, `@httpError 402`
/// (`smithy_query(query="operation",operation="CreateTask")`).
const QUOTA_CODE: &str = "ServiceQuotaExceededException";
const QUOTA_STATUS: u16 = 402;

/// The two Regions this suite signs for: the one it fills to the bound, and one that
/// holds a device of its own so a create there is not refused by the device
/// precondition.
const FILLED_REGION: &str = "eu-west-1";
const OTHER_REGION: &str = "us-west-2";

/// How many devices each Region has Snow Device Management activated on.
const DEVICES_PER_REGION: u64 = 2;

/// How far out of reach a held execution's due instant is put: long enough that no
/// host finishes filling the bound before it, and the tasks therefore stay
/// `IN_PROGRESS` for as long as a row needs them to.
const HOLD: Duration = Duration::from_secs(3_600);

/// The page `ListTasks` serves at most, as the model's `@range(min: 1, max: 100)` on
/// `maxResults` bounds it.
const PAGE: usize = 100;

/// One emulator: a router carrying only this service over one shared engine, with
/// devices activated in both Regions (the one state seam that puts a device in the
/// inventory, since the API models no device-creation operation).
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let activated = {
        let state =
            service_state(&*engine).expect("the in-memory engine carries this service's state");
        let mut store = state.lock();
        let filled = store.activate_devices(FILLED_REGION, DEVICES_PER_REGION, SystemTime::now());
        store.activate_devices(OTHER_REGION, DEVICES_PER_REGION, SystemTime::now());
        filled
    };
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine, activated)
}

/// One device id this account manages in `region`.
fn a_device_in(engine: &Arc<engine::InMemoryEngine>, region: &str) -> String {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let store = state.lock();
    store
        .devices
        .values()
        .find(|device| device.region == region)
        .map(|device| device.managed_device_id.clone())
        .unwrap_or_else(|| panic!("the emulator activates a device in {region}"))
}

/// Put the instant the store owes every pending execution its next state out of reach,
/// so the tasks already created stay in flight however long the rest of the row takes.
fn hold_pending(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let held = SystemTime::now() + HOLD;
    for execution in store.executions.values_mut() {
        if execution.due.is_some() {
            execution.due = Some(held);
        }
    }
}

/// Bring every pending execution's due instant back within reach, far enough back that
/// every span the store owes them has elapsed: the next call's settle carries them to
/// their terminal state, and their tasks with them.
///
/// A device performs one execution at a time, so the executions this suite stacks on one
/// device run one after another: the instant has to reach back past the WHOLE chain, not
/// just past one execution's spans, or the ones behind the first would still be running.
fn release_pending(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    let pending = u32::try_from(
        store
            .executions
            .values()
            .filter(|execution| execution.due.is_some())
            .count(),
    )
    .expect("a test holds fewer pending executions than u32 counts");
    let chain = (EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN) * (pending + 1);
    let due = SystemTime::now() - chain - Duration::from_millis(50);
    for execution in store.executions.values_mut() {
        if execution.due.is_some() {
            execution.due = Some(due);
        }
    }
}

/// Dispatch one request signed for `region`.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "host",
            format!("snow-device-management.{region}.amazonaws.com"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/\
                 snow-device-management/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `CreateTask` in `region`, aimed at `device`.
async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    device: &str,
    command: Value,
) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/task",
        Some(json!({ "targets": [device], "command": command })),
    )
    .await
}

/// `CancelTask` in `region`.
async fn cancel_in(router: &MultiServiceRouter, region: &str, task_id: &str) -> Answer {
    call_in(
        router,
        region,
        "POST",
        &format!("/task/{task_id}/cancel"),
        None,
    )
    .await
}

/// `DescribeTask` in `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str, task_id: &str) -> Answer {
    call_in(router, region, "POST", &format!("/task/{task_id}"), None).await
}

/// `DescribeExecution` in `region`.
async fn describe_execution_in(
    router: &MultiServiceRouter,
    region: &str,
    task_id: &str,
    device: &str,
) -> Answer {
    call_in(
        router,
        region,
        "POST",
        &format!("/task/{task_id}/execution/{device}"),
        None,
    )
    .await
}

/// Apply the store's retention bound to `region` with a `keep` this suite chooses,
/// answering how many records it dropped.
///
/// The production bound is far larger than a test can fill a Region to, so the policy
/// is driven here with a small one: what is under test is WHICH records it forgets,
/// which is the same whatever the number is. `CreateTask` applies it with
/// `TASK_HISTORY_LIMIT`.
fn trim_history(engine: &Arc<engine::InMemoryEngine>, region: &str, keep: usize) -> usize {
    let state = service_state(&**engine).expect("the in-memory engine carries this service's state");
    let mut store = state.lock();
    store.trim_task_history(region, keep)
}

/// Percent-encode a `nextToken` for a query string: the token is base64, whose `+`,
/// `/` and `=` are not query-safe.
fn encode_token(token: &str) -> String {
    token
        .replace('%', "%25")
        .replace('+', "%2B")
        .replace('/', "%2F")
        .replace('=', "%3D")
}

/// Every task id `ListTasks` reports in `region`, paged to the end, optionally through
/// the `state` filter.
async fn listed_task_ids(
    router: &MultiServiceRouter,
    region: &str,
    state: Option<&str>,
    case: &str,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = format!("?maxResults={PAGE}");
        if let Some(state) = state {
            query.push_str(&format!("&state={state}"));
        }
        if let Some(token) = token.as_deref() {
            query.push_str(&format!("&nextToken={}", encode_token(token)));
        }
        let body = ok(
            &call_in(router, region, "GET", &format!("/tasks{query}"), None).await,
            case,
        );
        let page = body["tasks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a tasks array, got {body}"))
            .clone();
        for task in &page {
            ids.insert(string(task, "taskId", case));
        }
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    ids
}

/// Assert a create was refused with the modelled quota error.
fn assert_quota_exceeded(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, QUOTA_STATUS,
        "case {case:?}: expected HTTP {QUOTA_STATUS}, got {} with {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains(QUOTA_CODE),
        "case {case:?}: expected a {QUOTA_CODE}, got {}",
        answer.body
    );
}

/// Fill `region` with `count` tasks that stay in flight, answering their ids in order.
async fn fill_in_flight(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    region: &str,
    device: &str,
    count: usize,
    case: &str,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        // The command alternates, so the fill is not one repeated request: both
        // members of the modelled `Command` union consume the quota the same way.
        let command = if index % 2 == 0 {
            json!({ "reboot": {} })
        } else {
            json!({ "unlock": {} })
        };
        let answer = create_in(router, region, device, command).await;
        let body = ok(&answer, case);
        ids.push(string(&body, "taskId", case));
        // Held immediately, so the task this call minted cannot be carried out of
        // flight by a later call's settle while the fill is still running.
        hold_pending(engine);
    }
    assert_eq!(
        ids.iter().collect::<BTreeSet<_>>().len(),
        count,
        "case {case:?}: every create must mint its own task id"
    );
    ids
}

/// How a row's filled tasks become terminal, and the `TaskState` that leaves them in.
#[derive(Debug, Clone, Copy)]
enum HowTerminal {
    /// The clock finishes every execution, so the task is `COMPLETED`.
    Clock,
    /// `CancelTask` stops the executions no device started, so the task is `CANCELED`.
    Cancel,
}

impl HowTerminal {
    fn state(self) -> &'static str {
        match self {
            Self::Clock => "COMPLETED",
            Self::Cancel => "CANCELED",
        }
    }
}

#[tokio::test]
async fn the_quota_counts_tasks_in_flight_not_tasks_ever_created() {
    for how in [HowTerminal::Clock, HowTerminal::Cancel] {
        let case = format!("tasks made terminal by {how:?}");
        let case = case.as_str();
        let (router, engine, _) = emulator();
        let device = a_device_in(&engine, FILLED_REGION);

        // One short of the bound: nothing is exceeded yet.
        let filled = fill_in_flight(
            &router,
            &engine,
            FILLED_REGION,
            &device,
            IN_FLIGHT_TASK_QUOTA - 1,
            case,
        )
        .await;

        // At the bound: the create that takes the last one is served, because a quota
        // is exceeded only past it.
        let at_bound = create_in(&router, FILLED_REGION, &device, json!({ "reboot": {} })).await;
        let at_bound_id = string(&ok(&at_bound, case), "taskId", case);
        hold_pending(&engine);

        // One past the bound: the modelled refusal.
        let past_bound = create_in(&router, FILLED_REGION, &device, json!({ "reboot": {} })).await;
        assert_quota_exceeded(&past_bound, case);

        let in_flight = listed_task_ids(&router, FILLED_REGION, Some("IN_PROGRESS"), case).await;
        assert_eq!(
            in_flight.len(),
            IN_FLIGHT_TASK_QUOTA,
            "case {case:?}: the refusal must be owed to exactly the tasks in flight"
        );
        assert!(
            in_flight.contains(&at_bound_id),
            "case {case:?}: the task created at the bound must be in flight, got {in_flight:?}"
        );

        // Now take every one of those tasks out of flight.
        match how {
            HowTerminal::Clock => {
                release_pending(&engine);
                // Any call settles what the clock already owes.
                let _ = listed_task_ids(&router, FILLED_REGION, None, case).await;
            }
            HowTerminal::Cancel => {
                for task_id in filled.iter().chain(std::iter::once(&at_bound_id)) {
                    ok(&cancel_in(&router, FILLED_REGION, task_id).await, case);
                }
            }
        }

        // Nothing was pruned: every task ever created is still reported, in the
        // terminal state it reached, and none of them is in flight any more.
        let all = listed_task_ids(&router, FILLED_REGION, None, case).await;
        assert_eq!(
            all.len(),
            IN_FLIGHT_TASK_QUOTA,
            "case {case:?}: a terminal task keeps its record, got {} tasks",
            all.len()
        );
        let terminal = listed_task_ids(&router, FILLED_REGION, Some(how.state()), case).await;
        assert_eq!(
            terminal, all,
            "case {case:?}: every filled task must report {}",
            how.state()
        );
        assert_eq!(
            listed_task_ids(&router, FILLED_REGION, Some("IN_PROGRESS"), case).await,
            BTreeSet::new(),
            "case {case:?}: no task may still be in flight"
        );

        // A sample of the records, read back one by one: the first, one in the middle
        // and the one created at the bound.
        for task_id in [
            &filled[0],
            &filled[filled.len() / 2],
            &at_bound_id,
        ] {
            let described = ok(&describe_in(&router, FILLED_REGION, task_id).await, case);
            assert_eq!(
                string(&described, "state", case),
                how.state(),
                "case {case:?}: DescribeTask must report {} for {task_id}",
                how.state()
            );
        }

        // And the quota is free again: the create that was refused a moment ago is
        // served now, with a task id and ARN of its own.
        let after = ok(
            &create_in(&router, FILLED_REGION, &device, json!({ "reboot": {} })).await,
            case,
        );
        let fresh_id = string(&after, "taskId", case);
        let fresh_arn = string(&after, "taskArn", case);
        assert!(
            !all.contains(&fresh_id),
            "case {case:?}: the create after the quota freed must mint a fresh task id, got \
             {fresh_id}"
        );
        assert_eq!(
            fresh_arn,
            format!("arn:aws:snow-device-management:{FILLED_REGION}:{ACCOUNT}:task/{fresh_id}"),
            "case {case:?}: the fresh task's ARN must name it in the signed region"
        );
        assert_eq!(
            ok(&describe_in(&router, FILLED_REGION, &fresh_id).await, case)["state"],
            json!("IN_PROGRESS"),
            "case {case:?}: the fresh task is the one in flight now"
        );
    }
}

#[tokio::test]
async fn the_quota_is_counted_per_region() {
    // What one Region holds is not what another holds: a Region filled to the bound
    // refuses, while a create signed for a Region holding nothing in flight is served
    // — and the two Regions' tasks stay invisible to each other.
    let case = "one region at the bound, another empty";
    let (router, engine, _) = emulator();
    let filled_device = a_device_in(&engine, FILLED_REGION);
    let other_device = a_device_in(&engine, OTHER_REGION);

    let filled = fill_in_flight(
        &router,
        &engine,
        FILLED_REGION,
        &filled_device,
        IN_FLIGHT_TASK_QUOTA,
        case,
    )
    .await;

    assert_quota_exceeded(
        &create_in(&router, FILLED_REGION, &filled_device, json!({ "reboot": {} })).await,
        case,
    );

    let elsewhere = ok(
        &create_in(&router, OTHER_REGION, &other_device, json!({ "unlock": {} })).await,
        case,
    );
    let elsewhere_id = string(&elsewhere, "taskId", case);
    hold_pending(&engine);

    assert_eq!(
        listed_task_ids(&router, OTHER_REGION, None, case).await,
        BTreeSet::from([elsewhere_id.clone()]),
        "case {case:?}: the other region holds only the task created there"
    );
    let here = listed_task_ids(&router, FILLED_REGION, None, case).await;
    assert_eq!(
        here,
        filled.iter().cloned().collect::<BTreeSet<_>>(),
        "case {case:?}: the filled region holds only its own tasks"
    );
    assert!(
        !here.contains(&elsewhere_id),
        "case {case:?}: a task of another region must not be reported here"
    );

    // The filled Region is still at its own bound: serving the other Region's create
    // did not change what this one holds.
    assert_quota_exceeded(
        &create_in(&router, FILLED_REGION, &filled_device, json!({ "reboot": {} })).await,
        case,
    );
}

/// One row of the retention table: which of the created tasks are terminal when the
/// bound is applied, and how many records it may then forget.
struct Retention {
    case: &'static str,
    /// The indexes, in creation order, of the tasks cancelled before the bound runs.
    cancelled: &'static [usize],
    dropped: usize,
}

#[tokio::test]
async fn the_history_bound_forgets_the_oldest_terminal_tasks_and_nothing_in_flight() {
    // The other half of bounding the store: nothing deletes a task record, so past the
    // records a Region keeps, the OLDEST TERMINAL ones make room — a task still in
    // flight is never forgotten, and a Region holding fewer records than the bound
    // forgets nothing. The rows differ along how many of the Region's tasks are
    // terminal and which of them, which is what the choice depends on.
    const CREATED: usize = 4;
    const KEEP: usize = 2;
    let table = [
        Retention {
            case: "nothing terminal, so nothing to forget",
            cancelled: &[],
            dropped: 0,
        },
        Retention {
            case: "fewer terminal than the excess",
            cancelled: &[2],
            dropped: 1,
        },
        Retention {
            case: "the two oldest are terminal",
            cancelled: &[0, 1],
            dropped: 2,
        },
        Retention {
            case: "every task is terminal",
            cancelled: &[0, 1, 2, 3],
            dropped: 2,
        },
    ];

    for row in table {
        let (router, engine, _) = emulator();
        let device = a_device_in(&engine, FILLED_REGION);
        let created =
            fill_in_flight(&router, &engine, FILLED_REGION, &device, CREATED, row.case).await;

        for index in row.cancelled {
            ok(
                &cancel_in(&router, FILLED_REGION, &created[*index]).await,
                row.case,
            );
        }

        assert_eq!(
            trim_history(&engine, FILLED_REGION, KEEP),
            row.dropped,
            "case {:?}: the bound must forget {} record(s)",
            row.case,
            row.dropped
        );

        // The forgotten records are the oldest terminal ones, in creation order; every
        // other task is still reported, whatever state it is in.
        let forgotten: BTreeSet<String> = row
            .cancelled
            .iter()
            .take(row.dropped)
            .map(|index| created[*index].clone())
            .collect();
        let kept: BTreeSet<String> = created
            .iter()
            .filter(|task_id| !forgotten.contains(*task_id))
            .cloned()
            .collect();
        assert_eq!(
            listed_task_ids(&router, FILLED_REGION, None, row.case).await,
            kept,
            "case {:?}: the region must report exactly the records it kept",
            row.case
        );
        for task_id in &forgotten {
            assert_not_found(
                &describe_in(&router, FILLED_REGION, task_id).await,
                row.case,
            );
            assert_not_found(
                &describe_execution_in(&router, FILLED_REGION, task_id, &device).await,
                row.case,
            );
        }
        for task_id in &kept {
            let described = ok(&describe_in(&router, FILLED_REGION, task_id).await, row.case);
            assert!(
                !string(&described, "state", row.case).is_empty(),
                "case {:?}: a kept task must still describe, got {described}",
                row.case
            );
            ok(
                &describe_execution_in(&router, FILLED_REGION, task_id, &device).await,
                row.case,
            );
        }

        // And a Region already inside the bound loses nothing on a second pass.
        assert_eq!(
            trim_history(&engine, FILLED_REGION, CREATED),
            0,
            "case {:?}: a region inside the bound must forget nothing",
            row.case
        );
    }
}

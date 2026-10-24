// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AWS Outposts capacity-task slice, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! site + Outpost harness is [`crate::site_and_outpost_lifecycle`]'s.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `StartCapacityTask` mints a `CapacityTaskId` of the form its model declares,
//!    persists every member of the task the caller requested, and answers it from
//!    what was persisted; `GetCapacityTask` answers that same record, member for
//!    member, whether the Outpost was named by bare id or by full ARN.
//! 2. A task starts in the non-terminal `REQUESTED` status and advances through the
//!    modelled statuses on the clock alone, ending `COMPLETED` with a
//!    `CompletionDate` that equals its `LastModifiedDate` and never moving again. A
//!    read reports the status the task holds at the instant it is asked for, and
//!    never moves the task itself.
//! 3. One active capacity task per Outpost and per order: a second one is refused
//!    409 `ConflictException` naming the resource the conflict is about, and the
//!    same request is accepted once the first task has completed.
//! 4. A capacity task is a task OF an Outpost: an identifier that names nothing is
//!    404 `NotFoundException` with the identifier echo of the member it arrived in,
//!    a task of another Outpost is not reachable under this one's route, and the
//!    Outpost going away takes its tasks with it.
//! 5. A DRY RUN is answered 501 `NotImplemented` and persists nothing — the verdict
//!    it asks for is about installed instance capacity this emulator does not model.
//! 6. The store a task lands in is bounded: at the emulator's entry ceiling a start
//!    is refused and nothing is stored.

use std::collections::VecDeque;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    store_full, CapacityTaskRecord, CAPACITY_TASK_COMPLETED, CAPACITY_TASK_IN_PROGRESS,
    CAPACITY_TASK_REQUESTED, CAPACITY_TASK_STEP, MAX_RESOURCES,
};

use crate::site_and_outpost_lifecycle::{
    segment, text, Outposts, Reply, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};

/// The `CapacityTaskId` prefix the modelled pattern `^cap-[a-f0-9]{17}$` declares,
/// and the exact length (`length(min: 21, max: 21)`) it pins.
const CAPACITY_TASK_ID_PREFIX: &str = "cap-";
/// See [`CAPACITY_TASK_ID_PREFIX`].
const CAPACITY_TASK_ID_LENGTH: usize = 21;

/// A `CapacityTaskId` this emulator never mints — well formed, so the boundary
/// accepts it, and naming nothing.
const ABSENT_CAPACITY_TASK_ID: &str = "cap-00000000000000000";

/// An `OrderId` of the modelled form `^oo-[a-f0-9]{17}$`. Orders are not persisted
/// by this slice (`CreateOrder` is still a scaffold stub), so an order id is only
/// ever a value a capacity task carries and is keyed by.
const ORDER_ID: &str = "oo-1111111111111111a";
/// A second order, for the case that shows the one-active-task rule is per order.
const OTHER_ORDER_ID: &str = "oo-2222222222222222b";

/// The two values the modelled `TaskActionOnBlockingInstances` enum accepts, from
/// `smithy_query(query="shape",shape="TaskActionOnBlockingInstances")`.
const TASK_ACTIONS: [&str; 2] = ["WAIT_FOR_EVACUATION", "FAIL_TASK"];

/// The modelled statuses a task this emulator starts moves through, in order:
/// `CapacityTaskStatus`'s `REQUESTED` -> `IN_PROGRESS` -> `COMPLETED`.
fn progression() -> [&'static str; 3] {
    [
        CAPACITY_TASK_REQUESTED,
        CAPACITY_TASK_IN_PROGRESS,
        CAPACITY_TASK_COMPLETED,
    ]
}

/// Wait until every step of a capacity task's clock-owed progression has fallen
/// due.
///
/// Blocking with `std::thread::sleep` for the same reason
/// [`crate::site_and_outpost_lifecycle::settle`] does: the workspace's `tokio`
/// carries no timer driver.
fn settle_capacity_task() {
    std::thread::sleep(2 * CAPACITY_TASK_STEP + std::time::Duration::from_millis(500));
}

/// Whether `id` is a `CapacityTaskId` of the modelled form: `cap-` plus exactly 17
/// lowercase hex characters, 21 characters in all.
fn is_capacity_task_id(id: &str) -> bool {
    id.len() == CAPACITY_TASK_ID_LENGTH
        && match id.strip_prefix(CAPACITY_TASK_ID_PREFIX) {
            Some(hex) => hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            None => false,
        }
}

/// The capacity-task collection route of an Outpost, by either identifier form.
fn capacity_route(outpost_identifier: &str) -> String {
    format!("/outposts/{}/capacity", segment(outpost_identifier))
}

/// The route of ONE capacity task of an Outpost.
fn task_route(outpost_identifier: &str, capacity_task_id: &str) -> String {
    format!(
        "{}/{}",
        capacity_route(outpost_identifier),
        segment(capacity_task_id)
    )
}

/// The members of a capacity task that NEVER change once it is created: everything
/// but the three clock-driven ones (`CapacityTaskStatus`, `LastModifiedDate`,
/// `CompletionDate`), which rule 2 states on its own.
fn stable(task: &Value) -> Value {
    let mut stripped = task.clone();
    if let Some(members) = stripped.as_object_mut() {
        for clock_driven in ["CapacityTaskStatus", "LastModifiedDate", "CompletionDate"] {
            members.remove(clock_driven);
        }
    }
    stripped
}

/// An epoch-seconds timestamp member — how restJson1 renders this model's
/// `ISO8601Timestamp`, which carries no `@timestampFormat` — or `None` when the
/// response omits it.
fn stamp(task: &Value, member: &str) -> Option<f64> {
    task.get(member).and_then(Value::as_f64)
}

/// One Outpost of a fresh site, so each case starts from an Outpost that has never
/// run a capacity task. Returns its bare `OutpostId`.
async fn outpost(outposts: &Outposts, name: &str) -> String {
    let site = outposts.create_site(&json!({ "Name": format!("{name}-site") })).await;
    let created = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&created, "OutpostId")
}

/// `StartCapacityTask` on `outpost_identifier`.
async fn start(outposts: &Outposts, outpost_identifier: &str, request: &Value) -> Reply {
    outposts
        .call("POST", &capacity_route(outpost_identifier), Some(request))
        .await
}

/// `StartCapacityTask`, required to have been accepted.
async fn start_ok(outposts: &Outposts, outpost_identifier: &str, request: &Value) -> Value {
    outposts
        .ok("POST", &capacity_route(outpost_identifier), Some(request))
        .await
}

/// `GetCapacityTask`.
async fn get(outposts: &Outposts, outpost_identifier: &str, capacity_task_id: &str) -> Reply {
    outposts
        .call("GET", &task_route(outpost_identifier, capacity_task_id), None)
        .await
}

/// `GetCapacityTask`, required to have answered 200.
async fn get_ok(outposts: &Outposts, outpost_identifier: &str, capacity_task_id: &str) -> Value {
    outposts
        .ok("GET", &task_route(outpost_identifier, capacity_task_id), None)
        .await
}

/// A pool of `count` hosts of `instance_type`, the modelled `InstanceTypeCapacity`.
fn pool(instance_type: &str, count: i64) -> Value {
    json!({ "InstanceType": instance_type, "Count": count })
}

/// Rule 1: whatever the caller asked for, `StartCapacityTask` persists it and
/// `GetCapacityTask` reads it back member for member.
///
/// The rows differ along everything the rule generalises over: how many instance
/// pools (none, one, several), the `Count` of a pool at both published bounds of
/// the modelled `range(min: 0, max: 9999)` and inside it, whether the optional
/// `OrderId`, `AssetId`, `InstancesToExclude` and `TaskActionOnBlockingInstances`
/// are present, which sub-lists of `InstancesToExclude` are, both values of the
/// `TaskActionOnBlockingInstances` enum, and whether the defaulted `DryRun` member
/// is sent at all.
#[tokio::test]
async fn start_capacity_task_persists_and_get_capacity_task_reads_it_back() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Value)> = vec![
        (
            "one pool, no optional member and no DryRun",
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
        ),
        (
            "one pool, DryRun sent explicitly false",
            json!({ "InstancePools": [pool("m5.4xlarge", 2)], "DryRun": false }),
        ),
        (
            "a count at the modelled range minimum",
            json!({ "InstancePools": [pool("m5.xlarge", 0)] }),
        ),
        (
            "a count at the modelled range maximum",
            json!({ "InstancePools": [pool("c5.24xlarge", 9999)] }),
        ),
        (
            "several pools",
            json!({
                "InstancePools": [
                    pool("m5.large", 3),
                    pool("c5.xlarge", 7),
                    pool("r5.2xlarge", 1),
                ],
            }),
        ),
        (
            "no pools at all",
            json!({ "InstancePools": [] }),
        ),
        (
            "an order and an asset",
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "OrderId": ORDER_ID,
                "AssetId": "1234567890",
            }),
        ),
        (
            "instances to exclude, every sub-list",
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "InstancesToExclude": {
                    "Instances": ["i-0123456789abcdef0", "i-00000000000000001"],
                    "AccountIds": ["000000000000"],
                    "Services": ["EC2"],
                },
            }),
        ),
        (
            "instances to exclude, one sub-list only",
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "InstancesToExclude": { "Services": ["AWS", "EKS"] },
            }),
        ),
        (
            "every optional member at once",
            json!({
                "InstancePools": [pool("m5.4xlarge", 4), pool("m5.8xlarge", 2)],
                "OrderId": OTHER_ORDER_ID,
                "AssetId": "9876543210",
                "InstancesToExclude": { "Instances": ["i-0123456789abcdef0"] },
                "TaskActionOnBlockingInstances": TASK_ACTIONS[0],
                "DryRun": false,
            }),
        ),
        (
            "the other TaskActionOnBlockingInstances value",
            json!({
                "InstancePools": [pool("m5.4xlarge", 1)],
                "TaskActionOnBlockingInstances": TASK_ACTIONS[1],
            }),
        ),
    ];

    for (index, (case, request)) in cases.iter().enumerate() {
        // A fresh Outpost per row: the one-active-task rule (rule 3) is what stops
        // two tasks running on one Outpost, and is stated there.
        let outpost_id = outpost(&outposts, &format!("capacity-{index}")).await;
        let started = start_ok(&outposts, &outpost_id, request).await;
        let task_id = text(&started, "CapacityTaskId");

        assert!(
            is_capacity_task_id(&task_id),
            "case {case:?}: minted CapacityTaskId {task_id:?} is not of the form \
             cap- plus 17 lowercase hex characters"
        );
        assert_eq!(
            text(&started, "OutpostId"),
            outpost_id,
            "case {case:?}: the task must report the Outpost it runs on, got {started}"
        );
        assert_eq!(
            text(&started, "CapacityTaskStatus"),
            CAPACITY_TASK_REQUESTED,
            "case {case:?}: a just-started task is awaiting its next step, got {started}"
        );
        assert_eq!(
            started.get("DryRun"),
            Some(&Value::Bool(false)),
            "case {case:?}: expected DryRun false, got {started}"
        );
        assert_eq!(
            started.get("RequestedInstancePools"),
            request.get("InstancePools"),
            "case {case:?}: the task must report the pools the request asked for, got {started}"
        );
        for (member, requested) in [
            ("OrderId", request.get("OrderId")),
            ("AssetId", request.get("AssetId")),
            ("InstancesToExclude", request.get("InstancesToExclude")),
            (
                "TaskActionOnBlockingInstances",
                request.get("TaskActionOnBlockingInstances"),
            ),
        ] {
            assert_eq!(
                started.get(member),
                requested,
                "case {case:?}: member {member} must be what the request named \
                 (and absent when it named none), got {started}"
            );
        }
        assert_eq!(
            started.get("Failed"),
            None,
            "case {case:?}: nothing failed this task, got {started}"
        );
        assert_eq!(
            started.get("CompletionDate"),
            None,
            "case {case:?}: a task that has not completed has no CompletionDate, got {started}"
        );
        let created = stamp(&started, "CreationDate");
        assert_eq!(
            (created.is_some(), stamp(&started, "LastModifiedDate")),
            (true, created),
            "case {case:?}: a task that has not changed yet was last modified when it \
             was created, got {started}"
        );

        // The round trip: the read answers the record the start persisted.
        let read = get_ok(&outposts, &outpost_id, &task_id).await;
        assert_eq!(
            stable(&read),
            stable(&started),
            "case {case:?}: GetCapacityTask must answer the persisted task"
        );
        assert_eq!(
            stamp(&read, "CreationDate"),
            created,
            "case {case:?}: a task's CreationDate never changes, got {read}"
        );
        assert!(
            progression().contains(&text(&read, "CapacityTaskStatus").as_str()),
            "case {case:?}: status {:?} is not one of the statuses a task moves through: {read}",
            text(&read, "CapacityTaskStatus")
        );
    }
}

/// Rule 1, second half: both identifier forms the modelled `OutpostIdentifier`
/// pattern accepts — the bare id and the full Outpost ARN — reach the same task,
/// and neither reaches a task of another Outpost (rule 4).
#[tokio::test]
async fn both_outpost_identifier_forms_read_the_same_capacity_task() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "both-forms").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 2)], "OrderId": ORDER_ID });
    let started = start_ok(&outposts, &outpost_id, &request).await;
    let task_id = text(&started, "CapacityTaskId");
    let outpost_arn = format!("{OUTPOST_ARN_PREFIX}{outpost_id}");

    for (case, identifier) in [
        ("by bare Outpost id", outpost_id.clone()),
        ("by full Outpost ARN", outpost_arn),
    ] {
        let read = get_ok(&outposts, &identifier, &task_id).await;
        assert_eq!(
            stable(&read),
            stable(&started),
            "case {case:?}: the two identifier forms name the same task"
        );
        assert_eq!(
            text(&read, "OutpostId"),
            outpost_id,
            "case {case:?}: the task reports the Outpost's id whichever form named it, got {read}"
        );
    }
}

/// Rule 2: the clock alone moves a task, through the modelled statuses in order, to
/// `COMPLETED` — and once there it stays, with `CompletionDate` equal to
/// `LastModifiedDate` and no earlier than `CreationDate`.
///
/// The observed statuses are collected by polling, and asserted to be an ordered
/// sub-sequence of the progression rather than a fixed schedule: a reader that
/// looks late sees a later status, never a different one, and never an earlier one.
#[tokio::test]
async fn a_capacity_task_runs_to_completion_on_the_clock_alone() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "progression").await;
    let started = start_ok(
        &outposts,
        &outpost_id,
        &json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
    )
    .await;
    let task_id = text(&started, "CapacityTaskId");
    let created = stamp(&started, "CreationDate").unwrap_or_default();

    // The first read happens inside the first step's span, so the transient status
    // the start reported is still what a caller sees.
    let first = get_ok(&outposts, &outpost_id, &task_id).await;
    assert_eq!(
        text(&first, "CapacityTaskStatus"),
        CAPACITY_TASK_REQUESTED,
        "a caller reading straight after the start still sees the requested task, got {first}"
    );

    let mut observed: Vec<String> = Vec::new();
    let mut last = Value::Null;
    let deadline = std::time::Instant::now() + 6 * CAPACITY_TASK_STEP;
    while std::time::Instant::now() < deadline {
        let read = get_ok(&outposts, &outpost_id, &task_id).await;
        let status = text(&read, "CapacityTaskStatus");
        if observed.last() != Some(&status) {
            observed.push(status.clone());
        }
        last = read;
        if status == CAPACITY_TASK_COMPLETED {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert_eq!(
        observed.first().map(String::as_str),
        Some(CAPACITY_TASK_REQUESTED),
        "the first status observed must be the one the start reported, got {observed:?}"
    );
    assert_eq!(
        observed.last().map(String::as_str),
        Some(CAPACITY_TASK_COMPLETED),
        "the task must reach the terminal status on the clock alone, got {observed:?}"
    );
    let mut expected = progression().into_iter();
    for status in &observed {
        assert!(
            expected.any(|next| next == status.as_str()),
            "observed statuses {observed:?} are not in the order a task moves through them"
        );
    }

    let completion = stamp(&last, "CompletionDate");
    assert_eq!(
        (completion.is_some(), stamp(&last, "LastModifiedDate")),
        (true, completion),
        "a completed task was last modified when it completed, got {last}"
    );
    let span = completion.unwrap_or_default() - created;
    let owed = (2 * CAPACITY_TASK_STEP).as_secs_f64();
    assert!(
        span >= owed - 0.001 && span < owed + 5.0,
        "the completion must be dated when the clock owed it ({owed}s after creation), \
         got {span}s: {last}"
    );

    // A read never moves the task: once terminal, every later read answers the same
    // record, including the dates.
    settle_capacity_task();
    let again = get_ok(&outposts, &outpost_id, &task_id).await;
    assert_eq!(
        again, last,
        "a settled task answers the same record to every later read"
    );
}

/// Rule 3: one active capacity task per Outpost and per order, and the refusal
/// names the resource the conflict is about — then the same request is accepted
/// once the first task has completed.
#[tokio::test]
async fn one_active_capacity_task_per_outpost_and_per_order() {
    let outposts = Outposts::new();
    let first_outpost = outpost(&outposts, "conflict-first").await;
    let second_outpost = outpost(&outposts, "conflict-second").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)], "OrderId": ORDER_ID });
    let active = text(
        &start_ok(&outposts, &first_outpost, &request).await,
        "CapacityTaskId",
    );

    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            "a second task on the same Outpost",
            &first_outpost,
            first_outpost.clone(),
            "OUTPOST",
        ),
        (
            "a task on another Outpost for the same order",
            &second_outpost,
            ORDER_ID.to_string(),
            "ORDER",
        ),
    ];
    for (case, outpost_identifier, resource_id, resource_type) in &cases {
        let reply = start(&outposts, outpost_identifier, &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            (
                text(&reply.body, "ResourceId"),
                text(&reply.body, "ResourceType")
            ),
            (resource_id.clone(), (*resource_type).to_string()),
            "case {case:?}: the conflict must name the resource it is about, got {}",
            reply.body
        );
        assert!(
            text(&reply.body, "Message").contains(&active),
            "case {case:?}: the conflict must name the task already active ({active}), got {}",
            reply.body
        );
    }

    // A task for ANOTHER order on another Outpost is nobody's conflict.
    let other = start_ok(
        &outposts,
        &second_outpost,
        &json!({ "InstancePools": [pool("m5.4xlarge", 1)], "OrderId": OTHER_ORDER_ID }),
    )
    .await;
    assert_ne!(
        text(&other, "CapacityTaskId"),
        active,
        "two tasks are two resources, with two ids"
    );

    // Once the clock has completed them, the same requests are accepted.
    settle_capacity_task();
    let after = start_ok(&outposts, &first_outpost, &request).await;
    assert_ne!(
        text(&after, "CapacityTaskId"),
        active,
        "a second task on the same Outpost is a NEW task, got {after}"
    );
    assert_eq!(
        text(&after, "CapacityTaskStatus"),
        CAPACITY_TASK_REQUESTED,
        "the new task starts where every task starts, got {after}"
    );
    // Both tasks are still readable: completing one did not remove it.
    for (case, task_id) in [("the completed task", &active), ("the new task", &text(&after, "CapacityTaskId"))] {
        let read = get_ok(&outposts, &first_outpost, task_id).await;
        assert_eq!(
            text(&read, "CapacityTaskId"),
            *task_id,
            "case {case:?}: every task of an Outpost stays readable"
        );
    }
}

/// One refused capacity-task request: what to send, and the status, wire error code
/// and — when one of this service's two message rules fixes it — the exact body it
/// must be refused with.
struct Refused {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    message: Option<String>,
}

/// Rule 4 and rule 5: every refusal the capacity-task routes state, each with the
/// status, wire error code and — for the two message rules this service words its
/// refusals by — the exact body.
#[tokio::test]
async fn capacity_task_refusals_name_the_identifier_and_the_constraint() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "refusals").await;
    let other_outpost = outpost(&outposts, "refusals-other").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    let task_id = text(
        &start_ok(&outposts, &outpost_id, &request).await,
        "CapacityTaskId",
    );
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");

    let cases: Vec<Refused> = vec![
        Refused {
            case: "GetCapacityTask on an Outpost nothing holds, by bare id",
            method: "GET",
            path: task_route(ABSENT_OUTPOST_ID, &task_id),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: Some(format!("outpostIdentifier={ABSENT_OUTPOST_ID}")),
        },
        Refused {
            case: "GetCapacityTask on an Outpost nothing holds, by ARN",
            method: "GET",
            path: task_route(&absent_outpost_arn, &task_id),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: Some(format!("outpostIdentifier={absent_outpost_arn}")),
        },
        Refused {
            case: "GetCapacityTask on a task id nothing holds",
            method: "GET",
            path: task_route(&outpost_id, ABSENT_CAPACITY_TASK_ID),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: Some(format!("capacityTaskId={ABSENT_CAPACITY_TASK_ID}")),
        },
        Refused {
            case: "GetCapacityTask for a task of another Outpost",
            method: "GET",
            path: task_route(&other_outpost, &task_id),
            body: None,
            status: 404,
            code: "NotFoundException",
            message: Some(format!("capacityTaskId={task_id}")),
        },
        Refused {
            case: "GetCapacityTask with a task id outside its pattern",
            method: "GET",
            path: task_route(&outpost_id, "cap-zzzzzzzzzzzzzzzzz"),
            body: None,
            status: 400,
            code: "ValidationException",
            message: Some(
                "1 validation error detected: Value at 'capacityTaskId' failed to \
                 satisfy constraint: Member must satisfy regular expression pattern: \
                 ^cap-[a-f0-9]{17}$"
                    .to_string(),
            ),
        },
        Refused {
            case: "GetCapacityTask with an Outpost identifier that is no identifier",
            method: "GET",
            path: task_route("not-an-outpost", &task_id),
            body: None,
            status: 400,
            code: "ValidationException",
            message: None,
        },
        Refused {
            case: "StartCapacityTask on an Outpost nothing holds",
            method: "POST",
            path: capacity_route(ABSENT_OUTPOST_ID),
            body: Some(request.clone()),
            status: 404,
            code: "NotFoundException",
            message: Some(format!("outpostIdentifier={ABSENT_OUTPOST_ID}")),
        },
        Refused {
            case: "StartCapacityTask with an instance type outside its pattern",
            method: "POST",
            path: capacity_route(&other_outpost),
            body: Some(json!({ "InstancePools": [pool("M5.4XLARGE", 1)] })),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        Refused {
            case: "StartCapacityTask with a pool count above its range",
            method: "POST",
            path: capacity_route(&other_outpost),
            body: Some(json!({ "InstancePools": [pool("m5.4xlarge", 10_000)] })),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        Refused {
            case: "StartCapacityTask with the required pools absent",
            method: "POST",
            path: capacity_route(&other_outpost),
            body: Some(json!({})),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        Refused {
            case: "StartCapacityTask as a dry run",
            method: "POST",
            path: capacity_route(&other_outpost),
            body: Some(json!({ "InstancePools": [pool("m5.4xlarge", 1)], "DryRun": true })),
            status: 501,
            code: "NotImplemented",
            message: None,
        },
    ];

    for expected in &cases {
        let reply = outposts
            .call(expected.method, &expected.path, expected.body.as_ref())
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (expected.status, expected.code),
            "case {:?}: expected {} {}, got {} {} {}",
            expected.case,
            expected.status,
            expected.code,
            reply.status,
            reply.error_code,
            reply.body
        );
        if let Some(message) = &expected.message {
            // A modelled error shape carries `Message`; the framework's synthetic
            // `ValidationException` envelope carries `message`.
            let actual = match reply.body.get("Message") {
                Some(_) => text(&reply.body, "Message"),
                None => text(&reply.body, "message"),
            };
            assert_eq!(
                &actual, message,
                "case {:?}: expected the refusal body {message:?}, got {:?}",
                expected.case, reply.body
            );
        }
    }

    // Rule 5: the dry run changed nothing — the Outpost it named can still take a
    // task, which it could not if the dry run had left one active.
    let after_dry_run = start_ok(&outposts, &other_outpost, &request).await;
    assert!(
        is_capacity_task_id(&text(&after_dry_run, "CapacityTaskId")),
        "a dry run must persist nothing, so the next start is a fresh task, got {after_dry_run}"
    );
}

/// Rule 6: the store a capacity task lands in is BOUNDED.
///
/// A completed task is never released — `GetCapacityTask` must still answer it — so
/// the only thing that keeps a caller repeating `StartCapacityTask` from growing the
/// store without limit is a maximum entry count. Past it the request is refused with
/// the modelled `ConflictException` and nothing is stored, and the refusal is worded
/// so it cannot be mistaken for the one-active-task one.
///
/// The ceiling is `MAX_RESOURCES` entries, which no sequence of calls could fill
/// inside a test, so this case seeds the store directly and then drives the refusal
/// over the wire.
#[tokio::test]
async fn the_capacity_task_store_is_bounded() {
    let outposts = Outposts::new();
    let below = outpost(&outposts, "ceiling-below").await;
    let at = outpost(&outposts, "ceiling-at").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });

    // One short of the ceiling: the store can still take a task.
    seed_completed_tasks(&outposts, MAX_RESOURCES - 1);
    let accepted = start_ok(&outposts, &below, &request).await;
    assert!(
        is_capacity_task_id(&text(&accepted, "CapacityTaskId")),
        "below the ceiling a start is accepted, got {accepted}"
    );

    // And at the ceiling it cannot — on an Outpost of its own, so what refuses the
    // request is the bound and not the one-active-task rule.
    let reply = start(&outposts, &at, &request).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (409, "ConflictException"),
        "at the ceiling the start must be refused, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        text(&reply.body, "Message"),
        store_full("capacity tasks"),
        "the refusal must say the emulator's store is full, got {}",
        reply.body
    );
    let held = outposts.store().lock().capacity_tasks.len();
    assert_eq!(
        held, MAX_RESOURCES,
        "a refused start must store nothing: expected {MAX_RESOURCES} tasks held, got {held}"
    );
}

/// Fill the store with `count` capacity tasks that have already completed, each on
/// an Outpost of its own that nothing holds — so none of them is active, and none is
/// reachable through a route.
fn seed_completed_tasks(outposts: &Outposts, count: usize) {
    let now = std::time::SystemTime::now();
    let store = outposts.store();
    let mut store = store.lock();
    for index in 0..count {
        let capacity_task_id = format!("cap-{index:017x}");
        store.capacity_tasks.insert(
            capacity_task_id.clone(),
            CapacityTaskRecord {
                sequence: index as u64,
                capacity_task_id,
                outpost_id: format!("op-{index:017x}"),
                order_id: None,
                asset_id: None,
                requested_instance_pools: Vec::new(),
                instances_to_exclude: None,
                dry_run: false,
                task_action_on_blocking_instances: None,
                status: CAPACITY_TASK_COMPLETED.to_string(),
                failed: None,
                created_at: now,
                last_modified_at: now,
                completed_at: Some(now),
                pending: VecDeque::new(),
            },
        );
    }
}

/// Rule 4, last clause: a capacity task exists only as a task of its Outpost, so
/// deleting the Outpost takes it with it — and leaves other Outposts' tasks alone.
#[tokio::test]
async fn deleting_an_outpost_takes_its_capacity_tasks_with_it() {
    let outposts = Outposts::new();
    let doomed = outpost(&outposts, "doomed").await;
    let keeper = outpost(&outposts, "keeper").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    let doomed_task = text(&start_ok(&outposts, &doomed, &request).await, "CapacityTaskId");
    let kept_task = text(&start_ok(&outposts, &keeper, &request).await, "CapacityTaskId");

    let _ = outposts
        .ok("DELETE", &format!("/outposts/{doomed}"), None)
        .await;

    let gone = get(&outposts, &doomed, &doomed_task).await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "NotFoundException"),
        "a deleted Outpost's task is gone to GetCapacityTask, got {} {}",
        gone.status,
        gone.body
    );
    assert_eq!(
        text(&gone.body, "Message"),
        format!("outpostIdentifier={doomed}"),
        "the echo names the Outpost the caller asked under, got {}",
        gone.body
    );
    let kept = get_ok(&outposts, &keeper, &kept_task).await;
    assert_eq!(
        text(&kept, "CapacityTaskId"),
        kept_task,
        "deleting one Outpost leaves every other Outpost's task readable, got {kept}"
    );
}

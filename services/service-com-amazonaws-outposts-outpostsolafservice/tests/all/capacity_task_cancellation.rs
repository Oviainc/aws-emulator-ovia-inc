// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelCapacityTask`, driven over the wire through the same
//! front door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite
//! reuses), starting its tasks through [`crate::capacity_task_lifecycle`]'s own
//! `StartCapacityTask` helper and reading them back through its `GetCapacityTask` one.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Cancelling a capacity task moves it to the modelled `CANCELLATION_IN_PROGRESS`
//!    status — cancelled, and cleaning up resources — and nothing else about the
//!    stored task changes; the progression the clock still owed it is replaced by the
//!    one a cancellation owes, so the task settles in `CANCELLED` instead of ever
//!    completing, carries no `CompletionDate` (nothing ran successfully), and is not
//!    moved again by the clock or by being read.
//! 2. A task being cleaned up is still work in hand, so the one-active-task rule still
//!    refuses another task on its Outpost; once the cancellation has settled the same
//!    request is accepted.
//! 3. A task can be cancelled only while Amazon Web Services Outposts is still
//!    awaiting a step on it: a task it is already running, one it has finished with,
//!    and one already being cancelled are each 409 `ConflictException` naming the
//!    resource the task belongs to in `ResourceId` / `ResourceType` and the status the
//!    task is in, with the stored task exactly as it was.
//! 4. A capacity task exists only as a task OF an Outpost: an identifier that names
//!    nothing — a task id nothing minted, a task of another Outpost, an Outpost this
//!    region's inventory does not hold — is 404 `NotFoundException` with the
//!    identifier echo of the member it arrived in, and nothing is cancelled.
//!
//! Where the seeded state comes from: two of the modelled `CapacityTaskStatus` values
//! are reached by no call of this API at all (see [`crate::delete_conflicts`]), so a
//! row that needs one puts the stored record in it through that suite's store seam.
//! Every assertion still goes over the wire.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::CAPACITY_TASK_STATUS_VALUES;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    capacity_task_conflict, capacity_task_status_conflict, identifier_not_found,
    outpost_not_found, CAPACITY_TASK_CANCELLATION_IN_PROGRESS, CAPACITY_TASK_CANCELLED,
    CAPACITY_TASK_COMPLETED, CAPACITY_TASK_FAILED, CAPACITY_TASK_ID_MEMBER,
    CAPACITY_TASK_IN_PROGRESS, CAPACITY_TASK_REQUESTED, CAPACITY_TASK_STEP,
    OUTPOST_IDENTIFIER_MEMBER, RESOURCE_TYPE_OUTPOST,
};

use crate::capacity_task_lifecycle::{
    an_order, get_ok, install, outpost, pool, settle_capacity_task_steps, stamp, start, start_ok,
    task_route, ABSENT_CAPACITY_TASK_ID, INSTALLED,
};
use crate::delete_conflicts::hold_task_in;
use crate::site_and_outpost_lifecycle::{
    text, without, Outposts, Reply, ABSENT_OUTPOST_ID, OUTPOST_ARN_PREFIX,
};

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="operation",operation="CancelCapacityTask")`: `ConflictException`,
/// HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// The wire error code an identifier that names nothing arrives as. See
/// [`CONFLICT_CODE`].
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The modelled `CapacityTaskStatus` of a task held up by running EC2 instances —
/// "the capacity task requires capacity to run. You must stop the recommended EC2
/// running instances to free up capacity for the task to run", as
/// `smithy_query(query="doc",shape="GetCapacityTaskOutput",member="CapacityTaskStatus")`
/// words it. Amazon Web Services Outposts is still awaiting a step on such a task, so
/// it is one a cancellation answers.
const CAPACITY_TASK_WAITING_FOR_EVACUATION: &str = "WAITING_FOR_EVACUATION";

/// A region other than the harness's home one, for the Outpost that exists but is not
/// part of the inventory the call is addressed to.
const OTHER_REGION: &str = "us-west-2";

/// `CancelCapacityTask` on one task of one Outpost: the operation binds both its
/// members to the URI (`POST /outposts/{OutpostIdentifier}/capacity/{CapacityTaskId}`),
/// so the request carries no payload of its own.
async fn cancel(outposts: &Outposts, outpost_identifier: &str, capacity_task_id: &str) -> Reply {
    outposts
        .call(
            "POST",
            &task_route(outpost_identifier, capacity_task_id),
            Some(&json!({})),
        )
        .await
}

/// `CancelCapacityTask` through `region`'s endpoint. See [`cancel`].
async fn cancel_in(
    outposts: &Outposts,
    region: &str,
    outpost_identifier: &str,
    capacity_task_id: &str,
) -> Reply {
    outposts
        .call_in(
            region,
            "POST",
            &task_route(outpost_identifier, capacity_task_id),
            Some(&json!({})),
        )
        .await
}

/// `CancelCapacityTask`, required to have been served, with the empty body the
/// modelled output declares.
async fn cancel_ok(outposts: &Outposts, outpost_identifier: &str, capacity_task_id: &str, case: &str) {
    let reply = cancel(outposts, outpost_identifier, capacity_task_id).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert!(
        reply
            .body
            .as_object()
            .is_none_or(|members| members.is_empty()),
        "case {case:?}: CancelCapacityTask models no output members, got {}",
        reply.body
    );
}

/// The members of a capacity task a cancellation must leave exactly as they were:
/// everything but the status and the date of the last change, which rule 1 asserts on
/// their own.
fn unchanged_by_a_cancellation(task: &Value) -> Value {
    without(
        &without(task, "CapacityTaskStatus"),
        "LastModifiedDate",
    )
}

/// Wait until the ONE step a cancellation owes the clock has fallen due.
fn settle_cancellation() {
    settle_capacity_task_steps(1);
}

/// The full ARN form of an Outpost id, which the modelled `OutpostIdentifier` pattern
/// accepts beside the bare id.
fn arn(outpost_id: &str) -> String {
    format!("{OUTPOST_ARN_PREFIX}{outpost_id}")
}

/// Rule 1: a cancellation moves the task to `CANCELLATION_IN_PROGRESS`, leaves
/// everything else about it alone, and the clock settles it in `CANCELLED` rather than
/// ever completing it.
///
/// The rows differ along everything the rule generalises over: how the Outpost is
/// named (bare id, full ARN), which members the task carries (no instance pools, one,
/// several, plus each optional member a task can hold), and the status the task is
/// cancelled FROM — the one a start reports and the other status in which Outposts is
/// still awaiting a step on the task.
#[tokio::test]
async fn cancelling_a_capacity_task_settles_it_as_cancelled() {
    let outposts = Outposts::new();
    // (case, the request to start, the status the task is cancelled from, whether the
    //  Outpost is named by its ARN)
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        (
            "one instance pool, the Outpost named by bare id",
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            CAPACITY_TASK_REQUESTED,
            false,
        ),
        (
            "every optional member a task can carry, the Outpost named by ARN",
            json!({
                "InstancePools": [pool("m5.4xlarge", 2), pool("c5.2xlarge", 1)],
                "OrderId": "",
                "AssetId": INSTALLED[0],
                "TaskActionOnBlockingInstances": "WAIT_FOR_EVACUATION",
                "InstancesToExclude": { "AccountIds": ["000000000000"] },
                "DryRun": false,
            }),
            CAPACITY_TASK_REQUESTED,
            true,
        ),
        (
            "no instance pools at all",
            json!({ "InstancePools": [] }),
            CAPACITY_TASK_REQUESTED,
            false,
        ),
        (
            "a task waiting for an instance to be evacuated",
            json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            CAPACITY_TASK_WAITING_FOR_EVACUATION,
            false,
        ),
    ];

    // (case, the Outpost identifier the task was addressed by, the task id, the task
    //  as the cancellation left it)
    let mut cancelled: Vec<(&str, String, String, Value)> = Vec::new();
    for (index, (case, request, from, by_arn)) in rows.iter().enumerate() {
        // An Outpost of its own per row: one active capacity task per Outpost, so a
        // row that starts a task needs an Outpost that has never run one.
        let outpost_id = outpost(&outposts, &format!("cancel-task-{index}")).await;
        let identifier = match by_arn {
            true => arn(&outpost_id),
            false => outpost_id.clone(),
        };
        // The optional references the request names must resolve to resources the
        // service holds, or the start is refused: the asset is recorded as hardware
        // installed on this Outpost and the order is one actually placed against it.
        let mut request = request.clone();
        if request.get("AssetId").is_some() {
            install(&outposts, &outpost_id, &[INSTALLED[0]], case);
        }
        if request.get("OrderId").is_some() {
            request["OrderId"] = json!(an_order(&outposts, &outpost_id).await);
        }
        let started = start_ok(&outposts, &identifier, &request).await;
        let task_id = text(&started, "CapacityTaskId");
        if *from != CAPACITY_TASK_REQUESTED {
            hold_task_in(&outposts, &task_id, from, case);
        }

        // The task as it reads at the instant it is cancelled: the status it is in is
        // what the cancellation moves it out of.
        let before = get_ok(&outposts, &identifier, &task_id).await;
        assert_eq!(
            text(&before, "CapacityTaskStatus"),
            *from,
            "case {case:?}: expected the task to be {from} before it is cancelled, got \
             {before}"
        );

        cancel_ok(&outposts, &identifier, &task_id, case).await;

        let after = get_ok(&outposts, &identifier, &task_id).await;
        assert_eq!(
            text(&after, "CapacityTaskStatus"),
            CAPACITY_TASK_CANCELLATION_IN_PROGRESS,
            "case {case:?}: expected the cancelled task to be \
             {CAPACITY_TASK_CANCELLATION_IN_PROGRESS}, got {after}"
        );
        assert_eq!(
            unchanged_by_a_cancellation(&after),
            unchanged_by_a_cancellation(&before),
            "case {case:?}: a cancellation changes the status and nothing else, got {after}"
        );
        let modified = stamp(&after, "LastModifiedDate");
        assert!(
            modified >= stamp(&after, "CreationDate"),
            "case {case:?}: the cancellation dates the change no earlier than the \
             creation, got {after}"
        );
        cancelled.push((case, identifier, task_id, after));
    }

    // The step a cancellation owes the clock: the task settles in CANCELLED, and that
    // is the end of it.
    settle_cancellation();
    for (case, identifier, task_id, when_cancelled) in &cancelled {
        let settled = get_ok(&outposts, identifier, task_id).await;
        assert_eq!(
            text(&settled, "CapacityTaskStatus"),
            CAPACITY_TASK_CANCELLED,
            "case {case:?}: expected the cancellation to settle in \
             {CAPACITY_TASK_CANCELLED}, got {settled}"
        );
        assert_eq!(
            stamp(&settled, "CompletionDate"),
            None,
            "case {case:?}: a cancelled task never ran successfully, so it carries no \
             CompletionDate, got {settled}"
        );
        assert_eq!(
            unchanged_by_a_cancellation(&settled),
            unchanged_by_a_cancellation(when_cancelled),
            "case {case:?}: settling the cancellation changes the status and the date of \
             the change only, got {settled}"
        );
        let cleaned_up = stamp(when_cancelled, "LastModifiedDate").unwrap_or_default();
        let span = stamp(&settled, "LastModifiedDate").unwrap_or_default() - cleaned_up;
        let owed = CAPACITY_TASK_STEP.as_secs_f64();
        assert!(
            span >= owed - 0.001 && span < owed + 5.0,
            "case {case:?}: the settled cancellation must be dated when the clock owed it \
             ({owed}s after the cancellation), got {span}s: {settled}"
        );
        // A read never moves a settled task: every later read answers the same record.
        let again = get_ok(&outposts, identifier, task_id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a settled task answers the same record to every later read"
        );
    }
}

/// Rule 2: a task being cleaned up is still work in hand on its Outpost; once the
/// cancellation settles, the Outpost can take another task.
#[tokio::test]
async fn a_task_being_cancelled_still_holds_its_outpost_until_it_settles() {
    let outposts = Outposts::new();
    let outpost_id = outpost(&outposts, "cancel-then-start").await;
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    let first = text(
        &start_ok(&outposts, &outpost_id, &request).await,
        "CapacityTaskId",
    );
    cancel_ok(&outposts, &outpost_id, &first, "the first task").await;

    let refused = start(&outposts, &outpost_id, &request).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, CONFLICT_CODE),
        "a task still cleaning up is active, so a second one is refused, got {} {} {}",
        refused.status,
        refused.error_code,
        refused.body
    );
    assert_eq!(
        text(&refused.body, "Message"),
        capacity_task_conflict(RESOURCE_TYPE_OUTPOST, &outpost_id, &first),
        "the refusal names the task still holding the Outpost, got {}",
        refused.body
    );

    settle_cancellation();
    let accepted = start(&outposts, &outpost_id, &request).await;
    assert_eq!(
        accepted.status,
        200,
        "a settled cancellation holds nothing up, so the next task starts, got {} {}",
        accepted.status,
        accepted.body
    );
    let second = text(&accepted.body, "CapacityTaskId");
    assert_ne!(
        second, first,
        "the accepted start mints a task of its own, got {}",
        accepted.body
    );
    let cancelled = get_ok(&outposts, &outpost_id, &first).await;
    assert_eq!(
        text(&cancelled, "CapacityTaskStatus"),
        CAPACITY_TASK_CANCELLED,
        "starting another task leaves the cancelled one cancelled, got {cancelled}"
    );
}

/// Rule 3: only a task Amazon Web Services Outposts is still awaiting a step on can be
/// cancelled.
///
/// The rows are EVERY value of the modelled `CapacityTaskStatus`
/// (`ops::types::CAPACITY_TASK_STATUS_VALUES`, read off the model's own enum, so the
/// set is not a copy this suite could drift from), each paired with what a cancellation
/// of a task in it answers.
#[tokio::test]
async fn a_task_is_cancellable_only_while_a_step_is_still_awaited() {
    let outposts = Outposts::new();
    // (case, the status the stored task is held in, whether a cancellation is served)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a task awaiting the next step by Amazon Web Services Outposts",
            CAPACITY_TASK_REQUESTED,
            true,
        ),
        (
            "a task waiting for an instance to be evacuated",
            CAPACITY_TASK_WAITING_FOR_EVACUATION,
            true,
        ),
        (
            "a task that is running, which the model says cannot be cancelled",
            CAPACITY_TASK_IN_PROGRESS,
            false,
        ),
        (
            "a task that has completed successfully",
            CAPACITY_TASK_COMPLETED,
            false,
        ),
        (
            "a task that could not be completed",
            CAPACITY_TASK_FAILED,
            false,
        ),
        (
            "a task already cancelled and cleaning up",
            CAPACITY_TASK_CANCELLATION_IN_PROGRESS,
            false,
        ),
        (
            "a task that is already cancelled",
            CAPACITY_TASK_CANCELLED,
            false,
        ),
    ];
    assert_eq!(
        rows.len(),
        CAPACITY_TASK_STATUS_VALUES.len(),
        "every modelled CapacityTaskStatus has a row of its own: \
         {CAPACITY_TASK_STATUS_VALUES:?}"
    );
    for (case, status, _) in &rows {
        assert!(
            CAPACITY_TASK_STATUS_VALUES.contains(status),
            "case {case:?}: {status} is a value the model declares"
        );
    }

    for (index, (case, status, served)) in rows.iter().enumerate() {
        // An Outpost of its own per row, for the reason rule 1's rows take one.
        let outpost_id = outpost(&outposts, &format!("cancellable-{index}")).await;
        let task_id = text(
            &start_ok(
                &outposts,
                &outpost_id,
                &json!({ "InstancePools": [pool("m5.4xlarge", 1)] }),
            )
            .await,
            "CapacityTaskId",
        );
        hold_task_in(&outposts, &task_id, status, case);
        let before = get_ok(&outposts, &outpost_id, &task_id).await;

        let reply = cancel(&outposts, &outpost_id, &task_id).await;
        if *served {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected the cancellation to be served, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            let after = get_ok(&outposts, &outpost_id, &task_id).await;
            assert_eq!(
                text(&after, "CapacityTaskStatus"),
                CAPACITY_TASK_CANCELLATION_IN_PROGRESS,
                "case {case:?}: expected the served cancellation to leave the task \
                 {CAPACITY_TASK_CANCELLATION_IN_PROGRESS}, got {after}"
            );
            // And a second cancellation of the same task changes nothing: it is already
            // cancelled and cleaning up.
            let twice = cancel(&outposts, &outpost_id, &task_id).await;
            assert_eq!(
                (twice.status, twice.error_code.as_str()),
                (409, CONFLICT_CODE),
                "case {case:?}: expected a second cancellation to be refused 409 \
                 {CONFLICT_CODE}, got {} {} {}",
                twice.status,
                twice.error_code,
                twice.body
            );
            assert_eq!(
                text(&twice.body, "Message"),
                capacity_task_status_conflict(&task_id, CAPACITY_TASK_CANCELLATION_IN_PROGRESS),
                "case {case:?}: expected the refusal to name the status the task is in, got \
                 {}",
                twice.body
            );
            continue;
        }

        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, CONFLICT_CODE),
            "case {case:?}: expected 409 {CONFLICT_CODE}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceType"),
            RESOURCE_TYPE_OUTPOST,
            "case {case:?}: expected ResourceType {RESOURCE_TYPE_OUTPOST}, got {}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceId"),
            outpost_id,
            "case {case:?}: expected ResourceId {outpost_id:?}, got {}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            capacity_task_status_conflict(&task_id, status),
            "case {case:?}: expected the refusal to name the status the task is in, got {}",
            reply.body
        );
        let after = get_ok(&outposts, &outpost_id, &task_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused cancellation leaves the task exactly as it was, got \
             {after}"
        );
    }
}

/// Rule 4: an identifier this region's inventory holds nothing for is not found, and
/// nothing is cancelled.
///
/// The rows differ along which member names nothing and why: a task id nothing ever
/// minted, a task that exists but belongs to ANOTHER Outpost, an Outpost id nothing
/// minted, and an Outpost that exists in another region — which this region's endpoint
/// does not serve.
#[tokio::test]
async fn an_identifier_this_region_holds_nothing_for_is_not_found() {
    let outposts = Outposts::new();
    let request = json!({ "InstancePools": [pool("m5.4xlarge", 1)] });
    let here = outpost(&outposts, "cancel-not-found").await;
    let elsewhere = outpost(&outposts, "cancel-other-outpost").await;
    let task_here = text(
        &start_ok(&outposts, &here, &request).await,
        "CapacityTaskId",
    );
    let task_elsewhere = text(
        &start_ok(&outposts, &elsewhere, &request).await,
        "CapacityTaskId",
    );
    // An Outpost of another region, with a task of its own: a resource belongs to the
    // region it was created in, so this region's endpoint serves neither.
    let other_site = outposts.site_id_in(OTHER_REGION, "other-region-site").await;
    let other_outpost = text(
        &outposts
            .create_outpost_in(
                OTHER_REGION,
                &json!({ "Name": "other-region", "SiteId": other_site }),
            )
            .await,
        "OutpostId",
    );
    let other_task = text(
        &outposts
            .ok_in(
                OTHER_REGION,
                "POST",
                &format!("/outposts/{other_outpost}/capacity"),
                Some(&request),
            )
            .await,
        "CapacityTaskId",
    );
    // The other region's task is held where its start put it, so what the last
    // assertion here states about the region that DOES serve it is not racing the
    // clock-owed progression of a task nothing in this case polls.
    hold_task_in(
        &outposts,
        &other_task,
        CAPACITY_TASK_REQUESTED,
        "the other region's task",
    );

    // (case, the Outpost identifier sent, the task id sent, the member the refusal is
    // about)
    let rows: Vec<(&str, String, String, &str)> = vec![
        (
            "a CapacityTaskId this emulator never minted",
            here.clone(),
            ABSENT_CAPACITY_TASK_ID.to_string(),
            CAPACITY_TASK_ID_MEMBER,
        ),
        (
            "a task of another Outpost, named under this one's route",
            here.clone(),
            task_elsewhere.clone(),
            CAPACITY_TASK_ID_MEMBER,
        ),
        (
            "an OutpostId this emulator never minted",
            ABSENT_OUTPOST_ID.to_string(),
            task_here.clone(),
            OUTPOST_IDENTIFIER_MEMBER,
        ),
        (
            "an Outpost ARN this emulator never minted",
            arn(ABSENT_OUTPOST_ID),
            task_here.clone(),
            OUTPOST_IDENTIFIER_MEMBER,
        ),
        (
            "an Outpost of another region, named through this one's endpoint",
            other_outpost.clone(),
            other_task.clone(),
            OUTPOST_IDENTIFIER_MEMBER,
        ),
    ];

    for (case, outpost_identifier, capacity_task_id, member) in &rows {
        let reply = cancel(&outposts, outpost_identifier, capacity_task_id).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, NOT_FOUND_CODE),
            "case {case:?}: expected 404 {NOT_FOUND_CODE}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        // Each refusal carries the wording of the kind of resource it is about: the
        // capacity-task identifier echo for a task, and the Outpost sentence — which
        // names the Outpost's own bare id — for the parent Outpost.
        let expected = match *member == CAPACITY_TASK_ID_MEMBER {
            true => identifier_not_found(member, capacity_task_id),
            false => outpost_not_found(outpost_identifier),
        };
        assert_eq!(
            text(&reply.body, "Message"),
            expected,
            "case {case:?}: expected $.Message {expected:?}, got {}",
            reply.body
        );
    }

    // Nothing the refused calls named was cancelled: each task is still on the course
    // its own start put it on, and the region that serves the other Outpost still
    // cancels its task.
    for (case, outpost_identifier, capacity_task_id) in [
        ("the task of this region's Outpost", &here, &task_here),
        (
            "the task of the other Outpost of this region",
            &elsewhere,
            &task_elsewhere,
        ),
    ] {
        let task = get_ok(&outposts, outpost_identifier, capacity_task_id).await;
        let status = text(&task, "CapacityTaskStatus");
        assert!(
            status != CAPACITY_TASK_CANCELLATION_IN_PROGRESS && status != CAPACITY_TASK_CANCELLED,
            "case {case:?}: a refused call cancels nothing, got {task}"
        );
    }
    let served = cancel_in(&outposts, OTHER_REGION, &other_outpost, &other_task).await;
    assert_eq!(
        served.status,
        200,
        "the region the Outpost belongs to serves the cancellation, got {} {}",
        served.status,
        served.body
    );
}

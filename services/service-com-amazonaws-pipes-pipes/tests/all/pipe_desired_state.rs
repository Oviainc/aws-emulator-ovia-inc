// SPDX-License-Identifier: CC0-1.0

//! What `UpdatePipe`'s `DesiredState` DOES to a pipe, end to end over the wire, with IAM
//! and Amazon SQS mounted beside pipes on one engine (the harness of
//! `tests/all/pipe_metrics.rs` and `tests/all/pipe_data_plane.rs`, reused rather than
//! copied).
//!
//! `tests/all/pipe_update.rs` asserts what `UpdatePipe` WRITES and what `DescribePipe`
//! then reports; `tests/all/pipe_data_plane.rs` asserts the run/stop gate through
//! `StopPipe`/`StartPipe`. The two rules here are neither, and both are about the gate
//! being the pipe's own stored state rather than which operation moved it:
//!
//! 1. A pipe consumes its source only while its record rests in `RUNNING`, so
//!    `UpdatePipe(DesiredState)` suspends and resumes that consumption exactly as the
//!    dedicated lifecycle calls do: while the record does not rest in `RUNNING` the target
//!    receives nothing and the backlog stays VISIBLE and counted on the source — withheld,
//!    not consumed and dropped — and the whole of that backlog is delivered once the record
//!    rests in `RUNNING` again.
//! 2. That gate is re-read before every read of the source, not once per batch: a poll
//!    that is still collecting (which it does for as long as
//!    `MaximumBatchingWindowInSeconds` allows) takes nothing more from the source once the
//!    record says the pipe no longer runs, while the records it had already taken are
//!    still delivered, because they entered the pipe while it ran.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::{RUNNING, STOPPED};
use service_com_amazonaws_pipes_pipes::state::requested_state;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, given_queue, send, visible, wait_for_counted,
    wait_for_visible, QUIET,
};
use crate::pipe_execution_role::{wait_for_state, IN_FLIGHT_ATTRIBUTE};
use crate::pipe_lifecycle::{call, describe, Reply, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};
use crate::pipe_update::update;

/// `TargetParameters.InputTemplate`, so a delivered message carries the record's own body
/// and a row can attribute it: `<$.body>` selects the `body` of the record envelope.
const BODY_TEMPLATE: &str = "<$.body>";

/// How long work the pipe has to do is given before a row fails. Well above the poll
/// interval, the state-settle span and every batching window configured below, so a row
/// fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(30);

/// Every Amazon SQS call the pipe below makes, allowed on exactly the resource it is made
/// on: the source is received from, read about and deleted from, and the target is sent to.
fn stage_grant(source_arn: &str, target_arn: &str) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
            "Resource": source_arn,
        },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target_arn },
    ])
}

/// `CreatePipe`: an SQS source, an SQS target, the row's batching parameters and the
/// template that makes a delivered message carry the record's own body.
async fn create_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
    batch_size: Option<i64>,
    window_seconds: Option<i64>,
) -> Reply {
    let mut queue_parameters = serde_json::Map::new();
    if let Some(batch_size) = batch_size {
        queue_parameters.insert("BatchSize".to_string(), json!(batch_size));
    }
    if let Some(window_seconds) = window_seconds {
        queue_parameters.insert(
            "MaximumBatchingWindowInSeconds".to_string(),
            json!(window_seconds),
        );
    }
    let body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "DesiredState": requested_state::RUNNING,
        "SourceParameters": { "SqsQueueParameters": Value::Object(queue_parameters) },
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// `UpdatePipe` carrying the state the caller wants the pipe in — and `RoleArn`, which the
/// request requires — which is how a client that declares `DesiredState` rather than
/// calling `StopPipe`/`StartPipe` toggles a pipe.
async fn toggle(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    desired_state: &str,
) -> Reply {
    update(
        router,
        name,
        json!({ "RoleArn": role_arn, "DesiredState": desired_state }),
    )
    .await
}

/// The markers one row sends: `<prefix>-<case>-0`, `<prefix>-<case>-1`, … — distinct per
/// row and per record, so a delivery can be attributed.
fn markers(prefix: &str, case: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("{prefix}-{case}-{index}"))
        .collect()
}

/// The bodies `delivered` carries, sorted, so a row compares sets rather than an order the
/// rule does not promise.
fn sorted(delivered: &[String]) -> Vec<String> {
    let mut bodies = delivered.to_vec();
    bodies.sort();
    bodies
}

/// One row of the toggle table: the batching parameters the pipe polls under and how many
/// records are sent while it is stopped.
struct Toggle {
    case: &'static str,
    /// `BatchSize`, or `None` for a pipe whose source parameters name none.
    batch_size: Option<i64>,
    /// `MaximumBatchingWindowInSeconds`, or `None` for a poll that waits for nothing.
    window_seconds: Option<i64>,
    /// How many records are sent to the source while the pipe is stopped.
    held: usize,
}

/// RULE: a pipe consumes its source only while its record rests in `RUNNING`, and which
/// call moved it there is nothing to the data plane — so `UpdatePipe(DesiredState=STOPPED)`
/// suspends the source consumption, leaving every record sent afterwards waiting on the
/// source (visible and counted, never consumed and dropped) and the target receiving
/// nothing, and `UpdatePipe(DesiredState=RUNNING)` resumes it and delivers the whole
/// backlog that accumulated.
///
/// The rows differ along what the rule generalises over: the `BatchSize` the poll runs
/// under (one, a few, the parameters' own default), whether a batching window lets a poll
/// span several reads, and how many records are withheld.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_pipe_desired_state_suspends_and_resumes_source_consumption() {
    let rows = [
        Toggle {
            case: "batch-of-one-no-window",
            batch_size: Some(1),
            window_seconds: None,
            held: 1,
        },
        Toggle {
            case: "batch-of-three-one-second-window",
            batch_size: Some(3),
            window_seconds: Some(1),
            held: 3,
        },
        Toggle {
            case: "default-batching-parameters",
            batch_size: None,
            window_seconds: None,
            held: 2,
        },
        Toggle {
            case: "batch-of-ten-two-second-window",
            batch_size: Some(10),
            window_seconds: Some(2),
            held: 4,
        },
    ];

    let router = emulator();
    for row in rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let role = given_role_stating(
            &router,
            &format!("{case}-role"),
            SERVICE_PRINCIPAL,
            stage_grant(&source_arn, &target_arn),
        )
        .await;

        let created = create_pipe(
            &router,
            case,
            &source_arn,
            &target_arn,
            &role,
            row.batch_size,
            row.window_seconds,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let reported = wait_for_state(&router, case, RUNNING, BUDGET).await;
        assert_eq!(
            reported, RUNNING,
            "case {case:?}: the pipe reports {reported}, so it polls nothing"
        );

        // The control: under a record that rests in RUNNING the pipe does move a record,
        // so a row that sees nothing delivered later saw it because the pipe was stopped.
        let control = markers("control", case, 1);
        send(&router, &source_url, &control).await;
        let delivered = collect_at_least(&router, &target_url, control.len(), BUDGET).await;
        assert_eq!(
            sorted(&delivered),
            control,
            "case {case:?}: expected a RUNNING pipe to deliver {control:?} rendered by its \
             input template, got {delivered:?}"
        );

        // Stopped through UpdatePipe's DesiredState, which is what a client declaring the
        // state rather than calling StopPipe sends.
        let stopped = toggle(&router, case, &role, requested_state::STOPPED).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            stopped.status, stopped.body
        );
        assert_eq!(
            stopped.string("DesiredState"),
            requested_state::STOPPED,
            "case {case:?}: UpdatePipe must echo the DesiredState it was asked for, got {}",
            stopped.body
        );
        let reported = wait_for_state(&router, case, STOPPED, BUDGET).await;
        assert_eq!(
            reported, STOPPED,
            "case {case:?}: the pipe reports CurrentState {reported} after the stop"
        );

        // It HOLDS there: the state is the record's own, so re-reading it does not move it.
        tokio::time::sleep(QUIET).await;
        let holding = describe(&router, case).await;
        assert_eq!(
            holding.string("CurrentState"),
            STOPPED,
            "case {case:?}: a stopped pipe must hold CurrentState STOPPED, got {}",
            holding.body
        );
        assert_eq!(
            holding.string("DesiredState"),
            requested_state::STOPPED,
            "case {case:?}: a stopped pipe must report DesiredState STOPPED, got {}",
            holding.body
        );

        // The records sent while it is stopped: nothing is delivered and nothing is
        // consumed, so every one of them is still WAITING on the source.
        let withheld = markers("held", case, row.held);
        send(&router, &source_url, &withheld).await;
        let quiet = collect_during(&router, &target_url, QUIET).await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: a pipe stopped through UpdatePipe must deliver nothing, got {quiet:?}"
        );
        let waiting = visible(&router, &source_url).await;
        assert_eq!(
            waiting,
            i64::try_from(withheld.len()).unwrap_or_default(),
            "case {case:?}: the withheld records must stay visible and counted on the \
             source, {waiting} of {} are",
            withheld.len()
        );

        // And back: the same member resumes the pipe, which then delivers the backlog.
        let resumed = toggle(&router, case, &role, requested_state::RUNNING).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            resumed.status, resumed.body
        );
        assert_eq!(
            resumed.string("DesiredState"),
            requested_state::RUNNING,
            "case {case:?}: UpdatePipe must echo the DesiredState it was asked for, got {}",
            resumed.body
        );
        let reported = wait_for_state(&router, case, RUNNING, BUDGET).await;
        assert_eq!(
            reported, RUNNING,
            "case {case:?}: the pipe reports CurrentState {reported} after the resume"
        );
        let drained = collect_at_least(&router, &target_url, withheld.len(), BUDGET).await;
        assert_eq!(
            sorted(&drained),
            withheld,
            "case {case:?}: expected the whole backlog held while the pipe was stopped, \
             got {drained:?}"
        );
        let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: a drained backlog is deleted from the source, {left} are still \
             waiting"
        );
    }
}

/// One row of the open-window table: a batch size larger than the one record each row
/// sends, so the poll waits its whole window out instead of returning as soon as the batch
/// is full.
struct OpenWindow {
    case: &'static str,
    batch_size: i64,
    /// `MaximumBatchingWindowInSeconds`: how long the poll that took the primer keeps
    /// collecting. Long enough that the stop certainly lands while it is still open.
    window_seconds: i64,
}

impl OpenWindow {
    /// That window as a span, for the wait that lets it close.
    fn window(&self) -> Duration {
        Duration::from_secs(u64::try_from(self.window_seconds).unwrap_or_default())
    }
}

/// RULE: the run gate is the pipe's own record, re-read before EVERY read of the source
/// rather than once per batch, so a poll that is still collecting takes nothing more from
/// the source the moment the record says the pipe no longer runs — a record that arrives
/// after the stop is left waiting on the source even though a poll was in progress when it
/// arrived — while the records that poll had already taken are still delivered, because
/// they entered the pipe while it ran.
///
/// The rows differ along what the rule generalises over: the batch size the poll is
/// collecting towards and how long its batching window keeps it collecting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_a_batching_window_is_open_takes_nothing_more_from_the_source() {
    let rows = [
        OpenWindow {
            case: "ten-record-batch-six-second-window",
            batch_size: 10,
            window_seconds: 6,
        },
        OpenWindow {
            case: "five-record-batch-three-second-window",
            batch_size: 5,
            window_seconds: 3,
        },
    ];

    let router = emulator();
    for row in rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let role = given_role_stating(
            &router,
            &format!("{case}-role"),
            SERVICE_PRINCIPAL,
            stage_grant(&source_arn, &target_arn),
        )
        .await;

        let created = create_pipe(
            &router,
            case,
            &source_arn,
            &target_arn,
            &role,
            Some(row.batch_size),
            Some(row.window_seconds),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let reported = wait_for_state(&router, case, RUNNING, BUDGET).await;
        assert_eq!(
            reported, RUNNING,
            "case {case:?}: the pipe reports {reported}, so it polls nothing"
        );

        // The poll is OPEN once it has taken the primer and not deleted it: the batch is
        // short of its size, so the poll keeps collecting for the whole window.
        let primer = markers("primer", case, 1);
        send(&router, &source_url, &primer).await;
        let taken = wait_for_counted(&router, &source_url, &[IN_FLIGHT_ATTRIBUTE], 1, BUDGET).await;
        assert_eq!(
            taken, 1,
            "case {case:?}: expected the poller to have taken the primer of {source_arn}, \
             in flight {taken}"
        );

        // Stopped while that poll is still collecting.
        let stopped = toggle(&router, case, &role, requested_state::STOPPED).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            stopped.status, stopped.body
        );
        let reported = wait_for_state(&router, case, STOPPED, BUDGET).await;
        assert_eq!(
            reported, STOPPED,
            "case {case:?}: the pipe reports CurrentState {reported} after the stop"
        );

        // Sent while the window the primer opened would still have been collecting.
        let withheld = markers("held", case, 3);
        send(&router, &source_url, &withheld).await;

        // Past the end of that window, plus a quiet span on top: what the target holds is
        // then everything the poll in flight ever delivered.
        tokio::time::sleep(row.window()).await;
        let arrived = collect_during(&router, &target_url, QUIET).await;
        assert_eq!(
            sorted(&arrived),
            primer,
            "case {case:?}: expected only the record the poll had already taken, got {arrived:?}"
        );
        let waiting = visible(&router, &source_url).await;
        assert_eq!(
            waiting,
            i64::try_from(withheld.len()).unwrap_or_default(),
            "case {case:?}: a record arriving after the stop must be left waiting on the \
             source even though a poll was open, {waiting} of {} are",
            withheld.len()
        );

        // Resumed, the backlog the open poll did not take is moved.
        let resumed = toggle(&router, case, &role, requested_state::RUNNING).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: UpdatePipe answered {} {}",
            resumed.status, resumed.body
        );
        let drained = collect_at_least(&router, &target_url, withheld.len(), BUDGET).await;
        assert_eq!(
            sorted(&drained),
            withheld,
            "case {case:?}: expected the records withheld by the stop once the pipe runs \
             again, got {drained:?}"
        );
    }
}

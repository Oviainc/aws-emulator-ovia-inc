// SPDX-License-Identifier: CC0-1.0

//! Lifecycle CONFLICTS: what a pipe answers a `StartPipe`/`StopPipe` that arrives while
//! a move it is already making has not landed.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so what is asserted is the HTTP behaviour of the operations and not a
//! Rust helper.
//!
//! The rule both tables assert: a pipe makes ONE move at a time, so a lifecycle call
//! that lands while a transition is in flight is refused with the modelled
//! `ConflictException` (409) naming the pipe, and the move already in flight still lands
//! — which is what makes exactly one of several simultaneous callers the one that drove
//! the transition.

use std::time::Duration;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{
    call, create, describe, minimal_body, router, Reply, PAST_SETTLE, REGION,
};

/// The source and target the fixture pipes name. Nothing here turns on either resource.
const SOURCE: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const TARGET: &str = "arn:aws:lambda:us-west-2:000000000000:function:handler";

/// How long a wait for the state a move settles into is given before it is a failure.
const SETTLE_BUDGET: Duration = Duration::from_secs(5);

/// How long between two reads while waiting for a move to land.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// One lifecycle call on the pipe named `name`: `suffix` is `start` or `stop`.
async fn lifecycle(router: &MultiServiceRouter, name: &str, suffix: &str) -> Reply {
    call(
        router,
        "POST",
        &format!("/v1/pipes/{name}/{suffix}"),
        None,
        REGION,
    )
    .await
}

/// Wait until the pipe named `name` reports `want` as its `CurrentState`, answering what
/// it reported last.
async fn wait_for_state(router: &MultiServiceRouter, name: &str, want: &str) -> String {
    let deadline = std::time::Instant::now() + SETTLE_BUDGET;
    loop {
        let seen = describe(router, name).await.string("CurrentState");
        if seen == want || std::time::Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row of the conflict table.
struct Conflict {
    /// The case, which is also the pipe's name.
    case: &'static str,
    /// The state the pipe is asked to be in when it is created, so the row starts from a
    /// pipe heading for `RUNNING` or for `STOPPED`.
    desired: &'static str,
    /// Whether the row waits for the create to land before starting its own move: a row
    /// that does not is refused for the CREATING transition itself.
    settle_first: bool,
    /// The call that starts the move in flight, when the row starts one past the create.
    in_flight: Option<&'static str>,
    /// The call that arrives while that move is in flight.
    arriving: &'static str,
    /// The state the pipe rests in once the move in flight has landed.
    lands_in: &'static str,
}

/// The ONE sentence a lifecycle call refused for a move in flight carries, built from
/// the pipe the request named. Recorded for the loser of two concurrent `StartPipe`
/// calls and for an `UpdatePipe` during a transition alike, so it names neither the
/// operation called nor the state the pipe is passing through.
fn conflict_sentence(pipe: &str) -> String {
    format!("Pipe {pipe} is updating concurrently.")
}

/// RULE: a pipe makes one move at a time — a `StartPipe` or `StopPipe` that lands while
/// a transition has not settled is refused with `ConflictException` (409), carrying the
/// one sentence Pipes refuses a conflicting lifecycle call with plus the `resourceId` and
/// `resourceType` the model requires; the transition in flight is unaffected and still
/// lands.
///
/// The rows differ along which transition is in flight (the create itself, a stop, a
/// start) and which call arrives during it, which is what the rule generalises over.
#[tokio::test]
async fn a_lifecycle_call_during_a_move_in_flight_is_refused_with_a_conflict() {
    let rows = [
        Conflict {
            case: "creating-then-start",
            desired: "RUNNING",
            settle_first: false,
            in_flight: None,
            arriving: "start",
            lands_in: "RUNNING",
        },
        Conflict {
            case: "creating-then-stop",
            desired: "RUNNING",
            settle_first: false,
            in_flight: None,
            arriving: "stop",
            lands_in: "RUNNING",
        },
        Conflict {
            case: "starting-then-start",
            desired: "STOPPED",
            settle_first: true,
            in_flight: Some("start"),
            arriving: "start",
            lands_in: "RUNNING",
        },
        Conflict {
            case: "starting-then-stop",
            desired: "STOPPED",
            settle_first: true,
            in_flight: Some("start"),
            arriving: "stop",
            lands_in: "RUNNING",
        },
        Conflict {
            case: "stopping-then-start",
            desired: "RUNNING",
            settle_first: true,
            in_flight: Some("stop"),
            arriving: "start",
            lands_in: "STOPPED",
        },
        Conflict {
            case: "stopping-then-stop",
            desired: "RUNNING",
            settle_first: true,
            in_flight: Some("stop"),
            arriving: "stop",
            lands_in: "STOPPED",
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let mut body = minimal_body(SOURCE, TARGET);
        body["DesiredState"] = json!(row.desired);
        let created = create(&router, case, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the fixture pipe must be created, got {created:?}"
        );
        if row.settle_first {
            tokio::time::sleep(PAST_SETTLE).await;
        }
        if let Some(suffix) = row.in_flight {
            let accepted = lifecycle(&router, case, suffix).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the call that starts the move in flight must be accepted, \
                 got {accepted:?}"
            );
        }

        let refused = lifecycle(&router, case, row.arriving).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException while a move is in flight, \
             got {refused:?}"
        );
        assert_eq!(
            refused.message(),
            conflict_sentence(case),
            "case {case:?}: expected the one sentence naming the pipe, got {refused:?}"
        );
        assert_eq!(
            (
                refused.string("resourceId"),
                refused.string("resourceType")
            ),
            (case.to_string(), "pipe".to_string()),
            "case {case:?}: the conflict must name the resource it is about, got {refused:?}"
        );

        // The refusal changed nothing: the move in flight lands exactly as it would have.
        let landed = wait_for_state(&router, case, row.lands_in).await;
        assert_eq!(
            landed, row.lands_in,
            "case {case:?}: expected the move in flight to land in {}, got {landed:?}",
            row.lands_in
        );
    }
}

/// RULE: of several `StartPipe` calls fired at one resting pipe, exactly ONE is accepted
/// — the first through the gate starts the move, every other is refused with
/// `ConflictException` (409) — and the pipe still lands in `RUNNING` having made that one
/// move.
///
/// The rows differ along how many callers race, which is what "exactly one winner"
/// generalises over.
#[tokio::test]
async fn simultaneous_starts_have_exactly_one_winner() {
    for callers in [2usize, 3, 5] {
        let case = format!("race-{callers}");
        let router = router();
        let mut body = minimal_body(SOURCE, TARGET);
        body["DesiredState"] = json!("STOPPED");
        let created = create(&router, &case, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the fixture pipe must be created, got {created:?}"
        );
        // Resting STOPPED, so every caller below finds a pipe that admits a start.
        tokio::time::sleep(PAST_SETTLE).await;
        assert_eq!(
            describe(&router, &case).await.string("CurrentState"),
            "STOPPED",
            "case {case:?}: the fixture pipe must rest STOPPED before the race"
        );

        let mut replies = Vec::new();
        for _ in 0..callers {
            replies.push(lifecycle(&router, &case, "start").await);
        }
        let accepted: Vec<&Reply> = replies.iter().filter(|reply| reply.status == 200).collect();
        assert_eq!(
            accepted.len(),
            1,
            "case {case:?}: expected exactly one accepted StartPipe out of {callers}, \
             got {replies:?}"
        );
        for refused in replies.iter().filter(|reply| reply.status != 200) {
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (409, "ConflictException"),
                "case {case:?}: every loser must be refused with the modelled conflict, \
                 got {refused:?}"
            );
            assert_eq!(
                refused.message(),
                conflict_sentence(&case),
                "case {case:?}: every loser must be told the one conflict sentence, \
                 got {refused:?}"
            );
        }
        assert_eq!(
            accepted[0].string("CurrentState"),
            "STARTING",
            "case {case:?}: the accepted call reports the transition it started, \
             got {:?}",
            accepted[0]
        );

        let landed = wait_for_state(&router, &case, "RUNNING").await;
        assert_eq!(
            landed, "RUNNING",
            "case {case:?}: the one move the winner started must land in RUNNING, got {landed:?}"
        );
        assert_eq!(
            describe(&router, &case).await.string("DesiredState"),
            "RUNNING",
            "case {case:?}: the pipe the race started must be asked to run"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN an activity task's clock starts and what the scheduling
//! state's `Retry` does with a task that failed, driven over this service's own wire
//! protocol (`awsJson1_0`, `POST /` with `X-Amz-Target: AWSStepFunctions.<Operation>`)
//! through the production `MultiServiceRouter`.
//!
//! The rules under test:
//!
//! 1. an activity task's `TimeoutSeconds` runs from the instant a worker CLAIMED it —
//!    "for Activities, the count begins when GetActivityTask receives a token and
//!    ActivityStarted is logged in the execution event history" — so a task no worker
//!    holds has started nothing, is never timed out however long that span has passed,
//!    and is still there for a later poll;
//! 2. a task that failed — because the clock closed it with `States.Timeout`, or
//!    because the worker holding it reported a failure — is RETRIED by the first
//!    retrier of the scheduling state's `Retry` whose `ErrorEquals` names that error
//!    and still has attempts left: the attempt is closed, its token answers nothing,
//!    and the next attempt is queued for a worker `IntervalSeconds` scaled by
//!    `BackoffRate` once per attempt already taken — capped by `MaxDelaySeconds` —
//!    after the failure, with the execution still `RUNNING`;
//! 3. the error that SURVIVES is the task's own: once the retrier that named it has
//!    spent its `MaxAttempts`, the execution fails with that error and the cause the
//!    failure carried, and a state whose retriers name another error is not retried at
//!    all;
//! 4. `Catch` on a state whose `Resource` is an activity is NOT interpreted: handing a
//!    failure to a catcher continues the walk past the state, which a task the clock
//!    closes has no request to make the states after it from, so such a state is not
//!    run rather than run as though it declared no catcher.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `describe_execution`)
//! is the one `state_machine_lifecycle` builds and the activity helpers are
//! `activity_tasks`'; `tests/all.rs` is this binary's crate root, so both are reached
//! through `crate::<suite>`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{
    activity_retry, RUNTIME_ERROR, TIMEOUT_ERROR,
};

use crate::activity_tasks::{activity_definition, claimed, create_activity, queued_tasks};
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, Emulator, PAST_THE_EXECUTION_DELAY,
};

/// Long enough that a one-second bound a scheduling state declared has run out.
const PAST_A_ONE_SECOND_BOUND: Duration = Duration::from_millis(1_300);

/// The error a worker reports in the rows that drive `SendTaskFailure`.
const REPORTED_ERROR: &str = "WorkerSaidNo";

/// The cause it reports beside it.
const REPORTED_CAUSE: &str = "the worker could not do the work";

/// The status a running execution reports.
const RUNNING: &str = "RUNNING";

/// The status a failed one reports.
const FAILED: &str = "FAILED";

/// `(status, error, cause)` of an execution, as `DescribeExecution` projects them.
async fn outcome(emulator: &Emulator, execution_arn: &str) -> (String, String, String) {
    let described = describe_execution(emulator, execution_arn).await;
    let member = |name: &str| {
        described
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    (member("status"), member("error"), member("cause"))
}

// ---------------------------------------------------------------------------
// Rule 1: the clock starts at the claim
// ---------------------------------------------------------------------------

/// A task no worker has claimed is never timed out, and the same task claimed later
/// times out that span after the CLAIM — so the span is the task's run and not the
/// time since the state scheduled it.
///
/// Rows differ along the bounds the state declares (a timeout alone, a timeout beside
/// a heartbeat window, a longer timeout) and along the input the state carries, so the
/// rule is pinned on more than one bound.
#[tokio::test]
async fn an_unclaimed_activity_task_is_never_timed_out() {
    let emulator = Emulator::new();

    // (case, the bounds the state declares, the execution input)
    let cases: Vec<(&str, Value, &str)> = vec![
        ("TimeoutSeconds alone", json!({ "TimeoutSeconds": 1 }), "{}"),
        (
            "TimeoutSeconds beside a heartbeat window",
            json!({ "TimeoutSeconds": 1, "HeartbeatSeconds": 1 }),
            r#"{"n":1}"#,
        ),
        (
            "a longer TimeoutSeconds",
            json!({ "TimeoutSeconds": 2 }),
            r#"{"who":"nobody"}"#,
        ),
    ];

    for (index, (case, bounds, input)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("unclaimed-{index}")).await;
        let definition = activity_definition(&activity, bounds);
        let machine =
            create_machine(&emulator, &format!("unclaimed-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", input).await;

        // Well past every bound the state declared, with nobody ever polling.
        std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
        std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
        let (status, error, cause) = outcome(&emulator, &execution).await;
        assert_eq!(
            status, RUNNING,
            "case {case:?}: expected the execution to be {RUNNING} with no worker having polled, \
             got {status:?} error {error:?} cause {cause:?}"
        );
        let (held, claimable) = queued_tasks(&emulator, &activity);
        assert_eq!(
            (held, claimable),
            (1, 1),
            "case {case:?}: expected the unclaimed task to still be claimable, got {held} held \
             and {claimable} claimable"
        );

        // Claimed now, the task runs out of its bound from THIS instant.
        claimed(&emulator, case, &activity, Some("late-worker")).await;
        let (status, error, cause) = outcome(&emulator, &execution).await;
        assert_eq!(
            status, RUNNING,
            "case {case:?}: expected the just-claimed task to leave the execution {RUNNING}, got \
             {status:?} error {error:?} cause {cause:?}"
        );
        std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
        std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
        let (status, error, cause) = outcome(&emulator, &execution).await;
        assert_eq!(
            (status.as_str(), error.as_str()),
            (FAILED, TIMEOUT_ERROR),
            "case {case:?}: expected {FAILED} with {TIMEOUT_ERROR} once the claimed task ran out \
             of its bound, got {status:?} error {error:?} cause {cause:?}"
        );
        assert!(
            !cause.is_empty(),
            "case {case:?}: expected a cause saying which bound ran out, got {cause:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: which retrier answers, and how long after the failure
// ---------------------------------------------------------------------------

/// Which retrier of a state's `Retry` answers for an error, and how long after the
/// failure the attempt it owes is due.
///
/// A table over the arithmetic the language publishes for a retrier — "IntervalSeconds
/// (Optional) ... default 1", "MaxAttempts (Optional) ... default 3", "BackoffRate
/// (Optional) ... default 2.0", and `MaxDelaySeconds` as the cap on the product — with
/// a row on each published default, on each attempt up to and one past `MaxAttempts`,
/// on the cap and one past it, and on the errors a retrier does and does not name.
#[test]
fn a_retrier_grows_its_interval_by_backoffrate_and_clamps_at_maxdelayseconds() {
    /// The one state the definitions below declare, whose `Retry` is read.
    const STATE: &str = "Work";

    /// One row: the case, the state's `Retry` array, how many attempts each retrier
    /// has spent, and the retrier and the seconds owed — `None` when nothing is.
    type Row = (&'static str, Value, Vec<u64>, Option<(usize, f64)>);

    // (case, the Retry array, how many attempts each retrier has spent, the retrier
    // and the seconds owed — or None when nothing is)
    let cases: Vec<Row> = vec![
        (
            "the published defaults, first attempt",
            json!([{ "ErrorEquals": ["States.ALL"] }]),
            vec![0],
            Some((0, 1.0)),
        ),
        (
            "the published defaults, second attempt",
            json!([{ "ErrorEquals": ["States.ALL"] }]),
            vec![1],
            Some((0, 2.0)),
        ),
        (
            "the published defaults, third attempt",
            json!([{ "ErrorEquals": ["States.ALL"] }]),
            vec![2],
            Some((0, 4.0)),
        ),
        (
            "the published default MaxAttempts, spent",
            json!([{ "ErrorEquals": ["States.ALL"] }]),
            vec![3],
            None,
        ),
        (
            "an interval and a backoff the state declares",
            json!([{
                "ErrorEquals": [TIMEOUT_ERROR],
                "IntervalSeconds": 3,
                "BackoffRate": 2.0,
                "MaxAttempts": 5,
            }]),
            vec![1],
            Some((0, 6.0)),
        ),
        (
            "MaxDelaySeconds, which the product has not reached",
            json!([{
                "ErrorEquals": [TIMEOUT_ERROR],
                "IntervalSeconds": 3,
                "BackoffRate": 2.0,
                "MaxAttempts": 5,
                "MaxDelaySeconds": 10,
            }]),
            vec![1],
            Some((0, 6.0)),
        ),
        (
            "MaxDelaySeconds, which the product has gone past",
            json!([{
                "ErrorEquals": [TIMEOUT_ERROR],
                "IntervalSeconds": 3,
                "BackoffRate": 2.0,
                "MaxAttempts": 5,
                "MaxDelaySeconds": 10,
            }]),
            vec![2],
            Some((0, 10.0)),
        ),
        (
            "MaxDelaySeconds, every later attempt staying at the cap",
            json!([{
                "ErrorEquals": [TIMEOUT_ERROR],
                "IntervalSeconds": 3,
                "BackoffRate": 2.0,
                "MaxAttempts": 5,
                "MaxDelaySeconds": 10,
            }]),
            vec![4],
            Some((0, 10.0)),
        ),
        (
            "a BackoffRate of 1, which leaves the interval where it is",
            json!([{ "ErrorEquals": ["States.ALL"], "IntervalSeconds": 2, "BackoffRate": 1.0 }]),
            vec![2],
            Some((0, 2.0)),
        ),
        (
            "an interval of zero, which owes the next attempt at once",
            json!([{ "ErrorEquals": ["States.ALL"], "IntervalSeconds": 0 }]),
            vec![0],
            Some((0, 0.0)),
        ),
        (
            "the SECOND retrier, which is the first to name the error",
            json!([
                { "ErrorEquals": ["Lambda.ServiceException"] },
                { "ErrorEquals": [TIMEOUT_ERROR], "IntervalSeconds": 7, "BackoffRate": 1.0 },
            ]),
            vec![0, 0],
            Some((1, 7.0)),
        ),
        (
            "a retrier that names another error",
            json!([{ "ErrorEquals": ["SomethingElse"] }]),
            vec![0],
            None,
        ),
        (
            "no attempt spent yet, counted from an empty record",
            json!([{ "ErrorEquals": ["States.ALL"], "IntervalSeconds": 5, "BackoffRate": 1.0 }]),
            Vec::new(),
            Some((0, 5.0)),
        ),
    ];

    for (case, retry, taken, expected) in cases {
        let definition = json!({
            "StartAt": STATE,
            "States": { STATE: { "Type": "Task", "Resource": "arn:aws:states:us-west-2:000000000000:activity:a", "Retry": retry, "End": true } },
        })
        .to_string();
        let answered = activity_retry(&definition, STATE, TIMEOUT_ERROR, &taken);
        match expected {
            None => assert!(
                answered.is_none(),
                "case {case:?}: expected no retry to be owed, got {answered:?}"
            ),
            Some((retrier, seconds)) => {
                let (index, delay) = answered.unwrap_or_else(|| {
                    panic!("case {case:?}: expected retrier {retrier} to owe {seconds}s, got None")
                });
                assert_eq!(
                    index, retrier,
                    "case {case:?}: expected retrier {retrier}, got {index} owing {delay:?}"
                );
                assert!(
                    (delay.as_secs_f64() - seconds).abs() < 1e-9,
                    "case {case:?}: expected {seconds}s before the next attempt, got {delay:?}"
                );
            }
        }
    }

    // "A States.Runtime error isn't retriable, and will always cause the execution to
    // fail. A retry or catch on States.ALL won't catch States.Runtime errors."
    let definition = json!({
        "StartAt": STATE,
        "States": { STATE: { "Type": "Task", "Resource": "arn:aws:states:us-west-2:000000000000:activity:a", "Retry": [{ "ErrorEquals": ["States.ALL"] }], "End": true } },
    })
    .to_string();
    let answered = activity_retry(&definition, STATE, RUNTIME_ERROR, &[0]);
    assert!(
        answered.is_none(),
        "expected {RUNTIME_ERROR} never to be retried, got {answered:?}"
    );

    // A state that declares no `Retry`, and a state the definition does not declare,
    // owe nothing either.
    let bare = json!({
        "StartAt": STATE,
        "States": { STATE: { "Type": "Task", "Resource": "arn:aws:states:us-west-2:000000000000:activity:a", "End": true } },
    })
    .to_string();
    for (case, state_name) in [("a state with no Retry", STATE), ("an undeclared state", "Gone")] {
        let answered = activity_retry(&bare, state_name, TIMEOUT_ERROR, &[0]);
        assert!(
            answered.is_none(),
            "case {case:?}: expected no retry to be owed, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 2 and 3: the retried task, and the error that survives
// ---------------------------------------------------------------------------

/// A task that failed is re-scheduled while its state's `Retry` owes an attempt, and
/// the execution fails with the task's OWN error once that retrier is spent.
///
/// Rows differ along WHAT failed the task — the clock running out of the state's
/// `TimeoutSeconds`, and a worker reporting a failure with `SendTaskFailure` — along
/// how many attempts the retrier allows, and along whether its `ErrorEquals` names the
/// error at all. Each row drives every attempt through a real poll, so what is
/// asserted is that a worker is handed the retried task.
#[tokio::test]
async fn a_failed_activity_task_is_retried_until_its_retrier_is_spent() {
    let emulator = Emulator::new();

    // (case, whether the clock fails the task, the Retry array, how many claims the
    // whole run hands out, the error and cause the execution must end with)
    let cases: Vec<(&str, bool, Value, usize, &str, &str)> = vec![
        (
            "the clock, retried once",
            true,
            json!([{ "ErrorEquals": [TIMEOUT_ERROR], "IntervalSeconds": 0, "MaxAttempts": 1 }]),
            2,
            TIMEOUT_ERROR,
            "",
        ),
        (
            "the clock, retried twice on the States.ALL wildcard",
            true,
            json!([{ "ErrorEquals": ["States.ALL"], "IntervalSeconds": 0, "MaxAttempts": 2 }]),
            3,
            TIMEOUT_ERROR,
            "",
        ),
        (
            "the clock, with a retrier that names another error",
            true,
            json!([{ "ErrorEquals": ["SomethingElse"], "IntervalSeconds": 0 }]),
            1,
            TIMEOUT_ERROR,
            "",
        ),
        (
            "a worker's reported failure, retried once",
            false,
            json!([{ "ErrorEquals": [REPORTED_ERROR], "IntervalSeconds": 0, "MaxAttempts": 1 }]),
            2,
            REPORTED_ERROR,
            REPORTED_CAUSE,
        ),
        (
            "a worker's reported failure, with a retrier that names another error",
            false,
            json!([{ "ErrorEquals": [TIMEOUT_ERROR], "IntervalSeconds": 0 }]),
            1,
            REPORTED_ERROR,
            REPORTED_CAUSE,
        ),
    ];

    for (index, (case, by_clock, retry, claims, error, cause)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("retried-{index}")).await;
        let mut bounds = json!({ "Retry": retry });
        if by_clock {
            bounds
                .as_object_mut()
                .expect("the bounds are an object")
                .insert("TimeoutSeconds".to_string(), json!(1));
        }
        let definition = activity_definition(&activity, bounds);
        let machine =
            create_machine(&emulator, &format!("retried-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", r#"{"n":1}"#).await;

        let mut tokens: Vec<String> = Vec::new();
        for attempt in 1..=claims {
            let (token, input) = claimed(&emulator, case, &activity, Some("w-1")).await;
            assert_eq!(
                input,
                json!({ "n": 1 }),
                "case {case:?}: expected attempt {attempt} to carry the state's input, got {input}"
            );
            assert!(
                !tokens.contains(&token),
                "case {case:?}: expected attempt {attempt} to carry a fresh taskToken, got one \
                 already handed out"
            );
            tokens.push(token.clone());
            // Every attempt but the last leaves the execution running: the state has
            // not failed while its `Retry` still owes an attempt.
            if by_clock {
                std::thread::sleep(PAST_A_ONE_SECOND_BOUND);
            } else {
                emulator
                    .ok(
                        "SendTaskFailure",
                        &json!({
                            "taskToken": token,
                            "error": REPORTED_ERROR,
                            "cause": REPORTED_CAUSE,
                        }),
                    )
                    .await;
            }
            if attempt < claims {
                let (status, seen_error, seen_cause) = outcome(&emulator, &execution).await;
                assert_eq!(
                    status, RUNNING,
                    "case {case:?}: expected the execution to be {RUNNING} after attempt \
                     {attempt} of {claims}, got {status:?} error {seen_error:?} cause \
                     {seen_cause:?}"
                );
            }
        }

        // A reply under a token whose attempt has been closed resolves nothing: the
        // worker that held it cannot answer for the attempt that replaced it.
        let (status, code) = emulator
            .err(
                "SendTaskSuccess",
                &json!({ "taskToken": tokens[0].clone(), "output": "{}" }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TaskTimedOut"),
            "case {case:?}: expected the closed attempt's token to answer nothing, got {status} \
             {code}"
        );

        // The error that survives the retries is the task's own.
        if !by_clock {
            std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        }
        let (status, seen_error, seen_cause) = outcome(&emulator, &execution).await;
        assert_eq!(
            (status.as_str(), seen_error.as_str()),
            (FAILED, error),
            "case {case:?}: expected {FAILED} with {error}, got {status:?} error {seen_error:?} \
             cause {seen_cause:?}"
        );
        if !cause.is_empty() {
            assert_eq!(
                seen_cause, cause,
                "case {case:?}: expected the cause the worker reported, got {seen_cause:?}"
            );
        } else {
            assert!(
                !seen_cause.is_empty(),
                "case {case:?}: expected a cause saying what ran out, got {seen_cause:?}"
            );
        }
        // Nothing is left queued for a worker once the state has failed.
        let (held, _) = queued_tasks(&emulator, &activity);
        assert_eq!(
            held, 0,
            "case {case:?}: expected no task to be left outstanding, got {held}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: what an activity state's members do NOT include
// ---------------------------------------------------------------------------

/// A definition whose `Task` state polls `activity_arn` and carries `extra`, with the
/// state a `Catch` catcher transitions to declared beside it where `extra` declares
/// one: a definition whose `Next` named no declared state, or that declared a state no
/// transition reaches, would be refused at create time, which is a different refusal
/// from the one under test.
fn definition_with_a_recovery_state(activity_arn: &str, extra: Value) -> String {
    let Value::Object(extra) = extra else {
        panic!("expected an object of state members, got {extra:?}")
    };
    let catching = extra.contains_key("Catch");
    let mut state = json!({ "Type": "Task", "Resource": activity_arn, "End": true });
    let members = state.as_object_mut().expect("the state is an object");
    for (member, value) in extra {
        members.insert(member, value);
    }
    let mut states = json!({ "Work": state });
    if catching {
        states.as_object_mut().expect("the states are an object").insert(
            "Recover".to_string(),
            json!({ "Type": "Pass", "End": true }),
        );
    }
    json!({ "StartAt": "Work", "States": states }).to_string()
}

/// A state whose `Resource` is an activity and which carries a member this emulator
/// does not interpret is not run at all: the execution fails with `States.Runtime`
/// naming that member, rather than running as though the member were absent.
///
/// Rows span the members an activity task state may NOT carry here, so the boundary is
/// pinned where it is rather than only on the one member the interpreter was last
/// taught.
#[tokio::test]
async fn an_activity_state_carrying_a_member_this_emulator_does_not_interpret_is_not_run() {
    let emulator = Emulator::new();

    // (case, the member the state declares)
    let cases: Vec<(&str, Value)> = vec![
        (
            "Catch, which continues the walk past the state",
            json!({ "Catch": [{ "ErrorEquals": ["States.ALL"], "Next": "Recover" }] }),
        ),
        (
            "Credentials, which changes who the work is done as",
            json!({ "Credentials": { "RoleArn": "arn:aws:iam::000000000000:role/Other" } }),
        ),
    ];

    for (index, (case, member)) in cases.into_iter().enumerate() {
        let activity = create_activity(&emulator, &format!("declined-{index}")).await;
        let definition = definition_with_a_recovery_state(&activity, member);
        let machine =
            create_machine(&emulator, &format!("declined-machine-{index}"), &definition).await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;

        std::thread::sleep(PAST_THE_EXECUTION_DELAY);
        let (status, error, cause) = outcome(&emulator, &execution).await;
        assert_eq!(
            (status.as_str(), error.as_str()),
            (FAILED, RUNTIME_ERROR),
            "case {case:?}: expected {FAILED} with {RUNTIME_ERROR}, got {status:?} error \
             {error:?} cause {cause:?}"
        );
        assert!(
            cause.contains("does not") && cause.contains("Work"),
            "case {case:?}: expected a cause naming the state and what was not run, got {cause:?}"
        );
        // Nothing was scheduled for a worker: a state that is not run schedules no
        // task. Read from the store rather than by polling, because a poll with
        // nothing to hand out is HELD open.
        let (held, _) = queued_tasks(&emulator, &activity);
        assert_eq!(
            held, 0,
            "case {case:?}: expected a state that was not run to schedule nothing, got {held} \
             task(s)"
        );
    }
}

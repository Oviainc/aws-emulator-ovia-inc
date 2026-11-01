// SPDX-License-Identifier: CC0-1.0

//! What `StartNextPendingJobExecution` hands a device that polls AFTER a deadline the
//! clock already made due.
//!
//! The rule under test: "pending" is the thing's queue read at the CURRENT instant, so
//! the `TIMED_OUT` transition every armed timer whose deadline has passed already owes
//! is in force before the queue is read ("If the job execution status is not set to a
//! terminal state before this timer expires, or before the timer is reset ... the job
//! execution status will be automatically set to TIMED_OUT" —
//! `StartNextPendingJobExecutionRequest$stepTimeoutInMinutes`, and the same for the
//! timer a command start arms). An execution past its deadline is therefore terminal,
//! hence not pending: the poll neither hands it out nor re-arms it, and the execution
//! waiting BEHIND it is the one started. The move is recorded in the shared store (the
//! IoT control plane reads the same record), stamped at the DEADLINE it was owed at
//! rather than at the poll, with `versionNumber` left alone — the service's own timer
//! is not a device update.
//!
//! The table spans which timer armed the deadline (the command start's
//! `executionTimeoutSeconds`, the step timer a poll armed, both at once, none at all),
//! how far from that deadline the poll arrives (a week past, an hour past, a second
//! past, exactly due, still to come), the stage the execution was in when the deadline
//! passed (`QUEUED`, never taken up, and `IN_PROGRESS`), and whether the poll itself
//! carries a `stepTimeoutInMinutes` — at its published bounds — that would re-arm the
//! timer.
//!
//! Each row's state is reached through REAL requests (a command start, a poll that
//! takes the execution up), with only the passage of time stood in for, so what is
//! asserted is the wire behaviour a device agent sees over restJson1 through the
//! multi-service router.

use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    now_epoch_seconds, FIRST_VERSION_NUMBER, MAX_STEP_TIMEOUT_MINUTES, MIN_STEP_TIMEOUT_MINUTES,
    STATUS_IN_PROGRESS, STATUS_QUEUED,
};
use service_common::iot_job_executions::STATUS_TIMED_OUT;

// The emulator, its HTTP calls and the stored-record readers every suite of this crate
// shares.
use crate::support::{Emulator, StoredExecution};

/// Seconds in one minute: `stepTimeoutInMinutes` is documented in MINUTES while every
/// stored deadline is in seconds since the epoch.
const SECONDS_PER_MINUTE: i64 = 60;

/// One row: a device holding one execution whose timers were armed by its own
/// requests, taken up or not, whose soonest deadline then sits `deadline_in` seconds
/// from the poll under test, with a second execution queued behind it.
struct PollCase {
    case: &'static str,
    /// One device per row, so the rows cannot interfere with each other.
    thing: &'static str,
    /// `executionTimeoutSeconds` the command start arms the head's execution timer
    /// with.
    exec_timeout_seconds: Option<i64>,
    /// When set, the head is TAKEN UP first by a poll carrying this
    /// `stepTimeoutInMinutes` (the inner `None` is a poll that carries none), which is
    /// how a device reaches an `IN_PROGRESS` execution with — or without — a step
    /// timer.
    take_up_with: Option<Option<i64>>,
    /// Where the head's ARMED deadlines then sit, in seconds from the seeding instant:
    /// negative is a deadline already passed, `0` one exactly due, positive one still
    /// to come. A row that arms no timer has none to move.
    deadline_in: i64,
    /// The `stepTimeoutInMinutes` the poll under test carries, if any.
    poll_step_minutes: Option<i64>,
    /// Whether the clock owes the head `TIMED_OUT` by the time that poll arrives.
    head_times_out: bool,
}

impl PollCase {
    /// True when the row's own requests arm a timer on the head at all.
    fn head_arms_a_timer(&self) -> bool {
        self.exec_timeout_seconds.is_some() || matches!(self.take_up_with, Some(Some(_)))
    }

    /// The body of a poll carrying this row's `stepTimeoutInMinutes`, or a poll that
    /// carries no member at all.
    fn poll_body(&self) -> Value {
        step_timeout_body(self.poll_step_minutes)
    }
}

/// A `$next` body carrying `stepTimeoutInMinutes`, or an empty one when the poll sends
/// the member not at all.
fn step_timeout_body(minutes: Option<i64>) -> Value {
    minutes.map_or_else(|| json!({}), |minutes| json!({"stepTimeoutInMinutes": minutes}))
}

/// The table both tests run.
fn poll_cases() -> Vec<PollCase> {
    vec![
        PollCase {
            case: "a queued command execution whose own timer passed an hour ago",
            thing: "laser-queued-timer-past",
            exec_timeout_seconds: Some(60),
            take_up_with: None,
            deadline_in: -3_600,
            poll_step_minutes: Some(10),
            head_times_out: true,
        },
        PollCase {
            case: "a queued command execution whose own timer is exactly due",
            thing: "laser-queued-timer-due",
            exec_timeout_seconds: Some(1),
            take_up_with: None,
            deadline_in: 0,
            poll_step_minutes: None,
            head_times_out: true,
        },
        PollCase {
            case: "an execution a poll took up, its step timer a second past",
            thing: "laser:step-timer-past",
            exec_timeout_seconds: None,
            // The published lower bound of `stepTimeoutInMinutes` (1 to 10080).
            take_up_with: Some(Some(MIN_STEP_TIMEOUT_MINUTES)),
            deadline_in: -1,
            poll_step_minutes: Some(MIN_STEP_TIMEOUT_MINUTES),
            head_times_out: true,
        },
        PollCase {
            case: "both timers armed on an execution in progress, a week past",
            thing: "laser_both-timers-past",
            exec_timeout_seconds: Some(600),
            // The published upper bound of `stepTimeoutInMinutes` (7 days).
            take_up_with: Some(Some(MAX_STEP_TIMEOUT_MINUTES)),
            deadline_in: -604_800,
            poll_step_minutes: Some(MAX_STEP_TIMEOUT_MINUTES),
            head_times_out: true,
        },
        PollCase {
            case: "a queued command execution whose timer is still to come",
            thing: "laser-queued-timer-future",
            exec_timeout_seconds: Some(3_600),
            take_up_with: None,
            deadline_in: 3_600,
            poll_step_minutes: Some(10),
            head_times_out: false,
        },
        PollCase {
            case: "an execution in progress whose step timer is still to come",
            thing: "laser-in-progress-live",
            exec_timeout_seconds: None,
            take_up_with: Some(Some(10)),
            deadline_in: 600,
            poll_step_minutes: Some(10),
            head_times_out: false,
        },
        PollCase {
            case: "an execution in progress with no timer armed at all",
            thing: "laser-in-progress-untimed",
            exec_timeout_seconds: None,
            take_up_with: Some(None),
            deadline_in: 0,
            poll_step_minutes: Some(5),
            head_times_out: false,
        },
        PollCase {
            case: "a queued execution with no timer armed at all",
            thing: "laser-queued-untimed",
            exec_timeout_seconds: None,
            take_up_with: None,
            deadline_in: 0,
            poll_step_minutes: None,
            head_times_out: false,
        },
    ]
}

/// One row's device: the execution at the head of its queue, the one waiting behind it,
/// and both records as the store holds them when the poll under test arrives.
struct Seeded {
    head: String,
    follower: String,
    head_before: StoredExecution,
    follower_before: StoredExecution,
}

/// Put one row's device into the state it describes, through real requests only.
async fn seed(emulator: &Emulator, case: &PollCase) -> Seeded {
    let label = case.case;
    let head = emulator
        .start_command_on(case.thing, case.exec_timeout_seconds)
        .await;
    if let Some(step_minutes) = case.take_up_with {
        // A device takes an execution up — and arms its step timer — by polling for
        // it, which is the operation under test used as the row's own setup.
        let (status, taken) = emulator
            .start_next(case.thing, step_timeout_body(step_minutes))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: taking the head up must answer 200, got {status} {taken}"
        );
        assert_eq!(
            taken["execution"]["jobId"].as_str(),
            Some(head.as_str()),
            "case {label:?}: the setup poll must hand out the head, got {taken}"
        );
    }
    // The execution waiting BEHIND the head: the one a poll must start once the clock
    // has finished the head, and must leave alone while the head is still live. It is
    // enqueued while the head's deadline is still to come, so the poll under test is
    // the first call that meets a deadline already passed.
    let follower = emulator.start_command_on(case.thing, None).await;

    // What a live device reaches by waiting, without a test that sleeps for it: the
    // deadlines the row's own requests armed are moved, and a timer nobody armed stays
    // unarmed.
    let deadline = now_epoch_seconds() + case.deadline_in;
    emulator.with_stored_execution(case.thing, &head, |record| {
        record.step_timeout_expires_at = record.step_timeout_expires_at.map(|_| deadline);
        record.execution_timeout_expires_at = record.execution_timeout_expires_at.map(|_| deadline);
    });

    let head_before = emulator.stored_execution(case.thing, &head);
    let follower_before = emulator.stored_execution(case.thing, &follower);
    assert_eq!(
        head_before.deadline().is_some(),
        case.head_arms_a_timer(),
        "case {label:?}: the row's requests decide whether a timer is armed, got {head_before:?}"
    );
    // Keeps the table honest: the row's own claim about the clock, checked against the
    // deadline the record carries.
    assert_eq!(
        head_before
            .deadline()
            .is_some_and(|deadline| deadline <= now_epoch_seconds()),
        case.head_times_out,
        "case {label:?}: expected the clock to owe TIMED_OUT == {}, got {head_before:?}",
        case.head_times_out
    );
    assert_eq!(
        (
            follower_before.status.as_str(),
            follower_before.version_number
        ),
        (STATUS_QUEUED, FIRST_VERSION_NUMBER),
        "case {label:?}: the execution behind must start out queued, got {follower_before:?}"
    );

    Seeded {
        head,
        follower,
        head_before,
        follower_before,
    }
}

/// RULE for the QUEUED execution a poll STARTS: it moves to `IN_PROGRESS` at the
/// instant of the call, `startedAt` and `lastUpdatedAt` are stamped there, the version
/// advances by one (starting an execution IS a device update), and the step timer the
/// poll carried is armed from that same instant — while a poll carrying no
/// `stepTimeoutInMinutes` leaves the stored timers as they were, and the job's own
/// in-progress timer is never re-armed by a start.
fn assert_started(
    label: &str,
    which: &str,
    before: &StoredExecution,
    after: &StoredExecution,
    poll_step_minutes: Option<i64>,
    called_between: (i64, i64),
) {
    let (earliest, latest) = called_between;
    assert_eq!(
        (after.status.as_str(), after.version_number),
        (STATUS_IN_PROGRESS, before.version_number + 1),
        "case {label:?}: {which} must be started with its version advanced, got {after:?}"
    );
    assert!(
        after
            .started_at
            .is_some_and(|at| (earliest..=latest).contains(&at)),
        "case {label:?}: {which} must be stamped as started within {earliest}..={latest}, got {after:?}"
    );
    assert!(
        (earliest..=latest).contains(&after.last_updated_at),
        "case {label:?}: {which} must be stamped as updated within {earliest}..={latest}, got {after:?}"
    );
    match poll_step_minutes {
        Some(minutes) => {
            let armed = after.step_timeout_expires_at.unwrap_or_else(|| {
                panic!("case {label:?}: {which} must carry the step timer the poll asked for, got {after:?}")
            });
            let from = earliest + minutes * SECONDS_PER_MINUTE;
            let to = latest + minutes * SECONDS_PER_MINUTE;
            assert!(
                (from..=to).contains(&armed),
                "case {label:?}: {which} must arm its step timer within {from}..={to}, got {armed}"
            );
        }
        None => assert_eq!(
            after.step_timeout_expires_at, before.step_timeout_expires_at,
            "case {label:?}: {which}: a poll carrying no timeout must not arm one, got {after:?}"
        ),
    }
    assert_eq!(
        after.execution_timeout_expires_at, before.execution_timeout_expires_at,
        "case {label:?}: {which}: the job's own timer is not re-armed by a start, got {after:?}"
    );
}

/// RULE for the execution a poll resolves to that is ALREADY in progress: the poll
/// starts nothing, so `startedAt` and the job's own in-progress timer stay the first
/// start's, and it writes exactly what it CARRIES — a `stepTimeoutInMinutes` re-arms
/// the step timer from the instant of the call and advances the version (applying a
/// member a device sent is a device update), while a poll carrying no member at all
/// leaves the record exactly as it was.
fn assert_resumed(
    label: &str,
    before: &StoredExecution,
    after: &StoredExecution,
    poll_step_minutes: Option<i64>,
    called_between: (i64, i64),
) {
    let (earliest, latest) = called_between;
    let Some(minutes) = poll_step_minutes else {
        assert_eq!(
            after, before,
            "case {label:?}: a poll carrying nothing to apply must re-hand the execution unchanged, got {after:?}"
        );
        return;
    };
    assert_eq!(
        (
            after.status.as_str(),
            after.version_number,
            after.started_at,
            after.execution_timeout_expires_at
        ),
        (
            STATUS_IN_PROGRESS,
            before.version_number + 1,
            before.started_at,
            before.execution_timeout_expires_at
        ),
        "case {label:?}: a resumed execution keeps its start and advances its version once, got {after:?}"
    );
    assert!(
        (earliest..=latest).contains(&after.last_updated_at),
        "case {label:?}: a resumed execution is stamped as updated within {earliest}..={latest}, got {after:?}"
    );
    let armed = after.step_timeout_expires_at.unwrap_or_else(|| {
        panic!("case {label:?}: the poll's step timeout must arm a timer, got {after:?}")
    });
    let from = earliest + minutes * SECONDS_PER_MINUTE;
    let to = latest + minutes * SECONDS_PER_MINUTE;
    assert!(
        (from..=to).contains(&armed),
        "case {label:?}: a resumed execution's step timer must be re-armed within {from}..={to}, got {armed}"
    );
}

/// RULE: the queue a poll reads is the one the CLOCK has already decided, so an
/// execution whose armed deadline has passed is `TIMED_OUT` — recorded at that
/// deadline, with its version untouched and its timers spent — and the execution
/// waiting behind it is the one started; an execution whose deadline has NOT passed is
/// the one handed out, started if it was queued and resumed — applying what the poll
/// carries — if the device already had it in progress.
#[tokio::test]
async fn a_poll_starts_the_execution_behind_every_deadline_that_has_passed() {
    let emulator = Emulator::new();

    for case in poll_cases() {
        let label = case.case;
        let seeded = seed(&emulator, &case).await;

        let earliest = now_epoch_seconds();
        let (status, answered) = emulator.start_next(case.thing, case.poll_body()).await;
        let latest = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {label:?}: the poll must answer 200, got {status} {answered}"
        );

        let expected = if case.head_times_out {
            &seeded.follower
        } else {
            &seeded.head
        };
        assert_eq!(
            answered["execution"]["jobId"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: expected the poll to hand out {expected}, got {answered}"
        );
        assert_eq!(
            answered["execution"]["status"].as_str(),
            Some(STATUS_IN_PROGRESS),
            "case {label:?}: a handed-out execution is in progress, got {answered}"
        );
        assert!(
            answered["execution"]["jobDocument"].is_string(),
            "case {label:?}: the device needs the job document to run the job, got {answered}"
        );

        let head_after = emulator.stored_execution(case.thing, &seeded.head);
        let follower_after = emulator.stored_execution(case.thing, &seeded.follower);
        if case.head_times_out {
            let deadline = seeded
                .head_before
                .deadline()
                .unwrap_or_else(|| panic!("case {label:?}: a timed-out row arms a timer"));
            assert_eq!(
                head_after,
                StoredExecution {
                    status: STATUS_TIMED_OUT.to_string(),
                    // The service's own timer is not a device update, so the version a
                    // device already holds still names this record.
                    version_number: seeded.head_before.version_number,
                    started_at: seeded.head_before.started_at,
                    // Owed at the deadline, so stamped there and not at this poll.
                    last_updated_at: deadline,
                    // Terminal: the deadlines are spent, so the poll re-armed neither.
                    step_timeout_expires_at: None,
                    execution_timeout_expires_at: None,
                },
                "case {label:?}: the head must be timed out at its deadline {deadline}, got {head_after:?}"
            );
            assert_started(
                label,
                "the execution behind the head",
                &seeded.follower_before,
                &follower_after,
                case.poll_step_minutes,
                (earliest, latest),
            );
        } else {
            if seeded.head_before.status == STATUS_IN_PROGRESS {
                assert_resumed(
                    label,
                    &seeded.head_before,
                    &head_after,
                    case.poll_step_minutes,
                    (earliest, latest),
                );
            } else {
                assert_started(
                    label,
                    "the head",
                    &seeded.head_before,
                    &head_after,
                    case.poll_step_minutes,
                    (earliest, latest),
                );
            }
            assert_eq!(
                follower_after, seeded.follower_before,
                "case {label:?}: the execution behind a live head must stay queued, got {follower_after:?}"
            );
        }

        // `approximateSecondsBeforeTimedOut` is reported from the deadline the
        // handed-out record carries AFTER the poll — nothing is reported for a record
        // with no timer armed.
        let handed = emulator.stored_execution(case.thing, expected);
        let reported = answered["execution"]["approximateSecondsBeforeTimedOut"].as_i64();
        match handed.deadline() {
            Some(deadline) => assert!(
                reported.is_some_and(|seconds| ((deadline - latest)..=(deadline - earliest))
                    .contains(&seconds)),
                "case {label:?}: expected the seconds left on the deadline {deadline}, got {answered}"
            ),
            None => assert_eq!(
                reported, None,
                "case {label:?}: an execution with no timer armed reports no seconds left, got {answered}"
            ),
        }
    }
}

/// RULE, from the symptom's side: a device that keeps polling cannot keep an execution
/// the clock finished alive. Every poll after the deadline answers the same way — the
/// finished execution stays `TIMED_OUT` at its own deadline with its version untouched
/// — and the execution started behind it is resumed, applying whatever that poll
/// carries, rather than started again.
#[tokio::test]
async fn polling_on_cannot_revive_the_execution_the_clock_finished() {
    let emulator = Emulator::new();

    for case in poll_cases().into_iter().filter(|case| case.head_times_out) {
        let label = case.case;
        let seeded = seed(&emulator, &case).await;
        let deadline = seeded
            .head_before
            .deadline()
            .unwrap_or_else(|| panic!("case {label:?}: a timed-out row arms a timer"));

        let mut first_handed: Option<StoredExecution> = None;
        for poll in 1..=3 {
            let (status, answered) = emulator.start_next(case.thing, case.poll_body()).await;
            assert_eq!(
                status, 200,
                "case {label:?}: poll {poll} must answer 200, got {status} {answered}"
            );
            // The first poll starts the execution behind (one device update); a later
            // poll advances it again only when it carries a member to apply.
            let expected_version = seeded.follower_before.version_number
                + if case.poll_step_minutes.is_some() {
                    poll
                } else {
                    1
                };
            assert_eq!(
                (
                    answered["execution"]["jobId"].as_str(),
                    answered["execution"]["versionNumber"].as_i64()
                ),
                (Some(seeded.follower.as_str()), Some(expected_version)),
                "case {label:?}: poll {poll} must hand out the execution behind at version {expected_version}, got {answered}"
            );

            let head_after = emulator.stored_execution(case.thing, &seeded.head);
            assert_eq!(
                (
                    head_after.status.as_str(),
                    head_after.last_updated_at,
                    head_after.version_number
                ),
                (
                    STATUS_TIMED_OUT,
                    deadline,
                    seeded.head_before.version_number
                ),
                "case {label:?}: poll {poll} must leave the finished execution as the clock left it, got {head_after:?}"
            );

            let follower_after = emulator.stored_execution(case.thing, &seeded.follower);
            match &first_handed {
                None => first_handed = Some(follower_after),
                Some(first) => {
                    // `startedAt` and the job's own in-progress timer belong to the
                    // first start, so no later poll may move them.
                    assert_eq!(
                        (
                            follower_after.status.as_str(),
                            follower_after.started_at,
                            follower_after.execution_timeout_expires_at
                        ),
                        (
                            first.status.as_str(),
                            first.started_at,
                            first.execution_timeout_expires_at
                        ),
                        "case {label:?}: poll {poll} must not start the execution again, got {follower_after:?}"
                    );
                    if case.poll_step_minutes.is_none() {
                        assert_eq!(
                            &follower_after, first,
                            "case {label:?}: poll {poll} carries nothing to apply, so it must change nothing, got {follower_after:?}"
                        );
                    }
                }
            }
        }
    }
}

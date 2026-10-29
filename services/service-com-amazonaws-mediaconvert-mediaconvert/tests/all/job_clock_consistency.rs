// SPDX-License-Identifier: CC0-1.0

//! What the clock is still owed from a job, over every path that changes it.
//!
//! The rule every case below is one row of: a job carries a due instant — and the pending
//! index entry that materialises it — for exactly the changes still owed from it, so a job
//! that is no longer waiting in a queue carries NONE, and a job whose queue is `PAUSED`
//! carries no exit from `SUBMITTED` whichever write path put it in that state.
//!
//! It comes from two shapes' own documentation
//! (`smithy_query(query="shape",shape="QueueStatus,HopDestination",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`):
//! `QueueStatus` is "Queues can be ACTIVE or PAUSED. If you pause a queue, jobs in that
//! queue won't begin", so nothing is due from a job a paused queue is holding; and
//! `HopDestination` is "Configuration for a destination queue to which the job can hop once
//! a customer-defined minimum wait time has passed", for a job in "the backlog of the queue
//! that you submit your job to" (`CreateJob$HopDestinations`), so a job that has left that
//! backlog is owed no hop either.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness. Two
//! things are reached past HTTP, both read-only on the suite's side: the CLOCK, because a
//! hop's wait and a job's settle span are durations no suite can sit out (the harness pins
//! the span and `advance` hands `settle` the instant a later reader would arrive at), and
//! the stored record, because a due instant that is owed from nothing is a defect in the
//! STATE whose only symptom a response shows is the one
//! [`a_held_job_waits_its_whole_span_from_the_moment_it_can_begin`] pins.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::JobClock;
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{queue_arn, settings as job_settings, Emulator, ROLE};
use super::queue_hopping::{advance, seed_queues, transitions};
use super::queue_update::update_queue;

/// One minute, the unit `WaitMinutes` is stated in.
const MINUTE: Duration = Duration::from_secs(60);

/// Long enough that no row's job leaves `SUBMITTED` on the strength of the clock: the rows
/// that use it reach a terminal status through `CancelJob` instead.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(100 * 24 * 60 * 60);

/// The queue a row submits its job to when the row is not about a paused queue.
const SOURCE: &str = "clock-src";
/// The queue a row's first hop destination names.
const DESTINATION: &str = "clock-dst";
/// The queue a row's second hop destination names.
const SECOND_DESTINATION: &str = "clock-dst2";
/// A `PAUSED` queue, for the rows about a job its queue is holding.
const PAUSED_SOURCE: &str = "clock-paused";

/// Everything the clock still owes the job `id`, read off the store the handlers write to.
fn job_clock(emulator: &Emulator, id: &str) -> JobClock {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .job_clock(id)
}

/// Assert that nothing at all is owed from the job `id`: no instant on its record, no entry
/// in either pending index, and no hop destination left to use.
fn assert_nothing_due(case: &str, when: &str, clock: JobClock) {
    assert_eq!(
        clock,
        JobClock::default(),
        "case {case:?}: {when}, nothing must be due from the job, got {clock:?}"
    );
}

/// How a row's job reaches a status nothing is due from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    /// The clock takes it out of `SUBMITTED` once its settle span has passed.
    Settle,
    /// `CancelJob` takes it out of `SUBMITTED`.
    Cancel,
}

/// One row of the "a job that is no longer waiting is owed nothing" table.
struct TerminalCase {
    case: &'static str,
    /// The queue the job is submitted to.
    source: &'static str,
    /// The `hopDestinations` list the request carries.
    hops: Value,
    /// The span the row's emulator is pinned to.
    settle_span: Duration,
    /// How the job stops waiting in its queue.
    terminal: Terminal,
    /// How far the clock is advanced before the job is judged.
    advance_by: Duration,
    /// The status the job must report afterwards.
    expected_status: &'static str,
    /// The queue it must be on afterwards, and how many moves it must report.
    expected_queue: &'static str,
    expected_transitions: usize,
}

/// The rows: the class the rule generalises over — which write path makes the job stop
/// waiting, how many hop destinations it still had, how many of them the clock had already
/// used, and whether its queue was holding it back at the time.
fn terminal_cases() -> Vec<TerminalCase> {
    vec![
        TerminalCase {
            case: "leaves SUBMITTED on the clock with no hop destination at all",
            source: SOURCE,
            hops: json!([]),
            settle_span: MINUTE,
            terminal: Terminal::Settle,
            advance_by: 2 * MINUTE,
            expected_status: "ERROR",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "leaves SUBMITTED on the clock with one hop still waiting for its wait",
            source: SOURCE,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 5 }]),
            settle_span: MINUTE,
            terminal: Terminal::Settle,
            advance_by: 2 * MINUTE,
            expected_status: "ERROR",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "leaves SUBMITTED on the clock with two hops still waiting",
            source: SOURCE,
            hops: json!([
                { "queue": DESTINATION, "waitMinutes": 5 },
                { "queue": SECOND_DESTINATION, "waitMinutes": 10 }
            ]),
            settle_span: MINUTE,
            terminal: Terminal::Settle,
            advance_by: 2 * MINUTE,
            expected_status: "ERROR",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "leaves SUBMITTED on the clock after its only hop has been used",
            source: SOURCE,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 1 }]),
            settle_span: 3 * MINUTE,
            terminal: Terminal::Settle,
            advance_by: 4 * MINUTE,
            expected_status: "ERROR",
            expected_queue: DESTINATION,
            expected_transitions: 1,
        },
        TerminalCase {
            case: "leaves SUBMITTED on the clock with its first hop used and its second \
                   still waiting",
            source: SOURCE,
            hops: json!([
                { "queue": DESTINATION, "waitMinutes": 1 },
                { "queue": SECOND_DESTINATION, "waitMinutes": 10 }
            ]),
            settle_span: 3 * MINUTE,
            terminal: Terminal::Settle,
            advance_by: 4 * MINUTE,
            expected_status: "ERROR",
            expected_queue: DESTINATION,
            expected_transitions: 1,
        },
        TerminalCase {
            case: "cancelled with no hop destination at all",
            source: SOURCE,
            hops: json!([]),
            settle_span: HELD_IN_SUBMITTED,
            terminal: Terminal::Cancel,
            advance_by: 2 * MINUTE,
            expected_status: "CANCELED",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "cancelled with one hop still waiting for its wait",
            source: SOURCE,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 1 }]),
            settle_span: HELD_IN_SUBMITTED,
            terminal: Terminal::Cancel,
            advance_by: 2 * MINUTE,
            expected_status: "CANCELED",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "cancelled with two hops still waiting",
            source: SOURCE,
            hops: json!([
                { "queue": DESTINATION, "waitMinutes": 1 },
                { "queue": SECOND_DESTINATION, "waitMinutes": 10 }
            ]),
            settle_span: HELD_IN_SUBMITTED,
            terminal: Terminal::Cancel,
            advance_by: 12 * MINUTE,
            expected_status: "CANCELED",
            expected_queue: SOURCE,
            expected_transitions: 0,
        },
        TerminalCase {
            case: "cancelled while its PAUSED queue was holding it",
            source: PAUSED_SOURCE,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 1 }]),
            settle_span: HELD_IN_SUBMITTED,
            terminal: Terminal::Cancel,
            advance_by: 2 * MINUTE,
            expected_status: "CANCELED",
            expected_queue: PAUSED_SOURCE,
            expected_transitions: 0,
        },
    ]
}

/// A job that is no longer waiting in a queue is owed nothing further: no exit from
/// `SUBMITTED`, no hop, and no entry in either pending index.
#[tokio::test]
async fn a_job_that_stops_waiting_is_owed_nothing_further() {
    for (index, row) in terminal_cases().iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::with_settle_span(row.settle_span);
        seed_queues(
            &emulator,
            &[
                (SOURCE, false),
                (DESTINATION, false),
                (SECOND_DESTINATION, false),
                (PAUSED_SOURCE, true),
            ],
        )
        .await;

        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": row.source,
                "hopDestinations": row.hops.clone(),
                "settings": job_settings(
                    &format!("s3://example-bucket/in/clock-{index}.mp4"),
                    "_k",
                ),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");

        match row.terminal {
            Terminal::Settle => advance(&emulator, row.advance_by),
            Terminal::Cancel => {
                let cancelled = emulator.cancel_job(&id).await;
                assert_eq!(
                    cancelled.status, 202,
                    "case {case:?}: expected CancelJob HTTP 202, got {} with body {}",
                    cancelled.status, cancelled.body
                );
                advance(&emulator, row.advance_by);
            }
        }

        let fetched = emulator.get_job(&id).await;
        assert_eq!(
            fetched.string("/job/status"),
            row.expected_status,
            "case {case:?}: expected status {}, got {}",
            row.expected_status,
            fetched.body
        );
        assert_eq!(
            fetched.at("/job/queue"),
            &json!(queue_arn(row.expected_queue)),
            "case {case:?}: expected the job on queue {:?}, got {}",
            row.expected_queue,
            fetched.body
        );
        // The moves the job DID make are history, and retiring its clock keeps them.
        assert_eq!(
            transitions(&fetched).len(),
            row.expected_transitions,
            "case {case:?}: expected {} recorded move(s), got {:?}",
            row.expected_transitions,
            transitions(&fetched)
        );
        assert_nothing_due(
            case,
            "once it has stopped waiting in a queue",
            job_clock(&emulator, &id),
        );

        // Past every wait the job's hop destinations could have asked for: a job owed
        // nothing stays where it is, and no later clock pass finds anything to do.
        advance(&emulator, 30 * MINUTE);
        let later = emulator.get_job(&id).await;
        assert_eq!(
            later.at("/job"),
            fetched.at("/job"),
            "case {case:?}: a later clock pass must change nothing, got {}",
            later.body
        );
        assert_nothing_due(case, "after a later clock pass", job_clock(&emulator, &id));
    }
}

/// One row of the "a held job is owed no exit from `SUBMITTED`" table.
struct HeldCase {
    case: &'static str,
    /// Whether the queue is created `PAUSED`.
    paused_at_create: bool,
    /// Whether the queue is paused by `UpdateQueue` before the job is submitted.
    paused_before_submit: bool,
    /// The `hopDestinations` list the request carries.
    hops: Value,
    /// Whether a hop is owed: one entry per hop list that states a wait.
    expected_pending_hops: usize,
}

/// The rows: how the queue came to be `PAUSED` — the thing the rule generalises over — plus
/// the hop list, which a pause does not touch, and an `ACTIVE` control that shows the
/// instant IS armed when nothing holds the job back.
fn held_cases() -> Vec<HeldCase> {
    vec![
        HeldCase {
            case: "created PAUSED, no hop destination",
            paused_at_create: true,
            paused_before_submit: false,
            hops: json!([]),
            expected_pending_hops: 0,
        },
        HeldCase {
            case: "created PAUSED, one hop destination",
            paused_at_create: true,
            paused_before_submit: false,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 5 }]),
            expected_pending_hops: 1,
        },
        HeldCase {
            case: "paused by UpdateQueue before the submit, one hop destination",
            paused_at_create: false,
            paused_before_submit: true,
            hops: json!([{ "queue": DESTINATION, "waitMinutes": 5 }]),
            expected_pending_hops: 1,
        },
        HeldCase {
            case: "paused by UpdateQueue before the submit, two hop destinations",
            paused_at_create: false,
            paused_before_submit: true,
            hops: json!([
                { "queue": DESTINATION, "waitMinutes": 5 },
                { "queue": SECOND_DESTINATION, "waitMinutes": 10 }
            ]),
            expected_pending_hops: 1,
        },
    ]
}

/// A job submitted to a `PAUSED` queue is stored owing nothing: "jobs in that queue won't
/// begin", so no instant for its exit from `SUBMITTED` and no `pending` entry — and the
/// queue going `ACTIVE` is what arms one, so nothing is lost.
///
/// The hops of a held job are untouched, because a job a queue is holding is still waiting
/// in that queue, which is exactly what queue hopping is for.
#[tokio::test]
async fn a_job_in_a_paused_queue_is_owed_no_exit_from_submitted() {
    for (index, row) in held_cases().iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::with_settle_span(MINUTE);
        let name = format!("held-clock-{index}");
        seed_queues(
            &emulator,
            &[
                (name.as_str(), row.paused_at_create),
                (DESTINATION, false),
                (SECOND_DESTINATION, false),
            ],
        )
        .await;
        if row.paused_before_submit {
            let paused = update_queue(&emulator, &name, json!({ "status": "PAUSED" })).await;
            assert_eq!(
                paused.status, 200,
                "case {case:?}: the queue must pause, got {} with body {}",
                paused.status, paused.body
            );
        }

        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": name.clone(),
                "hopDestinations": row.hops.clone(),
                "settings": job_settings(
                    &format!("s3://example-bucket/in/held-clock-{index}.mp4"),
                    "_q",
                ),
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = created.string("/job/id");

        let clock = job_clock(&emulator, &id);
        assert_eq!(
            (clock.settles_at.is_some(), clock.pending_settles),
            (false, 0),
            "case {case:?}: a job its queue is holding must carry no due instant for its \
             exit from SUBMITTED, got {clock:?}"
        );
        assert_eq!(
            (clock.hops_at.is_some(), clock.pending_hops),
            (row.expected_pending_hops > 0, row.expected_pending_hops),
            "case {case:?}: a held job is still owed its hops, got {clock:?}"
        );

        // A control job on an ACTIVE queue, identical but for its queue: it IS armed, so
        // the rows above assert a gate and not a clock that arms nothing.
        let control_queue = format!("{name}-active");
        seed_queues(&emulator, &[(control_queue.as_str(), false)]).await;
        let control = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": control_queue,
                "hopDestinations": row.hops.clone(),
                "settings": job_settings(
                    &format!("s3://example-bucket/in/control-clock-{index}.mp4"),
                    "_q",
                ),
            }))
            .await;
        assert_eq!(
            control.status, 201,
            "case {case:?}: the control job must be created, got {} with body {}",
            control.status, control.body
        );
        let control_clock = job_clock(&emulator, &control.string("/job/id"));
        assert_eq!(
            (
                control_clock.settles_at.is_some(),
                control_clock.pending_settles
            ),
            (true, 1),
            "case {case:?}: a job on an ACTIVE queue must be owed its exit from SUBMITTED, \
             got {control_clock:?}"
        );

        // However long the clock has had, a held job has not begun.
        advance(&emulator, 2 * MINUTE);
        let held = emulator.get_job(&id).await;
        assert_eq!(
            held.string("/job/status"),
            "SUBMITTED",
            "case {case:?}: a job in a PAUSED queue must stay SUBMITTED, got {}",
            held.body
        );

        // Activating the queue is what arms the instant the submit did not: the job begins,
        // so nothing was lost by storing it owing nothing.
        let resumed = update_queue(&emulator, &name, json!({ "status": "ACTIVE" })).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: the queue must activate, got {} with body {}",
            resumed.status, resumed.body
        );
        advance(&emulator, 2 * MINUTE);
        let begun = emulator.get_job(&id).await;
        assert_ne!(
            begun.string("/job/status"),
            "SUBMITTED",
            "case {case:?}: once its queue is ACTIVE the job must leave SUBMITTED, got {}",
            begun.body
        );
        assert_nothing_due(
            case,
            "once the released job has left SUBMITTED",
            job_clock(&emulator, &id),
        );
    }
}

/// The one response difference the rule makes: a job its queue was holding waits its WHOLE
/// span from the moment it reaches a queue that lets it begin, because it was never owed
/// that exit while it was held — the time its queue spent paused does not count towards it.
#[tokio::test]
async fn a_held_job_waits_its_whole_span_from_the_moment_it_can_begin() {
    // Ten minutes, so the hop (one minute) lands well inside the span and the span's end
    // can be read on either side of the hop.
    let span = 10 * MINUTE;
    let emulator = Emulator::with_settle_span(span);
    seed_queues(&emulator, &[("span-paused", true), ("span-active", false)]).await;

    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": "span-paused",
            "hopDestinations": [{ "queue": "span-active", "waitMinutes": 1 }],
            "settings": job_settings("s3://example-bucket/in/span.mp4", "_w"),
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture job must be created, got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");

    // The hop's wait has passed: the job is on a queue that lets it begin, and has only
    // just got there.
    advance(&emulator, 2 * MINUTE);
    let hopped = emulator.get_job(&id).await;
    assert_eq!(
        hopped.at("/job/queue"),
        &json!(queue_arn("span-active")),
        "case a job released by a hop: it must hop to the ACTIVE queue, got {}",
        hopped.body
    );
    assert_eq!(
        hopped.string("/job/status"),
        "SUBMITTED",
        "case a job released by a hop: it has only just reached a queue that lets it \
         begin, got {}",
        hopped.body
    );

    // Past the span counted from the SUBMIT, but not from the hop: the job has not begun,
    // because nothing was ever due from it while its queue held it.
    advance(&emulator, span + Duration::from_secs(30));
    let waiting = emulator.get_job(&id).await;
    assert_eq!(
        waiting.string("/job/status"),
        "SUBMITTED",
        "case a job released by a hop: its span runs from the hop, not from the submit, \
         got {}",
        waiting.body
    );

    // Past the span counted from the hop: it begins, and then nothing is due from it.
    advance(&emulator, MINUTE + span + Duration::from_secs(30));
    let begun = emulator.get_job(&id).await;
    assert_ne!(
        begun.string("/job/status"),
        "SUBMITTED",
        "case a job released by a hop: a span past the hop it must have begun, got {}",
        begun.body
    );
    assert_nothing_due(
        "a job released by a hop",
        "once the whole span past its hop has passed",
        job_clock(&emulator, &id),
    );
}

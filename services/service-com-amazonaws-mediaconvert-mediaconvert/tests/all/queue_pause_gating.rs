// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the one control a queue's `Status` gives a client over the
//! jobs in it.
//!
//! The rule under test is `QueueStatus`' own documentation
//! (`smithy_query(query="shape",shape="QueueStatus")`): "Queues can be ACTIVE or PAUSED.
//! If you pause a queue, jobs in that queue won't begin. Jobs that are running when you
//! pause a queue continue to run until they finish or result in an error." So a job's exit
//! from `SUBMITTED` is gated on its queue being `ACTIVE`: a job in a `PAUSED` queue stays
//! `SUBMITTED` however long the clock has had, however the queue came to be paused, and
//! leaves `SUBMITTED` only once the queue is `ACTIVE` again.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see; the
//! `PUT` helper is the one `queue_update` already defines for this operation.

use std::time::Duration;

use serde_json::json;

use super::job_lifecycle::{settings, Emulator, ROLE};
use super::queue_update::update_queue;

/// The settle span most rows configure their emulator with: short, because they only ever
/// have to wait past it.
///
/// The due instant is stored and never slept for by a handler, so a span only ever costs
/// this suite its own waits.
const GATED_SETTLE: Duration = Duration::from_millis(300);

/// The span for the one row that pauses the queue AFTER submitting the job to it: that row
/// has to get its `UpdateQueue` in before the job's due instant, and `CreateJob` probes the
/// job's inputs before it answers, so the window is made wide enough that a loaded machine
/// cannot lose the race (which would fail the row instead of exercising the gate).
const GATED_SETTLE_AFTER_SUBMIT: Duration = Duration::from_secs(2);

/// How long past a row's settle span this suite waits before judging the gate.
fn past(settle: Duration) -> Duration {
    settle + Duration::from_millis(500)
}

/// A job's status, as `GetJob` reports it.
async fn job_status(emulator: &Emulator, id: &str) -> String {
    emulator.get_job(id).await.string("/job/status")
}

/// Submit one job to `queue` and return its id.
async fn submit(emulator: &Emulator, queue: &str, input: &str, modifier: &str) -> String {
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": queue,
            "settings": settings(input, modifier)
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "a fixture job for queue {queue:?} must be created, got {} with body {}",
        created.status, created.body
    );
    created.string("/job/id")
}

/// A job in a `PAUSED` queue is still `SUBMITTED` after the clock has had long enough for
/// an identical job on an `ACTIVE` queue to leave it, and leaves `SUBMITTED` only once the
/// queue is `ACTIVE` again.
///
/// The rows span HOW a queue comes to be paused, which is what the rule generalises over:
/// created paused, paused by `UpdateQueue` after the job was submitted, and paused by
/// `UpdateQueue` before the job was submitted at all.
#[tokio::test]
async fn a_job_in_a_paused_queue_stays_submitted_until_the_queue_is_active() {
    for (label, name, paused_at_create, pause_before_submit, settle) in [
        ("created PAUSED", "hold-born-paused", true, false, GATED_SETTLE),
        (
            "paused after the job was submitted",
            "hold-paused-after",
            false,
            false,
            GATED_SETTLE_AFTER_SUBMIT,
        ),
        (
            "paused before the job was submitted",
            "hold-paused-before",
            false,
            true,
            GATED_SETTLE,
        ),
    ] {
        // One emulator per row, each with the span that row needs: the row that pauses
        // after the submit gets a window it cannot lose, the others get a short one.
        let emulator = Emulator::with_settle_span(settle);
        let past_the_settle = past(settle);
        let mut request = json!({ "name": name });
        if paused_at_create {
            request["status"] = json!("PAUSED");
        }
        let created = emulator.create_queue(request).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            created.status, created.body
        );
        if pause_before_submit {
            let paused = update_queue(&emulator, name, json!({ "status": "PAUSED" })).await;
            assert_eq!(
                paused.status, 200,
                "case {label:?}: the queue must pause, got {} with body {}",
                paused.status, paused.body
            );
        }

        let held = submit(&emulator, name, "s3://example-bucket/in/held.mp4", "_held").await;
        if !paused_at_create && !pause_before_submit {
            let paused = update_queue(&emulator, name, json!({ "status": "PAUSED" })).await;
            assert_eq!(
                paused.status, 200,
                "case {label:?}: the queue must pause, got {} with body {}",
                paused.status, paused.body
            );
        }

        // The control job is identical but for its queue, which is ACTIVE, so it is what
        // says the clock has had long enough.
        let control_queue = format!("{name}-active");
        let created = emulator
            .create_queue(json!({ "name": control_queue.clone() }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the control queue must be created, got {} with body {}",
            created.status, created.body
        );
        let control = submit(
            &emulator,
            &control_queue,
            "s3://example-bucket/in/held.mp4",
            "_held",
        )
        .await;

        // The due instant is stored, not slept for by a handler; this suite waits past it.
        tokio::time::sleep(past_the_settle).await;

        assert_ne!(
            job_status(&emulator, &control).await,
            "SUBMITTED",
            "case {label:?}: the control job on an ACTIVE queue must leave SUBMITTED"
        );
        assert_eq!(
            job_status(&emulator, &held).await,
            "SUBMITTED",
            "case {label:?}: a job in a PAUSED queue must stay SUBMITTED"
        );
        // Waiting longer changes nothing: the gate is the queue's status, not the clock.
        tokio::time::sleep(past_the_settle).await;
        assert_eq!(
            job_status(&emulator, &held).await,
            "SUBMITTED",
            "case {label:?}: a job in a PAUSED queue must stay SUBMITTED however long it \
             waits"
        );
        // A paused queue reports its held job as SUBMITTED, which is what a client reads
        // the backlog from.
        let queue = emulator.get_queue(name).await;
        assert_eq!(
            queue.at("/queue/submittedJobsCount"),
            &json!(1),
            "case {label:?}: the paused queue must still count its held job, got {}",
            queue.body
        );

        let resumed = update_queue(&emulator, name, json!({ "status": "ACTIVE" })).await;
        assert_eq!(
            resumed.status, 200,
            "case {label:?}: the queue must activate, got {} with body {}",
            resumed.status, resumed.body
        );
        tokio::time::sleep(past_the_settle).await;
        assert_ne!(
            job_status(&emulator, &held).await,
            "SUBMITTED",
            "case {label:?}: once its queue is ACTIVE the held job must leave SUBMITTED"
        );
    }
}

/// A job held by a paused queue is still cancellable: it never began, so the one thing a
/// client can do with it is take it out of the queue — and activating the queue afterwards
/// does not resurrect a job that is already terminal.
#[tokio::test]
async fn a_job_held_by_a_paused_queue_can_still_be_cancelled() {
    let emulator = Emulator::with_settle_span(GATED_SETTLE);
    let created = emulator
        .create_queue(json!({ "name": "hold-and-cancel", "status": "PAUSED" }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );
    let id = submit(
        &emulator,
        "hold-and-cancel",
        "s3://example-bucket/in/held.mp4",
        "_held",
    )
    .await;
    tokio::time::sleep(past(GATED_SETTLE)).await;

    let cancelled = emulator.cancel_job(&id).await;
    assert_eq!(
        cancelled.status, 202,
        "a held job must be cancellable, got {} with body {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        job_status(&emulator, &id).await,
        "CANCELED",
        "the cancelled job must read back CANCELED"
    );

    let resumed = update_queue(&emulator, "hold-and-cancel", json!({ "status": "ACTIVE" })).await;
    assert_eq!(
        resumed.status, 200,
        "the queue must activate, got {} with body {}",
        resumed.status, resumed.body
    );
    tokio::time::sleep(past(GATED_SETTLE)).await;
    assert_eq!(
        job_status(&emulator, &id).await,
        "CANCELED",
        "a terminal job must stay terminal when its queue is activated"
    );
}

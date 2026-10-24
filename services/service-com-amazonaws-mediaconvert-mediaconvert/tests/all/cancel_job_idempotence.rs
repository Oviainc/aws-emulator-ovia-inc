// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CancelJob`'s behaviour on a job that is already
//! terminal.
//!
//! The rule under test: `CancelJob` is RETRY-SAFE on the state it produces. A call that
//! asks for a job to be `CANCELED` when the job is already `CANCELED` has nothing to do,
//! so it succeeds again with the operation's own success status and changes nothing about
//! the job — while a job in a terminal status the cancellation can no longer reach
//! conflicts with the request (`ConflictException`, 409) and a job this account does not
//! hold is `NotFoundException` (404).
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::json;

use super::job_lifecycle::{settings, Emulator, PAST_THE_SETTLE, ROLE, SHORT_SETTLE};

/// Re-cancelling a `CANCELED` job succeeds every time and leaves the job exactly as the
/// first call left it.
///
/// The rows span what the rule generalises over: how many repeats are sent, which queue
/// the job is in, and whether a read of the job came between the calls — none of which
/// is what the answer depends on.
#[tokio::test]
async fn re_cancelling_a_canceled_job_is_an_idempotent_success() {
    let emulator = Emulator::new();
    let created = emulator.create_queue(json!({ "name": "idem-queue" })).await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );

    for (label, queue, repeats, read_between) in [
        ("default queue, one repeat", "Default", 1usize, false),
        ("custom queue, one repeat", "idem-queue", 1, false),
        ("custom queue, five repeats", "idem-queue", 5, false),
        ("custom queue, a read between each repeat", "idem-queue", 3, true),
    ] {
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings("s3://example-bucket/in/a.mp4", "_a")
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "case {label:?}: the fixture job must be created, got {} with body {}",
            job.status, job.body
        );
        let id = job.string("/job/id");

        let first = emulator.cancel_job(&id).await;
        assert_eq!(
            first.status, 202,
            "case {label:?}: the first CancelJob must succeed, got {} with body {}",
            first.status, first.body
        );
        let settled = emulator.get_job(&id).await;
        assert_eq!(
            settled.string("/job/status"),
            "CANCELED",
            "case {label:?}: the job must be CANCELED, got {}",
            settled.body
        );
        let canceled_job = settled.at("/job").clone();

        for repeat in 1..=repeats {
            let again = emulator.cancel_job(&id).await;
            assert_eq!(
                again.status, 202,
                "case {label:?}: repeat #{repeat} must succeed, got {} with body {}",
                again.status, again.body
            );
            assert_eq!(
                again.error_type, None,
                "case {label:?}: repeat #{repeat} must carry no error, got {:?}",
                again.error_type
            );
            assert_eq!(
                again.body,
                json!({}),
                "case {label:?}: repeat #{repeat} answers CancelJob's empty output, got {}",
                again.body
            );
            if read_between {
                let between = emulator.get_job(&id).await;
                assert_eq!(
                    between.at("/job"),
                    &canceled_job,
                    "case {label:?}: repeat #{repeat} must change nothing about the job, \
                     got {}",
                    between.body
                );
            }
        }

        // Nothing moved: the job a later caller reads is byte for byte the one the first
        // cancellation produced, finish time included.
        let after = emulator.get_job(&id).await;
        assert_eq!(
            after.at("/job"),
            &canceled_job,
            "case {label:?}: the repeats must leave the stored job untouched, got {}",
            after.body
        );
    }
}

/// A job in a terminal status the cancellation can no longer reach still conflicts, so
/// the idempotent repeat is the `CANCELED` state and not every terminal state.
#[tokio::test]
async fn cancelling_a_job_in_another_terminal_status_still_conflicts() {
    // A settle span this suite can wait past, so the job reaches its own terminal status
    // rather than being cancelled into one.
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": settings("s3://example-bucket/in/a.mp4", "_a")
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "the fixture job must be created, got {} with body {}",
        job.status, job.body
    );
    let id = job.string("/job/id");

    // The due instant is stored, not slept for by a handler; this suite waits past it.
    tokio::time::sleep(PAST_THE_SETTLE).await;
    let settled = emulator.get_job(&id).await;
    let status = settled.string("/job/status");
    assert_ne!(
        status, "CANCELED",
        "the job must reach a terminal status of its own, got {}",
        settled.body
    );
    assert_ne!(
        status, "SUBMITTED",
        "the job must have left SUBMITTED, got {}",
        settled.body
    );

    for attempt in 1..=2 {
        let reply = emulator.cancel_job(&id).await;
        assert_eq!(
            reply.status, 409,
            "attempt #{attempt}: a job in {status} must conflict, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ConflictException"),
            "attempt #{attempt}: expected ConflictException, got {:?}",
            reply.error_type
        );
    }
    assert_eq!(
        emulator.get_job(&id).await.string("/job/status"),
        status,
        "a refused cancellation must leave the job's status alone"
    );
}

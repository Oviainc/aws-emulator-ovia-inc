// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteQueue`.
//!
//! `DeleteQueue` is documented as "Permanently delete a queue you have created", so the
//! rule under test is: a `DELETE` on a stored custom queue removes it from the store —
//! every later read of that name is `NotFoundException` — while a name the store does
//! not hold is `NotFoundException` and a queue the caller did not create or that still
//! owes work is `ConflictException`. Nothing else in the store moves.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use crate::job_lifecycle::{arn, settings, Emulator, Reply, PAST_THE_SETTLE, ROLE, SHORT_SETTLE};

/// `DELETE /2017-08-29/queues/{Name}` — the binding `smithy_query(query="operation",
/// operation="DeleteQueue")` reports for this operation.
async fn delete_queue(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("DELETE", &format!("/2017-08-29/queues/{name}"), None)
        .await
}

/// One queue to create, delete and fail to read back again.
struct Case {
    label: &'static str,
    name: &'static str,
    request: Value,
}

/// The rows span what a caller can vary about the queue being deleted: which optional
/// members it carries, both `PricingPlan` values (so the `RESERVED` row exercises a queue
/// that owns a reservation plan), both `QueueStatus` values, whether it is tagged, and
/// the shape of the name itself.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the required Name",
            name: "minimal",
            request: json!({ "name": "minimal" }),
        },
        Case {
            label: "described and tagged",
            name: "described-and-tagged",
            request: json!({
                "name": "described-and-tagged",
                "description": "a queue with every optional scalar set",
                "concurrentJobs": 3,
                "maximumConcurrentFeeds": 0,
                "tags": { "team": "media", "keyOnly": "" }
            }),
        },
        Case {
            label: "paused on-demand",
            name: "paused",
            request: json!({ "name": "paused", "status": "PAUSED", "pricingPlan": "ON_DEMAND" }),
        },
        Case {
            // The row the issue did not cite: a reserved queue, which carries a
            // ReservationPlan the on-demand rows do not.
            label: "reserved with a reservation plan",
            name: "reserved",
            request: json!({
                "name": "reserved",
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "EXPIRE",
                    "reservedSlots": 1
                }
            }),
        },
        Case {
            label: "a long mixed-case name",
            name: "A-Queue-Name_That.Is-Rather-Longer-0123456789",
            request: json!({ "name": "A-Queue-Name_That.Is-Rather-Longer-0123456789" }),
        },
    ]
}

/// A deleted queue is gone from the store: the delete is accepted with the modelled 202
/// and an empty body, every later read of that name is not found, and a second delete of
/// the same name is not found too. The queues that were not deleted are untouched.
#[tokio::test]
async fn delete_queue_removes_the_queue_and_a_later_read_cannot_find_it() {
    let emulator = Emulator::new();
    let cases = cases();

    for case in &cases {
        let created = emulator.create_queue(case.request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: the queue must be created, got {} with body {}",
            case.label, created.status, created.body
        );
        let read = emulator.get_queue(case.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the queue must read back before the delete, got {} with body {}",
            case.label, read.status, read.body
        );
    }

    for (index, case) in cases.iter().enumerate() {
        let deleted = delete_queue(&emulator, case.name).await;
        assert_eq!(
            deleted.status, 202,
            "case {:?}: expected HTTP 202, got {} with body {}",
            case.label, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {:?}: DeleteQueue models no output members, got body {}",
            case.label, deleted.body
        );

        let gone = emulator.get_queue(case.name).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: the deleted queue must be gone, got {} with body {}",
            case.label, gone.status, gone.body
        );
        assert_eq!(
            gone.error_type.as_deref(),
            Some("NotFoundException"),
            "case {:?}: expected NotFoundException, got {:?}",
            case.label,
            gone.error_type
        );

        let again = delete_queue(&emulator, case.name).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: a second delete has nothing to delete, got {} with body {}",
            case.label, again.status, again.body
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("NotFoundException"),
            "case {:?}: expected NotFoundException on the second delete, got {:?}",
            case.label,
            again.error_type
        );

        // Deleting one queue removes exactly that queue: every row not deleted yet, and
        // the built-in queue, still read back.
        for survivor in cases.iter().skip(index + 1) {
            let still_there = emulator.get_queue(survivor.name).await;
            assert_eq!(
                still_there.status, 200,
                "case {:?}: deleting {:?} must not touch it, got {} with body {}",
                survivor.label, case.label, still_there.status, still_there.body
            );
        }
        let built_in = emulator.get_queue("Default").await;
        assert_eq!(
            built_in.status, 200,
            "case {:?}: the built-in queue must survive, got {} with body {}",
            case.label, built_in.status, built_in.body
        );
    }
}

/// `DeleteQueue` deletes "a queue you have created", and `Queue`'s `Type` member says
/// "System queues are built in. You can't modify or delete system queues" — so the
/// built-in queue is refused with the code AWS is recorded answering for a system
/// resource, repeatedly, and it is still there afterwards.
#[tokio::test]
async fn a_built_in_system_queue_cannot_be_deleted() {
    let emulator = Emulator::new();

    // The name the state module publishes for the built-in queue, so this cannot drift.
    let system = service_com_amazonaws_mediaconvert_mediaconvert::state::DEFAULT_QUEUE_NAME;

    for attempt in 1..=2 {
        let refused = delete_queue(&emulator, system).await;
        assert_eq!(
            refused.status, 403,
            "case attempt {attempt}: expected HTTP 403 for the system queue, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ForbiddenException"),
            "case attempt {attempt}: expected ForbiddenException, got {:?}",
            refused.error_type
        );
        // The wording names the request member the delete read, not the queue's name.
        assert_eq!(
            refused.string("/message"),
            service_com_amazonaws_mediaconvert_mediaconvert::state::system_resource_delete_message(
                "name"
            ),
            "case attempt {attempt}: the refusal must name the request member, got {}",
            refused.body
        );
        let still_there = emulator.get_queue(system).await;
        assert_eq!(
            still_there.string("/queue/type"),
            "SYSTEM",
            "case attempt {attempt}: the system queue must survive the refusal, got {}",
            still_there.body
        );
    }
}

/// A name the store does not hold is not found — whatever the name looks like, including
/// one that differs from a stored queue only in case, since queue names are the stored
/// keys and not folded.
#[tokio::test]
async fn deleting_a_name_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator.create_queue(json!({ "name": "alpha" })).await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );

    for name in [
        "no-such-queue",
        "ALPHA",
        "Alpha",
        "alpha-",
        "a-queue-with-a-much-longer-name",
    ] {
        let refused = delete_queue(&emulator, name).await;
        assert_eq!(
            refused.status, 404,
            "case {name:?}: expected HTTP 404, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: expected NotFoundException, got {:?}",
            refused.error_type
        );
        // The wording names the REQUEST MEMBER the lookup read, so it is the same for
        // every row however the supplied name differs — it is not built from the value.
        assert_eq!(
            refused.string("/message"),
            service_com_amazonaws_mediaconvert_mediaconvert::state::not_found_message("name"),
            "case {name:?}: the refusal must name the request member, got {}",
            refused.body
        );
    }

    // None of those refusals removed the queue that IS stored.
    let survivor = emulator.get_queue("alpha").await;
    assert_eq!(
        survivor.status, 200,
        "the stored queue must survive every refusal, got {} with body {}",
        survivor.status, survivor.body
    );
}

/// A queue that still holds jobs in a non-terminal status conflicts with the delete:
/// removing it would leave those jobs' `Queue` naming a queue no read can resolve. Once
/// every one of them is terminal the same delete is accepted, and the jobs themselves
/// stay readable with the queue they recorded.
#[tokio::test]
async fn a_queue_holding_non_terminal_jobs_cannot_be_deleted_until_they_are_terminal() {
    // Jobs are held in SUBMITTED for the whole case, so this is not a race with the clock.
    let emulator = Emulator::new();
    let created = emulator.create_queue(json!({ "name": "busy" })).await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );

    let mut ids = Vec::new();
    for (index, modifier) in ["_a", "_b", "_c"].iter().enumerate() {
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": "busy",
                "settings": settings("s3://example-bucket/in/a.mp4", modifier)
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "case job #{index}: the fixture job must be created, got {} with body {}",
            job.status, job.body
        );
        ids.push(job.string("/job/id"));
    }

    // One row per outstanding job: while any of them is still non-terminal the delete is
    // refused, so the guard is on the set of live jobs and not on "the queue has jobs".
    for (index, id) in ids.iter().enumerate() {
        let refused = delete_queue(&emulator, "busy").await;
        assert_eq!(
            refused.status, 409,
            "case {} job(s) still submitted: expected HTTP 409, got {} with body {}",
            ids.len() - index,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ConflictException"),
            "case {} job(s) still submitted: expected ConflictException, got {:?}",
            ids.len() - index,
            refused.error_type
        );
        let canceled = emulator.cancel_job(id).await;
        assert_eq!(
            canceled.status, 202,
            "case job {id:?}: the fixture cancel must be accepted, got {} with body {}",
            canceled.status, canceled.body
        );
    }

    let deleted = delete_queue(&emulator, "busy").await;
    assert_eq!(
        deleted.status, 202,
        "with every job terminal the queue must delete, got {} with body {}",
        deleted.status, deleted.body
    );
    let gone = emulator.get_queue("busy").await;
    assert_eq!(
        gone.status, 404,
        "the deleted queue must be gone, got {} with body {}",
        gone.status, gone.body
    );

    // The delete removed the queue and nothing else: each job still reads back, naming
    // the queue it was submitted to.
    for id in &ids {
        let job = emulator.get_job(id).await;
        assert_eq!(
            job.status, 200,
            "case job {id:?}: a job outlives its queue, got {} with body {}",
            job.status, job.body
        );
        assert_eq!(
            job.string("/job/queue"),
            arn("queues/busy"),
            "case job {id:?}: the job keeps the queue it recorded, got {}",
            job.body
        );
    }
}

/// The conflict is decided on the job statuses the clock has already made due, not on
/// whichever status was last written: a job that has reached its terminal status with
/// nobody polling it no longer blocks the delete.
#[tokio::test]
async fn a_queue_becomes_deletable_once_the_clock_settles_its_jobs() {
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let created = emulator.create_queue(json!({ "name": "settling" })).await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );
    let job = emulator
        .create_job(json!({
            "role": ROLE,
            "queue": "settling",
            "settings": settings("s3://example-bucket/in/a.mp4", "_a")
        }))
        .await;
    assert_eq!(
        job.status, 201,
        "the fixture job must be created, got {} with body {}",
        job.status, job.body
    );
    let id = job.string("/job/id");

    // The due instant is stored, never slept for by a handler; this case waits past it
    // and reads NOTHING in between, so the delete is the first call to settle the job.
    tokio::time::sleep(PAST_THE_SETTLE).await;

    let deleted = delete_queue(&emulator, "settling").await;
    assert_eq!(
        deleted.status, 202,
        "a queue whose jobs are already due to be terminal must delete, got {} with body {}",
        deleted.status, deleted.body
    );
    let gone = emulator.get_queue("settling").await;
    assert_eq!(
        gone.status, 404,
        "the deleted queue must be gone, got {} with body {}",
        gone.status, gone.body
    );
    let settled = emulator.get_job(&id).await;
    assert_eq!(
        settled.string("/job/status"),
        "ERROR",
        "the job the delete settled must have left SUBMITTED, got {}",
        settled.body
    );
}

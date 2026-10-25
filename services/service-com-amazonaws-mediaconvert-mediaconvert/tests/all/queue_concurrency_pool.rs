// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ACCOUNTING behind a queue's concurrency: the pool a
//! request's `ConcurrentJobs` is allocated out of, and the refusal a request that cannot
//! be allocated is answered with.
//!
//! The rule: an on-demand queue's `ConcurrentJobs` is an allocation out of the Region's
//! concurrent-job pool — "Specify the maximum number of jobs your queue can process
//! concurrently. For on-demand queues, the value you enter is constrained by your service
//! quotas for Maximum concurrent jobs, per on-demand queue and Maximum concurrent jobs,
//! per account", and `ListQueues$UnallocatedConcurrentJobs` is "the remaining number of
//! concurrent jobs that are not associated with a queue and are available to allocate to a
//! queue. You can allocate these jobs when you create or update a queue"
//! (`smithy_query(query="doc",shape="CreateQueueRequest",member="ConcurrentJobs")` and
//! `smithy_query(query="doc",shape="ListQueuesResponse",member="UnallocatedConcurrentJobs")`).
//! So: a value that is not a number of jobs at all is refused with
//! `BadRequestException` (400), a value past what that pool has left for the queue is
//! refused with `ServiceQuotaExceededException` (402), each refusal states the pool the
//! listing reports, a refused request changes neither the stored queue nor the pool, and
//! the pool an accepted allocation is measured against is what the queues stored SO FAR
//! have taken out of it.
//!
//! The sibling suite `queue_capacity_validation` asserts the status and error shape of a
//! single queue's allocation against a fresh account's pool; what this suite adds is the
//! WORDING of each refusal and the pool arithmetic ACROSS queues — several on-demand
//! queues draining one pool, a reserved queue whose plan buys its capacity instead of
//! drawing from it, a deletion giving capacity back, and a queue restating the allocation
//! it already holds.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    ConcurrencyRefusal, DEFAULT_CONCURRENT_JOBS, TOTAL_CONCURRENT_JOBS,
};

use super::job_lifecycle::{Emulator, Reply};
use super::queue_update::update_queue;

/// `GET /2017-08-29/queues` — the account's queues beside the Region's concurrent-job
/// pool.
async fn list_queues(emulator: &Emulator) -> Reply {
    emulator.send("GET", "/2017-08-29/queues", None).await
}

/// `DELETE /2017-08-29/queues/{Name}`, which gives a queue's allocation back to the pool.
async fn delete_queue(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("DELETE", &format!("/2017-08-29/queues/{name}"), None)
        .await
}

/// What a fresh account has left to allocate: the Region's ceiling less the allocation of
/// the built-in on-demand queue every account carries. Arithmetic on the crate's own
/// constants, never a restated number.
fn fresh_pool() -> i32 {
    TOTAL_CONCURRENT_JOBS - DEFAULT_CONCURRENT_JOBS
}

/// `ListQueues` must report the Region's ceiling and `expected` left to allocate.
async fn assert_pool(emulator: &Emulator, label: &str, expected: i32) {
    let listed = list_queues(emulator).await;
    assert_eq!(
        listed.at("/totalConcurrentJobs"),
        &json!(TOTAL_CONCURRENT_JOBS),
        "case {label:?}: expected the Region's ceiling {TOTAL_CONCURRENT_JOBS}, got {}",
        listed.body
    );
    assert_eq!(
        listed.at("/unallocatedConcurrentJobs"),
        &json!(expected),
        "case {label:?}: expected {expected} concurrent jobs left to allocate, got {}",
        listed.body
    );
}

/// The status, `x-amzn-errortype` and `message` a refusal must answer with.
fn assert_refusal(label: &str, reply: &Reply, refusal: ConcurrencyRefusal) {
    let (expected_status, expected_error) = match refusal {
        ConcurrencyRefusal::Negative(_) => (400, "BadRequestException"),
        ConcurrencyRefusal::Unavailable { .. } => (402, "ServiceQuotaExceededException"),
    };
    assert_eq!(
        reply.status, expected_status,
        "case {label:?}: expected HTTP {expected_status}, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(expected_error),
        "case {label:?}: expected {expected_error}, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        refusal.message(),
        "case {label:?}: expected the refusal to state the rule, got {}",
        reply.body
    );
}

/// A `CreateQueue` body allocating `entered` concurrent jobs to `name`.
fn on_demand(name: &str, entered: i32) -> Value {
    json!({ "name": name, "concurrentJobs": entered })
}

/// A reserved queue's `CreateQueue` body, reserving `slots` transcode slots. `Commitment`
/// models exactly one value (`smithy_query(query="shape",shape="Commitment")`).
fn reserved(name: &str, slots: i32) -> Value {
    json!({
        "name": name,
        "pricingPlan": "RESERVED",
        "reservationPlanSettings": {
            "commitment": "ONE_YEAR",
            "renewalType": "EXPIRE",
            "reservedSlots": slots
        }
    })
}

/// Each refusal states the pool the listing reports, on the create that states an
/// allocation and on the update that states one alike, and leaves both the stored queues
/// and the pool exactly as they were.
///
/// The rows span the two ways an allocation is refused and the sizes each takes: one job
/// below the floor of an allocation, the extreme of the member's `__integer` type, one job
/// past what the pool has left, the whole Region's ceiling, and an allocation orders of
/// magnitude past it.
#[tokio::test]
async fn a_refused_allocation_states_the_pool_the_listing_reports() {
    // What the pool has left for the queue each row works on: a fresh account's remainder,
    // because the queue being created holds nothing yet and the queue being updated is
    // measured without its OWN allocation.
    let available = fresh_pool();
    // The allocation the updated queue already holds, well inside the pool so that no row
    // is refused for the fixture's sake.
    let stored = 300;
    let cases = [
        ("one job below the floor", -1, ConcurrencyRefusal::Negative(-1)),
        (
            "the negative allocation the report carried",
            -5,
            ConcurrencyRefusal::Negative(-5),
        ),
        (
            "the bottom of the member's type",
            i32::MIN,
            ConcurrencyRefusal::Negative(i32::MIN),
        ),
        (
            "one job past what the pool has left",
            available + 1,
            ConcurrencyRefusal::Unavailable {
                requested: available + 1,
                available,
            },
        ),
        (
            "the whole Region's ceiling",
            TOTAL_CONCURRENT_JOBS,
            ConcurrencyRefusal::Unavailable {
                requested: TOTAL_CONCURRENT_JOBS,
                available,
            },
        ),
        (
            "far past the ceiling",
            1_000_000,
            ConcurrencyRefusal::Unavailable {
                requested: 1_000_000,
                available,
            },
        ),
    ];

    for (label, entered, refusal) in cases {
        // The CREATE that states it: nothing is stored and the pool is untouched.
        let emulator = Emulator::new();
        let created = emulator.create_queue(on_demand("pool", entered)).await;
        assert_refusal(label, &created, refusal);
        let fetched = emulator.get_queue("pool").await;
        assert_eq!(
            fetched.status, 404,
            "case {label:?} via CreateQueue: a refused create must store nothing, got {} \
             with body {}",
            fetched.status, fetched.body
        );
        assert_pool(&emulator, label, available).await;

        // The UPDATE that states it, on a queue already holding an allocation: the stored
        // queue survives untouched and keeps the pool down by exactly what it holds.
        let emulator = Emulator::new();
        let before = emulator.create_queue(on_demand("pool", stored)).await;
        assert_eq!(
            before.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            before.status, before.body
        );
        let updated = update_queue(&emulator, "pool", json!({ "concurrentJobs": entered })).await;
        assert_refusal(label, &updated, refusal);
        let fetched = emulator.get_queue("pool").await;
        assert_eq!(
            fetched.at("/queue"),
            before.at("/queue"),
            "case {label:?} via UpdateQueue: a refused update must change nothing, got {}",
            fetched.body
        );
        assert_pool(&emulator, label, available - stored).await;
    }
}

/// The pool an allocation is measured against is what the queues stored so far have taken
/// out of it: one account's on-demand queues drain one pool, a reserved queue's slots are
/// bought by its plan rather than drawn from the pool, and deleting a queue gives its
/// allocation back.
#[tokio::test]
async fn an_allocation_is_measured_against_what_the_other_queues_took() {
    let emulator = Emulator::new();
    let available = fresh_pool();
    assert_pool(&emulator, "a fresh account", available).await;

    // The first queue takes well over half of what is left.
    let first = 500;
    let created = emulator.create_queue(on_demand("first", first)).await;
    assert_eq!(
        created.status, 201,
        "case \"the first allocation\": expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    assert_pool(&emulator, "the first allocation", available - first).await;

    // A second queue asking for more than the REMAINDER is refused, even though the same
    // request would have been served against the fresh pool.
    let remainder = available - first;
    let refused = emulator.create_queue(on_demand("second", first)).await;
    assert_refusal(
        "a second allocation past the remainder",
        &refused,
        ConcurrencyRefusal::Unavailable {
            requested: first,
            available: remainder,
        },
    );
    assert_eq!(
        emulator.get_queue("second").await.status,
        404,
        "case \"a second allocation past the remainder\": a refused create must store nothing"
    );
    assert_pool(&emulator, "a second allocation past the remainder", remainder).await;

    // Exactly the remainder is served, and drains the pool.
    let drained = emulator.create_queue(on_demand("second", remainder)).await;
    assert_eq!(
        drained.status, 201,
        "case \"exactly the remainder\": expected HTTP 201, got {} with body {}",
        drained.status, drained.body
    );
    assert_pool(&emulator, "exactly the remainder", 0).await;

    // A RESERVED queue's capacity is bought by its reservation plan — the pool the listing
    // reports is "across all of your on-demand queues" — so a large plan neither draws
    // from the drained pool nor is refused by it.
    let slots = 900;
    let plan = emulator.create_queue(reserved("reserved", slots)).await;
    assert_eq!(
        plan.status, 201,
        "case \"a reserved queue past the drained pool\": expected HTTP 201, got {} with \
         body {}",
        plan.status, plan.body
    );
    assert_eq!(
        plan.at("/queue/concurrentJobs"),
        &json!(slots),
        "case \"a reserved queue past the drained pool\": expected the plan's {slots} slots \
         to be the queue's concurrency, got {}",
        plan.body
    );
    assert_pool(&emulator, "a reserved queue past the drained pool", 0).await;

    // With the pool drained, a single job cannot be allocated — but zero jobs, the floor of
    // an allocation, still can.
    let starved = emulator.create_queue(on_demand("third", 1)).await;
    assert_refusal(
        "a single job out of a drained pool",
        &starved,
        ConcurrencyRefusal::Unavailable {
            requested: 1,
            available: 0,
        },
    );
    let zero = emulator.create_queue(on_demand("third", 0)).await;
    assert_eq!(
        zero.status, 201,
        "case \"no jobs at all out of a drained pool\": expected HTTP 201, got {} with body {}",
        zero.status, zero.body
    );
    assert_pool(&emulator, "no jobs at all out of a drained pool", 0).await;

    // Deleting an on-demand queue gives its allocation back, so the pool is derived from
    // the queues that are stored rather than from a running total.
    let deleted = delete_queue(&emulator, "first").await;
    assert_eq!(
        deleted.status, 202,
        "case \"a deleted queue gives its allocation back\": expected HTTP 202, got {} with \
         body {}",
        deleted.status, deleted.body
    );
    assert_pool(&emulator, "a deleted queue gives its allocation back", first).await;
    let reused = emulator.create_queue(on_demand("fourth", first)).await;
    assert_eq!(
        reused.status, 201,
        "case \"the freed allocation is allocatable again\": expected HTTP 201, got {} with \
         body {}",
        reused.status, reused.body
    );
    assert_pool(&emulator, "the freed allocation is allocatable again", 0).await;
}

/// A queue restating its own allocation is measured against the pool ONCE: the capacity it
/// already holds is not counted against it, but every other queue's is.
#[tokio::test]
async fn a_queue_restating_its_allocation_is_measured_against_the_pool_once() {
    let emulator = Emulator::new();
    let available = fresh_pool();
    let created = emulator.create_queue(on_demand("solo", available)).await;
    assert_eq!(
        created.status, 201,
        "case \"a queue holding the whole remainder\": expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    assert_pool(&emulator, "a queue holding the whole remainder", 0).await;

    // Restating the allocation it already holds is served, even though the pool reports
    // nothing left: what the queue holds is not taken from it twice.
    let restated = update_queue(&emulator, "solo", json!({ "concurrentJobs": available })).await;
    assert_eq!(
        restated.status, 200,
        "case \"restating the allocation it holds\": expected HTTP 200, got {} with body {}",
        restated.status, restated.body
    );
    assert_eq!(
        restated.at("/queue/concurrentJobs"),
        &json!(available),
        "case \"restating the allocation it holds\": expected {available} concurrent jobs, \
         got {}",
        restated.body
    );
    assert_pool(&emulator, "restating the allocation it holds", 0).await;

    // One job past it is still past the pool.
    let refused = update_queue(&emulator, "solo", json!({ "concurrentJobs": available + 1 })).await;
    assert_refusal(
        "one job past the allocation it holds",
        &refused,
        ConcurrencyRefusal::Unavailable {
            requested: available + 1,
            available,
        },
    );

    // Lowering it gives the difference back to the pool.
    let lowered = 300;
    let shrunk = update_queue(&emulator, "solo", json!({ "concurrentJobs": lowered })).await;
    assert_eq!(
        shrunk.status, 200,
        "case \"lowering the allocation\": expected HTTP 200, got {} with body {}",
        shrunk.status, shrunk.body
    );
    assert_pool(&emulator, "lowering the allocation", available - lowered).await;

    // Another queue takes part of what was freed, and from then on the first queue is
    // measured against what that one took.
    let other = 400;
    let second = emulator.create_queue(on_demand("other", other)).await;
    assert_eq!(
        second.status, 201,
        "case \"another queue takes the freed capacity\": expected HTTP 201, got {} with \
         body {}",
        second.status, second.body
    );
    let left_for_solo = available - other;
    let regrow = update_queue(
        &emulator,
        "solo",
        json!({ "concurrentJobs": left_for_solo + 1 }),
    )
    .await;
    assert_refusal(
        "regrowing past what the other queue left",
        &regrow,
        ConcurrencyRefusal::Unavailable {
            requested: left_for_solo + 1,
            available: left_for_solo,
        },
    );
    let regrow = update_queue(
        &emulator,
        "solo",
        json!({ "concurrentJobs": left_for_solo }),
    )
    .await;
    assert_eq!(
        regrow.status, 200,
        "case \"regrowing to what the other queue left\": expected HTTP 200, got {} with \
         body {}",
        regrow.status, regrow.body
    );
    assert_pool(&emulator, "regrowing to what the other queue left", 0).await;
}

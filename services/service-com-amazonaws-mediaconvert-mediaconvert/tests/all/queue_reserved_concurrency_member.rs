// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of a `ConcurrentJobs` a request states for a RESERVED queue.
//!
//! The rule: which value a queue's concurrency comes from is decided by its PRICING PLAN.
//! A reserved queue buys its capacity by the reserved transcode slots of its plan
//! (`smithy_query(query="doc",shape="ReservationPlan",member="ReservedSlots")`: "the
//! number of reserved transcode slots (RTS) for this queue. The number of RTS determines
//! how many jobs the queue can process in parallel; each RTS can process one job at a
//! time"), and the model points a caller who states the member at the plan instead
//! (`smithy_query(query="doc",shape="CreateQueueRequest",member="ConcurrentJobs")`: "For
//! reserved queues, specify the number of jobs you can process concurrently in your
//! reservation plan instead";
//! `smithy_query(query="doc",shape="UpdateQueueRequest",member="ConcurrentJobs")`: "For
//! reserved queues, update your reservation plan instead in order to increase your yearly
//! commitment"). So the member DOES NOT APPLY to a reserved queue: whatever the request
//! enters, such a queue reports the slot count of the plan it holds, and the two members
//! of one projected `Queue` agree through every reader. An on-demand queue has no plan, so
//! its concurrency is the allocation the request states.
//!
//! The sibling suite `queue_reserved_concurrency` covers the same rule for the calls that
//! state NO `ConcurrentJobs`; this one is about the calls that state one beside — or
//! instead of — a plan. What a plan is allowed to say is `queue_capacity_validation`'s
//! rule, and what an on-demand allocation is measured against is
//! `queue_concurrency_pool`'s.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see. The
//! `ListQueues` reader, the plan body and the listing lookup are the sibling suite's, so
//! the two suites project a queue through exactly the same calls.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    DEFAULT_CONCURRENT_JOBS, MIN_RESERVED_SLOTS, TOTAL_CONCURRENT_JOBS,
};

use super::job_lifecycle::{Emulator, Reply};
use super::queue_reserved_concurrency::{list_queues, listed, plan};
use super::queue_update::update_queue;

/// The queue every row works on.
const QUEUE: &str = "reserved";

/// The concurrency and the slot count the queue projects through the create's own
/// response, `GetQueue` and `ListQueues` alike, asserted to be the same pair everywhere.
async fn assert_projected(
    emulator: &Emulator,
    label: &str,
    created: &Reply,
    expected_slots: i32,
) {
    let fetched = emulator.get_queue(QUEUE).await;
    assert_eq!(
        fetched.status, 200,
        "case {label:?}: expected HTTP 200 from GetQueue, got {} with body {}",
        fetched.status, fetched.body
    );
    let listing = list_queues(emulator).await;
    let from_listing = Reply {
        status: listing.status,
        error_type: None,
        body: json!({ "queue": listed(&listing, QUEUE) }),
    };

    for (source, reply) in [
        ("the call itself", created),
        ("GetQueue", &fetched),
        ("ListQueues", &from_listing),
    ] {
        let actual = reply.at("/queue/concurrentJobs");
        assert_eq!(
            actual,
            &json!(expected_slots),
            "case {label:?} via {source}: expected the plan's {expected_slots} slots as \
             concurrentJobs, got {actual}"
        );
        let slots = reply.at("/queue/reservationPlan/reservedSlots");
        assert_eq!(
            slots,
            &json!(expected_slots),
            "case {label:?} via {source}: expected reservationPlan.reservedSlots \
             {expected_slots}, got {slots}"
        );
        assert_eq!(
            reply.at("/queue/concurrentJobs"),
            reply.at("/queue/reservationPlan/reservedSlots"),
            "case {label:?} via {source}: a reserved queue's concurrency must be the slot \
             count its plan reserves, got {}",
            reply.body
        );
    }
}

/// One create of a reserved queue that states both the plan and the member.
struct CreateCase {
    label: &'static str,
    /// `ReservationPlanSettings$ReservedSlots` the create states.
    slots: i32,
    /// The `ConcurrentJobs` the create states beside it.
    entered: i32,
}

/// The rows span what the rule generalises over: the RELATION between the stated member
/// and the plan's slot count (far above, just above, equal, below, zero), the size of the
/// plan (the published floor, a moderate count, a large one), and a member past the whole
/// Region's on-demand pool — which no reserved queue draws from.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the member far above the plan",
            slots: 2,
            entered: 5,
        },
        CreateCase {
            label: "the member one above the plan",
            slots: MIN_RESERVED_SLOTS,
            entered: MIN_RESERVED_SLOTS + 1,
        },
        CreateCase {
            label: "the member restating the plan's own count",
            slots: 7,
            entered: 7,
        },
        CreateCase {
            label: "the member below the plan",
            slots: 40,
            entered: 3,
        },
        CreateCase {
            label: "the member asking for no concurrency at all",
            slots: 6,
            entered: 0,
        },
        CreateCase {
            label: "the member past the whole Region's on-demand pool",
            slots: 250,
            entered: TOTAL_CONCURRENT_JOBS + 1,
        },
        CreateCase {
            label: "the member orders of magnitude past the plan",
            slots: MIN_RESERVED_SLOTS,
            entered: 999_999,
        },
    ]
}

/// A `CreateQueue` that states `ConcurrentJobs` for a reserved queue reports the plan's
/// slot count, not the value it entered — through the create, `GetQueue` and `ListQueues`.
#[tokio::test]
async fn a_created_reserved_queue_ignores_the_concurrency_the_request_states() {
    for case in create_cases() {
        let CreateCase {
            label,
            slots,
            entered,
        } = case;
        // One account per row, so a row only ever reads its own mutation.
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({
                "name": QUEUE,
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": plan("EXPIRE", slots),
                "concurrentJobs": entered
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: expected HTTP 201 from CreateQueue, got {} with body {}",
            created.status, created.body
        );
        assert_projected(&emulator, label, &created, slots).await;
    }
}

/// One update of a reserved queue that states `ConcurrentJobs`.
struct UpdateCase {
    label: &'static str,
    /// The slot count the create's plan reserves.
    created_slots: i32,
    /// The `UpdateQueue` body, which always states `ConcurrentJobs`.
    update: Value,
    /// The slot count — and so the concurrency — the queue must report afterwards.
    expected_slots: i32,
}

/// The rows span whether the update carries a plan at all, whether that plan raises or
/// restates the count, and how the stated member relates to it (above, below, zero, far
/// past the Region's pool).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "the member alone, far above the standing plan",
            created_slots: 2,
            update: json!({ "concurrentJobs": 999_999 }),
            expected_slots: 2,
        },
        UpdateCase {
            label: "the member alone, below the standing plan",
            created_slots: 30,
            update: json!({ "concurrentJobs": 1 }),
            expected_slots: 30,
        },
        UpdateCase {
            label: "the member alone, asking for no concurrency at all",
            created_slots: MIN_RESERVED_SLOTS,
            update: json!({ "concurrentJobs": 0 }),
            expected_slots: MIN_RESERVED_SLOTS,
        },
        UpdateCase {
            label: "the member beside a plan that adds capacity",
            created_slots: 2,
            update: json!({
                "reservationPlanSettings": plan("AUTO_RENEW", 9),
                "concurrentJobs": 4
            }),
            expected_slots: 9,
        },
        UpdateCase {
            label: "the member beside a plan restating the count the queue reserves",
            created_slots: 5,
            update: json!({
                "reservationPlanSettings": plan("EXPIRE", 5),
                "concurrentJobs": 120
            }),
            expected_slots: 5,
        },
        UpdateCase {
            label: "the member beside a plan, among the queue's other members",
            created_slots: MIN_RESERVED_SLOTS,
            update: json!({
                "status": "PAUSED",
                "description": "after",
                "maximumConcurrentFeeds": 2,
                "reservationPlanSettings": plan("AUTO_RENEW", 11),
                "concurrentJobs": TOTAL_CONCURRENT_JOBS
            }),
            expected_slots: 11,
        },
    ]
}

/// An `UpdateQueue` that states `ConcurrentJobs` on a reserved queue leaves the
/// concurrency where the queue's plan puts it: the standing plan's count when the call
/// carries no plan, and the new plan's count when it does.
#[tokio::test]
async fn an_updated_reserved_queue_ignores_the_concurrency_the_request_states() {
    for case in update_cases() {
        let UpdateCase {
            label,
            created_slots,
            update,
            expected_slots,
        } = case;
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({
                "name": QUEUE,
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": plan("EXPIRE", created_slots)
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            created.status, created.body
        );

        let updated = update_queue(&emulator, QUEUE, update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );
        assert_projected(&emulator, label, &updated, expected_slots).await;
    }
}

/// The complement of the rule, pinned beside it: the same stated member DOES decide an
/// on-demand queue's concurrency, because such a queue has no plan to buy capacity with.
/// Each row is the same pair of calls as above, on a queue whose pricing plan is the
/// default one.
#[tokio::test]
async fn an_on_demand_queue_still_takes_the_concurrency_the_request_states() {
    // Each row stays inside the Region's pool, which is `queue_concurrency_pool`'s rule.
    for entered in [0, 1, 12, DEFAULT_CONCURRENT_JOBS] {
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({ "name": "on-demand", "concurrentJobs": entered }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {entered:?} jobs: expected HTTP 201 from CreateQueue, got {} with body {}",
            created.status, created.body
        );
        let actual = created.at("/queue/concurrentJobs");
        assert_eq!(
            actual,
            &json!(entered),
            "case {entered:?} jobs: expected concurrentJobs {entered}, got {actual}"
        );

        let updated = update_queue(
            &emulator,
            "on-demand",
            json!({ "concurrentJobs": entered + 1 }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {entered:?} jobs: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );
        let actual = updated.at("/queue/concurrentJobs");
        assert_eq!(
            actual,
            &json!(entered + 1),
            "case {entered:?} jobs: expected concurrentJobs {} after the update, got {actual}",
            entered + 1
        );
        let fetched = emulator.get_queue("on-demand").await;
        let actual = fetched.at("/queue/concurrentJobs");
        assert_eq!(
            actual,
            &json!(entered + 1),
            "case {entered:?} jobs: expected GetQueue to report concurrentJobs {}, got \
             {actual}",
            entered + 1
        );
    }
}

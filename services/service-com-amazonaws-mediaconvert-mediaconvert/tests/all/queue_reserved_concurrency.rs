// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the concurrency a RESERVED queue reports.
//!
//! The rule: a reserved queue's capacity is bought by its reservation plan, so the slot
//! count the stored plan reserves IS that queue's `ConcurrentJobs`
//! (`smithy_query(query="doc",shape="CreateQueueRequest",member="ConcurrentJobs")`: "For
//! reserved queues, specify the number of jobs you can process concurrently in your
//! reservation plan instead", and
//! `smithy_query(query="doc",shape="UpdateQueueRequest",member="ConcurrentJobs")`: "For
//! reserved queues, update your reservation plan instead in order to increase your yearly
//! commitment"). Every call on such a queue therefore stores the stored plan's
//! `ReservedSlots` as the queue's concurrency, whether or not the same call states
//! `ConcurrentJobs` itself — so the two members of one projected `Queue` never contradict
//! each other, and every later reader (`GetQueue`, `ListQueues`) projects the same pair.
//! The sibling suite `queue_reserved_concurrency_member` covers the calls that state the
//! member.
//!
//! What a plan is allowed to say — the floor of a slot count, and that the count is
//! one-way — is the sibling suite `queue_capacity_validation`'s rule; what an on-demand
//! queue's stated allocation is measured against is `queue_concurrency_pool`'s. This suite
//! is about the value the queue REPORTS once a plan has been stored.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::MIN_RESERVED_SLOTS;

use super::job_lifecycle::{Emulator, Reply};
use super::queue_update::update_queue;

/// The queue every row works on.
const QUEUE: &str = "reserved";

/// `GET /2017-08-29/queues` — the account's queues, the third channel that projects a
/// queue's concurrency.
pub(crate) async fn list_queues(emulator: &Emulator) -> Reply {
    emulator.send("GET", "/2017-08-29/queues", None).await
}

/// A `ReservationPlanSettings` reserving `slots` transcode slots. `Commitment` models
/// exactly one value and `RenewalType` two
/// (`smithy_query(query="shape",shape="Commitment,RenewalType")`).
pub(crate) fn plan(renewal_type: &str, slots: i32) -> Value {
    json!({
        "commitment": "ONE_YEAR",
        "renewalType": renewal_type,
        "reservedSlots": slots
    })
}

/// One reserved queue, one update of it, and the concurrency the queue must report after.
struct Case {
    label: &'static str,
    /// The `CreateQueue` body the row starts from.
    create: Value,
    /// The `UpdateQueue` body.
    update: Value,
    /// `Queue$ConcurrentJobs` afterwards.
    concurrent_jobs: i32,
    /// `Queue$ReservationPlan$ReservedSlots` afterwards.
    reserved_slots: i32,
}

/// A reserved queue's `CreateQueue` body, with any extra members merged in.
fn reserved_create(slots: i32, extra: Value) -> Value {
    let mut body = json!({
        "name": QUEUE,
        "pricingPlan": "RESERVED",
        "reservationPlanSettings": plan("EXPIRE", slots)
    });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            body[key] = value.clone();
        }
    }
    body
}

/// The table spans what the rule generalises over: the SIZE of the slot count the new plan
/// states (the published floor `MIN_RESERVED_SLOTS`, a small increase, a large one), both
/// `RenewalType` values, whether the new plan raises the count or restates it, where the
/// concurrency the queue carried before came from (the plan the create stored, or a
/// `ConcurrentJobs` the create stated), and whether the update carries a plan at all.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the plan grows: capacity added to a reserved queue",
            create: reserved_create(2, json!({})),
            update: json!({ "reservationPlanSettings": plan("EXPIRE", 8) }),
            concurrent_jobs: 8,
            reserved_slots: 8,
        },
        Case {
            label: "a plan at the published floor, restated",
            create: reserved_create(MIN_RESERVED_SLOTS, json!({})),
            update: json!({ "reservationPlanSettings": plan("AUTO_RENEW", MIN_RESERVED_SLOTS) }),
            concurrent_jobs: MIN_RESERVED_SLOTS,
            reserved_slots: MIN_RESERVED_SLOTS,
        },
        Case {
            label: "one slot above the floor, auto-renewing",
            create: reserved_create(MIN_RESERVED_SLOTS, json!({})),
            update: json!({ "reservationPlanSettings": plan("AUTO_RENEW", MIN_RESERVED_SLOTS + 1) }),
            concurrent_jobs: MIN_RESERVED_SLOTS + 1,
            reserved_slots: MIN_RESERVED_SLOTS + 1,
        },
        Case {
            label: "a plan restating the count the queue already reserves",
            create: reserved_create(3, json!({})),
            update: json!({ "reservationPlanSettings": plan("AUTO_RENEW", 3) }),
            concurrent_jobs: 3,
            reserved_slots: 3,
        },
        Case {
            label: "a large plan, far above what the queue reserved",
            create: reserved_create(2, json!({})),
            update: json!({ "reservationPlanSettings": plan("EXPIRE", 250) }),
            concurrent_jobs: 250,
            reserved_slots: 250,
        },
        Case {
            label: "the create stated its own concurrency; the new plan replaces it",
            create: reserved_create(2, json!({ "concurrentJobs": 5 })),
            update: json!({ "reservationPlanSettings": plan("EXPIRE", 9) }),
            concurrent_jobs: 9,
            reserved_slots: 9,
        },
        Case {
            label: "the update states the member itself beside the plan",
            create: reserved_create(2, json!({})),
            update: json!({
                "reservationPlanSettings": plan("EXPIRE", 7),
                "concurrentJobs": 7
            }),
            concurrent_jobs: 7,
            reserved_slots: 7,
        },
        Case {
            label: "the plan grows beside the other members of the update",
            create: reserved_create(2, json!({ "description": "before" })),
            update: json!({
                "status": "PAUSED",
                "description": "after",
                "maximumConcurrentFeeds": 2,
                "reservationPlanSettings": plan("AUTO_RENEW", 6)
            }),
            concurrent_jobs: 6,
            reserved_slots: 6,
        },
        Case {
            label: "an update carrying no plan keeps the concurrency the plan set",
            create: reserved_create(4, json!({})),
            update: json!({ "description": "renamed nothing but the text" }),
            concurrent_jobs: 4,
            reserved_slots: 4,
        },
    ]
}

/// The queue `ListQueues` reports under `name`, or `Value::Null` when the listing holds no
/// such queue.
pub(crate) fn listed(listing: &Reply, name: &str) -> Value {
    listing
        .at("/queues")
        .as_array()
        .and_then(|queues| {
            queues
                .iter()
                .find(|queue| queue.get("name").and_then(Value::as_str) == Some(name))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

/// A reserved queue reports the slot count its stored reservation plan reserves as its
/// `ConcurrentJobs`, through every channel that projects the queue.
#[tokio::test]
async fn a_reserved_queue_reports_the_concurrency_its_plan_reserves() {
    for case in cases() {
        let Case {
            label,
            create,
            update,
            concurrent_jobs,
            reserved_slots,
        } = case;
        // One account per row, so a row only ever reads its own mutation.
        let emulator = Emulator::new();
        let created = emulator.create_queue(create).await;
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

        let fetched = emulator.get_queue(QUEUE).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetQueue, got {} with body {}",
            fetched.status, fetched.body
        );
        let listing = list_queues(&emulator).await;
        let from_listing = Reply {
            status: listing.status,
            error_type: None,
            body: json!({ "queue": listed(&listing, QUEUE) }),
        };

        for (source, reply) in [
            ("UpdateQueue", &updated),
            ("GetQueue", &fetched),
            ("ListQueues", &from_listing),
        ] {
            let actual = reply.at("/queue/concurrentJobs");
            assert_eq!(
                actual,
                &json!(concurrent_jobs),
                "case {label:?} via {source}: expected concurrentJobs {concurrent_jobs}, \
                 got {actual}"
            );
            let slots = reply.at("/queue/reservationPlan/reservedSlots");
            assert_eq!(
                slots,
                &json!(reserved_slots),
                "case {label:?} via {source}: expected reservationPlan.reservedSlots \
                 {reserved_slots}, got {slots}"
            );
            // The whole point of the rule: the two members of one projected queue agree.
            assert_eq!(
                reply.at("/queue/concurrentJobs"),
                reply.at("/queue/reservationPlan/reservedSlots"),
                "case {label:?} via {source}: a reserved queue's concurrency must be the \
                 slot count its plan reserves, got {}",
                reply.body
            );
        }
    }
}

/// The concurrency follows the plan on EVERY update that stores one, not only the first:
/// successive increases each move it, and an update that carries no plan leaves it where
/// the last plan put it.
#[tokio::test]
async fn each_successive_plan_moves_the_concurrency_it_reserves() {
    let emulator = Emulator::new();
    let created = emulator
        .create_queue(reserved_create(MIN_RESERVED_SLOTS, json!({})))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.at("/queue/concurrentJobs"),
        &json!(MIN_RESERVED_SLOTS),
        "case \"the create\": expected the plan's {MIN_RESERVED_SLOTS} slots as the \
         concurrency, got {}",
        created.body
    );

    // Each step raises the count — `ReservedSlots` is one-way — and the concurrency must
    // land on the count of the plan that step stored.
    for slots in [MIN_RESERVED_SLOTS + 1, MIN_RESERVED_SLOTS + 4, 120] {
        let updated = update_queue(
            &emulator,
            QUEUE,
            json!({ "reservationPlanSettings": plan("EXPIRE", slots) }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {slots:?} slots: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );
        let actual = updated.at("/queue/concurrentJobs");
        assert_eq!(
            actual,
            &json!(slots),
            "case {slots:?} slots: expected concurrentJobs {slots}, got {actual}"
        );
    }

    // An update that stores no plan changes neither member.
    let untouched = update_queue(&emulator, QUEUE, json!({ "status": "PAUSED" })).await;
    assert_eq!(
        untouched.status, 200,
        "case \"no plan at all\": expected HTTP 200 from UpdateQueue, got {} with body {}",
        untouched.status, untouched.body
    );
    let actual = untouched.at("/queue/concurrentJobs");
    assert_eq!(
        actual,
        &json!(120),
        "case \"no plan at all\": expected the last plan's 120 slots to stand, got {actual}"
    );
}

/// The complement of the rule: an ON-DEMAND queue has no plan to take its concurrency
/// from, so the value the request states is the one it reports — and no reservation plan
/// appears on it.
#[tokio::test]
async fn an_on_demand_queue_reports_the_concurrency_the_request_states() {
    let emulator = Emulator::new();
    let created = emulator
        .create_queue(json!({ "name": "on-demand", "concurrentJobs": 3 }))
        .await;
    assert_eq!(
        created.status, 201,
        "the fixture queue must be created, got {} with body {}",
        created.status, created.body
    );

    for entered in [0, 1, 12, 300] {
        let updated = update_queue(
            &emulator,
            "on-demand",
            json!({ "concurrentJobs": entered }),
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
            &json!(entered),
            "case {entered:?} jobs: expected concurrentJobs {entered}, got {actual}"
        );
        let plan = updated.at("/queue/reservationPlan");
        assert_eq!(
            plan,
            &Value::Null,
            "case {entered:?} jobs: an on-demand queue has no reservation plan, got {plan}"
        );
    }
}

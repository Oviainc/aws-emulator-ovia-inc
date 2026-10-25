// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the CAPACITY a queue may be given, on both the create and
//! the update that can state it.
//!
//! Two rules, each asserted over a table that spans what it generalises over rather than
//! the one value a report happened to carry:
//!
//! * A reserved queue's slot count is ONE-WAY.
//!   `ReservationPlanSettings$ReservedSlots`: "Specifies the number of reserved transcode
//!   slots (RTS) for this queue. The number of RTS determines how many jobs the queue can
//!   process in parallel; each RTS can process one job at a time. You can't decrease the
//!   number of RTS in your reserved queue. You can increase the number of RTS by extending
//!   your existing commitment with a new 12-month commitment for the larger number." So a
//!   plan stating fewer slots than the stored one is refused with `BadRequestException`
//!   (400) and the stored plan is left exactly as it was, while one stating the same or
//!   more is applied and restarts the 12-month term.
//! * A concurrency a REQUEST ENTERS is an allocation out of the Region's pool.
//!   `ConcurrentJobs`: "Specify the maximum number of jobs your queue can process
//!   concurrently. For on-demand queues, the value you enter is constrained by your service
//!   quotas for Maximum concurrent jobs, per on-demand queue and Maximum concurrent jobs,
//!   per account", and `ListQueues$UnallocatedConcurrentJobs` is "the remaining number of
//!   concurrent jobs that are not associated with a queue and are available to allocate to
//!   a queue". So a negative count is `BadRequestException` (400), a count past what the
//!   pool has left for that queue is `ServiceQuotaExceededException` (402), and an accepted
//!   one is visible both on the queue and in the pool the listing reports.
//!
//! Every case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    reserved_slots_below_minimum_message, reserved_slots_decrease_message,
    DEFAULT_CONCURRENT_JOBS, MIN_RESERVED_SLOTS, RESERVATION_TERM, TOTAL_CONCURRENT_JOBS,
};

use super::job_lifecycle::{Emulator, Reply};
use super::queue_update::update_queue;

/// The queue every case works on.
const QUEUE: &str = "capacity";

/// `GET /2017-08-29/queues` — the account's queues beside the Region's concurrent-job
/// pool.
async fn list_queues(emulator: &Emulator) -> Reply {
    emulator.send("GET", "/2017-08-29/queues", None).await
}

/// A reserved queue's `CreateQueue` body, reserving `slots` of them.
fn reserved_create(slots: i32) -> Value {
    json!({
        "name": QUEUE,
        "pricingPlan": "RESERVED",
        // `Commitment` models exactly one value
        // (`smithy_query(query="shape",shape="Commitment")`).
        "reservationPlanSettings": {
            "commitment": "ONE_YEAR",
            "renewalType": "EXPIRE",
            "reservedSlots": slots
        }
    })
}

/// An `UpdateQueue` body carrying a reservation plan for `slots` slots.
fn reserved_update(slots: i32, renewal_type: &str) -> Value {
    json!({
        "reservationPlanSettings": {
            "commitment": "ONE_YEAR",
            "renewalType": renewal_type,
            "reservedSlots": slots
        }
    })
}

/// What an update of a reserved queue's plan must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotOutcome {
    /// The plan is stored and the commitment restarts.
    Applied,
    /// Fewer slots than the queue already reserves: the one-way rule refuses it.
    Decrease,
    /// Fewer slots than a plan can state at all.
    BelowMinimum,
}

/// One row of the slot table: the queue's stored slot count, the count an update states,
/// the `RenewalType` it states, and what must come back.
struct SlotCase {
    label: &'static str,
    stored: i32,
    requested: i32,
    renewal_type: &'static str,
    outcome: SlotOutcome,
}

/// The rows span what the rule generalises over: the DIRECTION of the change (down, equal,
/// up), its SIZE (one slot and several), the stored count it starts from, both modelled
/// `RenewalType` values (`smithy_query(query="shape",shape="RenewalType")`), the published
/// floor of a plan ([`MIN_RESERVED_SLOTS`]) and one value below it, and a negative count.
fn slot_cases() -> Vec<SlotCase> {
    vec![
        SlotCase {
            label: "a decrease to a quarter of the stored slots",
            stored: 4,
            requested: 1,
            renewal_type: "EXPIRE",
            outcome: SlotOutcome::Decrease,
        },
        SlotCase {
            label: "a decrease by a single slot",
            stored: 4,
            requested: 3,
            renewal_type: "AUTO_RENEW",
            outcome: SlotOutcome::Decrease,
        },
        SlotCase {
            label: "a decrease from the smallest plan there is",
            stored: MIN_RESERVED_SLOTS,
            requested: MIN_RESERVED_SLOTS - 1,
            renewal_type: "EXPIRE",
            outcome: SlotOutcome::BelowMinimum,
        },
        SlotCase {
            label: "a negative slot count",
            stored: 4,
            requested: -3,
            renewal_type: "EXPIRE",
            outcome: SlotOutcome::BelowMinimum,
        },
        SlotCase {
            label: "the same slot count: not a decrease",
            stored: 4,
            requested: 4,
            renewal_type: "AUTO_RENEW",
            outcome: SlotOutcome::Applied,
        },
        SlotCase {
            label: "an increase by a single slot",
            stored: 4,
            requested: 5,
            renewal_type: "EXPIRE",
            outcome: SlotOutcome::Applied,
        },
        SlotCase {
            label: "an increase to four times the stored slots",
            stored: 2,
            requested: 8,
            renewal_type: "AUTO_RENEW",
            outcome: SlotOutcome::Applied,
        },
        SlotCase {
            label: "an increase from the smallest plan there is",
            stored: MIN_RESERVED_SLOTS,
            requested: MIN_RESERVED_SLOTS + 1,
            renewal_type: "EXPIRE",
            outcome: SlotOutcome::Applied,
        },
    ]
}

/// `ReservedSlots` is one-way: an update may raise a reserved queue's slot count (which
/// restarts its 12-month commitment) and may restate it, but one that would lower it is
/// refused and the stored plan survives untouched.
#[tokio::test]
async fn a_reserved_queues_slot_count_can_only_increase() {
    for case in slot_cases() {
        let SlotCase {
            label,
            stored,
            requested,
            renewal_type,
            outcome,
        } = case;
        // One account per row, so a row only ever reads its own mutation.
        let emulator = Emulator::new();
        let created = emulator.create_queue(reserved_create(stored)).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the reserved queue must be created, got {} with body {}",
            created.status, created.body
        );

        let body = reserved_update(requested, renewal_type);
        let updated = update_queue(&emulator, QUEUE, body).await;
        let fetched = emulator.get_queue(QUEUE).await;

        match outcome {
            SlotOutcome::Applied => {
                assert_eq!(
                    updated.status, 200,
                    "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
                    updated.status, updated.body
                );
                for (source, reply) in [("UpdateQueue", &updated), ("GetQueue", &fetched)] {
                    assert_eq!(
                        reply.at("/queue/reservationPlan/reservedSlots"),
                        &json!(requested),
                        "case {label:?} via {source}: expected reservedSlots {requested}, got {}",
                        reply.body
                    );
                    assert_eq!(
                        reply.string("/queue/reservationPlan/renewalType"),
                        renewal_type,
                        "case {label:?} via {source}: expected renewalType {renewal_type:?}, \
                         got {}",
                        reply.body
                    );
                    // "You can increase the number of RTS by extending your existing
                    // commitment with a new 12-month commitment": the applied plan is a
                    // fresh term, purchased no earlier than the one it replaced and
                    // running the full commitment from there.
                    let purchased = reply
                        .at("/queue/reservationPlan/purchasedAt")
                        .as_i64()
                        .unwrap_or_default();
                    let expires = reply
                        .at("/queue/reservationPlan/expiresAt")
                        .as_i64()
                        .unwrap_or_default();
                    let created_purchase = created
                        .at("/queue/reservationPlan/purchasedAt")
                        .as_i64()
                        .unwrap_or_default();
                    assert!(
                        purchased >= created_purchase,
                        "case {label:?} via {source}: expected a commitment purchased no \
                         earlier than {created_purchase}, got {purchased}"
                    );
                    let term = i64::try_from(RESERVATION_TERM.as_secs()).unwrap_or_default();
                    assert_eq!(
                        expires - purchased,
                        term,
                        "case {label:?} via {source}: expected a {term}-second term, got \
                         {} from body {}",
                        expires - purchased,
                        reply.body
                    );
                }
            }
            SlotOutcome::Decrease | SlotOutcome::BelowMinimum => {
                assert_eq!(
                    updated.status, 400,
                    "case {label:?}: expected HTTP 400 from UpdateQueue, got {} with body {}",
                    updated.status, updated.body
                );
                assert_eq!(
                    updated.error_type.as_deref(),
                    Some("BadRequestException"),
                    "case {label:?}: expected BadRequestException, got {:?} with body {}",
                    updated.error_type,
                    updated.body
                );
                let expected_message = if outcome == SlotOutcome::Decrease {
                    reserved_slots_decrease_message(stored, requested)
                } else {
                    reserved_slots_below_minimum_message(requested)
                };
                assert_eq!(
                    updated.string("/message"),
                    expected_message,
                    "case {label:?}: expected the refusal to state the rule, got {}",
                    updated.body
                );
                // The refused update leaves the whole stored queue — plan, term and all —
                // exactly as the create left it.
                assert_eq!(
                    fetched.at("/queue"),
                    created.at("/queue"),
                    "case {label:?}: a refused update must change nothing, got {}",
                    fetched.body
                );
            }
        }
    }
}

/// A plan reserves at least one slot on the CREATE too: the floor is the plan's, not one
/// operation's, and a refused create stores no queue at all.
#[tokio::test]
async fn a_new_reservation_plan_reserves_at_least_one_slot() {
    // The rows straddle the published floor: below it, at it, and above it.
    let cases = [
        ("a negative slot count", -4, 400),
        ("no slots at all", 0, 400),
        ("the smallest plan there is", MIN_RESERVED_SLOTS, 201),
        ("a plan above the floor", MIN_RESERVED_SLOTS + 6, 201),
    ];

    for (label, slots, expected) in cases {
        let emulator = Emulator::new();
        let created = emulator.create_queue(reserved_create(slots)).await;
        assert_eq!(
            created.status, expected,
            "case {label:?}: expected HTTP {expected} from CreateQueue, got {} with body {}",
            created.status, created.body
        );

        let fetched = emulator.get_queue(QUEUE).await;
        if expected == 201 {
            assert_eq!(
                fetched.at("/queue/reservationPlan/reservedSlots"),
                &json!(slots),
                "case {label:?}: expected the stored plan to reserve {slots}, got {}",
                fetched.body
            );
        } else {
            assert_eq!(
                created.error_type.as_deref(),
                Some("BadRequestException"),
                "case {label:?}: expected BadRequestException, got {:?} with body {}",
                created.error_type,
                created.body
            );
            assert_eq!(
                created.string("/message"),
                reserved_slots_below_minimum_message(slots),
                "case {label:?}: expected the refusal to state the floor, got {}",
                created.body
            );
            assert_eq!(
                fetched.status, 404,
                "case {label:?}: a refused create must store nothing, got {} with body {}",
                fetched.status, fetched.body
            );
        }
    }
}

/// What a concurrency a request entered must answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allocation {
    /// Stored, and reflected in the pool the listing reports.
    Allocated,
    /// Not a number of jobs: `BadRequestException` (400).
    NotANumberOfJobs,
    /// More than the pool has left for the queue: `ServiceQuotaExceededException` (402).
    PastThePool,
}

impl Allocation {
    /// The status and error shape this outcome is answered with on a request that states
    /// it, where `success` is the operation's own success code.
    fn expected(self, success: u16) -> (u16, Option<&'static str>) {
        match self {
            Self::Allocated => (success, None),
            Self::NotANumberOfJobs => (400, Some("BadRequestException")),
            Self::PastThePool => (402, Some("ServiceQuotaExceededException")),
        }
    }
}

/// `ConcurrentJobs` a request enters is an allocation out of the Region's pool, on the
/// create that states it and on the update that states it alike: a negative count is
/// refused, a count past what the pool has left for the queue is refused, and an accepted
/// one is what the queue and the listing's `UnallocatedConcurrentJobs` then report.
///
/// The rows span the whole class the rule divides: the published floor of an allocation
/// (zero jobs) and one below it, a small allocation, the pool's exact remainder, one job
/// past that remainder, the whole Region's ceiling, an allocation orders of magnitude past
/// it, and the extreme of the member's `__integer` type.
#[tokio::test]
async fn a_concurrency_a_request_enters_is_bounded_by_the_region_pool() {
    // A fresh account holds only the built-in `Default` queue, which is on-demand and
    // already holds its own allocation — so this is what the pool has left for the queue
    // each row works on, whether that queue is being created or updated. Arithmetic on the
    // crate's own constants, never a restated number.
    let available = TOTAL_CONCURRENT_JOBS - DEFAULT_CONCURRENT_JOBS;
    let cases = [
        ("no jobs at all: the floor of an allocation", 0, Allocation::Allocated),
        ("a single job", 1, Allocation::Allocated),
        ("a moderate allocation", 150, Allocation::Allocated),
        ("exactly what the pool has left", available, Allocation::Allocated),
        ("one job past what the pool has left", available + 1, Allocation::PastThePool),
        ("the whole Region's ceiling", TOTAL_CONCURRENT_JOBS, Allocation::PastThePool),
        ("far past the ceiling", 1_000_000, Allocation::PastThePool),
        ("one job below the floor", -1, Allocation::NotANumberOfJobs),
        ("the negative allocation the report carried", -5, Allocation::NotANumberOfJobs),
        ("the bottom of the member's type", i32::MIN, Allocation::NotANumberOfJobs),
    ];

    for (label, entered, outcome) in cases {
        // The CREATE that states it: a refused one stores no queue at all.
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({ "name": QUEUE, "concurrentJobs": entered }))
            .await;
        let (expected_status, expected_error) = outcome.expected(201);
        assert_eq!(
            created.status, expected_status,
            "case {label:?} via CreateQueue: expected HTTP {expected_status}, got {} with \
             body {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_type.as_deref(),
            expected_error,
            "case {label:?} via CreateQueue: expected {expected_error:?}, got {:?} with \
             body {}",
            created.error_type,
            created.body
        );
        let fetched = emulator.get_queue(QUEUE).await;
        if outcome == Allocation::Allocated {
            assert_eq!(
                fetched.at("/queue/concurrentJobs"),
                &json!(entered),
                "case {label:?} via CreateQueue: expected the stored queue to allocate \
                 {entered}, got {}",
                fetched.body
            );
            assert_pool(label, "CreateQueue", &list_queues(&emulator).await, available - entered);
        } else {
            assert_eq!(
                fetched.status, 404,
                "case {label:?} via CreateQueue: a refused create must store nothing, got \
                 {} with body {}",
                fetched.status, fetched.body
            );
            assert_pool(label, "CreateQueue", &list_queues(&emulator).await, available);
        }

        // The UPDATE that states it, on a queue whose allocation the service filled in: a
        // refused one leaves the stored queue exactly as it was.
        let emulator = Emulator::new();
        let before = emulator.create_queue(json!({ "name": QUEUE })).await;
        assert_eq!(
            before.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            before.status, before.body
        );
        let updated = update_queue(&emulator, QUEUE, json!({ "concurrentJobs": entered })).await;
        let (expected_status, expected_error) = outcome.expected(200);
        assert_eq!(
            updated.status, expected_status,
            "case {label:?} via UpdateQueue: expected HTTP {expected_status}, got {} with \
             body {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.error_type.as_deref(),
            expected_error,
            "case {label:?} via UpdateQueue: expected {expected_error:?}, got {:?} with \
             body {}",
            updated.error_type,
            updated.body
        );
        let fetched = emulator.get_queue(QUEUE).await;
        if outcome == Allocation::Allocated {
            assert_eq!(
                fetched.at("/queue/concurrentJobs"),
                &json!(entered),
                "case {label:?} via UpdateQueue: expected the stored queue to allocate \
                 {entered}, got {}",
                fetched.body
            );
            assert_pool(label, "UpdateQueue", &list_queues(&emulator).await, available - entered);
        } else {
            assert_eq!(
                fetched.at("/queue"),
                before.at("/queue"),
                "case {label:?} via UpdateQueue: a refused update must change nothing, got {}",
                fetched.body
            );
            // The queue keeps the allocation the service filled in, so the pool is down by
            // exactly that and never by the refused request's value.
            assert_pool(
                label,
                "UpdateQueue",
                &list_queues(&emulator).await,
                available - DEFAULT_CONCURRENT_JOBS,
            );
        }
    }
}

/// `ListQueues` must report the Region's ceiling and `expected` left to allocate.
fn assert_pool(label: &str, source: &str, listed: &Reply, expected: i32) {
    assert_eq!(
        listed.at("/totalConcurrentJobs"),
        &json!(TOTAL_CONCURRENT_JOBS),
        "case {label:?} via {source}: expected the Region's ceiling, got {}",
        listed.body
    );
    assert_eq!(
        listed.at("/unallocatedConcurrentJobs"),
        &json!(expected),
        "case {label:?} via {source}: expected {expected} concurrent jobs left to \
         allocate, got {}",
        listed.body
    );
}

/// `MaximumConcurrentFeeds` keeps its published range on the update too: `__integerMin0`
/// makes zero the bottom of it (`smithy_query(query="operation",operation="UpdateQueue")`),
/// so a value below that is refused and the stored queue is untouched.
#[tokio::test]
async fn maximum_concurrent_feeds_keeps_its_published_range_on_an_update() {
    // The published bound, one past it, and a value well inside it.
    let cases = [
        ("one below the published minimum", -1, 400),
        ("far below the published minimum", -250, 400),
        ("the published minimum", 0, 200),
        ("inside the range", 6, 200),
    ];

    for (label, feeds, expected) in cases {
        let emulator = Emulator::new();
        let before = emulator
            .create_queue(json!({ "name": QUEUE, "maximumConcurrentFeeds": 4 }))
            .await;
        assert_eq!(
            before.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            before.status, before.body
        );

        let body = json!({ "maximumConcurrentFeeds": feeds });
        let updated = update_queue(&emulator, QUEUE, body).await;
        assert_eq!(
            updated.status, expected,
            "case {label:?}: expected HTTP {expected} from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );

        let fetched = emulator.get_queue(QUEUE).await;
        if expected == 200 {
            assert_eq!(
                fetched.at("/queue/maximumConcurrentFeeds"),
                &json!(feeds),
                "case {label:?}: expected maximumConcurrentFeeds {feeds}, got {}",
                fetched.body
            );
        } else {
            assert_eq!(
                fetched.at("/queue"),
                before.at("/queue"),
                "case {label:?}: a refused update must change nothing, got {}",
                fetched.body
            );
        }
    }
}

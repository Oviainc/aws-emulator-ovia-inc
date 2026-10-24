// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdateQueue`.
//!
//! `UpdateQueue` is documented as "Modify one of your existing queues", and each of its
//! optional members is "the new <member> for the queue, if you are changing it", so the
//! rule under test is: a member the request carries replaces the stored one, a member it
//! omits keeps the value an earlier call persisted, and the mutation is READ-YOUR-WRITE
//! visible — the update's own response and every later `GetQueue` project the same stored
//! record. A name the store does not hold is `NotFoundException` (404), a built-in
//! (`SYSTEM`) queue cannot be modified at all (403), and a reservation plan applies only
//! to a reserved queue (400 otherwise).
//!
//! What a queue's `Status` then means for the jobs in it — a `PAUSED` queue's jobs "won't
//! begin" — is the rule the sibling suite `queue_pause_gating` covers.
//!
//! Each case drives the real restJson1 wire path through the router (the harness is the
//! one `job_lifecycle` builds), so what is asserted is the bytes a client would see.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    not_found_message, DEFAULT_CONCURRENT_JOBS, DEFAULT_MAXIMUM_CONCURRENT_FEEDS,
};

use super::job_lifecycle::{Emulator, Reply};

/// `PUT /2017-08-29/queues/{Name}` — the binding
/// `smithy_query(query="operation",operation="UpdateQueue")` reports, success code 200.
pub(crate) async fn update_queue(emulator: &Emulator, name: &str, body: Value) -> Reply {
    emulator
        .send("PUT", &format!("/2017-08-29/queues/{name}"), Some(body))
        .await
}

/// `DELETE /2017-08-29/queues/{Name}`.
async fn delete_queue(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .send("DELETE", &format!("/2017-08-29/queues/{name}"), None)
        .await
}

/// The name of the built-in queue every MediaConvert account carries. `Queue$Type`:
/// "System queues are built in. You can't modify or delete system queues."
const SYSTEM_QUEUE: &str = "Default";

/// The `Queue` members `UpdateQueue` can change, under the `jsonName` each goes on the
/// wire with (`smithy_query(query="shape",shape="Queue")` and
/// `smithy_query(query="operation",operation="UpdateQueue")`).
const MUTABLE_MEMBERS: [&str; 4] = [
    "status",
    "description",
    "concurrentJobs",
    "maximumConcurrentFeeds",
];

/// One queue, one update of it, and the stored queue the update must leave behind.
struct Case {
    label: &'static str,
    /// The `CreateQueue` body the row starts from.
    create: Value,
    /// The `UpdateQueue` body.
    update: Value,
    /// The value each member of [`MUTABLE_MEMBERS`] must carry afterwards; a member this
    /// object leaves out must be ABSENT from the stored queue.
    expect: Value,
}

/// The table spans the input class along what the rule generalises over: WHICH members
/// the update carries (each one alone, all of them, none of them), both `QueueStatus`
/// values in both directions, a member being set for the first time versus replaced,
/// the published lower bound of `MaximumConcurrentFeeds` (`__integerMin0`, range min 0 —
/// `smithy_query(query="operation",operation="UpdateQueue")`), and whether the queue
/// being updated carried the member before.
///
/// `concurrentJobs` and `maximumConcurrentFeeds` are members the service always fills,
/// so a row that changes neither still expects the crate's own
/// `DEFAULT_CONCURRENT_JOBS` / `DEFAULT_MAXIMUM_CONCURRENT_FEEDS` rather than an absent
/// member — the expectation reads the constants instead of restating their values.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "status alone: ACTIVE -> PAUSED, the description is kept",
            create: json!({ "name": "q", "description": "probe queue for lifecycle test" }),
            update: json!({ "status": "PAUSED" }),
            expect: json!({
                "status": "PAUSED",
                "description": "probe queue for lifecycle test",
                "concurrentJobs": DEFAULT_CONCURRENT_JOBS,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "status and description together, the recorded shape of the mutation",
            create: json!({ "name": "q", "description": "probe queue for lifecycle test" }),
            update: json!({ "status": "PAUSED", "description": "updated description" }),
            expect: json!({
                "status": "PAUSED",
                "description": "updated description",
                "concurrentJobs": DEFAULT_CONCURRENT_JOBS,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "status alone: PAUSED -> ACTIVE, the other direction",
            create: json!({ "name": "q", "status": "PAUSED", "description": "paused at birth" }),
            update: json!({ "status": "ACTIVE" }),
            expect: json!({
                "status": "ACTIVE",
                "description": "paused at birth",
                "concurrentJobs": DEFAULT_CONCURRENT_JOBS,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "description alone: the status stays where it was",
            create: json!({ "name": "q", "status": "PAUSED" }),
            update: json!({ "description": "described after the fact" }),
            expect: json!({
                "status": "PAUSED",
                "description": "described after the fact",
                "concurrentJobs": DEFAULT_CONCURRENT_JOBS,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "concurrentJobs alone, set for the first time",
            create: json!({ "name": "q" }),
            update: json!({ "concurrentJobs": 7 }),
            expect: json!({
                "status": "ACTIVE",
                "concurrentJobs": 7,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "concurrentJobs replacing a value the create stored",
            create: json!({ "name": "q", "concurrentJobs": 3 }),
            update: json!({ "concurrentJobs": 11 }),
            expect: json!({
                "status": "ACTIVE",
                "concurrentJobs": 11,
                "maximumConcurrentFeeds": DEFAULT_MAXIMUM_CONCURRENT_FEEDS
            }),
        },
        Case {
            label: "maximumConcurrentFeeds at its published minimum of 0",
            create: json!({ "name": "q", "maximumConcurrentFeeds": 4 }),
            update: json!({ "maximumConcurrentFeeds": 0 }),
            expect: json!({
                "status": "ACTIVE",
                "concurrentJobs": DEFAULT_CONCURRENT_JOBS,
                "maximumConcurrentFeeds": 0
            }),
        },
        Case {
            label: "every mutable member at once",
            create: json!({ "name": "q", "description": "before", "concurrentJobs": 1 }),
            update: json!({
                "status": "PAUSED",
                "description": "after",
                "concurrentJobs": 9,
                "maximumConcurrentFeeds": 2
            }),
            expect: json!({
                "status": "PAUSED",
                "description": "after",
                "concurrentJobs": 9,
                "maximumConcurrentFeeds": 2
            }),
        },
        Case {
            label: "no mutable member at all: every stored value survives",
            create: json!({
                "name": "q",
                "status": "PAUSED",
                "description": "untouched",
                "concurrentJobs": 5,
                "maximumConcurrentFeeds": 1
            }),
            update: json!({}),
            expect: json!({
                "status": "PAUSED",
                "description": "untouched",
                "concurrentJobs": 5,
                "maximumConcurrentFeeds": 1
            }),
        },
    ]
}

/// An applied `UpdateQueue` is visible to the next `GetQueue`: the members it carried
/// read back as the new values, the members it omitted keep the stored ones, and the
/// update's own response is the same projection the read serializes.
#[tokio::test]
async fn an_update_is_read_your_write_visible_to_the_next_get_queue() {
    for case in cases() {
        let Case {
            label,
            create,
            update,
            expect,
        } = case;
        // One account per row, so a row only ever reads its own mutation.
        let emulator = Emulator::new();
        let name = create
            .pointer("/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let created = emulator.create_queue(create).await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the fixture queue must be created, got {} with body {}",
            created.status, created.body
        );

        let updated = update_queue(&emulator, &name, update).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );

        let fetched = emulator.get_queue(&name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetQueue, got {} with body {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("/queue"),
            updated.at("/queue"),
            "case {label:?}: GetQueue must project what UpdateQueue stored, got {}",
            fetched.body
        );

        for (source, reply) in [("UpdateQueue", &updated), ("GetQueue", &fetched)] {
            for member in MUTABLE_MEMBERS {
                let actual = reply.at(&format!("/queue/{member}"));
                let wanted = expect.get(member).unwrap_or(&Value::Null);
                assert_eq!(
                    actual, wanted,
                    "case {label:?} via {source}: expected {member} {wanted}, got {actual}"
                );
            }
            // The identity and the creation instant are not the update's to move.
            assert_eq!(
                reply.string("/queue/name"),
                name,
                "case {label:?} via {source}: expected the name unchanged, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/queue/createdAt"),
                created.at("/queue/createdAt"),
                "case {label:?} via {source}: CreatedAt must not move, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("/queue/type"),
                "CUSTOM",
                "case {label:?} via {source}: a queue a caller created is CUSTOM, got {}",
                reply.body
            );
            // `Queue$LastUpdated` is "the timestamp in epoch seconds for when you most
            // recently updated the queue", so it is whole seconds and never earlier than
            // the creation instant.
            let created_at = reply.at("/queue/createdAt").as_i64();
            let last_updated = reply.at("/queue/lastUpdated").as_i64();
            assert!(
                last_updated.is_some_and(|updated_at| created_at
                    .is_some_and(|created_at| updated_at >= created_at)),
                "case {label:?} via {source}: expected lastUpdated at or after createdAt, got {}",
                reply.body
            );
        }
    }
}

/// A queue the store does not hold cannot be modified: `UpdateQueue` answers the modelled
/// `NotFoundException` (404), exactly as `GetQueue` does, and nothing is created under
/// that name.
///
/// The rows differ in how they miss: never created, deleted a moment ago, a prefix of a
/// stored name, and a different case of one.
#[tokio::test]
async fn updating_a_queue_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    for name in ["present", "deleted"] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: the fixture queue must be created, got {} with body {}",
            created.status, created.body
        );
    }
    let removed = delete_queue(&emulator, "deleted").await;
    assert_eq!(
        removed.status, 202,
        "the queue the rows read as gone must be deleted, got {} with body {}",
        removed.status, removed.body
    );

    for name in ["absent", "deleted", "presen", "PRESENT"] {
        let reply = update_queue(&emulator, name, json!({ "description": "post-delete" })).await;
        assert_eq!(
            reply.status, 404,
            "case {name:?}: expected HTTP 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("NotFoundException"),
            "case {name:?}: expected NotFoundException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            not_found_message("name"),
            "case {name:?}: the refusal must name the member the lookup read, got {}",
            reply.body
        );

        // A refused update creates nothing, so the name is still unreadable.
        let fetched = emulator.get_queue(name).await;
        assert_eq!(
            fetched.status, 404,
            "case {name:?}: a refused update must store nothing, got {} with body {}",
            fetched.status, fetched.body
        );
    }
}

/// The account's built-in queue is not "one of your existing queues": every modification
/// of it is the modelled `ForbiddenException` (403), and it reads back exactly as before.
///
/// The rows differ in which member the refused request tried to change.
#[tokio::test]
async fn the_built_in_system_queue_cannot_be_modified() {
    let emulator = Emulator::new();
    let before = emulator.get_queue(SYSTEM_QUEUE).await;
    assert_eq!(
        before.status, 200,
        "the built-in queue must be readable, got {} with body {}",
        before.status, before.body
    );
    assert_eq!(
        before.string("/queue/type"),
        "SYSTEM",
        "the built-in queue is SYSTEM-typed, got {}",
        before.body
    );

    let attempts = [
        ("pause it", json!({ "status": "PAUSED" })),
        ("re-describe it", json!({ "description": "mine now" })),
        ("resize it", json!({ "concurrentJobs": 1 })),
        (
            "change several members at once",
            json!({ "status": "PAUSED", "description": "mine now", "maximumConcurrentFeeds": 3 }),
        ),
        ("change nothing at all", json!({})),
    ];

    for (label, body) in attempts {
        let reply = update_queue(&emulator, SYSTEM_QUEUE, body).await;
        assert_eq!(
            reply.status, 403,
            "case {label:?}: expected HTTP 403, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ForbiddenException"),
            "case {label:?}: expected ForbiddenException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );

        let after = emulator.get_queue(SYSTEM_QUEUE).await;
        assert_eq!(
            after.at("/queue"),
            before.at("/queue"),
            "case {label:?}: the refused update must leave the built-in queue untouched, got {}",
            after.body
        );
    }
}

/// `ReservationPlanSettings` is "required for reserved queues and not applicable to
/// on-demand queues": an update that carries one for a reserved queue replaces its
/// reservation plan, and one for an on-demand queue is the modelled `BadRequestException`
/// (400) that changes nothing.
///
/// The accepted rows span both `RenewalType` values
/// (`smithy_query(query="shape",shape="RenewalType")`: AUTO_RENEW, EXPIRE) and more than
/// one reserved-slot count; the refused rows differ in what else the request asked for.
#[tokio::test]
async fn a_reservation_plan_applies_only_to_a_reserved_queue() {
    // `Commitment`'s only modelled value is ONE_YEAR
    // (`smithy_query(query="shape",shape="Commitment")`).
    let plans = [
        ("auto-renewing, one slot", json!({
            "commitment": "ONE_YEAR",
            "renewalType": "AUTO_RENEW",
            "reservedSlots": 1
        })),
        ("expiring, four slots", json!({
            "commitment": "ONE_YEAR",
            "renewalType": "EXPIRE",
            "reservedSlots": 4
        })),
    ];

    for (label, plan) in &plans {
        let emulator = Emulator::new();
        let created = emulator
            .create_queue(json!({
                "name": "reserved",
                "pricingPlan": "RESERVED",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "EXPIRE",
                    "reservedSlots": 2
                }
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {label:?}: the reserved queue must be created, got {} with body {}",
            created.status, created.body
        );

        let updated = update_queue(
            &emulator,
            "reserved",
            json!({ "reservationPlanSettings": plan }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: expected HTTP 200 from UpdateQueue, got {} with body {}",
            updated.status, updated.body
        );

        let fetched = emulator.get_queue("reserved").await;
        for (source, reply) in [("UpdateQueue", &updated), ("GetQueue", &fetched)] {
            for member in ["commitment", "renewalType", "reservedSlots"] {
                let actual = reply.at(&format!("/queue/reservationPlan/{member}"));
                let wanted = plan.get(member).unwrap_or(&Value::Null);
                assert_eq!(
                    actual, wanted,
                    "case {label:?} via {source}: expected reservationPlan.{member} {wanted}, \
                     got {actual}"
                );
            }
        }
    }

    // An on-demand queue has no reservation plan to replace, so the request is refused
    // and every member of the queue is left where it was.
    let emulator = Emulator::new();
    let created = emulator
        .create_queue(json!({ "name": "on-demand", "description": "no plan here" }))
        .await;
    assert_eq!(
        created.status, 201,
        "the on-demand queue must be created, got {} with body {}",
        created.status, created.body
    );

    let refusals = [
        (
            "a plan alone",
            json!({ "reservationPlanSettings": {
                "commitment": "ONE_YEAR",
                "renewalType": "AUTO_RENEW",
                "reservedSlots": 1
            }}),
        ),
        (
            "a plan beside members that would otherwise apply",
            json!({
                "status": "PAUSED",
                "description": "would have changed",
                "reservationPlanSettings": {
                    "commitment": "ONE_YEAR",
                    "renewalType": "EXPIRE",
                    "reservedSlots": 3
                }
            }),
        ),
    ];

    for (label, body) in refusals {
        let reply = update_queue(&emulator, "on-demand", body).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );

        let after = emulator.get_queue("on-demand").await;
        assert_eq!(
            after.at("/queue"),
            created.at("/queue"),
            "case {label:?}: the refused update must change nothing, got {}",
            after.body
        );
    }
}

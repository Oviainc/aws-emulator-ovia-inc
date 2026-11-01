// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Capacity Reservation slice an interruptible allocation needs:
//! `CreateCapacityReservation` makes the source, `CreateInterruptibleCapacityReservationAllocation`
//! allocates its unused capacity, and `GetCapacityReservationUsage` reads both back. Driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. An allocation is made FROM a stored Capacity Reservation: an id of the `cr-`
//!    resource-id SHAPE the account holds no reservation for — the all-zero placeholder
//!    included — is reported `InvalidCapacityReservationId.NotFound`, and only a value
//!    outside that shape is refused `InvalidCapacityReservationId.Malformed`, before any
//!    lookup. That rule is covered across BOTH allocation operations in
//!    [`super::interruptible_allocation_capacity_reservation_ids`].
//! 2. "Your Capacity Reservation must be in active state with no end date set and have
//!    available capacity for allocation"
//!    (`CreateInterruptibleCapacityReservationAllocationRequest$CapacityReservationId`): a
//!    source in any other state, one with an end date, one that is itself interruptible and
//!    one that already carries an allocation are refused with `IncorrectState`, and an
//!    `InstanceCount` outside `1..=AvailableInstanceCount` with `InvalidParameterValue`.
//! 3. A successful allocation MOVES that many unused instances out of the source into a new
//!    interruptible Capacity Reservation: both the source's `TotalInstanceCount` and its
//!    `AvailableInstanceCount` drop by the allocated count, and it carries the allocation
//!    naming the reservation it created, which reserves the same capacity under its own id.
//! 4. The allocation starts `pending` — the one word the modelled
//!    `InterruptibleCapacityReservationAllocationStatus` carries for a request that was
//!    accepted and is not complete — and the CLOCK owes it `active` ("active when complete",
//!    `InterruptibleCapacityAllocation$Status`), with the whole `TargetInstanceCount`
//!    allocated, one `TRANSITION_DELAY` later, whether or not anybody calls again.
//! 5. `ClientToken` makes the request idempotent: a replay reports the same allocation and
//!    allocates nothing more.
//! 6. The source itself is what `CreateCapacityReservation` reserved: the counts, the zone,
//!    the tenancy and the end type the request asked for, or the defaults its own
//!    documentation publishes.
//!
//! Every table row is a distinct input along what those rules generalise over — which id,
//! which source state, how much capacity, how much of it is asked for, which zero-size
//! preference, which tags — so no single recorded request is the specification.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{Tag, CAPACITY_RESERVATION_STATE_VALUES};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    CapacityReservationSpec, CAPACITY_RESERVATION_STATE_ACTIVE, INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING,
    INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE,
    INTERRUPTIBLE_ALLOCATION_STATUS_PENDING, INTERRUPTION_TYPE_ADHOC,
    MAX_STORED_CAPACITY_RESERVATIONS, TRANSITION_DELAY, ZERO_SIZE_PREFERENCE_DEFAULT,
    ZERO_SIZE_PREFERENCE_RETAIN,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, unescaped};

/// The Region every call here is signed for. The store holds Capacity Reservations per
/// account rather than per Region, so the Region only selects the ec2Query envelope and the
/// Region a minted reservation's ARN is built in.
const REGION: &str = "us-east-1";

/// Long enough past a clock-owed move's due instant that it has fallen due, with the margin
/// every other suite of this crate gives one.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `CreateInterruptibleCapacityReservationAllocation` call carrying `params`, as its
/// HTTP status and XML body.
async fn allocate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "CreateInterruptibleCapacityReservationAllocation",
        params,
    )
    .await
}

/// The form fields of one allocation request.
fn allocation_params(source_id: &str, instance_count: i32) -> Vec<(String, String)> {
    vec![
        (
            "CapacityReservationId".to_string(),
            source_id.to_string(),
        ),
        ("InstanceCount".to_string(), instance_count.to_string()),
    ]
}

/// One `CreateCapacityReservation` call carrying `params`.
async fn create_reservation(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateCapacityReservation", params).await
}

/// The form fields that reserve `instance_count` instances for immediate use: the members
/// `CreateCapacityReservation` requires, plus the zone the capacity sits in.
fn reservation_params(instance_count: i32) -> Vec<(String, String)> {
    vec![
        ("InstanceType".to_string(), SOURCE_INSTANCE_TYPE.to_string()),
        (
            "InstancePlatform".to_string(),
            SOURCE_INSTANCE_PLATFORM.to_string(),
        ),
        ("InstanceCount".to_string(), instance_count.to_string()),
        (
            "AvailabilityZone".to_string(),
            SOURCE_AVAILABILITY_ZONE.to_string(),
        ),
    ]
}

/// An `active` Capacity Reservation of `instance_count` instances with no end date, created
/// OVER THE WIRE — the one state an allocation may be made from.
async fn create_source(router: &MultiServiceRouter, instance_count: i32) -> String {
    let (status, body) = create_reservation(router, &reservation_params(instance_count)).await;
    assert_eq!(
        status, 200,
        "creating a source of {instance_count} instances: expected HTTP 200, \
         got {status} with body {body}"
    );
    element(&body, "capacityReservationId").unwrap_or_else(|| {
        panic!("creating a source of {instance_count} instances: expected an id in {body}")
    })
}

/// One `GetCapacityReservationUsage` call for `capacity_reservation_id`.
async fn usage(router: &MultiServiceRouter, capacity_reservation_id: &str) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "GetCapacityReservationUsage",
        &[(
            "CapacityReservationId".to_string(),
            capacity_reservation_id.to_string(),
        )],
    )
    .await
}

/// The usage report of `capacity_reservation_id`, which must be readable.
async fn reported_usage(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> String {
    let (status, body) = usage(router, capacity_reservation_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {capacity_reservation_id} back, \
         got {status} with body {body}"
    );
    body
}

/// A tag, as a tag specification carries them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// The instance type, platform and zone every reservation in this suite reserves capacity
/// for. The type is a member of the modelled `InstanceType` catalog and the platform a
/// member of `CapacityReservationInstancePlatform`
/// (`smithy_query(query="shape",shape="CapacityReservationInstancePlatform")`).
const SOURCE_INSTANCE_TYPE: &str = "c5.xlarge";
const SOURCE_INSTANCE_PLATFORM: &str = "Linux/UNIX";
const SOURCE_AVAILABILITY_ZONE: &str = "us-east-1a";

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for: `cr-`
/// followed by the long id form EC2 issues, which is what makes the service look it up at
/// all rather than refuse it as malformed before the lookup.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// What one seeded source Capacity Reservation is created with: the capacity it reserves,
/// the state it sits in and whether it has an end date. Everything else is the same across
/// the rows, so each row differs only along what the rules generalise over.
fn source_spec(
    instance_count: i32,
    state: &'static str,
    end_date: Option<SystemTime>,
) -> CapacityReservationSpec {
    CapacityReservationSpec {
        instance_type: SOURCE_INSTANCE_TYPE.to_string(),
        instance_platform: SOURCE_INSTANCE_PLATFORM.to_string(),
        availability_zone: SOURCE_AVAILABILITY_ZONE.to_string(),
        availability_zone_id: "use1-az1".to_string(),
        region: REGION.to_string(),
        tenancy: "default".to_string(),
        instance_count,
        ebs_optimized: false,
        ephemeral_storage: false,
        end_date,
        instance_match_criteria: "open".to_string(),
        created_at: SystemTime::now(),
        // Capacity for immediate use: no requested delivery date, nothing committed to and
        // no delivery method — the members only a future-dated request names.
        start_date: None,
        commitment: None,
        delivery_preference: None,
        state,
        tags: Vec::new(),
        interruption_info: None,
        zero_size_preference: None,
        pending: None,
    }
}

/// Write one source Capacity Reservation straight into the engine's store.
///
/// Only for the states no implemented operation puts a reservation in — a `failed` or an
/// `expired` one — so a row that needs one is seeded here. Every other row goes through
/// `CreateCapacityReservation` over the wire ([`create_source`]), and a `cancelled` one
/// through `CancelCapacityReservation` (see [`super::capacity_reservation_cancellation`]).
fn seed_source(engine: &engine::InMemoryEngine, spec: CapacityReservationSpec) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_capacity_reservation(spec)
        .expect("the store accepts a seeded Capacity Reservation")
}

/// The `AvailableInstanceCount` the store holds for `capacity_reservation_id`.
fn stored_available(engine: &engine::InMemoryEngine, capacity_reservation_id: &str) -> i32 {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservations
        .get(capacity_reservation_id)
        .and_then(|record| record.view.available_instance_count)
        .unwrap_or_default()
}

/// How many Capacity Reservations the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().capacity_reservations.len()
}

// ---------------------------------------------------------------------------
// The allocation is made from a STORED source reservation
// ---------------------------------------------------------------------------

/// An id the account holds no Capacity Reservation for is refused — one of the `cr-`
/// resource-id SHAPE with `InvalidCapacityReservationId.NotFound` and a value outside that
/// shape with `InvalidCapacityReservationId.Malformed`, before any lookup — and nothing is
/// written for either.
#[tokio::test]
async fn an_unknown_source_is_refused_whatever_the_id_looks_like() {
    let (router, engine) = super::ec2_query::emulator();
    // One stored reservation, so "not found" is about THIS id rather than about an empty
    // store.
    let known = create_source(&router, 4).await;
    let before = stored_count(&engine);

    // The rows differ in the FORM of the id: the long id form EC2 issues, the null id, the
    // short form, a leading digit above zero, another resource's id, a known id with a
    // suffix, and something that is not an id at all. The first four are of the `cr-`
    // resource-id shape, so each is LOOKED UP and reported not found.
    let cases: [(&str, &str); 7] = [
        (UNKNOWN_CAPACITY_RESERVATION_ID, "NotFound"),
        ("cr-00000000000000000", "NotFound"),
        ("cr-12345678", "NotFound"),
        ("cr-1234567890abcdef0", "NotFound"),
        ("cb-00000000000000001", "Malformed"),
        (&format!("{known}x"), "Malformed"),
        ("not-an-id", "Malformed"),
    ];
    for (case, half) in cases {
        let expected = format!("InvalidCapacityReservationId.{half}");
        let (status, body) = allocate(&router, &allocation_params(case, 1)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(case),
            "case {case:?}: expected the message to name the id, got {message:?}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        before,
        "a refused allocation must write nothing: the store held {before} reservations"
    );
    assert_eq!(
        stored_available(&engine, &known),
        4,
        "a refused allocation must not touch another reservation's available capacity"
    );
}

/// A source that is not `active`, one with an end date, one that is itself interruptible and
/// one that already carries an allocation are each refused with `IncorrectState`, and the
/// capacity they hold is left alone.
#[tokio::test]
async fn a_source_that_is_not_active_with_no_end_date_cannot_be_allocated_from() {
    let (router, engine) = super::ec2_query::emulator();

    // Every member of `CapacityReservationState`
    // (`smithy_query(query="shape",shape="CapacityReservationState")`) except `active`, as
    // the generated catalog lists them — so a state added to the model is covered without
    // this list being rewritten.
    let mut cases: Vec<(String, String)> = Vec::new();
    for state in CAPACITY_RESERVATION_STATE_VALUES
        .iter()
        .filter(|state| **state != CAPACITY_RESERVATION_STATE_ACTIVE)
    {
        let id = seed_source(&engine, source_spec(4, state, None));
        cases.push((format!("state {state}"), id));
    }

    // Active, but with an end date set: "with no end date set". Created over the wire, so
    // the end date is one a caller can really ask for; the two rows differ in how far out
    // it is.
    for (case, end_date) in [
        ("an end date in 2035", "2035-06-01T12:00:00Z"),
        ("an end date in 2040", "2040-01-15T08:30:00Z"),
    ] {
        let mut params = reservation_params(4);
        params.push(("EndDateType".to_string(), "limited".to_string()));
        params.push(("EndDate".to_string(), end_date.to_string()));
        let (status, body) = create_reservation(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the reservation to be created, got {status} with \
             body {body}"
        );
        let id = element(&body, "capacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an id in {body}"));
        cases.push((case.to_string(), id));
    }

    // An interruptible reservation holds capacity that was allocated FROM a source; it is
    // not a source to allocate from. Made the only way one is made: by allocating.
    let parent = create_source(&router, 4).await;
    let (status, body) = allocate(&router, &allocation_params(&parent, 4)).await;
    assert_eq!(
        status, 200,
        "making an interruptible reservation: expected HTTP 200, got {status} with body {body}"
    );
    let report = reported_usage(&router, "the allocating source", &parent).await;
    let interruptible = element(&report, "interruptibleCapacityReservationId")
        .unwrap_or_else(|| panic!("expected an allocation id in {report}"));
    cases.push(("an interruptible source".to_string(), interruptible));

    for (case, id) in &cases {
        let (status, body) = allocate(&router, &allocation_params(id, 1)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        assert_eq!(
            stored_available(&engine, id),
            4,
            "case {case:?}: a refused allocation must leave the available capacity at 4"
        );
    }

    // A source that already carries an allocation: the model carries ONE allocation per
    // reservation, so a second one is refused rather than overwriting the first.
    let source = create_source(&router, 4).await;
    let (status, body) = allocate(&router, &allocation_params(&source, 1)).await;
    assert_eq!(
        status, 200,
        "the first allocation: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = allocate(&router, &allocation_params(&source, 1)).await;
    assert_eq!(
        status, 400,
        "a second allocation: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a second allocation: expected IncorrectState, got {body}"
    );
    assert_eq!(
        stored_available(&engine, &source),
        3,
        "a second allocation must not take more capacity out of the source"
    );
}

/// `InstanceCount` must be at least one instance and at most the instances the source has
/// available: "You can only allocate available instances (also called unused capacity)".
#[tokio::test]
async fn instance_count_must_be_between_one_and_the_available_capacity() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the source's capacity, the count asked for, accepted?) — rows on both sides of
    // each bound, and one far past it.
    let cases: [(&str, i32, i32, bool); 9] = [
        ("none of them", 4, 0, false),
        ("a negative count", 4, -1, false),
        ("one past the capacity", 4, 5, false),
        ("far past the capacity", 4, 1_000, false),
        ("the lower bound", 4, 1, true),
        ("inside the bounds", 4, 3, true),
        ("the whole capacity", 4, 4, true),
        ("the whole of a one-instance reservation", 1, 1, true),
        ("one past a one-instance reservation", 1, 2, false),
    ];
    for (case, capacity, count, accepted) in cases {
        let source = create_source(&router, capacity).await;
        let (status, body) = allocate(&router, &allocation_params(&source, count)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                stored_available(&engine, &source),
                capacity - count,
                "case {case:?}: expected {} instances left available",
                capacity - count
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            stored_available(&engine, &source),
            capacity,
            "case {case:?}: a refused allocation must leave all {capacity} instances available"
        );
    }
}

// ---------------------------------------------------------------------------
// What a successful allocation does
// ---------------------------------------------------------------------------

/// A successful allocation moves the instances it names out of the source into a new
/// interruptible Capacity Reservation, and both are reported from the store afterwards.
#[tokio::test]
async fn an_allocation_moves_unused_capacity_into_a_new_interruptible_reservation() {
    // Everything this test asserts is read back over the wire, so the store is reached
    // only through the service.
    let (router, _engine) = super::ec2_query::emulator();

    // (case, the source's capacity, the count allocated, the ZeroSizePreference asked for)
    // — both members of `ZeroSizePreference`
    // (`smithy_query(query="shape",shape="ZeroSizePreference")`) and a request that names
    // none, which takes the documented default.
    let cases: [(&str, i32, i32, Option<&str>); 4] = [
        ("no preference named", 8, 3, None),
        ("retain", 8, 1, Some(ZERO_SIZE_PREFERENCE_RETAIN)),
        ("default", 8, 8, Some(ZERO_SIZE_PREFERENCE_DEFAULT)),
        ("a single instance of many", 64, 1, None),
    ];
    for (case, capacity, count, preference) in cases {
        let source = create_source(&router, capacity).await;
        let mut params = allocation_params(&source, count);
        if let Some(preference) = preference {
            params.push(("ZeroSizePreference".to_string(), preference.to_string()));
        }
        let expected_preference = preference.unwrap_or(ZERO_SIZE_PREFERENCE_DEFAULT);

        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        for (member, expected) in [
            ("sourceCapacityReservationId", source.clone()),
            ("targetInstanceCount", count.to_string()),
            // This operation reports the status of the allocation REQUEST, which has just
            // been accepted; the ALLOCATION's own status is `pending` until the clock moves
            // it, and is asserted below where a reader reports it.
            (
                "status",
                INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING.to_string(),
            ),
            ("interruptionType", INTERRUPTION_TYPE_ADHOC.to_string()),
        ] {
            assert_eq!(
                element(&body, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}>{expected}</{member}>, got {body}"
            );
        }

        // The source, read back: the allocated instances left it — they are neither
        // reserved by it nor available for it to launch any more — and the allocation it now
        // carries names the reservation they went to.
        let report = reported_usage(&router, case, &source).await;
        for (member, expected) in [
            ("capacityReservationId", source.clone()),
            ("instanceType", "c5.xlarge".to_string()),
            ("totalInstanceCount", (capacity - count).to_string()),
            ("availableInstanceCount", (capacity - count).to_string()),
            ("state", CAPACITY_RESERVATION_STATE_ACTIVE.to_string()),
            ("interruptible", "false".to_string()),
            ("instanceCount", "0".to_string()),
            ("targetInstanceCount", count.to_string()),
            ("status", INTERRUPTIBLE_ALLOCATION_STATUS_PENDING.to_string()),
            ("zeroSizePreference", expected_preference.to_string()),
            ("interruptionType", INTERRUPTION_TYPE_ADHOC.to_string()),
        ] {
            assert_eq!(
                element(&report, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the source's <{member}>{expected}</{member}>, \
                 got {report}"
            );
        }

        // The interruptible reservation the allocation created, read back under its own id:
        // it reserves the capacity that left the source, and names the source it came from.
        let interruptible = element(&report, "interruptibleCapacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an allocation id in {report}"));
        assert_ne!(
            interruptible, source,
            "case {case:?}: the interruptible reservation must have its own id"
        );
        let child = reported_usage(&router, case, &interruptible).await;
        for (member, expected) in [
            ("capacityReservationId", interruptible.clone()),
            ("instanceType", "c5.xlarge".to_string()),
            ("totalInstanceCount", count.to_string()),
            ("availableInstanceCount", count.to_string()),
            // "The Capacity Reservation request was successful but the capacity
            // provisioning is still pending" (`CapacityReservation$State`).
            ("state", "pending".to_string()),
            ("interruptible", "true".to_string()),
            ("sourceCapacityReservationId", source.clone()),
            ("interruptionType", INTERRUPTION_TYPE_ADHOC.to_string()),
        ] {
            assert_eq!(
                element(&child, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the interruptible reservation's \
                 <{member}>{expected}</{member}>, got {child}"
            );
        }
    }
}

/// The allocation starts `pending` and the clock owes it `active` with the whole target
/// allocated — and a read never moves it earlier or later than that.
#[tokio::test]
async fn the_allocation_reaches_active_once_the_clock_passes_the_transition() {
    // The clock-owed move is observed the way a caller observes it: over the wire.
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_source(&router, 10).await;

    let (status, body) = allocate(&router, &allocation_params(&source, 6)).await;
    assert_eq!(
        status, 200,
        "the allocation: expected HTTP 200, got {status} with body {body}"
    );

    // Straight after the call the caller still sees the transient state: the allocation is
    // pending, with nothing allocated yet.
    let report = reported_usage(&router, "before the transition", &source).await;
    assert_eq!(
        element(&report, "status").as_deref(),
        Some(INTERRUPTIBLE_ALLOCATION_STATUS_PENDING),
        "before the transition: expected a pending allocation, got {report}"
    );
    assert_eq!(
        element(&report, "instanceCount").as_deref(),
        Some("0"),
        "before the transition: expected no instances allocated yet, got {report}"
    );
    let interruptible = element(&report, "interruptibleCapacityReservationId")
        .unwrap_or_else(|| panic!("expected an allocation id in {report}"));
    let child = reported_usage(&router, "before the transition", &interruptible).await;
    assert_eq!(
        element(&child, "state").as_deref(),
        Some("pending"),
        "before the transition: expected a pending reservation, got {child}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let report = reported_usage(&router, "after the transition", &source).await;
    assert_eq!(
        element(&report, "status").as_deref(),
        Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE),
        "after the transition: expected an active allocation, got {report}"
    );
    assert_eq!(
        element(&report, "instanceCount").as_deref(),
        Some("6"),
        "after the transition: expected the whole target allocated, got {report}"
    );
    assert_eq!(
        element(&report, "availableInstanceCount").as_deref(),
        Some("4"),
        "after the transition: the allocated capacity stays out of the source, got {report}"
    );
    let child = reported_usage(&router, "after the transition", &interruptible).await;
    assert_eq!(
        element(&child, "state").as_deref(),
        Some(CAPACITY_RESERVATION_STATE_ACTIVE),
        "after the transition: expected an active reservation, got {child}"
    );

    // A read does not change what a later caller sees.
    let again = reported_usage(&router, "a second read", &source).await;
    assert_eq!(
        element(&again, "status").as_deref(),
        element(&report, "status").as_deref(),
        "a second read must report the same allocation status, got {again}"
    );
    assert_eq!(
        element(&again, "availableInstanceCount").as_deref(),
        element(&report, "availableInstanceCount").as_deref(),
        "a second read must report the same available capacity, got {again}"
    );
}

/// "Unique, case-sensitive identifier that you provide to ensure the idempotency of the
/// request" (`ClientToken`): a replay reports the allocation that token made and allocates
/// nothing more, and the same token naming another source is refused rather than answered
/// with the first allocation.
#[tokio::test]
async fn a_replay_of_the_same_client_token_allocates_nothing_more() {
    let (router, engine) = super::ec2_query::emulator();
    let source = create_source(&router, 9).await;
    let other = create_source(&router, 9).await;

    // The rows differ in the token's own form: a UUID-shaped one and a short opaque one.
    for token in ["5f3a9c2e-1b4d-4c7a-9f2e-0d8b6a1c3e5f", "token-1"] {
        let source = create_source(&router, 9).await;
        let mut params = allocation_params(&source, 2);
        params.push(("ClientToken".to_string(), token.to_string()));

        let (status, first) = allocate(&router, &params).await;
        assert_eq!(
            status, 200,
            "token {token:?}: expected HTTP 200, got {status} with body {first}"
        );
        let (status, replay) = allocate(&router, &params).await;
        assert_eq!(
            status, 200,
            "token {token:?}: expected the replay to succeed, got {status} with body {replay}"
        );
        for member in [
            "sourceCapacityReservationId",
            "targetInstanceCount",
            "status",
            "interruptionType",
        ] {
            assert_eq!(
                element(&replay, member).as_deref(),
                element(&first, member).as_deref(),
                "token {token:?}: expected the replay's <{member}> to match the first \
                 answer, got {replay}"
            );
        }
        assert_eq!(
            stored_available(&engine, &source),
            7,
            "token {token:?}: a replay must not allocate a second time"
        );
    }

    // A token already used against one source, now naming another: not a replay of this
    // request.
    let token = "shared-token";
    let mut params = allocation_params(&source, 2);
    params.push(("ClientToken".to_string(), token.to_string()));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first use of the shared token: expected HTTP 200, got {status} with body {body}"
    );
    let mut params = allocation_params(&other, 2);
    params.push(("ClientToken".to_string(), token.to_string()));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 400,
        "the token reused for another source: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatch",
        "the token reused for another source: expected IdempotentParameterMismatch, got {body}"
    );
    assert_eq!(
        stored_available(&engine, &other),
        9,
        "the token reused for another source must allocate nothing from it"
    );

    // The same token against the SAME source, but asking for something else: a token names
    // ONE request, so each of these is a mismatch rather than a replay — and none of them
    // allocates.
    let picky = create_source(&router, 9).await;
    let token = "picky-token";
    let mut params = allocation_params(&picky, 2);
    params.push(("ClientToken".to_string(), token.to_string()));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first use of the picky token: expected HTTP 200, got {status} with body {body}"
    );
    // (case, the members to add to a request naming the same token)
    let mismatches: [(&str, i32, Option<&str>); 3] = [
        ("a larger count", 3, None),
        ("a smaller count", 1, None),
        ("another zero-size preference", 2, Some(ZERO_SIZE_PREFERENCE_RETAIN)),
    ];
    for (case, count, preference) in mismatches {
        let mut params = allocation_params(&picky, count);
        params.push(("ClientToken".to_string(), token.to_string()));
        if let Some(preference) = preference {
            params.push(("ZeroSizePreference".to_string(), preference.to_string()));
        }
        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            stored_available(&engine, &picky),
            7,
            "case {case:?}: a mismatched token must allocate nothing more"
        );
    }
}

/// "Checks whether you have the required permissions for the action, without actually
/// making the request" (`DryRun`): a request that would have succeeded answers
/// `DryRunOperation` and writes nothing, and one that would have been refused is refused
/// with that reason instead.
#[tokio::test]
async fn a_dry_run_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let source = create_source(&router, 5).await;
    let before = stored_count(&engine);

    let mut params = allocation_params(&source, 2);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_available(&engine, &source),
        5,
        "a dry run must leave the source's available capacity alone"
    );
    assert_eq!(
        stored_count(&engine),
        before,
        "a dry run must not mint an interruptible reservation"
    );

    // A dry run of a request that would have been refused is refused with ITS reason.
    let mut params = allocation_params(UNKNOWN_CAPACITY_RESERVATION_ID, 2);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 400,
        "a dry run that would have been refused: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityReservationId.NotFound",
        "a dry run that would have been refused: expected the refusal's own code, got {body}"
    );
}

/// "The tags to apply to the interruptible Capacity Reservation during creation"
/// (`TagSpecifications`): the tags land on the reservation the allocation creates, and a
/// specification naming any other resource type is refused rather than silently dropped.
#[tokio::test]
async fn tag_specifications_tag_the_interruptible_reservation_only() {
    let (router, engine) = super::ec2_query::emulator();

    // The rows differ in how many tags are carried, from none to several.
    let cases: [(&str, Vec<Tag>); 3] = [
        ("no tags", Vec::new()),
        ("one tag", vec![tag("Name", "burst")]),
        (
            "several tags",
            vec![tag("Name", "burst"), tag("Owner", "TeamA"), tag("Env", "dev")],
        ),
    ];
    for (case, tags) in cases {
        let source = create_source(&router, 4).await;
        let mut params = allocation_params(&source, 1);
        if !tags.is_empty() {
            params.push((
                "TagSpecification.1.ResourceType".to_string(),
                "capacity-reservation".to_string(),
            ));
            for (index, carried) in tags.iter().enumerate() {
                params.push((
                    format!("TagSpecification.1.Tag.{}.Key", index + 1),
                    carried.key.clone().unwrap_or_default(),
                ));
                params.push((
                    format!("TagSpecification.1.Tag.{}.Value", index + 1),
                    carried.value.clone().unwrap_or_default(),
                ));
            }
        }
        let (status, body) = allocate(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // Read the stored reservation the allocation created: `GetCapacityReservationUsage`
        // reports usage, not tags, so the tags are checked where they were written.
        let report = reported_usage(&router, case, &source).await;
        let interruptible = element(&report, "interruptibleCapacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an allocation id in {report}"));
        let stored_tags = engine
            .state::<Ec2State>()
            .lock()
            .capacity_reservations
            .get(&interruptible)
            .and_then(|record| record.view.tags.clone())
            .unwrap_or_default();
        let stored: Vec<(String, String)> = stored_tags
            .iter()
            .map(|carried| {
                (
                    carried.key.clone().unwrap_or_default(),
                    carried.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = tags
            .iter()
            .map(|carried| {
                (
                    carried.key.clone().unwrap_or_default(),
                    carried.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the interruptible reservation to carry {expected:?}, \
             got {stored:?}"
        );
        // The source keeps the tags it had, which is none: a tag specification tags the
        // resource the call creates.
        let source_tags = engine
            .state::<Ec2State>()
            .lock()
            .capacity_reservations
            .get(&source)
            .and_then(|record| record.view.tags.clone())
            .unwrap_or_default();
        assert!(
            source_tags.is_empty(),
            "case {case:?}: expected the source to keep its own tags, got {source_tags:?}"
        );
    }

    // A resource type this action does not tag.
    let source = create_source(&router, 4).await;
    let mut params = allocation_params(&source, 1);
    params.push((
        "TagSpecification.1.ResourceType".to_string(),
        "instance".to_string(),
    ));
    params.push(("TagSpecification.1.Tag.1.Key".to_string(), "Name".to_string()));
    params.push((
        "TagSpecification.1.Tag.1.Value".to_string(),
        "burst".to_string(),
    ));
    let (status, body) = allocate(&router, &params).await;
    assert_eq!(
        status, 400,
        "a tag specification naming another resource type: expected HTTP 400, \
         got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a tag specification naming another resource type: expected InvalidParameterValue, \
         got {body}"
    );
    assert_eq!(
        stored_available(&engine, &source),
        4,
        "a refused tag specification must leave the source's capacity alone"
    );
}

// ---------------------------------------------------------------------------
// Reading a reservation back
// ---------------------------------------------------------------------------

/// `GetCapacityReservationUsage` reports the stored reservation and nothing else: an id the
/// account holds no reservation for is refused, a bogus `NextToken` is refused, and the one
/// page it serves carries the owner's usage.
#[tokio::test]
async fn reading_a_reservation_back_reports_the_store() {
    let (router, engine) = super::ec2_query::emulator();
    let source = create_source(&router, 4).await;
    let stored_before = stored_count(&engine);

    let (status, body) = usage(&router, UNKNOWN_CAPACITY_RESERVATION_ID).await;
    assert_eq!(
        status, 400,
        "an unknown reservation: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityReservationId.NotFound",
        "an unknown reservation: expected InvalidCapacityReservationId.NotFound, got {body}"
    );

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "GetCapacityReservationUsage",
        &[
            ("CapacityReservationId".to_string(), source.clone()),
            ("NextToken".to_string(), "not-a-token".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a bogus NextToken: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "a bogus NextToken: expected InvalidPaginationToken, got {body}"
    );

    // Nothing in this emulator launches an instance INTO a Capacity Reservation, so no
    // account has usage of one to report: the usage set is empty, and it is not the
    // difference between the reserved and the available counts.
    let report = reported_usage(&router, "a fresh reservation", &source).await;
    assert!(
        elements(&report, "usedInstanceCount").is_empty(),
        "a fresh reservation: expected no instance usage at all, got {report}"
    );
    assert!(
        element(&report, "accountId").is_none(),
        "a fresh reservation: expected no usage row, got {report}"
    );
    assert!(
        element(&report, "nextToken").is_none(),
        "a fresh reservation: one page carries the whole report, got {report}"
    );
    assert_eq!(
        stored_count(&engine),
        stored_before,
        "reading a reservation back must not write to the store"
    );
}

// ---------------------------------------------------------------------------
// The source reservation an allocation is made from
// ---------------------------------------------------------------------------

/// One row of the create table: what one `CreateCapacityReservation` request asks for.
/// A member left `None` is a member the request does not name, which takes the default its
/// own documentation publishes.
struct CreateCase {
    case: &'static str,
    instance_count: i32,
    instance_type: &'static str,
    /// Whether the zone is named by its ID (`AvailabilityZoneId`) rather than by its name.
    by_zone_id: bool,
    tenancy: Option<&'static str>,
    instance_match_criteria: Option<&'static str>,
}

/// One row of the refusal table: the fields to change in an otherwise valid request, and
/// the answer that request must get.
struct RefusalCase {
    case: &'static str,
    /// Each entry replaces that form field; an empty value REMOVES it.
    changes: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// `CreateCapacityReservation` reserves what the request asks for, defaults the members it
/// leaves out to the words its own documentation publishes, and the reservation reads back
/// through `GetCapacityReservationUsage` with every one of them.
#[tokio::test]
async fn a_created_reservation_reserves_what_the_request_asked_for() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows differing along every member with a documented default and both ways of naming a
    // zone. The instance types are members of the modelled `InstanceType` catalog
    // (`smithy_query(query="shape",shape="InstanceType")`), the tenancies of
    // `CapacityReservationTenancy` and the criteria of `InstanceMatchCriteria`.
    let cases: [CreateCase; 5] = [
        CreateCase {
            case: "the smallest reservation",
            instance_count: 1,
            instance_type: "t3.micro",
            by_zone_id: false,
            tenancy: None,
            instance_match_criteria: None,
        },
        CreateCase {
            case: "named by zone id",
            instance_count: 4,
            instance_type: "c5.xlarge",
            by_zone_id: true,
            tenancy: None,
            instance_match_criteria: Some("targeted"),
        },
        CreateCase {
            case: "dedicated hardware",
            instance_count: 8,
            instance_type: "m5.large",
            by_zone_id: false,
            tenancy: Some("dedicated"),
            instance_match_criteria: Some("open"),
        },
        CreateCase {
            case: "shared hardware, no criteria named",
            instance_count: 64,
            instance_type: "r5.2xlarge",
            by_zone_id: false,
            tenancy: Some("default"),
            instance_match_criteria: None,
        },
        // "Valid range: 1 - 1000" (`CreateCapacityReservationRequest$InstanceCount`): the
        // published upper bound.
        CreateCase {
            case: "the largest reservation",
            instance_count: 1_000,
            instance_type: "c5.large",
            by_zone_id: false,
            tenancy: None,
            instance_match_criteria: None,
        },
    ];
    for row in cases {
        let CreateCase {
            case,
            instance_count: count,
            instance_type,
            by_zone_id,
            tenancy,
            instance_match_criteria: criteria,
        } = row;
        let mut params: Vec<(String, String)> = vec![
            ("InstanceType".to_string(), instance_type.to_string()),
            (
                "InstancePlatform".to_string(),
                SOURCE_INSTANCE_PLATFORM.to_string(),
            ),
            ("InstanceCount".to_string(), count.to_string()),
        ];
        if by_zone_id {
            params.push(("AvailabilityZoneId".to_string(), "use1-az1".to_string()));
        } else {
            params.push((
                "AvailabilityZone".to_string(),
                SOURCE_AVAILABILITY_ZONE.to_string(),
            ));
        }
        if let Some(tenancy) = tenancy {
            params.push(("Tenancy".to_string(), tenancy.to_string()));
        }
        if let Some(criteria) = criteria {
            params.push(("InstanceMatchCriteria".to_string(), criteria.to_string()));
        }

        let (status, body) = create_reservation(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        for (member, expected) in [
            ("instanceType", instance_type.to_string()),
            ("instancePlatform", SOURCE_INSTANCE_PLATFORM.to_string()),
            ("instanceCount", String::new()),
            ("totalInstanceCount", count.to_string()),
            // Nothing has launched into it yet, so all of it is available.
            ("availableInstanceCount", count.to_string()),
            ("availabilityZone", SOURCE_AVAILABILITY_ZONE.to_string()),
            ("availabilityZoneId", "use1-az1".to_string()),
            ("state", "active".to_string()),
            // "Default: open" (`InstanceMatchCriteria`).
            ("instanceMatchCriteria", criteria.unwrap_or("open").to_string()),
            // "default - The Capacity Reservation is created on hardware that is shared"
            // (`Tenancy`).
            ("tenancy", tenancy.unwrap_or("default").to_string()),
            // "unlimited - The Capacity Reservation remains active until you explicitly
            // cancel it" (`EndDateType`), the end type of a request naming no end date.
            ("endDateType", "unlimited".to_string()),
            ("reservationType", "default".to_string()),
            ("ownerId", engine::emulator_account_id().to_string()),
        ] {
            if expected.is_empty() {
                assert!(
                    element(&body, member).is_none(),
                    "case {case:?}: expected no <{member}>, got {body}"
                );
                continue;
            }
            assert_eq!(
                element(&body, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}>{expected}</{member}>, got {body}"
            );
        }

        // The same reservation, read back through a reader: a create and a read cannot
        // report one reservation differently.
        let id = element(&body, "capacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an id in {body}"));
        let report = reported_usage(&router, case, &id).await;
        for (member, expected) in [
            ("capacityReservationId", id.clone()),
            ("instanceType", instance_type.to_string()),
            ("totalInstanceCount", count.to_string()),
            ("availableInstanceCount", count.to_string()),
            ("state", "active".to_string()),
        ] {
            assert_eq!(
                element(&report, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the reader's <{member}>{expected}</{member}>, \
                 got {report}"
            );
        }
    }
}

/// A request `CreateCapacityReservation` cannot honour is refused with the code that says
/// why, and nothing is written for it. The request shapes this emulator holds no state for
/// (an Outpost, a cluster placement group) and the rules only a future-dated reservation is
/// held to are covered in `capacity_reservations.rs`.
#[tokio::test]
async fn a_reservation_request_outside_the_published_rules_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let before = stored_count(&engine);

    // Each row names one published rule.
    let cases: [RefusalCase; 10] = [
        RefusalCase {
            case: "an instance type the model does not carry",
            changes: &[("InstanceType", "c5.enormous")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "no instances",
            changes: &[("InstanceCount", "0")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "one past the published upper bound",
            changes: &[("InstanceCount", "1001")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a zone the Region does not have",
            changes: &[("AvailabilityZone", "eu-west-3c")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a zone id the Region does not have",
            changes: &[("AvailabilityZone", ""), ("AvailabilityZoneId", "euw3-az1")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "both zone forms",
            changes: &[("AvailabilityZoneId", "use1-az1")],
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "neither zone form",
            changes: &[("AvailabilityZone", "")],
            status: 400,
            code: "MissingParameter",
        },
        RefusalCase {
            case: "an end date with an unlimited end type",
            changes: &[("EndDate", "2035-06-01T12:00:00Z")],
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a limited end type with no end date",
            changes: &[("EndDateType", "limited")],
            status: 400,
            code: "MissingParameter",
        },
        RefusalCase {
            case: "an end date in the past",
            changes: &[
                ("EndDateType", "limited"),
                ("EndDate", "2001-01-01T00:00:00Z"),
            ],
            status: 400,
            code: "InvalidParameterValue",
        },
        // A FUTURE-DATED request is reserved rather than refused now, and the rules only it
        // is held to — the lead-time window its start date must fall in, the commitment it
        // must carry, the delivery method AWS supports, the match criteria and the instance
        // families it may reserve — are asserted in `capacity_reservations.rs`, whose rows
        // date their start dates off the clock rather than naming an absolute one that
        // would drift into the window as time passes.
    ];
    for row in cases {
        let RefusalCase {
            case,
            changes,
            status: expected_status,
            code: expected_code,
        } = row;
        let mut params = reservation_params(4);
        for (name, value) in changes {
            params.retain(|(existing, _)| existing != name);
            if !value.is_empty() {
                params.push(((*name).to_string(), (*value).to_string()));
            }
        }
        let (status, body) = create_reservation(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        before,
        "a refused reservation request must write nothing: the store held {before}"
    );
}

/// The store's own ceiling is enforced where a reservation is WRITTEN: once it holds
/// [`MAX_STORED_CAPACITY_RESERVATIONS`], the next insert writes nothing and the operations
/// that reserve capacity answer a quota refusal instead of growing the process.
#[tokio::test]
async fn the_store_refuses_a_reservation_past_its_ceiling() {
    let (router, engine) = super::ec2_query::emulator();

    // Fill the store through the one writer every path goes through.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        while guard.capacity_reservations.len() < MAX_STORED_CAPACITY_RESERVATIONS {
            assert!(
                guard
                    .insert_capacity_reservation(source_spec(
                        1,
                        CAPACITY_RESERVATION_STATE_ACTIVE,
                        None
                    ))
                    .is_some(),
                "filling the store: expected the insert to be accepted below the ceiling"
            );
        }
        let held = guard.capacity_reservations.len();
        assert!(
            guard
                .insert_capacity_reservation(source_spec(1, CAPACITY_RESERVATION_STATE_ACTIVE, None))
                .is_none(),
            "at the ceiling: expected the insert to be refused"
        );
        assert_eq!(
            guard.capacity_reservations.len(),
            held,
            "a refused insert must write nothing"
        );
    }

    // And the operation that reserves capacity answers a quota refusal rather than 200.
    let (status, body) = create_reservation(&router, &reservation_params(1)).await;
    assert_eq!(
        status, 503,
        "at the ceiling: expected the quota refusal's HTTP 503, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "RequestLimitExceeded",
        "at the ceiling: expected RequestLimitExceeded, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        MAX_STORED_CAPACITY_RESERVATIONS,
        "a refused create must not grow the store"
    );
}

/// A `ClientToken` makes the create idempotent: a replay reports the reservation that token
/// minted and reserves nothing more.
#[tokio::test]
async fn a_replayed_create_reserves_nothing_more() {
    let (router, engine) = super::ec2_query::emulator();

    for token in ["7c2f1a84-9d3b-4e5f-8a1c-2b6d0e4f9a3b", "token-2"] {
        let before = stored_count(&engine);
        let mut params = reservation_params(4);
        params.push(("ClientToken".to_string(), token.to_string()));

        let (status, first) = create_reservation(&router, &params).await;
        assert_eq!(
            status, 200,
            "token {token:?}: expected HTTP 200, got {status} with body {first}"
        );
        let (status, replay) = create_reservation(&router, &params).await;
        assert_eq!(
            status, 200,
            "token {token:?}: expected the replay to succeed, got {status} with body {replay}"
        );
        assert_eq!(
            element(&replay, "capacityReservationId"),
            element(&first, "capacityReservationId"),
            "token {token:?}: expected the replay to report the same reservation, got {replay}"
        );
        assert_eq!(
            stored_count(&engine),
            before + 1,
            "token {token:?}: a replay must not reserve a second time"
        );
    }

    // The same token asking for a DIFFERENT reservation is not a replay: a token names one
    // request, and nothing is reserved for the mismatched one.
    let token = "picky-create-token";
    let mut params = reservation_params(4);
    params.push(("ClientToken".to_string(), token.to_string()));
    let (status, body) = create_reservation(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first use of the picky token: expected HTTP 200, got {status} with body {body}"
    );
    let before = stored_count(&engine);
    // (case, the field to change in the otherwise identical request)
    let mismatches: [(&str, &str, &str); 4] = [
        ("another count", "InstanceCount", "5"),
        ("another instance type", "InstanceType", "m5.large"),
        ("another zone", "AvailabilityZone", "us-east-1b"),
        ("another tenancy", "Tenancy", "dedicated"),
    ];
    for (case, name, value) in mismatches {
        let mut params = reservation_params(4);
        params.retain(|(existing, _)| existing != name);
        params.push((name.to_string(), value.to_string()));
        params.push(("ClientToken".to_string(), token.to_string()));
        let (status, body) = create_reservation(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            before,
            "case {case:?}: a mismatched token must reserve nothing"
        );
    }
}

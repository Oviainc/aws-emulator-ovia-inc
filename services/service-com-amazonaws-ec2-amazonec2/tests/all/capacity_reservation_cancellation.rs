// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for cancelling a Capacity Reservation and for the terminal state that
//! cancellation gives an interruptible allocation: `CancelCapacityReservation` releases the
//! capacity, and `DescribeCapacityReservations` and `GetCapacityReservationUsage` read the
//! outcome back. Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A MUTATION of a Capacity Reservation is visible in the next read of it: "Cancels the
//!    specified Capacity Reservation, releases the reserved capacity, and changes the
//!    Capacity Reservation's state to `cancelled`" (`CancelCapacityReservation`), so every
//!    reader of that reservation reports `cancelled` — and reports the same thing, because
//!    both read one record.
//! 2. An interruptible allocation ties two reservations together and reaches a TERMINAL
//!    status when either side is cancelled: the allocation is `canceling` while the capacity
//!    moves and `canceled` from the instant that move falls due, whether or not anybody calls
//!    again; the interruptible reservation is `cancelled`; and the capacity it held is the
//!    source's again.
//! 3. A reservation in a state its capacity is not held in cannot be cancelled: "You can
//!    cancel a Capacity Reservation that is in the following states: `assessing` …
//!    `scheduled` … `active` … `delayed`" (same doc), so a second cancel is refused with
//!    `IncorrectState`.
//! 4. An id the account holds no reservation for is refused — a well-formed one with
//!    `InvalidCapacityReservationId.NotFound`, a value outside the id form EC2 issues with
//!    `InvalidCapacityReservationId.Malformed` — and nothing is written for either.
//! 5. A `DryRun` answers `DryRunOperation` and leaves the reservation where it was.
//! 6. "The ID of the Amazon Web Services account to which billing of the unused capacity of
//!    the Capacity Reservation is assigned"
//!    (`CapacityReservation$UnusedReservationBillingOwnerId`) is reported for every
//!    reservation, in whatever state.
//!
//! Every table row is a distinct input along what those rules generalise over — how much
//! capacity the reservation holds, how much of it is allocated, which side of the allocation
//! is cancelled, and which reader the outcome is read through.

use std::time::Duration;

use ec2::state::{
    CAPACITY_RESERVATION_STATE_ACTIVE, CAPACITY_RESERVATION_STATE_CANCELLED,
    INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING, INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE,
    INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED, INTERRUPTIBLE_ALLOCATION_STATUS_CANCELING,
    INTERRUPTIBLE_ALLOCATION_STATUS_PENDING, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, indexed};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The instance type, platform and zone every reservation in this suite reserves capacity
/// for — members of the modelled `InstanceType` and
/// `CapacityReservationInstancePlatform` catalogs.
const INSTANCE_TYPE: &str = "c5.xlarge";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for, so the service
/// looks it up rather than refusing it as malformed before the lookup.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// Long enough past a clock-owed move's due instant that it has fallen due, with the margin
/// every other suite of this crate gives one.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// An `active` Capacity Reservation of `instance_count` instances, created over the wire.
async fn create_reservation(router: &MultiServiceRouter, instance_count: i32) -> String {
    let params = [
        param("InstanceType", INSTANCE_TYPE),
        param("InstancePlatform", INSTANCE_PLATFORM),
        param("InstanceCount", &instance_count.to_string()),
        param("AvailabilityZone", AVAILABILITY_ZONE),
    ];
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateCapacityReservation", &params).await;
    assert_eq!(
        status, 200,
        "reserving {instance_count} instances: expected HTTP 200, got {status} with \
         body {body}"
    );
    element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("reserving {instance_count} instances: no id in {body}"))
}

/// One `CreateInterruptibleCapacityReservationAllocation` call for `count` instances of
/// `source_id`.
async fn allocate(
    router: &MultiServiceRouter,
    source_id: &str,
    count: i32,
) -> (u16, String) {
    let params = [
        param("CapacityReservationId", source_id),
        param("InstanceCount", &count.to_string()),
    ];
    super::ec2_query::call(
        router,
        REGION,
        "CreateInterruptibleCapacityReservationAllocation",
        &params,
    )
    .await
}

/// One `CancelCapacityReservation` call for `capacity_reservation_id`.
async fn cancel(
    router: &MultiServiceRouter,
    capacity_reservation_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![param("CapacityReservationId", capacity_reservation_id)];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "CancelCapacityReservation", &params).await
}

/// One `DescribeCapacityReservations` call naming `capacity_reservation_id`.
async fn describe(
    router: &MultiServiceRouter,
    capacity_reservation_id: &str,
) -> (u16, String) {
    let params = indexed("CapacityReservationId", &[capacity_reservation_id]);
    super::ec2_query::call(router, REGION, "DescribeCapacityReservations", &params).await
}

/// The one reservation `capacity_reservation_id` names, as `DescribeCapacityReservations`
/// reports it — asserted to be readable.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> String {
    let (status, body) = describe(router, capacity_reservation_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing {capacity_reservation_id}, got \
         {status} with body {body}"
    );
    body
}

/// The usage report of `capacity_reservation_id`, as `GetCapacityReservationUsage` reports
/// it — asserted to be readable.
async fn reported_usage(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> String {
    let params = [param("CapacityReservationId", capacity_reservation_id)];
    let (status, body) =
        super::ec2_query::call(router, REGION, "GetCapacityReservationUsage", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading the usage of \
         {capacity_reservation_id}, got {status} with body {body}"
    );
    body
}

/// A cancelled reservation is `cancelled` in every reader of it, and the capacity it
/// reserved is still the capacity it reserved.
#[tokio::test]
async fn a_cancelled_reservation_reports_cancelled_in_every_reader() {
    let (router, _engine) = super::ec2_query::emulator();

    // The rows differ in how much capacity the reservation holds: one instance, a few, and
    // many.
    for capacity in [1, 4, 64] {
        let case = format!("{capacity} instances");
        let id = create_reservation(&router, capacity).await;

        // Before: active, with every instance available.
        let body = described(&router, &case, &id).await;
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(CAPACITY_RESERVATION_STATE_ACTIVE),
            "case {case:?}: expected an active reservation before the cancel, got {body}"
        );

        // A dry run answers the outcome and writes nothing.
        let (status, dry) = cancel(&router, &id, true).await;
        assert_eq!(
            status, 412,
            "case {case:?}: a dry run that would have succeeded: expected HTTP 412, got \
             {status} with body {dry}"
        );
        assert_eq!(
            error_code(&dry),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {dry}"
        );
        let body = described(&router, &case, &id).await;
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(CAPACITY_RESERVATION_STATE_ACTIVE),
            "case {case:?}: a dry run must not cancel anything, got {body}"
        );

        let (status, body) = cancel(&router, &id, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected the cancel to answer true, got {body}"
        );

        // Both readers of one record: the state, and the capacity that was reserved.
        for (reader, body) in [
            ("DescribeCapacityReservations", described(&router, &case, &id).await),
            (
                "GetCapacityReservationUsage",
                reported_usage(&router, &case, &id).await,
            ),
        ] {
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(CAPACITY_RESERVATION_STATE_CANCELLED),
                "case {case:?}: expected {reader} to report a cancelled reservation, \
                 got {body}"
            );
            assert_eq!(
                element(&body, "totalInstanceCount").as_deref(),
                Some(capacity.to_string().as_str()),
                "case {case:?}: expected {reader} to still report the reserved count, \
                 got {body}"
            );
        }

        // "The ID of the Amazon Web Services account to which billing of the unused
        // capacity of the Capacity Reservation is assigned": present whatever the state.
        let body = described(&router, &case, &id).await;
        assert_eq!(
            element(&body, "unusedReservationBillingOwnerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the billing owner of the unused capacity, got {body}"
        );

        // A second cancel: the capacity is already released, so there is nothing to cancel.
        let (status, body) = cancel(&router, &id, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second cancel: expected HTTP 400, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: a second cancel: expected IncorrectState, got {body}"
        );
    }
}

/// Cancelling either side of an interruptible allocation releases it: the allocation reaches
/// its terminal status on the clock, the interruptible reservation is cancelled, and the
/// capacity goes back to the source.
#[tokio::test]
async fn cancelling_either_side_of_an_allocation_releases_it() {
    // (case, the source's capacity, the count allocated, whether the INTERRUPTIBLE side is
    // the one cancelled) — the rule holds whichever side the caller cancels, and whatever
    // share of the source was allocated.
    let cases: [(&str, i32, i32, bool); 4] = [
        ("the interruptible side, part of the capacity", 8, 3, true),
        ("the interruptible side, all of the capacity", 4, 4, true),
        ("the source side, part of the capacity", 8, 1, false),
        ("the source side, a single instance", 1, 1, false),
    ];
    for (case, capacity, count, cancel_interruptible) in cases {
        let (router, _engine) = super::ec2_query::emulator();
        let source = create_reservation(&router, capacity).await;

        let (status, body) = allocate(&router, &source, count).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 allocating, got {status} with body {body}"
        );
        // The create answers the status of the allocation REQUEST it has just accepted.
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING),
            "case {case:?}: expected the accepted request's status, got {body}"
        );

        // The source, read back: the allocated instances left it, and the allocation it
        // carries is `pending` with nothing allocated yet.
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "totalInstanceCount").as_deref(),
            Some((capacity - count).to_string().as_str()),
            "case {case:?}: expected the allocated instances to be drawn out of the \
             source's reserved count, got {body}"
        );
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_STATUS_PENDING),
            "case {case:?}: expected a pending allocation before the transition, got {body}"
        );
        assert_eq!(
            element(&body, "instanceCount").as_deref(),
            Some("0"),
            "case {case:?}: expected nothing allocated yet, got {body}"
        );
        let interruptible = element(&body, "interruptibleCapacityReservationId")
            .unwrap_or_else(|| panic!("case {case:?}: expected the allocation's reservation id in {body}"));

        // The clock owes the allocation `active` with the whole target allocated, and a READ
        // reports it without anybody writing anything.
        tokio::time::sleep(past_the_transition()).await;
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE),
            "case {case:?}: expected an active allocation after the transition, got {body}"
        );
        assert_eq!(
            element(&body, "instanceCount").as_deref(),
            Some(count.to_string().as_str()),
            "case {case:?}: expected the whole target allocated, got {body}"
        );

        // Cancel one side of the pair.
        let cancelled = if cancel_interruptible {
            interruptible.clone()
        } else {
            source.clone()
        };
        let (status, body) = cancel(&router, &cancelled, false).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 cancelling {cancelled}, got {status} with \
             body {body}"
        );

        // Straight after the call the caller still sees the transient status: the capacity
        // is on its way back.
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELING),
            "case {case:?}: expected a canceling allocation straight after the cancel, \
             got {body}"
        );
        // The capacity is the source's again, whichever side was cancelled.
        assert_eq!(
            element(&body, "totalInstanceCount").as_deref(),
            Some(capacity.to_string().as_str()),
            "case {case:?}: expected the allocated capacity back in the source's \
             reserved count, got {body}"
        );
        assert_eq!(
            element(&body, "availableInstanceCount").as_deref(),
            Some(capacity.to_string().as_str()),
            "case {case:?}: expected the allocated capacity available to the source \
             again, got {body}"
        );

        // And the terminal status the clock owes it, observed through a READ.
        tokio::time::sleep(past_the_transition()).await;
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED),
            "case {case:?}: expected the allocation to reach its terminal status, \
             got {body}"
        );
        assert_eq!(
            element(&body, "instanceCount").as_deref(),
            Some("0"),
            "case {case:?}: expected nothing allocated to a cancelled allocation, \
             got {body}"
        );
        // A read does not change what a later caller sees.
        let again = described(&router, case, &source).await;
        assert_eq!(
            element(&again, "status").as_deref(),
            element(&body, "status").as_deref(),
            "case {case:?}: a second read must report the same allocation status, \
             got {again}"
        );

        // The interruptible reservation itself is cancelled, in both readers.
        for (reader, body) in [
            (
                "DescribeCapacityReservations",
                described(&router, case, &interruptible).await,
            ),
            (
                "GetCapacityReservationUsage",
                reported_usage(&router, case, &interruptible).await,
            ),
        ] {
            assert_eq!(
                element(&body, "state").as_deref(),
                Some(CAPACITY_RESERVATION_STATE_CANCELLED),
                "case {case:?}: expected {reader} to report the interruptible \
                 reservation cancelled, got {body}"
            );
        }

        // The source's own state follows what was cancelled, and nothing else does.
        let expected_source_state = if cancel_interruptible {
            CAPACITY_RESERVATION_STATE_ACTIVE
        } else {
            CAPACITY_RESERVATION_STATE_CANCELLED
        };
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(expected_source_state),
            "case {case:?}: expected the source to be {expected_source_state}, got {body}"
        );
    }
}

/// One `UpdateInterruptibleCapacityReservationAllocation` call for `source_id`.
async fn update_allocation(
    router: &MultiServiceRouter,
    source_id: &str,
    target: i32,
    preference: Option<&str>,
) -> (u16, String) {
    let mut params = vec![
        param("CapacityReservationId", source_id),
        param("TargetInstanceCount", &target.to_string()),
    ];
    if let Some(preference) = preference {
        params.push(param("ZeroSizePreference", preference));
    }
    super::ec2_query::call(
        router,
        REGION,
        "UpdateInterruptibleCapacityReservationAllocation",
        &params,
    )
    .await
}

/// One row of the resize table: the capacity the source reserves, how much of it is first
/// allocated, the new target, the zero-size preference the update names, and whether the
/// allocation ends up released.
struct ResizeCase {
    case: &'static str,
    capacity: i32,
    allocated: i32,
    target: i32,
    preference: Option<&'static str>,
    released: bool,
}

/// Resizing an allocation moves capacity between the pair it ties together, and reducing it
/// to zero releases it or keeps it at zero capacity, as its zero-size preference says.
#[tokio::test]
async fn resizing_an_allocation_moves_capacity_between_the_pair() {
    let cases: [ResizeCase; 5] = [
        ResizeCase {
            case: "adding capacity to the allocation",
            capacity: 8,
            allocated: 2,
            target: 5,
            preference: None,
            released: false,
        },
        ResizeCase {
            case: "reclaiming part of it",
            capacity: 8,
            allocated: 5,
            target: 2,
            preference: None,
            released: false,
        },
        ResizeCase {
            case: "reclaiming all but one",
            capacity: 64,
            allocated: 60,
            target: 1,
            preference: None,
            released: false,
        },
        ResizeCase {
            case: "down to zero, kept at zero capacity",
            capacity: 8,
            allocated: 3,
            target: 0,
            preference: Some("retain"),
            released: false,
        },
        ResizeCase {
            case: "down to zero, released",
            capacity: 8,
            allocated: 3,
            target: 0,
            preference: Some("default"),
            released: true,
        },
    ];
    for row in cases {
        let ResizeCase {
            case,
            capacity,
            allocated,
            target,
            preference,
            released,
        } = row;
        let (router, _engine) = super::ec2_query::emulator();
        let source = create_reservation(&router, capacity).await;
        let (status, body) = allocate(&router, &source, allocated).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 allocating, got {status} with body {body}"
        );
        // The id of the reservation the allocation created, read out of the source — which
        // is where the model carries the allocation.
        let body = described(&router, case, &source).await;
        let interruptible = element(&body, "interruptibleCapacityReservationId")
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the allocation's reservation id in {body}")
            });
        tokio::time::sleep(past_the_transition()).await;

        let (status, body) = update_allocation(&router, &source, target, preference).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 updating, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "targetInstanceCount").as_deref(),
            Some(target.to_string().as_str()),
            "case {case:?}: expected the new target in the answer, got {body}"
        );

        // The capacity the allocation holds is out of the source, and the rest is the
        // source's: the pair always accounts for what was reserved.
        let expected_source_total = capacity - target;
        let body = described(&router, case, &source).await;
        assert_eq!(
            element(&body, "totalInstanceCount").as_deref(),
            Some(expected_source_total.to_string().as_str()),
            "case {case:?}: expected the source to hold the capacity the allocation does \
             not, got {body}"
        );

        // The clock owes the resized allocation its completed status.
        tokio::time::sleep(past_the_transition()).await;
        let body = described(&router, case, &source).await;
        if released {
            assert_eq!(
                element(&body, "status").as_deref(),
                Some(INTERRUPTIBLE_ALLOCATION_STATUS_CANCELED),
                "case {case:?}: expected the released allocation to reach its terminal \
                 status, got {body}"
            );
            let child = described(&router, case, &interruptible).await;
            assert_eq!(
                element(&child, "state").as_deref(),
                Some(CAPACITY_RESERVATION_STATE_CANCELLED),
                "case {case:?}: expected the interruptible reservation to be cancelled, \
                 got {child}"
            );
            continue;
        }
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE),
            "case {case:?}: expected the resized allocation to reach active, got {body}"
        );
        assert_eq!(
            element(&body, "instanceCount").as_deref(),
            Some(target.to_string().as_str()),
            "case {case:?}: expected the new target allocated, got {body}"
        );
        // The interruptible reservation reserves exactly what is allocated to it, and is
        // still the account's whatever that count is.
        let child = described(&router, case, &interruptible).await;
        assert_eq!(
            element(&child, "totalInstanceCount").as_deref(),
            Some(target.to_string().as_str()),
            "case {case:?}: expected the interruptible reservation to hold the allocated \
             capacity, got {child}"
        );
    }
}

/// An update that asks for more capacity than the source has left, and one on a reservation
/// that allocated nothing, are refused — and nothing moves.
#[tokio::test]
async fn an_update_is_held_to_the_capacity_the_source_has() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_reservation(&router, 4).await;
    let (status, body) = allocate(&router, &source, 1).await;
    assert_eq!(
        status, 200,
        "the allocation: expected HTTP 200, got {status} with body {body}"
    );

    // One past what the source has left (3 available after the allocation of 1), and far
    // past it.
    for target in [5, 1_000_000] {
        let (status, body) = update_allocation(&router, &source, target, None).await;
        assert_eq!(
            status, 400,
            "target {target}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "target {target}: expected InvalidParameterValue, got {body}"
        );
    }

    // The allocation is untouched by either refusal.
    let body = described(&router, "after the refusals", &source).await;
    assert_eq!(
        element(&body, "targetInstanceCount").as_deref(),
        Some("1"),
        "the refusals must leave the allocation alone, got {body}"
    );

    // A reservation that allocated nothing has no allocation to modify.
    let other = create_reservation(&router, 2).await;
    let (status, body) = update_allocation(&router, &other, 1, None).await;
    assert_eq!(
        status, 400,
        "a reservation with no allocation: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a reservation with no allocation: expected IncorrectState, got {body}"
    );
}

/// The create reports the status of the allocation REQUEST, in the request's own vocabulary,
/// for every status its allocation can be in — not only for the one it is in at acceptance.
#[tokio::test]
async fn the_create_reports_the_request_status_of_its_allocation() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_reservation(&router, 8).await;

    // A request whose allocation has not moved yet: still creating it.
    let token = "request-status-token";
    let params = [
        param("CapacityReservationId", &source),
        param("InstanceCount", "3"),
        param("ClientToken", token),
    ];
    let (status, body) =
        super::ec2_query::call(
            &router,
            REGION,
            "CreateInterruptibleCapacityReservationAllocation",
            &params,
        )
        .await;
    assert_eq!(
        status, 200,
        "the allocation: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(INTERRUPTIBLE_ALLOCATION_REQUEST_STATUS_CREATING),
        "a request whose allocation has not moved: expected the creating status, got {body}"
    );

    // Once the clock has moved the allocation, a replay of the same request reports the
    // status the allocation has REACHED, in the same vocabulary — upper case, not the
    // lower-case enum word a reader of the allocation reports.
    tokio::time::sleep(past_the_transition()).await;
    let (status, replay) =
        super::ec2_query::call(
            &router,
            REGION,
            "CreateInterruptibleCapacityReservationAllocation",
            &params,
        )
        .await;
    assert_eq!(
        status, 200,
        "the replay: expected HTTP 200, got {status} with body {replay}"
    );
    assert_eq!(
        element(&replay, "status").as_deref(),
        Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE.to_ascii_uppercase().as_str()),
        "a request whose allocation has completed: expected that status in the request's \
         vocabulary, got {replay}"
    );
    // The ALLOCATION's own status stays the lower-case enum word every reader reports.
    let described = described(&router, "the allocation's own status", &source).await;
    assert_eq!(
        element(&described, "status").as_deref(),
        Some(INTERRUPTIBLE_ALLOCATION_STATUS_ACTIVE),
        "the allocation's own status is the enum word, got {described}"
    );
}

/// An id the account holds no Capacity Reservation for is refused, and nothing is written for
/// it.
#[tokio::test]
async fn an_unknown_reservation_is_refused_whatever_the_id_looks_like() {
    let (router, _engine) = super::ec2_query::emulator();
    // One stored reservation, so "not found" is about THIS id rather than about an empty
    // store.
    let known = create_reservation(&router, 2).await;

    // The rows differ in the FORM of the id: the long id form EC2 issues, another resource's
    // id, a known id with a suffix, and something that is not an id at all.
    let suffixed = format!("{known}x");
    let cases: [(&str, &str); 4] = [
        (UNKNOWN_CAPACITY_RESERVATION_ID, "NotFound"),
        ("cb-00000000000000001", "Malformed"),
        (suffixed.as_str(), "Malformed"),
        ("not-an-id", "Malformed"),
    ];
    for (id, half) in cases {
        let expected = format!("InvalidCapacityReservationId.{half}");
        let (status, body) = cancel(&router, id, false).await;
        assert_eq!(
            status, 400,
            "case {id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {id:?}: expected {expected}, got {body}"
        );
    }

    // The known reservation is untouched by any of them.
    let body = described(&router, "the known reservation", &known).await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(CAPACITY_RESERVATION_STATE_ACTIVE),
        "the refusals must leave the stored reservation alone, got {body}"
    );
}

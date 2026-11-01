// SPDX-License-Identifier: CC0-1.0

//! How the interruptible-allocation pair screens the Capacity Reservation id a request
//! names: `CreateInterruptibleCapacityReservationAllocation` makes an allocation from a
//! source reservation and `UpdateInterruptibleCapacityReservationAllocation` resizes the one
//! it made, and both have to resolve that source first. Driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! so every call here carries one.
//!
//! The rules under test:
//!
//! 1. Both operations read a Capacity Reservation id by the SHAPE every EC2 resource id
//!    takes — `cr-` followed by 8 or 17 lowercase hexadecimal digits — so a value of that
//!    shape the account holds no reservation under, the all-zero placeholder every client
//!    reaches for included, is LOOKED UP and reported
//!    `InvalidCapacityReservationId.NotFound` at HTTP 400, with "The capacity-reservation ID
//!    '<id>' does not exist".
//! 2. Only a value OUTSIDE that shape is refused `InvalidCapacityReservationId.Malformed` at
//!    HTTP 400, with "Capacity Reservation ID '<id>' is malformed", before any lookup.
//! 3. A refusal of either kind writes nothing: the reservation the account does hold keeps
//!    its capacity and carries no allocation.
//! 4. An id the store DOES hold resolves through both operations, so the refusals above are
//!    about the id rather than about the operation.
//!
//! Every table row is a distinct id FORM — the long form EC2 issues, the all-zero
//! placeholder, the short form, a leading digit above zero, another resource kind's id, a
//! known id with a suffix, upper-case digits, too few digits, no digits and something that
//! is not an id at all — which is what rules 1 and 2 generalise over, so no single recorded
//! request is the specification. The id forms themselves are the two the service's own id
//! readers publish (`cr-` plus the short or the long resource-id form; see
//! `ec2::state::require_capacity_reservation_by_id_shape`).

use std::time::Duration;

use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::param;

/// The Region every call here is signed for. The store holds Capacity Reservations per
/// account rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The instance type, platform and zone every reservation in this suite reserves capacity
/// for — members of the modelled `InstanceType` and `CapacityReservationInstancePlatform`
/// catalogs.
const INSTANCE_TYPE: &str = "c5.xlarge";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// How much capacity the one reservation this suite stores reserves.
const KNOWN_CAPACITY: i32 = 4;

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

/// One `CreateInterruptibleCapacityReservationAllocation` call naming `source_id`.
async fn allocate(
    router: &MultiServiceRouter,
    source_id: &str,
    instance_count: i32,
) -> (u16, String) {
    let params = [
        param("CapacityReservationId", source_id),
        param("InstanceCount", &instance_count.to_string()),
    ];
    super::ec2_query::call(
        router,
        REGION,
        "CreateInterruptibleCapacityReservationAllocation",
        &params,
    )
    .await
}

/// One `UpdateInterruptibleCapacityReservationAllocation` call naming `source_id`.
async fn update_allocation(
    router: &MultiServiceRouter,
    source_id: &str,
    target_instance_count: i32,
) -> (u16, String) {
    let params = [
        param("CapacityReservationId", source_id),
        param("TargetInstanceCount", &target_instance_count.to_string()),
    ];
    super::ec2_query::call(
        router,
        REGION,
        "UpdateInterruptibleCapacityReservationAllocation",
        &params,
    )
    .await
}

/// The one reservation `capacity_reservation_id` names, as `DescribeCapacityReservations`
/// reports it — asserted to be readable.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> String {
    let params = indexed("CapacityReservationId", &[capacity_reservation_id]);
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeCapacityReservations", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing {capacity_reservation_id}, got \
         {status} with body {body}"
    );
    body
}

/// One row of the id table: what the row is, the id a request names, and which half of the
/// `InvalidCapacityReservationId` pair the service answers for it.
struct IdCase {
    case: &'static str,
    id: String,
    /// `true` when the id is of the `cr-` resource-id shape, so it is looked up and reported
    /// not found; `false` when it is outside that shape and refused as malformed.
    shaped: bool,
}

/// The answer the service gives for an id of this row: the code and the whole message.
fn expected_refusal(row: &IdCase) -> (String, String) {
    if row.shaped {
        (
            "InvalidCapacityReservationId.NotFound".to_string(),
            format!(
                "The capacity-reservation ID '{id}' does not exist",
                id = row.id
            ),
        )
    } else {
        (
            "InvalidCapacityReservationId.Malformed".to_string(),
            format!("Capacity Reservation ID '{id}' is malformed", id = row.id),
        )
    }
}

/// Both allocation operations classify a Capacity Reservation id they cannot resolve by its
/// SHAPE: one of the two published id forms is reported not found, anything else is refused
/// as malformed, and neither writes anything.
#[tokio::test]
async fn an_unresolvable_capacity_reservation_id_is_classified_by_its_shape() {
    let (router, _engine) = super::ec2_query::emulator();
    // One stored reservation, so "not found" is about THIS id rather than about an empty
    // store — and so a row can name a known id with something stuck on the end.
    let known = create_reservation(&router, KNOWN_CAPACITY).await;

    let rows = [
        IdCase {
            case: "the long id form EC2 issues",
            id: "cr-0ffffffffffffffff".to_string(),
            shaped: true,
        },
        IdCase {
            case: "the all-zero placeholder",
            id: "cr-00000000000000000".to_string(),
            shaped: true,
        },
        IdCase {
            case: "the short id form",
            id: "cr-12345678".to_string(),
            shaped: true,
        },
        IdCase {
            case: "a long id whose leading digit is above zero",
            id: "cr-1234567890abcdef0".to_string(),
            shaped: true,
        },
        IdCase {
            case: "another resource kind's id",
            id: "cb-00000000000000001".to_string(),
            shaped: false,
        },
        IdCase {
            case: "a known id with a suffix",
            id: format!("{known}x"),
            shaped: false,
        },
        IdCase {
            case: "upper-case hexadecimal digits",
            id: "cr-0ABCDEF0123456789".to_string(),
            shaped: false,
        },
        IdCase {
            case: "a digit outside the hexadecimal alphabet",
            id: "cr-0g23456789abcdef0".to_string(),
            shaped: false,
        },
        IdCase {
            case: "too few digits",
            id: "cr-0123456789abcde".to_string(),
            shaped: false,
        },
        IdCase {
            case: "no digits at all",
            id: "cr-".to_string(),
            shaped: false,
        },
        IdCase {
            case: "not an id at all",
            id: "not-an-id".to_string(),
            shaped: false,
        },
    ];

    for row in &rows {
        let (expected_code, expected_message) = expected_refusal(row);
        let case = row.case;
        let id = row.id.as_str();
        for (operation, (status, body)) in [
            (
                "CreateInterruptibleCapacityReservationAllocation",
                allocate(&router, id, 1).await,
            ),
            (
                "UpdateInterruptibleCapacityReservationAllocation",
                update_allocation(&router, id, 1).await,
            ),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?} ({id:?}) through {operation}: expected HTTP 400, got \
                 {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({id:?}) through {operation}: expected code \
                 {expected_code}, got {code} with body {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?} ({id:?}) through {operation}: expected message \
                 {expected_message:?}, got {message:?}"
            );
        }
    }

    // Nothing any of those refusals touched: the capacity the stored reservation reserves is
    // still available, and it carries no allocation.
    let body = described(&router, "the stored reservation", &known).await;
    assert_eq!(
        element(&body, "availableInstanceCount").as_deref(),
        Some(KNOWN_CAPACITY.to_string().as_str()),
        "the refusals must leave the stored reservation's capacity alone, got {body}"
    );
    assert_eq!(
        element(&body, "interruptibleCapacityReservationId"),
        None,
        "the refusals must write no allocation, got {body}"
    );
}

/// An id the store DOES hold resolves through both operations: the allocation is made from
/// it and then resized on it, and the resize is what the next read of the reservation
/// reports.
#[tokio::test]
async fn a_stored_capacity_reservation_id_resolves_through_both_operations() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_reservation(&router, KNOWN_CAPACITY).await;

    let allocated = 2;
    let (status, body) = allocate(&router, &source, allocated).await;
    assert_eq!(
        status, 200,
        "allocating {allocated} of {KNOWN_CAPACITY} instances: expected HTTP 200, got \
         {status} with body {body}"
    );
    // The allocation starts out with the move to its complete status owed, so the resize
    // happens once that move has fallen due.
    tokio::time::sleep(past_the_transition()).await;

    let target = 3;
    let (status, body) = update_allocation(&router, &source, target).await;
    assert_eq!(
        status, 200,
        "resizing the allocation to {target}: expected HTTP 200, got {status} with \
         body {body}"
    );
    assert_eq!(
        element(&body, "targetInstanceCount").as_deref(),
        Some(target.to_string().as_str()),
        "resizing to {target}: expected that target in the answer, got {body}"
    );

    // Read back through `DescribeCapacityReservations`, which is where the model carries the
    // allocation: the resize is on the stored reservation and not only in the answer.
    let body = described(&router, "the resized allocation", &source).await;
    assert_eq!(
        element(&body, "targetInstanceCount").as_deref(),
        Some(target.to_string().as_str()),
        "expected the stored allocation to carry the new target {target}, got {body}"
    );
    assert_eq!(
        element(&body, "availableInstanceCount").as_deref(),
        Some((KNOWN_CAPACITY - target).to_string().as_str()),
        "expected the source to have {} instances left after the resize, got {body}",
        KNOWN_CAPACITY - target
    );
}

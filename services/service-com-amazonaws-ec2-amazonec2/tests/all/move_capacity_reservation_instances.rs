// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `MoveCapacityReservationInstances`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=MoveCapacityReservationInstances`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A move SHIFTS CAPACITY AND PERSISTS IT: `InstanceCount` instances leave the source
//!    and arrive at the destination, so each side's `TotalInstanceCount` and
//!    `AvailableInstanceCount` change by that count, the capacity the two hold between them
//!    is conserved, and `DescribeCapacityReservations` reads both sides back as the move
//!    left them.
//! 2. A Capacity Reservation id this service cannot resolve is refused BEFORE the move, with
//!    `InvalidCapacityReservationId.Malformed` — this operation's validator answers that half
//!    both for a value outside the long id form EC2 issues and for a well-formed id the
//!    account holds no reservation under — on either side of the
//!    move.
//! 3. "The source Capacity Reservation and the destination Capacity Reservation must be
//!    `active`, owned by your Amazon Web Services account, and share the following:
//!    Instance type, Platform, Availability Zone, Tenancy, Placement group, Capacity
//!    Reservation end time - At specific time or Manually"
//!    (`MoveCapacityReservationInstances`): a side in any other state is refused with
//!    `IncorrectState`, and a destination that reserves another configuration with
//!    `InvalidParameterValue`.
//! 4. "Move AVAILABLE capacity": the count is at least one and at most what the source has
//!    available, and a `DryRun` request moves nothing.
//! 5. `ClientToken` makes the request idempotent: a replay reports the same move and moves
//!    the capacity only once, while the same token asking for a DIFFERENT move is answered
//!    `IdempotentParameterMismatch`.
//!
//! Every table row is a distinct input along what those rules generalise over — how much
//! capacity each side starts with, how much of it moves, the form of the id, which member
//! differs between the two sides, which state a side sits in — so no single request is the
//! specification.

use std::time::{Duration, SystemTime};

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    iso8601_utc, CapacityReservationSpec, CAPACITY_RESERVATION_STATE_ACTIVE,
    CAPACITY_RESERVATION_STATE_ASSESSING, CAPACITY_RESERVATION_STATE_PAYMENT_PENDING,
    CAPACITY_RESERVATION_STATE_PENDING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, indexed};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. A Capacity Reservation reserves capacity in
/// one Availability Zone, so the Region is what decides which zones a create may name.
const REGION: &str = "us-east-1";

/// The configuration both sides of a move reserve capacity for unless a row says
/// otherwise. The type is a member of the modelled `InstanceType` catalog and the platform
/// a member of `CapacityReservationInstancePlatform`
/// (`smithy_query(query="shape",shape="CapacityReservationInstancePlatform")`).
const INSTANCE_TYPE: &str = "c5.xlarge";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";
const AVAILABILITY_ZONE: &str = "us-east-1a";

/// That zone's id, for the rows seeded straight into the store.
const AVAILABILITY_ZONE_ID: &str = "use1-az1";

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for: `cr-`
/// followed by the long id form EC2 issues, which is what makes the service look it up at
/// all rather than refuse it as malformed before the lookup.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// Form fields from `(name, value)` pairs.
fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One `MoveCapacityReservationInstances` call carrying `params`, as its HTTP status and
/// XML body.
async fn move_capacity(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "MoveCapacityReservationInstances",
        params,
    )
    .await
}

/// The form fields of one move: the three members the operation requires.
fn move_params(source_id: &str, destination_id: &str, instance_count: i32) -> Vec<(String, String)> {
    vec![
        (
            "SourceCapacityReservationId".to_string(),
            source_id.to_string(),
        ),
        (
            "DestinationCapacityReservationId".to_string(),
            destination_id.to_string(),
        ),
        ("InstanceCount".to_string(), instance_count.to_string()),
    ]
}

/// One `CreateCapacityReservation` call, as the id it minted.
async fn create_reservation(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateCapacityReservation", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: creating a reservation expected HTTP 200, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a reservation id in {body}"))
}

/// The form fields that reserve `instance_count` instances of the shared configuration for
/// immediate use.
fn reservation_params(instance_count: i32) -> Vec<(String, String)> {
    let mut params = fields(&[
        ("InstanceType", INSTANCE_TYPE),
        ("InstancePlatform", INSTANCE_PLATFORM),
        ("AvailabilityZone", AVAILABILITY_ZONE),
    ]);
    params.push(("InstanceCount".to_string(), instance_count.to_string()));
    params
}

/// An `active` Capacity Reservation of `instance_count` instances of the shared
/// configuration, created OVER THE WIRE.
async fn create_side(router: &MultiServiceRouter, case: &str, instance_count: i32) -> String {
    create_reservation(router, case, &reservation_params(instance_count)).await
}

/// What one side of a move reserves, as a reader reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Counts {
    /// "The total number of instances for which the Capacity Reservation reserves capacity"
    /// (`CapacityReservation$TotalInstanceCount`).
    total: i32,
    /// "The remaining capacity" (`CapacityReservation$AvailableInstanceCount`).
    available: i32,
    /// The reservation's `CapacityReservationState`.
    state: String,
}

/// An element's integer child `name`, or a panic naming the case.
fn count_of(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> i32 {
    let text = child_text(node, name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{name}> on the reservation"));
    text.parse()
        .unwrap_or_else(|error| panic!("case {case:?}: <{name}> is not a count ({error}): {text}"))
}

/// The counts the move response reports for `side` — `sourceCapacityReservation` or
/// `destinationCapacityReservation` — beside the reservation id it names, so a row can pin
/// WHICH reservation each half of the answer is about.
fn reported_side(case: &str, body: &str, side: &str) -> (String, Counts) {
    let parsed = document(case, body);
    let node = child(parsed.root_element(), side)
        .unwrap_or_else(|| panic!("case {case:?}: expected a <{side}> in {body}"));
    let id = child_text(node, "capacityReservationId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <capacityReservationId> in {body}"));
    (
        id,
        Counts {
            total: count_of(case, node, "totalInstanceCount"),
            available: count_of(case, node, "availableInstanceCount"),
            state: child_text(node, "state").unwrap_or_default(),
        },
    )
}

/// The counts `DescribeCapacityReservations` reads back for `capacity_reservation_id` — the
/// round trip through an implemented reader, which is what makes a move PERSISTED rather
/// than only reported.
async fn described_counts(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> Counts {
    let params = indexed("CapacityReservationId", &[capacity_reservation_id]);
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeCapacityReservations", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {capacity_reservation_id} back expected HTTP 200, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "capacityReservationId").as_deref()
                    == Some(capacity_reservation_id)
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {capacity_reservation_id} in {body}")
        });
    Counts {
        total: count_of(case, node, "totalInstanceCount"),
        available: count_of(case, node, "availableInstanceCount"),
        state: child_text(node, "state").unwrap_or_default(),
    }
}

/// One Capacity Reservation written straight into the store, for the states and the clocks
/// no implemented operation puts a reservation in: nothing cancels a reservation yet
/// (`CancelCapacityReservation` is still a scaffold) and a create refuses an end date in
/// the past, so a row that needs one seeds it here. Every other row goes over the wire.
fn seed_side(
    engine: &engine::InMemoryEngine,
    state: &'static str,
    instance_count: i32,
    end_date: Option<SystemTime>,
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_capacity_reservation(CapacityReservationSpec {
            instance_type: INSTANCE_TYPE.to_string(),
            instance_platform: INSTANCE_PLATFORM.to_string(),
            availability_zone: AVAILABILITY_ZONE.to_string(),
            availability_zone_id: AVAILABILITY_ZONE_ID.to_string(),
            region: REGION.to_string(),
            tenancy: "default".to_string(),
            instance_count,
            ebs_optimized: false,
            ephemeral_storage: false,
            end_date,
            instance_match_criteria: "open".to_string(),
            created_at: SystemTime::now(),
            // Capacity for immediate use: no requested delivery date, nothing committed to
            // and no delivery method.
            start_date: None,
            commitment: None,
            delivery_preference: None,
            state,
            tags: Vec::new(),
            interruption_info: None,
            zero_size_preference: None,
            pending: None,
        })
        .expect("the store accepts a seeded Capacity Reservation")
}

// ---------------------------------------------------------------------------
// RULE 1: the capacity moves, and both sides read back as the move left them
// ---------------------------------------------------------------------------

/// One row of the success table: what it covers, what each side starts with, and how many
/// instances move.
type MoveRow = (&'static str, i32, i32, i32);

#[tokio::test]
async fn a_move_shifts_capacity_and_conserves_it() {
    // The rows differ along how much capacity each side holds and how much of it moves:
    // a single instance, part of the source, all of it, and into a destination larger than
    // the source.
    let rows: [MoveRow; 5] = [
        ("one instance of several", 4, 2, 1),
        ("part of the source", 8, 1, 5),
        ("all of the source's capacity", 3, 3, 3),
        ("into a destination larger than the source", 2, 10, 2),
        ("the whole of a single-instance source", 1, 1, 1),
    ];
    for (case, source_count, destination_count, moved) in rows {
        let (router, _engine) = super::ec2_query::emulator();
        let source = create_side(&router, case, source_count).await;
        let destination = create_side(&router, case, destination_count).await;

        let (status, body) =
            move_capacity(&router, &move_params(&source, &destination, moved)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            super::ec2_query::member(&body, "instanceCount").as_deref(),
            Some(moved.to_string().as_str()),
            "case {case:?}: expected the answer to report {moved} instances moved, got {body}"
        );

        // What the answer says each side now holds.
        let (reported_source, source_counts) =
            reported_side(case, &body, "sourceCapacityReservation");
        let (reported_destination, destination_counts) =
            reported_side(case, &body, "destinationCapacityReservation");
        assert_eq!(
            reported_source, source,
            "case {case:?}: expected the source half to name {source}, got {body}"
        );
        assert_eq!(
            reported_destination, destination,
            "case {case:?}: expected the destination half to name {destination}, got {body}"
        );

        let expected_source = Counts {
            total: source_count - moved,
            available: source_count - moved,
            state: CAPACITY_RESERVATION_STATE_ACTIVE.to_string(),
        };
        let expected_destination = Counts {
            total: destination_count + moved,
            available: destination_count + moved,
            state: CAPACITY_RESERVATION_STATE_ACTIVE.to_string(),
        };
        assert_eq!(
            source_counts, expected_source,
            "case {case:?}: expected the source at {expected_source:?}, \
             got {source_counts:?} in {body}"
        );
        assert_eq!(
            destination_counts, expected_destination,
            "case {case:?}: expected the destination at {expected_destination:?}, \
             got {destination_counts:?} in {body}"
        );

        // The capacity the two hold between them is the capacity they started with.
        assert_eq!(
            source_counts.total + destination_counts.total,
            source_count + destination_count,
            "case {case:?}: a move must conserve the capacity of the pair, got \
             {source_counts:?} and {destination_counts:?}"
        );

        // The round trip: an implemented reader reports what the move wrote.
        let read_source = described_counts(&router, case, &source).await;
        let read_destination = described_counts(&router, case, &destination).await;
        assert_eq!(
            read_source, expected_source,
            "case {case:?}: expected DescribeCapacityReservations to report the source at \
             {expected_source:?}, got {read_source:?}"
        );
        assert_eq!(
            read_destination, expected_destination,
            "case {case:?}: expected DescribeCapacityReservations to report the \
             destination at {expected_destination:?}, got {read_destination:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the ids a move cannot resolve
// ---------------------------------------------------------------------------

/// One row of the id table: what it covers, the value sent, and the half of
/// `InvalidCapacityReservationId` it must be answered with.
type IdRow = (&'static str, String, &'static str);

#[tokio::test]
async fn an_id_the_service_cannot_resolve_is_refused_on_either_side() {
    let (router, _engine) = super::ec2_query::emulator();
    let known = create_side(&router, "the known side", 4).await;

    // The rows differ in the FORM of the id: the long form EC2 issues (which is looked up
    // and not found), the null id, the short form, a leading digit above zero, another
    // resource's prefix, a known id with a suffix, and something that is not an id at all.
    // Every one of them is the malformed half for this operation: an id it cannot resolve to
    // a reservation of the account is not one EC2 issued this account.
    let rows: [IdRow; 7] = [
        (
            "a well-formed id nothing was reserved under",
            UNKNOWN_CAPACITY_RESERVATION_ID.to_string(),
            "Malformed",
        ),
        ("the null id", "cr-00000000000000000".to_string(), "Malformed"),
        ("the short id form", "cr-12345678".to_string(), "Malformed"),
        (
            "a leading digit above zero",
            "cr-1234567890abcdef0".to_string(),
            "Malformed",
        ),
        (
            "another resource's prefix",
            "cb-00000000000000001".to_string(),
            "Malformed",
        ),
        ("a known id with a suffix", format!("{known}x"), "Malformed"),
        ("not an id at all", "not-an-id".to_string(), "Malformed"),
    ];
    for (case, id, half) in rows {
        let expected = format!("InvalidCapacityReservationId.{half}");
        // The same value on each side of the move: whichever side names it, the id is the
        // reason the move is refused.
        for (side, params) in [
            ("source", move_params(&id, &known, 1)),
            ("destination", move_params(&known, &id, 1)),
        ] {
            let (status, body) = move_capacity(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?} as the {side}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?} as the {side}: expected {expected}, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(&id),
                "case {case:?} as the {side}: expected the message to name the id, \
                 got {message:?}"
            );
        }
    }

    // Nothing moved: the one reservation the account holds is as it was created.
    let read = described_counts(&router, "the known side", &known).await;
    assert_eq!(
        read,
        Counts {
            total: 4,
            available: 4,
            state: CAPACITY_RESERVATION_STATE_ACTIVE.to_string(),
        },
        "a refused move must leave the reservation it named alone, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: both sides active, and sharing their configuration
// ---------------------------------------------------------------------------

/// `now` shifted back by `seconds`, as the wire carries a timestamp.
fn timestamp_in(seconds: u64) -> String {
    iso8601_utc(SystemTime::now() + Duration::from_secs(seconds))
}

/// One row of the mismatch table: what it covers, the fields the DESTINATION is created
/// with, and the error code the move must be answered with.
type MismatchRow = (&'static str, Vec<(String, String)>, &'static str);

#[tokio::test]
async fn a_destination_reserving_another_configuration_is_refused() {
    // Each row changes exactly ONE member of the list the operation documents as shared,
    // leaving the rest as the source's.
    let rows: [MismatchRow; 5] = [
        (
            "another instance type",
            fields(&[
                ("InstanceType", "m5.large"),
                ("InstancePlatform", INSTANCE_PLATFORM),
                ("AvailabilityZone", AVAILABILITY_ZONE),
                ("InstanceCount", "2"),
            ]),
            "InvalidParameterValue",
        ),
        (
            "another platform",
            fields(&[
                ("InstanceType", INSTANCE_TYPE),
                ("InstancePlatform", "Red Hat Enterprise Linux"),
                ("AvailabilityZone", AVAILABILITY_ZONE),
                ("InstanceCount", "2"),
            ]),
            "InvalidParameterValue",
        ),
        (
            "another Availability Zone",
            fields(&[
                ("InstanceType", INSTANCE_TYPE),
                ("InstancePlatform", INSTANCE_PLATFORM),
                ("AvailabilityZone", "us-east-1b"),
                ("InstanceCount", "2"),
            ]),
            "InvalidParameterValue",
        ),
        (
            "another tenancy",
            fields(&[
                ("InstanceType", INSTANCE_TYPE),
                ("InstancePlatform", INSTANCE_PLATFORM),
                ("AvailabilityZone", AVAILABILITY_ZONE),
                ("InstanceCount", "2"),
                ("Tenancy", "dedicated"),
            ]),
            "InvalidParameterValue",
        ),
        (
            "an end time where the source has none",
            {
                let mut params = fields(&[
                    ("InstanceType", INSTANCE_TYPE),
                    ("InstancePlatform", INSTANCE_PLATFORM),
                    ("AvailabilityZone", AVAILABILITY_ZONE),
                    ("InstanceCount", "2"),
                    ("EndDateType", "limited"),
                ]);
                params.push(("EndDate".to_string(), timestamp_in(3 * 60 * 60)));
                params
            },
            "InvalidParameterValue",
        ),
    ];
    for (case, destination_params, expected) in rows {
        let (router, _engine) = super::ec2_query::emulator();
        let source = create_side(&router, case, 4).await;
        let destination = create_reservation(&router, case, &destination_params).await;

        let (status, body) = move_capacity(&router, &move_params(&source, &destination, 1)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let read = described_counts(&router, case, &source).await;
        assert_eq!(
            read.available, 4,
            "case {case:?}: a refused move must leave the source's capacity alone, \
             got {read:?}"
        );
    }
}

/// One row of the state table: what it covers, the state each side is seeded in, whether
/// the side carries an end date in the past, and which side is the one at fault.
type StateRow = (&'static str, &'static str, bool, bool);

#[tokio::test]
async fn a_side_that_is_not_active_is_refused_with_its_state() {
    // The rows differ along the state the offending side sits in — three a writer stored
    // and one the CLOCK derives from an end date that has passed — and along which side of
    // the move it is.
    let rows: [StateRow; 8] = [
        ("a pending source", CAPACITY_RESERVATION_STATE_PENDING, false, true),
        (
            "a pending destination",
            CAPACITY_RESERVATION_STATE_PENDING,
            false,
            false,
        ),
        (
            "an assessing source",
            CAPACITY_RESERVATION_STATE_ASSESSING,
            false,
            true,
        ),
        (
            "an assessing destination",
            CAPACITY_RESERVATION_STATE_ASSESSING,
            false,
            false,
        ),
        (
            "a source awaiting payment",
            CAPACITY_RESERVATION_STATE_PAYMENT_PENDING,
            false,
            true,
        ),
        (
            "a destination awaiting payment",
            CAPACITY_RESERVATION_STATE_PAYMENT_PENDING,
            false,
            false,
        ),
        (
            "a source whose end date has passed",
            CAPACITY_RESERVATION_STATE_ACTIVE,
            true,
            true,
        ),
        (
            "a destination whose end date has passed",
            CAPACITY_RESERVATION_STATE_ACTIVE,
            true,
            false,
        ),
    ];
    for (case, state, expired, at_fault_is_source) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let healthy = create_side(&router, case, 4).await;
        let end_date = expired.then(|| SystemTime::now() - Duration::from_secs(60 * 60));
        let at_fault = seed_side(&engine, state, 4, end_date);
        let (source, destination) = if at_fault_is_source {
            (at_fault.clone(), healthy.clone())
        } else {
            (healthy.clone(), at_fault.clone())
        };

        let (status, body) = move_capacity(&router, &move_params(&source, &destination, 1)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&at_fault),
            "case {case:?}: expected the message to name {at_fault}, got {message:?}"
        );
        let read = described_counts(&router, case, &healthy).await;
        assert_eq!(
            read.available, 4,
            "case {case:?}: a refused move must leave the other side alone, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_move_into_the_source_itself_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_side(&router, "one reservation", 4).await;

    let (status, body) = move_capacity(&router, &move_params(&source, &source, 1)).await;
    assert_eq!(
        status, 400,
        "a move into the source itself: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a move into the source itself: expected InvalidParameterValue, got {body}"
    );
    let read = described_counts(&router, "one reservation", &source).await;
    assert_eq!(
        read,
        Counts {
            total: 4,
            available: 4,
            state: CAPACITY_RESERVATION_STATE_ACTIVE.to_string(),
        },
        "a move into the source itself must change nothing, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: the count, and the dry run
// ---------------------------------------------------------------------------

/// One row of the count table: what it covers, what the source holds, how much the request
/// asks to move, and the HTTP status and code it must be answered with.
type CountRow = (&'static str, i32, i32, u16, &'static str);

#[tokio::test]
async fn the_count_is_bounded_by_what_the_source_has_available() {
    // The rows walk the bound: one below the floor, the floor itself, the ceiling (what the
    // source has available) and one and then far past it.
    let rows: [CountRow; 6] = [
        ("a negative count", 4, -1, 400, "InvalidParameterValue"),
        ("no instances at all", 4, 0, 400, "InvalidParameterValue"),
        ("the floor of one instance", 4, 1, 200, ""),
        ("exactly what the source has", 4, 4, 200, ""),
        ("one more than the source has", 4, 5, 400, "InvalidParameterValue"),
        (
            "far more than the source has",
            4,
            1_000,
            400,
            "InvalidParameterValue",
        ),
    ];
    for (case, source_count, requested, expected_status, expected_code) in rows {
        let (router, _engine) = super::ec2_query::emulator();
        let source = create_side(&router, case, source_count).await;
        let destination = create_side(&router, case, 2).await;

        let (status, body) =
            move_capacity(&router, &move_params(&source, &destination, requested)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let read = described_counts(&router, case, &source).await;
        if expected_status == 200 {
            assert_eq!(
                read.available,
                source_count - requested,
                "case {case:?}: expected the source to keep {} instances, got {read:?}",
                source_count - requested
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
            assert_eq!(
                read.available, source_count,
                "case {case:?}: a refused move must leave the source's capacity alone, \
                 got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_dry_run_moves_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_side(&router, "a dry run", 5).await;
    let destination = create_side(&router, "a dry run", 1).await;

    // A dry run of a request that WOULD have succeeded is answered `DryRunOperation`.
    let mut params = move_params(&source, &destination, 2);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = move_capacity(&router, &params).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    for (side, id, expected) in [("source", &source, 5), ("destination", &destination, 1)] {
        let read = described_counts(&router, "a dry run", id).await;
        assert_eq!(
            read.available, expected,
            "a dry run must leave the {side}'s capacity alone, got {read:?}"
        );
    }

    // A dry run of a request that would have been REFUSED is refused with its own reason.
    let mut params = move_params(&source, &destination, 99);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = move_capacity(&router, &params).await;
    assert_eq!(
        status, 400,
        "a dry run that would have been refused: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a dry run that would have been refused: expected the refusal's own code, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: the client token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_client_token_moves_the_capacity_only_once() {
    let (router, _engine) = super::ec2_query::emulator();
    let source = create_side(&router, "an idempotent move", 6).await;
    let destination = create_side(&router, "an idempotent move", 1).await;

    let token = "move-token-1";
    let mut params = move_params(&source, &destination, 2);
    params.push(("ClientToken".to_string(), token.to_string()));

    let (status, first) = move_capacity(&router, &params).await;
    assert_eq!(
        status, 200,
        "the first move: expected HTTP 200, got {status} with body {first}"
    );

    // The rows differ along what the REPEAT carries: the same request again (a replay), and
    // then three that change one member each under a token already in use.
    let (status, replay) = move_capacity(&router, &params).await;
    assert_eq!(
        status, 200,
        "a replay: expected HTTP 200, got {status} with body {replay}"
    );
    let (_, first_source) = reported_side("the first move", &first, "sourceCapacityReservation");
    let (_, replay_source) = reported_side("a replay", &replay, "sourceCapacityReservation");
    assert_eq!(
        replay_source, first_source,
        "a replay must report the move the token already made, got {replay_source:?} \
         where the first answered {first_source:?}"
    );
    for (side, id, expected) in [("source", &source, 4), ("destination", &destination, 3)] {
        let read = described_counts(&router, "a replay", id).await;
        assert_eq!(
            read.available, expected,
            "a replay must move the capacity only once: expected the {side} at {expected}, \
             got {read:?}"
        );
    }

    // A DIFFERENT move under the same token is not a replay.
    let other_destination = create_side(&router, "another destination", 1).await;
    let mismatches: [(&str, Vec<(String, String)>); 3] = [
        ("another count", move_params(&source, &destination, 3)),
        (
            "another destination",
            move_params(&source, &other_destination, 2),
        ),
        (
            "the sides exchanged",
            move_params(&destination, &source, 2),
        ),
    ];
    for (case, mut params) in mismatches {
        params.push(("ClientToken".to_string(), token.to_string()));
        let (status, body) = move_capacity(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IdempotentParameterMismatch",
            "case {case:?}: expected IdempotentParameterMismatch, got {body}"
        );
    }

    // A fresh token moves capacity again.
    let mut params = move_params(&source, &destination, 1);
    params.push(("ClientToken".to_string(), "move-token-2".to_string()));
    let (status, body) = move_capacity(&router, &params).await;
    assert_eq!(
        status, 200,
        "a fresh token: expected HTTP 200, got {status} with body {body}"
    );
    let read = described_counts(&router, "a fresh token", &source).await;
    assert_eq!(
        read.available, 3,
        "a fresh token must move capacity again, got {read:?}"
    );
}

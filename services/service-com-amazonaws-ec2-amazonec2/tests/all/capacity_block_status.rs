// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeCapacityBlockStatus`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeCapacityBlockStatus`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Two rules are under test:
//!
//! 1. A block's capacity IS THE CAPACITY OF THE CAPACITY RESERVATIONS IT HOLDS IT AS: each
//!    reservation contributes the capacity it reserves, split into the part that can still
//!    be launched into and the part that cannot, and the block's three totals are the sums
//!    of its reservations'. Capacity of a reservation that is not serving capacity at the
//!    moment of the call is not available.
//! 2. The INTERCONNECT STATUS is read off the same state: there is something to measure only
//!    while the block's reserved window is open and the block holds its capacity as
//!    reservations at all, so a block the clock does not put inside that window, and one
//!    naming no reservation, report `insufficient-data`; an open block whose every named
//!    reservation serves capacity is `ok`, and one missing some of it — a reservation not
//!    serving capacity, or one the account no longer holds — is `impaired`.
//!
//! Every table row is a distinct input along what those rules generalise over — where the
//! clock stands in the window, how many reservations a block holds its capacity as, how much
//! capacity each one reserves and how much of it is still launchable, which state each
//! reservation is in, and which page size and filter the request names — so no single
//! recorded block is the specification. The set of reported values covers every member of
//! `CapacityBlockInterconnectStatus`
//! (`smithy_query(query="shape",shape="CapacityBlockInterconnectStatus")`).
//!
//! Every response is read with the XML parser rather than by substring, so a block's own
//! `totalCapacity` is never confused with one of its reservations'.

use std::time::{Duration, SystemTime};

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    iso8601_utc, CapacityBlockSpec, CapacityReservationSpec, CAPACITY_RESERVATION_STATE_ACTIVE,
    CAPACITY_RESERVATION_STATE_PAYMENT_PENDING, CAPACITY_RESERVATION_STATE_PENDING,
    CAPACITY_RESERVATION_STATE_SCHEDULED,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, filters, indexed};
use super::wire::{child_text, document, set_items};

/// The Region every call here is signed for. The store holds Capacity Blocks per account
/// rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The Availability Zone every seeded block and reservation sits in, and its id.
const AVAILABILITY_ZONE: &str = "us-east-1a";
const AVAILABILITY_ZONE_ID: &str = "use1-az1";

/// One form field.
fn field(name: &str, value: impl ToString) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One `DescribeCapacityBlockStatus` call carrying `params`, as its HTTP status and XML body.
async fn describe_status(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeCapacityBlockStatus", params).await
}

/// `base` shifted by `hours`, which may be negative.
fn hours_from(base: SystemTime, hours: i64) -> SystemTime {
    let span = Duration::from_secs(hours.unsigned_abs() * 3_600);
    if hours < 0 {
        base - span
    } else {
        base + span
    }
}

// ---------------------------------------------------------------------------
// Reading the answer
// ---------------------------------------------------------------------------

/// The availability of one Capacity Reservation, as a response reported it.
#[derive(Debug, PartialEq, Eq)]
struct ReportedReservation {
    capacity_reservation_id: String,
    total: i64,
    available: i64,
    unavailable: i64,
}

/// The availability of one Capacity Block, as a response reported it.
#[derive(Debug, PartialEq, Eq)]
struct ReportedBlock {
    capacity_block_id: String,
    interconnect_status: String,
    total: i64,
    available: i64,
    unavailable: i64,
    reservations: Vec<ReportedReservation>,
}

/// The number `node`'s `name` member carries, or a failure naming the case: a total the
/// service did not report is not the same thing as a zero.
fn count_of(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> i64 {
    let text = child_text(node, name).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a <{name}> on the <{}> the service reported",
            node.tag_name().name()
        )
    });
    text.parse::<i64>()
        .unwrap_or_else(|error| panic!("case {case:?}: expected a number in <{name}> ({error})"))
}

/// Every `CapacityBlockStatus` a successful response carries, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<ReportedBlock> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "capacityBlockStatusSet")
        .into_iter()
        .map(|item| ReportedBlock {
            capacity_block_id: child_text(item, "capacityBlockId").unwrap_or_default(),
            interconnect_status: child_text(item, "interconnectStatus").unwrap_or_default(),
            total: count_of(case, item, "totalCapacity"),
            available: count_of(case, item, "totalAvailableCapacity"),
            unavailable: count_of(case, item, "totalUnavailableCapacity"),
            reservations: set_items(item, "capacityReservationStatusSet")
                .into_iter()
                .map(|row| ReportedReservation {
                    capacity_reservation_id: child_text(row, "capacityReservationId")
                        .unwrap_or_default(),
                    total: count_of(case, row, "totalCapacity"),
                    available: count_of(case, row, "totalAvailableCapacity"),
                    unavailable: count_of(case, row, "totalUnavailableCapacity"),
                })
                .collect(),
        })
        .collect()
}

/// The statuses of a successful call, or a failure naming the case.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<ReportedBlock> {
    let (status, body) = describe_status(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported(case, &body)
}

/// The Capacity Block ids a successful call reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    described(router, case, params)
        .await
        .into_iter()
        .map(|block| block.capacity_block_id)
        .collect()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One Capacity Reservation a seeded block holds its capacity as: what is written on the
/// record, and what the status of that reservation must therefore be.
struct ReservationRow {
    /// The `CapacityReservationState` the record is stored in.
    state: &'static str,
    /// An end date, in hours from the moment the suite seeded the store, or `None` for a
    /// reservation with no end date. One in the past is what makes a stored `active`
    /// reservation `expired` by the clock.
    end_hours: Option<i64>,
    /// "The total number of instances for which the Capacity Reservation reserves capacity."
    instance_count: i32,
    /// How much of that capacity is still launchable, as written on the record.
    available_instance_count: i32,
    /// The capacity this reservation must be reported as still offering.
    expected_available: i64,
}

/// One row of the status table: a Capacity Block, the reservations it holds its capacity as,
/// and the availability the two rules say it must be reported with.
struct StatusRow {
    case: &'static str,
    /// The reserved window, in hours from the moment the suite seeded the store.
    start_hours: i64,
    end_hours: i64,
    /// A state no clock drives, from `CapacityBlockResourceState`.
    pinned_state: Option<&'static str>,
    reservations: Vec<ReservationRow>,
    /// Capacity Reservation ids the block names that the account holds no reservation for.
    unknown_reservations: Vec<&'static str>,
    expected_interconnect: &'static str,
}

/// The blocks every assertion below is made over.
///
/// The rows differ along everything the two rules generalise over: where the clock stands in
/// the reserved window (before it, inside it, past it) and whether a state is pinned instead;
/// how many reservations a block holds its capacity as (none, one, two, three); how much
/// capacity each reserves and how much of it is still launchable (all, some, none); and which
/// state each reservation is in (serving, waiting on a payment, past its end date, named but
/// gone).
fn status_rows() -> Vec<StatusRow> {
    vec![
        StatusRow {
            case: "open window, one reservation, nothing launched",
            start_hours: -1,
            end_hours: 1,
            pinned_state: None,
            reservations: vec![ReservationRow {
                state: CAPACITY_RESERVATION_STATE_ACTIVE,
                end_hours: None,
                instance_count: 16,
                available_instance_count: 16,
                expected_available: 16,
            }],
            unknown_reservations: Vec::new(),
            expected_interconnect: "ok",
        },
        StatusRow {
            case: "open window, two reservations, one partly launched into",
            start_hours: -12,
            end_hours: 12,
            pinned_state: None,
            reservations: vec![
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: Some(24),
                    instance_count: 8,
                    available_instance_count: 3,
                    expected_available: 3,
                },
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: None,
                    instance_count: 64,
                    available_instance_count: 64,
                    expected_available: 64,
                },
            ],
            unknown_reservations: Vec::new(),
            expected_interconnect: "ok",
        },
        StatusRow {
            case: "open window, every instance launched into its reservation",
            start_hours: -2,
            end_hours: 336,
            pinned_state: None,
            reservations: vec![ReservationRow {
                state: CAPACITY_RESERVATION_STATE_ACTIVE,
                end_hours: None,
                instance_count: 4,
                available_instance_count: 0,
                expected_available: 0,
            }],
            unknown_reservations: Vec::new(),
            expected_interconnect: "ok",
        },
        StatusRow {
            case: "open window, one reservation still awaiting its payment",
            start_hours: -1,
            end_hours: 47,
            pinned_state: None,
            reservations: vec![
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: None,
                    instance_count: 2,
                    available_instance_count: 2,
                    expected_available: 2,
                },
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_PAYMENT_PENDING,
                    end_hours: None,
                    instance_count: 6,
                    available_instance_count: 6,
                    // Nothing can be launched into capacity that has not been paid for.
                    expected_available: 0,
                },
            ],
            unknown_reservations: Vec::new(),
            expected_interconnect: "impaired",
        },
        StatusRow {
            case: "open window, one reservation past its end date",
            start_hours: -3,
            end_hours: 3,
            pinned_state: None,
            reservations: vec![ReservationRow {
                state: CAPACITY_RESERVATION_STATE_ACTIVE,
                end_hours: Some(-1),
                instance_count: 5,
                available_instance_count: 5,
                // The reservation expired when the clock passed its end date.
                expected_available: 0,
            }],
            unknown_reservations: Vec::new(),
            expected_interconnect: "impaired",
        },
        StatusRow {
            case: "open window, a reservation the account no longer holds",
            start_hours: -4,
            end_hours: 4,
            pinned_state: None,
            reservations: Vec::new(),
            unknown_reservations: vec!["cr-0123456789abcdef0"],
            expected_interconnect: "impaired",
        },
        StatusRow {
            case: "window not open yet",
            start_hours: 24,
            end_hours: 48,
            pinned_state: None,
            reservations: vec![ReservationRow {
                // The capacity of a block whose window has not opened is held for the date
                // that window opens on, so it is not being served yet.
                state: CAPACITY_RESERVATION_STATE_SCHEDULED,
                end_hours: None,
                instance_count: 32,
                available_instance_count: 32,
                expected_available: 0,
            }],
            unknown_reservations: Vec::new(),
            expected_interconnect: "insufficient-data",
        },
        StatusRow {
            case: "window closed",
            start_hours: -48,
            end_hours: -24,
            pinned_state: None,
            reservations: vec![
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: Some(-24),
                    instance_count: 1,
                    available_instance_count: 1,
                    expected_available: 0,
                },
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: Some(-24),
                    instance_count: 2,
                    available_instance_count: 2,
                    expected_available: 0,
                },
                ReservationRow {
                    state: CAPACITY_RESERVATION_STATE_ACTIVE,
                    end_hours: Some(-24),
                    instance_count: 3,
                    available_instance_count: 3,
                    expected_available: 0,
                },
            ],
            unknown_reservations: Vec::new(),
            expected_interconnect: "insufficient-data",
        },
        StatusRow {
            case: "a cancelled block, whose window says nothing",
            start_hours: -1,
            end_hours: 1,
            pinned_state: Some("cancelled"),
            reservations: vec![ReservationRow {
                // Capacity whose provisioning never completed: nothing can be launched into
                // it.
                state: CAPACITY_RESERVATION_STATE_PENDING,
                end_hours: None,
                instance_count: 9,
                available_instance_count: 9,
                expected_available: 0,
            }],
            unknown_reservations: Vec::new(),
            expected_interconnect: "insufficient-data",
        },
        StatusRow {
            case: "open window, holding capacity as no reservation at all",
            start_hours: -1,
            end_hours: 1,
            pinned_state: None,
            reservations: Vec::new(),
            unknown_reservations: Vec::new(),
            expected_interconnect: "insufficient-data",
        },
    ]
}

/// What one seeded row became: the ids it was given, and the availability it must be reported
/// with.
struct Seeded {
    rows: Vec<StatusRow>,
    /// The Capacity Block id of each row, in row order.
    ids: Vec<String>,
    /// The expected status of each row, in row order.
    expected: Vec<ReportedBlock>,
}

/// Write the rows into the engine's store — the reservations first, then the block that holds
/// its capacity as them — and work out what each one must be reported as.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let seeded_at = SystemTime::now();
    let rows = status_rows();
    let store = engine.state::<Ec2State>();
    let mut ids = Vec::new();
    let mut expected = Vec::new();
    for row in &rows {
        let mut guard = store.lock();
        let mut reservation_ids = Vec::new();
        let mut reservations = Vec::new();
        for reservation in &row.reservations {
            let capacity_reservation_id = guard
                .insert_capacity_reservation(CapacityReservationSpec {
                    instance_type: "p5.48xlarge".to_string(),
                    instance_platform: "Linux/UNIX".to_string(),
                    availability_zone: AVAILABILITY_ZONE.to_string(),
                    availability_zone_id: AVAILABILITY_ZONE_ID.to_string(),
                    region: REGION.to_string(),
                    tenancy: "default".to_string(),
                    instance_count: reservation.instance_count,
                    ebs_optimized: false,
                    ephemeral_storage: false,
                    end_date: reservation
                        .end_hours
                        .map(|hours| hours_from(seeded_at, hours)),
                    instance_match_criteria: "open".to_string(),
                    created_at: hours_from(seeded_at, -72),
                    start_date: None,
                    commitment: None,
                    delivery_preference: None,
                    state: reservation.state,
                    tags: Vec::new(),
                    interruption_info: None,
                    zero_size_preference: None,
                    pending: None,
                })
                .expect("the store accepts a seeded Capacity Reservation");
            // Nothing in this emulator launches an instance INTO a reservation, so a row
            // whose capacity is partly in use is written here rather than launched for.
            if let Some(record) = guard
                .capacity_reservations
                .get_mut(&capacity_reservation_id)
            {
                record.view.available_instance_count =
                    Some(reservation.available_instance_count);
            }
            let total = i64::from(reservation.instance_count);
            reservations.push(ReportedReservation {
                capacity_reservation_id: capacity_reservation_id.clone(),
                total,
                available: reservation.expected_available,
                unavailable: total - reservation.expected_available,
            });
            reservation_ids.push(capacity_reservation_id);
        }
        // The ids the block names, the ones the account holds last, so a row that names a
        // reservation the store does not hold is not merely the final entry of the list.
        let mut named: Vec<String> = row
            .unknown_reservations
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        named.extend(reservation_ids);
        let capacity_block_id = guard
            .insert_capacity_block(CapacityBlockSpec {
                ultraserver_type: Some("u-p6e-gb200x72".to_string()),
                availability_zone: AVAILABILITY_ZONE.to_string(),
                availability_zone_id: AVAILABILITY_ZONE_ID.to_string(),
                capacity_reservation_ids: named,
                start_date: hours_from(seeded_at, row.start_hours),
                end_date: hours_from(seeded_at, row.end_hours),
                create_date: hours_from(seeded_at, -72),
                pinned_state: row.pinned_state,
                // These blocks are seeded rather than bought, so no upfront payment is
                // pending on any of them.
                payment_settles_at: None,
                tags: Vec::new(),
            })
            .expect("the store has room for a seeded Capacity Block");
        expected.push(ReportedBlock {
            capacity_block_id: capacity_block_id.clone(),
            interconnect_status: row.expected_interconnect.to_string(),
            total: reservations.iter().map(|row| row.total).sum(),
            available: reservations.iter().map(|row| row.available).sum(),
            unavailable: reservations.iter().map(|row| row.unavailable).sum(),
            reservations,
        });
        ids.push(capacity_block_id);
    }
    Seeded { rows, ids, expected }
}

/// The `CapacityBlockId.N` list naming the rows at `indices`.
fn id_params(seeded: &Seeded, indices: &[usize]) -> Vec<(String, String)> {
    let named: Vec<&str> = indices
        .iter()
        .map(|index| seeded.ids[*index].as_str())
        .collect();
    indexed("CapacityBlockId", &named)
}

// ---------------------------------------------------------------------------
// The status is computed from the store
// ---------------------------------------------------------------------------

/// Nothing in the EC2 API gives a fresh account a Capacity Block, so a fresh account has no
/// Capacity Block status — and asking is a READ: two calls answer the same and the store is
/// untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_statuses_and_describing_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let first = described_ids(&router, "fresh account", &[]).await;
    assert!(
        first.is_empty(),
        "fresh account: expected no Capacity Block statuses, got {first:?}"
    );
    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert!(
        guard.capacity_blocks.is_empty() && guard.capacity_reservations.is_empty(),
        "describing Capacity Block status must not write to the store"
    );
}

/// Every row is reported with the capacity its Capacity Reservations hold and the
/// interconnect status its window and those reservations put it in — and reading it again
/// answers exactly the same, so the report is a read.
#[tokio::test]
async fn the_status_of_a_block_is_the_capacity_of_the_reservations_it_holds_it_as() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (index, row) in seeded.rows.iter().enumerate() {
        let case = row.case;
        let params = id_params(&seeded, &[index]);
        let reported = described(&router, case, &params).await;
        let expected = &seeded.expected[index];
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one status, got {reported:?}"
        );
        assert_eq!(
            &reported[0], expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported[0]
        );
        // The three totals are two parts and their sum, whatever the row.
        assert_eq!(
            reported[0].available + reported[0].unavailable,
            reported[0].total,
            "case {case:?}: available plus unavailable must be the total, got {:?}",
            reported[0]
        );

        let again = described(&router, case, &params).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must answer the same, got {again:?}"
        );
    }

    // Asked for the whole account, the answer is every row in store order.
    let all = described(&router, "every block", &[]).await;
    assert_eq!(
        all, seeded.expected,
        "every block: expected every stored block's status in store order, got {all:?}"
    );

    // Between them the rows report every member of `CapacityBlockInterconnectStatus`.
    for status in ["ok", "impaired", "insufficient-data"] {
        assert!(
            all.iter()
                .any(|block| block.interconnect_status == status),
            "the rows must cover the interconnect status {status:?}, they report {:?}",
            all.iter()
                .map(|block| block.interconnect_status.clone())
                .collect::<Vec<_>>()
        );
    }
}

/// The answer is computed from the store, not fixed: capacity launched into a reservation
/// after a call moves from available to unavailable in the next one, and a block written
/// after a call shows up in it.
#[tokio::test]
async fn a_later_write_changes_what_the_next_call_reports() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Row 0 holds its capacity as one serving reservation with nothing launched into it.
    let before = described(&router, "before the launch", &id_params(&seeded, &[0])).await;
    let reservation_id = before[0].reservations[0].capacity_reservation_id.clone();
    let total = before[0].total;
    assert_eq!(
        before[0].available, total,
        "before the launch: expected all of the capacity available, got {:?}",
        before[0]
    );

    let taken = 6i64;
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let record = guard
            .capacity_reservations
            .get_mut(&reservation_id)
            .expect("the seeded reservation is in the store");
        record.view.available_instance_count = Some(
            i32::try_from(total - taken).expect("the seeded capacity fits an i32"),
        );
    }

    let after = described(&router, "after the launch", &id_params(&seeded, &[0])).await;
    assert_eq!(
        (after[0].total, after[0].available, after[0].unavailable),
        (total, total - taken, taken),
        "after the launch: expected {taken} of {total} reported unavailable, got {:?}",
        after[0]
    );

    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_capacity_block(CapacityBlockSpec {
            ultraserver_type: None,
            availability_zone: AVAILABILITY_ZONE.to_string(),
            availability_zone_id: AVAILABILITY_ZONE_ID.to_string(),
            capacity_reservation_ids: Vec::new(),
            start_date: hours_from(SystemTime::now(), -1),
            end_date: hours_from(SystemTime::now(), 1),
            create_date: SystemTime::now(),
            pinned_state: None,
            payment_settles_at: None,
            tags: Vec::new(),
        })
        .expect("the store has room for a later Capacity Block");
    let ids = described_ids(&router, "after a later write", &[]).await;
    assert!(
        ids.contains(&late_id),
        "after a later write: expected {late_id:?} to be reported too, got {ids:?}"
    );
}

/// A Capacity Block bought through `PurchaseCapacityBlock` reports the capacity the purchase
/// bought, read back through this operation: the round trip from a write to the status of
/// what was written.
#[tokio::test]
async fn a_purchased_block_reports_the_capacity_it_bought() {
    let (router, _engine) = super::ec2_query::emulator();

    // (case, the search's instance count): a purchase of one instance and one of several.
    for instance_count in [1i32, 8] {
        let case = format!("a purchase of {instance_count}");
        let search = vec![
            field("InstanceType", "p5.48xlarge"),
            field("InstanceCount", instance_count),
            field("CapacityDurationHours", 24),
            // A window at least an hour out, so the block is not racing the moment its own
            // window opens.
            field(
                "StartDateRange",
                iso8601_utc(SystemTime::now() + Duration::from_secs(2 * 60 * 60)),
            ),
            field("MaxResults", 1),
        ];
        let (status, body) =
            super::ec2_query::call(&router, REGION, "DescribeCapacityBlockOfferings", &search)
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the search for an offering must succeed, got {status} with {body}"
        );
        let offering_id = super::ec2_query::element(&body, "capacityBlockOfferingId")
            .unwrap_or_else(|| panic!("case {case:?}: expected an offering in {body}"));

        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "PurchaseCapacityBlock",
            &[
                field("CapacityBlockOfferingId", &offering_id),
                field("InstancePlatform", "Linux/UNIX"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the purchase must succeed, got {status} with {body}"
        );
        let block_id = super::ec2_query::element(&body, "capacityBlockId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a block in {body}"));

        let reported = described(
            &router,
            &case,
            &indexed("CapacityBlockId", &[block_id.as_str()]),
        )
        .await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the purchased block's status, got {reported:?}"
        );
        let block = &reported[0];
        assert_eq!(
            block.capacity_block_id, block_id,
            "case {case:?}: expected the purchased block {block_id:?}, got {block:?}"
        );
        // The purchase's capacity is held as one Capacity Reservation of the instance count
        // the offering sold, none of it launchable while the payment is still pending and the
        // block's window has not opened.
        assert_eq!(
            (
                block.total,
                block.available,
                block.unavailable,
                block.interconnect_status.as_str(),
                block.reservations.len()
            ),
            (
                i64::from(instance_count),
                0,
                i64::from(instance_count),
                "insufficient-data",
                1
            ),
            "case {case:?}: expected the bought capacity, unavailable until the payment \
             settles and the window opens, got {block:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CapacityBlockIds
// ---------------------------------------------------------------------------

/// `CapacityBlockIds` narrows the answer to the blocks it names, in store order whatever
/// order they were named in; an id the account holds no block for is refused rather than
/// reported as an absent row.
#[tokio::test]
async fn capacity_block_ids_narrow_the_answer_and_an_unknown_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let last = seeded.ids.len() - 1;

    // Rows: one id, two, the whole store, and the same two named in reverse.
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("one id", vec![3]),
        ("two ids", vec![1, last]),
        ("every id", (0..seeded.ids.len()).collect()),
        ("two ids, reversed", vec![last, 1]),
    ];
    for (case, indices) in cases {
        let reported = described_ids(&router, case, &id_params(&seeded, &indices)).await;
        let mut expected: Vec<String> = indices
            .iter()
            .map(|index| seeded.ids[*index].clone())
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the named blocks in store order, got {reported:?}"
        );
    }

    // An id of the right form that names no block, and one of no form at all.
    for unknown in ["cb-0123456789abcdef0", "not-an-id"] {
        let params = indexed("CapacityBlockId", &[unknown]);
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidCapacityBlockId.NotFound",
            "case {unknown:?}: expected InvalidCapacityBlockId.NotFound, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// The documented filter selects on the interconnect status the caller is shown, values
/// within one filter are OR'd, and the comparison is exact, case-sensitive and understands
/// EC2's `*` wildcard.
#[tokio::test]
async fn the_interconnect_status_filter_selects_the_blocks_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The ids of the rows whose status is `status`, in store order.
    let with = |status: &str| -> Vec<String> {
        seeded
            .expected
            .iter()
            .filter(|block| block.interconnect_status == status)
            .map(|block| block.capacity_block_id.clone())
            .collect()
    };

    // One row per member of `CapacityBlockInterconnectStatus`, one naming two of them, one
    // using the wildcard, and one naming a value no block carries.
    let cases: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("ok", vec!["ok"], with("ok")),
        ("impaired", vec!["impaired"], with("impaired")),
        (
            "insufficient-data",
            vec!["insufficient-data"],
            with("insufficient-data"),
        ),
        ("two values", vec!["ok", "impaired"], {
            let mut both = with("ok");
            both.extend(with("impaired"));
            both.sort();
            both
        }),
        ("a wildcard", vec!["i*"], {
            let mut both = with("impaired");
            both.extend(with("insufficient-data"));
            both.sort();
            both
        }),
        ("a value no block carries", vec!["degraded"], Vec::new()),
        // "Filter names are case-sensitive", and so are the values EC2 compares.
        ("the wrong case", vec!["OK"], Vec::new()),
    ];
    for (case, values, expected) in cases {
        let params = filters(&[("interconnect-status", &values)]);
        let reported = described_ids(&router, case, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused, rather than read as selecting
/// nothing.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a name differing only in case, a plural misspelling, a filter another describe
    // of this resource documents, a tag filter this operation does not, and an empty name.
    for name in [
        "Interconnect-Status",
        "interconnect-statuses",
        "capacity-block-id",
        "tag:Name",
        "",
    ] {
        let params = filters(&[(name, &["ok"])]);
        let (status, body) = describe_status(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports every block exactly once in store order; a request with no `MaxResults` is
/// answered in one page.
#[tokio::test]
async fn max_results_pages_the_store_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeCapacityBlockStatusMaxResults` is modelled `range 1..1000`, so the rows are
    // its lower bound, sizes that do and do not divide the store, the store's own size and
    // the published upper bound.
    for page_size in [1, 3, 4, seeded.ids.len(), 1000] {
        let case = format!("MaxResults={page_size}");
        let mut params = vec![field("MaxResults", page_size)];
        let mut walked: Vec<String> = Vec::new();
        loop {
            let (status, body) = describe_status(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} statuses, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|block| block.capacity_block_id));
            match super::ec2_query::member(&body, "nextToken") {
                Some(token) if !token.is_empty() => {
                    params = vec![field("MaxResults", page_size), field("NextToken", &token)];
                }
                _ => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: the walk must report every block exactly once in store order, \
             got {walked:?}"
        );
    }

    let unpaged = described_ids(&router, "no MaxResults", &[]).await;
    assert_eq!(
        unpaged, seeded.ids,
        "no MaxResults: expected every block in one answer, got {unpaged:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 1..1000`) and one past each.
    for (max_results, expected_status) in [(0, 400), (1, 200), (1000, 200), (1001, 400)] {
        let (status, body) = describe_status(&router, &[field("MaxResults", max_results)]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded.
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
    ];
    for token in cases {
        let (status, body) = describe_status(&router, &[field("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call. A request that would have been refused is
/// refused with THAT reason instead.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe_status(&router, &[field("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );

    let reported = described_ids(&router, "DryRun=false", &[field("DryRun", "false")]).await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected an ordinary answer, got {reported:?}"
    );

    // An unknown id beside the flag is refused with the id's reason, not with the dry run's.
    let mut params = indexed("CapacityBlockId", &["cb-0123456789abcdef0"]);
    params.push(field("DryRun", "true"));
    let (status, body) = describe_status(&router, &params).await;
    assert_eq!(
        status, 400,
        "DryRun with an unknown id: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityBlockId.NotFound",
        "DryRun with an unknown id: expected the id's refusal, got {body}"
    );
}

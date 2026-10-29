// SPDX-License-Identifier: CC0-1.0

//! What `ModifyCapacityReservationFleet` changes about a stored Capacity Reservation Fleet,
//! what a later `DescribeCapacityReservationFleets` then reports of it, and what the
//! modification refuses — driven over the wire the service actually speaks, `ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a modification is APPLIED TO THE STORE: a new total target capacity is re-reserved
//!    through the Fleet's Capacity Reservations (the specification the allocation strategy
//!    reserves on keeps the reservation it was created with, and its instance count covers
//!    the new target at its own weight), an end date is set or moved, `RemoveEndDate` takes
//!    it away, and everything the request did not name is left as stored;
//! 2. the call leaves the Fleet in the non-terminal `modifying` state and the CLOCK, not a
//!    caller, moves it on to `active`, changing the state word and nothing else;
//! 3. a request that cannot be served is refused with EC2's own error code and changes
//!    nothing: an id that is not a Fleet id at all before the store is read, a well-formed id
//!    that names no Fleet, two members that cannot hold together, a request that asks for no
//!    change, a capacity or an end date outside what the member admits, and a Fleet in a
//!    state a modification does not run from;
//! 4. `DryRun` performs nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which member the
//! request changes, whether the capacity rises or falls, how many capacity units an instance
//! provides, which specification the priorities make the Fleet reserve on, whether the Fleet
//! already had an end date, which malformed id form, which part of the request cannot be
//! served — so no row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::ops::types::CAPACITY_RESERVATION_INSTANCE_PLATFORM_VALUES;
use ec2::state::{
    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE, CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
    CAPACITY_RESERVATION_FLEET_STATE_MODIFYING, TRANSITION_DELAY,
};

use super::ec2_query::{call, element, elements, error_code, error_message, indexed, member};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// A well-formed Capacity Reservation Fleet id no Fleet of the account is stored under: the
/// mint counts from one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "crf-0dddddddddddddddd";

/// An instant well past any test run, for an end date a modification sets.
const LATER_END_DATE: &str = "2031-06-07T08:09:10Z";

/// A second such instant, for an end date a modification MOVES to.
const LATEST_END_DATE: &str = "2033-11-12T13:14:15Z";

/// An instant behind any test run: an end date names when the Fleet expires, so one already
/// past is not one a Fleet can be given.
const PAST_END_DATE: &str = "2001-02-03T04:05:06Z";

/// How long a clock-owed transition takes to fall due: the service's own span, plus a margin
/// so a slow machine cannot read the state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(400)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One Capacity Reservation of a Fleet, as a response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Reservation {
    capacity_reservation_id: Option<String>,
    instance_type: String,
    total_instance_count: i64,
    fulfilled_capacity: f64,
    weight: f64,
    create_date: Option<String>,
}

/// One Capacity Reservation Fleet, as a response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Fleet {
    id: String,
    state: String,
    total_target_capacity: i64,
    total_fulfilled_capacity: f64,
    create_time: String,
    end_date: Option<String>,
    reservations: Vec<Reservation>,
}

/// The text of a required child element, or a panic naming the case.
fn required(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child_text(node, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in <{}>", node.tag_name().name()))
}

/// A numeric child element as an `f64`.
fn number(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> f64 {
    let text = required(case, node, name);
    text.parse()
        .unwrap_or_else(|error| panic!("case {case:?}: <{name}> must be a number: {error}"))
}

/// One Fleet read off a `DescribeCapacityReservationFleets` entry.
fn fleet_from(case: &str, node: roxmltree::Node<'_, '_>) -> Fleet {
    let reservations = set_items(node, "instanceTypeSpecificationSet")
        .into_iter()
        .map(|item| Reservation {
            capacity_reservation_id: child_text(item, "capacityReservationId"),
            instance_type: required(case, item, "instanceType"),
            total_instance_count: number(case, item, "totalInstanceCount") as i64,
            fulfilled_capacity: number(case, item, "fulfilledCapacity"),
            weight: number(case, item, "weight"),
            create_date: child_text(item, "createDate"),
        })
        .collect();
    Fleet {
        id: required(case, node, "capacityReservationFleetId"),
        state: required(case, node, "state"),
        total_target_capacity: number(case, node, "totalTargetCapacity") as i64,
        total_fulfilled_capacity: number(case, node, "totalFulfilledCapacity"),
        create_time: required(case, node, "createTime"),
        end_date: child_text(node, "endDate"),
        reservations,
    }
}

/// The one Fleet a `DescribeCapacityReservationFleets` narrowed to `fleet_id` reports.
async fn described(case: &str, router: &MultiServiceRouter, fleet_id: &str) -> Fleet {
    let body = ok(
        case,
        router,
        "DescribeCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[fleet_id]),
    )
    .await;
    let doc = document(case, &body);
    let items = set_items(doc.root_element(), "capacityReservationFleetSet");
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: a describe narrowed to one id reports one Fleet, got body {body}"
    );
    fleet_from(case, items[0])
}

/// One `instanceTypeSpecification` of a create request, at `index` (counting from 1).
fn specification(index: usize, fields: &[(&str, String)]) -> Vec<Param> {
    fields
        .iter()
        .map(|(name, value)| {
            (
                format!("InstanceTypeSpecification.{index}.{name}"),
                value.clone(),
            )
        })
        .collect()
}

/// The first Availability Zone of [`REGION`], read off `DescribeAvailabilityZones` rather
/// than written down here.
async fn first_zone(router: &MultiServiceRouter) -> String {
    let body = ok("zones", router, "DescribeAvailabilityZones", &[]).await;
    let names = elements(&body, "zoneName");
    assert!(
        !names.is_empty(),
        "the Region must report Availability Zones, got {body}"
    );
    names[0].clone()
}

/// How many instances it takes to cover `target` capacity units at `weight` units each: the
/// quotient rounded up, since an instance is not reserved in fractions.
fn instance_count(target: i64, weight: f64) -> i64 {
    (target as f64 / weight).ceil() as i64
}

/// The Fleet one row of the table starts from: the instance types the Fleet may reserve on,
/// what one instance of each provides, the priority ordering them, its target capacity and
/// its end date.
struct Start {
    /// The weight of each specification; its length is the number of specifications.
    weights: &'static [f64],
    /// The priority of each specification, in the same order as `weights`.
    priorities: &'static [Option<i32>],
    total_target_capacity: i64,
    end_date: Option<&'static str>,
}

/// The instance types the rows draw from, by position: read off the modelled `InstanceType`
/// catalog (`crate::ops::types::INSTANCE_TYPE_VALUES`) rather than invented.
const INSTANCE_TYPES: [&str; 3] = ["m5.large", "c5.xlarge", "r5.2xlarge"];

impl Start {
    /// This Fleet as `CreateCapacityReservationFleet` form fields.
    fn params(&self, zone: &str) -> Vec<Param> {
        let mut params = vec![param(
            "TotalTargetCapacity",
            &self.total_target_capacity.to_string(),
        )];
        for (index, weight) in self.weights.iter().enumerate() {
            let mut fields: Vec<(&str, String)> = vec![
                ("InstanceType", INSTANCE_TYPES[index].to_string()),
                (
                    "InstancePlatform",
                    CAPACITY_RESERVATION_INSTANCE_PLATFORM_VALUES[0].to_string(),
                ),
                ("Weight", weight.to_string()),
                ("AvailabilityZone", zone.to_string()),
            ];
            if let Some(priority) = self.priorities[index] {
                fields.push(("Priority", priority.to_string()));
            }
            params.extend(specification(index + 1, &fields));
        }
        if let Some(end_date) = self.end_date {
            params.push(param("EndDate", end_date));
        }
        params
    }

    /// The specification this `prioritized` Fleet reserves on: the lowest `Priority` value,
    /// one that assigned none coming last, the earliest winning a tie.
    fn chosen(&self) -> usize {
        let mut best = 0;
        for index in 1..self.weights.len() {
            if self.priorities[index].unwrap_or(i32::MAX)
                < self.priorities[best].unwrap_or(i32::MAX)
            {
                best = index;
            }
        }
        best
    }
}

/// One row of the modification table: a Fleet, and the modification applied to it.
struct ModifyCase {
    case: &'static str,
    start: Start,
    /// `TotalTargetCapacity`, when the request names one.
    new_capacity: Option<i64>,
    /// `EndDate`, when the request names one.
    new_end_date: Option<&'static str>,
    /// `RemoveEndDate=true`, when the request names it.
    remove_end_date: bool,
}

impl ModifyCase {
    /// This row as `ModifyCapacityReservationFleet` form fields for `fleet_id`.
    fn params(&self, fleet_id: &str) -> Vec<Param> {
        let mut params = vec![param("CapacityReservationFleetId", fleet_id)];
        if let Some(capacity) = self.new_capacity {
            params.push(param("TotalTargetCapacity", &capacity.to_string()));
        }
        if let Some(end_date) = self.new_end_date {
            params.push(param("EndDate", end_date));
        }
        if self.remove_end_date {
            params.push(param("RemoveEndDate", "true"));
        }
        params
    }

    /// The target capacity the Fleet holds after this row's modification.
    fn expected_capacity(&self) -> i64 {
        self.new_capacity.unwrap_or(self.start.total_target_capacity)
    }

    /// The end date the Fleet holds after this row's modification, as the instant it names
    /// without its zone designator — a response spells the same instant with sub-second
    /// digits.
    fn expected_end_date(&self) -> Option<&'static str> {
        if self.remove_end_date {
            return None;
        }
        self.new_end_date.or(self.start.end_date)
    }
}

/// The rows of the modification table.
///
/// No row repeats another's combination of which member changes, whether the capacity rises
/// or falls, how many capacity units an instance provides, which specification the priorities
/// choose, or whether the Fleet already had an end date.
const MODIFY_CASES: &[ModifyCase] = &[
    ModifyCase {
        case: "the capacity raised, one specification at unit weight",
        start: Start {
            weights: &[1.0],
            priorities: &[None],
            total_target_capacity: 3,
            end_date: None,
        },
        new_capacity: Some(10),
        new_end_date: None,
        remove_end_date: false,
    },
    ModifyCase {
        case: "the capacity lowered, at a fractional weight",
        start: Start {
            weights: &[2.5],
            priorities: &[Some(0)],
            total_target_capacity: 10,
            end_date: None,
        },
        new_capacity: Some(5),
        new_end_date: None,
        remove_end_date: false,
    },
    ModifyCase {
        case: "the capacity raised on the second of three specifications, which holds \
               the priority",
        start: Start {
            weights: &[1.0, 4.0, 2.0],
            priorities: &[Some(5), Some(1), Some(9)],
            total_target_capacity: 7,
            end_date: None,
        },
        new_capacity: Some(20),
        new_end_date: None,
        remove_end_date: false,
    },
    ModifyCase {
        case: "the capacity down to a single unit, a weight above it covering it",
        start: Start {
            weights: &[99.0],
            priorities: &[Some(2)],
            total_target_capacity: 300,
            end_date: None,
        },
        new_capacity: Some(1),
        new_end_date: None,
        remove_end_date: false,
    },
    ModifyCase {
        case: "an end date set on a Fleet that had none",
        start: Start {
            weights: &[1.0],
            priorities: &[None],
            total_target_capacity: 2,
            end_date: None,
        },
        new_capacity: None,
        new_end_date: Some(LATER_END_DATE),
        remove_end_date: false,
    },
    ModifyCase {
        case: "an end date moved on a Fleet that had one",
        start: Start {
            weights: &[2.0],
            priorities: &[Some(1)],
            total_target_capacity: 6,
            end_date: Some(LATER_END_DATE),
        },
        new_capacity: None,
        new_end_date: Some(LATEST_END_DATE),
        remove_end_date: false,
    },
    ModifyCase {
        case: "the end date removed",
        start: Start {
            weights: &[1.0, 3.0],
            priorities: &[None, Some(4)],
            total_target_capacity: 5,
            end_date: Some(LATEST_END_DATE),
        },
        new_capacity: None,
        new_end_date: None,
        remove_end_date: true,
    },
    ModifyCase {
        case: "the capacity and the end date in one call",
        start: Start {
            weights: &[2.0, 1.0],
            priorities: &[Some(3), Some(8)],
            total_target_capacity: 4,
            end_date: None,
        },
        new_capacity: Some(9),
        new_end_date: Some(LATER_END_DATE),
        remove_end_date: false,
    },
];

/// Create the Fleet a row starts from, answering it as a describe reports it.
async fn create(case: &str, router: &MultiServiceRouter, start: &Start, zone: &str) -> Fleet {
    let body = ok(
        case,
        router,
        "CreateCapacityReservationFleet",
        &start.params(zone),
    )
    .await;
    let fleet_id = element(&body, "capacityReservationFleetId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a Fleet id: {body}"));
    described(case, router, &fleet_id).await
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the modification is applied, and the clock re-activates the Fleet
// ---------------------------------------------------------------------------

/// RULES 1 and 2: every member the modification names is applied to the STORED Fleet — the
/// capacity re-reserved through the Capacity Reservation the strategy chose, which keeps the
/// id and the create date it was reserved under — everything else is left as stored, and the
/// CLOCK moves the Fleet out of `modifying` into `active`.
#[tokio::test]
async fn a_modification_is_applied_to_the_store_and_the_clock_reactivates_the_fleet() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    for row in MODIFY_CASES {
        let case = row.case;
        let created = create(case, &router, &row.start, &zone).await;
        past_the_transition().await;

        let body = ok(
            case,
            &router,
            "ModifyCapacityReservationFleet",
            &row.params(&created.id),
        )
        .await;
        assert_eq!(
            member(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: a modification that succeeds returns true, got body {body}"
        );

        let modified = described(case, &router, &created.id).await;
        let chosen = row.start.chosen();
        let expected_capacity = row.expected_capacity();

        // RULE 2: the call leaves the Fleet in the transient state the clock owes `active`.
        assert_eq!(
            modified.state, CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
            "case {case:?}: a modified Fleet is being modified, got {:?}",
            modified.state
        );
        // RULE 1: what the request named is applied.
        assert_eq!(
            modified.total_target_capacity, expected_capacity,
            "case {case:?}: expected {expected_capacity} target capacity units, got {}",
            modified.total_target_capacity
        );
        match row.expected_end_date() {
            None => assert!(
                modified.end_date.is_none(),
                "case {case:?}: a Fleet with no end date reports none, got {:?}",
                modified.end_date
            ),
            Some(end_date) => {
                let instant = end_date.trim_end_matches('Z');
                let reported = modified.end_date.clone().unwrap_or_default();
                assert!(
                    reported.starts_with(instant),
                    "case {case:?}: expected the end date {instant:?}, got {reported:?}"
                );
            }
        }
        // The capacity is re-reserved through the Fleet's Capacity Reservations: the chosen
        // specification covers the whole target at its own weight, every other reserves
        // nothing, and the Fleet's total is its reservations'.
        assert_eq!(
            modified.reservations.len(),
            row.start.weights.len(),
            "case {case:?}: every instance type specification is still reported, got {:?}",
            modified.reservations
        );
        let mut expected_total = 0.0;
        for (index, reservation) in modified.reservations.iter().enumerate() {
            let weight = row.start.weights[index];
            let expected_count = if index == chosen {
                instance_count(expected_capacity, weight)
            } else {
                0
            };
            let expected_here = expected_count as f64 * weight;
            expected_total += expected_here;
            assert_eq!(
                (
                    reservation.instance_type.as_str(),
                    reservation.total_instance_count,
                ),
                (INSTANCE_TYPES[index], expected_count),
                "case {case:?}: specification {index} must reserve {expected_count} \
                 instances of its own type, got {reservation:?}"
            );
            assert!(
                (reservation.fulfilled_capacity - expected_here).abs() < 1e-9
                    && (reservation.weight - weight).abs() < 1e-9,
                "case {case:?}: specification {index} must fulfil {expected_here} capacity \
                 units at weight {weight}, got {reservation:?}"
            );
            // The Capacity Reservation the create reserved is MODIFIED, not replaced: it
            // keeps the id and the create date a caller already holds.
            assert_eq!(
                (
                    reservation.capacity_reservation_id.clone(),
                    reservation.create_date.clone(),
                ),
                (
                    created.reservations[index].capacity_reservation_id.clone(),
                    created.reservations[index].create_date.clone(),
                ),
                "case {case:?}: specification {index} must keep the Capacity Reservation it \
                 was created with, got {reservation:?}"
            );
            assert_eq!(
                reservation.capacity_reservation_id.is_some(),
                index == chosen,
                "case {case:?}: specification {index} holds a Capacity Reservation only \
                 where capacity is reserved on it, got {reservation:?}"
            );
        }
        assert!(
            (modified.total_fulfilled_capacity - expected_total).abs() < 1e-9
                && modified.total_fulfilled_capacity >= expected_capacity as f64,
            "case {case:?}: the Fleet must fulfil its reservations' {expected_total} capacity \
             units, at least the {expected_capacity} it targets, got {}",
            modified.total_fulfilled_capacity
        );
        // Nothing the request did not name is touched.
        assert_eq!(
            (modified.id.as_str(), modified.create_time.as_str()),
            (created.id.as_str(), created.create_time.as_str()),
            "case {case:?}: a modification leaves the Fleet's identity and create time alone"
        );

        // RULE 2: the CLOCK, not a caller, moves the Fleet back to `active` — a state word,
        // the capacity having been reserved by the call.
        past_the_transition().await;
        let active = described(case, &router, &created.id).await;
        assert_eq!(
            active.state, CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
            "case {case:?}: the clock owes a modified Fleet active, got {:?}",
            active.state
        );
        assert_eq!(
            Fleet {
                state: modified.state.clone(),
                ..active.clone()
            },
            modified,
            "case {case:?}: activation moves the state word and nothing else, got {active:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: refusals, and that a refused request changes nothing
// ---------------------------------------------------------------------------

/// The Fleet the refusal rows are tried against: one specification, unit weight, no end date.
const REFUSAL_START: Start = Start {
    weights: &[1.0],
    priorities: &[None],
    total_target_capacity: 4,
    end_date: None,
};

/// Every way a string can fail to be an id EC2 could have ISSUED, so a caller can tell a typo
/// from a Fleet the account does not hold.
///
/// The second row is the id the recorded unit behind this operation's stub issue probes; the
/// rest span the other ways an id can fail the form.
const MALFORMED_FLEET_IDS: [(&str, &str); 7] = [
    ("a leading digit above zero", "crf-fffffffffffffffff"),
    ("every digit zero", "crf-00000000000000000"),
    ("the retired short form", "crf-0abcdef1"),
    ("a non-hexadecimal digit", "crf-0zzzzzzzzzzzzzzzz"),
    ("eighteen digits", "crf-0123456789abcdef01"),
    ("another resource's prefix", "cr-0123456789abcdef0"),
    ("no prefix at all", "0123456789abcdef0"),
];

/// RULE 3: an id that is not a Capacity Reservation Fleet id at all is refused BEFORE the
/// store is read, with the one sentence this service words an unreadable id with, and a
/// well-formed id that names no Fleet is refused as a Fleet the account does not hold.
#[tokio::test]
async fn an_id_that_names_no_fleet_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;
    let fleet = create("a Fleet to leave alone", &router, &REFUSAL_START, &zone).await;

    for (case, malformed) in MALFORMED_FLEET_IDS {
        let (status, body) = api(
            &router,
            "ModifyCapacityReservationFleet",
            &[
                param("CapacityReservationFleetId", malformed),
                param("TotalTargetCapacity", "8"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {malformed:?}, got {status} with body {body}"
        );
        assert_eq!(
            (error_code(&body), error_message(&body)),
            (
                "InvalidCapacityReservationFleetId.Malformed".to_string(),
                format!("Capacity Reservation Fleet ID '{malformed}' is malformed"),
            ),
            "case {case:?}: expected the malformed-id refusal, got body {body}"
        );
    }

    // A well-formed id the account holds no Fleet under is a Fleet that does not exist.
    let case = "a well-formed id naming no Fleet";
    let (status, body) = api(
        &router,
        "ModifyCapacityReservationFleet",
        &[
            param("CapacityReservationFleetId", UNKNOWN_FLEET_ID),
            param("TotalTargetCapacity", "8"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityReservationFleetId.NotFound",
        "case {case:?}: expected the not-found code, got body {body}"
    );
    assert!(
        error_message(&body).contains(UNKNOWN_FLEET_ID),
        "case {case:?}: the message must name the id, got body {body}"
    );

    // None of those requests touched the Fleet the account does hold.
    let untouched = described("nothing was modified", &router, &fleet.id).await;
    assert_eq!(
        (
            untouched.total_target_capacity,
            untouched.end_date.clone(),
            untouched.reservations.clone(),
        ),
        (
            fleet.total_target_capacity,
            fleet.end_date.clone(),
            fleet.reservations.clone(),
        ),
        "a refused request modifies nothing, got {untouched:?} against {fleet:?}"
    );
}

/// RULE 3: a modification that cannot be served is refused with EC2's own code and leaves the
/// Fleet as stored.
///
/// The rows differ in WHICH part of the request cannot be served — two members that cannot
/// hold together, a request that asks for no change at all, a capacity below what the member
/// admits, an end date that is already past — not in the value of one of them.
#[tokio::test]
async fn a_modification_that_cannot_be_served_is_refused_and_changes_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;
    let fleet = create("a Fleet to leave alone", &router, &REFUSAL_START, &zone).await;

    let rows: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "both an end date and its removal",
            vec![
                param("EndDate", LATER_END_DATE),
                param("RemoveEndDate", "true"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "an end date beside a removal that removes nothing",
            vec![
                param("EndDate", LATER_END_DATE),
                param("RemoveEndDate", "false"),
            ],
            "InvalidParameterCombination",
        ),
        ("no change at all", Vec::new(), "MissingParameter"),
        (
            "a removal that removes nothing, and nothing else",
            vec![param("RemoveEndDate", "false")],
            "MissingParameter",
        ),
        (
            "no capacity units at all",
            vec![param("TotalTargetCapacity", "0")],
            "InvalidParameterValue",
        ),
        (
            "a negative capacity",
            vec![param("TotalTargetCapacity", "-3")],
            "InvalidParameterValue",
        ),
        (
            "an end date already past",
            vec![param("EndDate", PAST_END_DATE)],
            "InvalidParameterValue",
        ),
    ];

    for (case, extra, expected_code) in rows {
        let mut params = vec![param("CapacityReservationFleetId", &fleet.id)];
        params.extend(extra);
        let (status, body) = api(&router, "ModifyCapacityReservationFleet", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        let untouched = described(case, &router, &fleet.id).await;
        assert_eq!(
            (
                untouched.total_target_capacity,
                untouched.end_date.clone(),
                untouched.reservations.clone(),
            ),
            (
                fleet.total_target_capacity,
                fleet.end_date.clone(),
                fleet.reservations.clone(),
            ),
            "case {case:?}: a refused request modifies nothing, got {untouched:?}"
        );
    }
}

/// RULE 3: a Fleet that is being cancelled, or has been, is not one a modification runs from —
/// the request is refused with `IncorrectState` rather than answered a success that changed
/// nothing.
#[tokio::test]
async fn a_fleet_that_is_no_longer_reserving_cannot_be_modified() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;
    let fleet = create("a Fleet to cancel", &router, &REFUSAL_START, &zone).await;
    past_the_transition().await;

    let cancelled = ok(
        "cancel it",
        &router,
        "CancelCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[fleet.id.as_str()]),
    )
    .await;
    let doc = document("cancel it", &cancelled);
    assert_eq!(
        set_items(doc.root_element(), "successfulFleetCancellationSet").len(),
        1,
        "the Fleet must be cancellable, got body {cancelled}"
    );

    // While `cancelling`, and once the clock has made it `cancelled`: neither is a state a
    // modification runs from — and a DRY RUN against such a Fleet is answered the same
    // reason, since the Fleet is resolved before the flag is answered.
    for (case, dry_run) in [
        ("while cancelling", false),
        ("while cancelling, as a dry run", true),
        ("once cancelled", false),
        ("once cancelled, as a dry run", true),
    ] {
        if case == "once cancelled" {
            past_the_transition().await;
        }
        let mut params = vec![
            param("CapacityReservationFleetId", &fleet.id),
            param("TotalTargetCapacity", "8"),
        ];
        if dry_run {
            params.push(param("DryRun", "true"));
        }
        let (status, body) = api(&router, "ModifyCapacityReservationFleet", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected the wrong-state code, got body {body}"
        );
        let untouched = described(case, &router, &fleet.id).await;
        assert_eq!(
            untouched.total_target_capacity, fleet.total_target_capacity,
            "case {case:?}: a refused modification changes no capacity, got {untouched:?}"
        );
    }

    let gone = described("still cancelled", &router, &fleet.id).await;
    assert_eq!(
        gone.state, CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
        "a refused modification leaves the Fleet cancelled, got {:?}",
        gone.state
    );
}

// ---------------------------------------------------------------------------
// Rule 4: DryRun performs nothing
// ---------------------------------------------------------------------------

/// RULE 4: `DryRun` answers whether the request WOULD have succeeded and modifies nothing —
/// a request that would have been refused is refused with that reason instead.
#[tokio::test]
async fn a_dry_run_modifies_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;
    let fleet = create("a Fleet to dry-run against", &router, &REFUSAL_START, &zone).await;

    for (case, target_id, extra, expected_status, expected_code) in [
        (
            "a modification that would have succeeded",
            fleet.id.as_str(),
            vec![param("TotalTargetCapacity", "9")],
            412,
            "DryRunOperation",
        ),
        (
            "a modification that would have been refused",
            fleet.id.as_str(),
            vec![param("TotalTargetCapacity", "0")],
            400,
            "InvalidParameterValue",
        ),
        (
            // The Fleet is resolved before the flag is answered, so a dry run against a
            // Fleet that does not exist reports THAT rather than "it would have succeeded".
            "a modification of a Fleet the account does not hold",
            UNKNOWN_FLEET_ID,
            vec![param("TotalTargetCapacity", "9")],
            400,
            "InvalidCapacityReservationFleetId.NotFound",
        ),
    ] {
        let mut params = vec![
            param("CapacityReservationFleetId", target_id),
            param("DryRun", "true"),
        ];
        params.extend(extra);
        let (status, body) = api(&router, "ModifyCapacityReservationFleet", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        let untouched = described(case, &router, &fleet.id).await;
        assert_eq!(
            (
                untouched.total_target_capacity,
                untouched.end_date.clone(),
                untouched.reservations.clone(),
            ),
            (
                fleet.total_target_capacity,
                fleet.end_date.clone(),
                fleet.reservations.clone(),
            ),
            "case {case:?}: a dry run modifies nothing, got {untouched:?}"
        );
        // The Fleet's own state word is the CLOCK's, so it is asserted as "not a state this
        // call put it in" rather than against a snapshot a transition may have moved on.
        assert_ne!(
            untouched.state, CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
            "case {case:?}: a dry run starts no modification, got {untouched:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The capacity reservation of a load balancer, end to end over the wire:
//! `DescribeCapacityReservation` reports it and `ModifyCapacityReservation` writes it.
//!
//! The rules these tables pin:
//!
//! * `DescribeCapacityReservation` reports the reservation the STORE holds for the load
//!   balancer the request names, and nothing else: the instant a capacity-reservation
//!   write last changed it — `None` until one does — and, for a load balancer that
//!   reserves no minimum capacity, no capacity and no zone holding any. It is a reporting
//!   operation, so two reads of one load balancer answer alike, a read changes nothing a
//!   later read sees, and a write to one load balancer is invisible to every other.
//! * `ModifyCapacityReservation` reads the capacity a request asks to reserve against the
//!   published capacity-units quota of one load balancer FIRST: past that quota the
//!   request is the modelled `CapacityUnitsLimitExceeded`, naming the quota and the figure
//!   asked for, and a negative figure is no capacity to reserve at all. Within the quota
//!   the account capability decides it, and this account cannot configure minimum
//!   capacity, so it is the modelled `OperationNotPermitted` in the words AWS words that
//!   refusal.
//! * A reset is served, recorded, and reported by a later read as the instant it happened.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`, `members`, `first_text`) comes
//! from `provisioning_loop`, the load balancer creator from `load_balancer_attributes`,
//! and the request builder, the refusal reader and the absent-ARN speller from
//! `load_balancer_network_capacity_and_policy`.

use crate::load_balancer_attributes::create_load_balancer;
use crate::load_balancer_network_capacity_and_policy::{
    absent_load_balancer_arn, assert_refused, call_on,
};
use crate::provisioning_loop::{emulator, first_text, members, Reply, REGION};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::CAPACITY_UNITS_PER_LOAD_BALANCER;
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `CapacityUnitsLimitExceededException`
/// (`smithy_query(query="operation",operation="ModifyCapacityReservation")`).
const CAPACITY_UNITS_LIMIT_EXCEEDED: &str = "CapacityUnitsLimitExceeded";

/// The awsQuery error code of `OperationNotPermittedException`.
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery error code of `LoadBalancerNotFoundException`.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

/// The error code this service answers a value it cannot accept with, for a rule its model
/// states in prose rather than as an error shape.
const VALIDATION_ERROR: &str = "ValidationError";

/// The refusal AWS answers a request to configure minimum load balancer capacity with for
/// an account that does not have that capability, in the words it words it.
const MINIMUM_CAPACITY_NOT_SUPPORTED: &str = "This AWS account does not support configuring \
     minimum load balancer capacity reservation. For more information, please contact AWS \
     Support.";

/// What a reply reports about the capacity reservation of one load balancer: the four
/// members `DescribeCapacityReservation` and `ModifyCapacityReservation` both carry.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    /// `LastModifiedTime`, as reported.
    modified_at: Option<String>,
    /// `MinimumLoadBalancerCapacity$CapacityUnits`.
    capacity_units: Option<String>,
    /// `DecreaseRequestsRemaining`.
    decreases_remaining: Option<String>,
    /// One `(state code, zone name)` per `CapacityReservationState` member, in the order
    /// reported.
    zones: Vec<(String, String)>,
}

/// Read the capacity reservation out of one reply.
fn reported(reply: &Reply) -> Reported {
    Reported {
        modified_at: reply.text("LastModifiedTime"),
        capacity_units: reply.text("CapacityUnits"),
        decreases_remaining: reply.text("DecreaseRequestsRemaining"),
        zones: members(&reply.body, "CapacityReservationState")
            .iter()
            .map(|zone| {
                (
                    first_text(zone, "Code").unwrap_or_default(),
                    first_text(zone, "AvailabilityZone").unwrap_or_default(),
                )
            })
            .collect(),
    }
}

/// The capacity reservation a read reports for the load balancer `arn` names.
async fn read_reservation(router: &MultiServiceRouter, case: &str, arn: &str) -> Reported {
    let reply = call_on(router, "DescribeCapacityReservation", arn, &[]).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the read, got {} body {}",
        reply.status, reply.body
    );
    reported(&reply)
}

/// What a load balancer that reserves no minimum capacity reports, beside the instant of
/// the last capacity-reservation write (`modified_at`).
fn nothing_reserved(modified_at: Option<String>) -> Reported {
    Reported {
        modified_at,
        capacity_units: None,
        decreases_remaining: None,
        zones: Vec::new(),
    }
}

/// Ask `ModifyCapacityReservation` to reserve `units` capacity units for the load balancer
/// `arn` names.
async fn reserve(router: &MultiServiceRouter, arn: &str, units: i32) -> Reply {
    call_on(
        router,
        "ModifyCapacityReservation",
        arn,
        &[(
            "MinimumLoadBalancerCapacity.CapacityUnits",
            &units.to_string(),
        )],
    )
    .await
}

/// Reset the capacity reservation of the load balancer `arn` names.
async fn reset(router: &MultiServiceRouter, case: &str, arn: &str) -> Reported {
    let reply = call_on(
        router,
        "ModifyCapacityReservation",
        arn,
        &[("ResetCapacityReservation", "true")],
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from the reset, got {} body {}",
        reply.status, reply.body
    );
    reported(&reply)
}

// ---------------------------------------------------------------------------
// What a read reports is what the store holds.
// ---------------------------------------------------------------------------

/// Every load balancer this store holds reports the reservation it carries: no minimum
/// capacity, no zone holding any, and no instant until a capacity-reservation write
/// records one.
///
/// The rows span the load balancers a read can be made against, which is what the rule
/// generalises over: each family, both schemes, and placements of one, two and three
/// zones — a read is answered from the record, so none of that changes what is reported.
#[tokio::test]
async fn every_load_balancer_reports_the_reservation_its_record_holds() {
    let (router, _engine) = emulator();

    // (case, the fields the load balancer is created with)
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("an application load balancer in two zones", vec![]),
        (
            "an application load balancer in three zones",
            vec![
                ("Subnets.member.1", "subnet-11111111"),
                ("Subnets.member.2", "subnet-22222222"),
                ("Subnets.member.3", "subnet-33333333"),
            ],
        ),
        (
            "an internal application load balancer",
            vec![("Scheme", "internal")],
        ),
        (
            "a network load balancer in one zone",
            vec![("Type", "network"), ("Subnets.member.1", "subnet-11111111")],
        ),
        (
            "a gateway load balancer",
            vec![("Type", "gateway"), ("IpAddressType", "ipv4")],
        ),
    ];

    for (index, (case, fields)) in cases.iter().enumerate() {
        let arn = create_load_balancer(&router, &format!("lb-reservation-{index}"), fields).await;
        assert_eq!(
            read_reservation(&router, case, &arn).await,
            nothing_reserved(None),
            "case {case:?}: nothing has written a reservation for it"
        );
    }
}

/// A read of a capacity reservation reports, it does not write: two reads of one load
/// balancer answer alike, a read leaves what a later read sees untouched, and a reservation
/// write to one load balancer is invisible to every other.
#[tokio::test]
async fn reading_a_capacity_reservation_changes_nothing_a_later_read_sees() {
    let (router, _engine) = emulator();
    let written = create_load_balancer(&router, "lb-reservation-written", &[]).await;
    let untouched = create_load_balancer(&router, "lb-reservation-untouched", &[]).await;

    // Ten reads of one load balancer, each answering exactly what the first did.
    let first = read_reservation(&router, "the first read", &written).await;
    for attempt in 1..=10 {
        let case = format!("read {attempt} of the same load balancer");
        assert_eq!(
            read_reservation(&router, &case, &written).await,
            first,
            "case {case:?}: a read must answer what the record holds, unchanged by reading it"
        );
    }

    // A write to one load balancer is reported for that one alone.
    let recorded = reset(&router, "the recorded reset", &written).await;
    assert!(
        recorded.modified_at.is_some(),
        "case the recorded reset: the write records when it happened, got {recorded:?}"
    );
    assert_eq!(
        read_reservation(&router, "the load balancer the reset named", &written).await,
        nothing_reserved(recorded.modified_at.clone()),
        "case the load balancer the reset named: a read reports the instant the write recorded"
    );
    assert_eq!(
        read_reservation(&router, "the load balancer the reset did not name", &untouched).await,
        nothing_reserved(None),
        "case the load balancer the reset did not name: another load balancer's write is not \
         its own"
    );
}

/// A reset is served and recorded whatever has gone before it, and the instant a later read
/// reports is the one the write that touched the reservation answered.
///
/// The rows span the writes that can precede a read: none, one reset, a reset after a
/// refused reservation (which records nothing), and a second reset.
#[tokio::test]
async fn a_read_reports_the_instant_the_last_write_recorded() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-reservation-resets", &[]).await;

    // Nothing has been written yet.
    assert_eq!(
        read_reservation(&router, "before any write", &arn).await,
        nothing_reserved(None),
        "case before any write: no capacity-reservation write has touched it"
    );

    // A refused reservation records nothing.
    let refused = reserve(&router, &arn, 100).await;
    assert_refused("a refused reservation", &refused, OPERATION_NOT_PERMITTED, "");
    assert_eq!(
        read_reservation(&router, "after a refused reservation", &arn).await,
        nothing_reserved(None),
        "case after a refused reservation: a refusal writes nothing"
    );

    // Each reset is recorded, and what the read reports is what that write answered.
    for attempt in 1..=3 {
        let case = format!("reset {attempt}");
        let recorded = reset(&router, &case, &arn).await;
        assert!(
            recorded.modified_at.is_some(),
            "case {case:?}: the write records when it happened, got {recorded:?}"
        );
        assert_eq!(
            read_reservation(&router, &case, &arn).await,
            nothing_reserved(recorded.modified_at.clone()),
            "case {case:?}: a read reports what that write left behind"
        );
    }
}

// ---------------------------------------------------------------------------
// The capacity a reservation asks for.
// ---------------------------------------------------------------------------

/// The capacity units a request asks to reserve are read against the published
/// capacity-units quota of one load balancer before anything else: past the quota the
/// request is the modelled `CapacityUnitsLimitExceeded` naming the quota and the figure
/// asked for, a negative figure is refused for being no capacity at all, and a figure
/// inside the quota reaches the account capability — which this account does not have, so
/// it is the modelled `OperationNotPermitted` in AWS's own words.
///
/// The rows walk the published bound: zero, one, well inside it, one unit inside it, the
/// bound itself, one unit past it, far past it, and two values below the bottom of the
/// range. Nothing is persisted for any of them.
#[tokio::test]
async fn the_capacity_a_reservation_asks_for_is_read_against_the_published_quota() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-reservation-bound", &[]).await;
    let quota = CAPACITY_UNITS_PER_LOAD_BALANCER;

    // (case, the capacity units asked for, the awsQuery error code expected)
    let cases: Vec<(&str, i32, &str)> = vec![
        ("no capacity units", 0, OPERATION_NOT_PERMITTED),
        ("one capacity unit", 1, OPERATION_NOT_PERMITTED),
        ("a capacity well inside the quota", 100, OPERATION_NOT_PERMITTED),
        ("one unit inside the quota", quota - 1, OPERATION_NOT_PERMITTED),
        ("the published quota itself", quota, OPERATION_NOT_PERMITTED),
        (
            "one unit past the published quota",
            quota + 1,
            CAPACITY_UNITS_LIMIT_EXCEEDED,
        ),
        (
            "far past the published quota",
            quota * 100,
            CAPACITY_UNITS_LIMIT_EXCEEDED,
        ),
        ("a negative capacity", -1, VALIDATION_ERROR),
        ("a far negative capacity", -quota, VALIDATION_ERROR),
    ];

    for (case, units, code) in &cases {
        let reply = reserve(&router, &arn, *units).await;
        assert_refused(case, &reply, code, "");
        let message = reply.text("Message").unwrap_or_default();
        match *code {
            // The refusal names the quota the request passed and the figure it asked for,
            // both taken from the request and this service's own published limit.
            CAPACITY_UNITS_LIMIT_EXCEEDED => assert!(
                message.contains(&quota.to_string()) && message.contains(&units.to_string()),
                "case {case:?}: expected a message naming the quota and the figure asked for, \
                 got {message:?}"
            ),
            // The account capability is refused in the words AWS words it, with the
            // remediation it points a caller at.
            OPERATION_NOT_PERMITTED => assert_eq!(
                message, MINIMUM_CAPACITY_NOT_SUPPORTED,
                "case {case:?}: unexpected message in {}",
                reply.body
            ),
            _ => assert!(
                message.contains(&units.to_string()),
                "case {case:?}: expected a message naming the value refused, got {message:?}"
            ),
        }
        assert_eq!(
            read_reservation(&router, case, &arn).await,
            nothing_reserved(None),
            "case {case:?}: a refused reservation persists nothing"
        );
    }
}

/// The figure is read before the load balancer is looked up, so a reservation past the
/// published quota is refused for the figure even when the ARN names no load balancer this
/// store holds — while a reset of one that is not there is the modelled
/// `LoadBalancerNotFound`.
///
/// The rows span the shapes of an ARN that names no load balancer: one of each type code
/// and one of another Region.
#[tokio::test]
async fn an_absent_load_balancer_is_refused_for_the_figure_before_the_lookup() {
    let (router, _engine) = emulator();
    let quota = CAPACITY_UNITS_PER_LOAD_BALANCER;

    let absent: Vec<(&str, String)> = vec![
        (
            "an application load balancer ARN nothing carries",
            absent_load_balancer_arn(REGION, "app", "absent-capacity", "0000000000000000"),
        ),
        (
            "a network load balancer ARN nothing carries",
            absent_load_balancer_arn(REGION, "net", "absent-capacity-net", "1111111111111111"),
        ),
        (
            "a well-formed ARN of another Region",
            absent_load_balancer_arn("eu-west-1", "app", "elsewhere", "2222222222222222"),
        ),
    ];

    for (case, arn) in &absent {
        let past_the_quota = reserve(&router, arn, quota + 1).await;
        assert_refused(case, &past_the_quota, CAPACITY_UNITS_LIMIT_EXCEEDED, "");

        let inside_the_quota = reserve(&router, arn, 1).await;
        assert_refused(case, &inside_the_quota, OPERATION_NOT_PERMITTED, "");

        let cleared = call_on(
            &router,
            "ModifyCapacityReservation",
            arn,
            &[("ResetCapacityReservation", "true")],
        )
        .await;
        assert_refused(case, &cleared, LOAD_BALANCER_NOT_FOUND, arn);
    }
}

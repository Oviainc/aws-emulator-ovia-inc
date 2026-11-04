// SPDX-License-Identifier: CC0-1.0

//! The capacity reservation of a load balancer, end to end over the wire:
//! `DescribeCapacityReservation` reports it and `ModifyCapacityReservation` writes it.
//!
//! The rules these tables pin:
//!
//! * `DescribeCapacityReservation` reports the reservation the STORE holds for the load
//!   balancer the request names, and nothing else: the instant a capacity-reservation
//!   write last changed it — `None` until one does — and, for a load balancer that
//!   reserves no minimum capacity, no capacity and no zone holding any, beside the
//!   account's daily capacity-decrease allowance, which is an allowance of the day rather
//!   than of a reservation. It is a reporting operation, so two reads of one load balancer
//!   answer alike, a read changes nothing a later read sees, and a write to one load
//!   balancer is invisible to every other.
//! * `ModifyCapacityReservation` settles the ACCOUNT entitlement to configure minimum load
//!   balancer capacity FIRST, before the capacity a request asks for is read and before the
//!   load balancer is looked up: this account is not entitled, so every request that names
//!   `MinimumLoadBalancerCapacity` is the modelled `OperationNotPermitted` with the one
//!   account-level sentence AWS words that refusal in, byte-identical whatever figure the
//!   request carries, interpolating no value from it and persisting nothing.
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
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    CAPACITY_UNITS_PER_LOAD_BALANCER, DAILY_CAPACITY_DECREASES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The awsQuery error code of `OperationNotPermittedException`
/// (`smithy_query(query="operation",operation="ModifyCapacityReservation")`).
const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

/// The awsQuery error code of `LoadBalancerNotFoundException`.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

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
///
/// `DecreaseRequestsRemaining` is NOT part of "nothing reserved": it is the account's
/// allowance for the day ([`DAILY_CAPACITY_DECREASES`]), reported for every load balancer
/// whether or not it reserves capacity, and nothing here spends it — the rows that walk
/// every family are `reported_member_defaults.rs`.
fn nothing_reserved(modified_at: Option<String>) -> Reported {
    Reported {
        modified_at,
        capacity_units: None,
        decreases_remaining: Some(DAILY_CAPACITY_DECREASES.to_string()),
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

/// The figures a request can ask to reserve, walked across the published per-load-balancer
/// capacity-units quota (`crate::state::CAPACITY_UNITS_PER_LOAD_BALANCER`, the figure
/// `DescribeAccountLimits` publishes as `application-load-balancer-capacity-units`) and
/// across the range the modelled `CapacityUnits` integer carries: both sides of zero, one
/// unit, the documented application floor, each side of the quota and the quota itself, far
/// past it, and the two extremes of the integer.
fn capacity_figures() -> Vec<(&'static str, i32)> {
    let quota = CAPACITY_UNITS_PER_LOAD_BALANCER;
    vec![
        ("the smallest figure the modelled integer carries", i32::MIN),
        ("a far negative capacity", -quota),
        ("a negative capacity", -5),
        ("no capacity units", 0),
        ("one capacity unit", 1),
        ("the documented application floor", 100),
        ("one unit inside the published quota", quota - 1),
        ("the published quota itself", quota),
        ("one unit past the published quota", quota + 1),
        ("far past the published quota", quota * 100),
        ("the largest figure the modelled integer carries", i32::MAX),
    ]
}

/// Assert that `reply` is the account-level refusal: the modelled `OperationNotPermitted`
/// with the one sentence AWS words it in, carrying no figure from the request.
fn assert_not_entitled(case: &str, reply: &Reply) {
    assert_refused(case, reply, OPERATION_NOT_PERMITTED, "");
    let message = reply.text("Message").unwrap_or_default();
    assert_eq!(
        message, MINIMUM_CAPACITY_NOT_SUPPORTED,
        "case {case:?}: expected the account-level sentence, got body {}",
        reply.body
    );
    // Account-level fixed text: no unit count, no quota ceiling, no other request value.
    assert!(
        !message.chars().any(|character| character.is_ascii_digit()),
        "case {case:?}: the refusal must interpolate no request value, got {message:?}"
    );
}

/// Configuring minimum load balancer capacity is settled by the ACCOUNT entitlement before
/// the capacity the request asks for is read: this account is not entitled, so every figure
/// is refused with the modelled `OperationNotPermitted` and the one account-level sentence,
/// byte-identical in code and message across figures, and nothing is persisted for any of
/// them.
///
/// The rows are every figure in `capacity_figures()` — in particular both sides of the
/// published quota, which a value-dependent quota check would separate.
#[tokio::test]
async fn the_account_entitlement_refuses_every_capacity_a_reservation_asks_for() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-reservation-entitlement", &[]).await;

    // What the first row answered: every later row must answer exactly the same refusal.
    let mut first: Option<(String, String, &str)> = None;

    for (case, units) in capacity_figures() {
        let reply = reserve(&router, &arn, units).await;
        assert_not_entitled(case, &reply);

        let answered = (
            reply.error_code(),
            reply.text("Message").unwrap_or_default(),
            case,
        );
        match &first {
            None => first = Some(answered),
            Some((code, message, first_case)) => assert_eq!(
                (&answered.0, &answered.1),
                (code, message),
                "case {case:?}: every refusal on one load balancer must be identical to \
                 {first_case:?} in code and message"
            ),
        }

        assert_eq!(
            read_reservation(&router, case, &arn).await,
            nothing_reserved(None),
            "case {case:?}: a refused reservation persists nothing"
        );
    }
}

/// The entitlement is the ACCOUNT's, so it is settled before the load balancer is looked up
/// and whatever load balancer the request names: a reservation is the same
/// `OperationNotPermitted` for every load balancer this store holds and for an ARN it holds
/// none for — while a reset reaches the lookup, so an absent one is the modelled
/// `LoadBalancerNotFound`.
///
/// The rows span the load balancers a reservation can name — each family, both schemes,
/// and the shapes of an ARN that names none: one of each type code and one of another
/// Region — crossed with every figure in `capacity_figures()`.
#[tokio::test]
async fn the_entitlement_is_settled_before_the_load_balancer_is_looked_up() {
    let (router, _engine) = emulator();

    // (case, the fields the load balancer is created with)
    let families: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("an application load balancer", vec![]),
        (
            "an internal application load balancer",
            vec![("Scheme", "internal")],
        ),
        (
            "a network load balancer",
            vec![("Type", "network"), ("Subnets.member.1", "subnet-11111111")],
        ),
        (
            "a gateway load balancer",
            vec![("Type", "gateway"), ("IpAddressType", "ipv4")],
        ),
    ];

    let mut targets: Vec<(String, String, bool)> = Vec::new();
    for (index, (case, fields)) in families.iter().enumerate() {
        let arn = create_load_balancer(&router, &format!("lb-entitlement-{index}"), fields).await;
        targets.push(((*case).to_string(), arn, true));
    }
    for (case, arn) in [
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
    ] {
        targets.push((case.to_string(), arn, false));
    }

    for (target, arn, in_store) in &targets {
        for (figure, units) in capacity_figures() {
            let case = format!("{target} reserving {figure}");
            let reply = reserve(&router, arn, units).await;
            assert_not_entitled(&case, &reply);
        }

        // The reset is served for a load balancer this store holds and reaches the lookup
        // for one it does not: the entitlement gate covers reservations only.
        let cleared = call_on(
            &router,
            "ModifyCapacityReservation",
            arn,
            &[("ResetCapacityReservation", "true")],
        )
        .await;
        if *in_store {
            assert_eq!(
                cleared.status, 200,
                "case {target:?}: a reset is served, got {} body {}",
                cleared.status, cleared.body
            );
            // And the reset left nothing reserved, with the instant it happened recorded.
            let reported = read_reservation(&router, target, arn).await;
            assert!(
                reported.modified_at.is_some()
                    && reported == nothing_reserved(reported.modified_at.clone()),
                "case {target:?}: a served reset leaves nothing reserved and records when it \
                 happened, got {reported:?}"
            );
        } else {
            assert_refused(target, &cleared, LOAD_BALANCER_NOT_FOUND, arn);
        }
    }
}

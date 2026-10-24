// SPDX-License-Identifier: CC0-1.0

//! DescribeReservation, end to end over HTTP: `GET /v1/reservations/{ReservationArn}`.
//!
//! The subject is the rule DescribeReservation follows: the reservation the response
//! carries must be the RECORD the store holds under the ARN the request names — every
//! member projected from that record, in the state the CLOCK has brought it to — and a
//! request naming no stored reservation must be refused with the modelled
//! `NotFoundException`. Nothing in the response is fixed in the handler, so the cases below
//! vary what the rule generalises over: the stored state, whether the clock owes the record
//! a change, whether its `End` has passed, the offering terms copied onto it (duration,
//! price, reserved bitrate — including the optional member left absent), the form of its
//! name, and the form of an identifier that resolves nothing.
//!
//! The records are put in the store directly, the way a purchase stores one, through the
//! seeding helper `reservation_listing` owns: PurchaseOffering is still a scaffold
//! (`issues/things-that-do-not-work/20261024T010542Z-mediaconnect-purchaseoffering-stub.md`),
//! so there is no request that mints a reservation yet. Every DESCRIBE call below goes over
//! the wire through the same `MultiServiceRouter` the emulator's front door uses, so what
//! these tests assert is what a MediaConnect client sees: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::reservation_listing::{
    assert_documented_instant, assert_minted_reservation_arn, list_reservations, listed_by_arn,
    reservation_cases, seed_reservation, ReservationCase, CURRENCY_CODE, DURATION_UNITS,
    ONE_MONTH, PRICE_UNITS, RESOURCE_TYPE,
};

async fn describe_reservation(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/reservations/{arn}"), None)
        .await
}

/// DescribeReservation reports the stored reservation the request names, projected field
/// for field from the record and in the state the clock has brought it to — and reports it
/// exactly as ListReservations does, because both read the one projection.
#[tokio::test]
async fn describe_reports_the_stored_reservation_the_arn_names() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    let cases = reservation_cases();
    let arns: Vec<String> = cases
        .iter()
        .map(|case| seed_reservation(&emulator, case, now))
        .collect();

    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        let (status, code, body) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {name_of_case:?}: expected 200, got {status} {code:?}: {body}"
        );
        let reservation = &body["reservation"];
        assert_minted_reservation_arn(name_of_case, arn, case.name);
        assert_eq!(
            reservation["reservationArn"].as_str(),
            Some(arn.as_str()),
            "case {name_of_case:?}: the described reservation is the one the ARN names, got \
             {body}"
        );
        assert_eq!(
            reservation["reservationName"].as_str(),
            Some(case.name),
            "case {name_of_case:?}: the reported name is the stored one, got {body}"
        );
        assert_eq!(
            reservation["reservationState"].as_str(),
            Some(case.expected_state),
            "case {name_of_case:?}: expected state {:?}, got {body}",
            case.expected_state
        );
        assert_eq!(
            reservation["duration"].as_i64(),
            Some(i64::from(case.duration)),
            "case {name_of_case:?}: the reported duration is the stored one, got {body}"
        );
        assert_eq!(
            (
                reservation["durationUnits"].as_str(),
                reservation["priceUnits"].as_str(),
                reservation["currencyCode"].as_str(),
                reservation["pricePerUnit"].as_str(),
            ),
            (
                Some(DURATION_UNITS),
                Some(PRICE_UNITS),
                Some(CURRENCY_CODE),
                Some(case.price_per_unit),
            ),
            "case {name_of_case:?}: the billing terms are the stored ones, got {body}"
        );
        assert_eq!(
            reservation["resourceSpecification"]["resourceType"].as_str(),
            Some(RESOURCE_TYPE),
            "case {name_of_case:?}: the resource type is the stored one, got {body}"
        );
        assert_eq!(
            reservation["resourceSpecification"]["reservedBitrate"].as_i64(),
            case.reserved_bitrate.map(i64::from),
            "case {name_of_case:?}: the reserved bitrate is the stored one (absent when the \
             record names none), got {body}"
        );
        assert!(
            reservation["offeringArn"]
                .as_str()
                .is_some_and(|offering_arn| offering_arn.contains(":offering:")),
            "case {name_of_case:?}: the response names the offering the reservation was \
             bought from, got {body}"
        );
        assert!(
            reservation["offeringDescription"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "case {name_of_case:?}: the offering description is reported, got {body}"
        );
        for member in ["start", "end"] {
            let value = reservation[member].as_str().unwrap_or_default();
            assert_documented_instant(name_of_case, member, value);
        }
        assert!(
            reservation["start"].as_str() < reservation["end"].as_str(),
            "case {name_of_case:?}: a reservation ends after it starts, got {body}"
        );
    }

    // The two members copied from the offering the reservation was bought from are the
    // stored ones too, compared against the record itself rather than a shape check.
    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        let (stored_offering_arn, stored_offering_description) = {
            let state = emulator
                .engine
                .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
            let store = state.lock();
            let record = store
                .reservations
                .get(arn)
                .expect("the seeded reservation is stored");
            (
                record.offering_arn.clone(),
                record.offering_description.clone(),
            )
        };
        let (_, _, body) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (
                body["reservation"]["offeringArn"].as_str(),
                body["reservation"]["offeringDescription"].as_str(),
            ),
            (
                Some(stored_offering_arn.as_str()),
                Some(stored_offering_description.as_str()),
            ),
            "case {name_of_case:?}: the offering ARN and description are the stored ones, got \
             {body}"
        );
    }

    // One record, one projection: what DescribeReservation reports for an ARN is what
    // ListReservations reports for the same ARN, member for member.
    let (status, _, listing) = list_reservations(&emulator, "?maxResults=1000").await;
    assert_eq!(status, 200, "listing expected 200, got {status}: {listing}");
    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        let listed = listed_by_arn(&listing, arn).unwrap_or_else(|| {
            panic!("case {name_of_case:?}: the stored reservation must be listed, got {listing}")
        });
        let (_, _, described) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            described["reservation"], listed,
            "case {name_of_case:?}: describing and listing report the same reservation"
        );
    }
}

/// A read does not change what a later caller sees: describing the same reservation twice
/// answers the same record both times (no row of the table is due a clock change within the
/// test's lifetime), and describing it does not add, remove or alter what the account holds.
#[tokio::test]
async fn describing_a_reservation_changes_nothing() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    let cases = reservation_cases();
    let arns: Vec<String> = cases
        .iter()
        .map(|case| seed_reservation(&emulator, case, now))
        .collect();

    let (_, _, before) = list_reservations(&emulator, "?maxResults=1000").await;
    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        let (first_status, _, first) = describe_reservation(&emulator, arn).await;
        let (second_status, _, second) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (first_status, second_status),
            (200, 200),
            "case {name_of_case:?}: both reads expected 200, got {first_status} and \
             {second_status}"
        );
        assert_eq!(
            first, second,
            "case {name_of_case:?}: describing twice reports the same reservation"
        );
    }
    let (_, _, after) = list_reservations(&emulator, "?maxResults=1000").await;
    assert_eq!(
        after, before,
        "describing reservations leaves the account's reservations as they were, got {after}"
    );
}

/// The state DescribeReservation reports is the one the CLOCK has brought the record to,
/// not the one a call moves it to: a purchase still settling is reported `PROCESSING` and
/// becomes `ACTIVE` once its settle is due, and a running reservation is reported `ACTIVE`
/// until its `End` passes, with no call in between moving either.
#[tokio::test]
async fn the_clock_moves_a_described_reservation_state() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    // Short enough that the test does not wait long, and long enough that the first read
    // lands well before it, even on a loaded runner.
    let span = Duration::from_millis(1_500);
    let settling = ReservationCase {
        case: "purchase settling during the test",
        name: "res-settling",
        duration: 6,
        price_per_unit: "0.2500",
        reserved_bitrate: Some(50),
        start_before_now: Duration::from_secs(0),
        end_from_now: Ok(ONE_MONTH * 6),
        stored_state: mc::RESERVATION_STATE_PROCESSING,
        pending: Some((mc::RESERVATION_STATE_ACTIVE, Ok(span))),
        expected_state: mc::RESERVATION_STATE_PROCESSING,
    };
    let expiring = ReservationCase {
        case: "reservation ending during the test",
        name: "res-ending",
        duration: 1,
        price_per_unit: "0.1500",
        reserved_bitrate: Some(100),
        start_before_now: Duration::from_secs(60),
        end_from_now: Ok(span),
        stored_state: mc::RESERVATION_STATE_ACTIVE,
        pending: None,
        expected_state: mc::RESERVATION_STATE_ACTIVE,
    };
    let settling_arn = seed_reservation(&emulator, &settling, now);
    let expiring_arn = seed_reservation(&emulator, &expiring, now);

    for (case, arn, expected) in [
        (
            settling.case,
            &settling_arn,
            mc::RESERVATION_STATE_PROCESSING,
        ),
        (expiring.case, &expiring_arn, mc::RESERVATION_STATE_ACTIVE),
    ] {
        let (status, _, body) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (status, body["reservation"]["reservationState"].as_str()),
            (200, Some(expected)),
            "case {case:?}: before the clock settles it, expected {expected:?}, got {body}"
        );
    }
    let (_, _, ends_at) = describe_reservation(&emulator, &expiring_arn).await;
    let end_before = ends_at["reservation"]["end"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    tokio::time::sleep(span + Duration::from_millis(250)).await;

    for (case, arn, expected) in [
        (settling.case, &settling_arn, mc::RESERVATION_STATE_ACTIVE),
        (expiring.case, &expiring_arn, mc::RESERVATION_STATE_EXPIRED),
    ] {
        let (status, _, body) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (status, body["reservation"]["reservationState"].as_str()),
            (200, Some(expected)),
            "case {case:?}: once the clock has moved it, expected {expected:?}, got {body}"
        );
    }
    // An expired reservation is still a readable resource, and expiry does not rewrite the
    // terms of the agreement that was struck.
    let (_, _, expired) = describe_reservation(&emulator, &expiring_arn).await;
    assert_eq!(
        expired["reservation"]["end"].as_str(),
        Some(end_before.as_str()),
        "expiry does not move the reservation's End, got {expired}"
    );
}

/// One DescribeReservation request that resolves no reservation.
struct MissingCase {
    case: &'static str,
    /// The `ReservationArn` label the request carries.
    arn: String,
}

/// DescribeReservation refuses an identifier that names no stored reservation with the
/// modelled `NotFoundException` (404) — never with a 501, never with a success and never
/// with another reservation. The rows are the ways an identifier can resolve nothing: a
/// well-formed reservation ARN nobody purchased, one of another Region or account, another
/// service's ARN, another resource type of this service, the ARN of a reservation that was
/// never minted here, and a string that is no ARN at all.
#[tokio::test]
async fn an_unknown_reservation_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    // One reservation IS stored, so a refusal below cannot be the store simply being empty.
    let cases = reservation_cases();
    let stored = cases.first().expect("the table has rows");
    let stored_arn = seed_reservation(&emulator, stored, now);

    let missing = vec![
        MissingCase {
            case: "a well-formed reservation ARN nobody purchased",
            arn: "arn:aws:mediaconnect:us-east-1:000000000000:reservation:1-deadbeef:missing"
                .to_string(),
        },
        MissingCase {
            case: "a reservation ARN of another Region",
            arn: "arn:aws:mediaconnect:eu-west-1:000000000000:reservation:1-deadbeef:missing"
                .to_string(),
        },
        MissingCase {
            case: "a reservation ARN of another account",
            arn: "arn:aws:mediaconnect:us-east-1:999999999999:reservation:1-deadbeef:missing"
                .to_string(),
        },
        MissingCase {
            case: "another service's ARN",
            arn: "arn:aws:medialive:us-east-1:000000000000:reservation:1-deadbeef:missing"
                .to_string(),
        },
        MissingCase {
            case: "another resource type of this service",
            arn: stored_arn.replace(":reservation:", ":flow:"),
        },
        MissingCase {
            case: "the stored ARN with one character changed",
            arn: format!("{stored_arn}x"),
        },
        MissingCase {
            case: "an identifier that is no ARN at all",
            arn: "not-an-arn".to_string(),
        },
    ];

    for MissingCase { case, arn } in &missing {
        let (status, code, body) = describe_reservation(&emulator, arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
        assert!(
            body["reservation"].is_null(),
            "case {case:?}: a refusal carries no reservation, got {body}"
        );
    }

    // The stored reservation is still readable: a refused read did not evict it.
    let (status, code, body) = describe_reservation(&emulator, &stored_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the stored reservation is still described, got {status} {code:?}: {body}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `DescribeReservation` as a KEYED REPORT: "Get details for a reservation"
//! (`smithy_query(query="operation", operation="DescribeReservation")`) answers
//! with the stored reservation the `ReservationId` in its path names.
//!
//! The rule this suite pins, over a table rather than one request: a read is
//! resolved by the identifier in its path alone — it reports that reservation's
//! own stored members whatever else the account holds and whatever order the
//! reads arrive in, reporting it again reports the same thing (a report writes
//! nothing a later caller can see), and an identifier the account holds no
//! reservation for is the operation's modelled 404 carrying no reservation at
//! all.
//!
//! Complementary to `reservation_purchase`, which asserts what a purchase
//! PERSISTS by reading each reservation back immediately: that round trip cannot
//! tell a read keyed by its path from one that reports whatever was stored last,
//! which is what the rows below do.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::RESERVATION_RESOURCE_TYPE_VALUES;
use service_com_amazonaws_medialive_medialive::state::not_found_message;
use smithy_server_runtime::MultiServiceRouter;

use crate::reservation_purchase::{
    describe, first_offering, purchase, reservation, RESERVATION_MEMBERS,
};
use crate::wire::{error_code, error_message, medialive, DEFAULT_REGION};

/// One reservation this suite purchased: the case naming it, the id the store
/// minted for it and the reservation the purchase answered with.
#[derive(Debug)]
struct Row {
    case: String,
    id: String,
    purchased: Value,
}

/// Several reservations held at once, differing along the CONFIGURATION reserved
/// (one offering per reservable resource type the model declares,
/// `smithy_query(query="shape", shape="ReservationResourceType")`), the `Count`
/// of resources, the `Name` the request chose and whether it carried `Tags` — so
/// no two rows report the same members and a read that answered from the wrong
/// record cannot pass.
async fn purchased_rows(router: &MultiServiceRouter) -> Vec<Row> {
    let counts = [1, 3, 12, 40];
    let mut rows: Vec<Row> = Vec::new();
    for (index, resource_type) in RESERVATION_RESOURCE_TYPE_VALUES.iter().copied().enumerate() {
        let count = counts[index % counts.len()];
        let tagged = index % 2 == 0;
        let case = format!("{resource_type} x {count}, tagged {tagged}");
        let offering_id =
            first_offering(router, &case, &format!("resourceType={resource_type}")).await;

        let mut body = Map::new();
        body.insert("count".to_string(), json!(count));
        body.insert("name".to_string(), json!(format!("{resource_type}-lease")));
        if tagged {
            body.insert("tags".to_string(), json!({"owner": format!("team-{index}")}));
        }

        let (status, answered) =
            purchase(router, DEFAULT_REGION, &offering_id, Value::Object(body)).await;
        assert_eq!(
            status, 201,
            "setup {case:?}: expected PurchaseOffering to answer 201, got {status}: {answered}"
        );
        let purchased = reservation(&case, &answered);
        let id = purchased["reservationId"]
            .as_str()
            .unwrap_or_else(|| panic!("setup {case:?}: expected a minted id, got {purchased}"))
            .to_string();
        rows.push(Row {
            case,
            id,
            purchased,
        });
    }
    rows
}

/// A read reports the reservation its path names: read back in the REVERSE of
/// the order they were purchased in, every row still reports its own stored
/// members — not a neighbour's and not the one stored last — and every member
/// the shape declares matches what the purchase answered with.
#[tokio::test]
async fn a_read_reports_the_reservation_its_path_names() {
    let router = medialive();
    let rows = purchased_rows(&router).await;
    assert!(
        rows.len() > 1,
        "setup: a keyed read needs more than one reservation held, got {}",
        rows.len()
    );

    let mut reports: Vec<(&Row, Value)> = Vec::new();
    for row in rows.iter().rev() {
        let case = &row.case;
        let (status, described) = describe(&router, DEFAULT_REGION, &row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeReservation {} to answer 200, got {status}: \
             {described}",
            row.id
        );
        assert_eq!(
            described["reservationId"],
            json!(row.id),
            "case {case:?}: expected the read to report the id its path named, got {:?}",
            described["reservationId"]
        );
        for member in RESERVATION_MEMBERS {
            // Every row of this table names its reservation and the store
            // reports tags as an empty map rather than nothing, so every member
            // the shape declares is reported — asserted on its own, because a
            // member missing from BOTH responses would compare equal below.
            assert!(
                described.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: expected the read to report {member}, got {described}"
            );
            assert_eq!(
                described.get(member),
                row.purchased.get(member),
                "case {case:?}: expected the read to report the stored {member}, got {:?} against \
                 {:?}",
                described.get(member),
                row.purchased.get(member)
            );
        }
        // Not a neighbour's record: the identifying members belong to this row
        // alone.
        for other in rows.iter().filter(|other| other.id != row.id) {
            for member in ["reservationId", "arn"] {
                assert_ne!(
                    described.get(member),
                    other.purchased.get(member),
                    "case {case:?}: expected the read not to report {member} of {:?}, got {:?}",
                    other.case,
                    described.get(member)
                );
            }
        }
        reports.push((row, described));
    }

    // Reporting writes nothing a later caller sees: after every row has been
    // read, each one reports exactly what it reported the first time.
    for (row, reported) in &reports {
        let case = &row.case;
        let (status, again) = describe(&router, DEFAULT_REGION, &row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated read to answer 200, got {status}: {again}"
        );
        assert_eq!(
            &again, reported,
            "case {case:?}: expected a repeated read to report the same reservation, got {again}"
        );
    }
}

/// An identifier the account holds no reservation for is the modelled 404,
/// naming the identifier the request asked for and reporting no reservation —
/// and the reservations the account DOES hold are untouched by the refusals.
///
/// The rows differ along how the identifier misses: one of the right shape that
/// was never minted, a held id with a digit appended, a held id with its last
/// digit dropped, and one that is no id at all.
#[tokio::test]
async fn an_id_the_account_does_not_hold_is_not_found() {
    let router = medialive();
    let rows = purchased_rows(&router).await;
    let held = rows[0].id.clone();
    assert!(
        !held.is_empty(),
        "setup: expected the store to mint a non-empty reservation id"
    );
    // The held id with its last character dropped, which names no reservation
    // because an id is minted whole.
    let mut shortened = held.clone();
    shortened.pop();

    let misses = [
        ("a seven-digit id never minted", "1000000".to_string()),
        ("a held id with a digit appended", format!("{held}0")),
        ("a held id with its last digit dropped", shortened),
        ("no id at all", "no-such-reservation".to_string()),
    ];
    for (case, id) in misses {
        let (status, body) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeReservation {id} to be refused as not found, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for {id}, got {body}"
        );
        // Two independent checks on the message: that it is COMPOSED from the
        // request, against the helper every refusal in this service words itself
        // with, and that the PHRASING is the one the refusal states, written out
        // here so a drift of that phrasing cannot hide behind the helper.
        assert_eq!(
            error_message(&body),
            not_found_message("Reservation", &id),
            "case {case:?}: expected the refusal to name the id the request asked for, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Reservation {id} not found"),
            "case {case:?}: expected the refusal's stated phrasing, got {:?}",
            error_message(&body)
        );
        for member in RESERVATION_MEMBERS {
            assert!(
                body.get(member).is_none(),
                "case {case:?}: expected a refusal to report no reservation, got {member} in \
                 {body}"
            );
        }
    }

    for row in &rows {
        let case = &row.case;
        let (status, described) = describe(&router, DEFAULT_REGION, &row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a held reservation to still read 200 after the refusals, got \
             {status}: {described}"
        );
        for member in RESERVATION_MEMBERS {
            assert_eq!(
                described.get(member),
                row.purchased.get(member),
                "case {case:?}: expected a refused read to leave the held {member} untouched, got \
                 {:?} against {:?}",
                described.get(member),
                row.purchased.get(member)
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `DeleteReservation`: which reservations a delete acts on, which it refuses on
//! their state, and what the account can still read and no longer change
//! afterwards — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The rules asserted here, each over a table that spans its class rather than
//! one request:
//!
//! 1. "Delete an expired reservation": a reservation whose lease has run out
//!    moves to the terminal `DELETED` state its own enum declares, every other
//!    member reading exactly as it did before the call, and the record STAYS in
//!    the account — `DescribeReservation` and `ListReservations` go on reporting
//!    it there, member for member what the delete answered. A repeat is
//!    idempotent: the state the caller asked for is already the stored one.
//! 2. A reservation whose lease is still running is not an expired one, so the
//!    delete is refused with the modelled 409 and writes nothing at all.
//! 3. A `ReservationId` naming no reservation this account holds leaves nothing
//!    to delete, which is the modelled 404.
//! 4. A deleted reservation is a tombstone: it stays READABLE, and every
//!    mutation of it — the reservation's own update and a tag write addressed to
//!    its ARN — is refused as not found.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::RESERVATION_RESOURCE_TYPE_VALUES;
use service_com_amazonaws_medialive_medialive::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::reservation_purchase::{describe, first_offering, purchase, RESERVATION_MEMBERS};
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive_with_engine, DEFAULT_REGION};

/// The collection a delete is addressed to
/// (`smithy_query(query="operation", operation="DeleteReservation")`: DELETE
/// /prod/reservations/{ReservationId}).
const RESERVATIONS: &str = "/prod/reservations";

/// The states a reservation reports in this suite
/// (`smithy_query(query="shape", shape="ReservationState")`: ACTIVE | EXPIRED |
/// CANCELED | DELETED).
const ACTIVE: &str = "ACTIVE";
const EXPIRED: &str = "EXPIRED";
const DELETED: &str = "DELETED";

/// A lease that has run out by a margin rather than by a second: each row below
/// is several dispatches, and a lease ending within them would move the state
/// mid-row and fail it for timing rather than for behaviour.
const AN_HOUR: Duration = Duration::from_secs(3_600);
const A_YEAR: Duration = Duration::from_secs(31_536_000);

/// One member of a reported reservation, read as a client reads it: a member the
/// response omits and one it reports as null are the same absence.
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// `DeleteReservation` for `reservation_id`.
async fn delete(router: &MultiServiceRouter, reservation_id: &str) -> (u16, Value) {
    let uri = format!("{RESERVATIONS}/{reservation_id}");
    call(router, "DELETE", &uri, None).await
}

/// `UpdateReservation` for `reservation_id` — the mutation rule 4 sends at a
/// tombstone.
async fn update(router: &MultiServiceRouter, reservation_id: &str, body: Value) -> (u16, Value) {
    let uri = format!("{RESERVATIONS}/{reservation_id}");
    call(router, "PUT", &uri, Some(body)).await
}

/// Every reservation the account reports, by id, through `ListReservations`.
async fn listed(router: &MultiServiceRouter, case: &str) -> HashMap<String, Value> {
    let (status, body) = call(router, "GET", RESERVATIONS, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListReservations to answer 200, got {status}: {body}"
    );
    body["reservations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|reported| {
            (
                reported["reservationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                reported,
            )
        })
        .collect()
}

/// Purchase one reservation against `offering_id` and answer what the purchase
/// reported — the baseline a delete is measured against.
async fn purchased(
    router: &MultiServiceRouter,
    case: &str,
    offering_id: &str,
    body: Value,
) -> Value {
    let (status, answered) = purchase(router, DEFAULT_REGION, offering_id, body).await;
    assert_eq!(
        status, 201,
        "setup {case:?}: expected PurchaseOffering to answer 201, got {status}: {answered}"
    );
    let reservation = answered
        .get("reservation")
        .cloned()
        .filter(|reservation| !reservation.is_null())
        .unwrap_or_else(|| {
            panic!("setup {case:?}: expected a purchase to report a reservation, got {answered}")
        });
    assert_eq!(
        member(&reservation, "state"),
        json!(ACTIVE),
        "setup {case:?}: expected a purchased reservation to start ACTIVE, got {:?}",
        member(&reservation, "state")
    );
    reservation
}

/// Move the stored lease of `id` to `end`, which is what the CLOCK does to a
/// reservation over its term: `End` is on the record and the reported `State` is
/// derived from it, so this drives the state through the one thing that owns it
/// rather than writing a state nothing computed.
///
/// Shared with [`crate::idempotency_token_after_terminal_delete`], whose rows
/// also have to reach the expired lease a delete acts on, so the two suites run
/// a reservation's clock the same way.
pub(crate) fn end_the_lease(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    id: &str,
    end: SystemTime,
) {
    let store = state::service_state_in(&**engine, DEFAULT_REGION).expect("the medialive store");
    let mut store = store.lock();
    let reservation = store
        .reservations
        .get_mut(id)
        .unwrap_or_else(|| panic!("case {case:?}: expected the stored reservation {id}"));
    reservation.end = end;
}

/// The ids of the first offering of EVERY reservable resource type the model
/// declares, so the rows below delete reservations bought against different
/// configurations.
async fn offering_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES {
        let case = format!("the first {resource_type} offering");
        ids.push(first_offering(router, &case, &format!("resourceType={resource_type}")).await);
    }
    ids
}

/// One row of the delete tables: what the purchase chose, so a reservation under
/// test differs along what a client can choose about it.
struct Row {
    /// How the row names itself in an assertion.
    case: &'static str,
    /// The `Name` the purchase carried, if any.
    name: Option<&'static str>,
    /// The `Count` of reserved resources.
    count: i64,
    /// The `RenewalSettings` the purchase carried, if any.
    renewal: Value,
    /// The `Tags` the purchase carried, if any.
    tags: Value,
}

/// The purchases every rule below is applied to: the rows differ along which
/// caller-chosen members the reservation carries (none, each, all), the count,
/// and the renewal settings — what a delete must leave untouched.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a reservation carrying nothing but a count",
            name: None,
            count: 1,
            renewal: json!(null),
            tags: json!(null),
        },
        Row {
            case: "a named reservation",
            name: Some("named-lease"),
            count: 3,
            renewal: json!(null),
            tags: json!(null),
        },
        Row {
            case: "a reservation with renewal settings",
            name: None,
            count: 2,
            renewal: json!({"automaticRenewal": "ENABLED", "renewalCount": 4}),
            tags: json!(null),
        },
        Row {
            case: "a tagged reservation with every setting",
            name: Some("réservation-ロング"),
            count: 5,
            renewal: json!({"automaticRenewal": "UNAVAILABLE"}),
            tags: json!({"owner": "video-team", "cost-centre": "42"}),
        },
    ]
}

/// The purchase body one row sends.
fn purchase_body(row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("count".to_string(), json!(row.count));
    if let Some(name) = row.name {
        body.insert("name".to_string(), json!(name));
    }
    if !row.renewal.is_null() {
        body.insert("renewalSettings".to_string(), row.renewal.clone());
    }
    if !row.tags.is_null() {
        body.insert("tags".to_string(), row.tags.clone());
    }
    Value::Object(body)
}

/// Rule 1: a delete of a reservation whose lease has run out moves it to the
/// terminal `DELETED` state, keeps it readable there, and is idempotent.
///
/// The rows differ along WHEN the lease ran out (at this instant, an hour ago, a
/// year ago), along the offering configuration the reservation was bought
/// against, and along which caller-chosen members it carries — so what the rule
/// generalises over is what varies.
#[tokio::test]
async fn a_delete_of_an_expired_reservation_moves_it_to_the_terminal_state() {
    let (router, engine) = medialive_with_engine();
    let offerings = offering_ids(&router).await;

    for (index, row) in rows().iter().enumerate() {
        let now = SystemTime::now();
        let ends = [
            ("whose lease ends at this instant", now),
            ("whose lease ended an hour ago", now - AN_HOUR),
            ("whose lease ended a year ago", now - A_YEAR),
        ];
        for (ended, end) in ends {
            let offering_id = &offerings[index % offerings.len()];
            let case = format!("{} {ended}, bought against {offering_id}", row.case);
            let bought = purchased(&router, &case, offering_id, purchase_body(row)).await;
            let id = bought["reservationId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a minted id, got {bought}"))
                .to_string();

            // The lease runs out: the state the clock owes, read back before the
            // delete, is the state the delete acts on.
            end_the_lease(&engine, &case, &id, end);
            let (status, expired) = describe(&router, DEFAULT_REGION, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the reservation to be readable before the delete, \
                 got {status}: {expired}"
            );
            assert_eq!(
                member(&expired, "state"),
                json!(EXPIRED),
                "case {case:?}: expected the expired lease to report {EXPIRED}, got {:?}",
                member(&expired, "state")
            );

            let (status, deleted) = delete(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteReservation to answer 200, got {status}: \
                 {deleted}"
            );
            assert_eq!(
                member(&deleted, "state"),
                json!(DELETED),
                "case {case:?}: expected the deleted reservation to report {DELETED}, got {:?}",
                member(&deleted, "state")
            );
            // Nothing but the state moved: the lease, the mint, the tags and
            // everything the offering describes read as they did a call ago.
            for reported in RESERVATION_MEMBERS {
                if reported == "state" {
                    continue;
                }
                assert_eq!(
                    member(&deleted, reported),
                    member(&expired, reported),
                    "case {case:?}: expected {reported} to be left as it was read before the \
                     delete ({:?}), got {:?}",
                    member(&expired, reported),
                    member(&deleted, reported)
                );
            }

            // The record STAYS in the account: the keyed reader reports the
            // reservation member for member what the delete answered.
            let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the deleted reservation to stay readable, got \
                 {status}: {described}"
            );
            for reported in RESERVATION_MEMBERS {
                assert_eq!(
                    member(&described, reported),
                    member(&deleted, reported),
                    "case {case:?}: expected the read back {reported} to be what the delete \
                     answered ({:?}), got {:?}",
                    member(&deleted, reported),
                    member(&described, reported)
                );
            }

            // And so does the listing, which answers from the same projection.
            let reservations = listed(&router, &case).await;
            let reported = reservations.get(&id).unwrap_or_else(|| {
                panic!("case {case:?}: expected the listing to still report {id}, got {reservations:?}")
            });
            assert_eq!(
                member(reported, "state"),
                json!(DELETED),
                "case {case:?}: expected the listing to report {id} as {DELETED}, got {:?}",
                member(reported, "state")
            );

            // Idempotent: the state the caller asked for is already the stored
            // one, so a repeat answers the same reservation.
            for attempt in 1..=2 {
                let (status, again) = delete(&router, &id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected repeated delete {attempt} to answer 200, got \
                     {status}: {again}"
                );
                assert_eq!(
                    again, deleted,
                    "case {case:?}: expected repeated delete {attempt} to answer the same \
                     reservation"
                );
            }
        }
    }
}

/// Rule 2: a reservation whose lease is still running is refused on its state,
/// with the modelled 409, and the refusal writes nothing.
///
/// The rows differ along how much of the lease is left (the term the purchase
/// bought, an hour, a minute) and along what the reservation carries, so the
/// rule is asserted over the class rather than over one lease.
#[tokio::test]
async fn a_delete_of_a_running_lease_is_refused_and_writes_nothing() {
    let (router, engine) = medialive_with_engine();
    let offerings = offering_ids(&router).await;

    for (index, row) in rows().iter().enumerate() {
        let now = SystemTime::now();
        let leases = [
            ("the whole term the purchase bought", None),
            ("an hour of lease left", Some(now + AN_HOUR)),
            ("a year of lease left", Some(now + A_YEAR)),
        ];
        for (left, end) in leases {
            let offering_id = &offerings[index % offerings.len()];
            let case = format!("{} with {left}, bought against {offering_id}", row.case);
            let bought = purchased(&router, &case, offering_id, purchase_body(row)).await;
            let id = bought["reservationId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a minted id, got {bought}"))
                .to_string();
            if let Some(end) = end {
                end_the_lease(&engine, &case, &id, end);
            }

            // Twice: a refusal is not a step towards a delete, so the second
            // answer is the first one.
            for attempt in 1..=2 {
                let (status, answered) = delete(&router, &id).await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected attempt {attempt} to be refused on the running \
                     lease, got {status}: {answered}"
                );
                assert_eq!(
                    error_code(status, &answered),
                    "ConflictException",
                    "case {case:?}: expected ConflictException on attempt {attempt}, got \
                     {answered}"
                );
                assert_eq!(
                    error_message(&answered),
                    format!("Reservation {id} is in state {ACTIVE} and can only be deleted from {EXPIRED}"),
                    "case {case:?}: expected the refusal to name the id, the state it is in \
                     and the state a delete acts from, got {answered}"
                );
                for reported in RESERVATION_MEMBERS {
                    assert!(
                        answered.get(reported).is_none(),
                        "case {case:?}: expected a refusal to report no reservation member, \
                         got {reported} in {answered}"
                    );
                }
            }

            // Nothing was written: the reservation reads exactly as the purchase
            // reported it, still ACTIVE.
            let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the reservation to still be readable, got {status}: \
                 {described}"
            );
            assert_eq!(
                member(&described, "state"),
                json!(ACTIVE),
                "case {case:?}: expected a refused delete to leave the reservation {ACTIVE}, \
                 got {:?}",
                member(&described, "state")
            );
            for reported in RESERVATION_MEMBERS {
                // `End` is the one member the row may have moved itself, and
                // every member derived from it is derived on every read.
                if reported == "end" {
                    continue;
                }
                assert_eq!(
                    member(&described, reported),
                    member(&bought, reported),
                    "case {case:?}: expected a refused delete to leave {reported} as the \
                     purchase reported it ({:?}), got {:?}",
                    member(&bought, reported),
                    member(&described, reported)
                );
            }

            // An update of it still works, which is what tells "still leased"
            // apart from "gone".
            let (status, updated) = update(&router, &id, json!({"name": "still-mine"})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a reservation a delete refused to stay writable, got \
                 {status}: {updated}"
            );
        }
    }
}

/// Rule 3: an id naming no reservation this account holds leaves nothing to
/// delete, which is the modelled 404.
#[tokio::test]
async fn a_delete_of_an_id_naming_no_reservation_is_not_found() {
    let (router, engine) = medialive_with_engine();
    let offering_id = first_offering(&router, "the first offering", "maxResults=1").await;
    let bought = purchased(
        &router,
        "the one reservation the account holds",
        &offering_id,
        json!({"count": 1}),
    )
    .await;
    let id = bought["reservationId"]
        .as_str()
        .expect("a minted reservation id")
        .to_string();

    for (case, unknown) in [
        ("no id at all", "not-a-reservation".to_string()),
        ("zero", "0".to_string()),
        ("an offering id", offering_id.clone()),
        ("one digit past the minted id", format!("{id}9")),
        ("the minted id with a digit dropped", id[..id.len() - 1].to_string()),
    ] {
        let (status, answered) = delete(&router, &unknown).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DeleteReservation {unknown} to be refused as not found, \
             got {status}: {answered}"
        );
        assert_eq!(
            error_code(status, &answered),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for reservation {unknown}, got \
             {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("Reservation {unknown} not found"),
            "case {case:?}: expected the refusal to name the id the request asked for, got \
             {answered}"
        );
        for reported in RESERVATION_MEMBERS {
            assert!(
                answered.get(reported).is_none(),
                "case {case:?}: expected a refusal to report no reservation member, got \
                 {reported} in {answered}"
            );
        }
    }

    // The one reservation the account does hold was not touched by any of them —
    // and once its lease has run out it is deletable, so the refusals above are
    // about the id and not about the account.
    let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
    assert_eq!(
        status, 200,
        "expected the held reservation to still be readable, got {status}: {described}"
    );
    assert_eq!(
        member(&described, "state"),
        json!(ACTIVE),
        "expected the held reservation to be untouched, got {:?}",
        member(&described, "state")
    );
    end_the_lease(&engine, "the held reservation", &id, SystemTime::now() - AN_HOUR);
    let (status, deleted) = delete(&router, &id).await;
    assert_eq!(
        status, 200,
        "expected the held reservation to be deletable once expired, got {status}: {deleted}"
    );
}

/// Rule 4: a deleted reservation is a tombstone — readable, never writable.
///
/// The rows differ along WHICH mutation is sent at it: the reservation's own
/// update (both settings it carries) and a tag write addressed to its ARN, which
/// is the other way a client changes a reservation. Each is refused and the
/// reservation goes on reading exactly as the delete left it.
#[tokio::test]
async fn a_deleted_reservation_is_readable_but_no_longer_writable() {
    let (router, engine) = medialive_with_engine();
    let offering_id = first_offering(&router, "the first offering", "maxResults=1").await;
    let bought = purchased(
        &router,
        "the reservation every mutation is tried against",
        &offering_id,
        json!({
            "count": 2,
            "name": "retired-lease",
            "renewalSettings": {"automaticRenewal": "ENABLED", "renewalCount": 3},
            "tags": {"owner": "video-team"},
        }),
    )
    .await;
    let id = bought["reservationId"]
        .as_str()
        .expect("a minted reservation id")
        .to_string();
    let arn = bought["arn"].as_str().expect("a minted ARN").to_string();

    end_the_lease(&engine, "the tombstone", &id, SystemTime::now() - AN_HOUR);
    let (status, deleted) = delete(&router, &id).await;
    assert_eq!(
        status, 200,
        "setup: expected the expired reservation to be deletable, got {status}: {deleted}"
    );

    // The reservation's own mutation: every body `UpdateReservation` accepts,
    // refused on the id rather than on the body.
    for (case, body) in [
        ("a rename", json!({"name": "renamed-after-delete"})),
        (
            "new renewal settings",
            json!({"renewalSettings": {"automaticRenewal": "DISABLED"}}),
        ),
        (
            "both settings",
            json!({"name": "both", "renewalSettings": {"renewalCount": 9}}),
        ),
        ("neither setting", json!({})),
    ] {
        let (status, answered) = update(&router, &id, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {body} at a deleted reservation to be refused, got \
             {status}: {answered}"
        );
        assert_eq!(
            error_code(status, &answered),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for the tombstone, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("Reservation {id} already deleted"),
            "case {case:?}: expected the refusal to say the reservation was deleted, got \
             {answered}"
        );
    }

    // The other way a client changes a reservation: a tag write addressed to its
    // ARN, which no longer resolves to a resource a caller may change.
    let tags_uri = format!("/prod/tags/{}", percent_encoded(&arn));
    for (case, method, body) in [
        (
            "a tag write",
            "POST",
            Some(json!({"tags": {"owner": "someone-else"}})),
        ),
        ("a tag removal", "DELETE", None),
    ] {
        let uri = if method == "DELETE" {
            format!("{tags_uri}?tagKeys=owner")
        } else {
            tags_uri.clone()
        };
        let (status, answered) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {method} {uri} at a deleted reservation to be refused, \
             got {status}: {answered}"
        );
        assert_eq!(
            error_code(status, &answered),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for the tombstone's ARN, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("Resource {arn} not found"),
            "case {case:?}: expected the refusal to name the ARN the request asked for, got \
             {answered}"
        );
    }

    // Readable throughout: the reservation, and the tags it was purchased with,
    // read exactly as the delete left them.
    let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
    assert_eq!(
        status, 200,
        "expected the deleted reservation to stay readable, got {status}: {described}"
    );
    for reported in RESERVATION_MEMBERS {
        assert_eq!(
            member(&described, reported),
            member(&deleted, reported),
            "expected the refused mutations to leave {reported} as the delete answered it \
             ({:?}), got {:?}",
            member(&deleted, reported),
            member(&described, reported)
        );
    }
    let (status, tags) = call(&router, "GET", &tags_uri, None).await;
    assert_eq!(
        status, 200,
        "expected the deleted reservation's tags to stay readable, got {status}: {tags}"
    );
    assert_eq!(
        tags["tags"],
        json!({"owner": "video-team"}),
        "expected the tags the purchase carried, got {tags}"
    );
}

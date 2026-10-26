// SPDX-License-Identifier: CC0-1.0

//! `UpdateReservation`: which settings of a purchased reservation an update
//! changes, what it leaves exactly as it was, and which requests it refuses —
//! end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted
//! is the emulator's wire behaviour and not an internal function's return value.
//!
//! The rules asserted here, each over a table that spans its class rather than
//! one request:
//!
//! 1. An update replaces exactly the settings the request CARRIES (`Name`,
//!    `RenewalSettings`) and leaves every setting it omits as it was; every
//!    member a reservation derives from the offering it was bought from, from
//!    the mint and from the clock is untouched; and the response is the
//!    reservation read back out of state, so `DescribeReservation` reports
//!    exactly what the update answered with and a repeat of the same update
//!    answers the same reservation.
//! 2. The caller-chosen `Name` is held to this service's length bounds and
//!    `RenewalSettings$RenewalCount` to the model's own floor of one; each is
//!    refused with the modelled error for it and changes nothing.
//! 3. A `ReservationId` naming no reservation this account holds leaves nothing
//!    to change, which is the modelled 404.

use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::{
    RESERVATION_AUTOMATIC_RENEWAL_VALUES, RESERVATION_RESOURCE_TYPE_VALUES,
};
use service_com_amazonaws_medialive_medialive::state::MAX_RESOURCE_NAME_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, medialive};

/// The collection a purchase is addressed to
/// (`smithy_query(query="operation", operation="PurchaseOffering")`: POST
/// /prod/offerings/{OfferingId}/purchase), and the one an update is addressed to
/// (`smithy_query(query="operation", operation="UpdateReservation")`: PUT
/// /prod/reservations/{ReservationId}).
const OFFERINGS: &str = "/prod/offerings";
const RESERVATIONS: &str = "/prod/reservations";

/// Every member `Reservation` declares that `UpdateReservation` carries NO input
/// for (`smithy_query(query="shape", shape="Reservation")` minus the `Name` and
/// `RenewalSettings` its input names), so each must read the same before and
/// after an update.
const UNSETTABLE_MEMBERS: [&str; 17] = [
    "arn",
    "count",
    "currencyCode",
    "duration",
    "durationUnits",
    "end",
    "fixedPrice",
    "offeringDescription",
    "offeringId",
    "offeringType",
    "region",
    "reservationId",
    "resourceSpecification",
    "start",
    "state",
    "tags",
    "usagePrice",
];

/// Every member `Reservation` declares, which is member for member what
/// `DescribeReservationResponse` reports flat.
const RESERVATION_MEMBERS: [&str; 19] = [
    "arn",
    "count",
    "currencyCode",
    "duration",
    "durationUnits",
    "end",
    "fixedPrice",
    "name",
    "offeringDescription",
    "offeringId",
    "offeringType",
    "region",
    "renewalSettings",
    "reservationId",
    "resourceSpecification",
    "start",
    "state",
    "tags",
    "usagePrice",
];

/// The automatic-renewal status a request that named none reports
/// (`smithy_query(query="shape", shape="ReservationAutomaticRenewal")`:
/// DISABLED | ENABLED | UNAVAILABLE).
const DISABLED: &str = "DISABLED";

/// One member of a reported reservation, read as a client reads it: a member the
/// response omits and one it reports as null are the same absence.
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// The id of the first offering `filters` admits, read off `ListOfferings` — so
/// every row of the tables below buys a configuration the catalog really holds.
async fn first_offering(router: &MultiServiceRouter, case: &str, filters: &str) -> String {
    let uri = format!("{OFFERINGS}?{filters}&maxResults=1");
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected ListOfferings to answer 200, got {status}: {body}"
    );
    body["offerings"][0]["offeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup {case:?}: expected an offering under {filters}, got {body}"))
        .to_string()
}

/// The first offering of EVERY reservable resource type the model declares, so
/// the rows below update reservations bought against different configurations.
async fn offering_ids(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES {
        let case = format!("the first {resource_type} offering");
        ids.push(first_offering(router, &case, &format!("resourceType={resource_type}")).await);
    }
    ids
}

/// Purchase one reservation against `offering_id` and answer the reservation the
/// purchase reports — the baseline an update is measured against.
async fn purchase(
    router: &MultiServiceRouter,
    case: &str,
    offering_id: &str,
    body: Value,
) -> Value {
    let uri = format!("{OFFERINGS}/{offering_id}/purchase");
    let (status, answered) = call(router, "POST", &uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "setup {case:?}: expected PurchaseOffering to answer 201, got {status}: {answered}"
    );
    answered
        .get("reservation")
        .cloned()
        .filter(|reservation| !reservation.is_null())
        .unwrap_or_else(|| {
            panic!("setup {case:?}: expected a purchase to report a reservation, got {answered}")
        })
}

/// `UpdateReservation` for `reservation_id`.
async fn update(
    router: &MultiServiceRouter,
    reservation_id: &str,
    body: Value,
) -> (u16, Value) {
    let uri = format!("{RESERVATIONS}/{reservation_id}");
    call(router, "PUT", &uri, Some(body)).await
}

/// `DescribeReservation` for `reservation_id`.
async fn describe(router: &MultiServiceRouter, reservation_id: &str) -> (u16, Value) {
    let uri = format!("{RESERVATIONS}/{reservation_id}");
    call(router, "GET", &uri, None).await
}

/// The reservation an update answered with.
fn updated(case: &str, body: &Value) -> Value {
    body.get("reservation")
        .cloned()
        .filter(|reservation| !reservation.is_null())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an update to report a reservation, got {body}")
        })
}

/// The renewal settings a reservation reports once a request asked for `asked`:
/// what the request carried, with the automatic-renewal status filled in when it
/// named none — asking for no automatic renewal is not renewing, which is the
/// `DISABLED` the status enum declares.
fn normalised_renewal(asked: &Value) -> Value {
    let mut expected: Map<String, Value> = asked.as_object().cloned().unwrap_or_default();
    expected
        .entry("automaticRenewal".to_string())
        .or_insert_with(|| json!(DISABLED));
    Value::Object(expected)
}

/// Rule 1: an update replaces exactly the settings it carries, leaves the rest
/// of the reservation alone, and reports what was written.
///
/// The rows differ along WHICH settings the request carries (neither, one, the
/// other, both), along the form each carried setting takes (a short name, a name
/// at the upper bound, a multibyte name; renewal settings with and without a
/// count, with each status the enum declares, and an empty structure), and along
/// the reservation being updated (a different offering configuration, and a
/// baseline that did or did not name a reservation and did or did not ask for
/// renewal) — so what the rule generalises over is what varies.
#[tokio::test]
async fn an_update_replaces_only_the_settings_it_carries() {
    let router = medialive();
    let offerings = offering_ids(&router).await;
    let name_at_bound = "r".repeat(MAX_RESOURCE_NAME_LEN);

    let mut rows: Vec<(String, Value)> = vec![
        ("carries neither setting".to_string(), json!({})),
        (
            "carries a name only".to_string(),
            json!({"name": "renamed-lease"}),
        ),
        (
            format!("carries a name of {MAX_RESOURCE_NAME_LEN} characters"),
            json!({"name": name_at_bound}),
        ),
        (
            "carries a name of multibyte characters".to_string(),
            json!({"name": "réservation-ロング"}),
        ),
        (
            "carries renewal settings with a count".to_string(),
            json!({"renewalSettings": {"automaticRenewal": DISABLED, "renewalCount": 1}}),
        ),
        (
            "carries renewal settings naming no status".to_string(),
            json!({"renewalSettings": {"renewalCount": 4}}),
        ),
        (
            "carries empty renewal settings".to_string(),
            json!({"renewalSettings": {}}),
        ),
        (
            "carries both settings".to_string(),
            json!({"name": "both-changed", "renewalSettings": {"automaticRenewal": "ENABLED", "renewalCount": 12}}),
        ),
    ];
    // One row per status the enum declares, so the table spans that value set
    // rather than the one status a recorded request happened to carry.
    for status in RESERVATION_AUTOMATIC_RENEWAL_VALUES {
        rows.push((
            format!("carries automaticRenewal {status}"),
            json!({"renewalSettings": {"automaticRenewal": status}}),
        ));
    }

    // The reservations the rows are applied to: what the PURCHASE chose, so a
    // row's expectation for an omitted setting is a value the update did not
    // supply.
    let baselines = [
        (
            Some("first-lease"),
            json!({"automaticRenewal": "ENABLED", "renewalCount": 7}),
        ),
        (None, json!(null)),
        (Some("third-lease"), json!({"renewalCount": 2})),
    ];

    for (index, (row, body)) in rows.iter().enumerate() {
        let (baseline_name, baseline_renewal) = &baselines[index % baselines.len()];
        let offering_id = &offerings[index % offerings.len()];
        let count = 1 + i64::try_from(index % 5).expect("a small count");
        let case = format!(
            "{row}, over a reservation named {baseline_name:?} with renewal \
             {baseline_renewal} against offering {offering_id}"
        );

        let mut purchase_body = Map::new();
        purchase_body.insert("count".to_string(), json!(count));
        purchase_body.insert("tags".to_string(), json!({"owner": "video-team"}));
        if let Some(name) = baseline_name {
            purchase_body.insert("name".to_string(), json!(name));
        }
        if !baseline_renewal.is_null() {
            purchase_body.insert("renewalSettings".to_string(), baseline_renewal.clone());
        }
        let purchased =
            purchase(&router, &case, offering_id, Value::Object(purchase_body)).await;
        let id = purchased["reservationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a minted id, got {purchased}"))
            .to_string();

        let (status, answered) = update(&router, &id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateReservation to answer 200, got {status}: {answered}"
        );
        let changed = updated(&case, &answered);

        // A setting the request carried replaces the stored one; a setting it
        // omitted reads as the purchase left it.
        let expected_name = match body.get("name") {
            Some(name) => name.clone(),
            None => baseline_name.map_or(Value::Null, |name| json!(name)),
        };
        assert_eq!(
            member(&changed, "name"),
            expected_name,
            "case {case:?}: expected the name {expected_name} after the update, got {:?}",
            member(&changed, "name")
        );
        let expected_renewal = match body.get("renewalSettings") {
            Some(asked) => normalised_renewal(asked),
            None => normalised_renewal(baseline_renewal),
        };
        assert_eq!(
            member(&changed, "renewalSettings"),
            expected_renewal,
            "case {case:?}: expected the renewal settings {expected_renewal} after the \
             update, got {:?}",
            member(&changed, "renewalSettings")
        );

        // Nothing the update carries no input for moved: what the offering
        // describes, what the mint assigned, the lease and its state all read
        // exactly as the purchase reported them.
        for unsettable in UNSETTABLE_MEMBERS {
            assert_eq!(
                member(&changed, unsettable),
                member(&purchased, unsettable),
                "case {case:?}: expected {unsettable} to be left as the purchase reported \
                 it ({:?}), got {:?}",
                member(&purchased, unsettable),
                member(&changed, unsettable)
            );
        }

        // The response is the STORED reservation: the reader reports it member
        // for member.
        let (read_status, described) = describe(&router, &id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected DescribeReservation to answer 200, got \
             {read_status}: {described}"
        );
        for reported in RESERVATION_MEMBERS {
            assert_eq!(
                member(&described, reported),
                member(&changed, reported),
                "case {case:?}: expected the read back {reported} to be what the update \
                 answered ({:?}), got {:?}",
                member(&changed, reported),
                member(&described, reported)
            );
        }

        // The same update again writes the same settings, so it answers the same
        // reservation.
        let (repeat_status, repeated) = update(&router, &id, body.clone()).await;
        assert_eq!(
            repeat_status, 200,
            "case {case:?}: expected a repeated update to answer 200, got \
             {repeat_status}: {repeated}"
        );
        assert_eq!(
            updated(&case, &repeated),
            changed,
            "case {case:?}: expected a repeated update to answer the same reservation"
        );
    }
}

/// Rule 2: the caller-chosen settings are held to their bounds, and a refused
/// update changes nothing.
#[tokio::test]
async fn an_update_outside_the_bounds_is_refused_and_writes_nothing() {
    let router = medialive();
    let offering_id = first_offering(&router, "the first offering", "maxResults=1").await;
    let purchased = purchase(
        &router,
        "the reservation every refusal is tried against",
        &offering_id,
        json!({
            "count": 2,
            "name": "untouched-lease",
            "renewalSettings": {"automaticRenewal": "ENABLED", "renewalCount": 3},
            "tags": {"owner": "video-team"},
        }),
    )
    .await;
    let id = purchased["reservationId"]
        .as_str()
        .expect("a minted reservation id")
        .to_string();

    let past_the_bound = "x".repeat(MAX_RESOURCE_NAME_LEN + 1);
    let cases = [
        // `Name` targets bare `__string`, so these two are the handler's own
        // refusal rather than the request boundary's.
        ("an empty name", json!({"name": ""}), "BadRequestException"),
        (
            "a name one character past the bound",
            json!({"name": past_the_bound}),
            "BadRequestException",
        ),
        // `RenewalSettings$RenewalCount` targets `__integerMin1`: fewer than one
        // renewal is nothing to renew.
        (
            "a renewal count below the minimum",
            json!({"renewalSettings": {"renewalCount": 0}}),
            "BadRequestException",
        ),
        (
            "a negative renewal count",
            json!({"renewalSettings": {"renewalCount": -5}}),
            "BadRequestException",
        ),
        // `RenewalSettings$AutomaticRenewal` targets an enum, whose value set is
        // enforced at the request boundary — so a status the enum does not
        // declare never reaches the handler and is refused there.
        (
            "an automatic-renewal status the enum does not declare",
            json!({"renewalSettings": {"automaticRenewal": "SOMETIMES"}}),
            "ValidationException",
        ),
        (
            "a lowercase spelling of a declared status",
            json!({"renewalSettings": {"automaticRenewal": "enabled"}}),
            "ValidationException",
        ),
    ];

    for (case, body, expected_code) in cases {
        let (status, answered) = update(&router, &id, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {body} to be refused, got {status}: {answered}"
        );
        assert_eq!(
            error_code(status, &answered),
            expected_code,
            "case {case:?}: expected {expected_code} for {body}, got {answered}"
        );
        assert!(
            answered.get("reservation").is_none(),
            "case {case:?}: expected a refusal to report no reservation, got {answered}"
        );

        let (read_status, described) = describe(&router, &id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected the reservation to still be readable, got \
             {read_status}: {described}"
        );
        for reported in RESERVATION_MEMBERS {
            assert_eq!(
                member(&described, reported),
                member(&purchased, reported),
                "case {case:?}: expected a refused update to leave {reported} as the \
                 purchase reported it ({:?}), got {:?}",
                member(&purchased, reported),
                member(&described, reported)
            );
        }
    }
}

/// Rule 3: an id naming no reservation this account holds leaves nothing to
/// change, which is the modelled 404.
#[tokio::test]
async fn an_id_naming_no_reservation_is_not_found() {
    let router = medialive();
    let offering_id = first_offering(&router, "the first offering", "maxResults=1").await;
    let purchased = purchase(
        &router,
        "the one reservation the account holds",
        &offering_id,
        json!({"count": 1}),
    )
    .await;
    let id = purchased["reservationId"]
        .as_str()
        .expect("a minted reservation id")
        .to_string();

    for (case, unknown) in [
        ("no id at all", "not-a-reservation".to_string()),
        ("zero", "0".to_string()),
        ("an offering id", offering_id.clone()),
        ("one digit past the minted id", format!("{id}9")),
    ] {
        let (status, answered) = update(&router, &unknown, json!({"name": "renamed"})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UpdateReservation {unknown} to be refused as not \
             found, got {status}: {answered}"
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
                 {answered}"
            );
        }
    }

    // The reservation the account does hold was not touched by any of them.
    let (read_status, described) = describe(&router, &id).await;
    assert_eq!(
        read_status, 200,
        "expected the held reservation to still be readable, got {read_status}: {described}"
    );
    for reported in RESERVATION_MEMBERS {
        assert_eq!(
            member(&described, reported),
            member(&purchased, reported),
            "expected {reported} to be left as the purchase reported it ({:?}), got {:?}",
            member(&purchased, reported),
            member(&described, reported)
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `PurchaseOffering` and `DescribeReservation`: what buying an offering
//! persists, what the reservation it creates reports back, and which requests it
//! refuses — end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The rules asserted here, each over a table that spans its class rather than
//! one request:
//!
//! 1. A purchase creates ONE reservation against the offering its `OfferingId`
//!    names: the members describing what is reserved (the resource
//!    specification, the term, the currency, the prices and the offering
//!    description) are the ones `DescribeOffering` reports for that same id, the
//!    members the request chose (`Count`, `Name`, `RenewalSettings`, `Tags`) are
//!    stored verbatim, the id and the ARN are minted, the lease runs from `Start`
//!    for the offering's `Duration` of `DurationUnits`, and
//!    `DescribeReservation` reports exactly what the purchase answered with.
//! 2. `Start` is admitted only "between the first day of the current month and
//!    one year from now", defaults to now when absent, and anything else is the
//!    operation's modelled 400.
//! 3. A retried purchase (the same `@idempotencyToken` `RequestId`) answers with
//!    the reservation the first one created rather than buying a second.
//! 4. An id naming no offering, and one naming no reservation, are the modelled
//!    404s.
//! 5. A reservation is `ACTIVE` until its `End` and `EXPIRED` from then on — the
//!    clock's own transition, which reporting it never writes — and a read
//!    changes nothing a later caller sees.
//! 6. The caller-chosen strings are held to this service's length bounds and the
//!    store is held to its own ceiling, each refused with the modelled error for
//!    it rather than stored.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Datelike, Months, NaiveDateTime, Timelike, Utc};
use serde_json::{json, Map, Value};
use service_com_amazonaws_medialive_medialive::ops::types::{
    RESERVATION_AUTOMATIC_RENEWAL_VALUES, RESERVATION_RESOURCE_TYPE_VALUES,
    RESERVATION_SPECIAL_FEATURE_VALUES,
};
use service_com_amazonaws_medialive_medialive::state;
use service_com_amazonaws_medialive_medialive::state::{
    MAX_REQUEST_ID_LEN, MAX_RESERVATIONS, MAX_RESOURCE_NAME_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::percent_encoded;
use crate::wire::{
    call, call_in_region, error_code, error_message, medialive, medialive_with_engine, ACCOUNT,
    DEFAULT_REGION,
};

/// The collection a purchase is addressed to
/// (`smithy_query(query="operation", operation="PurchaseOffering")`: POST
/// /prod/offerings/{OfferingId}/purchase), and the one a reservation is read from
/// (`DescribeReservation`: GET /prod/reservations/{ReservationId}).
const OFFERINGS: &str = "/prod/offerings";
const RESERVATIONS: &str = "/prod/reservations";

/// Every member `Reservation` declares, by its wire name
/// (`smithy_query(query="shape", shape="Reservation")`), which is member for
/// member what `DescribeReservationResponse` reports flat.
pub(crate) const RESERVATION_MEMBERS: [&str; 19] = [
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

/// The members a reservation reports about the OFFERING it was bought from,
/// spelled identically on `Offering` and on `Reservation` (both shapes carry
/// them; `smithy_query(query="shape", shape="Offering,Reservation")`), so each
/// one is compared against what `DescribeOffering` reports for the same id.
const OFFERING_DERIVED_MEMBERS: [&str; 10] = [
    "currencyCode",
    "duration",
    "durationUnits",
    "fixedPrice",
    "offeringDescription",
    "offeringId",
    "offeringType",
    "region",
    "resourceSpecification",
    "usagePrice",
];

/// The form `Start` and `End` are documented in ("Reservation UTC start date and
/// time in ISO-8601 format, e.g. '2018-03-01T00:00:00'").
const RESERVATION_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// The state a reservation whose lease is still running reports
/// (`smithy_query(query="shape", shape="ReservationState")`: ACTIVE | EXPIRED |
/// CANCELED | DELETED).
const ACTIVE: &str = "ACTIVE";
const EXPIRED: &str = "EXPIRED";

/// The automatic-renewal status a purchase that named none reports
/// (`ReservationAutomaticRenewal`: DISABLED | ENABLED | UNAVAILABLE).
const DISABLED: &str = "DISABLED";
const ENABLED: &str = "ENABLED";

/// `PurchaseOffering` for `offering_id`, signed for `region`.
///
/// Shared with [`crate::describe_reservation_reporting`], so the two reservation
/// suites cannot disagree about how a purchase reaches the emulator.
pub(crate) async fn purchase(
    router: &MultiServiceRouter,
    region: &str,
    offering_id: &str,
    body: Value,
) -> (u16, Value) {
    let uri = format!("{OFFERINGS}/{offering_id}/purchase");
    call_in_region(router, region, "POST", &uri, Some(body)).await
}

/// `DescribeReservation` for `reservation_id`, signed for `region`. Shared with
/// [`crate::describe_reservation_reporting`] for the same reason as
/// [`purchase`].
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    reservation_id: &str,
) -> (u16, Value) {
    let uri = format!("{RESERVATIONS}/{reservation_id}");
    call_in_region(router, region, "GET", &uri, None).await
}

/// `DescribeOffering` for `offering_id` — the reader this suite re-derives every
/// offering-described member of a reservation from.
async fn describe_offering(router: &MultiServiceRouter, region: &str, offering_id: &str) -> Value {
    let uri = format!("{OFFERINGS}/{offering_id}");
    let (status, body) = call_in_region(router, region, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "setup: expected DescribeOffering {offering_id} to answer 200, got {status}: {body}"
    );
    body
}

/// The id of the first offering `filters` admits, read off `ListOfferings` — so
/// every row of the tables below names a configuration the catalog really holds.
pub(crate) async fn first_offering(
    router: &MultiServiceRouter,
    case: &str,
    filters: &str,
) -> String {
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

/// The offerings this suite buys against: the first of EVERY reservable resource
/// type the model declares, plus one carrying a `SpecialFeature` (which only a
/// CHANNEL offering does) — so the rows differ along the configuration being
/// reserved, which is what an offering's own members are derived from.
async fn offering_rows(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES {
        let case = format!("the first {resource_type} offering");
        let id = first_offering(router, &case, &format!("resourceType={resource_type}")).await;
        rows.push((case, id));
    }
    let special_feature = RESERVATION_SPECIAL_FEATURE_VALUES
        .first()
        .expect("the model declares at least one SpecialFeature");
    let case = format!("the first {special_feature} offering");
    let id = first_offering(router, &case, &format!("specialFeature={special_feature}")).await;
    rows.push((case, id));
    rows
}

/// One reservation time, parsed back out of the form it is reported in.
fn instant(case: &str, raw: &Value) -> DateTime<Utc> {
    let text = raw
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a reservation time, got {raw}"));
    NaiveDateTime::parse_from_str(text, RESERVATION_TIME_FORMAT)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected {text:?} in the documented ISO-8601 form: {error}")
        })
        .and_utc()
}

/// A time as a request spells it.
fn spelled(at: DateTime<Utc>) -> String {
    at.format(RESERVATION_TIME_FORMAT).to_string()
}

/// Midnight on the first day of the month `at` falls in — the earliest `Start`
/// the member admits.
fn first_of_month(at: DateTime<Utc>) -> DateTime<Utc> {
    at.date_naive()
        .with_day(1)
        .and_then(|first| first.and_hms_opt(0, 0, 0))
        .expect("the first of a month is a date")
        .and_utc()
}

/// `at` plus `months` calendar months — the arithmetic both the window's far end
/// ("one year from now") and a lease's `End` (the offering's term) are measured
/// in.
fn months_on(at: DateTime<Utc>, months: u32) -> DateTime<Utc> {
    at.checked_add_months(Months::new(months))
        .expect("a date a year on")
}

/// The reservation a purchase answered with.
pub(crate) fn reservation(case: &str, body: &Value) -> Value {
    body.get("reservation")
        .cloned()
        .filter(|reservation| !reservation.is_null())
        .unwrap_or_else(|| panic!("case {case:?}: expected a purchase to report a reservation, got {body}"))
}

/// Rule 1: a purchase creates one reservation against the offering its
/// `OfferingId` names — every member describing what is reserved is the one
/// `DescribeOffering` reports for that id, every member the request chose is
/// stored as sent, the id and ARN are minted, the lease runs for the offering's
/// own term, and `DescribeReservation` reports exactly that reservation back.
///
/// The rows differ along the CONFIGURATION bought (one per reservable resource
/// type, plus a special-feature offering), the COUNT of resources, whether the
/// request named the reservation, what renewal settings it asked for and what
/// tags it carried — the inputs a purchase is made of; the recorded case is one
/// row of them.
#[tokio::test]
async fn a_purchase_reserves_the_offering_its_id_names() {
    let router = medialive();
    let rows = offering_rows(&router).await;
    let counts = [1, 2, 25];
    let renewals = [
        json!(null),
        json!({"automaticRenewal": ENABLED, "renewalCount": 2}),
        json!({"automaticRenewal": DISABLED}),
        json!({"renewalCount": 3}),
    ];

    let mut seen_ids: Vec<String> = Vec::new();
    for (index, (configuration, offering_id)) in rows.iter().enumerate() {
        let count = counts[index % counts.len()];
        let renewal = renewals[index % renewals.len()].clone();
        let named = index % 2 == 0;
        let tagged = index % 3 != 0;
        let case = format!(
            "{configuration}, count {count}, renewal {renewal}, named {named}, tagged {tagged}"
        );

        let mut body = Map::new();
        body.insert("count".to_string(), json!(count));
        if named {
            body.insert("name".to_string(), json!(format!("lease-{index}")));
        }
        if tagged {
            body.insert(
                "tags".to_string(),
                json!({"owner": format!("team-{index}"), "tier": "gold"}),
            );
        }
        if !renewal.is_null() {
            body.insert("renewalSettings".to_string(), renewal.clone());
        }

        let (status, answered) =
            purchase(&router, DEFAULT_REGION, offering_id, Value::Object(body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected PurchaseOffering to answer 201, got {status}: {answered}"
        );
        let purchased = reservation(&case, &answered);

        // Every member the shape declares is reported, bar the one the request
        // did not supply.
        for member in RESERVATION_MEMBERS {
            if member == "name" && !named {
                assert!(
                    purchased.get(member).is_none_or(Value::is_null),
                    "case {case:?}: expected no name on a purchase that named none, got {purchased}"
                );
                continue;
            }
            assert!(
                purchased.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: expected the reservation to report {member}, got {purchased}"
            );
        }

        // What is reserved comes from the offering, through the same reader
        // `DescribeOffering` answers from.
        let offering = describe_offering(&router, DEFAULT_REGION, offering_id).await;
        for member in OFFERING_DERIVED_MEMBERS {
            assert_eq!(
                purchased.get(member),
                offering.get(member),
                "case {case:?}: expected {member} to report what the offering reports, got {:?} \
                 against {:?}",
                purchased.get(member),
                offering.get(member)
            );
        }

        // What the request chose is stored as sent.
        assert_eq!(
            purchased["count"],
            json!(count),
            "case {case:?}: expected the reserved count the request asked for, got {:?}",
            purchased["count"]
        );
        if named {
            assert_eq!(
                purchased["name"],
                json!(format!("lease-{index}")),
                "case {case:?}: expected the name the request supplied, got {:?}",
                purchased["name"]
            );
        }
        let expected_tags = if tagged {
            json!({"owner": format!("team-{index}"), "tier": "gold"})
        } else {
            json!({})
        };
        assert_eq!(
            purchased["tags"], expected_tags,
            "case {case:?}: expected the tags the request carried, got {:?}",
            purchased["tags"]
        );
        // A purchase that asked for no automatic renewal does not renew, which
        // is the DISABLED its enum declares; a count it named is kept as named.
        let expected_renewal = match renewal.as_object() {
            None => json!({"automaticRenewal": DISABLED}),
            Some(asked) => {
                let mut expected = asked.clone();
                expected
                    .entry("automaticRenewal".to_string())
                    .or_insert_with(|| json!(DISABLED));
                Value::Object(expected)
            }
        };
        assert_eq!(
            purchased["renewalSettings"], expected_renewal,
            "case {case:?}: expected the renewal settings the purchase asked for, got {:?}",
            purchased["renewalSettings"]
        );

        // The id and the ARN are the store's, and the id is unique.
        let id = purchased["reservationId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            id.len() == 7 && id.chars().all(char::is_numeric),
            "case {case:?}: expected a seven-digit reservation id, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: expected a purchase to mint a new reservation id, got {id:?} again \
             after {seen_ids:?}"
        );
        seen_ids.push(id.clone());
        assert_eq!(
            purchased["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:reservation:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got \
             {:?}",
            purchased["arn"]
        );

        // The lease: ACTIVE, starting now, running for the offering's own term.
        assert_eq!(
            purchased["state"],
            json!(ACTIVE),
            "case {case:?}: expected a purchased reservation to be ACTIVE, got {:?}",
            purchased["state"]
        );
        let start = instant(&case, &purchased["start"]);
        let end = instant(&case, &purchased["end"]);
        let term = purchased["duration"].as_i64().unwrap_or_default();
        let expected_end = months_on(start, u32::try_from(term).unwrap_or_default());
        assert_eq!(
            end, expected_end,
            "case {case:?}: expected the lease to end {term} months after it starts, got \
             {end} against {expected_end}"
        );

        // The round trip: the read reports exactly the reservation the purchase
        // answered with.
        let (status, described) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeReservation {id} to answer 200, got {status}: \
             {described}"
        );
        for member in RESERVATION_MEMBERS {
            assert_eq!(
                described.get(member),
                purchased.get(member),
                "case {case:?}: expected the read to report the purchased {member}, got {:?} \
                 against {:?}",
                described.get(member),
                purchased.get(member)
            );
        }
    }

    // `Count` is "Number of resources", modelled `__integerMin1`: the published
    // bound is served and anything under it is refused, with nothing purchased.
    let offering_id = &rows[0].1;
    for (case, count) in [("the published minimum", 1), ("two resources", 2)] {
        let (status, answered) =
            purchase(&router, DEFAULT_REGION, offering_id, json!({"count": count})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a count of {count} to be served, got {status}: {answered}"
        );
        assert_eq!(
            reservation(case, &answered)["count"],
            json!(count),
            "case {case:?}: expected the reserved count the request asked for"
        );
    }
    for (case, count) in [("one below the minimum", 0), ("a negative count", -1)] {
        let (status, body) =
            purchase(&router, DEFAULT_REGION, offering_id, json!({"count": count})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a count of {count} to be refused, got {status}: {body}"
        );
        let code = error_code(status, &body);
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled client error for a count of {count}, got {body}"
        );
        assert!(
            body.get("reservation").is_none(),
            "case {case:?}: expected a refusal to purchase nothing, got {body}"
        );
    }

    // `RenewalSettings$RenewalCount` is `__integerMin1` like `Count`, so this
    // handler refuses a count below that floor with the same modelled error.
    for (case, renewal) in [
        ("a renewal count below the minimum", json!({"renewalCount": 0})),
        ("a negative renewal count", json!({"renewalCount": -2})),
    ] {
        let (status, body) = purchase(
            &router,
            DEFAULT_REGION,
            offering_id,
            json!({"count": 1, "renewalSettings": renewal}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {renewal} to be refused, got {status}: {body}"
        );
        let code = error_code(status, &body);
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled client error for {renewal}, got {body}"
        );
        assert!(
            body.get("reservation").is_none(),
            "case {case:?}: expected a refusal to purchase nothing, got {body}"
        );
    }

    // `RenewalSettings$AutomaticRenewal` targets the `ReservationAutomaticRenewal`
    // enum, and the value set of an enum-targeted member is enforced at the
    // request boundary — so a value the enum does not declare never reaches the
    // handler and is refused there, as the boundary's `ValidationException`.
    for (case, renewal) in [
        (
            "an automatic-renewal status the enum does not declare",
            json!({"automaticRenewal": "SOMETIMES"}),
        ),
        (
            "a lowercase spelling of a declared status",
            json!({"automaticRenewal": "enabled"}),
        ),
    ] {
        let (status, body) = purchase(
            &router,
            DEFAULT_REGION,
            offering_id,
            json!({"count": 1, "renewalSettings": renewal}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {renewal} to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ValidationException",
            "case {case:?}: expected the boundary's refusal for {renewal}, got {body}"
        );
        assert!(
            body.get("reservation").is_none(),
            "case {case:?}: expected a refusal to purchase nothing, got {body}"
        );
    }

    // Every status the enum DOES declare is accepted, and reported back as sent.
    for status_value in RESERVATION_AUTOMATIC_RENEWAL_VALUES {
        let case = format!("automaticRenewal {status_value}");
        let (status, answered) = purchase(
            &router,
            DEFAULT_REGION,
            offering_id,
            json!({"count": 1, "renewalSettings": {"automaticRenewal": status_value}}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a declared status to be accepted, got {status}: {answered}"
        );
        assert_eq!(
            reservation(&case, &answered)["renewalSettings"],
            json!({"automaticRenewal": status_value}),
            "case {case:?}: expected the renewal status the request asked for"
        );
    }
}

/// Rule 2: `Start` is "Requested reservation start time (UTC) in ISO-8601
/// format. The specified time must be between the first day of the current month
/// and one year from now. If no value is given, the default is now." — so a time
/// inside that window is leased from as asked, an absent one leases from now, and
/// a time outside it or no time at all is the operation's modelled
/// `BadRequestException` with nothing purchased.
///
/// The rows differ along WHERE the requested start falls: both published bounds,
/// inside, just outside each end, and values that are not times at all.
#[tokio::test]
async fn a_requested_start_must_fall_in_the_window_the_member_documents() {
    let router = medialive();
    let offering_id = first_offering(&router, "any offering", "resourceType=CHANNEL").await;
    let now = DateTime::<Utc>::from(SystemTime::now())
        .with_nanosecond(0)
        .expect("a whole second is a time");
    let earliest = first_of_month(now);
    let latest = months_on(now, 12);

    // Inside the window, both bounds included.
    for (case, start) in [
        ("the first day of the current month", earliest),
        ("a second into the window", earliest + Duration::from_secs(1)),
        ("now", now),
        ("one year from now", latest),
        ("a day inside the far end", latest - Duration::from_secs(86_400)),
    ] {
        let requested = spelled(start);
        let (status, answered) = purchase(
            &router,
            DEFAULT_REGION,
            &offering_id,
            json!({"count": 1, "start": requested}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a start inside the window to be leased from, got {status}: \
             {answered}"
        );
        let purchased = reservation(case, &answered);
        assert_eq!(
            purchased["start"],
            json!(requested),
            "case {case:?}: expected the reservation to start when the request asked, got {:?}",
            purchased["start"]
        );
        let end = instant(case, &purchased["end"]);
        assert_eq!(
            end,
            months_on(start, 12),
            "case {case:?}: expected the lease to run a year from the requested start, got {end}"
        );
    }

    // Outside it, or not a time at all.
    for (case, requested) in [
        (
            "a second before the first of the month",
            spelled(earliest - Duration::from_secs(1)),
        ),
        (
            "the month before this one",
            spelled(first_of_month(earliest - Duration::from_secs(86_400))),
        ),
        (
            "a day past one year from now",
            spelled(latest + Duration::from_secs(86_400)),
        ),
        ("two years from now", spelled(months_on(now, 24))),
        ("no time at all", "whenever".to_string()),
        ("an impossible month", "2030-13-01T00:00:00".to_string()),
        ("an empty start", String::new()),
        ("epoch seconds", "1700000000".to_string()),
    ] {
        let (status, body) = purchase(
            &router,
            DEFAULT_REGION,
            &offering_id,
            json!({"count": 1, "start": requested.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {requested:?} to be refused, got {status}: {body}"
        );
        let code = error_code(status, &body);
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected the modelled client error for {requested:?}, got {body}"
        );
        assert!(
            body.get("reservation").is_none(),
            "case {case:?}: expected a refusal to purchase nothing, got {body}"
        );
        assert!(
            error_message(&body).contains("start"),
            "case {case:?}: expected the refusal to name the member, got {body}"
        );
    }

    // A purchase that names no start leases from now: inside the window, and the
    // reservation is ACTIVE.
    let (status, answered) = purchase(
        &router,
        DEFAULT_REGION,
        &offering_id,
        json!({"count": 1}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"no start\": expected the default to be now, got {status}: {answered}"
    );
    let purchased = reservation("no start", &answered);
    let start = instant("no start", &purchased["start"]);
    assert!(
        start >= earliest && start <= latest,
        "case \"no start\": expected the default start to be now, got {start}"
    );
    assert_eq!(
        purchased["state"],
        json!(ACTIVE),
        "case \"no start\": expected a reservation whose lease has begun to be ACTIVE, got {:?}",
        purchased["state"]
    );
}

/// Rule 3: `RequestId` is "Unique request ID to be specified. This is needed to
/// prevent retries from creating multiple resources", so a retry under the same
/// token answers with the reservation the first purchase created, while a
/// different token — or none — buys another one.
///
/// The rows differ along the token a purchase carries: the same one again, a
/// second one, and none at all.
#[tokio::test]
async fn a_retried_purchase_answers_with_the_reservation_it_already_made() {
    let router = medialive();
    let offering_id = first_offering(&router, "any offering", "resourceType=INPUT").await;
    let body = |token: Option<&str>| {
        let mut body = Map::new();
        body.insert("count".to_string(), json!(2));
        body.insert("name".to_string(), json!("retried"));
        if let Some(token) = token {
            body.insert("requestId".to_string(), json!(token));
        }
        Value::Object(body)
    };

    let (status, first) = purchase(&router, DEFAULT_REGION, &offering_id, body(Some("token-a"))).await;
    assert_eq!(
        status, 201,
        "case \"first purchase\": expected 201, got {status}: {first}"
    );
    let first = reservation("first purchase", &first);

    for attempt in 1..=3 {
        let case = format!("retry {attempt}");
        let (status, again) =
            purchase(&router, DEFAULT_REGION, &offering_id, body(Some("token-a"))).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a retry to answer 201, got {status}: {again}"
        );
        assert_eq!(
            reservation(&case, &again),
            first,
            "case {case:?}: expected the retry to answer with the reservation already purchased"
        );
    }

    for (case, token) in [
        ("another token", Some("token-b")),
        ("no token at all", None),
    ] {
        let (status, other) = purchase(&router, DEFAULT_REGION, &offering_id, body(token)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected a purchase under {token:?} to answer 201, got {status}: \
             {other}"
        );
        let other = reservation(case, &other);
        assert_ne!(
            other["reservationId"], first["reservationId"],
            "case {case:?}: expected a purchase under {token:?} to mint its own reservation, got \
             {:?}",
            other["reservationId"]
        );
    }
}

/// Rule 4: an `OfferingId` the catalog does not hold names nothing to buy, and a
/// `ReservationId` the account does not hold names nothing to read, so each is
/// the operation's own modelled `NotFoundException` (404) that creates and
/// reports nothing.
///
/// The rows differ along HOW the id misses: a value that is no id at all, one
/// outside the id space, and the other resource's id.
#[tokio::test]
async fn an_id_that_names_nothing_is_not_found() {
    let router = medialive();
    let offering_id = first_offering(&router, "any offering", "resourceType=OUTPUT").await;
    let (status, answered) = purchase(
        &router,
        DEFAULT_REGION,
        &offering_id,
        json!({"count": 1}),
    )
    .await;
    assert_eq!(status, 201, "setup: expected a purchase, got {status}: {answered}");
    let purchased = reservation("setup", &answered);
    let reservation_id = purchased["reservationId"].as_str().unwrap_or_default();

    for (case, id) in [
        ("no id at all", "not-an-offering".to_string()),
        ("zero", "0".to_string()),
        ("a reservation id", reservation_id.to_string()),
        ("far past the catalog", "99999999".to_string()),
    ] {
        let (status, body) = purchase(&router, DEFAULT_REGION, &id, json!({"count": 1})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected PurchaseOffering {id} to be refused as not found, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for offering {id}, got {body}"
        );
        assert!(
            body.get("reservation").is_none(),
            "case {case:?}: expected a refusal to purchase nothing, got {body}"
        );
    }

    for (case, id) in [
        ("no id at all", "not-a-reservation".to_string()),
        ("zero", "0".to_string()),
        ("an offering id", offering_id.clone()),
        ("one past the minted id", format!("{reservation_id}9")),
    ] {
        let (status, body) = describe(&router, DEFAULT_REGION, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DescribeReservation {id} to be refused as not found, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for reservation {id}, got {body}"
        );
        for member in RESERVATION_MEMBERS {
            assert!(
                body.get(member).is_none(),
                "case {case:?}: expected a refusal to report no reservation, got {member} in \
                 {body}"
            );
        }
    }
}

/// Rule 5: a reservation is `ACTIVE` while its lease runs and `EXPIRED` from its
/// `End` on — the clock's own transition, which a read reports without writing
/// anything — and it is reported by the region it was purchased in, which is the
/// region its `Region` member and its ARN name; no other region's endpoint
/// resolves its id.
///
/// The rows differ along WHERE the stored lease ends relative to the read: well
/// before it, a second before it, exactly at it, and after it.
#[tokio::test]
async fn the_clock_decides_whether_a_lease_still_runs() {
    let (router, engine) = medialive_with_engine();
    let offering_id = first_offering(&router, "any offering", "resourceType=MULTIPLEX").await;

    // Purchased against another region's endpoint: a reservation reserves
    // capacity where it was bought, not where it is read.
    let bought_in = "eu-west-1";
    let (status, answered) = purchase(
        &router,
        bought_in,
        &offering_id,
        json!({"count": 1, "tags": {"owner": "finance"}}),
    )
    .await;
    assert_eq!(status, 201, "setup: expected a purchase, got {status}: {answered}");
    let purchased = reservation("setup", &answered);
    let id = purchased["reservationId"]
        .as_str()
        .expect("a minted reservation id")
        .to_string();
    assert_eq!(
        purchased["region"],
        json!(bought_in),
        "setup: expected the reservation to name the region it was purchased in, got {:?}",
        purchased["region"]
    );
    assert_eq!(
        purchased["arn"],
        json!(format!("arn:aws:medialive:{bought_in}:{ACCOUNT}:reservation:{id}")),
        "setup: expected the ARN to name that region, got {:?}",
        purchased["arn"]
    );

    // The region the purchase was signed for is the region that holds the
    // reservation: it reserves capacity where it was bought.
    let (status, described) = describe(&router, bought_in, &id).await;
    assert_eq!(status, 200, "setup: expected the read to answer 200, got {status}: {described}");
    assert_eq!(
        described["region"],
        json!(bought_in),
        "expected the read to report the purchased region, got {:?}",
        described["region"]
    );

    // Another region's endpoint holds no reservation under that id.
    let (status, elsewhere) = describe(&router, DEFAULT_REGION, &id).await;
    assert_eq!(
        status, 404,
        "expected {DEFAULT_REGION} to hold no reservation {id}, got {status}: {elsewhere}"
    );

    let now = SystemTime::now();
    for (case, end, expected) in [
        ("a lease ending in an hour", now + Duration::from_secs(3_600), ACTIVE),
        // A margin, not a second: the three reads below are three dispatches, and
        // a one-second lease could expire between them, which would fail the row
        // for timing rather than for behaviour. The near edge is asserted from
        // the EXPIRED side instead, by the row that ends the lease at this
        // instant.
        ("a lease ending in a minute", now + Duration::from_secs(60), ACTIVE),
        ("a lease ending at this instant", now, EXPIRED),
        ("a lease that ended an hour ago", now - Duration::from_secs(3_600), EXPIRED),
        ("a lease that ended a year ago", now - Duration::from_secs(31_536_000), EXPIRED),
    ] {
        // The store's own record, so the state the read reports is derived from
        // the lease something stored rather than from a second copy.
        {
            let store = state::service_state_in(&*engine, bought_in)
                .expect("the medialive store");
            let mut store = store.lock();
            let reservation = store
                .reservations
                .get_mut(&id)
                .unwrap_or_else(|| panic!("case {case:?}: expected the stored reservation"));
            reservation.end = end;
        }

        let (status, described) = describe(&router, bought_in, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status}: {described}"
        );
        assert_eq!(
            described["state"],
            json!(expected),
            "case {case:?}: expected the state the clock owes, got {:?}",
            described["state"]
        );

        // Reporting changes nothing a later caller sees.
        for attempt in 1..=2 {
            let (status, again) = describe(&router, bought_in, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected repeated read {attempt} to answer 200, got {status}: \
                 {again}"
            );
            assert_eq!(
                again, described,
                "case {case:?}: expected repeated read {attempt} to report the same reservation"
            );
        }
    }

    // The tags the purchase carried are the reservation's own, readable through
    // the tag reader that addresses it by ARN.
    let arn = purchased["arn"].as_str().expect("a minted ARN");
    let (status, tags) = call_in_region(
        &router,
        bought_in,
        "GET",
        &format!("/prod/tags/{}", percent_encoded(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the reservation's tags to be readable by ARN, got {status}: {tags}"
    );
    let reported: HashMap<String, String> =
        serde_json::from_value(tags["tags"].clone()).unwrap_or_default();
    assert_eq!(
        reported,
        HashMap::from([("owner".to_string(), "finance".to_string())]),
        "expected the tags the purchase carried, got {tags}"
    );
}

/// Rule 6: the two caller-chosen strings a purchase carries are held to this
/// service's length bounds, and the store is held to its own ceiling — each
/// refused with the modelled error for it (400 `BadRequestException` for a
/// string, 429 `TooManyRequestsException` for a full store) and storing nothing.
///
/// The rows differ along WHICH bound is probed and where the value falls
/// relative to it: both ends of each length bound and one past each, then the
/// ceiling itself and one purchase past it.
#[tokio::test]
async fn the_caller_chosen_strings_and_the_store_are_bounded() {
    let router = medialive();
    let offering_id = first_offering(&router, "any offering", "resourceType=INPUT").await;

    for (member, bound) in [("name", MAX_RESOURCE_NAME_LEN), ("requestId", MAX_REQUEST_ID_LEN)] {
        // Both ends of the bound are served.
        for (case, length) in [("the shortest allowed", 1), ("the longest allowed", bound)] {
            let (status, answered) = purchase(
                &router,
                DEFAULT_REGION,
                &offering_id,
                json!({"count": 1, member: "x".repeat(length)}),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?} {member}: expected a {length}-character value to be served, got \
                 {status}: {answered}"
            );
        }
        // One character past either end is refused, and nothing is purchased.
        for (case, length) in [("empty", 0), ("one past the bound", bound + 1)] {
            let (status, body) = purchase(
                &router,
                DEFAULT_REGION,
                &offering_id,
                json!({"count": 1, member: "x".repeat(length)}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} {member}: expected a {length}-character value to be refused, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "BadRequestException",
                "case {case:?} {member}: expected the modelled client error, got {body}"
            );
            assert!(
                body.get("reservation").is_none(),
                "case {case:?} {member}: expected a refusal to purchase nothing, got {body}"
            );
            assert!(
                error_message(&body).contains(member),
                "case {case:?} {member}: expected the refusal to name the member, got {body}"
            );
        }
    }

    // The store's ceiling: a fresh router, filled to it, then one more purchase.
    let router = medialive();
    let offering_id = first_offering(&router, "any offering", "resourceType=INPUT").await;
    for purchased in 1..=MAX_RESERVATIONS {
        let (status, answered) = purchase(
            &router,
            DEFAULT_REGION,
            &offering_id,
            json!({"count": 1}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case \"filling the store\": expected purchase {purchased} of {MAX_RESERVATIONS} to \
             be served, got {status}: {answered}"
        );
    }
    let (status, body) = purchase(
        &router,
        DEFAULT_REGION,
        &offering_id,
        json!({"count": 1}),
    )
    .await;
    assert_eq!(
        status, 429,
        "case \"one past the ceiling\": expected the purchase past {MAX_RESERVATIONS} to be \
         throttled, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "TooManyRequestsException",
        "case \"one past the ceiling\": expected the modelled throttle, got {body}"
    );
    assert!(
        body.get("reservation").is_none(),
        "case \"one past the ceiling\": expected a refusal to purchase nothing, got {body}"
    );
}

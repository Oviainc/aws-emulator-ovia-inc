// SPDX-License-Identifier: CC0-1.0

//! ListReservations, end to end over HTTP: `GET /v1/reservations`.
//!
//! The subject is the rule ListReservations follows: it must report the reservations the
//! store HOLDS — every one of them, each exactly once, in one stable order, each projected
//! field for field from its record and in the state the CLOCK has brought it to — a page
//! at a time, rather than a value fixed in the handler. The cases below therefore vary
//! what that rule generalises over: how many reservations are stored, the page size
//! (across the modelled `MaxResults` range and its default), what shape the pagination
//! token has, and what state, terms and start/end instants a stored reservation carries.
//!
//! The records are put in the store directly, the way a purchase stores one: PurchaseOffering
//! is still a scaffold (`issues/things-that-do-not-work/20261024T010542Z-mediaconnect-purchaseoffering-stub.md`),
//! so there is no request that mints a reservation yet — exactly as
//! `gateway_lifecycle::seed_gateways` seeds the gateway store for the cases a round trip
//! cannot reach. Every LIST call below goes over the wire through the same
//! `MultiServiceRouter` the emulator's front door uses, so what these tests assert is what
//! a MediaConnect client sees: routing, restJson1 deserialization, constraint validation,
//! handler, serialization.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::ResourceSpecification;
use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// The only `CurrencyCode` MediaConnect bills a reservation in: "the currencyCode used for
/// your reservation is US dollars"
/// (`smithy_query(query="shape",shape="Reservation",member="CurrencyCode")`).
pub(crate) const CURRENCY_CODE: &str = "USD";

/// The only value the modelled `PriceUnits` enum carries
/// (`smithy_query(query="shape",shape="PriceUnits")`: HOURLY).
pub(crate) const PRICE_UNITS: &str = "HOURLY";

/// The only value the modelled `DurationUnits` enum carries
/// (`smithy_query(query="shape",shape="DurationUnits")`: MONTHS).
pub(crate) const DURATION_UNITS: &str = "MONTHS";

/// The only value the modelled `ResourceType` enum carries
/// (`smithy_query(query="shape",shape="ResourceType")`: Mbps_Outbound_Bandwidth).
pub(crate) const RESOURCE_TYPE: &str = "Mbps_Outbound_Bandwidth";

pub(crate) async fn list_reservations(
    emulator: &Emulator,
    query: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/reservations{query}"), None)
        .await
}

/// The `Reservations` member of a ListReservations response.
fn listed(body: &Value) -> Vec<Value> {
    body["reservations"].as_array().cloned().unwrap_or_default()
}

/// The ARNs of the `Reservations` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|reservation| {
            reservation["reservationArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The listed reservation with this ARN, or `None` when the page does not carry it.
pub(crate) fn listed_by_arn(body: &Value, arn: &str) -> Option<Value> {
    listed(body)
        .into_iter()
        .find(|reservation| reservation["reservationArn"].as_str() == Some(arn))
}

/// Every ARN a purchase mints for a reservation travels as an `@httpLabel` on
/// DescribeReservation, and is the key ListReservations pages from, so it must be one this
/// service could have minted.
///
/// The check runs through the crate's own encoding of that form — the same function
/// ListReservations validates a pagination token with — and then re-asserts, field by
/// field, the parts the form fixes, so a bug in that function cannot hide here.
pub(crate) fn assert_minted_reservation_arn(case: &str, arn: &str, name: &str) {
    assert!(
        mc::is_mintable_reservation_arn(arn),
        "case {case:?}: a listed ReservationArn must be one this service could mint, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: a reservation ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "reservation"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the reservation name, got {arn:?}"
    );
}

/// `Reservation$Start` and `Reservation$End` carry the documented form: "Use the following
/// format: `YYYY-MM-DDTHH:mm:SSZ`, where `T` and `Z` are literal characters"
/// (`smithy_query(query="doc",shape="PurchaseOfferingRequest",member="Start")`).
pub(crate) fn assert_documented_instant(case: &str, member: &str, value: &str) {
    assert_eq!(
        value.len(),
        20,
        "case {case:?}: {member} is YYYY-MM-DDTHH:mm:SSZ (20 characters), got {value:?}"
    );
    let bytes = value.as_bytes();
    assert_eq!(
        (
            bytes[4] as char,
            bytes[7] as char,
            bytes[10] as char,
            bytes[13] as char,
            bytes[16] as char,
            bytes[19] as char
        ),
        ('-', '-', 'T', ':', ':', 'Z'),
        "case {case:?}: {member} separators are -, -, T, :, :, Z, got {value:?}"
    );
    for (index, byte) in bytes.iter().enumerate() {
        if [4, 7, 10, 13, 16, 19].contains(&index) {
            continue;
        }
        assert!(
            byte.is_ascii_digit(),
            "case {case:?}: {member} character {index} is a digit, got {value:?}"
        );
    }
}

/// One reservation to put in the store, and the state the clock makes of it.
///
/// The rows differ along everything the reporting rule generalises over: the stored state,
/// whether the clock owes the record a change, whether its `End` has passed, the offering
/// terms copied onto it (duration, price, reserved bitrate — including the optional member
/// left absent) and the form of its name.
pub(crate) struct ReservationCase {
    pub(crate) case: &'static str,
    pub(crate) name: &'static str,
    /// `Reservation$Duration`, in [`DURATION_UNITS`].
    pub(crate) duration: i32,
    pub(crate) price_per_unit: &'static str,
    /// `ResourceSpecification$ReservedBitrate` — the one OPTIONAL member of the
    /// specification (`smithy_query(query="shape",shape="ResourceSpecification")`).
    pub(crate) reserved_bitrate: Option<i32>,
    /// `Reservation$Start`, as an offset BEFORE the instant the case runs at.
    pub(crate) start_before_now: Duration,
    /// `Reservation$End`: `Ok` is that long AFTER now (the reservation is still running),
    /// `Err` that long BEFORE it (its end has passed).
    pub(crate) end_from_now: Result<Duration, Duration>,
    /// The `ReservationState` the record is stored in.
    pub(crate) stored_state: &'static str,
    /// The state the clock owes it and the instant that change is due, as an offset from
    /// now (`Ok` in the future, `Err` in the past), or `None` when nothing is owed.
    pub(crate) pending: Option<(&'static str, Result<Duration, Duration>)>,
    /// The `ReservationState` ListReservations must report.
    pub(crate) expected_state: &'static str,
}

/// An instant `offset` after now (`Ok`) or before it (`Err`).
///
/// `pub(crate)` because `offering_listing` places a reservation against the clock the same
/// way — it is the one spelling of "this long either side of now" the suites seed with.
pub(crate) fn instant_from(now: SystemTime, offset: Result<Duration, Duration>) -> SystemTime {
    match offset {
        Ok(ahead) => now + ahead,
        Err(behind) => now - behind,
    }
}

/// Put one reservation straight into the store, the way a purchase stores one, and answer
/// the ARN it was minted with.
pub(crate) fn seed_reservation(
    emulator: &Emulator,
    case: &ReservationCase,
    now: SystemTime,
) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let suffix = store.next_suffix();
    let reservation_arn = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        REGION,
        &mc::reservation_resource_fragment(suffix, case.name),
    );
    store.reservations.insert(
        reservation_arn.clone(),
        mc::StoredReservation {
            reservation_arn: reservation_arn.clone(),
            reservation_name: case.name.to_string(),
            // The offering a reservation was bought from, in the form this service
            // publishes its catalogue in: an offering ARN carries no account-id field.
            offering_arn: mc::offering_arn(
                REGION,
                &mc::offering_resource_fragment(
                    case.reserved_bitrate.unwrap_or_default(),
                    case.duration,
                ),
            ),
            offering_description: mc::offering_description(
                case.reserved_bitrate.unwrap_or_default(),
                case.price_per_unit,
            ),
            currency_code: CURRENCY_CODE.to_string(),
            price_per_unit: case.price_per_unit.to_string(),
            price_units: PRICE_UNITS.to_string(),
            duration: case.duration,
            duration_units: DURATION_UNITS.to_string(),
            resource_specification: ResourceSpecification {
                reserved_bitrate: case.reserved_bitrate,
                resource_type: RESOURCE_TYPE.to_string(),
            },
            start_at: now - case.start_before_now,
            end_at: instant_from(now, case.end_from_now),
            reservation_state: case.stored_state.to_string(),
            pending: case.pending.map(|(status, due)| mc::PendingStatus {
                status: status.to_string(),
                due_at: instant_from(now, due),
            }),
        },
    );
    reservation_arn
}

/// One month, as the span the seeded reservations run for. Only its ORDER against the
/// clock matters to the cases below — whether a reservation's `End` has passed — so the
/// value is the test's own, not a duration the service documents.
pub(crate) const ONE_MONTH: Duration = Duration::from_secs(30 * 24 * 60 * 60);

pub(crate) fn reservation_cases() -> Vec<ReservationCase> {
    vec![
        ReservationCase {
            case: "active reservation still running",
            name: "res-active",
            duration: 12,
            price_per_unit: "0.3000",
            reserved_bitrate: Some(100),
            start_before_now: ONE_MONTH,
            end_from_now: Ok(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            pending: None,
            expected_state: mc::RESERVATION_STATE_ACTIVE,
        },
        ReservationCase {
            case: "active reservation whose End has passed is expired by the clock",
            name: "res_ended",
            duration: 1,
            price_per_unit: "0.1500",
            reserved_bitrate: Some(1_000),
            start_before_now: ONE_MONTH * 2,
            end_from_now: Err(Duration::from_secs(1)),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            pending: None,
            expected_state: mc::RESERVATION_STATE_EXPIRED,
        },
        ReservationCase {
            case: "purchase whose settle is not due yet stays processing",
            name: "res-processing",
            duration: 6,
            price_per_unit: "0.2500",
            reserved_bitrate: Some(50),
            start_before_now: Duration::from_secs(0),
            end_from_now: Ok(ONE_MONTH * 6),
            stored_state: mc::RESERVATION_STATE_PROCESSING,
            pending: Some((mc::RESERVATION_STATE_ACTIVE, Ok(Duration::from_secs(300)))),
            expected_state: mc::RESERVATION_STATE_PROCESSING,
        },
        ReservationCase {
            case: "purchase whose settle came due is active",
            name: "res-settled",
            duration: 6,
            price_per_unit: "0.2500",
            // The optional member left absent: an offering need not name a bitrate.
            reserved_bitrate: None,
            start_before_now: Duration::from_secs(60),
            end_from_now: Ok(ONE_MONTH * 6),
            stored_state: mc::RESERVATION_STATE_PROCESSING,
            pending: Some((mc::RESERVATION_STATE_ACTIVE, Err(Duration::from_secs(1)))),
            expected_state: mc::RESERVATION_STATE_ACTIVE,
        },
        ReservationCase {
            case: "canceled reservation is terminal, not expired on top",
            name: "res.canceled",
            duration: 12,
            price_per_unit: "0.0500",
            reserved_bitrate: Some(10_000),
            start_before_now: ONE_MONTH * 13,
            end_from_now: Err(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_CANCELED,
            pending: None,
            expected_state: mc::RESERVATION_STATE_CANCELED,
        },
        ReservationCase {
            case: "reservation already expired stays expired",
            name: "res-expired",
            duration: 1,
            price_per_unit: "0.1500",
            reserved_bitrate: Some(100),
            start_before_now: ONE_MONTH * 2,
            end_from_now: Err(ONE_MONTH),
            stored_state: mc::RESERVATION_STATE_EXPIRED,
            pending: None,
            expected_state: mc::RESERVATION_STATE_EXPIRED,
        },
    ]
}

/// ListReservations reports every reservation the store holds — "all reservations in all
/// states (such as active and expired)" — projected field for field from the record, in
/// the state the clock has brought it to. Nothing in the response is fixed in the handler:
/// each member below is compared against what was stored.
#[tokio::test]
async fn list_reports_every_stored_reservation_as_the_store_holds_it() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    let cases = reservation_cases();
    let arns: Vec<String> = cases
        .iter()
        .map(|case| seed_reservation(&emulator, case, now))
        .collect();

    let (status, code, body) = list_reservations(&emulator, "?maxResults=1000").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing expected 200, got {status} {code:?}: {body}"
    );
    assert_eq!(
        listed(&body).len(),
        cases.len(),
        "every stored reservation is listed exactly once, got {body}"
    );

    for (case, arn) in cases.iter().zip(&arns) {
        let name_of_case = case.case;
        let reservation = listed_by_arn(&body, arn).unwrap_or_else(|| {
            panic!("case {name_of_case:?}: the stored reservation must be listed, got {body}")
        });
        assert_minted_reservation_arn(name_of_case, arn, case.name);
        assert_eq!(
            reservation["reservationName"].as_str(),
            Some(case.name),
            "case {name_of_case:?}: the listed name is the stored one, got {reservation}"
        );
        assert_eq!(
            reservation["reservationState"].as_str(),
            Some(case.expected_state),
            "case {name_of_case:?}: expected state {:?}, got {reservation}",
            case.expected_state
        );
        assert_eq!(
            reservation["duration"].as_i64(),
            Some(i64::from(case.duration)),
            "case {name_of_case:?}: the listed duration is the stored one, got {reservation}"
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
            "case {name_of_case:?}: the billing terms are the stored ones, got {reservation}"
        );
        assert_eq!(
            reservation["resourceSpecification"]["resourceType"].as_str(),
            Some(RESOURCE_TYPE),
            "case {name_of_case:?}: the resource type is the stored one, got {reservation}"
        );
        assert_eq!(
            reservation["resourceSpecification"]["reservedBitrate"].as_i64(),
            case.reserved_bitrate.map(i64::from),
            "case {name_of_case:?}: the reserved bitrate is the stored one (absent when the \
             record names none), got {reservation}"
        );
        assert!(
            reservation["offeringArn"]
                .as_str()
                .is_some_and(|offering_arn| offering_arn.contains(":offering:")),
            "case {name_of_case:?}: the listed reservation names the offering it was bought \
             from, got {reservation}"
        );
        assert!(
            reservation["offeringDescription"]
                .as_str()
                .is_some_and(|description| !description.is_empty()),
            "case {name_of_case:?}: the offering description is reported, got {reservation}"
        );
        for member in ["start", "end"] {
            let value = reservation[member].as_str().unwrap_or_default();
            assert_documented_instant(name_of_case, member, value);
        }
        assert!(
            reservation["start"].as_str() < reservation["end"].as_str(),
            "case {name_of_case:?}: a reservation ends after it starts, got {reservation}"
        );
    }

    // A read does not change what a later caller sees: listing again reports the same
    // records, in the same order, in the same states (no row below is due a clock change
    // within the test's lifetime).
    let (_, _, again) = list_reservations(&emulator, "?maxResults=1000").await;
    assert_eq!(
        again, body,
        "listing twice reports the same reservations, got {again}"
    );
}

/// The instants `Reservation$Start` / `Reservation$End` are rendered at are the documented
/// format, to the second: the row is the doc's own example, "to specify 11:30pm on March
/// 5, 2020, enter 2020-03-05T23:30:00Z"
/// (`smithy_query(query="doc",shape="PurchaseOfferingRequest",member="Start")`).
#[tokio::test]
async fn listed_instants_are_rendered_in_the_documented_format() {
    /// 2020-03-05T23:30:00Z as seconds since the epoch.
    const MARCH_5_2020_2330Z: u64 = 1_583_451_000;

    let emulator = Emulator::new();
    let start_at = SystemTime::UNIX_EPOCH + Duration::from_secs(MARCH_5_2020_2330Z);
    // One month later, to the second: 2020-04-04T23:30:00Z.
    let end_at = start_at + ONE_MONTH;
    let arn = {
        let state = emulator
            .engine
            .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
        let mut store = state.lock();
        let suffix = store.next_suffix();
        let reservation_arn = engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            REGION,
            &mc::reservation_resource_fragment(suffix, "res-documented-instants"),
        );
        store.reservations.insert(
            reservation_arn.clone(),
            mc::StoredReservation {
                reservation_arn: reservation_arn.clone(),
                reservation_name: "res-documented-instants".to_string(),
                offering_arn: mc::offering_arn(
                    REGION,
                    &mc::offering_resource_fragment(100, 1),
                ),
                offering_description: mc::offering_description(100, "0.150"),
                currency_code: CURRENCY_CODE.to_string(),
                price_per_unit: "0.150".to_string(),
                price_units: PRICE_UNITS.to_string(),
                duration: 1,
                duration_units: DURATION_UNITS.to_string(),
                resource_specification: ResourceSpecification {
                    reserved_bitrate: Some(100),
                    resource_type: RESOURCE_TYPE.to_string(),
                },
                start_at,
                end_at,
                reservation_state: mc::RESERVATION_STATE_ACTIVE.to_string(),
                pending: None,
            },
        );
        reservation_arn
    };

    let (status, _, body) = list_reservations(&emulator, "").await;
    assert_eq!(status, 200, "listing expected 200, got {status}: {body}");
    let reservation = listed_by_arn(&body, &arn)
        .unwrap_or_else(|| panic!("the stored reservation must be listed, got {body}"));
    assert_eq!(
        (
            reservation["start"].as_str(),
            reservation["end"].as_str(),
            reservation["reservationState"].as_str()
        ),
        (
            Some("2020-03-05T23:30:00Z"),
            Some("2020-04-04T23:30:00Z"),
            // Its End is long past, so the clock has expired it.
            Some(mc::RESERVATION_STATE_EXPIRED)
        ),
        "the documented instant form, and a reservation whose End has passed is expired, \
         got {reservation}"
    );
}

/// Expiry is owed by the CLOCK, not by a call: a reservation whose `End` is still ahead is
/// reported active, and from that instant on every read reports it expired — without any
/// call in between moving it.
#[tokio::test]
async fn the_clock_expires_a_reservation_at_its_end() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    // Short enough that the test does not wait long, and long enough that the first read
    // lands before it.
    let runs_for = Duration::from_millis(400);
    let case = ReservationCase {
        case: "reservation ending during the test",
        name: "res-expiring",
        duration: 1,
        price_per_unit: "0.1500",
        reserved_bitrate: Some(100),
        start_before_now: Duration::from_secs(60),
        end_from_now: Ok(runs_for),
        stored_state: mc::RESERVATION_STATE_ACTIVE,
        pending: None,
        expected_state: mc::RESERVATION_STATE_ACTIVE,
    };
    let arn = seed_reservation(&emulator, &case, now);

    let (_, _, before) = list_reservations(&emulator, "").await;
    let reservation = listed_by_arn(&before, &arn)
        .unwrap_or_else(|| panic!("the stored reservation must be listed, got {before}"));
    assert_eq!(
        reservation["reservationState"].as_str(),
        Some(mc::RESERVATION_STATE_ACTIVE),
        "before its End the reservation is active, got {reservation}"
    );
    let end_before = reservation["end"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    tokio::time::sleep(runs_for + Duration::from_millis(100)).await;
    let (_, _, after) = list_reservations(&emulator, "").await;
    let reservation = listed_by_arn(&after, &arn)
        .unwrap_or_else(|| panic!("an expired reservation is still listed, got {after}"));
    assert_eq!(
        reservation["reservationState"].as_str(),
        Some(mc::RESERVATION_STATE_EXPIRED),
        "at its End the clock expires the reservation, got {reservation}"
    );
    assert_eq!(
        reservation["end"].as_str(),
        Some(end_before.as_str()),
        "expiry does not move the reservation's End, got {reservation}"
    );
}

/// ListReservations pages the persisted reservations: whatever `MaxResults` is, the pages
/// partition every stored reservation exactly once, in one stable order, and only the last
/// page omits `NextToken`. With no `MaxResults` the page holds the documented default of
/// ten.
#[tokio::test]
async fn list_pages_every_stored_reservation_exactly_once() {
    let emulator = Emulator::new();
    let now = SystemTime::now();
    // More than the documented default page size, so the default is observable.
    let total = 12;
    for index in 0..total {
        let case = ReservationCase {
            case: "paged reservation",
            name: "res-paged",
            duration: 12,
            price_per_unit: "0.3000",
            reserved_bitrate: Some(100 * (index + 1)),
            start_before_now: Duration::from_secs(60),
            end_from_now: Ok(ONE_MONTH * 12),
            stored_state: mc::RESERVATION_STATE_ACTIVE,
            pending: None,
            expected_state: mc::RESERVATION_STATE_ACTIVE,
        };
        seed_reservation(&emulator, &case, now);
    }

    let (_, _, all) = list_reservations(&emulator, "?maxResults=1000").await;
    let expected = listed_arns(&all);
    assert_eq!(
        expected.len(),
        usize::try_from(total).expect("a small count"),
        "every stored reservation is listed, got {expected:?}"
    );

    // "If MaxResults is not included in the request, the service defaults to pagination
    // with a maximum of 10 results per page"
    // (`smithy_query(query="doc",shape="ListReservationsRequest",member="MaxResults")`).
    let (status, _, default_page) = list_reservations(&emulator, "").await;
    assert_eq!(status, 200, "the default page expected 200, got {status}");
    assert_eq!(
        listed_arns(&default_page),
        expected[..10].to_vec(),
        "the default page holds the first ten stored reservations, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(expected[10].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): a page smaller than
    // the set, ones that divide it, one that equals it, and the maximum.
    for page_size in [1_usize, 2, 5, 12, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, code, body) = list_reservations(&emulator, &query).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "page size {page_size}: expected 200, got {status} {code:?}: {body}"
            );
            let page = listed_arns(&body);
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "page size {page_size}: a page that carries a token is full, got {body}"
                );
            }
            seen.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size}: the pages must partition every stored reservation once, \
             in order"
        );
    }

    // The token is the ARN the listing RESUMES AT, so a page resumed from it starts there
    // — even after the clock has expired the reservation it names, which stays listed.
    let (_, _, first_page) = list_reservations(&emulator, "?maxResults=3").await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let (status, code, resumed) =
        list_reservations(&emulator, &format!("?maxResults=1000&nextToken={token}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "resuming at a reservation's ARN expected 200, got {status} {code:?}: {resumed}"
    );
    assert_eq!(
        listed_arns(&resumed),
        expected[3..].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// An empty store is an empty listing, not a 404 and not a page of invented reservations:
/// the account holds no reservation until one is purchased.
#[tokio::test]
async fn an_account_with_no_reservations_is_listed_empty() {
    let emulator = Emulator::new();
    let (status, code, body) = list_reservations(&emulator, "").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "listing an empty store expected 200, got {status} {code:?}: {body}"
    );
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "an account with no reservations lists none, got {body}"
    );
    assert_eq!(
        body["nextToken"].as_str(),
        None,
        "the last page carries no token, got {body}"
    );
}

/// One refused ListReservations request.
struct RefusalCase {
    case: &'static str,
    query: String,
    status: u16,
    code: &'static str,
}

/// ListReservations refuses a page size outside the modelled range and a pagination token
/// it could not have issued — with the modelled `BadRequestException` (400), never with a
/// 501 and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    let cases = vec![
        // The modelled `MaxResults` range is 1..1000, so each bound is usable (the paging
        // suite above lists with 1 and with 1000) and each value past it is refused.
        RefusalCase {
            case: "MaxResults below the modelled range",
            query: "?maxResults=0".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "MaxResults above the modelled range",
            query: "?maxResults=1001".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        // A `NextToken` is the reservation ARN the listing resumes at, so a token the
        // service could not have minted is refused. The rows are the ways a token can fail
        // that form: not an ARN at all, another service's ARN, an account id that is not 12
        // digits, the wrong resource type, and an empty reservation name.
        RefusalCase {
            case: "a nextToken that is not an ARN",
            query: "?nextToken=not-a-token".to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken from another service",
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:reservation:1-deadbeef:res"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken whose account id is not 12 digits",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:12345:reservation:1-deadbeef:res"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:flow:1-deadbeef:res"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a nextToken with an empty reservation name",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:reservation:1-deadbeef:"
                .to_string(),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        query,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let (status, code, body) = list_reservations(&emulator, &query).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
    }

    // A token the service COULD have minted is accepted even when no such reservation is
    // stored: paging must survive what happened to the record between pages.
    let unknown = "arn:aws:mediaconnect:us-east-1:000000000000:reservation:1-deadbeef:missing";
    let (status, code, body) =
        list_reservations(&emulator, &format!("?nextToken={unknown}")).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "a well-formed token for an unknown reservation pages from it, got {status} {code:?}: \
         {body}"
    );
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "nothing is stored past that key, got {body}"
    );
}

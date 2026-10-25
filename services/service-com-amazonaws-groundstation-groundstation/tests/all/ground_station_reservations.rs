// SPDX-License-Identifier: CC0-1.0

//! `ListGroundStationReservations` (`GET /groundstation/{groundStationId}/reservation`),
//! end to end over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: a ground station reservation is a block of antenna time held at
//! that ground station, so the listing REPORTS every block held at the named ground
//! station whose own window overlaps the requested one — one item per block, carrying the
//! antenna it holds, the window it holds it for and what it is held for (a contact of this
//! account that still holds antenna time there, as a `CONTACT` reservation naming that
//! contact, or an antenna AWS has taken out of service, as a `MAINTENANCE` reservation
//! naming its maintenance type) — narrowed to the `reservationTypes` the request names,
//! in chronological order, a bounded page at a time, changing nothing a later caller sees.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table of reservations differing in where their window sits relative to the
//!   requested one (wholly before, touching each end, straddling each end, inside,
//!   spanning it, wholly after), in which ground station they are held at and in which
//!   type they are — each asserted in or out of the answer by the overlap rule itself;
//! * every value of the model's `ReservationType` as a filter, singly, together, in either
//!   order and named not at all;
//! * every value of the model's `ContactStatus`, asserting which ones still hold the
//!   antenna — and the same release through the front door, with `CancelContact`;
//! * the placement rule: an antenna serves one pass at a time, so a table of overlapping
//!   and disjoint windows lands on the antennas the rule requires, maintenance included,
//!   and a pass at a ground station whose antennas this account HAS been told about is
//!   placed on one of them — and, since a ground station's antennas are finite, a window
//!   every one of them is already held for is refused rather than placed;
//! * paging over a table of page sizes, with `maxResults` on each published bound of
//!   `PaginationMaxResults` and one past each, and a `nextToken` from another window or
//!   another listing refused rather than resumed;
//! * a window that runs backwards, refused;
//! * the operator setting that takes an antenna out of service, over a table of forms it
//!   accepts and forms it refuses.
//!
//! Maintenance is not scheduled through this API (the model declares no operation that
//! schedules it), so it is seeded into this service's own state — exactly where
//! `state::MAINTENANCE_ENV` lands it — and then read back through the front door.

use serde_json::Value;
use service_com_amazonaws_groundstation_groundstation::state::{
    antenna_name, parse_ground_stations, parse_maintenance, service_state,
    BAD_REQUEST_ERROR_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::minute_usage::STATUS_HOLDS_ANTENNA_TIME;
use crate::scheduling_loop::{create_mission_profile, harness_with_engine, list_pages, send, Reply};

/// The instant every window in this suite is measured from: a round epoch-seconds value
/// in the future, as the other suites of this target use.
const BASE: i64 = 1_800_000_000;

/// The ground station the listings below ask about, and a second one whose reservations
/// must stay out of those answers.
const STATION: &str = "Ohio-1";

/// See [`STATION`].
const OTHER_STATION: &str = "Oregon-1";

/// The window the listings below ask about: one hour from [`BASE`].
const WINDOW: (i64, i64) = (BASE, BASE + 3600);

/// The two values of the model's `ReservationType` (`smithy_query(query="shape",
/// shape="ReservationType")`).
const CONTACT: &str = "CONTACT";

/// See [`CONTACT`].
const MAINTENANCE: &str = "MAINTENANCE";

/// Whether two windows share any instant — the rule the listing's window filter applies,
/// written here independently of the handler so the expectations below come from the rule
/// rather than from the answer.
fn overlaps(first: (i64, i64), second: (i64, i64)) -> bool {
    first.1 >= second.0 && first.0 <= second.1
}

/// One reservation as the listing reported it, reduced to what this suite asserts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reservation {
    reservation_type: String,
    ground_station_id: String,
    antenna_name: String,
    start_time: i64,
    end_time: i64,
    /// The `reservationDetails` union, rendered as `<member>=<value>`: `contact=<id>` or
    /// `maintenance=<MaintenanceType>`.
    details: String,
}

/// A string member of `item` (empty when absent, so a missing member fails the comparison
/// it is part of rather than panicking here).
fn member(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A timestamp member of `item`, as epoch seconds (`restJson1` carries an
/// `epoch-seconds` timestamp as a JSON number).
fn seconds(item: &Value, name: &str) -> i64 {
    item.get(name)
        .and_then(Value::as_f64)
        .map(|value| value.round() as i64)
        .unwrap_or_default()
}

/// `item` as a [`Reservation`].
fn reservation(item: &Value) -> Reservation {
    let details = item.get("reservationDetails").cloned().unwrap_or(Value::Null);
    let rendered = match (details.get("contact"), details.get("maintenance")) {
        (Some(contact), _) => format!("contact={}", member(contact, "contactId")),
        (None, Some(maintenance)) => {
            format!("maintenance={}", member(maintenance, "maintenanceType"))
        }
        (None, None) => format!("unknown={details}"),
    };
    Reservation {
        reservation_type: member(item, "reservationType"),
        ground_station_id: member(item, "groundStationId"),
        antenna_name: member(item, "antennaName"),
        start_time: seconds(item, "startTime"),
        end_time: seconds(item, "endTime"),
        details: rendered,
    }
}

/// The route of a reservation listing: the ground station, the window, and the types it
/// filters on when it names any.
fn route(ground_station: &str, window: (i64, i64), types: &[&str]) -> String {
    let mut query = format!("startTime={}&endTime={}", window.0, window.1);
    for reservation_type in types {
        query.push_str(&format!("&reservationTypes={reservation_type}"));
    }
    format!("/groundstation/{ground_station}/reservation?{query}")
}

/// Every reservation the listing reports for `ground_station` over `window`, across as
/// many pages as it takes, in the order it gave them.
async fn reservations(
    router: &MultiServiceRouter,
    ground_station: &str,
    window: (i64, i64),
    types: &[&str],
) -> Vec<Reservation> {
    list_pages(
        router,
        &route(ground_station, window, types),
        "reservationList",
        None,
    )
    .await
    .iter()
    .map(reservation)
    .collect()
}

/// Order a set of reservations the way the listing must report them: by when the block
/// starts, then when it ends, then the antenna, then what it is held for.
fn in_listing_order(expected: &mut [Reservation]) {
    expected.sort_by(|left, right| {
        (
            left.start_time,
            left.end_time,
            &left.antenna_name,
            &left.reservation_type,
            &left.details,
        )
            .cmp(&(
                right.start_time,
                right.end_time,
                &right.antenna_name,
                &right.reservation_type,
                &right.details,
            ))
    });
}

/// Take the antennas `setting` describes out of service, as a handler sees them.
fn schedule_maintenance(engine: &engine::InMemoryEngine, setting: &str) {
    let windows = parse_maintenance(setting);
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .maintenance_reservations = windows;
}

/// Record the antennas `setting` describes at their ground stations, as a handler sees
/// them — seeded where `state::GROUND_STATIONS_ENV` lands them.
///
/// What a ground station HAS is what bounds how much antenna time it can hold at once, so
/// a suite that reserves overlapping passes at one station says how many antennas that
/// station has rather than relying on the one a station with no recorded inventory has.
fn stock_inventory(engine: &engine::InMemoryEngine, setting: &str) {
    let inventory = parse_ground_stations(setting);
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .ground_stations = inventory;
}

/// An inventory setting giving [`STATION`] `antennas` antennas, named as this emulator
/// numbers them, and [`OTHER_STATION`] one.
fn inventory_setting(antennas: usize) -> String {
    let names: Vec<String> = (1..=antennas).map(antenna_name).collect();
    format!(
        "{STATION}={},{OTHER_STATION}={}",
        names.join("+"),
        antenna_name(1)
    )
}

/// One contact to reserve: where its window sits relative to [`WINDOW`], which ground
/// station holds it, and the antenna the placement rule puts it on.
struct Pass {
    case: &'static str,
    ground_station: &'static str,
    window: (i64, i64),
    antenna: usize,
}

/// The maintenance this suite's front-door table schedules: two antennas at [`STATION`] —
/// one inside [`WINDOW`], one outside it — and one at [`OTHER_STATION`]. The antennas are
/// numbered past every antenna the table's passes are placed on, so the placement rule and
/// the window filter are asserted independently of each other.
fn maintenance_setting() -> String {
    format!(
        "{STATION}/Antenna 7/PLANNED/{}/{},\
         {STATION}/Antenna 8/UNPLANNED/{}/{},\
         {OTHER_STATION}/Antenna 7/PLANNED/{}/{}",
        BASE + 600,
        BASE + 1200,
        BASE + 7200,
        BASE + 10800,
        BASE + 1200,
        BASE + 1800,
    )
}

/// The passes the front-door table reserves, in the order it reserves them: rows differing
/// in where their window sits relative to [`WINDOW`] — wholly before it, ending exactly as
/// it opens, straddling its start, wholly inside it, spanning it, starting exactly as it
/// closes, wholly after it — and one held at another ground station.
const PASSES: &[Pass] = &[
    Pass {
        case: "wholly-before-the-window",
        ground_station: STATION,
        window: (BASE - 7200, BASE - 3600),
        antenna: 1,
    },
    Pass {
        case: "ends-exactly-as-the-window-opens",
        ground_station: STATION,
        window: (BASE - 1800, BASE),
        antenna: 1,
    },
    Pass {
        case: "straddles-the-start-of-the-window",
        ground_station: STATION,
        window: (BASE - 600, BASE + 600),
        antenna: 2,
    },
    Pass {
        case: "wholly-inside-the-window",
        ground_station: STATION,
        window: (BASE + 1200, BASE + 1800),
        antenna: 1,
    },
    Pass {
        case: "spans-the-whole-window",
        ground_station: STATION,
        window: (BASE - 60, BASE + 3660),
        antenna: 3,
    },
    Pass {
        case: "starts-exactly-as-the-window-closes",
        ground_station: STATION,
        window: (BASE + 3600, BASE + 5400),
        antenna: 1,
    },
    Pass {
        case: "wholly-after-the-window",
        ground_station: STATION,
        window: (BASE + 7200, BASE + 10800),
        antenna: 1,
    },
    Pass {
        case: "at-another-ground-station",
        ground_station: OTHER_STATION,
        window: (BASE + 1200, BASE + 1800),
        antenna: 1,
    },
];

/// The maintenance the setting above schedules, as the listing must report it.
fn maintenance_rows() -> Vec<(&'static str, Reservation)> {
    vec![
        (
            "maintenance-inside-the-window",
            Reservation {
                reservation_type: MAINTENANCE.to_string(),
                ground_station_id: STATION.to_string(),
                antenna_name: antenna_name(7),
                start_time: BASE + 600,
                end_time: BASE + 1200,
                details: "maintenance=PLANNED".to_string(),
            },
        ),
        (
            "maintenance-after-the-window",
            Reservation {
                reservation_type: MAINTENANCE.to_string(),
                ground_station_id: STATION.to_string(),
                antenna_name: antenna_name(8),
                start_time: BASE + 7200,
                end_time: BASE + 10800,
                details: "maintenance=UNPLANNED".to_string(),
            },
        ),
        (
            "maintenance-at-another-ground-station",
            Reservation {
                reservation_type: MAINTENANCE.to_string(),
                ground_station_id: OTHER_STATION.to_string(),
                antenna_name: antenna_name(7),
                start_time: BASE + 1200,
                end_time: BASE + 1800,
                details: "maintenance=PLANNED".to_string(),
            },
        ),
    ]
}

/// A router holding this service, with [`MAINTENANCE_SETTING`] in force and every pass of
/// [`PASSES`] reserved through the front door, beside the reservation each pass must be
/// reported as.
///
/// The mission profile carries pre- and post-pass durations, so a reservation that
/// reported the widened pass window instead of the contact's own would fail the
/// comparison.
async fn stocked_harness() -> (MultiServiceRouter, Vec<(&'static str, Reservation)>) {
    let (router, engine) = harness_with_engine();
    schedule_maintenance(&engine, &maintenance_setting());
    // Three antennas at the subject station, which is what the table's overlapping passes
    // need: an antenna serves one pass at a time, and at most three of the table's windows
    // share an instant.
    stock_inventory(&engine, &inventory_setting(3));
    let profile_arn = create_mission_profile(&router, "reservations", 1, Some(120), Some(180)).await;

    let mut held: Vec<(&'static str, Reservation)> = Vec::new();
    for pass in PASSES {
        let contact_id = reserve_pass_at(
            &router,
            &profile_arn,
            pass.ground_station,
            pass.case,
            pass.window,
        )
        .await;
        held.push((
            pass.case,
            Reservation {
                reservation_type: CONTACT.to_string(),
                ground_station_id: pass.ground_station.to_string(),
                antenna_name: antenna_name(pass.antenna),
                start_time: pass.window.0,
                end_time: pass.window.1,
                details: format!("contact={contact_id}"),
            },
        ));
    }
    held.extend(maintenance_rows());
    (router, held)
}

/// `ReserveContact` for a window at one ground station, answering the reply as it came.
async fn reserve_reply(
    router: &MultiServiceRouter,
    profile_arn: &str,
    ground_station: &str,
    window: (i64, i64),
) -> Reply {
    send(
        router,
        "POST",
        "/contact",
        Some(serde_json::json!({
            "missionProfileArn": profile_arn,
            "groundStation": ground_station,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await
}

/// `ReserveContact` for a window at one ground station, answering the `contactId`.
async fn reserve_pass_at(
    router: &MultiServiceRouter,
    profile_arn: &str,
    ground_station: &str,
    case: &str,
    window: (i64, i64),
) -> String {
    let reply = reserve_reply(router, profile_arn, ground_station, window).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: ReserveContact expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactId")
}

/// The reservations of `held` the rule puts in the answer for `ground_station` over
/// `window` with `types`, in the order the listing must report them.
fn expected(
    held: &[(&'static str, Reservation)],
    ground_station: &str,
    window: (i64, i64),
    types: &[&str],
) -> Vec<Reservation> {
    let mut selected: Vec<Reservation> = held
        .iter()
        .filter(|(_, reservation)| reservation.ground_station_id == ground_station)
        .filter(|(_, reservation)| {
            overlaps((reservation.start_time, reservation.end_time), window)
        })
        .filter(|(_, reservation)| {
            types.is_empty() || types.contains(&reservation.reservation_type.as_str())
        })
        .map(|(_, reservation)| reservation.clone())
        .collect();
    in_listing_order(&mut selected);
    selected
}

#[tokio::test]
async fn the_listing_reports_every_block_of_antenna_time_in_the_window() {
    let (router, held) = stocked_harness().await;

    // A ground station nothing is held at reports nothing: present, empty, and closed.
    let bare = send(
        &router,
        "GET",
        &route("Hawaii-1", WINDOW, &[]),
        None,
    )
    .await;
    assert_eq!(
        bare.status, 200,
        "case no-reservations: expected 200, got {} {}",
        bare.status, bare.body
    );
    assert_eq!(
        bare.field("reservationList"),
        &serde_json::json!([]),
        "case no-reservations: a ground station holding nothing must list nothing, got {}",
        bare.body
    );
    assert!(
        bare.field("nextToken").is_null(),
        "case no-reservations: an exhausted listing hands back no token, got {}",
        bare.body
    );

    // Each ground station the table touches, and each of three windows: the subject
    // window, one a day later that no reservation reaches, and one wide enough to hold
    // every row.
    let windows = [
        ("the-subject-window", WINDOW),
        ("a-window-nothing-reaches", (BASE + 86_400, BASE + 90_000)),
        ("a-window-around-everything", (BASE - 86_400, BASE + 86_400)),
    ];
    for (case, window) in windows {
        for ground_station in [STATION, OTHER_STATION] {
            let case = format!("{case}-at-{ground_station}");
            let reported = reservations(&router, ground_station, window, &[]).await;
            assert_eq!(
                reported,
                expected(&held, ground_station, window, &[]),
                "case {case}: the listing must report exactly the blocks of antenna time \
                 held at this ground station whose window overlaps the requested one, got \
                 {reported:#?}"
            );
        }
    }
}

#[tokio::test]
async fn the_type_filter_selects_the_reservation_types_it_names() {
    let (router, held) = stocked_harness().await;

    // Rows differ along which of the model's two `ReservationType` values the request
    // names: neither (no filtering), each one alone, and both in either order.
    let filters: [(&str, &[&str]); 5] = [
        ("no-filter", &[]),
        ("contact-only", &[CONTACT]),
        ("maintenance-only", &[MAINTENANCE]),
        ("both-types", &[CONTACT, MAINTENANCE]),
        ("both-types-reversed", &[MAINTENANCE, CONTACT]),
    ];
    for (case, types) in filters {
        let reported = reservations(&router, STATION, WINDOW, types).await;
        assert_eq!(
            reported,
            expected(&held, STATION, WINDOW, types),
            "case {case}: the listing must report exactly the reservation types the \
             request names, got {reported:#?}"
        );
    }

    // A value outside the modelled `ReservationType` enum fails the model's own rules, so
    // it is refused before the handler rather than quietly selecting nothing.
    let bogus = send(
        &router,
        "GET",
        &route(STATION, WINDOW, &["SCHEDULED"]),
        None,
    )
    .await;
    assert_eq!(
        bogus.status, 400,
        "case a-type-outside-the-enum: expected a refusal, got {} {}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_code(),
        BAD_REQUEST_ERROR_CODE,
        "case a-type-outside-the-enum: expected {BAD_REQUEST_ERROR_CODE}, got {}",
        bogus.body
    );
}

#[tokio::test]
async fn only_a_contact_still_holding_antenna_time_holds_a_reservation() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "status table", 60, None, None).await;
    let window = (BASE, BASE + 1800);
    let contact_id = reserve_pass_at(&router, &profile_arn, STATION, "status table", window).await;

    for (status, holds) in STATUS_HOLDS_ANTENNA_TIME {
        // Most of these statuses are reached by the scheduler and the pass itself, which
        // no request drives, so the stored contact's status is set directly — the one
        // place this suite reads the store instead of the front door.
        {
            let state = service_state(&*engine).expect("this service's state store");
            let mut guard = state.lock();
            let contact = guard
                .contacts
                .get_mut(&contact_id)
                .expect("the reserved contact");
            contact.contact_status = (*status).to_string();
            // The move is applied, so no clock-owed move can take the status back.
            contact.scheduling_settles_at = None;
            contact.cancellation_settles_at = None;
        }
        let reported = reservations(&router, STATION, WINDOW, &[]).await;
        let expected_count = usize::from(*holds);
        assert_eq!(
            reported.len(),
            expected_count,
            "case {status}: a contact in this status holds {expected_count} reservation(s) \
             at the ground station, got {reported:#?}"
        );
        if *holds {
            assert_eq!(
                reported[0],
                Reservation {
                    reservation_type: CONTACT.to_string(),
                    ground_station_id: STATION.to_string(),
                    antenna_name: antenna_name(1),
                    start_time: window.0,
                    end_time: window.1,
                    details: format!("contact={contact_id}"),
                },
                "case {status}: the reservation must name the contact that holds it, got \
                 {reported:#?}"
            );
        }
    }
}

#[tokio::test]
async fn cancelling_a_contact_releases_its_reservation_and_frees_the_antenna() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "release", 60, None, None).await;
    let window = (BASE, BASE + 1800);
    let contact_id = reserve_pass_at(&router, &profile_arn, STATION, "release", window).await;

    let before = reservations(&router, STATION, WINDOW, &[]).await;
    assert_eq!(
        before.len(),
        1,
        "case before-cancelling: the reserved pass holds one reservation, got {before:#?}"
    );
    assert_eq!(
        before[0].antenna_name,
        antenna_name(1),
        "case before-cancelling: the first pass at a ground station takes the first \
         antenna, got {before:#?}"
    );

    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancelling: CancelContact expected 200, got {} {}",
        cancelled.status, cancelled.body
    );

    let after = reservations(&router, STATION, WINDOW, &[]).await;
    assert!(
        after.is_empty(),
        "case after-cancelling: a released reservation holds no antenna time, got \
         {after:#?}"
    );

    // The antenna the released reservation held is free again, so the next pass over the
    // same window takes it rather than the one beside it.
    let successor =
        reserve_pass_at(&router, &profile_arn, STATION, "after-cancelling", window).await;
    let reused = reservations(&router, STATION, WINDOW, &[]).await;
    assert_eq!(
        reused,
        vec![Reservation {
            reservation_type: CONTACT.to_string(),
            ground_station_id: STATION.to_string(),
            antenna_name: antenna_name(1),
            start_time: window.0,
            end_time: window.1,
            details: format!("contact={successor}"),
        }],
        "case after-cancelling: the freed antenna takes the next pass, got {reused:#?}"
    );
}

#[tokio::test]
async fn an_antenna_holds_one_reservation_at_a_time() {
    let (router, engine) = harness_with_engine();
    // The first antenna of this ground station is out of service for the first half of the
    // table's windows, so the placement starts from the second one there.
    schedule_maintenance(
        &engine,
        &format!("{STATION}/Antenna 1/UNPLANNED/{}/{}", BASE, BASE + 900),
    );
    // Four antennas at the subject station: what it HAS is what bounds how many
    // overlapping passes it can hold, and the table reserves four at once.
    stock_inventory(&engine, &inventory_setting(4));
    let profile_arn = create_mission_profile(&router, "placement", 1, None, None).await;

    // Rows differ along how each window relates to the ones reserved before it: the first
    // pass, one overlapping it, one overlapping both, one touching an earlier end exactly,
    // one disjoint from everything, and one at a ground station holding nothing.
    let rows: [(&str, &str, (i64, i64), usize); 6] = [
        ("first-pass-of-the-day", STATION, (BASE, BASE + 600), 2),
        ("overlaps-the-first", STATION, (BASE + 300, BASE + 900), 3),
        ("overlaps-both", STATION, (BASE + 400, BASE + 500), 4),
        ("touches-the-first-pass-end", STATION, (BASE + 600, BASE + 700), 4),
        ("disjoint-from-everything", STATION, (BASE + 1200, BASE + 1800), 1),
        ("another-ground-station", OTHER_STATION, (BASE, BASE + 600), 1),
    ];

    for (case, ground_station, window, antenna) in rows {
        let contact_id =
            reserve_pass_at(&router, &profile_arn, ground_station, case, window).await;
        let reported = reservations(&router, ground_station, window, &[CONTACT]).await;
        let placed = reported
            .iter()
            .find(|reservation| reservation.details == format!("contact={contact_id}"))
            .unwrap_or_else(|| panic!("case {case}: the new reservation is not reported"));
        assert_eq!(
            placed.antenna_name,
            antenna_name(antenna),
            "case {case}: an antenna holds one reservation at a time, so this pass belongs \
             on {}, got {placed:#?}",
            antenna_name(antenna)
        );
    }

    // A window every antenna of the station is already held for has no antenna time left
    // to reserve, so the reservation is refused and nothing is written: the first antenna
    // is under maintenance and the other three hold the rows above.
    let window = (BASE + 450, BASE + 550);
    let before = reservations(&router, STATION, window, &[]).await;
    let refused = reserve_reply(&router, &profile_arn, STATION, window).await;
    assert_eq!(
        refused.status, 431,
        "case every-antenna-held: expected the modelled InvalidParameterException status \
         431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterException",
        "case every-antenna-held: wrong error shape, got {}",
        refused.body
    );
    assert!(
        refused.body.get("contactId").is_none(),
        "case every-antenna-held: a refused reservation mints no contactId, got {}",
        refused.body
    );
    let after = reservations(&router, STATION, window, &[]).await;
    assert_eq!(
        after, before,
        "case every-antenna-held: a refused reservation holds no antenna time, got \
         {after:#?}"
    );

    // Nothing at the ground station shares an antenna with an overlapping reservation.
    let whole_day = reservations(&router, STATION, (BASE - 86_400, BASE + 86_400), &[]).await;
    for (index, left) in whole_day.iter().enumerate() {
        for right in &whole_day[index + 1..] {
            assert!(
                left.antenna_name != right.antenna_name
                    || !overlaps(
                        (left.start_time, left.end_time),
                        (right.start_time, right.end_time)
                    ),
                "case one-pass-per-antenna: {left:?} and {right:?} overlap on one antenna"
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_through_its_reservations() {
    let (router, held) = stocked_harness().await;
    let whole = expected(&held, STATION, WINDOW, &[]);
    assert!(
        whole.len() > 2,
        "case whole-listing: the table must hold enough reservations to page over, got \
         {whole:#?}"
    );

    for size in [1_i64, 2, 3, 5, 6, 7, 100] {
        let paged: Vec<Reservation> = list_pages(
            &router,
            &route(STATION, WINDOW, &[]),
            "reservationList",
            Some(size),
        )
        .await
        .iter()
        .map(reservation)
        .collect();
        assert_eq!(
            paged, whole,
            "case maxResults={size}: paging must yield every reservation exactly once, in \
             the listing's order, got {paged:#?}"
        );
    }

    // Each published bound of `PaginationMaxResults` and one value past it (source:
    // `smithy_query(query="operation", operation="ListGroundStationReservations")` — range
    // 1..100). A value the model refuses never reaches the handler.
    for (case, size, accepted) in [
        ("the lower bound", 1_i64, true),
        ("the upper bound", 100, true),
        ("one below the lower bound", 0, false),
        ("one above the upper bound", 101, false),
    ] {
        let reply = send(
            &router,
            "GET",
            &format!("{}&maxResults={size}", route(STATION, WINDOW, &[])),
            None,
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 200,
                "case maxResults={size} ({case}): expected 200, got {} {}",
                reply.status, reply.body
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case maxResults={size} ({case}): expected a refusal, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                BAD_REQUEST_ERROR_CODE,
                "case maxResults={size} ({case}): expected {BAD_REQUEST_ERROR_CODE}, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn the_listing_refuses_a_next_token_it_did_not_issue() {
    let (router, _held) = stocked_harness().await;

    let own_token = send(
        &router,
        "GET",
        &format!("{}&maxResults=1", route(STATION, WINDOW, &[])),
        None,
    )
    .await
    .field("nextToken")
    .as_str()
    .map(str::to_string)
    .expect("a capped page hands back a token");

    // An offset counts within the FILTERED sequence, so a token is only good for the scope
    // it was issued under: the same listing under another ground station, another window
    // or another type filter refuses it, as does a token this listing never issued at all.
    let refused: [(&str, String); 5] = [
        (
            "another-ground-station",
            format!(
                "{}&maxResults=1&nextToken={own_token}",
                route(OTHER_STATION, WINDOW, &[])
            ),
        ),
        (
            "another-window",
            format!(
                "{}&maxResults=1&nextToken={own_token}",
                route(STATION, (BASE, BASE + 7200), &[])
            ),
        ),
        (
            "another-type-filter",
            format!(
                "{}&maxResults=1&nextToken={own_token}",
                route(STATION, WINDOW, &[CONTACT])
            ),
        ),
        (
            "a-token-this-service-never-issued",
            format!(
                "{}&maxResults=1&nextToken=not-a-token",
                route(STATION, WINDOW, &[])
            ),
        ),
        (
            "a-token-from-another-listing",
            format!(
                "{}&maxResults=1&nextToken=satellites.1",
                route(STATION, WINDOW, &[])
            ),
        ),
    ];
    for (case, path) in refused {
        let reply = send(&router, "GET", &path, None).await;
        assert_eq!(
            reply.status, 431,
            "case {case}: expected the modelled 431 refusal, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
            reply.body
        );
    }

    // The token the listing DID issue resumes it.
    let resumed = send(
        &router,
        "GET",
        &format!(
            "{}&maxResults=1&nextToken={own_token}",
            route(STATION, WINDOW, &[])
        ),
        None,
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case own-token: expected 200, got {} {}",
        resumed.status, resumed.body
    );
}

#[tokio::test]
async fn a_window_that_runs_backwards_is_refused() {
    let (router, _held) = stocked_harness().await;

    // Rows differ along the window: one of zero length (the same instant twice, a window
    // that still describes an interval) and two that run backwards by a second and by an
    // hour.
    let rows: [(&str, (i64, i64), u16); 3] = [
        ("a-zero-length-window", (BASE, BASE), 200),
        ("backwards-by-a-second", (BASE, BASE - 1), 431),
        ("backwards-by-an-hour", (BASE + 3600, BASE), 431),
    ];
    for (case, window, status) in rows {
        let reply = send(&router, "GET", &route(STATION, window, &[]), None).await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected {status}, got {} {}",
            reply.status, reply.body
        );
        if status != 200 {
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {case}: wrong error shape, got {}",
                reply.body
            );
            assert_eq!(
                reply.string("parameterName"),
                "endTime",
                "case {case}: the refusal must name the member that is wrong, got {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn the_operator_setting_reads_the_antennas_out_of_service() {
    // Rows differ along the form of the setting: one window, several, surrounding
    // whitespace, names at the published length bounds of `GroundStationName` (4..97) and
    // `AntennaName` (4..256), and entries that are not a ground station, an antenna, a
    // `MaintenanceType` and a forwards-running window — each of which takes nothing out of
    // service rather than something unintended.
    let shortest_station = "Ohio";
    let longest_station = "O".repeat(97);
    let longest_antenna = "A".repeat(256);
    let rows: Vec<(&str, String, usize)> = vec![
        (
            "one-window",
            "Ohio 1/Antenna 1/PLANNED/100/200".to_string(),
            1,
        ),
        (
            "several-windows",
            "Ohio 1/Antenna 1/PLANNED/100/200,Ohio 1/Antenna 2/UNPLANNED/300/400".to_string(),
            2,
        ),
        (
            "surrounding-whitespace",
            "  Ohio 1 / Antenna 1 / PLANNED / 100 / 200  ".to_string(),
            1,
        ),
        (
            "names-at-their-shortest",
            format!("{shortest_station}/Ante/PLANNED/100/200"),
            1,
        ),
        (
            "names-at-their-longest",
            format!("{longest_station}/{longest_antenna}/PLANNED/100/200"),
            1,
        ),
        (
            "a-ground-station-one-character-too-long",
            format!("{}/Antenna 1/PLANNED/100/200", "O".repeat(98)),
            0,
        ),
        (
            "a-ground-station-one-character-too-short",
            "Ohi/Antenna 1/PLANNED/100/200".to_string(),
            0,
        ),
        (
            "an-antenna-one-character-too-long",
            format!("Ohio 1/{}/PLANNED/100/200", "A".repeat(257)),
            0,
        ),
        ("unset", String::new(), 0),
        (
            "a-maintenance-type-outside-the-enum",
            "Ohio 1/Antenna 1/ROUTINE/100/200".to_string(),
            0,
        ),
        (
            "a-window-that-is-not-epoch-seconds",
            "Ohio 1/Antenna 1/PLANNED/noon/midnight".to_string(),
            0,
        ),
        (
            "a-window-that-runs-backwards",
            "Ohio 1/Antenna 1/PLANNED/200/100".to_string(),
            0,
        ),
        (
            "a-window-of-zero-length",
            "Ohio 1/Antenna 1/PLANNED/100/100".to_string(),
            0,
        ),
        (
            "too-few-fields",
            "Ohio 1/Antenna 1/PLANNED/100".to_string(),
            0,
        ),
        (
            "too-many-fields",
            "Ohio 1/Antenna 1/PLANNED/100/200/300".to_string(),
            0,
        ),
        (
            "one-good-entry-beside-a-bad-one",
            "Ohio 1/Antenna 1/PLANNED/200/100,Ohio 1/Antenna 2/PLANNED/100/200".to_string(),
            1,
        ),
    ];

    for (case, setting, count) in rows {
        let windows = parse_maintenance(&setting);
        assert_eq!(
            windows.len(),
            count,
            "case {case}: expected {count} maintenance window(s) from {setting:?}, got \
             {windows:#?}"
        );
    }

    // The window the setting describes is the one a handler reads, member by member.
    let read = parse_maintenance("Ohio 1/Antenna 4/UNPLANNED/1800000600/1800001200");
    let window = read.first().expect("one maintenance window");
    assert_eq!(
        (
            window.ground_station.as_str(),
            window.antenna_name.as_str(),
            window.maintenance_type.as_str(),
            window.start_time.as_secs_f64() as i64,
            window.end_time.as_secs_f64() as i64,
        ),
        ("Ohio 1", "Antenna 4", "UNPLANNED", 1_800_000_600, 1_800_001_200),
        "case one-window-read-back: the setting's own values must land in the store, got \
         {window:#?}"
    );
}

/// A ground station whose name carries a space — the character set a `GroundStationName`
/// is spelled from includes one (`smithy_query(query="shape",
/// shape="GroundStationName")`), and the name is an `@httpLabel`, so it reaches the
/// handler percent-encoded.
#[tokio::test]
async fn a_ground_station_name_carrying_a_space_addresses_its_own_reservations() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "spaced", 60, None, None).await;
    let window = (BASE, BASE + 1800);
    let contact_id = reserve_pass_at(&router, &profile_arn, "Ohio 1", "spaced", window).await;

    let reported = reservations(&router, "Ohio%201", WINDOW, &[]).await;
    assert_eq!(
        reported,
        vec![Reservation {
            reservation_type: CONTACT.to_string(),
            ground_station_id: "Ohio 1".to_string(),
            antenna_name: antenna_name(1),
            start_time: window.0,
            end_time: window.1,
            details: format!("contact={contact_id}"),
        }],
        "case spaced-name: the listing must report the reservation held at the ground \
         station the label names, got {reported:#?}"
    );
}

/// A listing is a REPORT: two identical calls answer the same thing, and nothing a later
/// caller sees changed because somebody read.
#[tokio::test]
async fn the_listing_changes_nothing_a_later_caller_sees() {
    let (router, held) = stocked_harness().await;
    let first = reservations(&router, STATION, WINDOW, &[]).await;
    // Long enough for every clock-owed move this service makes due, so a second read
    // taken after one cannot differ because a status move landed between them.
    std::thread::sleep(crate::scheduling_loop::AFTER_SETTLE);
    let second = reservations(&router, STATION, WINDOW, &[]).await;
    assert_eq!(
        first,
        expected(&held, STATION, WINDOW, &[]),
        "case first-read: the listing must report the blocks held in the window, got \
         {first:#?}"
    );
    assert_eq!(
        second, first,
        "case second-read: a reporting operation must answer the same twice, got \
         {second:#?}"
    );
}

/// One row of the placement table below: the case, the ground station the pass is reserved
/// at, its window, and the antenna the rule puts it on — `None` where the rule refuses the
/// reservation because every antenna of that station is already held for the window.
type PlacementRow = (&'static str, &'static str, (i64, i64), Option<&'static str>);

#[tokio::test]
async fn a_pass_is_placed_on_an_antenna_its_ground_station_has() {
    let (router, engine) = harness_with_engine();
    // The antennas this account has been told are at the ground station, spelled unlike
    // the names the emulator numbers itself, so which list a placement came from is
    // visible. Seeded where `state::GROUND_STATIONS_ENV` lands them — the inventory
    // `ListAntennas` reports.
    {
        let state = service_state(&*engine).expect("this service's state store");
        state.lock().ground_stations = parse_ground_stations(&format!("{STATION}=Dish B+Dish A"));
    }
    let profile_arn = create_mission_profile(&router, "inventory", 1, None, None).await;

    // Rows differ along how each window relates to the ones reserved before it, and along
    // whether their ground station has a recorded inventory at all: the first pass and one
    // overlapping it take the station's own antennas in name order, a third overlapping
    // both is REFUSED (the station has two antennas and both are held, so the window holds
    // no antenna time left to reserve), one disjoint from everything takes the station's
    // first antenna again, and one at a station this account was told nothing about takes
    // the single antenna the emulator numbers for it. `None` is the refused row.
    let rows: [PlacementRow; 5] = [
        (
            "first-pass-takes-the-first-antenna",
            STATION,
            (BASE, BASE + 600),
            Some("Dish A"),
        ),
        (
            "overlapping-pass-takes-the-next",
            STATION,
            (BASE + 300, BASE + 900),
            Some("Dish B"),
        ),
        (
            "every-recorded-antenna-held",
            STATION,
            (BASE + 400, BASE + 500),
            None,
        ),
        (
            "disjoint-pass-reuses-the-first-antenna",
            STATION,
            (BASE + 1200, BASE + 1800),
            Some("Dish A"),
        ),
        (
            "a-station-with-no-recorded-inventory",
            OTHER_STATION,
            (BASE, BASE + 600),
            Some("Antenna 1"),
        ),
    ];

    for (case, ground_station, window, antenna) in rows {
        let reply = reserve_reply(&router, &profile_arn, ground_station, window).await;
        let Some(antenna) = antenna else {
            assert_eq!(
                reply.status, 431,
                "case {case}: a window every antenna of the station is held for must be \
                 refused, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "InvalidParameterException",
                "case {case}: wrong error shape, got {}",
                reply.body
            );
            continue;
        };
        assert_eq!(
            reply.status, 200,
            "case {case}: ReserveContact expected 200, got {} {}",
            reply.status, reply.body
        );
        let contact_id = reply.string("contactId");
        let reported = reservations(&router, ground_station, window, &[CONTACT]).await;
        let placed = reported
            .iter()
            .find(|reservation| reservation.details == format!("contact={contact_id}"))
            .unwrap_or_else(|| panic!("case {case}: the new reservation is not reported"));
        assert_eq!(
            placed.antenna_name, antenna,
            "case {case}: a pass is placed on the first free antenna its ground station \
             has, expected {antenna:?}, got {placed:#?}"
        );
    }
}

#[tokio::test]
async fn the_window_binds_in_the_epoch_seconds_form_the_model_declares() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "wire form", 60, None, None).await;
    let window = (BASE, BASE + 1800);
    reserve_pass_at(&router, &profile_arn, STATION, "wire form", window).await;

    /// `instant` as the RFC-3339 `date-time` string a query string carries it as (`:` is
    /// the one character the rendering carries that a query string reserves).
    fn date_time(instant: i64) -> String {
        chrono::DateTime::from_timestamp(instant, 0)
            .unwrap_or_else(|| panic!("{instant} is an epoch-seconds instant"))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
            .replace(':', "%3A")
    }

    // Rows differ along the FORM of the two bound timestamps: the `epoch-seconds` form the
    // members declare (`smithy_query(query="operation",
    // operation="ListGroundStationReservations")` — `"timestampFormat": "epoch-seconds"`),
    // the same instant with a fractional part, the protocol's default RFC-3339
    // `date-time`, and three forms that name no instant at all.
    let rows: Vec<(&str, String, String, u16)> = vec![
        (
            "epoch-seconds",
            WINDOW.0.to_string(),
            WINDOW.1.to_string(),
            200,
        ),
        (
            "epoch-seconds-with-a-fraction",
            format!("{}.25", WINDOW.0),
            format!("{}.75", WINDOW.1),
            200,
        ),
        (
            "rfc-3339-date-time",
            date_time(WINDOW.0),
            date_time(WINDOW.1),
            200,
        ),
        (
            "scientific-notation",
            "1.8e9".to_string(),
            "1.8000036e9".to_string(),
            400,
        ),
        (
            "not-a-number-at-all",
            "noon".to_string(),
            "midnight".to_string(),
            400,
        ),
        ("an-empty-value", String::new(), String::new(), 400),
    ];

    for (case, start, end, status) in rows {
        let reply = send(
            &router,
            "GET",
            &format!("/groundstation/{STATION}/reservation?startTime={start}&endTime={end}"),
            None,
        )
        .await;
        assert_eq!(
            reply.status, status,
            "case {case}: expected {status} for startTime={start:?} endTime={end:?}, got {} {}",
            reply.status, reply.body
        );
        if status == 200 {
            let reported: Vec<Reservation> = reply
                .field("reservationList")
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: reservationList must be a list"))
                .iter()
                .map(reservation)
                .collect();
            assert_eq!(
                reported.len(),
                1,
                "case {case}: every accepted form names the same window, which holds one \
                 reservation, got {reported:#?}"
            );
            assert_eq!(
                (reported[0].start_time, reported[0].end_time),
                window,
                "case {case}: the reservation's own window is reported whatever form the \
                 request's was in, got {reported:#?}"
            );
        }
    }
}

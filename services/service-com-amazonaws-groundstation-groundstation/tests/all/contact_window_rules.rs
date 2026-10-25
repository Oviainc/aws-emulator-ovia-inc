// SPDX-License-Identifier: CC0-1.0

//! What `ReserveContact` will and will not reserve, end to end over HTTP.
//!
//! The RULE under test: a contact RESERVES a ground station antenna for its window, so a
//! request is refused — with the operation's modelled `InvalidParameterException` (HTTP
//! 431), minting no id and writing nothing — when the window holds no antenna time it
//! could reserve:
//!
//! * the window's antenna time has already elapsed (the clock is past its `endTime`), and
//! * every antenna the ground station has is already held for a window overlapping it, by
//!   another contact of this account that still holds antenna time there or by an antenna
//!   AWS has taken out of service.
//!
//! A ground station's antennas are FINITE, which is what makes the second condition
//! reachable: the antennas this account has been told are at the station, or the single one
//! the emulator numbers for a station it was told nothing about.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table of windows differing in where they sit relative to the clock — last year,
//!   hours ago, ending just now, straddling the clock, later today and next month — with
//!   the refused rows asserted to leave neither a contact nor a scheduled minute behind;
//! * a table of windows differing in how they overlap a pass already held at the station —
//!   the same window, starting inside it, ending inside it, containing it, contained by it,
//!   touching either end exactly, and disjoint on either side — each expected in or out by
//!   the overlap rule itself;
//! * a table of ground stations differing in how many antennas they have, each holding
//!   exactly that many overlapping passes, one per antenna, and refusing the next;
//! * an antenna out of service for the window, which holds it against a reservation;
//! * the release: a cancelled contact holds nothing, so the window it held is reservable
//!   again, and the reservation reads back through `DescribeContact`.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    antenna_name, parse_ground_stations, service_state,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::minute_usage::{minute_usage, minutes, month_of};
use crate::scheduling_loop::{create_mission_profile, harness_with_engine, send, Reply};

/// The ground station these suites reserve at, and a second one used where two passes have
/// to be held at the same instant.
const STATION: &str = "Ohio-1";

/// An instant in the future every window of the overlap table is measured from: a round
/// epoch-seconds value, as the other suites of this target use.
const BASE: i64 = 1_800_000_000;

/// The status the operation's modelled bad-parameter refusal carries
/// (`smithy_query(query="operation", operation="ReserveContact")` — `InvalidParameterException`,
/// HTTP 431).
const REFUSED_STATUS: u16 = 431;

/// See [`REFUSED_STATUS`].
const REFUSED_ERROR: &str = "InvalidParameterException";

/// The clock, in epoch seconds — the instant the rule about elapsed antenna time is
/// measured against.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch-seconds instant fits in i64")
}

/// Whether two windows, each `(start, end)` in epoch seconds, share any instant — the rule
/// the placement applies, written here independently of the handler so the expectations
/// below come from the rule rather than from the answer. CLOSED at both ends, so two
/// windows that merely touch share an instant.
fn overlaps(first: (i64, i64), second: (i64, i64)) -> bool {
    first.1 >= second.0 && first.0 <= second.1
}

/// Record `count` antennas at `ground_station`, as a handler sees them — seeded where
/// `state::GROUND_STATIONS_ENV` lands them, which is how an account is told which antennas
/// a ground station has.
fn stock_inventory(engine: &engine::InMemoryEngine, ground_station: &str, count: usize) {
    let names: Vec<String> = (1..=count).map(antenna_name).collect();
    let inventory = parse_ground_stations(&format!("{ground_station}={}", names.join("+")));
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .ground_stations = inventory;
}

/// Take `antenna` at `ground_station` out of service over `window`, as a handler sees it —
/// seeded where `state::MAINTENANCE_ENV` lands it, since no operation of this API schedules
/// maintenance.
fn schedule_maintenance(
    engine: &engine::InMemoryEngine,
    ground_station: &str,
    antenna: &str,
    window: (i64, i64),
) {
    let windows = service_com_amazonaws_groundstation_groundstation::state::parse_maintenance(
        &format!(
            "{ground_station}/{antenna}/PLANNED/{}/{}",
            window.0, window.1
        ),
    );
    assert_eq!(
        windows.len(),
        1,
        "the maintenance setting must describe one window, got {windows:?}"
    );
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .maintenance_reservations = windows;
}

/// `ReserveContact` for `window` at `ground_station`, answering the reply as it came.
async fn reserve(
    router: &MultiServiceRouter,
    profile_arn: &str,
    ground_station: &str,
    window: (i64, i64),
) -> Reply {
    send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": ground_station,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await
}

/// `ReserveContact` for `window`, asserting it was accepted and answering the `contactId`.
async fn reserve_ok(
    router: &MultiServiceRouter,
    profile_arn: &str,
    ground_station: &str,
    case: &str,
    window: (i64, i64),
) -> String {
    let reply = reserve(router, profile_arn, ground_station, window).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: ReserveContact expected 200, got {} {}",
        reply.status, reply.body
    );
    reply.string("contactId")
}

/// Assert `reply` is the operation's modelled bad-parameter refusal naming `parameter`,
/// carrying a message and no minted id.
fn assert_refused(reply: &Reply, case: &str, parameter: &str) {
    assert_eq!(
        reply.status, REFUSED_STATUS,
        "case {case}: expected the modelled {REFUSED_ERROR} status {REFUSED_STATUS}, got \
         {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSED_ERROR,
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.field("parameterName"),
        &json!(parameter),
        "case {case}: the refusal must name the parameter at fault, got {}",
        reply.body
    );
    assert!(
        reply
            .body
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "case {case}: the refusal must carry a message, got {}",
        reply.body
    );
    assert!(
        reply.body.get("contactId").is_none(),
        "case {case}: a refused reservation mints no contactId, got {}",
        reply.body
    );
}

/// Every `contactId` `ListContacts` reports over `window`, in whatever order it answered.
async fn listed_contacts(router: &MultiServiceRouter, window: (i64, i64)) -> Vec<String> {
    let reply = send(
        router,
        "POST",
        "/contacts",
        Some(json!({
            "statusList": ["SCHEDULING", "SCHEDULED", "CANCELLED"],
            "startTime": window.0,
            "endTime": window.1,
            "maxResults": 100,
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ListContacts expected 200, got {} {}",
        reply.status, reply.body
    );
    let mut ids: Vec<String> = reply
        .field("contactList")
        .as_array()
        .expect("contactList is a list")
        .iter()
        .map(|item| {
            item.get("contactId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The antennas `ListGroundStationReservations` reports held at `ground_station` over
/// `window`, sorted.
async fn held_antennas(
    router: &MultiServiceRouter,
    ground_station: &str,
    window: (i64, i64),
) -> Vec<String> {
    let reply = send(
        router,
        "GET",
        &format!(
            "/groundstation/{ground_station}/reservation?startTime={}&endTime={}&maxResults=100",
            window.0, window.1
        ),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "ListGroundStationReservations expected 200, got {} {}",
        reply.status, reply.body
    );
    let mut antennas: Vec<String> = reply
        .field("reservationList")
        .as_array()
        .expect("reservationList is a list")
        .iter()
        .map(|item| {
            item.get("antennaName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    antennas.sort();
    antennas
}

#[tokio::test]
async fn a_window_whose_antenna_time_has_elapsed_is_not_reservable() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "elapsed windows", 60, None, None).await;
    let now = now_seconds();

    // Rows differ along where the window sits relative to the clock: wholly over (a year
    // ago, hours ago, and one that ended as this test started), still running (it began
    // before the clock and ends after it), and wholly ahead (later today, next month). The
    // accepted rows are disjoint from each other, since an antenna holds one pass at a
    // time.
    let rows: [(&str, (i64, i64), bool); 6] = [
        ("a-window-last-year", (now - 400 * 86_400, now - 400 * 86_400 + 600), false),
        ("a-window-hours-ago", (now - 4 * 3600, now - 3 * 3600), false),
        ("a-window-that-just-ended", (now - 600, now), false),
        ("a-window-still-running", (now - 600, now + 300), true),
        ("a-window-later-today", (now + 3600, now + 4200), true),
        ("a-window-next-month", (now + 30 * 86_400, now + 30 * 86_400 + 600), true),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (case, window, reservable) in rows {
        let reply = reserve(&router, &profile_arn, STATION, window).await;
        if reservable {
            assert_eq!(
                reply.status, 200,
                "case {case}: a window with antenna time left in it is reservable, got {} {}",
                reply.status, reply.body
            );
            accepted.push(reply.string("contactId"));
        } else {
            assert_refused(&reply, case, "endTime");
        }
    }

    // Nothing the refused rows asked for was written: the contacts the store holds over
    // every window the table touches are exactly the accepted ones.
    accepted.sort();
    let listed = listed_contacts(
        &router,
        (now - 401 * 86_400, now + 31 * 86_400),
    )
    .await;
    assert_eq!(
        listed, accepted,
        "case nothing-written: only the accepted reservations may be stored, got {listed:?}"
    );

    // And a refused window counts no antenna time for its month: the month a year back
    // holds none of the accepted rows, so its usage is the refused row's alone — zero.
    let elapsed_month = month_of(now - 400 * 86_400);
    let usage = minute_usage(&router, elapsed_month).await;
    assert_eq!(
        minutes(&usage, "totalScheduledMinutes", "elapsed-month"),
        0,
        "case elapsed-month: a refused reservation schedules no antenna time, got {}",
        usage.body
    );
}

#[tokio::test]
async fn a_window_an_antenna_already_holds_is_not_reservable() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "overlapping windows", 60, None, None).await;

    // The pass the station's one antenna holds. The station has no recorded inventory, so
    // it has the single antenna this emulator numbers for it.
    let held = (BASE, BASE + 600);
    let first = reserve_ok(&router, &profile_arn, STATION, "the-held-pass", held).await;

    // Rows differ along how the window relates to the one already held: the same window,
    // starting inside it, ending inside it, containing it, contained by it, touching each
    // end exactly, and disjoint on either side with a gap. Whether each is reservable comes
    // from the overlap rule itself, not from a recorded answer.
    let rows: [(&str, (i64, i64)); 9] = [
        ("the-same-window", held),
        ("starts-inside-it", (BASE + 300, BASE + 900)),
        ("ends-inside-it", (BASE - 300, BASE + 300)),
        ("contains-it", (BASE - 300, BASE + 900)),
        ("contained-by-it", (BASE + 100, BASE + 500)),
        ("touches-its-end", (BASE + 600, BASE + 1200)),
        ("touches-its-start", (BASE - 600, BASE)),
        ("disjoint-before-it", (BASE - 1800, BASE - 1200)),
        ("disjoint-after-it", (BASE + 1200, BASE + 1800)),
    ];

    let mut accepted = vec![first];
    for (case, window) in rows {
        let reply = reserve(&router, &profile_arn, STATION, window).await;
        if overlaps(window, held) {
            assert_refused(&reply, case, "startTime");
        } else {
            assert_eq!(
                reply.status, 200,
                "case {case}: this window shares no instant with the one held, so it is \
                 reservable, got {} {}",
                reply.status, reply.body
            );
            accepted.push(reply.string("contactId"));
        }
    }

    accepted.sort();
    let listed = listed_contacts(&router, (BASE - 86_400, BASE + 86_400)).await;
    assert_eq!(
        listed, accepted,
        "case nothing-written: a refused reservation stores no contact, got {listed:?}"
    );
}

#[tokio::test]
async fn the_antennas_a_ground_station_has_bound_the_passes_it_holds_at_once() {
    // Rows differ along how many antennas the ground station has: one it was told nothing
    // about, and inventories of one, two, three and five antennas. An inventory is at most
    // 100 antennas long (`smithy_query(query="shape", shape="AntennaList")` — len 0..100),
    // so every row is inside that bound.
    let rows: [(&str, Option<usize>, usize); 5] = [
        ("no-recorded-inventory", None, 1),
        ("one-antenna", Some(1), 1),
        ("two-antennas", Some(2), 2),
        ("three-antennas", Some(3), 3),
        ("five-antennas", Some(5), 5),
    ];

    for (case, inventory, capacity) in rows {
        // A store of its own per row, so what one row holds is not what the next sees.
        let (router, engine) = harness_with_engine();
        if let Some(count) = inventory {
            stock_inventory(&engine, STATION, count);
        }
        let profile_arn = create_mission_profile(&router, "capacity", 1, None, None).await;

        // As many overlapping passes as the station has antennas, each on an antenna of its
        // own: every window of the row contains the instant `BASE + 600`.
        let subject = (BASE, BASE + 600);
        for index in 0..capacity {
            let offset = i64::try_from(index).expect("a row index fits in i64") * 10;
            reserve_ok(
                &router,
                &profile_arn,
                STATION,
                &format!("{case}/pass-{index}"),
                (BASE + offset, BASE + 600),
            )
            .await;
        }
        let antennas = held_antennas(&router, STATION, subject).await;
        assert_eq!(
            antennas.len(),
            capacity,
            "case {case}: the station must hold its {capacity} overlapping passes, got \
             {antennas:?}"
        );
        let mut distinct = antennas.clone();
        distinct.dedup();
        assert_eq!(
            distinct, antennas,
            "case {case}: an antenna holds one pass at a time, so each pass is on an \
             antenna of its own, got {antennas:?}"
        );

        // One more overlapping pass has no antenna left to go on, so it is refused.
        let refused = reserve(&router, &profile_arn, STATION, (BASE + 300, BASE + 500)).await;
        assert_refused(&refused, &format!("{case}/one-pass-too-many"), "startTime");
        assert_eq!(
            held_antennas(&router, STATION, subject).await,
            antennas,
            "case {case}: a refused reservation holds no antenna"
        );

        // A window the station holds nothing for is still reservable, however full the
        // subject window is.
        reserve_ok(
            &router,
            &profile_arn,
            STATION,
            &format!("{case}/another-window"),
            (BASE + 7200, BASE + 7800),
        )
        .await;
    }
}

#[tokio::test]
async fn an_antenna_out_of_service_holds_it_against_a_reservation() {
    let (router, engine) = harness_with_engine();
    // The station's only antenna is out of service for the first window, and nothing holds
    // it for the second.
    let under_maintenance = (BASE, BASE + 3600);
    schedule_maintenance(&engine, STATION, &antenna_name(1), under_maintenance);
    let profile_arn = create_mission_profile(&router, "maintenance", 60, None, None).await;

    let refused = reserve(&router, &profile_arn, STATION, (BASE + 600, BASE + 1200)).await;
    assert_refused(&refused, "inside-the-maintenance-window", "startTime");

    let clear = (BASE + 7200, BASE + 7800);
    let contact_id = reserve_ok(&router, &profile_arn, STATION, "clear-of-it", clear).await;
    assert_eq!(
        held_antennas(&router, STATION, clear).await,
        vec![antenna_name(1)],
        "case clear-of-it: the antenna is free outside the maintenance window"
    );
    assert!(
        !contact_id.is_empty(),
        "case clear-of-it: an accepted reservation carries a contactId"
    );
}

#[tokio::test]
async fn releasing_a_reservation_makes_its_window_reservable_again() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "release", 60, Some(120), Some(60)).await;
    let window = (BASE, BASE + 1800);
    let first = reserve_ok(&router, &profile_arn, STATION, "the-first-pass", window).await;

    // The round trip: the reservation reads back through `DescribeContact` as the window
    // and station it was taken for.
    let described = send(&router, "GET", &format!("/contact/{first}"), None).await;
    assert_eq!(
        described.status, 200,
        "case read-back: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    for (member, expected) in [("startTime", window.0 as f64), ("endTime", window.1 as f64)] {
        let actual = described.field(member).as_f64();
        assert_eq!(
            actual,
            Some(expected),
            "case read-back: {member} must be {expected}, got {actual:?}"
        );
    }
    assert_eq!(
        described.string("groundStation"),
        STATION,
        "case read-back: the contact must be held at the station it was reserved at, got {}",
        described.body
    );

    // While it holds the antenna, an overlapping window is refused.
    let overlapping = (window.0 + 60, window.1 + 60);
    let refused = reserve(&router, &profile_arn, STATION, overlapping).await;
    assert_refused(&refused, "while-held", "startTime");

    let cancelled = send(&router, "DELETE", &format!("/contact/{first}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel: CancelContact expected 200, got {} {}",
        cancelled.status, cancelled.body
    );

    // A released reservation holds nothing, so the same window is reservable again — the
    // refusal is read from what the station holds, not from the window itself.
    let successor = reserve_ok(
        &router,
        &profile_arn,
        STATION,
        "after-the-release",
        overlapping,
    )
    .await;
    assert_ne!(
        successor, first,
        "case after-the-release: the second reservation is a contact of its own"
    );
    assert_eq!(
        held_antennas(&router, STATION, overlapping).await,
        vec![antenna_name(1)],
        "case after-the-release: the freed antenna takes the next pass"
    );
}

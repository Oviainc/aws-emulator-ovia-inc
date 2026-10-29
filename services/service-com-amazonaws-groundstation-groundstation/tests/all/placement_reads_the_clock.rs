// SPDX-License-Identifier: CC0-1.0

//! Which antenna time `ReserveContact` finds free, end to end over HTTP.
//!
//! The RULE under test: a contact's status is whatever the moves the clock had already made
//! due put it in at the instant of the call, so a reservation is PLACED against those
//! statuses — the placement materialises every due move before it reads them, exactly as
//! every reporting operation does. An antenna is therefore free, or held, by the clock
//! alone: two callers asking in either order are answered the same, and no call needs
//! another call before it to be answered correctly.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table of contacts differing in the move the service owed them when the reservation
//!   arrived — a reservation still owed its confirmation and one whose confirmation fell
//!   due, a release that completed outright on either side of the confirmation, and a
//!   stopped pass from each stage of a pass under way, both while its release is still in
//!   flight and once it fell due — each crossed with the ORDER the reservation arrives in:
//!   as the very first call since the move fell due, and after a reporting call that would
//!   itself have settled it. Both orders must answer the same, and the status the store
//!   holds afterwards must be the one the clock owed;
//! * the same rule driven by the pass clock alone, with no status written into the store:
//!   a pass that begins, is stopped mid-pass and whose release falls due, after which the
//!   reservation of its window is the first call the service sees.
//!
//! Each row is judged on a store of its own, and the status the store holds is read
//! directly where that is the only way to observe it without making the very call whose
//! absence is the point — every reporting operation settles the due moves itself, so a read
//! through one could not tell a move the reservation materialised from one the read did.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    antenna_name, service_state, CONTACT_CANCELLED, CONTACT_CANCELLING, CONTACT_PASS,
    CONTACT_POSTPASS, CONTACT_PREPASS, CONTACT_SCHEDULED, CONTACT_SCHEDULING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_cancellation::put_in_status;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, is_uuid, send, stock_ground_stations, Reply,
    AFTER_SETTLE,
};

/// The ground station every row holds its pass at. Nothing is recorded about its antennas,
/// so the placement gives it the single antenna this emulator numbers for such a station —
/// which is what makes "the antenna is free" and "the antenna is held" the two answers a
/// second reservation of the same window can get.
const STATION: &str = "Ohio-1";

/// The window every row of the table reserves: a round epoch-seconds instant in the
/// future, as the other suites of this target use, so no row is refused for antenna time
/// that has already elapsed.
const WINDOW: (i64, i64) = (1_800_000_000, 1_800_001_800);

/// The status the operation's modelled bad-parameter refusal carries
/// (`smithy_query(query="operation", operation="ReserveContact")` —
/// `InvalidParameterException`, HTTP 431).
const REFUSED_STATUS: u16 = 431;

/// See [`REFUSED_STATUS`].
const REFUSED_ERROR: &str = "InvalidParameterException";

/// One row of the table: the state the store's one contact is left in before the second
/// reservation of its window arrives.
struct Row {
    case: &'static str,
    /// The status the contact is put into before `CancelContact` is called on it, or `None`
    /// for a row whose contact is never cancelled. Which side of its pass the contact
    /// stands on is what decides whether the release completes outright or is carried out
    /// over a span.
    cancelled_in: Option<&'static str>,
    /// Whether the clock is allowed to overtake the move the setup left owed before the
    /// reservation is judged.
    settled: bool,
    /// The status the rule puts this contact in at the instant the reservation arrives:
    /// the one the moves already due leave it in.
    status_at_reservation: &'static str,
    /// Whether the station's antenna is then free for another pass over the same window —
    /// a contact in [`Row::status_at_reservation`] holds no antenna.
    antenna_free: bool,
}

/// The table: a row per move the service can owe a contact at the instant a reservation of
/// its window arrives, on both sides of the span that move takes.
///
/// The statuses are the model's own (`smithy_query(query="shape", shape="ContactStatus")`),
/// taken on the side of the pass `CancelContact`'s doc puts each one: a cancel before the
/// pass releases the reservation outright, while one during the pass is a stop, carried out
/// over a span.
const ROWS: &[Row] = &[
    Row {
        case: "a-reservation-still-owed-its-confirmation",
        cancelled_in: None,
        settled: false,
        status_at_reservation: CONTACT_SCHEDULING,
        antenna_free: false,
    },
    Row {
        case: "a-reservation-whose-confirmation-fell-due",
        cancelled_in: None,
        settled: true,
        status_at_reservation: CONTACT_SCHEDULED,
        antenna_free: false,
    },
    Row {
        case: "a-pass-cancelled-while-still-being-scheduled",
        cancelled_in: Some(CONTACT_SCHEDULING),
        settled: false,
        status_at_reservation: CONTACT_CANCELLED,
        antenna_free: true,
    },
    Row {
        case: "a-pass-cancelled-once-scheduled",
        cancelled_in: Some(CONTACT_SCHEDULED),
        settled: true,
        status_at_reservation: CONTACT_CANCELLED,
        antenna_free: true,
    },
    Row {
        case: "a-pass-stopped-in-prepass-still-releasing",
        cancelled_in: Some(CONTACT_PREPASS),
        settled: false,
        status_at_reservation: CONTACT_CANCELLING,
        antenna_free: false,
    },
    Row {
        case: "a-pass-stopped-in-prepass-whose-release-fell-due",
        cancelled_in: Some(CONTACT_PREPASS),
        settled: true,
        status_at_reservation: CONTACT_CANCELLED,
        antenna_free: true,
    },
    Row {
        case: "a-pass-stopped-mid-pass-still-releasing",
        cancelled_in: Some(CONTACT_PASS),
        settled: false,
        status_at_reservation: CONTACT_CANCELLING,
        antenna_free: false,
    },
    Row {
        case: "a-pass-stopped-mid-pass-whose-release-fell-due",
        cancelled_in: Some(CONTACT_PASS),
        settled: true,
        status_at_reservation: CONTACT_CANCELLED,
        antenna_free: true,
    },
    Row {
        case: "a-pass-stopped-in-postpass-whose-release-fell-due",
        cancelled_in: Some(CONTACT_POSTPASS),
        settled: true,
        status_at_reservation: CONTACT_CANCELLED,
        antenna_free: true,
    },
];

/// One row's store, stood up and left with its owed move not yet applied.
struct Standing {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    profile_arn: String,
    contact_id: String,
}

/// The clock, in epoch seconds — what the pass-driven suite measures its window from.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch-seconds instant fits in i64")
}

/// `ReserveContact` for `window` at [`STATION`], answering the reply as it came.
async fn reserve(
    router: &MultiServiceRouter,
    profile_arn: &str,
    window: (i64, i64),
) -> Reply {
    send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": STATION,
            "startTime": window.0,
            "endTime": window.1,
        })),
    )
    .await
}

/// The status the STORE holds for `contact_id`.
///
/// Read directly, not through `DescribeContact`: every reporting operation materialises the
/// moves the clock has made due, so a read through one could not tell a move the
/// reservation applied from one the read itself applied.
fn stored_status(engine: &engine::InMemoryEngine, contact_id: &str) -> String {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get(contact_id)
        .unwrap_or_else(|| panic!("the store holds no contact {contact_id}"))
        .contact_status
        .clone()
}

/// The antenna the STORE records `contact_id`'s pass as placed on.
fn stored_antenna(engine: &engine::InMemoryEngine, contact_id: &str) -> String {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get(contact_id)
        .unwrap_or_else(|| panic!("the store holds no contact {contact_id}"))
        .antenna_name
        .clone()
}

/// How many contacts the STORE holds — what a refused reservation may not have grown.
fn stored_contacts(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .len()
}

/// Stand up a store of this row's own, holding ONE contact at [`STATION`] over [`WINDOW`]
/// in the state the row describes, with the move the row leaves owed not yet applied.
async fn stand_up(row: &Row) -> Standing {
    let (router, engine) = harness_with_engine();
    // The account is served by the station the row's contact is held at.
    stock_ground_stations(&engine, STATION);
    // A pre-pass and a post-pass period, so the contact has every stage of a pass the rows
    // put it in.
    let profile_arn = create_mission_profile(&router, row.case, 60, Some(120), Some(60)).await;

    let reserved = reserve(&router, &profile_arn, WINDOW).await;
    assert_eq!(
        reserved.status, 200,
        "case {}: the first reservation of a free antenna is accepted, got {} {}",
        row.case, reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");

    if let Some(status) = row.cancelled_in {
        // The stages of a pass are reached by the pass itself, which no request drives, so
        // the contact is put under way in the store — as `contact_cancellation.rs` does for
        // the same reason. The release itself then goes through the front door.
        put_in_status(&engine, &contact_id, status);
        let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
        assert_eq!(
            cancelled.status, 200,
            "case {}: CancelContact expected 200, got {} {}",
            row.case, cancelled.status, cancelled.body
        );
    }

    Standing {
        router,
        engine,
        profile_arn,
        contact_id,
    }
}

/// Reserve this row's window again — as the first call since its owed move fell due when
/// `read_first` is false, and after a reporting call that would itself have settled that
/// move when it is true — and hold the answer to the rule.
async fn judge(row: &Row, standing: &Standing, read_first: bool) {
    let order = if read_first {
        "a-settling-read-first"
    } else {
        "the-reservation-first"
    };
    let case = format!("{}/{order}", row.case);

    if read_first {
        let described = send(
            &standing.router,
            "GET",
            &format!("/contact/{}", standing.contact_id),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeContact expected 200, got {} {}",
            described.status, described.body
        );
        let reported = described.string("contactStatus");
        assert_eq!(
            reported, row.status_at_reservation,
            "case {case}: the contact must report the status the moves already due leave it \
             in, expected {}, got {reported:?}",
            row.status_at_reservation
        );
    }

    let held_before = stored_contacts(&standing.engine);
    let reply = reserve(&standing.router, &standing.profile_arn, WINDOW).await;

    // The placement read the statuses the clock owed, whichever order the calls came in:
    // the store's account of the first contact afterwards is that status, so the
    // reservation materialised the due move itself rather than leaving it to a later call.
    let settled = stored_status(&standing.engine, &standing.contact_id);
    assert_eq!(
        settled, row.status_at_reservation,
        "case {case}: a reservation is placed against the statuses the clock has made due, \
         so the contact must stand in {} when it is answered, got {settled:?}",
        row.status_at_reservation
    );

    if row.antenna_free {
        assert_eq!(
            reply.status, 200,
            "case {case}: a contact in {} holds no antenna, so the window is reservable \
             again, got {} {}",
            row.status_at_reservation, reply.status, reply.body
        );
        let successor = reply.string("contactId");
        assert!(
            is_uuid(&successor),
            "case {case}: an accepted reservation mints a contactId, got {successor:?}"
        );
        assert_ne!(
            successor, standing.contact_id,
            "case {case}: the second reservation is a contact of its own"
        );
        let placed = stored_antenna(&standing.engine, &successor);
        assert_eq!(
            placed,
            antenna_name(1),
            "case {case}: the freed antenna takes the next pass, got {placed:?}"
        );
        assert_eq!(
            stored_contacts(&standing.engine),
            held_before + 1,
            "case {case}: an accepted reservation stores exactly one more contact"
        );

        // And it reads back through the front door as the pass it was taken for.
        let described = send(
            &standing.router,
            "GET",
            &format!("/contact/{successor}"),
            None,
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case}: DescribeContact expected 200 for the successor, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.string("groundStation"),
            STATION,
            "case {case}: the successor is held at the station it was reserved at, got {}",
            described.body
        );
        for (member, expected) in [
            ("startTime", WINDOW.0 as f64),
            ("endTime", WINDOW.1 as f64),
        ] {
            let actual = described.field(member).as_f64();
            assert_eq!(
                actual,
                Some(expected),
                "case {case}: the successor's {member} must be {expected}, got {actual:?}"
            );
        }
    } else {
        assert_eq!(
            reply.status, REFUSED_STATUS,
            "case {case}: a contact in {} still holds the station's only antenna, so the \
             overlapping window is refused with the modelled {REFUSED_ERROR} status \
             {REFUSED_STATUS}, got {} {}",
            row.status_at_reservation, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            REFUSED_ERROR,
            "case {case}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.field("parameterName"),
            &json!("startTime"),
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
        assert_eq!(
            stored_contacts(&standing.engine),
            held_before,
            "case {case}: a refused reservation stores no contact"
        );
    }
}

#[tokio::test]
async fn a_reservation_is_placed_against_the_contact_statuses_the_clock_has_made_due() {
    // Every row is judged in BOTH orders, each on a store of its own: a settling read taken
    // in one order would otherwise be the call that materialises the other order's move.
    let orders = [false, true];

    // The rows whose owed move must be allowed to fall due are stood up first and waited
    // out together — so the span is slept once rather than once per row, and so that in the
    // reservation-first order nothing at all is asked of the store between the instant the
    // move fell due and the reservation that has to observe it.
    let mut due: Vec<(&Row, bool, Standing)> = Vec::new();
    for row in ROWS.iter().filter(|row| row.settled) {
        for read_first in orders {
            due.push((row, read_first, stand_up(row).await));
        }
    }
    std::thread::sleep(AFTER_SETTLE);
    for (row, read_first, standing) in &due {
        judge(row, standing, *read_first).await;
    }

    // And the rows judged while the owed move is still in flight, each stood up and judged
    // at once so its span cannot pass in between.
    for row in ROWS.iter().filter(|row| !row.settled) {
        for read_first in orders {
            let standing = stand_up(row).await;
            judge(row, &standing, read_first).await;
        }
    }
}

#[tokio::test]
async fn a_reservation_is_the_first_call_after_a_stopped_passs_release_fell_due() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn =
        create_mission_profile(&router, "the pass clock", 60, Some(120), Some(60)).await;

    // A window that begins moments from now and runs for ten minutes: the pass begins on
    // the clock alone, and the window still has antenna time left in it when it is
    // reserved again below.
    let now = now_seconds();
    let window = (now + 2, now + 600);
    let reserved = reserve(&router, &profile_arn, window).await;
    assert_eq!(
        reserved.status, 200,
        "case the-first-pass: expected 200, got {} {}",
        reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");

    // The reservation's confirmation and the start of its pass are both due by now.
    std::thread::sleep(AFTER_SETTLE);
    let described = send(&router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case under-way: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    let under_way = described.string("contactStatus");
    assert!(
        [CONTACT_PREPASS, CONTACT_PASS, CONTACT_POSTPASS].contains(&under_way.as_str()),
        "case under-way: the clock has carried this contact into its pass, expected one of \
         {:?}, got {under_way:?}",
        [CONTACT_PREPASS, CONTACT_PASS, CONTACT_POSTPASS]
    );

    // A cancel of a pass under way is a STOP, carried out over a span — and the only call
    // made before the reservation below.
    let stopped = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        stopped.status, 200,
        "case stop: CancelContact expected 200, got {} {}",
        stopped.status, stopped.body
    );
    let stopping = stored_status(&engine, &contact_id);
    assert_eq!(
        stopping, CONTACT_CANCELLING,
        "case stop: a stop of a pass under way is carried out over a span, so the contact \
         stands in {CONTACT_CANCELLING}, got {stopping:?}"
    );

    // The release falls due with the service asked nothing meanwhile, so the reservation
    // below is the FIRST call to see it — and it must be answered by the clock, not by
    // whether something settled the move for it.
    std::thread::sleep(AFTER_SETTLE);
    let successor = reserve(&router, &profile_arn, window).await;
    assert_eq!(
        successor.status, 200,
        "case after-the-release: the released reservation holds no antenna once its stop \
         fell due, so the window is reservable, got {} {}",
        successor.status, successor.body
    );
    let successor_id = successor.string("contactId");
    assert_ne!(
        successor_id, contact_id,
        "case after-the-release: the second reservation is a contact of its own"
    );

    let released = stored_status(&engine, &contact_id);
    assert_eq!(
        released, CONTACT_CANCELLED,
        "case after-the-release: the reservation materialised the release that had fallen \
         due, so the stopped contact stands in {CONTACT_CANCELLED}, got {released:?}"
    );
    let placed = stored_antenna(&engine, &successor_id);
    assert_eq!(
        placed,
        antenna_name(1),
        "case after-the-release: the freed antenna takes the next pass, got {placed:?}"
    );

    let read_back = send(&router, "GET", &format!("/contact/{successor_id}"), None).await;
    assert_eq!(
        read_back.status, 200,
        "case after-the-release: DescribeContact expected 200, got {} {}",
        read_back.status, read_back.body
    );
    assert_eq!(
        read_back.string("groundStation"),
        STATION,
        "case after-the-release: the successor is held at the station it was reserved at, \
         got {}",
        read_back.body
    );
}

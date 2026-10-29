// SPDX-License-Identifier: CC0-1.0

//! The coupling between a satellite and the ephemeris its passes are flown on, driven
//! through `CreateEphemeris` (`POST /ephemeris`), `UpdateEphemeris`
//! (`PUT /ephemeris/{ephemerisId}`), `DeleteEphemeris` (`DELETE /ephemeris/{ephemerisId}`),
//! `ListContacts`' `ephemeris` filter (`POST /contacts`) and `GetSatellite`
//! (`GET /satellite/{satelliteId}`), end to end over HTTP through the same front door as
//! the rest of this target's suites.
//!
//! The RULE under test: a satellite's passes are flown on the ONE ephemeris in force for
//! it — of the ephemerides the account holds `ENABLED` for that satellite, the one of
//! highest precedence (the highest `priority`, which is "a priority score that determines
//! which ephemeris to use when multiple ephemerides overlap. Higher numbers take
//! precedence." — `smithy_query(query="doc", shape="CreateEphemerisRequest",
//! member="priority")`, then the most recently created, then the larger `ephemerisId`) —
//! so an ephemeris another supersedes is the trajectory of no pass: no contact of that
//! satellite points it, an `ephemeris` filter naming it selects none of them, and it
//! deletes while they stand, while the one in force is held by every non-terminal contact
//! of that satellite and its delete is refused with the modelled `ResourceInUseException`
//! (HTTP 409). The ephemeris a contact names OUTRIGHT in its program track is a different
//! coupling, which precedence does not touch.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning what the precedence rule generalises over — how many ephemerides the
//!   account holds enabled for one satellite (two and three), which position in creation
//!   order carries the winning `priority`, a pair tied at one priority that the chosen
//!   tie-break settles, and the absent `priority` member that takes the documented default
//!   of 1 — each row read back through `GetSatellite`, then through the `ephemeris` filter
//!   naming each of its ephemerides in turn, then through a delete of each of them;
//! * the move: the pointing follows whichever ephemeris takes the satellite, so disabling
//!   the one in force hands every contact of that satellite to the one that was superseded,
//!   and the two deletes swap their answers;
//! * the other coupling, held apart: a superseded ephemeris a contact's program track names
//!   outright IS held by it, and both its delete and the in-force one's are refused until
//!   that contact reaches an end of its lifecycle.
//!
//! The harness, the signed `restJson1` requests, the onboarding helper, the ephemeris data
//! builders and the stored-contact helpers are [`crate::scheduling_loop`]'s,
//! [`crate::ephemeris_catalog`]'s and [`crate::ephemeris_deletion`]'s, reused rather than
//! rebuilt.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    ephemeris_in_use_message, CONTACT_AVAILABLE, TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{create_ok, onboard, tle_inline, wait_out_validation, FUTURE};
use crate::ephemeris_deletion::{put_in_status, satellite_arn, CONTACT_STATUSES};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The ground station every contact here is reserved at. A `GroundStationName` is 4..97
/// characters of `[ a-zA-Z0-9-._:=]` (`smithy_query(query="shape",
/// shape="GroundStationName")`).
const GROUND_STATION: &str = "Ohio 1";

/// The validity range the data every ephemeris here carries declares.
const DATA_RANGE: (i64, i64) = (1_800_000_000, 1_800_086_400);

/// The window every listing in this suite is taken over: wide enough to hold every contact
/// reserved below, so what one row's answer differs from another's by is the filter and not
/// the window.
const WINDOW: (i64, i64) = (1_799_000_000, 1_803_000_000);

/// The statuses a listing of STORED contacts asks for: every modelled `ContactStatus`
/// except `AVAILABLE`, which is an offered window rather than a stored contact and whose
/// search needs three further members with it.
fn stored_statuses() -> Vec<&'static str> {
    CONTACT_STATUSES
        .iter()
        .copied()
        .filter(|status| *status != CONTACT_AVAILABLE)
        .collect()
}

/// `CreateEphemeris` for an ephemeris of `satellite_id` named `name`, asked to be enabled,
/// carrying `priority` when the row names one and the documented default when it does not.
async fn create_for(
    router: &MultiServiceRouter,
    case: &str,
    satellite_id: &str,
    name: &str,
    priority: Option<i64>,
) -> String {
    let mut body = json!({
        "name": name,
        "enabled": true,
        "expirationTime": FUTURE,
        "satelliteId": satellite_id,
        "ephemeris": tle_inline(&[DATA_RANGE]),
    });
    if let Some(priority) = priority {
        body["priority"] = json!(priority);
    }
    create_ok(router, case, body).await
}

/// `ReserveContact` for the window a day per `slot` after [`DATA_RANGE`]'s start, flown
/// against `satellite_arn` and carrying `tracking` when the caller names overrides; answers
/// the `contactId` it minted.
///
/// `slot` moves each contact's window, because an antenna serves one pass at a time: a
/// second reservation of the SAME window at one ground station is refused.
async fn reserve(
    router: &MultiServiceRouter,
    case: &str,
    profile_arn: &str,
    satellite_arn: &str,
    tracking: Option<Value>,
    slot: usize,
) -> String {
    let start = DATA_RANGE.0 + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
    let mut body = json!({
        "missionProfileArn": profile_arn,
        "groundStation": GROUND_STATION,
        "startTime": start,
        "endTime": start + 600,
        "satelliteArn": satellite_arn,
    });
    if let Some(tracking) = tracking {
        body["trackingOverrides"] = tracking;
    }
    let reserved = send(router, "POST", "/contact", Some(body)).await;
    assert_eq!(
        reserved.status, 200,
        "case {case}: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// `DeleteEphemeris` for `ephemeris_id`.
async fn delete(router: &MultiServiceRouter, ephemeris_id: &str) -> Reply {
    send(router, "DELETE", &format!("/ephemeris/{ephemeris_id}"), None).await
}

/// `UpdateEphemeris` for `ephemeris_id` with `body`, asserting it was accepted.
async fn update_ok(router: &MultiServiceRouter, case: &str, ephemeris_id: &str, body: Value) {
    let reply = send(
        router,
        "PUT",
        &format!("/ephemeris/{ephemeris_id}"),
        Some(body),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected UpdateEphemeris 200, got {} {}",
        reply.status, reply.body
    );
}

/// The `ephemerisId` `GetSatellite` reports as the satellite's `currentEphemeris`, or
/// `None` where the account holds none in force for it.
async fn current_ephemeris(
    router: &MultiServiceRouter,
    case: &str,
    satellite_id: &str,
) -> Option<String> {
    let reply = send(router, "GET", &format!("/satellite/{satellite_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetSatellite 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .body
        .get("currentEphemeris")
        .filter(|value| !value.is_null())
        .and_then(|current| current.get("ephemerisId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `contactId`s `ListContacts` reports over [`WINDOW`] for every stored status, under
/// the `ephemeris` filter naming `filter` when one is named.
async fn listed_contacts(
    router: &MultiServiceRouter,
    case: &str,
    filter: Option<&str>,
) -> Vec<String> {
    let mut body = json!({
        "statusList": stored_statuses(),
        "startTime": WINDOW.0,
        "endTime": WINDOW.1,
    });
    if let Some(ephemeris_id) = filter {
        body["ephemeris"] = json!({"azEl": {"id": ephemeris_id}});
    }
    let reply = send(router, "POST", "/contacts", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: contactList must be a list, got {}", reply.body))
        .iter()
        .filter_map(|item| {
            item.get("contactId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Assert the contact `contact_id` has not reached an end of its lifecycle, which is what
/// makes the in-use question bite at all.
async fn assert_non_terminal(router: &MultiServiceRouter, case: &str, contact_id: &str) {
    let reply = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        reply.status, reply.body
    );
    let status = reply.string("contactStatus");
    assert!(
        !TERMINAL_CONTACT_STATUSES.contains(&status.as_str()),
        "case {case}: the contact must still be non-terminal, got {status:?}"
    );
}

/// Assert the delete of `ephemeris_id` is refused as the modelled in-use failure naming
/// `contact_id`.
async fn assert_delete_refused(
    router: &MultiServiceRouter,
    case: &str,
    ephemeris_id: &str,
    contact_id: &str,
) {
    let refused = delete(router, ephemeris_id).await;
    assert_eq!(
        refused.status, 409,
        "case {case}: expected the modelled ResourceInUseException status 409, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ResourceInUseException",
        "case {case}: wrong error shape, got {}",
        refused.body
    );
    assert_eq!(
        refused.string("message"),
        ephemeris_in_use_message(std::slice::from_ref(&contact_id.to_string())),
        "case {case}: the refusal must name the contact that holds the ephemeris, got {}",
        refused.body
    );
}

/// Assert the delete of `ephemeris_id` is answered.
async fn assert_deleted(router: &MultiServiceRouter, case: &str, ephemeris_id: &str) {
    let deleted = delete(router, ephemeris_id).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: nothing is flown on a superseded ephemeris, so the delete is \
         answered, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("ephemerisId"),
        ephemeris_id,
        "case {case}: DeleteEphemeris must answer the ephemeris it removed, got {}",
        deleted.body
    );
}

/// One row of the table: the enabled ephemerides one satellite holds, as the `priority`
/// each `CreateEphemeris` names in creation order, and which of them the precedence rule
/// puts in force.
struct Row {
    case: &'static str,
    /// The `priority` each ephemeris of the row is created with, in creation order; `None`
    /// where the request names none and the documented default of 1 applies.
    priorities: Vec<Option<i64>>,
    /// The index, in creation order, of the ephemeris in force for the row's satellite.
    in_force: usize,
}

/// The table: one row per way several enabled ephemerides of one satellite can stand,
/// spanning their count, where in creation order the winner sits, a tie at one priority and
/// the absent `priority` member.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "two, the higher priority stored second",
            priorities: vec![Some(1), Some(9)],
            in_force: 1,
        },
        Row {
            case: "two, the higher priority stored first",
            priorities: vec![Some(9), Some(1)],
            in_force: 0,
        },
        Row {
            // Both published bounds of `CustomerEphemerisPriority`
            // (`smithy_query(query="operation", operation="CreateEphemeris")` — range
            // 1..99999) and a value between them.
            case: "three, the highest priority stored in the middle",
            priorities: vec![Some(1), Some(99_999), Some(500)],
            in_force: 1,
        },
        Row {
            // The documented priority leaves this pair tied, and the tie-break is the
            // CHOSEN one: the most recently created of equal priority.
            case: "two tied at one priority",
            priorities: vec![Some(4), Some(4)],
            in_force: 1,
        },
        Row {
            // "The default is 1. Must be 1 or greater." (`smithy_query(query="doc",
            // shape="CreateEphemerisRequest", member="priority")`), so the explicit 2
            // outranks the absent member.
            case: "the absent priority member against an explicit two",
            priorities: vec![None, Some(2)],
            in_force: 1,
        },
        Row {
            case: "the absent priority member outranking nothing but itself",
            priorities: vec![Some(2), None],
            in_force: 0,
        },
    ]
}

/// What one row of [`rows`] stored: its satellite, its contact and the ids its ephemerides
/// were minted under, in creation order.
struct Stored {
    satellite_id: String,
    contact_id: String,
    ephemerides: Vec<String>,
}

#[tokio::test]
async fn only_the_ephemeris_in_force_for_a_satellite_is_held_by_its_contacts() {
    let (router, engine) = harness_with_engine();
    let rows = rows();
    // One satellite per row, each with its own ephemerides and its own contact, so a row's
    // refusal names exactly the contact of that row and one row's delete cannot be what the
    // next row observes. The NORAD catalogue numbers fall inside
    // `state::NORAD_SATELLITE_ID_RANGE`.
    let setting: Vec<String> = (0..rows.len())
        .map(|row| format!("{}={GROUND_STATION}", 25_100 + row))
        .collect();
    let satellites = onboard(&engine, &setting.join(","));
    assert_eq!(
        satellites.len(),
        rows.len(),
        "case setup: the onboarding must store one satellite per row, got {satellites:?}"
    );
    let profile_arn = create_mission_profile(&router, "ephemeris precedence", 60, None, None).await;

    // Every row's ephemerides and its contact, stored through the front door. The slot runs
    // over the whole table, so no two contacts ask for the same window at one station.
    let mut stored: Vec<Stored> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let satellite_id = satellites[index].clone();
        let mut ephemerides: Vec<String> = Vec::new();
        for (held, priority) in row.priorities.iter().enumerate() {
            ephemerides.push(
                create_for(
                    &router,
                    row.case,
                    &satellite_id,
                    &format!("row {index} held {held}"),
                    *priority,
                )
                .await,
            );
        }
        let flown = satellite_arn(&engine, &satellite_id);
        let contact_id = reserve(&router, row.case, &profile_arn, &flown, None, index).await;
        stored.push(Stored {
            satellite_id,
            contact_id,
            ephemerides,
        });
    }

    // ONE wait for the whole table: every ephemeris leaves `VALIDATING` and the scheduler
    // makes its own move on the clock alone, so each row is judged against the statuses that
    // hold once both are due.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let stored = &stored[index];
        let in_force = &stored.ephemerides[row.in_force];
        assert_non_terminal(&router, case, &stored.contact_id).await;

        // The satellite read names the SAME one ephemeris the pointing is settled by: one
        // rule, one answer.
        let reported = current_ephemeris(&router, case, &stored.satellite_id).await;
        assert_eq!(
            reported.as_deref(),
            Some(in_force.as_str()),
            "case {case}: the satellite must report the ephemeris of highest precedence as \
             its currentEphemeris, got {reported:?}"
        );

        // The filter naming each of the row's ephemerides selects the contact exactly when
        // that ephemeris is the one in force.
        for (held, ephemeris_id) in stored.ephemerides.iter().enumerate() {
            let filtered = listed_contacts(
                &router,
                &format!("{case}, filtered by held {held}"),
                Some(ephemeris_id),
            )
            .await;
            let expected = ephemeris_id == in_force;
            assert_eq!(
                filtered.contains(&stored.contact_id),
                expected,
                "case {case}, filtered by held {held}: the filter must select the contact \
                 exactly when the ephemeris it names is the one in force (expected \
                 {expected}), got {filtered:?}"
            );
        }

        // Every superseded ephemeris deletes while that contact stands — nothing is flown on
        // it — and the one in force is refused, naming the contact.
        for (held, ephemeris_id) in stored.ephemerides.iter().enumerate() {
            if ephemeris_id == in_force {
                continue;
            }
            assert_deleted(&router, &format!("{case}, deleting held {held}"), ephemeris_id).await;
        }
        assert_delete_refused(
            &router,
            &format!("{case}, deleting the one in force"),
            in_force,
            &stored.contact_id,
        )
        .await;

        // A refused delete writes nothing, and dropping the superseded ones left the
        // satellite with the same trajectory it already had.
        let after = current_ephemeris(&router, case, &stored.satellite_id).await;
        assert_eq!(
            after.as_deref(),
            Some(in_force.as_str()),
            "case {case}: the deletes of the superseded ephemerides and the refused delete \
             of the one in force leave the satellite reporting the same one, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_pointing_follows_whichever_ephemeris_takes_the_satellite() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, &format!("25994={GROUND_STATION}"));
    let [satellite_id] = <[String; 1]>::try_from(satellites).expect("one satellite is on-boarded");
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn = create_mission_profile(&router, "pointing moves", 60, None, None).await;

    let lower = create_for(&router, "the lower priority", &satellite_id, "at one", Some(1)).await;
    let higher = create_for(&router, "the higher priority", &satellite_id, "at nine", Some(9)).await;
    let contact_id = reserve(&router, "the standing contact", &profile_arn, &flown, None, 0).await;
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);
    assert_non_terminal(&router, "the standing contact", &contact_id).await;

    // While the higher priority one is in force, it is the one the contact holds.
    for (case, ephemeris_id, selects) in [
        ("the higher priority, in force", &higher, true),
        ("the lower priority, superseded", &lower, false),
    ] {
        let filtered = listed_contacts(&router, case, Some(ephemeris_id)).await;
        assert_eq!(
            filtered.contains(&contact_id),
            selects,
            "case {case}: the filter must select the contact exactly when the ephemeris it \
             names is the one in force (expected {selects}), got {filtered:?}"
        );
    }
    assert_delete_refused(&router, "the higher priority stands", &higher, &contact_id).await;

    // Disabling it hands the satellite to the one it superseded, and the two answers swap:
    // the one that is no longer in force deletes, and the one that took over is refused.
    update_ok(
        &router,
        "disable the higher priority",
        &higher,
        json!({"enabled": false}),
    )
    .await;
    let taken_over = current_ephemeris(&router, "disable the higher priority", &satellite_id).await;
    assert_eq!(
        taken_over.as_deref(),
        Some(lower.as_str()),
        "case disable the higher priority: the superseded ephemeris takes the satellite, got \
         {taken_over:?}"
    );
    for (case, ephemeris_id, selects) in [
        ("the lower priority, now in force", &lower, true),
        ("the higher priority, now disabled", &higher, false),
    ] {
        let filtered = listed_contacts(&router, case, Some(ephemeris_id)).await;
        assert_eq!(
            filtered.contains(&contact_id),
            selects,
            "case {case}: the filter must select the contact exactly when the ephemeris it \
             names is the one in force (expected {selects}), got {filtered:?}"
        );
    }
    assert_delete_refused(&router, "the lower priority took over", &lower, &contact_id).await;
    assert_deleted(&router, "the disabled one is flown on by nothing", &higher).await;
}

#[tokio::test]
async fn a_superseded_ephemeris_a_program_track_names_is_still_held_by_that_contact() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard(&engine, &format!("27424={GROUND_STATION}"));
    let [satellite_id] = <[String; 1]>::try_from(satellites).expect("one satellite is on-boarded");
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn = create_mission_profile(&router, "program track naming", 60, None, None).await;

    let named = create_for(&router, "the ephemeris named outright", &satellite_id, "named at one", Some(1)).await;
    let in_force = create_for(&router, "the ephemeris in force", &satellite_id, "in force at nine", Some(9)).await;
    // The program track names the SUPERSEDED ephemeris by the `tle` member of
    // `ProgramTrackSettings`, which is the member the type of the data it was created with
    // requires (`smithy_query(query="shape", shape="ProgramTrackSettings")`).
    let contact_id = reserve(
        &router,
        "the contact naming it outright",
        &profile_arn,
        &flown,
        Some(json!({"programTrackSettings": {"tle": {"ephemerisId": named}}})),
        0,
    )
    .await;
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);
    assert_non_terminal(&router, "the contact naming it outright", &contact_id).await;

    let reported = current_ephemeris(&router, "the satellite's trajectory", &satellite_id).await;
    assert_eq!(
        reported.as_deref(),
        Some(in_force.as_str()),
        "case the satellite's trajectory: the higher priority ephemeris is the one in force, \
         got {reported:?}"
    );

    // BOTH couplings hold this contact: the one it names outright, whatever precedence says
    // of it, and the one in force for the satellite it flies.
    for (case, ephemeris_id) in [
        ("named outright by the program track", &named),
        ("in force for the flown satellite", &in_force),
    ] {
        let filtered = listed_contacts(&router, case, Some(ephemeris_id)).await;
        assert!(
            filtered.contains(&contact_id),
            "case {case}: the filter must select the contact the ephemeris points, got \
             {filtered:?}"
        );
        assert_delete_refused(&router, case, ephemeris_id, &contact_id).await;
    }

    // Once that contact reaches an end of its lifecycle it holds neither of them, so both
    // delete — the whole terminal half of the enum, each status in turn.
    for status in TERMINAL_CONTACT_STATUSES {
        put_in_status(&engine, &contact_id, status);
        let filtered = listed_contacts(&router, status, Some(&named)).await;
        assert!(
            filtered.contains(&contact_id),
            "case {status}: a contact at an end of its lifecycle still named the ephemeris, \
             so the filter must answer it, got {filtered:?}"
        );
    }
    assert_deleted(&router, "named outright, contact terminal", &named).await;
    assert_deleted(&router, "in force, contact terminal", &in_force).await;
}

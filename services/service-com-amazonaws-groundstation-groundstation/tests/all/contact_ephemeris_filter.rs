// SPDX-License-Identifier: CC0-1.0

//! `ListContacts`' `ephemeris` filter (`POST /contacts`) end to end over HTTP, through the
//! same front door as the rest of this target's suites.
//!
//! The RULE under test: the `ephemeris` member is a filter "for selecting contacts that use
//! a specific ephemeris" (`smithy_query(query="shape", shape="EphemerisFilter")`), so what
//! it selects is settled by the POINTING alone — the ephemeris in force for the satellite a
//! contact flies, or the one the contact's program track names — whatever status the contact
//! holds. Whether a contact still HOLDS an ephemeris is a different question, asked only by
//! the delete that rule protects (`tests/all/ephemeris_deletion.rs`), and its restriction to
//! contacts that have not reached an end of the lifecycle is no part of a listing whose
//! `statusList` is free to ask for exactly the statuses at those ends.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning EVERY modelled `ContactStatus` crossed with both ways a contact names
//!   an ephemeris, each row answered against a set of filters — the ephemeris that points
//!   it, the one that points the rows of the other coupling, one in force for a satellite no
//!   contact flies, and a well-formed `Uuid` naming no ephemeris — with the row's contact
//!   selected by exactly the filter naming the ephemeris that points it;
//! * the two questions held apart over the whole terminal half of the enum: a contact in
//!   each terminal status is still selected by a filter naming the ephemeris that points it,
//!   while that same ephemeris deletes — because nothing holds it any longer — after which
//!   the filter naming it selects nothing and the unfiltered listing still answers the
//!   contact.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    CONTACT_AVAILABLE, TERMINAL_CONTACT_STATUSES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{az_el_inline, create_ok, onboard, wait_out_validation, FUTURE};
use crate::ephemeris_deletion::{put_in_status, satellite_arn, CONTACT_STATUSES};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, ABSENT_UUID, AFTER_SETTLE, REGION,
};

/// The ground station every contact here is reserved at, and the station
/// `ephemeris_catalog::az_el_inline` supplies its pointing angles for. A
/// `GroundStationName` is 4..97 characters of `[ a-zA-Z0-9-._:=]`
/// (`smithy_query(query="shape", shape="GroundStationName")`).
const GROUND_STATION: &str = "Ohio 1";

/// The NORAD catalogue numbers this suite's two satellites are on-boarded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: (i32, i32) = (25_994, 27_424);

/// The validity range the pointing data every ephemeris here carries declares.
const DATA_RANGE: (i64, i64) = (1_800_000_000, 1_800_086_400);

/// The window every listing in this suite is taken over: wide enough to hold every contact
/// reserved below, so what one row's answer differs from another's by is the filter and not
/// the window.
const WINDOW: (i64, i64) = (1_799_000_000, 1_803_000_000);

/// A satellite ARN of this account that no onboarding names, for the contacts coupled to an
/// ephemeris only by their program track: no ephemeris applies to a satellite this account
/// is not on-boarded to, so such a contact is pointed by exactly the ephemeris it names.
///
/// `satelliteArn` is `arn:aws:groundstation:<region>:<account>:satellite/<Uuid>`
/// (`smithy_query(query="shape", shape="satelliteArn")`).
fn untracked_satellite_arn() -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:satellite/22222222-2222-2222-2222-222222222222",
        engine::emulator_account_id()
    )
}

/// `TrackingOverrides` whose program track names the azimuth-elevation ephemeris
/// `ephemeris_id` — the `azEl` member of `ProgramTrackSettings`
/// (`smithy_query(query="shape", shape="ProgramTrackSettings")`), which is the form an
/// `AzElEphemerisFilter` names one in too.
fn az_el_program_track(ephemeris_id: &str) -> Value {
    json!({"programTrackSettings": {"azEl": {"ephemerisId": ephemeris_id}}})
}

/// A `CreateEphemeris` body carrying azimuth-elevation pointing data for
/// [`GROUND_STATION`], named `name`, applying to `satellite_id` when one is named, and asked
/// to be enabled or not.
fn az_el_ephemeris(name: &str, satellite_id: Option<&str>, enabled: bool) -> Value {
    let mut body = json!({
        "name": name,
        "enabled": enabled,
        "expirationTime": FUTURE,
        "ephemeris": az_el_inline(&[DATA_RANGE]),
    });
    if let Some(satellite_id) = satellite_id {
        body["satelliteId"] = json!(satellite_id);
    }
    body
}

/// `ReserveContact` for the window a day per `slot` after [`DATA_RANGE`]'s start, flown
/// against `satellite_arn` and carrying `tracking` when the row names overrides; answers the
/// `contactId` it minted.
///
/// `slot` moves each row's window, because an antenna serves one pass at a time: a second
/// reservation of the SAME window at one ground station is refused.
async fn reserve_ok(
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

/// `ListContacts` over [`WINDOW`] for `statuses`, under the `ephemeris` filter `filter`
/// names when it names one.
///
/// A request asking for `AVAILABLE` carries the three members that operation's own
/// documentation requires with it ("if statusList contains AVAILABLE, the request must
/// include groundStation, missionprofileArn, and satelliteArn"), which is also what confines
/// its computed half to the pairing the row is about.
async fn list(
    router: &MultiServiceRouter,
    statuses: &[&str],
    satellite_arn: &str,
    profile_arn: &str,
    filter: Option<&str>,
) -> Reply {
    let mut body = json!({
        "statusList": statuses,
        "startTime": WINDOW.0,
        "endTime": WINDOW.1,
    });
    if statuses.contains(&CONTACT_AVAILABLE) {
        body["groundStation"] = json!(GROUND_STATION);
        body["satelliteArn"] = json!(satellite_arn);
        body["missionProfileArn"] = json!(profile_arn);
    }
    if let Some(ephemeris_id) = filter {
        body["ephemeris"] = json!({"azEl": {"id": ephemeris_id}});
    }
    send(router, "POST", "/contacts", Some(body)).await
}

/// The `contactId`s [`list`] answers, asserting the request was ANSWERED.
///
/// An offered `AVAILABLE` window is not a reservation and carries no `contactId`, so only
/// the stored contacts a listing reports are collected here.
async fn listed_contacts(
    router: &MultiServiceRouter,
    case: &str,
    statuses: &[&str],
    satellite_arn: &str,
    profile_arn: &str,
    filter: Option<&str>,
) -> Vec<String> {
    let reply = list(router, statuses, satellite_arn, profile_arn, filter).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContacts 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactList must be a list, got {}",
                reply.body
            )
        })
        .iter()
        .filter_map(|item| {
            item.get("contactId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// One row of the table: a contact in `status`, coupled to the ephemeris `uses` names in the
/// way `coupling` describes.
struct Row {
    coupling: &'static str,
    status: &'static str,
    contact_id: String,
    /// The satellite ARN the row's contact is flown against, which its own listing has to
    /// name when it asks for `AVAILABLE`.
    satellite_arn: String,
    /// The ephemeris that points the row's contact — the one filter that must select it.
    uses: String,
}

#[tokio::test]
async fn the_ephemeris_filter_selects_by_pointing_in_every_contact_status() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(
        &engine,
        &format!("{}={GROUND_STATION},{}={GROUND_STATION}", NORAD.0, NORAD.1),
    );
    let [flown, unflown] =
        <[String; 2]>::try_from(onboarded).expect("two satellites are on-boarded");
    let flown_arn = satellite_arn(&engine, &flown);
    let profile_arn =
        create_mission_profile(&router, "ephemeris filter statuses", 60, None, None).await;

    // The ephemeris in force for the satellite the first coupling's contacts fly.
    let in_force = create_ok(
        &router,
        "the ephemeris in force",
        az_el_ephemeris("pointing in force", Some(&flown), true),
    )
    .await;
    // The ephemeris the second coupling's contacts name in their program track: it applies
    // to no satellite and the caller never asked for it to be enabled, so the NAMING is the
    // only thing that couples it to a contact.
    let named_outright = create_ok(
        &router,
        "the ephemeris a program track names",
        az_el_ephemeris("named by a program track", None, false),
    )
    .await;
    // An ephemeris in force for the OTHER on-boarded satellite, which no contact here flies:
    // it points nothing, so a filter naming it selects nothing however the contacts stand.
    let unrelated = create_ok(
        &router,
        "an ephemeris of another satellite",
        az_el_ephemeris("pointing of the other satellite", Some(&unflown), true),
    )
    .await;

    // One contact per row, each in its own window: the status filter is what isolates a
    // row's contact in the answer, and both couplings are stored for every status.
    let untracked = untracked_satellite_arn();
    let mut rows: Vec<Row> = Vec::new();
    for (slot, status) in CONTACT_STATUSES.iter().enumerate() {
        let satellite_row = Row {
            coupling: "the ephemeris in force for the satellite the contact flies",
            status,
            contact_id: reserve_ok(
                &router,
                status,
                &profile_arn,
                &flown_arn,
                None,
                slot * 2,
            )
            .await,
            satellite_arn: flown_arn.clone(),
            uses: in_force.clone(),
        };
        let program_track_row = Row {
            coupling: "the ephemeris the contact's program track names",
            status,
            contact_id: reserve_ok(
                &router,
                status,
                &profile_arn,
                &untracked,
                Some(az_el_program_track(&named_outright)),
                slot * 2 + 1,
            )
            .await,
            satellite_arn: untracked.clone(),
            uses: named_outright.clone(),
        };
        rows.push(satellite_row);
        rows.push(program_track_row);
    }

    // ONE wait for the whole table: the ephemerides finish validating and the scheduler
    // makes its own move, both on the clock alone. The listing below then materialises
    // those moves, so the statuses each row is put into afterwards are the ones in force.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);
    listed_contacts(
        &router,
        "settle what the clock owes",
        CONTACT_STATUSES,
        &flown_arn,
        &profile_arn,
        None,
    )
    .await;

    // Every filter the table is answered against, and the ephemeris each one names.
    let filters: Vec<(&str, &str)> = vec![
        ("the ephemeris in force for the flown satellite", &in_force),
        ("the ephemeris a program track names", &named_outright),
        ("an ephemeris in force for a satellite no contact flies", &unrelated),
        ("a well-formed Uuid naming no ephemeris", ABSENT_UUID),
    ];

    for row in &rows {
        put_in_status(&engine, &row.contact_id, row.status);
        let statuses = [row.status];

        let unfiltered = listed_contacts(
            &router,
            &format!("{} in {}, unfiltered", row.coupling, row.status),
            &statuses,
            &row.satellite_arn,
            &profile_arn,
            None,
        )
        .await;
        assert!(
            unfiltered.contains(&row.contact_id),
            "case {} in {}: the unfiltered listing must answer the contact, got {unfiltered:?}",
            row.coupling,
            row.status
        );

        for (filter_case, ephemeris_id) in &filters {
            let case = format!("{} in {}, filtered by {filter_case}", row.coupling, row.status);
            let filtered = listed_contacts(
                &router,
                &case,
                &statuses,
                &row.satellite_arn,
                &profile_arn,
                Some(ephemeris_id),
            )
            .await;
            assert!(
                filtered.iter().all(|id| unfiltered.contains(id)),
                "case {case}: a filter only narrows, so it can answer nothing the unfiltered \
                 listing did not, got {filtered:?} against {unfiltered:?}"
            );
            let expected = *ephemeris_id == row.uses;
            assert_eq!(
                filtered.contains(&row.contact_id),
                expected,
                "case {case}: the filter must select the contact exactly when the ephemeris it \
                 names points it (expected {expected}), got {filtered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_terminal_contact_is_filtered_by_the_ephemeris_it_no_longer_holds() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, &format!("{}={GROUND_STATION}", NORAD.0));
    let [flown] = <[String; 1]>::try_from(onboarded).expect("one satellite is on-boarded");
    let flown_arn = satellite_arn(&engine, &flown);
    let profile_arn = create_mission_profile(&router, "terminal filter", 60, None, None).await;
    let pointing = create_ok(
        &router,
        "the ephemeris in force",
        az_el_ephemeris("pointing a terminal contact flew", Some(&flown), true),
    )
    .await;
    let contact_id = reserve_ok(
        &router,
        "a contact that reaches an end",
        &profile_arn,
        &flown_arn,
        None,
        0,
    )
    .await;
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);
    listed_contacts(
        &router,
        "settle what the clock owes",
        CONTACT_STATUSES,
        &flown_arn,
        &profile_arn,
        None,
    )
    .await;

    // The whole terminal half of the enum: the ends of the lifecycle are
    // `state::TERMINAL_CONTACT_STATUSES`, and the filter answers the contact in every one of
    // them, exactly as it does while the contact is still to be flown.
    for status in TERMINAL_CONTACT_STATUSES {
        assert!(
            CONTACT_STATUSES.contains(status),
            "case {status}: a terminal status must be a modelled ContactStatus"
        );
        put_in_status(&engine, &contact_id, status);
        let filtered = listed_contacts(
            &router,
            status,
            &[status],
            &flown_arn,
            &profile_arn,
            Some(&pointing),
        )
        .await;
        assert_eq!(
            filtered,
            vec![contact_id.clone()],
            "case {status}: a contact at an end of the lifecycle still used the ephemeris that \
             points it, so the filter must answer it, got {filtered:?}"
        );
    }

    // And the other question, on the same store: nothing holds the ephemeris any longer, so
    // the delete the in-use rule guards is ANSWERED rather than refused.
    let deleted = send(&router, "DELETE", &format!("/ephemeris/{pointing}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case delete while only a terminal contact points it: expected 200, got {} {}",
        deleted.status, deleted.body
    );

    // With the ephemeris gone it points nothing, so the filter naming it selects nothing —
    // while the same listing without the filter still answers the contact.
    let last_status = TERMINAL_CONTACT_STATUSES
        .last()
        .expect("the terminal set is not empty");
    let orphaned = listed_contacts(
        &router,
        "a filter naming a deleted ephemeris",
        &[last_status],
        &flown_arn,
        &profile_arn,
        Some(&pointing),
    )
    .await;
    assert!(
        orphaned.is_empty(),
        "case a filter naming a deleted ephemeris: it points nothing, so the page is empty, got \
         {orphaned:?}"
    );
    let unfiltered = listed_contacts(
        &router,
        "unfiltered after the delete",
        &[last_status],
        &flown_arn,
        &profile_arn,
        None,
    )
    .await;
    assert_eq!(
        unfiltered,
        vec![contact_id.clone()],
        "case unfiltered after the delete: the contact itself is untouched by the delete, got \
         {unfiltered:?}"
    );
}

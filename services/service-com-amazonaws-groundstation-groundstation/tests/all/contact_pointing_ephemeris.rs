// SPDX-License-Identifier: CC0-1.0

//! The ephemeris a read of a contact reports, driven through `ReserveContact`
//! (`POST /contact`), `UpdateContact` (`POST /contact/{contactId}/versions`),
//! `DescribeContact` (`GET /contact/{contactId}`), `DescribeContactVersion`
//! (`GET /contact/{contactId}/versions/{versionId}`) and `ListContacts` (`POST /contacts`),
//! end to end over HTTP through the same front door as the rest of this target's suites.
//!
//! The RULE under test: `ephemeris` is "the ephemeris that determines antenna pointing
//! directions for the contact" (`smithy_query(query="doc",
//! shape="DescribeContactResponse", member="ephemeris")`, and the same sentence on
//! `shape="ContactData"`), so every read of a contact reports the ephemeris the
//! configuration that read is about is flown from — the one the program track of its
//! tracking overrides names when they name one, since a program track flies the antenna
//! from a supplied trajectory, and otherwise the one in force for the satellite it is flown
//! against — carrying that ephemeris's own `ephemerisId` and `EphemerisType`
//! (`smithy_query(query="shape", shape="EphemerisResponseData")`), and nothing at all when
//! no ephemeris this account holds determines it.
//!
//! So the suites below assert that rule over SETS of inputs rather than over one call:
//!
//! * a table spanning both ways a contact's pointing is determined — every member of
//!   `ProgramTrackSettings` (`azEl`, `oem`, `tle`), the satellite coupling with one
//!   ephemeris in force and with two of different priority, the two determinations crossed,
//!   and the two ways nothing determines it — each row read back through all THREE readers
//!   of a stored contact;
//! * the same rule across a version history, where each version carries its own
//!   configuration: every version reports the ephemeris IT is flown from and the contact
//!   reports the version in force's;
//! * the computed and the stored halves of one listing: a window offered under an ephemeris
//!   and the contact reserved over that window report one and the same ephemeris.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{
    CONTACT_AVAILABLE, EPHEMERIS_TYPE_AZ_EL, EPHEMERIS_TYPE_OEM, EPHEMERIS_TYPE_TLE,
    FIRST_VERSION_ID,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::available_contacts::{
    availability, az_el, ephemeris_body, list_ok, now_seconds, Segment, GROUND_STATION,
};
use crate::ephemeris_catalog::{
    az_el_inline, create_ok, onboard, program_track_ephemeris, tle_inline, wait_out_validation,
};
use crate::ephemeris_deletion::satellite_arn;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The NORAD catalogue numbers this suite's satellites are on-boarded under, inside
/// `state::NORAD_SATELLITE_ID_RANGE`.
const NORAD: (i32, i32, i32) = (25_994, 27_424, 43_013);

/// The validity range the pointing data of every fixed-window ephemeris here declares.
const DATA_RANGE: (i64, i64) = (1_800_000_000, 1_800_086_400);

/// The window the listings of the table suite are taken over: wide enough to hold every
/// contact reserved below, so what a row's answer differs from another's by is the
/// configuration the row names and not the window.
const WINDOW: (i64, i64) = (1_799_000_000, 1_803_000_000);

/// The statuses a reserved contact of this suite holds while it is read: a reservation is
/// admitted `SCHEDULING` and the scheduler moves it to `SCHEDULED` on the clock alone, so a
/// listing that must answer every stored contact names both.
const RESERVED_STATUSES: [&str; 2] = ["SCHEDULING", "SCHEDULED"];

/// `TrackingOverrides` whose program track names `ephemeris_id` in the `ProgramTrackSettings`
/// member `member` — `azEl`, `oem` or `tle`, the three members of that union
/// (`smithy_query(query="shape", shape="ProgramTrackSettings")`).
fn program_track(member: &str, ephemeris_id: &str) -> Value {
    json!({"programTrackSettings": {member: {"ephemerisId": ephemeris_id}}})
}

/// `ReserveContact` for the window a day per `slot` after [`DATA_RANGE`]'s start, flown
/// against `satellite` when the row names one and carrying `tracking` when it names
/// overrides; answers the `contactId` it minted.
///
/// `slot` moves each row's window, because an antenna serves one pass at a time: a second
/// reservation of the same window at one ground station is refused.
async fn reserve_ok(
    router: &MultiServiceRouter,
    case: &str,
    profile_arn: &str,
    satellite: Option<&str>,
    tracking: Option<Value>,
    slot: usize,
) -> String {
    let start = DATA_RANGE.0 + i64::try_from(slot).expect("a row index fits in i64") * 86_400;
    let mut body = json!({
        "missionProfileArn": profile_arn,
        "groundStation": GROUND_STATION,
        "startTime": start,
        "endTime": start + 600,
    });
    if let Some(satellite) = satellite {
        body["satelliteArn"] = json!(satellite);
    }
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

/// The `ephemeris` member of `body`, as `(ephemerisId, ephemerisType)`; `None` when the
/// read reports no ephemeris at all.
///
/// A present member is read through both of its members, so a report that drops the id or
/// the type is a different answer from one that carries them.
fn reported(body: &Value) -> Option<(String, String)> {
    let member = body.get("ephemeris")?;
    if member.is_null() {
        return None;
    }
    Some((
        member
            .get("ephemerisId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        member
            .get("ephemerisType")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ))
}

/// What a row expects, in the form [`reported`] answers in.
fn expected(ephemeris: Option<(&str, &str)>) -> Option<(String, String)> {
    ephemeris.map(|(id, kind)| (id.to_string(), kind.to_string()))
}

/// `DescribeContact`, asserting it answered.
async fn describe_contact(router: &MultiServiceRouter, contact_id: &str, case: &str) -> Reply {
    let reply = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// `DescribeContactVersion` for one version of one contact, asserting it answered.
async fn describe_version(
    router: &MultiServiceRouter,
    contact_id: &str,
    version_id: i32,
    case: &str,
) -> Reply {
    let reply = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeContactVersion 200, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// One row of the table: the configuration a contact is reserved with, and the ephemeris
/// every read of it must report.
struct Row {
    case: &'static str,
    /// The satellite ARN the row's contact is flown against, when it names one.
    satellite: Option<String>,
    /// The tracking overrides it is reserved with, when it names any.
    tracking: Option<Value>,
    /// The `(ephemerisId, ephemerisType)` the reads must report, or `None` when nothing
    /// this account holds determines the row's pointing.
    expected: Option<(String, String)>,
    /// The contact the row reserved, once it has been.
    contact_id: String,
}

/// One version an update appends: what it names, and the ephemeris the version it appends
/// must be described with.
struct Update {
    case: &'static str,
    /// The `UpdateContact` body the row sends.
    body: Value,
    /// The `(ephemerisId, ephemerisType)` the version it appends must report.
    expected: Option<(String, String)>,
}

#[tokio::test]
async fn every_read_of_a_contact_reports_the_ephemeris_that_determines_its_pointing() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(
        &engine,
        &format!(
            "{}={GROUND_STATION},{}={GROUND_STATION},{}={GROUND_STATION}",
            NORAD.0, NORAD.1, NORAD.2
        ),
    );
    let [pointed, contested, bare] =
        <[String; 3]>::try_from(onboarded).expect("three satellites are on-boarded");
    let pointed_arn = satellite_arn(&engine, &pointed);
    let contested_arn = satellite_arn(&engine, &contested);
    let bare_arn = satellite_arn(&engine, &bare);
    let profile_arn = create_mission_profile(&router, "contact pointing", 60, None, None).await;

    // One ephemeris of each form a program track may name, stored through `CreateEphemeris`:
    // each applies to no satellite, so NAMING it is the only thing that points a contact at
    // it.
    let tracked_az_el = program_track_ephemeris(&router, "azEl").await;
    let tracked_oem = program_track_ephemeris(&router, "oem").await;
    let tracked_tle = program_track_ephemeris(&router, "tle").await;

    // The ephemeris in force for the first satellite: the only one the account holds for it.
    let in_force = create_ok(
        &router,
        "the ephemeris in force",
        ephemeris_body(
            "pointing in force",
            Some(&pointed),
            true,
            None,
            az_el_inline(&[DATA_RANGE]),
        ),
    )
    .await;
    // Two ephemerides for the second satellite, of different `priority` — "higher numbers
    // take precedence" (`smithy_query(query="doc", shape="CreateEphemerisRequest",
    // member="priority")`) — so the one in force is the higher-priority one, and it is of
    // another `EphemerisType` than the one it supersedes: the type reported is the type of
    // the ephemeris that determines the pointing, not a constant.
    let superseded = create_ok(
        &router,
        "the superseded ephemeris",
        ephemeris_body(
            "lower priority",
            Some(&contested),
            true,
            Some(1),
            tle_inline(&[DATA_RANGE]),
        ),
    )
    .await;
    let takes_precedence = create_ok(
        &router,
        "the higher-priority ephemeris",
        ephemeris_body(
            "higher priority",
            Some(&contested),
            true,
            Some(2),
            az_el_inline(&[DATA_RANGE]),
        ),
    )
    .await;
    assert_ne!(
        superseded, takes_precedence,
        "the two ephemerides held for one satellite are distinct"
    );

    // The rows: both ways a contact's pointing is determined, each form of program track,
    // the precedence among several ephemerides of one satellite, the two determinations
    // crossed on ONE contact, and the two ways nothing determines the pointing at all.
    //
    // Each row's contact is reserved below, one window per row.
    let mut table: Vec<Row> = vec![
        Row {
            case: "a program track naming an azEl ephemeris",
            satellite: Some(bare_arn.clone()),
            tracking: Some(program_track("azEl", &tracked_az_el)),
            expected: expected(Some((&tracked_az_el, EPHEMERIS_TYPE_AZ_EL))),
            contact_id: String::new(),
        },
        Row {
            case: "a program track naming an oem ephemeris",
            satellite: Some(bare_arn.clone()),
            tracking: Some(program_track("oem", &tracked_oem)),
            expected: expected(Some((&tracked_oem, EPHEMERIS_TYPE_OEM))),
            contact_id: String::new(),
        },
        Row {
            case: "a program track naming a tle ephemeris",
            satellite: Some(bare_arn.clone()),
            tracking: Some(program_track("tle", &tracked_tle)),
            expected: expected(Some((&tracked_tle, EPHEMERIS_TYPE_TLE))),
            contact_id: String::new(),
        },
        Row {
            case: "no overrides, one ephemeris in force for the satellite flown",
            satellite: Some(pointed_arn.clone()),
            tracking: None,
            expected: expected(Some((&in_force, EPHEMERIS_TYPE_AZ_EL))),
            contact_id: String::new(),
        },
        Row {
            case: "no overrides, the higher-priority ephemeris of the satellite flown",
            satellite: Some(contested_arn.clone()),
            tracking: None,
            expected: expected(Some((&takes_precedence, EPHEMERIS_TYPE_AZ_EL))),
            contact_id: String::new(),
        },
        Row {
            case: "a program track on a satellite that also has an ephemeris in force",
            satellite: Some(pointed_arn.clone()),
            tracking: Some(program_track("oem", &tracked_oem)),
            expected: expected(Some((&tracked_oem, EPHEMERIS_TYPE_OEM))),
            contact_id: String::new(),
        },
        Row {
            case: "no overrides and no ephemeris in force for the satellite flown",
            satellite: Some(bare_arn.clone()),
            tracking: None,
            expected: None,
            contact_id: String::new(),
        },
        Row {
            case: "no satellite and no overrides",
            satellite: None,
            tracking: None,
            expected: None,
            contact_id: String::new(),
        },
    ];

    for (slot, row) in table.iter_mut().enumerate() {
        row.contact_id = reserve_ok(
            &router,
            row.case,
            &profile_arn,
            row.satellite.as_deref(),
            row.tracking.clone(),
            slot,
        )
        .await;
    }

    // ONE wait for the whole table: every ephemeris finishes validating on the clock alone,
    // and only an ephemeris that has validated and was asked to be enabled is in force.
    wait_out_validation();
    std::thread::sleep(AFTER_SETTLE);

    // The stored half of one listing, so every row is also read as `ListContacts` reports
    // it rather than only through the two describes.
    let listed = list_ok(
        &router,
        "the stored contacts",
        json!({
            "statusList": RESERVED_STATUSES,
            "startTime": WINDOW.0,
            "endTime": WINDOW.1,
        }),
    )
    .await;

    for row in &table {
        let case = row.case;
        let described = describe_contact(&router, &row.contact_id, case).await;
        assert_eq!(
            reported(&described.body),
            row.expected,
            "case {case}: DescribeContact must report the ephemeris that determines the \
             contact's antenna pointing, got {}",
            described.field("ephemeris")
        );
        // The overrides the read reports are the ones the pointing was read from, so the
        // two members cannot drift apart.
        assert_eq!(
            described.body.get("trackingOverrides").cloned(),
            row.tracking,
            "case {case}: DescribeContact must report the overrides the contact is flown \
             with, got {}",
            described.field("trackingOverrides")
        );
        assert_eq!(
            described
                .body
                .get("satelliteArn")
                .and_then(Value::as_str)
                .map(str::to_string),
            row.satellite,
            "case {case}: DescribeContact must report the satellite the contact flies, got {}",
            described.field("satelliteArn")
        );

        let version = describe_version(&router, &row.contact_id, FIRST_VERSION_ID, case).await;
        assert_eq!(
            reported(&version.body),
            row.expected,
            "case {case}: DescribeContactVersion must report the ephemeris this version's \
             configuration points the antenna with, got {}",
            version.field("ephemeris")
        );

        let item = listed
            .iter()
            .find(|item| item.get("contactId").and_then(Value::as_str) == Some(&row.contact_id))
            .unwrap_or_else(|| {
                panic!("case {case}: the listing must answer the stored contact, got {listed:?}")
            });
        assert_eq!(
            reported(item),
            row.expected,
            "case {case}: a stored contact's listed item must report the same ephemeris the \
             describes do, got {item}"
        );
    }
}

#[tokio::test]
async fn each_version_reports_the_ephemeris_its_own_configuration_points_the_antenna_with() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, &format!("{}={GROUND_STATION}", NORAD.0));
    let [satellite] = <[String; 1]>::try_from(onboarded).expect("one satellite is on-boarded");
    let satellite = satellite_arn(&engine, &satellite);
    let profile_arn = create_mission_profile(&router, "version pointing", 60, None, None).await;
    let first = program_track_ephemeris(&router, "azEl").await;
    let second = program_track_ephemeris(&router, "tle").await;

    let contact_id = reserve_ok(
        &router,
        "reserved with a program track",
        &profile_arn,
        Some(&satellite),
        Some(program_track("azEl", &first)),
        0,
    )
    .await;

    // Each update appends a version, which takes force on the clock alone. The second row
    // names NO overrides, so the version it appends inherits the ones before it: the
    // ephemeris reported for it is the inherited program track's, not nothing.
    let updates: Vec<Update> = vec![
        Update {
            case: "a program track of another form",
            body: json!({"trackingOverrides": program_track("tle", &second)}),
            expected: expected(Some((&second, EPHEMERIS_TYPE_TLE))),
        },
        Update {
            case: "an update naming no overrides",
            body: json!({"satelliteArn": satellite.clone()}),
            expected: expected(Some((&second, EPHEMERIS_TYPE_TLE))),
        },
    ];

    // The version a reservation mints, plus one per update: each entry is what the version
    // at that position must report.
    let mut per_version: Vec<(&str, Option<(String, String)>)> = vec![(
        "the version the reservation minted",
        expected(Some((&first, EPHEMERIS_TYPE_AZ_EL))),
    )];
    for update in updates {
        let case = update.case;
        let updated = send(
            &router,
            "POST",
            &format!("/contact/{contact_id}/versions"),
            Some(update.body),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateContact 200, got {} {}",
            updated.status, updated.body
        );
        per_version.push((case, update.expected));
    }
    // The version an update appends takes force after the span this service owes it, so the
    // read below is taken once that move has fallen due.
    std::thread::sleep(AFTER_SETTLE);

    for (position, (case, pointing)) in per_version.iter().enumerate() {
        let version_id = FIRST_VERSION_ID + i32::try_from(position).expect("a position fits");
        let reply = describe_version(&router, &contact_id, version_id, case).await;
        assert_eq!(
            &reported(&reply.body),
            pointing,
            "case {case}: version {version_id} must report the ephemeris its OWN overrides \
             point the antenna with, got {}",
            reply.field("ephemeris")
        );
    }

    // And the contact itself reports the version IN FORCE's — the newest, now that every
    // version move the clock owed has fallen due.
    let described = describe_contact(&router, &contact_id, "the version in force").await;
    let (_, newest) = per_version
        .last()
        .expect("the history holds at least the reserved version");
    assert_eq!(
        &reported(&described.body),
        newest,
        "case the version in force: DescribeContact reports the ephemeris the newest version \
         points the antenna with, got {}",
        described.field("ephemeris")
    );
}

#[tokio::test]
async fn a_contact_reserved_over_an_offered_window_reports_the_ephemeris_it_was_offered_under() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, &format!("{}={GROUND_STATION}", NORAD.1));
    let [satellite_id] = <[String; 1]>::try_from(onboarded).expect("one satellite is on-boarded");
    let satellite = satellite_arn(&engine, &satellite_id);
    // Pointing data putting ONE pass above the horizon, placed relative to the clock: an
    // availability search offers what can still be flown.
    let base = now_seconds() + 3_600;
    let pointing = create_ok(
        &router,
        "the pointing data a window is offered from",
        ephemeris_body(
            "offered pointing",
            Some(&satellite_id),
            true,
            None,
            az_el(
                GROUND_STATION,
                "DEGREE_ANGLE",
                &[Segment {
                    reference_epoch: base,
                    valid: (base, base + 3_600),
                    points: vec![
                        (0.0, -5.0),
                        (60.0, 10.0),
                        (120.0, 45.5),
                        (180.0, 8.0),
                        (240.0, -3.0),
                    ],
                }],
            ),
        ),
    )
    .await;
    wait_out_validation();
    let profile_arn = create_mission_profile(&router, "offered pointing", 60, None, None).await;
    let window = (base - 7_200, base + 86_400);

    let offered = list_ok(
        &router,
        "the offered windows",
        availability(window, &satellite, &profile_arn, json!({})),
    )
    .await;
    let first = offered
        .first()
        .unwrap_or_else(|| panic!("case the offered windows: a pass is offered, got {offered:?}"));
    assert_eq!(
        reported(first),
        expected(Some((&pointing, EPHEMERIS_TYPE_AZ_EL))),
        "case the offered windows: a computed window carries the ephemeris its geometry was \
         read from, got {first}"
    );
    let taken = (
        first
            .get("startTime")
            .and_then(Value::as_f64)
            .expect("an offered window carries a startTime"),
        first
            .get("endTime")
            .and_then(Value::as_f64)
            .expect("an offered window carries an endTime"),
    );

    let reserved = send(
        &router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": GROUND_STATION,
            "satelliteArn": satellite,
            "startTime": taken.0,
            "endTime": taken.1,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "case reserve the offered window: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    let contact_id = reserved.string("contactId");
    std::thread::sleep(AFTER_SETTLE);

    // The reservation is flown on the trajectory the window was offered from, so the
    // contact reports exactly the ephemeris the offer did.
    let described = describe_contact(&router, &contact_id, "the reservation").await;
    assert_eq!(
        reported(&described.body),
        expected(Some((&pointing, EPHEMERIS_TYPE_AZ_EL))),
        "case the reservation: a contact reserved over an offered window reports the \
         ephemeris that window was offered under, got {}",
        described.field("ephemeris")
    );

    // Both halves of ONE listing: the stored contact and whatever is still offered for the
    // same pairing report the same ephemeris, so a caller cannot tell the member's meaning
    // apart between them.
    let both = list_ok(
        &router,
        "both halves of one listing",
        json!({
            "statusList": [CONTACT_AVAILABLE, RESERVED_STATUSES[0], RESERVED_STATUSES[1]],
            "startTime": window.0,
            "endTime": window.1,
            "groundStation": GROUND_STATION,
            "satelliteArn": satellite,
            "missionProfileArn": profile_arn,
        }),
    )
    .await;
    assert!(
        both.iter()
            .any(|item| item.get("contactId").and_then(Value::as_str) == Some(contact_id.as_str())),
        "case both halves of one listing: the stored contact must be on the page, got {both:?}"
    );
    for item in &both {
        assert_eq!(
            reported(item),
            expected(Some((&pointing, EPHEMERIS_TYPE_AZ_EL))),
            "case both halves of one listing: every item is pointed by the one ephemeris in \
             force for this pairing, got {item}"
        );
    }
}

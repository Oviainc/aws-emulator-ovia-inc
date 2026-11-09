// SPDX-License-Identifier: CC0-1.0

//! `DescribeContactVersion`: one contact AS OF one of its versions, end to end over HTTP.
//!
//! The RULE under test: `GET /contact/{contactId}/versions/{versionId}` describes the
//! contact the first label names as of the version the second one names. The
//! contact-level facts — the window, the ground station, the status, the region, the
//! mission profile and the dataflows of its edges, the tags — are the contact's own and
//! read identically for every version of it and identically to `DescribeContact`; the
//! version-scoped facts are the NAMED version's: the satellite that version flies the
//! contact against, the tracking overrides it flies it with, and that version's own
//! record. A `contactId` naming no contact, or a `versionId` inside the modelled range
//! that the contact holds no version at, is refused with `ResourceNotFoundException`
//! (HTTP 434) rather than described as something else.
//!
//! The suites assert that rule over a SET of inputs rather than one call:
//!
//! * a history grown one version at a time by `UpdateContact` calls differing along which
//!   members they name and which `ProgramTrackSettings` member their overrides select,
//!   with EVERY version of the contact described back after each step and cross-checked
//!   against `DescribeContact` and `ListContactVersions`;
//! * a contact reserved with neither a satellite nor overrides, so the version-scoped
//!   members are asserted absent as well as present, and inherited by a later version;
//! * a table of `versionId`s the contact holds no version at — including the published
//!   upper bound of the modelled range — and a table of well-formed `Uuid`s naming no
//!   contact, each refused;
//! * each published bound of the modelled `versionId` range and one step past either end;
//! * the same version described twice, and described across the clock-owed `UPDATING` ->
//!   `ACTIVE` move an update leaves behind.
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_satellite_reference_integrity::onboard_satellite_arns;
use crate::ephemeris_catalog::program_track_ephemeris;
use crate::scheduling_loop::{
    create_config, create_tracking_config, harness_with_engine, send, spectrum_config,
    stock_ground_stations, Reply, ABSENT_UUID, AFTER_SETTLE, RECORDING_BUCKET, STATION,
};

/// The window every reservation in this suite books: far enough ahead that the contact
/// stays `SCHEDULED` for the whole run, so a contact-level fact cannot change between two
/// reads of it.
const CONTACT_START: i64 = 2_000_000_000;
const CONTACT_END: i64 = CONTACT_START + 600;

/// How far the mission profile under test widens the contact's window on either side, so
/// `prePassStartTime` and `postPassEndTime` are facts of their own rather than copies of
/// `startTime` and `endTime`.
const PRE_PASS_SECONDS: i64 = 60;
const POST_PASS_SECONDS: i64 = 30;

/// The members of this operation's output that belong to the CONTACT rather than to one
/// of its versions, so every version of a contact must report them identically — and
/// identically to `DescribeContact`.
///
/// Source: the output of `smithy_query(query="operation",
/// operation="DescribeContactVersion")`, less the four version-scoped members
/// (`satelliteArn`, `trackingOverrides`, `ephemeris`, `version`) and the four this emulator
/// reports absent because nothing has flown a pass ([`UNFLOWN_MEMBERS`]).
const CONTACT_LEVEL_MEMBERS: &[&str] = &[
    "contactId",
    "missionProfileArn",
    "startTime",
    "endTime",
    "prePassStartTime",
    "postPassEndTime",
    "groundStation",
    "contactStatus",
    "region",
    "tags",
    "dataflowList",
];

/// The output members that report what a FLOWN pass produced: the error a contact or a
/// dataflow failed with and the geometry an antenna tracked (`maximumElevation`,
/// `visibilityStartTime`, `visibilityEndTime`). Nothing in this emulator flies a pass, so a
/// description reports none of them.
///
/// `ephemeris` is NOT one of them: it is the ephemeris that DETERMINES the antenna pointing
/// (`smithy_query(query="doc", shape="DescribeContactVersionResponse",
/// member="ephemeris")`), which is settled by the configuration a version carries rather
/// than by a flown pass — so it is version-scoped, asserted here from each version's own
/// overrides ([`expected_ephemeris`]) and over the whole rule in
/// `crate::contact_pointing_ephemeris`.
const UNFLOWN_MEMBERS: &[&str] = &[
    "errorMessage",
    "maximumElevation",
    "visibilityStartTime",
    "visibilityEndTime",
];

/// `TrackingOverrides` selecting the `azEl`, `oem` or `tle` member of
/// `ProgramTrackSettings` (`smithy_query(query="shape", shape="ProgramTrackSettings")` —
/// all three of them) for the ephemeris `uuid`.
fn overrides(member: &str, uuid: &str) -> Value {
    json!({"programTrackSettings": {member: {"ephemerisId": uuid}}})
}

/// The `ephemeris` a version carrying the overrides `tracking` must be described with: the
/// ephemeris its program track names, under the `EphemerisType` the member naming it
/// requires — `azEl` -> `AZ_EL`, `oem` -> `OEM`, `tle` -> `TLE`, the three members of
/// `ProgramTrackSettings` (`smithy_query(query="shape", shape="ProgramTrackSettings")`).
///
/// `Value::Null` when the version's overrides name no program track: no ephemeris this
/// suite stores applies to a satellite, so nothing else determines the pointing of a
/// contact here (the satellite coupling is `crate::contact_pointing_ephemeris`'s subject).
fn expected_ephemeris(tracking: &Option<Value>) -> Value {
    let Some(settings) = tracking
        .as_ref()
        .and_then(|overrides| overrides.get("programTrackSettings"))
        .and_then(Value::as_object)
    else {
        return Value::Null;
    };
    let (member, reference) = settings
        .iter()
        .next()
        .expect("a programTrackSettings names one member");
    let ephemeris_type = match member.as_str() {
        "azEl" => state::EPHEMERIS_TYPE_AZ_EL,
        "oem" => state::EPHEMERIS_TYPE_OEM,
        "tle" => state::EPHEMERIS_TYPE_TLE,
        other => panic!("case {other}: not a ProgramTrackSettings member"),
    };
    json!({
        "ephemerisId": reference.get("ephemerisId").cloned().unwrap_or(Value::Null),
        "ephemerisType": ephemeris_type,
    })
}

/// A mission profile with ONE dataflow edge, answering its ARN.
///
/// An edge is what makes `dataflowList` a non-empty contact-level fact, so the assertion
/// that every version reports the same dataflows is about something. `antenna-downlink` ->
/// `s3-recording` is one of the pairs this service admits
/// (`state::DATAFLOW_EDGE_CAPABILITY_PAIRS`).
async fn create_profile_with_an_edge(router: &MultiServiceRouter, name: &str) -> String {
    let tracking_arn = create_tracking_config(router, &format!("{name} tracking")).await;
    let (_, _, downlink_arn) = create_config(
        router,
        &format!("{name} downlink"),
        json!({"antennaDownlinkConfig": {"spectrumConfig": spectrum_config(7812.5, 30.5)}}),
        None,
    )
    .await;
    let (_, _, recording_arn) = create_config(
        router,
        &format!("{name} recording"),
        json!({"s3RecordingConfig": {
            "bucketArn": format!("arn:aws:s3:::{RECORDING_BUCKET}"),
            "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
            "prefix": "data/{satellite_id}/{year}/{month}/{day}",
        }}),
        None,
    )
    .await;

    let created = send(
        router,
        "POST",
        "/missionprofile",
        Some(json!({
            "name": name,
            "minimumViableContactDurationSeconds": 180,
            "dataflowEdges": [[downlink_arn, recording_arn]],
            "trackingConfigArn": tracking_arn,
            "contactPrePassDurationSeconds": PRE_PASS_SECONDS,
            "contactPostPassDurationSeconds": POST_PASS_SECONDS,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateMissionProfile {name}: expected 200, got {} {}",
        created.status, created.body
    );
    let id = created.string("missionProfileId");
    let got = send(router, "GET", &format!("/missionprofile/{id}"), None).await;
    assert_eq!(
        got.status, 200,
        "GetMissionProfile {name}: expected 200, got {} {}",
        got.status, got.body
    );
    got.string("missionProfileArn")
}

/// `ReserveContact`, asserting it was accepted at the first version, and answering the
/// `contactId`.
async fn reserve_contact(
    router: &MultiServiceRouter,
    profile_arn: &str,
    satellite: Option<&str>,
    tracking: Option<Value>,
) -> String {
    let mut body = json!({
        "missionProfileArn": profile_arn,
        "groundStation": STATION,
        "startTime": CONTACT_START,
        "endTime": CONTACT_END,
        "tags": {"mission": "version-description"},
    });
    if let Some(arn) = satellite {
        body["satelliteArn"] = json!(arn);
    }
    if let Some(tracking) = tracking {
        body["trackingOverrides"] = tracking;
    }
    let reserved = send(router, "POST", "/contact", Some(body)).await;
    assert_eq!(
        reserved.status, 200,
        "ReserveContact: expected 200, got {} {}",
        reserved.status, reserved.body
    );
    assert_eq!(
        reserved.field("versionId"),
        &json!(state::FIRST_VERSION_ID),
        "ReserveContact: a reserved contact starts at version {}, got {}",
        state::FIRST_VERSION_ID,
        reserved.field("versionId")
    );
    reserved.string("contactId")
}

/// `UpdateContact` (`POST /contact/{contactId}/versions`), asserting it was accepted and
/// answering the `versionId` it appended.
async fn append_version(
    router: &MultiServiceRouter,
    contact_id: &str,
    case: &str,
    body: Value,
) -> i64 {
    let updated = send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case}: expected UpdateContact 200, got {} {}",
        updated.status, updated.body
    );
    updated
        .field("versionId")
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: UpdateContact answers a versionId, got {}", updated.body))
}

/// `DescribeContactVersion` for one version of one contact.
async fn describe_version(
    router: &MultiServiceRouter,
    contact_id: &str,
    version_id: i64,
) -> Reply {
    send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await
}

/// `DescribeContactVersion`, asserting it answered.
async fn described_version(
    router: &MultiServiceRouter,
    contact_id: &str,
    version_id: i64,
    case: &str,
) -> Reply {
    let reply = describe_version(router, contact_id, version_id).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeContactVersion 200, got {} {}",
        reply.status, reply.body
    );
    reply
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

/// The records `ListContactVersions` reports for `contact_id`, oldest first.
async fn listed_versions(router: &MultiServiceRouter, contact_id: &str, case: &str) -> Vec<Value> {
    let reply = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions"),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListContactVersions 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .field("contactVersionsList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactVersionsList must be a list, got {}",
                reply.body
            )
        })
        .clone()
}

/// What one version of the contact must be described with: the satellite it flies the
/// contact against and the tracking overrides it flies it with, each absent when no
/// version up to it named one.
type VersionConfiguration = (Option<String>, Option<Value>);

/// Describe EVERY version of `contact_id` and assert the rule on each: the version-scoped
/// members are `expected[versionId - 1]`, the version record is the one
/// `ListContactVersions` reports at that position, the contact-level members are the ones
/// `DescribeContact` reports, and the unflown members carry nothing. The NEWEST version is
/// also asserted to describe the contact exactly as `DescribeContact` does, since that is
/// the version in force.
async fn assert_every_version_describes_its_own_configuration(
    router: &MultiServiceRouter,
    contact_id: &str,
    expected: &[VersionConfiguration],
    case: &str,
) {
    let contact = describe_contact(router, contact_id, case).await;
    let listed = listed_versions(router, contact_id, case).await;
    assert_eq!(
        listed.len(),
        expected.len(),
        "case {case}: the contact must hold {} versions, got {listed:?}",
        expected.len()
    );

    for (index, (satellite, tracking)) in expected.iter().enumerate() {
        let version_id = index as i64 + i64::from(state::FIRST_VERSION_ID);
        let case = format!("{case}, version {version_id}");
        let reply = described_version(router, contact_id, version_id, &case).await;

        assert_eq!(
            reply
                .body
                .get("satelliteArn")
                .and_then(Value::as_str)
                .map(str::to_string),
            *satellite,
            "case {case}: expected the satellite this version flies the contact against \
             ({satellite:?}), got {}",
            reply.field("satelliteArn")
        );
        assert_eq!(
            reply.body.get("trackingOverrides").cloned(),
            *tracking,
            "case {case}: expected the overrides this version flies the contact with \
             ({tracking:?}), got {}",
            reply.field("trackingOverrides")
        );
        assert_eq!(
            reply.field("version"),
            &listed[index],
            "case {case}: the version record must be the one the listing reports, got {} and {}",
            reply.field("version"),
            listed[index]
        );
        assert_eq!(
            reply.field("version").get("versionId").cloned(),
            Some(json!(version_id)),
            "case {case}: the record must be the version the path asked for, got {}",
            reply.field("version")
        );

        for member in CONTACT_LEVEL_MEMBERS {
            assert_eq!(
                reply.field(member),
                contact.field(member),
                "case {case}: {member} belongs to the contact, so every version reports what \
                 DescribeContact reports: got {} and {}",
                reply.field(member),
                contact.field(member)
            );
        }
        assert_eq!(
            reply.body.get("ephemeris").cloned().unwrap_or(Value::Null),
            expected_ephemeris(tracking),
            "case {case}: expected the ephemeris this version's program track points the \
             antenna with, got {}",
            reply.field("ephemeris")
        );
        for member in UNFLOWN_MEMBERS {
            assert_eq!(
                reply.body.get(*member).unwrap_or(&Value::Null),
                &Value::Null,
                "case {case}: {member} is reported only for a flown pass, got {}",
                reply.field(member)
            );
        }

        // The newest version is the one in force, so describing it answers exactly what
        // describing the contact answers.
        if version_id == expected.len() as i64 {
            assert_eq!(
                reply.body, contact.body,
                "case {case}: the newest version describes the contact as DescribeContact \
                 does, got {} and {}",
                reply.body, contact.body
            );
        }
    }
}

#[tokio::test]
async fn every_version_is_described_with_the_configuration_that_version_carried() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_profile_with_an_edge(&router, "version description").await;

    // A contact is flown against a satellite of THIS account, so the satellites the rows
    // below name are on-boarded first (`crate::contact_satellite_reference_integrity`): an
    // ARN no onboarding carries references nothing and is refused.
    let satellites = onboard_satellite_arns(&engine, 3);
    let first_satellite = satellites[0].clone();
    let second_satellite = satellites[1].clone();
    let third_satellite = satellites[2].clone();
    // One ephemeris of each form a program track may name, stored through
    // `CreateEphemeris`: the reference a contact version carries has to resolve to an
    // ephemeris this account holds, of the form the member naming it requires.
    let az_el_ephemeris = program_track_ephemeris(&router, "azEl").await;
    let oem_ephemeris = program_track_ephemeris(&router, "oem").await;
    let tle_ephemeris = program_track_ephemeris(&router, "tle").await;
    let reserved_overrides = overrides("azEl", &az_el_ephemeris);

    let contact_id = reserve_contact(
        &router,
        &profile_arn,
        Some(&first_satellite),
        Some(reserved_overrides.clone()),
    )
    .await;

    // One row per form an update takes, chained onto ONE contact so the history grows and
    // every earlier version has to keep describing what IT carried: which members the
    // request names, and which `ProgramTrackSettings` member its overrides select. The
    // last two columns are the configuration the version that row appends carries.
    let rows: Vec<(&str, Value, Option<String>, Option<Value>)> = vec![
        (
            "satelliteArn alone",
            json!({"satelliteArn": second_satellite}),
            Some(second_satellite.clone()),
            Some(reserved_overrides.clone()),
        ),
        (
            "trackingOverrides alone, oem program track",
            json!({"trackingOverrides": overrides("oem", &oem_ephemeris)}),
            Some(second_satellite.clone()),
            Some(overrides("oem", &oem_ephemeris)),
        ),
        (
            "both members, tle program track",
            json!({
                "satelliteArn": third_satellite,
                "trackingOverrides": overrides("tle", &tle_ephemeris),
            }),
            Some(third_satellite.clone()),
            Some(overrides("tle", &tle_ephemeris)),
        ),
        (
            "trackingOverrides naming no program track settings",
            json!({"trackingOverrides": {}}),
            Some(third_satellite.clone()),
            Some(json!({})),
        ),
        (
            "satelliteArn back to the one reserved",
            json!({"satelliteArn": first_satellite}),
            Some(first_satellite.clone()),
            Some(json!({})),
        ),
    ];

    // Past the settle span, so the reservation's version has taken force before the first
    // read.
    std::thread::sleep(AFTER_SETTLE);
    let mut expected: Vec<VersionConfiguration> =
        vec![(Some(first_satellite.clone()), Some(reserved_overrides))];
    assert_every_version_describes_its_own_configuration(
        &router,
        &contact_id,
        &expected,
        "reserved only",
    )
    .await;

    for (case, body, satellite, tracking) in rows {
        let appended = append_version(&router, &contact_id, case, body).await;
        expected.push((satellite, tracking));
        assert_eq!(
            appended,
            expected.len() as i64,
            "case {case}: an update appends the next version, got {appended}"
        );
        // Past the settle span, so the appended version is in force and every version
        // before it has been superseded.
        std::thread::sleep(AFTER_SETTLE);
        assert_every_version_describes_its_own_configuration(&router, &contact_id, &expected, case)
            .await;
    }
}

#[tokio::test]
async fn a_version_scoped_member_no_version_named_is_reported_absent_and_then_inherited() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn = create_profile_with_an_edge(&router, "absent configuration").await;
    // Reserved with neither a satellite nor overrides, so the first version carries none.
    let contact_id = reserve_contact(&router, &profile_arn, None, None).await;
    std::thread::sleep(AFTER_SETTLE);

    let mut expected: Vec<VersionConfiguration> = vec![(None, None)];
    assert_every_version_describes_its_own_configuration(
        &router,
        &contact_id,
        &expected,
        "reserved without a configuration",
    )
    .await;

    // An update naming ONLY the overrides leaves the satellite absent, because no version
    // up to it named one; the next one names a satellite and inherits those overrides.
    let tracking = overrides("azEl", &program_track_ephemeris(&router, "azEl").await);
    let satellite = onboard_satellite_arns(&engine, 1)[0].clone();
    for (case, body, expected_row) in [
        (
            "overrides alone, still no satellite",
            json!({"trackingOverrides": tracking.clone()}),
            (None, Some(tracking.clone())),
        ),
        (
            "satellite alone, inheriting the overrides",
            json!({"satelliteArn": satellite}),
            (Some(satellite.clone()), Some(tracking.clone())),
        ),
    ] {
        append_version(&router, &contact_id, case, body).await;
        expected.push(expected_row);
        std::thread::sleep(AFTER_SETTLE);
        assert_every_version_describes_its_own_configuration(&router, &contact_id, &expected, case)
            .await;
    }
}

#[tokio::test]
async fn a_version_the_contact_does_not_hold_is_not_described() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn = create_profile_with_an_edge(&router, "version not found").await;
    let contact_id = reserve_contact(&router, &profile_arn, None, None).await;
    std::thread::sleep(AFTER_SETTLE);

    // `versionId`s inside the modelled range (1..128 — `smithy_query(query="shape",
    // shape="VersionId")`) that this one-version contact holds no version at: the one just
    // past its newest, two in the middle, and the published upper bound.
    let absent_versions = [
        i64::from(state::FIRST_VERSION_ID) + 1,
        7,
        64,
        i64::from(state::MAX_CONTACT_VERSIONS),
    ];
    for version_id in absent_versions {
        let reply = describe_version(&router, &contact_id, version_id).await;
        assert_eq!(
            reply.status, 434,
            "case version {version_id}: expected the modelled ResourceNotFoundException \
             status 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case version {version_id}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            "Contact version not found",
            "case version {version_id}: wrong refusal text, got {}",
            reply.body
        );
        assert!(
            !reply.string("message").contains(&contact_id),
            "case version {version_id}: the refusal discloses no id, got {}",
            reply.body
        );
    }

    // The version the contact DOES hold is described instead, so the rows above are
    // refused for the version they named and not because every read is.
    let described = described_version(
        &router,
        &contact_id,
        i64::from(state::FIRST_VERSION_ID),
        "the version the contact holds",
    )
    .await;
    assert_eq!(
        described.string("contactId"),
        contact_id,
        "case the version the contact holds: another contact came back, got {}",
        described.body
    );

    // Appending a version makes the id that was absent describable, and moves the boundary
    // one along: the rule is about what the contact holds, not about a fixed id.
    append_version(
        &router,
        &contact_id,
        "boundary moves",
        json!({"satelliteArn": onboard_satellite_arns(&engine, 1)[0].clone()}),
    )
    .await;
    std::thread::sleep(AFTER_SETTLE);
    for (version_id, expected_status) in [
        (i64::from(state::FIRST_VERSION_ID) + 1, 200),
        (i64::from(state::FIRST_VERSION_ID) + 2, 434),
    ] {
        let reply = describe_version(&router, &contact_id, version_id).await;
        assert_eq!(
            reply.status, expected_status,
            "case boundary moves, version {version_id}: expected {expected_status}, got {} {}",
            reply.status, reply.body
        );
    }

    // A `contactId` naming no contact: well-formed `Uuid`s (a run of zeros, a run of `f`s
    // and a mixed one, so the refusal cannot be passing on one id's shape), each refused
    // with the SAME text an unresolvable version is — this operation addresses a contact
    // version, so either label failing to resolve leaves no contact version to describe.
    for absent in [
        ABSENT_UUID,
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    ] {
        assert_ne!(
            absent, contact_id,
            "case contact {absent}: this row must name no contact"
        );
        let reply = describe_version(&router, absent, i64::from(state::FIRST_VERSION_ID)).await;
        assert_eq!(
            reply.status, 434,
            "case contact {absent}: expected 434, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case contact {absent}: wrong error shape, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            "Contact version not found",
            "case contact {absent}: wrong refusal text, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_version_id_outside_the_modelled_range_is_a_bad_request() {
    let (router, engine) = harness_with_engine();
    stock_ground_stations(&engine, STATION);
    let profile_arn = create_profile_with_an_edge(&router, "version bounds").await;
    let contact_id = reserve_contact(&router, &profile_arn, None, None).await;
    std::thread::sleep(AFTER_SETTLE);

    // One step past either published bound of `VersionId` (range 1..128 —
    // `smithy_query(query="shape", shape="VersionId")`): the request never reaches the
    // handler, so it is refused as a bad request rather than as a missing resource.
    for version_id in [
        i64::from(state::FIRST_VERSION_ID) - 1,
        i64::from(state::MAX_CONTACT_VERSIONS) + 1,
    ] {
        let reply = describe_version(&router, &contact_id, version_id).await;
        assert_eq!(
            reply.status, 400,
            "case version {version_id}: a value outside the modelled range is refused as a \
             bad request, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            state::BAD_REQUEST_ERROR_CODE,
            "case version {version_id}: wrong error code, got {}",
            reply.body
        );
    }

    // Both published bounds are INSIDE the range, so neither is a bad request: the lower
    // one is the version the contact holds and the upper one is a version it does not.
    for (version_id, expected_status) in [
        (i64::from(state::FIRST_VERSION_ID), 200),
        (i64::from(state::MAX_CONTACT_VERSIONS), 434),
    ] {
        let reply = describe_version(&router, &contact_id, version_id).await;
        assert_eq!(
            reply.status, expected_status,
            "case version {version_id}: a value on the published bound must not be refused as \
             a bad request, got {} {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn describing_a_version_changes_nothing_a_later_caller_sees() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_profile_with_an_edge(&router, "version reads").await;
    let satellites = onboard_satellite_arns(&engine, 2);
    let reserved_satellite = satellites[0].clone();
    let contact_id =
        reserve_contact(&router, &profile_arn, Some(&reserved_satellite), None).await;
    std::thread::sleep(AFTER_SETTLE);

    let first = described_version(&router, &contact_id, 1, "first read").await;
    let second = described_version(&router, &contact_id, 1, "second read").await;
    assert_eq!(
        first.body, second.body,
        "case repeat read: a read must change nothing a later caller sees, got {} then {}",
        first.body, second.body
    );
    assert_eq!(
        first.field("version").get("status").cloned(),
        Some(json!(state::ACTIVE_VERSION_STATUS)),
        "case first read: the reservation's version is the one in force, got {}",
        first.field("version")
    );

    // An update appends a version the service owes a move to ACTIVE. Read straight after,
    // the appended version is in one of the two statuses that move spans; the version
    // before it keeps the configuration IT carried either way. The reads materialise that
    // move; they do not cause it.
    let updated_satellite = satellites[1].clone();
    append_version(
        &router,
        &contact_id,
        "pending update",
        json!({"satelliteArn": updated_satellite}),
    )
    .await;

    let pending = described_version(&router, &contact_id, 2, "pending update").await;
    let pending_status = pending
        .field("version")
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        pending_status == state::UPDATING_VERSION_STATUS
            || pending_status == state::ACTIVE_VERSION_STATUS,
        "case pending update: an appended version is applied or still being applied, got \
         {pending_status:?}"
    );
    assert_eq!(
        pending.string("satelliteArn"),
        updated_satellite,
        "case pending update: the appended version is described with what IT names, got {}",
        pending.body
    );

    std::thread::sleep(AFTER_SETTLE);

    let settled_first = described_version(&router, &contact_id, 1, "settled, version 1").await;
    assert_eq!(
        settled_first.string("satelliteArn"),
        reserved_satellite,
        "case settled, version 1: a superseded version keeps the satellite it carried, got {}",
        settled_first.body
    );
    assert_eq!(
        settled_first.field("version").get("status").cloned(),
        Some(json!(state::SUPERSEDED_VERSION_STATUS)),
        "case settled, version 1: a version a newer one replaced is superseded, got {}",
        settled_first.field("version")
    );
    assert_eq!(
        settled_first.field("version").get("created"),
        first.field("version").get("created"),
        "case settled, version 1: a version's created instant must not move, got {} and {}",
        settled_first.field("version"),
        first.field("version")
    );
    assert_eq!(
        settled_first.field("contactStatus"),
        first.field("contactStatus"),
        "case settled, version 1: an update changes no contact-level fact, got {} and {}",
        settled_first.field("contactStatus"),
        first.field("contactStatus")
    );

    let settled_second = described_version(&router, &contact_id, 2, "settled, version 2").await;
    assert_eq!(
        settled_second.field("version").get("status").cloned(),
        Some(json!(state::ACTIVE_VERSION_STATUS)),
        "case settled, version 2: the appended version has taken force, got {}",
        settled_second.field("version")
    );
    assert_eq!(
        settled_second.string("satelliteArn"),
        updated_satellite,
        "case settled, version 2: the version in force flies the satellite it named, got {}",
        settled_second.body
    );
}

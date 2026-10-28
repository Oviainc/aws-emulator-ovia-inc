// SPDX-License-Identifier: CC0-1.0

//! The ephemeris reference a contact's program track carries, end to end over HTTP.
//!
//! The RULE under test: a contact version's `trackingOverrides.programTrackSettings`
//! reference an ephemeris of THIS account BY ID, and both writers of a contact version
//! resolve that reference before storing it — an `ephemerisId` this account holds no
//! ephemeris under is refused as a missing resource (`ResourceNotFoundException`, HTTP
//! 434) and one naming an ephemeris of another `EphemerisType` than the union member
//! carrying it requires — `azEl` an `AZ_EL`, `oem` an `OEM`, `tle` a `TLE`
//! (`smithy_query(query="shape", shape="ProgramTrackSettings")`) — is refused as a bad
//! parameter (`InvalidParameterException`, HTTP 431); neither refusal stores anything,
//! and a reference that does resolve is stored and read back. While a non-terminal
//! contact's program track names an ephemeris, that ephemeris is in use and cannot be
//! deleted, so a stored reference keeps resolving.
//!
//! The suites assert that rule over SETS of inputs rather than one call:
//!
//! * every `ProgramTrackSettings` member crossed with every well-formed `Uuid` of a
//!   catalogue of ids that name nothing;
//! * every member crossed with every `EphemerisType` a stored ephemeris can carry, so the
//!   three accepted pairings and the six refused ones are asserted by the same table;
//! * the same rule on `ReserveContact`, which takes the same overrides for a contact's
//!   FIRST version;
//! * an ephemeris deleted after a contact was pointed at it, and one still held by a
//!   standing contact;
//! * overrides that name no program track at all, and an ephemeris named at each stage of
//!   its own validation.
//!
//! Every request goes through the real front door — routing, deserialization, constraint
//! validation, the handler and serialization — on the harness `scheduling_loop.rs` stands
//! up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{self, Contact};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{
    az_el_inline, create_ok, program_track_ephemeris, tle_inline, wait_out_validation, FUTURE,
    OEM_BODY,
};
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The contact window a reservation in slot 0 books; a later slot books the same window a
/// day per slot later, so each reservation of a suite holds an antenna window of its own.
const CONTACT_START: i64 = 1_900_000_000;
const CONTACT_END: i64 = CONTACT_START + 600;

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that name no
/// ephemeris: a run of zeros, a run of `f`s and a mixed one, so an assertion about a
/// refusal cannot be passing on one id's shape.
const ABSENT_EPHEMERIS_IDS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
];

/// Every member of `ProgramTrackSettings`, with the `EphemerisType` the ephemeris it names
/// has to carry.
///
/// Source: `smithy_query(query="shape", shape="ProgramTrackSettings")` — the three members
/// `azEl`, `oem` and `tle`, documented "Program track settings for AzElEphemeris /
/// OEMEphemeris / TLEEphemeris" — and `shape="EphemerisType"` for the values.
fn program_track_members() -> Vec<(&'static str, &'static str)> {
    vec![
        ("azEl", state::EPHEMERIS_TYPE_AZ_EL),
        ("oem", state::EPHEMERIS_TYPE_OEM),
        ("tle", state::EPHEMERIS_TYPE_TLE),
    ]
}

/// `TrackingOverrides` selecting `member` of `ProgramTrackSettings` for the ephemeris
/// `ephemeris_id`.
fn overrides(member: &str, ephemeris_id: &str) -> Value {
    json!({"programTrackSettings": {member: {"ephemerisId": ephemeris_id}}})
}

/// `UpdateContact` as the model routes it: `POST /contact/{contactId}/versions`.
async fn update_contact(router: &MultiServiceRouter, contact_id: &str, body: Value) -> Reply {
    send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body),
    )
    .await
}

/// `ReserveContact` for the window a day per `slot` after [`CONTACT_START`], carrying
/// `tracking` when the row names overrides.
async fn reserve_contact(
    router: &MultiServiceRouter,
    profile_arn: &str,
    tracking: Option<Value>,
    slot: i64,
) -> Reply {
    let shift = slot * 86_400;
    let mut body = json!({
        "missionProfileArn": profile_arn,
        "groundStation": "Ohio-1",
        "startTime": CONTACT_START + shift,
        "endTime": CONTACT_END + shift,
    });
    if let Some(tracking) = tracking {
        body["trackingOverrides"] = tracking;
    }
    send(router, "POST", "/contact", Some(body)).await
}

/// `ReserveContact` as above, asserting it was accepted, and the `contactId` it minted.
async fn reserve_ok(
    router: &MultiServiceRouter,
    case: &str,
    profile_arn: &str,
    tracking: Option<Value>,
    slot: i64,
) -> String {
    let reserved = reserve_contact(router, profile_arn, tracking, slot).await;
    assert_eq!(
        reserved.status, 200,
        "case {case}: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// The store's own record of `contact_id`, cloned out from under the lock — how a suite
/// counts the versions a refused write must not have appended.
fn stored_contact(engine: &engine::InMemoryEngine, contact_id: &str) -> Contact {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get(contact_id)
        .expect("the reserved contact is in the store")
        .clone()
}

/// How many contacts the store holds.
fn stored_contacts(engine: &engine::InMemoryEngine) -> usize {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .len()
}

/// The `trackingOverrides` `DescribeContact` reports for `contact_id`.
async fn described_overrides(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
) -> Option<Value> {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described.body.get("trackingOverrides").cloned()
}

/// An ephemeris of the type `ephemeris_type`, created through `CreateEphemeris`.
///
/// The data form fixes the type (`state::ephemeris_type_of`), so the table below names the
/// type it wants and this picks the member that yields it.
async fn ephemeris_of_type(router: &MultiServiceRouter, ephemeris_type: &str) -> String {
    let member = program_track_members()
        .into_iter()
        .find(|(_, required)| *required == ephemeris_type)
        .map(|(member, _)| member)
        .unwrap_or_else(|| panic!("case {ephemeris_type}: no program track names this type"));
    program_track_ephemeris(router, member).await
}

#[tokio::test]
async fn an_update_naming_an_ephemeris_this_account_does_not_hold_is_refused() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "absent program track", 180, None, None).await;
    let contact_id = reserve_ok(&router, "absent program track", &profile_arn, None, 0).await;

    // Every member of the union crossed with every id of the catalogue: the refusal is
    // about the id resolving to nothing, so it cannot depend on which member names it.
    for (member, _) in program_track_members() {
        for absent in ABSENT_EPHEMERIS_IDS {
            let case = format!("{member} naming {absent}");
            let refused = update_contact(
                &router,
                &contact_id,
                json!({"trackingOverrides": overrides(member, absent)}),
            )
            .await;
            assert_eq!(
                refused.status, 434,
                "case {case}: expected the modelled ResourceNotFoundException status 434, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ResourceNotFoundException",
                "case {case}: wrong error shape, got {}",
                refused.body
            );
            assert_eq!(
                refused.string("message"),
                state::EPHEMERIS_NOT_FOUND,
                "case {case}: wrong not-found text, got {}",
                refused.body
            );
            let contact = stored_contact(&engine, &contact_id);
            assert_eq!(
                contact.versions.len(),
                1,
                "case {case}: a refused update appends no version, got {} on record",
                contact.versions.len()
            );
        }
    }
}

#[tokio::test]
async fn a_program_track_must_name_an_ephemeris_of_the_form_its_member_requires() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "typed program track", 180, None, None).await;
    let contact_id = reserve_ok(&router, "typed program track", &profile_arn, None, 0).await;

    // One stored ephemeris per `EphemerisType` a customer request can create, so the table
    // below is the FULL cross product of the member naming the reference and the form of
    // the ephemeris it points at: three pairings agree and six do not.
    let mut stored: Vec<(&'static str, String)> = Vec::new();
    for (_, ephemeris_type) in program_track_members() {
        stored.push((ephemeris_type, ephemeris_of_type(&router, ephemeris_type).await));
    }

    let mut expected_versions = 1;
    for (member, required_type) in program_track_members() {
        for (stored_type, ephemeris_id) in &stored {
            let case = format!("{member} naming a {stored_type} ephemeris");
            let body = json!({"trackingOverrides": overrides(member, ephemeris_id)});
            let reply = update_contact(&router, &contact_id, body).await;

            if *stored_type == required_type {
                expected_versions += 1;
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a program track naming an ephemeris of its own form is \
                     accepted, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.field("versionId"),
                    &json!(expected_versions),
                    "case {case}: an accepted update appends version {expected_versions}, got {}",
                    reply.field("versionId")
                );
                // Past the settle span, so the appended version is the one in force, and
                // the stored reference is read back through DescribeContact.
                std::thread::sleep(AFTER_SETTLE);
                let reported = described_overrides(&router, &case, &contact_id).await;
                assert_eq!(
                    reported,
                    Some(overrides(member, ephemeris_id)),
                    "case {case}: the contact must be flown with the overrides it took, got \
                     {reported:?}"
                );
            } else {
                assert_eq!(
                    reply.status, 431,
                    "case {case}: expected the modelled InvalidParameterException status 431, \
                     got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    "InvalidParameterException",
                    "case {case}: wrong error shape, got {}",
                    reply.body
                );
                assert_eq!(
                    reply.string("message"),
                    state::wrong_ephemeris_type_message(required_type),
                    "case {case}: the refusal must state the type the member takes, got {}",
                    reply.body
                );
                assert_eq!(
                    reply.field("parameterName"),
                    &json!(format!(
                        "trackingOverrides.programTrackSettings.{member}.ephemerisId"
                    )),
                    "case {case}: the refusal must name the member at fault, got {}",
                    reply.body
                );
            }

            let contact = stored_contact(&engine, &contact_id);
            assert_eq!(
                contact.versions.len(),
                expected_versions as usize,
                "case {case}: expected {expected_versions} versions on record, got {}",
                contact.versions.len()
            );
        }
    }
}

#[tokio::test]
async fn a_reservation_resolves_the_program_track_its_first_version_carries() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "reserved program track", 180, None, None).await;

    // A reservation takes the same overrides for a contact's FIRST version, so it is held
    // to the same reference rule: nothing is reserved for an id that resolves to nothing
    // or to an ephemeris of another form. One antenna window per row, so a row's
    // reservation is never refused for the window the row before it holds.
    for (row, (member, required_type)) in program_track_members().into_iter().enumerate() {
        let slot = i64::try_from(row).expect("a row index fits in i64");
        let other_type = program_track_members()
            .into_iter()
            .map(|(_, ephemeris_type)| ephemeris_type)
            .find(|ephemeris_type| *ephemeris_type != required_type)
            .expect("the union has more than one member");
        let wrong_form = ephemeris_of_type(&router, other_type).await;
        let right_form = ephemeris_of_type(&router, required_type).await;
        let before = stored_contacts(&engine);

        for absent in ABSENT_EPHEMERIS_IDS {
            let case = format!("reserving with {member} naming {absent}");
            let refused = reserve_contact(
                &router,
                &profile_arn,
                Some(overrides(member, absent)),
                slot,
            )
            .await;
            assert_eq!(
                refused.status, 434,
                "case {case}: expected the modelled ResourceNotFoundException status 434, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.string("message"),
                state::EPHEMERIS_NOT_FOUND,
                "case {case}: wrong not-found text, got {}",
                refused.body
            );
        }

        let case = format!("reserving with {member} naming a {other_type} ephemeris");
        let refused = reserve_contact(
            &router,
            &profile_arn,
            Some(overrides(member, &wrong_form)),
            slot,
        )
        .await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected the modelled InvalidParameterException status 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.string("message"),
            state::wrong_ephemeris_type_message(required_type),
            "case {case}: the refusal must state the type the member takes, got {}",
            refused.body
        );
        assert_eq!(
            stored_contacts(&engine),
            before,
            "case {case}: a refused reservation stores no contact, got {} on record",
            stored_contacts(&engine)
        );

        let case = format!("reserving with {member} naming a {required_type} ephemeris");
        let contact_id = reserve_ok(
            &router,
            &case,
            &profile_arn,
            Some(overrides(member, &right_form)),
            slot,
        )
        .await;
        let reported = described_overrides(&router, &case, &contact_id).await;
        assert_eq!(
            reported,
            Some(overrides(member, &right_form)),
            "case {case}: the contact must be flown with the overrides it was reserved with, \
             got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_ephemeris_a_standing_contact_program_tracks_is_in_use_until_it_is_not() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "program track in use", 180, None, None).await;

    // An ephemeris no satellite of this account is on-boarded for, so what holds it is the
    // contact's program track and nothing else: this suite onboards no satellite, and the
    // create names none.
    let held = create_ok(
        &router,
        "held by a program track",
        json!({
            "name": "held by a program track",
            "enabled": true,
            "expirationTime": FUTURE,
            "ephemeris": tle_inline(&[(1_800_000_000, 1_800_086_400)]),
        }),
    )
    .await;
    let contact_id = reserve_ok(
        &router,
        "program track in use",
        &profile_arn,
        Some(overrides("tle", &held)),
        0,
    )
    .await;

    let refused = send(&router, "DELETE", &format!("/ephemeris/{held}"), None).await;
    assert_eq!(
        refused.status, 409,
        "case in use: expected the modelled ResourceInUseException status 409, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        state::ephemeris_in_use_message(std::slice::from_ref(&contact_id)),
        "case in use: the refusal must name the contact that holds the ephemeris, got {}",
        refused.body
    );

    // Released: the contact is terminal, so nothing of it is still to be flown and the
    // ephemeris it named is no longer held.
    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case released: expected CancelContact 200, got {} {}",
        cancelled.status, cancelled.body
    );
    std::thread::sleep(AFTER_SETTLE);
    let deleted = send(&router, "DELETE", &format!("/ephemeris/{held}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case released: a contact that will never be flown holds nothing, so the delete is \
         answered, got {} {}",
        deleted.status, deleted.body
    );

    // The resolution is against the store as it stands: the id of a DELETED ephemeris
    // names nothing, exactly as one that was never minted does.
    let other = reserve_ok(&router, "after the delete", &profile_arn, None, 1).await;
    let refused = update_contact(
        &router,
        &other,
        json!({"trackingOverrides": overrides("tle", &held)}),
    )
    .await;
    assert_eq!(
        refused.status, 434,
        "case after the delete: expected the modelled ResourceNotFoundException status 434, \
         got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        state::EPHEMERIS_NOT_FOUND,
        "case after the delete: wrong not-found text, got {}",
        refused.body
    );
    let contact = stored_contact(&engine, &other);
    assert_eq!(
        contact.versions.len(),
        1,
        "case after the delete: a refused update appends no version, got {} on record",
        contact.versions.len()
    );
}

#[tokio::test]
async fn overrides_that_name_no_program_track_resolve_nothing() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "no program track", 180, None, None).await;

    // `programTrackSettings` is optional on `TrackingOverrides` (`smithy_query(query="shape",
    // shape="TrackingOverrides")`), so overrides that name none reference no ephemeris and
    // have nothing to resolve — on either writer of a contact version.
    let contact_id = reserve_ok(
        &router,
        "reserved with empty overrides",
        &profile_arn,
        Some(json!({})),
        0,
    )
    .await;
    let reported = described_overrides(&router, "reserved with empty overrides", &contact_id).await;
    assert_eq!(
        reported,
        Some(json!({})),
        "case reserved with empty overrides: the contact keeps the overrides it was reserved \
         with, got {reported:?}"
    );

    let updated = update_contact(&router, &contact_id, json!({"trackingOverrides": {}})).await;
    assert_eq!(
        updated.status, 200,
        "case updated with empty overrides: expected 200, got {} {}",
        updated.status, updated.body
    );
}

#[tokio::test]
async fn a_program_track_resolves_an_ephemeris_whatever_status_it_is_in() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "validating track", 180, None, None).await;
    let contact_id = reserve_ok(&router, "validating track", &profile_arn, None, 0).await;

    // What the reference resolves is EXISTENCE and form, not the verdict of the validation
    // an ephemeris is admitted into: a program track naming one still `VALIDATING`, one
    // that validated `DISABLED` (the create asks for neither) and one already `EXPIRED` is
    // accepted. The rows are read straight after the create (still validating) and again
    // after the validation falls due.
    for (case, data) in [
        ("oem", json!({"oem": {"oemData": OEM_BODY}})),
        ("azEl", az_el_inline(&[(1_800_000_000, 1_800_086_400)])),
        ("tle", tle_inline(&[(1_800_000_000, 1_800_086_400)])),
    ] {
        let ephemeris_id = create_ok(
            &router,
            case,
            json!({
                "name": format!("{case} still validating"),
                "expirationTime": FUTURE,
                "ephemeris": data,
            }),
        )
        .await;
        for stage in ["still validating", "once the validation has fallen due"] {
            let accepted = update_contact(
                &router,
                &contact_id,
                json!({"trackingOverrides": overrides(case, &ephemeris_id)}),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {case} {stage}: a program track naming an ephemeris of its own form is \
                 accepted, got {} {}",
                accepted.status, accepted.body
            );
            wait_out_validation();
        }
    }
}

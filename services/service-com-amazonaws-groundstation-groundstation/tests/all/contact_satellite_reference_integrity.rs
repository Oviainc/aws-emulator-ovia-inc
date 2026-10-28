// SPDX-License-Identifier: CC0-1.0

//! The satellite reference a contact version carries, end to end over HTTP.
//!
//! The RULE under test: a contact version's `satelliteArn` references a satellite THIS
//! account is on-boarded to, and both writers of a contact version resolve that reference
//! before storing it — an ARN no onboarding of this account carries is refused as a missing
//! resource (`ResourceNotFoundException`, HTTP 434) and stores nothing, while an ARN an
//! onboarding does carry is stored and read back. A reservation whose satellite resolves is
//! one the rules keyed on that satellite can see: the ephemeris in force for it is in use
//! while the contact stands, so it cannot be deleted out from under the pass.
//!
//! The suites assert that rule over SETS of inputs rather than one call:
//!
//! * a catalogue of well-formed `satelliteArn`s that name no onboarding — differing in the
//!   `Uuid` they name, in whether they carry a region segment and in the account they name
//!   — crossed with both writers of a contact version;
//! * every satellite of a catalogue of onboardings, reserved against and then updated to,
//!   with each read back through `DescribeContact` and `DescribeContactVersion`;
//! * a request naming no satellite at all, which references nothing and resolves nothing,
//!   on either writer;
//! * the in-use consequence of a reference that resolves, asserted through
//!   `DeleteEphemeris`.
//!
//! Every request goes through the real front door — routing, deserialization, constraint
//! validation, the handler and serialization — on the harness `scheduling_loop.rs` stands
//! up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{self, Contact};
use smithy_server_runtime::MultiServiceRouter;

use crate::ephemeris_catalog::{az_el_inline, create_ok, onboard, wait_out_validation, FUTURE};
use crate::ephemeris_deletion::satellite_arn;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE, REGION,
};

/// The contact window a reservation in slot 0 books; a later slot books the same window a
/// day per slot later, so each reservation of a suite holds an antenna window of its own.
const CONTACT_START: i64 = 1_950_000_000;
const CONTACT_END: i64 = CONTACT_START + 600;

/// The ground station every reservation here is placed at.
const GROUND_STATION: &str = "Ohio-1";

/// Onboard `count` satellites to this account and answer the `satelliteArn` each onboarding
/// minted, in NORAD catalogue order — so a suite that needs several distinct satellites can
/// index them and read the same one on every run.
///
/// `pub(crate)`: a contact is flown against a satellite of THIS account, so every sibling
/// suite that reserves or updates one against a `satelliteArn` onboards it through here
/// rather than spelling an ARN out by hand. Onboarding happens outside this API, so the
/// catalogue is written straight into the store, exactly where `state::SATELLITES_ENV`
/// lands it (`crate::ephemeris_catalog::onboard`), and the ARN read back is the one that
/// onboarding minted (`crate::ephemeris_deletion::satellite_arn`).
pub(crate) fn onboard_satellite_arns(engine: &engine::InMemoryEngine, count: usize) -> Vec<String> {
    let setting = (0..count)
        .map(|index| format!("{}=Ohio-1+Ohio 1+Dubbo-1", 25_000 + index))
        .collect::<Vec<String>>()
        .join(",");
    let onboarded = onboard(engine, &setting);
    assert_eq!(
        onboarded.len(),
        count,
        "case onboard-{count}: the setting {setting:?} must onboard {count} satellites, got \
         {onboarded:?}"
    );
    onboarded
        .iter()
        .map(|satellite_id| satellite_arn(engine, satellite_id))
        .collect()
}

/// Well-formed `satelliteArn`s naming no satellite this account is on-boarded to, each with
/// the `Uuid` it names beside it.
///
/// The rows differ along everything the modelled pattern leaves free
/// (`^arn:aws:groundstation:([-a-z0-9]{1,50})?:[0-9]{12}:satellite/[a-f0-9]{8}-[a-f0-9]{4}-\
/// [a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, length 82..132 —
/// `smithy_query(query="operation", operation="ReserveContact")`): the id, the optional
/// region segment (absent, which is the form an onboarding of this service mints, and
/// present), and the account the ARN is scoped to. `onboarded_id` is the id of a satellite
/// this account IS on-boarded to, which the last two rows re-address under an account and a
/// region no onboarding carries, so a refusal cannot be passing merely because the id is
/// unknown.
fn absent_satellite_arns(onboarded_id: &str) -> Vec<(String, String, String)> {
    let account: String = engine::emulator_account_id().to_string();
    let rows = vec![
        (
            "a run of zeros, no region segment".to_string(),
            String::new(),
            account.clone(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a run of fs, in the signing region".to_string(),
            REGION.to_string(),
            account.clone(),
            "ffffffff-ffff-4fff-bfff-ffffffffffff".to_string(),
        ),
        (
            "a mixed id, in another region".to_string(),
            "ap-southeast-2".to_string(),
            account.clone(),
            "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d".to_string(),
        ),
        (
            "an on-boarded id under another account".to_string(),
            String::new(),
            "210987654321".to_string(),
            onboarded_id.to_string(),
        ),
        (
            "an on-boarded id under a region no onboarding carries".to_string(),
            REGION.to_string(),
            account,
            onboarded_id.to_string(),
        ),
    ];
    rows.into_iter()
        .map(|(case, region, account, satellite_id)| {
            let arn = format!("arn:aws:groundstation:{region}:{account}:satellite/{satellite_id}");
            assert!(
                (82..=132).contains(&arn.len()),
                "case {case}: a satelliteArn is 82..132 characters, got {} in {arn:?}",
                arn.len()
            );
            (case, arn, satellite_id)
        })
        .collect()
}

/// `ReserveContact` for the window a day per `slot` after [`CONTACT_START`], carrying
/// `satellite` when the row names one.
async fn reserve_contact(
    router: &MultiServiceRouter,
    profile_arn: &str,
    satellite: Option<&str>,
    slot: i64,
) -> Reply {
    let shift = slot * 86_400;
    let mut body = json!({
        "missionProfileArn": profile_arn,
        "groundStation": GROUND_STATION,
        "startTime": CONTACT_START + shift,
        "endTime": CONTACT_END + shift,
    });
    if let Some(arn) = satellite {
        body["satelliteArn"] = json!(arn);
    }
    send(router, "POST", "/contact", Some(body)).await
}

/// `ReserveContact` as above, asserting it was accepted, and the `contactId` it minted.
async fn reserve_ok(
    router: &MultiServiceRouter,
    case: &str,
    profile_arn: &str,
    satellite: Option<&str>,
    slot: i64,
) -> String {
    let reserved = reserve_contact(router, profile_arn, satellite, slot).await;
    assert_eq!(
        reserved.status, 200,
        "case {case}: expected ReserveContact 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
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

/// The `satelliteArn` `DescribeContact` reports for `contact_id`.
async fn described_satellite(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
) -> Option<String> {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described
        .body
        .get("satelliteArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `satelliteArn` `DescribeContactVersion` reports for one version of `contact_id`.
async fn described_version_satellite(
    router: &MultiServiceRouter,
    case: &str,
    contact_id: &str,
    version_id: i64,
) -> Option<String> {
    let described = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContactVersion 200, got {} {}",
        described.status, described.body
    );
    described
        .body
        .get("satelliteArn")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert `reply` is the modelled refusal of a satellite reference that resolves to
/// nothing: `ResourceNotFoundException` at HTTP 434, over the text this service answers for
/// a satellite it is not on-boarded to, naming the id the ARN carries.
fn assert_refused(reply: &Reply, case: &str, satellite_id: &str) {
    assert_eq!(
        reply.status, 434,
        "case {case}: expected the modelled ResourceNotFoundException status 434, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "ResourceNotFoundException",
        "case {case}: wrong error shape, got {}",
        reply.body
    );
    assert_eq!(
        reply.string("message"),
        state::satellite_not_found_message(satellite_id),
        "case {case}: the refusal must name the satellite the lookup looked for, got {}",
        reply.body
    );
}

#[tokio::test]
async fn both_writers_refuse_a_satellite_this_account_is_not_onboarded_to() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, "25994=Ohio-1");
    let flown = satellite_arn(&engine, &onboarded[0]);
    let profile_arn =
        create_mission_profile(&router, "absent satellite", 180, None, None).await;
    let contact_id = reserve_ok(&router, "absent satellite", &profile_arn, Some(&flown), 0).await;
    let held = stored_contacts(&engine);

    // Every row of the catalogue crossed with both writers of a contact version: the
    // refusal is about the ARN resolving to no onboarding of this account, so it cannot
    // depend on which writer carries it.
    for (row, arn, satellite_id) in absent_satellite_arns(&onboarded[0]) {
        let case = format!("reserving against {row}");
        let refused = reserve_contact(&router, &profile_arn, Some(&arn), 1).await;
        assert_refused(&refused, &case, &satellite_id);
        assert_eq!(
            stored_contacts(&engine),
            held,
            "case {case}: a refused reservation stores no contact, got {} on record",
            stored_contacts(&engine)
        );

        let case = format!("updating to {row}");
        let refused = update_contact(&router, &contact_id, json!({"satelliteArn": arn})).await;
        assert_refused(&refused, &case, &satellite_id);
        let contact = stored_contact(&engine, &contact_id);
        assert_eq!(
            contact.versions.len(),
            1,
            "case {case}: a refused update appends no version, got {} on record",
            contact.versions.len()
        );
        assert_eq!(
            contact.satellite_arn(),
            Some(flown.as_str()),
            "case {case}: a refused update leaves the satellite in force alone, got {:?}",
            contact.satellite_arn()
        );
    }
}

#[tokio::test]
async fn both_writers_accept_every_onboarded_satellite_and_read_it_back() {
    let (router, engine) = harness_with_engine();
    // Several onboardings, so each row of the tables below names a DIFFERENT satellite of
    // this account rather than the same one twice.
    let satellites = onboard_satellite_arns(&engine, 3);
    let profile_arn =
        create_mission_profile(&router, "onboarded satellites", 180, None, None).await;

    // Reserved against: one contact per onboarding, each in a window of its own, with the
    // stored reference read back through the contact and through its first version.
    for (row, satellite) in satellites.iter().enumerate() {
        let slot = i64::try_from(row).expect("a row index fits in i64");
        let case = format!("reserved against satellite {row}");
        let contact_id = reserve_ok(&router, &case, &profile_arn, Some(satellite), slot).await;
        let reported = described_satellite(&router, &case, &contact_id).await;
        assert_eq!(
            reported.as_deref(),
            Some(satellite.as_str()),
            "case {case}: the contact must be flown against the satellite it was reserved \
             with, got {reported:?}"
        );
        let reported =
            described_version_satellite(&router, &case, &contact_id, i64::from(state::FIRST_VERSION_ID))
                .await;
        assert_eq!(
            reported.as_deref(),
            Some(satellite.as_str()),
            "case {case}: the first version must carry the satellite it was reserved with, got \
             {reported:?}"
        );
    }

    // Updated to: one contact taking each onboarding in turn, so every row both appends a
    // version and supersedes the one before it.
    let case = "updated through every onboarding";
    let contact_id = reserve_ok(&router, case, &profile_arn, None, 8).await;
    assert_eq!(
        described_satellite(&router, case, &contact_id).await,
        None,
        "case {case}: a reservation naming no satellite references none"
    );
    for (row, satellite) in satellites.iter().enumerate() {
        let case = format!("updated to satellite {row}");
        let expected_version = i64::try_from(row).expect("a row index fits in i64") + 2;
        let updated = update_contact(&router, &contact_id, json!({"satelliteArn": satellite})).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateContact 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.field("versionId"),
            &json!(expected_version),
            "case {case}: an accepted update appends version {expected_version}, got {}",
            updated.field("versionId")
        );

        // Past the settle span, so the appended version is the one in force and the stored
        // reference is what every read of the contact reports.
        std::thread::sleep(AFTER_SETTLE);
        let reported = described_satellite(&router, &case, &contact_id).await;
        assert_eq!(
            reported.as_deref(),
            Some(satellite.as_str()),
            "case {case}: the contact must be flown against the satellite its active version \
             names, got {reported:?}"
        );
        let reported =
            described_version_satellite(&router, &case, &contact_id, expected_version).await;
        assert_eq!(
            reported.as_deref(),
            Some(satellite.as_str()),
            "case {case}: the appended version must carry the satellite it named, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_write_naming_no_satellite_resolves_nothing() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 1);
    let profile_arn = create_mission_profile(&router, "no satellite", 180, None, None).await;

    // `satelliteArn` is optional on both writers (`smithy_query(query="operation",
    // operation="ReserveContact,UpdateContact")`), so a write that names none references no
    // satellite and has nothing to resolve — on an account on-boarded to one and, below, on
    // one on-boarded to none.
    let contact_id = reserve_ok(&router, "reserved naming no satellite", &profile_arn, None, 0).await;
    let updated = update_contact(&router, &contact_id, json!({"trackingOverrides": {}})).await;
    assert_eq!(
        updated.status, 200,
        "case updated naming no satellite: expected UpdateContact 200, got {} {}",
        updated.status, updated.body
    );
    assert_eq!(
        described_satellite(&router, "updated naming no satellite", &contact_id).await,
        None,
        "case updated naming no satellite: a contact no version named a satellite for \
         references none"
    );

    // The satellite a later version DOES name is resolved, and the onboarding is what makes
    // it resolve: the same ARN is refused once this account is on-boarded to nothing.
    let satellite = satellites[0].clone();
    let accepted = update_contact(
        &router,
        &contact_id,
        json!({"satelliteArn": satellite.clone()}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case updated to the onboarding: expected UpdateContact 200, got {} {}",
        accepted.status, accepted.body
    );
    state::service_state(&*engine)
        .expect("this service's state store")
        .lock()
        .satellites
        .clear();
    let refused = update_contact(&router, &contact_id, json!({"satelliteArn": satellite})).await;
    assert_eq!(
        refused.status, 434,
        "case the onboarding withdrawn: an ARN no onboarding carries references nothing, got \
         {} {}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn an_ephemeris_a_standing_contact_flies_is_in_use_because_the_satellite_resolves() {
    let (router, engine) = harness_with_engine();
    let onboarded = onboard(&engine, "27424=Ohio-1");
    let satellite_id = onboarded[0].clone();
    let flown = satellite_arn(&engine, &satellite_id);
    let profile_arn = create_mission_profile(&router, "satellite in use", 180, None, None).await;

    // The ephemeris in force for the satellite the contact is flown against: enabled, so it
    // is the one that points the antenna once its validation has fallen due.
    let ephemeris_id = create_ok(
        &router,
        "in force for the flown satellite",
        json!({
            "name": "in force for the flown satellite",
            "enabled": true,
            "satelliteId": satellite_id,
            "expirationTime": FUTURE,
            "ephemeris": az_el_inline(&[(CONTACT_START - 86_400, CONTACT_END + 86_400)]),
        }),
    )
    .await;
    wait_out_validation();

    let contact_id = reserve_ok(&router, "satellite in use", &profile_arn, Some(&flown), 0).await;
    let refused = send(
        &router,
        "DELETE",
        &format!("/ephemeris/{ephemeris_id}"),
        None,
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case in use: expected the modelled ResourceInUseException status 409, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        state::ephemeris_in_use_message(std::slice::from_ref(&contact_id)),
        "case in use: the refusal must name the contact flown against the satellite, got {}",
        refused.body
    );

    // Released: the contact is terminal, so nothing of it is still to be flown and the
    // ephemeris its satellite's trajectory came from is no longer held.
    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case released: expected CancelContact 200, got {} {}",
        cancelled.status, cancelled.body
    );
    std::thread::sleep(AFTER_SETTLE);
    let deleted = send(
        &router,
        "DELETE",
        &format!("/ephemeris/{ephemeris_id}"),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "case released: a contact that will never be flown holds nothing, so the delete is \
         answered, got {} {}",
        deleted.status, deleted.body
    );
}

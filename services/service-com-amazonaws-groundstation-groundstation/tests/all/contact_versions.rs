// SPDX-License-Identifier: CC0-1.0

//! `UpdateContact`: the versions a contact takes, end to end over HTTP.
//!
//! The RULE under test: an accepted `UpdateContact` APPENDS the next version of the
//! contact — `versionId` one past the newest — carrying the members the request named
//! over the ones the newest version already held; the service applies that version a
//! short span later, at which point it is `ACTIVE`, every version before it is
//! `SUPERSEDED`, and it is the configuration every read of the contact reports. A
//! request that names no change, addresses no contact, addresses one whose pass can no
//! longer be flown, or would need a `versionId` past the modelled maximum is refused
//! with a modelled error and appends nothing.
//!
//! The rows of each table differ along what that rule generalises over: which members an
//! update names, which `ProgramTrackSettings` member its overrides select, how many
//! updates have been chained onto one contact, and the form of the `clientToken`. Every
//! request goes through the real front door — routing, deserialization, constraint
//! validation, the handler and serialization — using the harness `scheduling_loop.rs`
//! already stands up.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::{self, Contact};
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE, REGION,
};

/// The contact window every reservation in this suite books.
const CONTACT_START: i64 = 1_900_000_000;
const CONTACT_END: i64 = CONTACT_START + 600;

/// Well-formed `Uuid`s (model pattern
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`) that name nothing: a
/// run of zeros, a run of `f`s and a mixed one, so an assertion about a refusal cannot be
/// passing on one id's shape.
const ABSENT_CONTACT_IDS: &[&str] = &[
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-4fff-bfff-ffffffffffff",
    "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
];

/// A well-formed `satelliteArn` for the satellite `uuid`: the model's pattern
/// `^arn:aws:groundstation:([-a-z0-9]{1,50})?:[0-9]{12}:satellite/<uuid>$`, length
/// 82..132 (`smithy_query(query="operation", operation="UpdateContact")`).
fn satellite_arn(uuid: &str) -> String {
    format!(
        "arn:aws:groundstation:{REGION}:{}:satellite/{uuid}",
        engine::EMULATOR_ACCOUNT_ID
    )
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

/// `ReserveContact`, asserting it was accepted at version 1, and answering the contact id.
async fn reserve_contact(
    router: &MultiServiceRouter,
    mission_profile_arn: &str,
    satellite_arn: Option<&str>,
) -> String {
    let mut body = json!({
        "missionProfileArn": mission_profile_arn,
        "groundStation": "Ohio-1",
        "startTime": CONTACT_START,
        "endTime": CONTACT_END,
    });
    if let Some(arn) = satellite_arn {
        body["satelliteArn"] = json!(arn);
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

/// The store's own record of `contact_id`, cloned out from under the lock.
///
/// Read directly rather than over HTTP where the point is to observe a state the service
/// owes a move out of: a store read settles nothing, so it shows the admitted state
/// without racing that span.
fn stored_contact(engine: &engine::InMemoryEngine, contact_id: &str) -> Contact {
    state::service_state(engine)
        .expect("this service's state store")
        .lock()
        .contacts
        .get(contact_id)
        .expect("the reserved contact is in the store")
        .clone()
}

/// `DescribeContact`, asserting it answered, and handing back the whole answer.
async fn describe_contact(router: &MultiServiceRouter, case: &str, contact_id: &str) -> Reply {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: expected DescribeContact 200, got {} {}",
        described.status, described.body
    );
    described
}

#[tokio::test]
async fn each_accepted_update_appends_the_next_version_and_applies_what_it_named() {
    let (router, engine) = harness_with_engine();
    let profile_arn =
        create_mission_profile(&router, "contact-versions", 180, Some(60), Some(30)).await;

    let first_satellite = satellite_arn("11111111-1111-4111-8111-111111111111");
    let second_satellite = satellite_arn("22222222-2222-4222-8222-222222222222");
    let third_satellite = satellite_arn("33333333-3333-4333-8333-333333333333");
    let contact_id = reserve_contact(&router, &profile_arn, Some(&first_satellite)).await;

    // One row per form an update takes, chained onto ONE contact so each row also tests
    // what the row before it left behind: which members the request names, and which
    // member of `ProgramTrackSettings` the overrides select (`azEl`, `oem` and `tle` —
    // `smithy_query(query="shape", shape="ProgramTrackSettings")`, all three of them).
    // The last two columns are the configuration the contact must be flown with once the
    // version has been applied.
    let rows: Vec<(&str, Value, Option<String>, Option<Value>)> = vec![
        (
            "satelliteArn alone",
            json!({"satelliteArn": second_satellite}),
            Some(second_satellite.clone()),
            None,
        ),
        (
            "trackingOverrides alone, azEl program track",
            json!({"trackingOverrides": {"programTrackSettings": {
                "azEl": {"ephemerisId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"},
            }}}),
            Some(second_satellite.clone()),
            Some(json!({"programTrackSettings": {
                "azEl": {"ephemerisId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"},
            }})),
        ),
        (
            "both members, oem program track",
            json!({
                "satelliteArn": third_satellite,
                "trackingOverrides": {"programTrackSettings": {
                    "oem": {"ephemerisId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"},
                }},
            }),
            Some(third_satellite.clone()),
            Some(json!({"programTrackSettings": {
                "oem": {"ephemerisId": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"},
            }})),
        ),
        (
            "trackingOverrides alone, tle program track",
            json!({"trackingOverrides": {"programTrackSettings": {
                "tle": {"ephemerisId": "cccccccc-cccc-4ccc-8ccc-cccccccccccc"},
            }}}),
            Some(third_satellite.clone()),
            Some(json!({"programTrackSettings": {
                "tle": {"ephemerisId": "cccccccc-cccc-4ccc-8ccc-cccccccccccc"},
            }})),
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
        (
            "satelliteArn beside a clientToken",
            json!({"satelliteArn": second_satellite, "clientToken": "chained-update"}),
            Some(second_satellite.clone()),
            Some(json!({})),
        ),
    ];

    let mut expected_version = state::FIRST_VERSION_ID;
    let mut in_force = Some(first_satellite.clone());
    for (case, body, satellite, tracking) in rows {
        expected_version += 1;

        let updated = update_contact(&router, &contact_id, body).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("contactId"),
            contact_id,
            "case {case}: another contactId came back"
        );
        assert_eq!(
            updated.field("versionId"),
            &json!(expected_version),
            "case {case}: an update appends version {expected_version}, got {}",
            updated.field("versionId")
        );

        // The version is admitted UPDATING with its move to ACTIVE owed by the service,
        // and until that move the contact is still flown with the version in force.
        {
            let contact = stored_contact(&engine, &contact_id);
            let latest = contact
                .latest_version()
                .expect("a contact always has at least one version");
            assert_eq!(
                latest.status,
                state::UPDATING_VERSION_STATUS,
                "case {case}: an appended version is admitted UPDATING, got {:?}",
                latest.status
            );
            assert!(
                latest.activates_at.is_some(),
                "case {case}: the service must owe the appended version its move to ACTIVE, \
                 got {latest:?}"
            );
            assert_eq!(
                contact.satellite_arn().map(str::to_string),
                in_force,
                "case {case}: until the version is applied the contact keeps the configuration \
                 in force, got {:?}",
                contact.satellite_arn()
            );
        }

        std::thread::sleep(AFTER_SETTLE);

        let described = describe_contact(&router, case, &contact_id).await;
        let version = described.field("version").clone();
        assert_eq!(
            version.get("versionId").and_then(Value::as_i64),
            Some(i64::from(expected_version)),
            "case {case}: the contact reports its newest version, got {version}"
        );
        assert_eq!(
            version.get("status").and_then(Value::as_str),
            Some(state::ACTIVE_VERSION_STATUS),
            "case {case}: an applied version is ACTIVE, got {version}"
        );
        assert!(
            version.get("activated").is_some(),
            "case {case}: an applied version reports when it was activated, got {version}"
        );
        assert_eq!(
            described
                .body
                .get("satelliteArn")
                .and_then(Value::as_str)
                .map(str::to_string),
            satellite,
            "case {case}: expected satelliteArn {satellite:?}, got {}",
            described.field("satelliteArn")
        );
        assert_eq!(
            described.body.get("trackingOverrides").cloned(),
            tracking,
            "case {case}: expected trackingOverrides {tracking:?}, got {}",
            described.field("trackingOverrides")
        );

        // The history keeps every version, and applying the newest superseded them all.
        let contact = stored_contact(&engine, &contact_id);
        assert_eq!(
            contact.versions.len(),
            expected_version as usize,
            "case {case}: expected {expected_version} versions on record, got {}",
            contact.versions.len()
        );
        for older in &contact.versions[..contact.versions.len() - 1] {
            assert_eq!(
                older.status,
                state::SUPERSEDED_VERSION_STATUS,
                "case {case}: version {} must be SUPERSEDED, got {:?}",
                older.version_id,
                older.status
            );
            assert!(
                older.superseded.is_some(),
                "case {case}: version {} must report when it was superseded, got {older:?}",
                older.version_id
            );
        }

        in_force = satellite;
    }
}

#[tokio::test]
async fn a_contact_is_listed_under_the_satellite_its_active_version_names() {
    let (router, _engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "listed-by-version", 180, None, None).await;

    let reserved_satellite = satellite_arn("44444444-4444-4444-8444-444444444444");
    let updated_satellite = satellite_arn("55555555-5555-4555-8555-555555555555");
    let contact_id = reserve_contact(&router, &profile_arn, Some(&reserved_satellite)).await;

    let updated = update_contact(
        &router,
        &contact_id,
        json!({"satelliteArn": updated_satellite}),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case update: expected 200, got {} {}",
        updated.status, updated.body
    );
    std::thread::sleep(AFTER_SETTLE);

    for (case, satellite, expected) in [
        (
            "the satellite the active version names",
            &updated_satellite,
            vec![contact_id.clone()],
        ),
        (
            "the satellite the superseded version named",
            &reserved_satellite,
            Vec::new(),
        ),
    ] {
        let listed = send(
            &router,
            "POST",
            "/contacts",
            Some(json!({
                "statusList": ["SCHEDULING", "SCHEDULED"],
                "startTime": CONTACT_START - 3600,
                "endTime": CONTACT_END + 3600,
                "satelliteArn": satellite,
            })),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListContacts 200, got {} {}",
            listed.status, listed.body
        );
        let ids: Vec<String> = listed
            .field("contactList")
            .as_array()
            .map(|contacts| {
                contacts
                    .iter()
                    .filter_map(|contact| contact.get("contactId"))
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids, expected,
            "case {case}: expected {expected:?}, got {ids:?}"
        );
    }
}

#[tokio::test]
async fn an_update_the_contact_cannot_take_is_refused_and_appends_nothing() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "refused-updates", 180, None, None).await;
    let satellite = satellite_arn("66666666-6666-4666-8666-666666666666");
    let contact_id = reserve_contact(&router, &profile_arn, Some(&satellite)).await;

    // An id that names no contact: the MUTATION path's text, which names the id the
    // request addressed and the account the write was scoped to.
    for absent in ABSENT_CONTACT_IDS {
        let refused = update_contact(&router, absent, json!({"satelliteArn": satellite})).await;
        assert_eq!(
            refused.status, 434,
            "case absent {absent}: expected the modelled ResourceNotFoundException status 434, \
             got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case absent {absent}: wrong error shape, got {}",
            refused.body
        );
        assert_eq!(
            refused.string("message"),
            state::contact_mutation_not_found_message(absent),
            "case absent {absent}: wrong message, got {}",
            refused.body
        );
    }

    // A request that names nothing to change: the only configuration an update can carry
    // is `satelliteArn` and `trackingOverrides`.
    for (case, body) in [
        ("an empty body", json!({})),
        ("a clientToken alone", json!({"clientToken": "no-change"})),
    ] {
        let refused = update_contact(&router, &contact_id, body).await;
        assert_eq!(
            refused.status, 431,
            "case {case}: expected the modelled InvalidParameterException status 431, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterException",
            "case {case}: wrong error shape, got {}",
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

    // A contact whose pass will never be flown takes no further version.
    let cancelled = send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel: expected 200, got {} {}",
        cancelled.status, cancelled.body
    );
    let refused = update_contact(&router, &contact_id, json!({"satelliteArn": satellite})).await;
    assert_eq!(
        refused.status, 431,
        "case cancelled contact: expected the modelled InvalidParameterException status 431, \
         got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidParameterException",
        "case cancelled contact: wrong error shape, got {}",
        refused.body
    );
    let message = refused.string("message");
    assert!(
        message.contains("CANCELLED"),
        "case cancelled contact: the refusal must name the status that refuses it, got {message:?}"
    );
    let contact = stored_contact(&engine, &contact_id);
    assert_eq!(
        contact.versions.len(),
        1,
        "case cancelled contact: a refused update appends no version, got {} on record",
        contact.versions.len()
    );
}

#[tokio::test]
async fn a_replayed_client_token_appends_no_second_version() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "idempotent-updates", 180, None, None).await;
    let reserved_satellite = satellite_arn("77777777-7777-4777-8777-777777777777");
    let updated_satellite = satellite_arn("88888888-8888-4888-8888-888888888888");

    // One row per form the modelled `ClientToken` admits — length 1..64, pattern
    // `^[!-~]{1,64}$` (`smithy_query(query="shape", shape="ClientToken")`): the shortest
    // token, an everyday retry token, one made of printable punctuation and one of the
    // full 64 characters.
    let longest = "t".repeat(64);
    for token in ["a", "retry-token-2026", "!#$%&'()*+,-./:;<=>?@[]^_`{|}~", &longest] {
        let case = format!("clientToken of {} characters", token.len());
        let contact_id = reserve_contact(&router, &profile_arn, Some(&reserved_satellite)).await;
        let body = json!({"satelliteArn": updated_satellite, "clientToken": token});

        let first = update_contact(&router, &contact_id, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected 200, got {} {}",
            first.status, first.body
        );
        assert_eq!(
            first.field("versionId"),
            &json!(2),
            "case {case}: the first update appends version 2, got {}",
            first.field("versionId")
        );

        let replayed = update_contact(&router, &contact_id, body).await;
        assert_eq!(
            replayed.status, 200,
            "case {case}: a replay is accepted, got {} {}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.field("versionId"),
            first.field("versionId"),
            "case {case}: a replayed token answers with the version it appended, got {}",
            replayed.field("versionId")
        );
        let contact = stored_contact(&engine, &contact_id);
        assert_eq!(
            contact.versions.len(),
            2,
            "case {case}: a replayed token appends no second version, got {} on record",
            contact.versions.len()
        );

        // Another token is another request, so it appends the next version. Spelled by
        // replacing the first character, so it stays inside the modelled length.
        let other: String = std::iter::once('z').chain(token.chars().skip(1)).collect();
        let next = update_contact(
            &router,
            &contact_id,
            json!({"satelliteArn": reserved_satellite, "clientToken": other}),
        )
        .await;
        assert_eq!(
            next.status, 200,
            "case {case}: expected 200 for a fresh token, got {} {}",
            next.status, next.body
        );
        assert_eq!(
            next.field("versionId"),
            &json!(3),
            "case {case}: a fresh token appends version 3, got {}",
            next.field("versionId")
        );
        let contact = stored_contact(&engine, &contact_id);
        assert_eq!(
            contact.versions.len(),
            3,
            "case {case}: expected 3 versions on record, got {}",
            contact.versions.len()
        );
    }
}

#[tokio::test]
async fn a_contact_takes_no_more_versions_than_the_modelled_maximum() {
    let (router, engine) = harness_with_engine();
    let profile_arn = create_mission_profile(&router, "version-ceiling", 180, None, None).await;
    let even_satellite = satellite_arn("99999999-9999-4999-8999-999999999999");
    let odd_satellite = satellite_arn("aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
    let contact_id = reserve_contact(&router, &profile_arn, Some(&odd_satellite)).await;

    // Every version the model's `VersionId` admits past the first (range 1..128 —
    // `smithy_query(query="shape", shape="VersionId")`), up to and including the
    // published maximum.
    for version in (state::FIRST_VERSION_ID + 1)..=state::MAX_CONTACT_VERSIONS {
        let satellite = if version % 2 == 0 {
            &even_satellite
        } else {
            &odd_satellite
        };
        let updated =
            update_contact(&router, &contact_id, json!({"satelliteArn": satellite})).await;
        assert_eq!(
            updated.status, 200,
            "case version {version}: expected 200, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.field("versionId"),
            &json!(version),
            "case version {version}: an update appends the next version, got {}",
            updated.field("versionId")
        );
    }

    // One past the published maximum: there is no `versionId` left for the answer to
    // carry, so the update is refused with the modelled limit error.
    let refused =
        update_contact(&router, &contact_id, json!({"satelliteArn": even_satellite})).await;
    assert_eq!(
        refused.status, 429,
        "case one past the maximum: expected the modelled ResourceLimitExceededException status \
         429, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ResourceLimitExceededException",
        "case one past the maximum: wrong error shape, got {}",
        refused.body
    );

    let contact = stored_contact(&engine, &contact_id);
    assert_eq!(
        contact.versions.len(),
        state::MAX_CONTACT_VERSIONS as usize,
        "case one past the maximum: a refused update appends nothing, got {} versions on record",
        contact.versions.len()
    );
    assert_eq!(
        contact.latest_version_id(),
        state::MAX_CONTACT_VERSIONS,
        "case one past the maximum: the newest version stays the published maximum, got {}",
        contact.latest_version_id()
    );
}

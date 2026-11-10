// SPDX-License-Identifier: CC0-1.0

//! What `ContactVersion$lastUpdated` reports, for every version of a contact and at every
//! stage its history goes through.
//!
//! The RULE under test: a version record's `lastUpdated` is the instant THAT VERSION was
//! last changed — the instant it was superseded ("a version is superseded when a newer
//! version becomes active"), else the instant it was activated ("a version is activated
//! when it becomes the current active version of the contact"), else the instant it was
//! created, those being the only moves a version record takes
//! (`smithy_query(query="shape", shape="ContactVersion")`). So nothing that leaves a
//! version record untouched can move its `lastUpdated`: not a change to the CONTACT (the
//! scheduler confirming the reservation, a release being accepted or carried out) and not
//! a change to ANOTHER version (an update appending the next one, which the service then
//! applies).
//!
//! The suites assert that rule over a SET of inputs rather than one call, the rows
//! differing along what the rule generalises over:
//!
//! * the STANDING of the version read — created only (a version still being applied),
//!   activated (the version in force), activated then superseded, and created then
//!   superseded without ever being activated (a pending version a newer one overtook);
//! * the LENGTH of the history it sits in, from the reservation's single version up, and
//!   its POSITION in that history (oldest, in the middle, newest);
//! * which members the update that appended it named (`satelliteArn`, `trackingOverrides`,
//!   both — the configuration an update can carry, `smithy_query(query="operation",
//!   operation="UpdateContact")`);
//! * the CHANGE TO THE CONTACT that happens between two reads of one unchanged version
//!   (the `SCHEDULING` -> `SCHEDULED` confirmation, a cancel before the pass, and the stop
//!   of a pass under way together with its `CANCELLING` -> `CANCELLED` settle).
//!
//! Every request goes through the real front door — the router, deserialization,
//! constraint validation, the handler and serialization — on the harness
//! `scheduling_loop.rs` stands up, and all three readers of a version record
//! (`ListContactVersions`, `DescribeContactVersion`, `DescribeContact`) are cross-checked,
//! so the answer cannot differ per operation.

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::contact_cancellation::put_in_status;
use crate::contact_satellite_reference_integrity::onboard_satellite_arns;
use crate::scheduling_loop::{
    create_mission_profile, harness_with_engine, send, Reply, AFTER_SETTLE,
};

/// The window the first reservation of a suite books; later ones are shifted a day per
/// slot. Far enough ahead that no reservation is refused for having elapsed and that no
/// pass is carried along by the clock during the run.
const CONTACT_START: i64 = 1_955_000_000;

/// How long each reserved pass runs — past the mission profile's minimum viable duration.
const CONTACT_SECONDS: i64 = 600;

/// `ReserveContact` for the pass a day per `slot` after [`CONTACT_START`], so two
/// reservations of one suite hold windows of their own: an antenna serves one pass at a
/// time.
async fn reserve_contact(
    router: &MultiServiceRouter,
    profile_arn: &str,
    satellite: &str,
    slot: i64,
) -> String {
    let start = CONTACT_START + slot * 86_400;
    let reserved = send(
        router,
        "POST",
        "/contact",
        Some(json!({
            "missionProfileArn": profile_arn,
            "groundStation": "Ohio-1",
            "startTime": start,
            "endTime": start + CONTACT_SECONDS,
            "satelliteArn": satellite,
        })),
    )
    .await;
    assert_eq!(
        reserved.status, 200,
        "ReserveContact in slot {slot}: expected 200, got {} {}",
        reserved.status, reserved.body
    );
    reserved.string("contactId")
}

/// `UpdateContact` (`POST /contact/{contactId}/versions`), asserting it was accepted and
/// answering the `versionId` it appended.
async fn append_version(router: &MultiServiceRouter, contact_id: &str, body: &Value) -> i64 {
    let updated = send(
        router,
        "POST",
        &format!("/contact/{contact_id}/versions"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "UpdateContact {body}: expected 200, got {} {}",
        updated.status, updated.body
    );
    updated
        .field("versionId")
        .as_i64()
        .unwrap_or_else(|| panic!("UpdateContact {body}: must answer a versionId, got {}", updated.body))
}

/// `ListContactVersions`' whole history for one contact, oldest first.
async fn version_history(
    router: &MultiServiceRouter,
    contact_id: &str,
    case: &str,
) -> Vec<Value> {
    let listed = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case {case}: ListContactVersions expected 200, got {} {}",
        listed.status, listed.body
    );
    listed
        .field("contactVersionsList")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: contactVersionsList must be a list, got {}",
                listed.body
            )
        })
}

/// `DescribeContact`, asserting it was answered.
async fn describe_contact(router: &MultiServiceRouter, contact_id: &str, case: &str) -> Reply {
    let described = send(router, "GET", &format!("/contact/{contact_id}"), None).await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContact expected 200, got {} {}",
        described.status, described.body
    );
    described
}

/// `DescribeContactVersion`'s `version` record for one `versionId`.
async fn described_version(
    router: &MultiServiceRouter,
    contact_id: &str,
    version_id: i64,
    case: &str,
) -> Value {
    let described = send(
        router,
        "GET",
        &format!("/contact/{contact_id}/versions/{version_id}"),
        None,
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case {case}: DescribeContactVersion expected 200 for version {version_id}, got {} {}",
        described.status, described.body
    );
    described.field("version").clone()
}

/// One timestamp member of a version record, as a number, or `None` when the record does
/// not carry it.
fn stamp(version: &Value, member: &str) -> Option<f64> {
    version.get(member).and_then(Value::as_f64)
}

/// The member of a version record holding the instant that version was last changed by a
/// move of its OWN — superseded, else activated, else created — which is the rule this
/// suite is about.
fn own_last_change_member(version: &Value) -> &'static str {
    if stamp(version, "superseded").is_some() {
        "superseded"
    } else if stamp(version, "activated").is_some() {
        "activated"
    } else {
        "created"
    }
}

/// Assert the rule of every record of one history, and that the three readers of a version
/// record answer the same way.
async fn assert_every_version_reports_its_own_last_change(
    router: &MultiServiceRouter,
    contact_id: &str,
    case: &str,
) -> Vec<Value> {
    let history = version_history(router, contact_id, case).await;
    assert!(
        !history.is_empty(),
        "case {case}: a contact always has at least one version, got {history:?}"
    );
    for version in &history {
        let version_id = version
            .get("versionId")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: every record carries a versionId, got {version}"));
        let case = format!("{case}/version {version_id}");
        let created = stamp(version, "created").unwrap_or_else(|| {
            panic!("case {case}: every version record carries its created instant, got {version}")
        });
        let member = own_last_change_member(version);
        assert_eq!(
            version.get("lastUpdated"),
            version.get(member),
            "case {case}: a version's lastUpdated is its own last change (superseded, else \
             activated, else created), so it must read its {member}, got {version}"
        );
        // The instants a version record carries are ordered by the moves that write them.
        if let Some(activated) = stamp(version, "activated") {
            assert!(
                activated >= created,
                "case {case}: a version is activated no earlier than it was created, got \
                 {version}"
            );
        }
        if let Some(superseded) = stamp(version, "superseded") {
            assert!(
                superseded >= created
                    && stamp(version, "activated").is_none_or(|activated| superseded >= activated),
                "case {case}: a version is superseded no earlier than it was created and \
                 activated, got {version}"
            );
        }
        // Every reader of this record answers the same lastUpdated.
        let described = described_version(router, contact_id, version_id, &case).await;
        assert_eq!(
            described.get("lastUpdated"),
            version.get("lastUpdated"),
            "case {case}: DescribeContactVersion must report the lastUpdated the listing \
             does, got {described} and {version}"
        );
    }

    // And the newest record is the one the contact itself reports.
    let described = describe_contact(router, contact_id, case).await;
    assert_eq!(
        described.field("version").get("lastUpdated"),
        history[history.len() - 1].get("lastUpdated"),
        "case {case}: DescribeContact reports the newest version's lastUpdated, got {} and {}",
        described.field("version"),
        history[history.len() - 1]
    );
    history
}

/// The `lastUpdated` of each record of a history, keyed by `versionId`, as the comparable
/// value a later read is held to.
fn last_updated_by_version(history: &[Value]) -> Vec<(Option<&Value>, Option<&Value>)> {
    history
        .iter()
        .map(|version| (version.get("versionId"), version.get("lastUpdated")))
        .collect()
}

/// The three shapes an `UpdateContact` body takes: the configuration an update can carry is
/// `satelliteArn` and `trackingOverrides` (`smithy_query(query="operation",
/// operation="UpdateContact")`), so a request names one, the other, or both.
fn update_body(form: usize, satellite: &str) -> Value {
    match form % 3 {
        0 => json!({"satelliteArn": satellite}),
        1 => json!({"trackingOverrides": {}}),
        _ => json!({"satelliteArn": satellite, "trackingOverrides": {}}),
    }
}

#[tokio::test]
async fn appending_a_version_moves_no_earlier_versions_last_updated() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 4);
    let profile_arn = create_mission_profile(&router, "version stamps", 180, None, None).await;
    let contact_id = reserve_contact(&router, &profile_arn, &satellites[0], 0).await;
    std::thread::sleep(AFTER_SETTLE);

    // Rows of growing history: one chained update per row, each naming a different set of
    // members, so the rule is asserted at every history length from one up and at every
    // position in it.
    let mut before = assert_every_version_reports_its_own_last_change(
        &router,
        &contact_id,
        "the reservation's version alone",
    )
    .await;
    for form in 0..3_usize {
        let body = update_body(form, &satellites[1 + form]);
        let case = format!("after update {form} ({body})");
        let appended = append_version(&router, &contact_id, &body).await;

        // Read WITHIN the activation span, where the version before the appended one is
        // neither superseded (nothing has taken force over it) nor the newest: it changed
        // in no way, so its reported lastUpdated must not have moved either.
        let pending =
            assert_every_version_reports_its_own_last_change(&router, &contact_id, &case).await;
        let appended_record = pending
            .iter()
            .find(|version| version.get("versionId").and_then(Value::as_i64) == Some(appended))
            .unwrap_or_else(|| panic!("case {case}: the appended version is in the history, got {pending:?}"));
        for earlier in &before {
            let version_id = earlier.get("versionId").and_then(Value::as_i64);
            let now = pending
                .iter()
                .find(|version| version.get("versionId").and_then(Value::as_i64) == version_id)
                .unwrap_or_else(|| {
                    panic!("case {case}: version {version_id:?} is still in the history, got {pending:?}")
                });
            if stamp(now, "superseded").is_some() {
                // The span elapsed between the two calls and the appended version took
                // force: this version's own supersession is then its last change, which the
                // rule above has already asserted.
                continue;
            }
            assert_eq!(
                now.get("lastUpdated"),
                earlier.get("lastUpdated"),
                "case {case}: appending another version changed nothing about version \
                 {version_id:?}, so its lastUpdated must stand at {:?}, got {now}",
                earlier.get("lastUpdated")
            );
            assert_ne!(
                now.get("lastUpdated"),
                appended_record.get("created"),
                "case {case}: version {version_id:?} must not report the instant version \
                 {appended} was created, got {now}"
            );
        }

        // Past the span the appended version is in force and the ones before it are
        // superseded: each then reports its own new last change and nothing else.
        std::thread::sleep(AFTER_SETTLE);
        let settled = assert_every_version_reports_its_own_last_change(
            &router,
            &contact_id,
            &format!("{case}, settled"),
        )
        .await;
        for version in &settled {
            let version_id = version.get("versionId").and_then(Value::as_i64);
            if version_id == Some(appended) {
                assert_eq!(
                    version.get("lastUpdated"),
                    version.get("activated"),
                    "case {case}: the version in force was last changed when it took force, \
                     got {version}"
                );
            } else {
                assert_eq!(
                    version.get("lastUpdated"),
                    version.get("superseded"),
                    "case {case}: a superseded version was last changed when it was \
                     superseded, got {version}"
                );
            }
        }
        before = settled;
    }
}

#[tokio::test]
async fn a_pending_version_a_newer_one_overtook_reports_its_own_created() {
    let (router, engine) = harness_with_engine();
    let satellites = onboard_satellite_arns(&engine, 3);
    let profile_arn = create_mission_profile(&router, "overtaken versions", 180, None, None).await;
    let contact_id = reserve_contact(&router, &profile_arn, &satellites[0], 0).await;

    // Two updates chained back to back, so the second is appended while the first is still
    // being applied: version 2 is then a version the contact has moved past that no newer
    // version has superseded yet, and it was never activated.
    let overtaken = append_version(&router, &contact_id, &json!({"satelliteArn": satellites[1]})).await;
    let newest = append_version(&router, &contact_id, &json!({"satelliteArn": satellites[2]})).await;
    let pending =
        assert_every_version_reports_its_own_last_change(&router, &contact_id, "overtaken pending")
            .await;
    let record = pending
        .iter()
        .find(|version| version.get("versionId").and_then(Value::as_i64) == Some(overtaken))
        .unwrap_or_else(|| panic!("case overtaken pending: version {overtaken} is in the history, got {pending:?}"));
    if stamp(record, "superseded").is_none() {
        assert_eq!(
            record.get("lastUpdated"),
            record.get("created"),
            "case overtaken pending: a version that was never activated and has not been \
             superseded was last changed when it was created, got {record}"
        );
    }

    // Once the newest version takes force, the overtaken one is superseded without ever
    // having been activated, and that supersession is its last change.
    std::thread::sleep(AFTER_SETTLE);
    let settled =
        assert_every_version_reports_its_own_last_change(&router, &contact_id, "overtaken settled")
            .await;
    let record = settled
        .iter()
        .find(|version| version.get("versionId").and_then(Value::as_i64) == Some(overtaken))
        .unwrap_or_else(|| panic!("case overtaken settled: version {overtaken} is in the history, got {settled:?}"));
    assert!(
        record.get("activated").is_none_or(Value::is_null),
        "case overtaken settled: a version a newer one overtook was never activated, got \
         {record}"
    );
    assert_eq!(
        record.get("lastUpdated"),
        record.get("superseded"),
        "case overtaken settled: it was last changed when it was superseded, got {record}"
    );
    let in_force = settled
        .iter()
        .find(|version| version.get("versionId").and_then(Value::as_i64) == Some(newest))
        .unwrap_or_else(|| panic!("case overtaken settled: version {newest} is in the history, got {settled:?}"));
    assert_eq!(
        in_force.get("status").and_then(Value::as_str),
        Some(state::ACTIVE_VERSION_STATUS),
        "case overtaken settled: the newest version took force, got {in_force}"
    );
}

/// What changes the CONTACT between two reads of a history no version move falls due in.
#[derive(Debug, Clone, Copy)]
enum ContactChange {
    /// The scheduler confirming the reservation: `SCHEDULING` -> `SCHEDULED`, which the
    /// clock owes the contact and a read materialises.
    Confirmed,
    /// A cancel of a contact whose pass is still ahead: the reservation is released
    /// outright.
    Released,
    /// A cancel of a pass already under way, put into `from` first: the release is carried
    /// out as a stop, which settles on `CANCELLED` on its own.
    Stopped { from: &'static str },
}

#[tokio::test]
async fn a_change_to_the_contact_alone_moves_no_versions_last_updated() {
    // Rows: each kind of change a contact takes that touches none of its version records,
    // against a history of two versions (one superseded, one in force) so the rule is
    // asserted on a version in every standing a settled history holds.
    let rows: &[(ContactChange, &str)] = &[
        (ContactChange::Confirmed, "SCHEDULED"),
        (ContactChange::Released, "CANCELLED"),
        (
            ContactChange::Stopped {
                from: state::CONTACT_PASS,
            },
            "CANCELLED",
        ),
    ];

    for (slot, (change, expected_status)) in rows.iter().enumerate() {
        let case = format!("{change:?}");
        let (router, engine) = harness_with_engine();
        let satellites = onboard_satellite_arns(&engine, 2);
        let profile_arn = create_mission_profile(&router, "contact changes", 180, None, None).await;
        let contact_id = reserve_contact(
            &router,
            &profile_arn,
            &satellites[0],
            i64::try_from(slot).expect("a row index fits in i64"),
        )
        .await;

        // A second version for every row but the one about the confirmation, which has to
        // be read while the reservation is still being confirmed.
        if !matches!(change, ContactChange::Confirmed) {
            std::thread::sleep(AFTER_SETTLE);
            append_version(&router, &contact_id, &json!({"satelliteArn": satellites[1]})).await;
            std::thread::sleep(AFTER_SETTLE);
        }

        let before =
            assert_every_version_reports_its_own_last_change(&router, &contact_id, &case).await;
        let status_before = describe_contact(&router, &contact_id, &case)
            .await
            .string("contactStatus");

        match change {
            // Nothing to request: the confirmation is the clock's own, waited out below.
            ContactChange::Confirmed => {}
            ContactChange::Released => {
                let cancelled =
                    send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
                assert_eq!(
                    cancelled.status, 200,
                    "case {case}: CancelContact expected 200, got {} {}",
                    cancelled.status, cancelled.body
                );
            }
            ContactChange::Stopped { from } => {
                // No request drives a pass, so the status the cancel arrives in is written
                // into the store — as `contact_cancellation.rs` does, for the same reason.
                put_in_status(&engine, &contact_id, from);
                let cancelled =
                    send(&router, "DELETE", &format!("/contact/{contact_id}"), None).await;
                assert_eq!(
                    cancelled.status, 200,
                    "case {case}: CancelContact expected 200, got {} {}",
                    cancelled.status, cancelled.body
                );
            }
        }
        // Long enough that every move the change left owed to the contact has fallen due.
        std::thread::sleep(AFTER_SETTLE);

        let after =
            assert_every_version_reports_its_own_last_change(&router, &contact_id, &case).await;
        let status_after = describe_contact(&router, &contact_id, &case)
            .await
            .string("contactStatus");
        assert_eq!(
            &status_after, expected_status,
            "case {case}: the contact must have taken the change, got {status_after:?} from \
             {status_before:?}"
        );
        assert_eq!(
            last_updated_by_version(&after),
            last_updated_by_version(&before),
            "case {case}: a change to the contact touches no version record, so every \
             version's lastUpdated must stand where it stood, got {:?} after {:?}",
            last_updated_by_version(&after),
            last_updated_by_version(&before)
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Which version an `UpdateLandingZone` may move a landing zone to, end to end over HTTP.
//!
//! The rule under test: a landing zone is updated FORWARD only — an update deploys the
//! requested version over the version the landing zone already has, so the SAME version
//! (re-applying the configuration at that version) or a numerically newer one is a
//! configuration the service can reach from there, and a numerically older one is refused
//! with `ValidationException` (400), with nothing queued, deployed or logged.
//!
//! That is a different rule from the catalog membership `landing_zone_validation`
//! asserts: there the question is whether the requested version EXISTS, here whether the
//! landing zone can get to it from where it is. Both refusals are 400s, so the rows below
//! only ever ask for versions the catalog does offer
//! (`state::CATALOG_LANDING_ZONE_VERSIONS`), which keeps the two rules apart.
//!
//! Versions are ordered per component NUMERICALLY, the same order
//! `latestAvailableVersion` is derived with, so `3.10` is newer than `3.3`.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, version_downgrade_message, CATALOG_LANDING_ZONE_VERSIONS,
};

use crate::landing_zone_lifecycle::{
    call, emulator, emulator_with_engine, manifest_governing, past_the_settle_span, Answer,
};

use smithy_server_runtime::MultiServiceRouter;

/// Create the account's one landing zone at `version` and wait for its `CREATE` operation
/// to become due, so a later update is not refused for an operation in flight.
async fn create_settled_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    version: &str,
    manifest: &serde_json::Value,
) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": version, "manifest": manifest }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the create to return 200, got {} body {:?}",
        created.status, created.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// `GetLandingZone`'s `landingZone` member for `identifier`.
async fn get_landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let answer: Answer = call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected get-landingzone to return 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("landingZone").clone()
}

/// RULE, over EVERY ordered pair of versions the catalog offers: an update is refused
/// exactly when the version it asks for is numerically older than the landing zone's.
///
/// The pairs are generated from `state::CATALOG_LANDING_ZONE_VERSIONS`, so every entry —
/// not only the ones the issue cited — is covered on both sides of the comparison. The
/// extra rows reach what the catalog cannot: a two-digit minor (`3.10`, newer than `3.3`
/// under numeric order though earlier under string order) and a value the modelled
/// pattern admits but no order can place.
#[test]
fn an_older_version_is_refused_and_the_same_or_newer_is_not() {
    for (deployed_index, deployed) in CATALOG_LANDING_ZONE_VERSIONS.iter().enumerate() {
        for (requested_index, requested) in CATALOG_LANDING_ZONE_VERSIONS.iter().enumerate() {
            let case = format!("{deployed} -> {requested}");
            let refusal = version_downgrade_message(deployed, requested);
            assert_eq!(
                refusal.is_some(),
                requested_index < deployed_index,
                "case {case:?}: expected a refusal only for a move backwards, got {refusal:?}"
            );
            if let Some(message) = refusal {
                assert!(
                    message.contains(deployed) && message.contains(requested),
                    "case {case:?}: expected the refusal to name both versions, got \
                     {message:?}"
                );
            }
        }
    }

    // (case, the landing zone's version, the requested version, whether it is refused)
    let extra = [
        ("a two-digit minor is newer, not older", "3.3", "3.10", false),
        (
            "and the move back from it is older",
            "3.10",
            "3.3",
            true,
        ),
        ("a larger major is newer", "3.3", "10.0", false),
        ("a smaller major is older", "10.0", "3.3", true),
        ("the same two-digit minor is not a move", "3.10", "3.10", false),
        // `^\d+.\d+$` leaves its separator unescaped, so the model admits values that
        // carry no numeric components to compare: nothing establishes a move backwards.
        ("a version with no orderable components", "3x3", "2.0", false),
        ("a requested version that cannot be ordered", "3.3", "3x3", false),
    ];
    for (case, deployed, requested, refused) in extra {
        let refusal = version_downgrade_message(deployed, requested);
        assert_eq!(
            refusal.is_some(),
            refused,
            "case {case:?} ({deployed} -> {requested}): expected refused={refused}, got \
             {refusal:?}"
        );
    }
}

/// RULE: the version a further update is ordered against is the one the landing zone is
/// ON ITS WAY to while a deployment is still in flight, so a version already accepted
/// cannot be walked backwards by a later call that arrives before it is due.
///
/// The in-flight state is produced by driving a real update through the router and reading
/// the landing zone out of the shared engine before the clock makes that deployment due —
/// the same store the handlers write — so what is asserted is the service's own state and
/// not a record this test assembled.
#[tokio::test]
async fn the_order_is_against_the_version_a_deployment_is_on_its_way_to() {
    let newest = CATALOG_LANDING_ZONE_VERSIONS
        .last()
        .copied()
        .expect("the catalog offers at least one version");
    let oldest = CATALOG_LANDING_ZONE_VERSIONS[0];

    let (router, engine) = emulator_with_engine();
    let deployed_manifest = manifest_governing(&["us-west-2"]);
    let arn =
        create_settled_landing_zone(&router, "a deployment in flight", oldest, &deployed_manifest)
            .await;

    let accepted = call(
        &router,
        "/update-landingzone",
        serde_json::json!({ "landingZoneIdentifier": arn, "version": newest }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the update that goes in flight: expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );

    // Read the store the handlers write, while the deployment is still owed.
    let state = service_state(&*engine).expect("the service is mounted on an engine with state");
    let store = state.lock();
    let landing_zone = store
        .landing_zones
        .get(&arn)
        .expect("the created landing zone is stored under its ARN");
    assert_eq!(
        (landing_zone.version.as_str(), landing_zone.configured_version()),
        (oldest, newest),
        "with a deployment in flight: expected {oldest:?} deployed and {newest:?} on its \
         way, got {landing_zone:?}"
    );
    assert!(
        version_downgrade_message(landing_zone.configured_version(), oldest).is_some(),
        "expected a move back to {oldest:?} to be refused while {newest:?} is being \
         deployed, got no refusal"
    );
}

/// RULE, over the wire: an update to a version older than the landing zone's is
/// `ValidationException` (400) and changes nothing — no configuration is queued and none
/// is deployed once the span passes. The landing zone being ACTIVE straight after the
/// refusal is what says no `UPDATE` operation was started either: a started one leaves it
/// `PROCESSING` until the clock makes it due.
///
/// Rows differ along the SIZE of the move backwards (one minor, one minor across the
/// major boundary, the whole published line) and along where in the catalog the landing
/// zone sits.
#[tokio::test]
async fn an_update_to_an_older_version_is_refused_over_the_wire() {
    // (case, the deployed version, the version the update asks for)
    let rows = [
        ("one minor step back", "2.1", "2.0"),
        ("one minor step back from the newest", "3.3", "3.2"),
        ("back across the major boundary", "3.0", "2.9"),
        ("the whole published line, newest to oldest", "3.3", "2.0"),
    ];

    for (case, deployed_version, requested_version) in rows {
        let router = emulator();
        let deployed_manifest = manifest_governing(&["us-west-2"]);
        let arn =
            create_settled_landing_zone(&router, case, deployed_version, &deployed_manifest).await;

        let answer = call(
            &router,
            "/update-landingzone",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": requested_version,
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({deployed_version} -> {requested_version}): expected 400 \
             ValidationException, got {:?} body {:?}",
            (answer.status, answer.error_code.as_deref()),
            answer.body
        );
        assert!(
            answer
                .body
                .get("message")
                .and_then(|value| value.as_str())
                .is_some_and(|message| message.contains(requested_version)
                    && message.contains(deployed_version)),
            "case {case:?}: expected the refusal to name both versions, got {:?}",
            answer.body
        );

        // Nothing was queued and no operation was started: the landing zone is still
        // ACTIVE on the version it had, and it stays there once the span has passed.
        let immediately = get_landing_zone(&router, case, &arn).await;
        assert_eq!(
            (
                immediately.get("version").cloned(),
                immediately.get("status").cloned(),
            ),
            (
                Some(serde_json::json!(deployed_version)),
                Some(serde_json::json!("ACTIVE")),
            ),
            "case {case:?}: expected the refused update to leave {deployed_version:?} \
             deployed and ACTIVE, got {immediately:?}"
        );

        tokio::time::sleep(past_the_settle_span()).await;
        let later = get_landing_zone(&router, case, &arn).await;
        assert_eq!(
            (later.get("version").cloned(), later.get("manifest").cloned()),
            (
                Some(serde_json::json!(deployed_version)),
                Some(deployed_manifest.clone()),
            ),
            "case {case:?}: expected the refused update to deploy nothing once the span \
             passed, got {later:?}"
        );
    }
}

/// RULE, over the wire: a move that is NOT backwards is accepted and deployed — the same
/// version re-applied, and a step forward — so the refusal above is the move backwards
/// and not the comparison itself.
///
/// The forward rows of the catalog are covered in `landing_zone_update`; the row that
/// matters here is the BOUNDARY, the same version, which is accepted.
#[tokio::test]
async fn re_applying_the_deployed_version_or_moving_forward_is_accepted() {
    // (case, the deployed version, the version the update asks for)
    let rows = [
        ("the catalog's oldest version re-applied", "2.0", "2.0"),
        ("a mid-line version re-applied", "3.1", "3.1"),
        ("the catalog's newest version re-applied", "3.3", "3.3"),
        ("one step forward", "3.1", "3.2"),
    ];

    for (case, deployed_version, requested_version) in rows {
        let router = emulator();
        let deployed_manifest = manifest_governing(&["us-west-2"]);
        let arn =
            create_settled_landing_zone(&router, case, deployed_version, &deployed_manifest).await;
        let updated_manifest = manifest_governing(&["us-west-2", "eu-west-1"]);

        let answer = call(
            &router,
            "/update-landingzone",
            serde_json::json!({
                "landingZoneIdentifier": arn,
                "version": requested_version,
                "manifest": updated_manifest,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} ({deployed_version} -> {requested_version}): expected 200, got \
             {} body {:?}",
            answer.status, answer.body
        );

        tokio::time::sleep(past_the_settle_span()).await;
        let detail = get_landing_zone(&router, case, &arn).await;
        assert_eq!(
            (
                detail.get("version").cloned(),
                detail.get("manifest").cloned(),
                detail.get("status").cloned(),
            ),
            (
                Some(serde_json::json!(requested_version)),
                Some(updated_manifest.clone()),
                Some(serde_json::json!("ACTIVE")),
            ),
            "case {case:?}: expected the requested configuration deployed and ACTIVE, got \
             {detail:?}"
        );
    }
}

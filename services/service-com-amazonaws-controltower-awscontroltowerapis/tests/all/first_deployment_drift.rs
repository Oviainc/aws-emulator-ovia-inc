// SPDX-License-Identifier: CC0-1.0

//! The drift a resource reports while its FIRST configuration is still being deployed, end
//! to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes and JSON
//! bodies — not the handlers' Rust signatures.
//!
//! The RULE under test: a drift assessment is a property of a DEPLOYED configuration, so an
//! asynchronous operation that has STARTED deploying one reports no assessment for it until
//! the clock makes that deployment due, and reports the assessment the deployed
//! configuration earns from then on. [`crate::in_flight_configuration_change_drift`] asserts
//! the rule for a call that changes an ALREADY deployed configuration and
//! [`crate::in_flight_teardown_drift`] for one that removes it; this suite asserts it for the
//! FIRST-EVER deployment of a resource — above all `CreateLandingZone`, whose landing zone is
//! `PROCESSING` until its `CREATE` operation is due.
//!
//! For a landing zone "no assessment" is the absent `LandingZoneDetail$driftStatus` member
//! (the status set the model declares is `DRIFTED` | `IN_SYNC`, neither of which means
//! "unassessed"), and the assessment a freshly deployed one earns is `IN_SYNC`: nothing has
//! diverged from a configuration at the instant it is deployed. The status values written out
//! here come from `smithy_query(query="shape", shape="LandingZoneStatus,\
//! LandingZoneDriftStatus,EnablementStatus,DriftStatus")` on this crate's `smithy-model/`:
//! `ACTIVE` | `PROCESSING` | `FAILED`, `DRIFTED` | `IN_SYNC`, `SUCCEEDED` | `FAILED` |
//! `UNDER_CHANGE`, and `DRIFTED` | `IN_SYNC` | `NOT_CHECKING` | `UNKNOWN`.
//!
//! The landing zone table's rows are what the rule generalises over — WHICH configuration the
//! create starts deploying: the version (the oldest, a middle and the newest entry of
//! `state::CATALOG_LANDING_ZONE_VERSIONS`, the catalog `CreateLandingZone` accepts a version
//! against), how many regions the manifest governs (none, one, several), whether
//! `remediationTypes` is carried (`INHERITANCE_DRIFT` is the only value the model's
//! `RemediationType` declares, and the list's modelled length is 1..1) and whether `tags`
//! are. Each row is its own emulator, because the account holds one landing zone.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    BASELINE_CATALOG, CATALOG_LANDING_ZONE_VERSIONS, CONTROL_CATALOG,
};

use crate::enabled_control_lifecycle::{
    details as control_details, drift_dimensions, enable_with, ou, status_summary_status,
};
use crate::in_flight_configuration_change_drift::{
    baseline_details, baseline_status, deployed_summary, drift_summary, enable_baseline_on,
    ou_target,
};
use crate::landing_zone_lifecycle::{
    call, emulator, manifest_governing, past_the_settle_span, Answer,
};

/// One row of the landing zone table: the configuration a `CreateLandingZone` call starts
/// deploying.
#[derive(Debug, Clone)]
struct CreateRow {
    case: String,
    version: &'static str,
    /// The regions the request's manifest governs.
    governed_regions: &'static [&'static str],
    /// The `remediationTypes` member the request carries; `None` omits it.
    remediation_types: Option<&'static [&'static str]>,
    /// The `tags` member the request carries; `None` omits it.
    tags: Option<serde_json::Value>,
}

/// The landing zone table, spanning the versions the catalog publishes and the shapes of the
/// configuration a create can carry.
fn create_rows() -> Vec<CreateRow> {
    // The catalog is the source of the versions a create is accepted at, so the row set
    // follows it rather than naming versions of its own: its ends and its middle.
    let oldest = CATALOG_LANDING_ZONE_VERSIONS
        .first()
        .copied()
        .expect("the catalog must publish at least one landing zone version");
    let newest = CATALOG_LANDING_ZONE_VERSIONS
        .last()
        .copied()
        .expect("the catalog must publish at least one landing zone version");
    let middle = CATALOG_LANDING_ZONE_VERSIONS[CATALOG_LANDING_ZONE_VERSIONS.len() / 2];

    vec![
        CreateRow {
            case: format!("the oldest published version {oldest}, one governed region"),
            version: oldest,
            governed_regions: &["us-west-2"],
            remediation_types: None,
            tags: None,
        },
        CreateRow {
            case: format!("a middle published version {middle}, several governed regions"),
            version: middle,
            governed_regions: &["us-west-2", "eu-west-1", "ap-south-1"],
            remediation_types: None,
            tags: None,
        },
        CreateRow {
            case: format!("the newest published version {newest}, no governed region"),
            version: newest,
            governed_regions: &[],
            remediation_types: None,
            tags: None,
        },
        CreateRow {
            case: format!("{newest} with remediationTypes"),
            version: newest,
            governed_regions: &["us-west-2"],
            remediation_types: Some(&["INHERITANCE_DRIFT"]),
            tags: None,
        },
        CreateRow {
            case: format!("{oldest} with tags and remediationTypes"),
            version: oldest,
            governed_regions: &["eu-central-1", "us-east-1"],
            remediation_types: Some(&["INHERITANCE_DRIFT"]),
            tags: Some(serde_json::json!({ "team": "platform", "tier": "prod" })),
        },
    ]
}

/// POST `/create-landingzone` for one row, asserting it succeeded.
async fn create(router: &MultiServiceRouter, row: &CreateRow) -> Answer {
    let mut body = serde_json::json!({
        "version": row.version,
        "manifest": manifest_governing(row.governed_regions),
    });
    if let Some(types) = row.remediation_types {
        body["remediationTypes"] = serde_json::json!(types);
    }
    if let Some(tags) = row.tags.clone() {
        body["tags"] = tags;
    }
    let answer = call(router, "/create-landingzone", body).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: create-landingzone expected 200, got {} body {:?}",
        row.case, answer.status, answer.body
    );
    answer
}

/// `GetLandingZone`'s `landingZone` detail for `identifier`, asserting it resolved.
async fn landing_zone(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
) -> serde_json::Value {
    let got = call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: get-landingzone expected 200, got {} body {:?}",
        got.status, got.body
    );
    got.get("landingZone").clone()
}

/// The `status` and `driftStatus` members a landing zone reports, each absent as `None`.
fn reported(detail: &serde_json::Value) -> (Option<&str>, Option<serde_json::Value>) {
    (
        detail.get("status").and_then(serde_json::Value::as_str),
        detail.get("driftStatus").cloned(),
    )
}

/// RULE: a landing zone whose `CREATE` has started deploying its first configuration reports
/// no `driftStatus` at all while that operation is in flight — whatever version, manifest,
/// remediation types and tags the create carried — and reports `IN_SYNC` once the clock makes
/// the deployment due.
#[tokio::test]
async fn a_landing_zone_being_created_reports_no_drift_assessment() {
    let rows = create_rows();
    assert!(
        rows.len() >= 4,
        "the table must span the rule, got {} rows",
        rows.len()
    );

    // One emulator per row: the account holds at most one landing zone, so the rows cannot
    // share a store.
    let mut created: Vec<(CreateRow, MultiServiceRouter, String)> = Vec::new();
    for row in rows {
        let router = emulator();
        let answer = create(&router, &row).await;
        let arn = answer.string("arn");

        // Read back straight away, inside the window the CREATE is in flight: nothing is
        // deployed yet, so there is no configuration to have diverged from and no
        // assessment to report.
        let detail = landing_zone(&router, &row.case, &arn).await;
        assert_eq!(
            reported(&detail),
            (Some("PROCESSING"), None),
            "case {:?}: expected PROCESSING and no driftStatus while the create is still \
             deploying the manifest, got {detail:?}",
            row.case
        );

        // Reporting does not deploy anything: a second read inside the same window still
        // withholds the assessment.
        let again = landing_zone(&router, &row.case, &arn).await;
        assert_eq!(
            reported(&again),
            (Some("PROCESSING"), None),
            "case {:?}: expected a read not to publish the assessment the deployment owes, \
             got {again:?}",
            row.case
        );

        created.push((row, router, arn));
    }

    // The clock makes every CREATE due; nothing sleeps server-side, the wait is the test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (row, router, arn) in &created {
        let detail = landing_zone(router, &row.case, arn).await;
        assert_eq!(
            reported(&detail),
            (
                Some("ACTIVE"),
                Some(serde_json::json!({ "status": "IN_SYNC" }))
            ),
            "case {:?}: expected ACTIVE and IN_SYNC once the create's deployment is due, got \
             {detail:?}",
            row.case
        );

        // The deployed configuration is the one the create supplied, so the assessment is
        // about the manifest a caller can read back.
        assert_eq!(
            detail.get("version").and_then(serde_json::Value::as_str),
            Some(row.version),
            "case {:?}: expected the created version {:?} to be deployed, got {detail:?}",
            row.case,
            row.version
        );
        assert_eq!(
            detail.get("manifest"),
            Some(&manifest_governing(row.governed_regions)),
            "case {:?}: expected the manifest the create was given to be deployed, got \
             {detail:?}",
            row.case
        );
        let expected_remediation_types = row
            .remediation_types
            .map(|types| serde_json::json!(types));
        assert_eq!(
            detail.get("remediationTypes").cloned(),
            expected_remediation_types,
            "case {:?}: expected remediationTypes {expected_remediation_types:?}, got \
             {detail:?}",
            row.case
        );

        // A report does not disturb what a later caller sees: the assessment is the same on
        // a second read.
        let reread = landing_zone(router, &row.case, arn).await;
        assert_eq!(
            reported(&reread),
            reported(&detail),
            "case {:?}: expected the reported status and drift to be stable across reads, got \
             {reread:?}",
            row.case
        );
    }
}

/// RULE (the same one, over the resources this service deploys a FIRST configuration for): a
/// landing zone a create is deploying, an enabled control an enable is deploying and an
/// enabled baseline an enable is applying all withhold their drift assessment until the clock
/// makes that operation due, each in the form its own shape has for "not assessed" — an
/// absent `driftStatus`, `UNKNOWN` in every dimension, and an absent `driftStatusSummary`.
#[tokio::test]
async fn every_first_deployment_withholds_its_drift_assessment() {
    let router = emulator();

    // The landing zone: the configuration the create starts deploying.
    let created = create(
        &router,
        &CreateRow {
            case: "the first deployment of a landing zone".to_string(),
            version: CATALOG_LANDING_ZONE_VERSIONS
                .last()
                .copied()
                .expect("the catalog must publish at least one landing zone version"),
            governed_regions: &["us-west-2"],
            remediation_types: None,
            tags: None,
        },
    )
    .await;
    let landing_zone_arn = created.string("arn");

    // The control and the baseline: the first configuration an enable deploys. Both are
    // read off the catalogs the enables resolve an identifier against, so no case can name
    // one this emulator does not publish.
    let catalog_control = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable)
        .expect("the catalog must publish an enablable control");
    let (control_arn, _) = enable_with(
        &router,
        &crate::enabled_control_lifecycle::control(catalog_control.name),
        &ou("fdd1"),
        None,
    )
    .await;

    let catalog_baseline = BASELINE_CATALOG
        .first()
        .expect("the catalog must publish at least one baseline");
    let baseline_version = catalog_baseline
        .versions
        .last()
        .copied()
        .expect("every catalog baseline offers at least one version");
    let enabled_baseline = enable_baseline_on(
        &router,
        catalog_baseline,
        baseline_version,
        None,
        &ou_target("fdd", 1),
    )
    .await;
    assert_eq!(
        enabled_baseline.status, 200,
        "enable-baseline expected 200, got {} body {:?}",
        enabled_baseline.status, enabled_baseline.body
    );
    let baseline_arn = enabled_baseline.string("arn");

    // In flight: none of the three has a deployed configuration to assess.
    let case = "every first deployment in flight";
    let detail = landing_zone(&router, case, &landing_zone_arn).await;
    assert_eq!(
        reported(&detail),
        (Some("PROCESSING"), None),
        "case {case:?}: expected the landing zone to report PROCESSING and no driftStatus, \
         got {detail:?}"
    );
    let control = control_details(&router, &control_arn).await;
    assert_eq!(
        (status_summary_status(&control), drift_dimensions(&control)),
        (
            Some("UNDER_CHANGE"),
            (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN"))
        ),
        "case {case:?}: expected the control to report UNDER_CHANGE with UNKNOWN drift, got \
         {control:?}"
    );
    let baseline = baseline_details(&router, case, &baseline_arn).await;
    assert_eq!(
        (baseline_status(&baseline), drift_summary(&baseline)),
        (Some("UNDER_CHANGE"), None),
        "case {case:?}: expected the baseline to report UNDER_CHANGE and no \
         driftStatusSummary, got {baseline:?}"
    );

    tokio::time::sleep(past_the_settle_span()).await;

    // Deployed: each reports the assessment its own shape carries.
    let case = "every first deployment due";
    let detail = landing_zone(&router, case, &landing_zone_arn).await;
    assert_eq!(
        reported(&detail),
        (
            Some("ACTIVE"),
            Some(serde_json::json!({ "status": "IN_SYNC" }))
        ),
        "case {case:?}: expected the landing zone to report ACTIVE and IN_SYNC, got {detail:?}"
    );
    let control = control_details(&router, &control_arn).await;
    assert_eq!(
        (status_summary_status(&control), drift_dimensions(&control)),
        (
            Some("SUCCEEDED"),
            (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC"))
        ),
        "case {case:?}: expected the control to report SUCCEEDED and IN_SYNC in every \
         dimension, got {control:?}"
    );
    let expected_summary = deployed_summary(catalog_baseline);
    let baseline = baseline_details(&router, case, &baseline_arn).await;
    assert_eq!(
        (baseline_status(&baseline), drift_summary(&baseline)),
        (Some("SUCCEEDED"), expected_summary.clone()),
        "case {case:?}: expected the baseline to report SUCCEEDED and driftStatusSummary \
         {expected_summary:?}, got {baseline:?}"
    );
}

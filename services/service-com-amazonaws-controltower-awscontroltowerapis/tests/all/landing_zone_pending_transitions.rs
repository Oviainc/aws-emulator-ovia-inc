// SPDX-License-Identifier: CC0-1.0

//! The transitions an asynchronous landing-zone call parks on the landing zone, end to end
//! over HTTP.
//!
//! The RULE under test: a transition an asynchronous call starts is parked ON the resource
//! it changes, carrying its own due instant and the identifier of the operation that
//! started it, so what a reporting call reports, what a second mutating call collides with
//! and what the clock applies depend on the resource and that operation's terminal status
//! alone — never on the bounded, self-evicting operation log still holding the record of
//! the call that started it, and a transition whose own operation FAILED applies nothing.
//!
//! Asserted over TABLES whose rows are what the rule generalises over: WHICH asynchronous
//! call parked the transition — `UpdateLandingZone` (a configuration being deployed),
//! `ResetLandingZone` (the deployed configuration being re-deployed) and
//! `DeleteLandingZone` (a decommissioning) — and, for an update, which members of the
//! configuration the call carried.
//!
//! Two things no sequence of calls can produce are seeded straight into the store, the way
//! the sibling suites' retention cases are: a record dropped out of the log while the
//! transition it started is still owed (the log evicts the oldest record only once every
//! record is in flight, and a landing zone carries at most one operation at a time), and an
//! operation that FAILED (nothing in this crate fails a landing-zone operation yet).
//! Everything else is driven through HTTP.
//!
//! The retention rule is the crate's own —
//! `service_..._awscontroltowerapis::state::MAX_LANDING_ZONE_OPERATIONS` records, the
//! oldest TERMINAL one evicted first and the oldest overall only when every record is in
//! flight. The statuses and operation types come from
//! `smithy_query(query="shape", shape="LandingZoneStatus,LandingZoneOperationStatus,LandingZoneOperationType,RemediationType")`
//! on this crate's `smithy-model/`: `ACTIVE` | `PROCESSING` | `FAILED`, `SUCCEEDED` |
//! `FAILED` | `IN_PROGRESS`, `CREATE` | `DELETE` | `RESET` | `UPDATE`, and the single
//! `RemediationType` value `INHERITANCE_DRIFT`.

use std::time::{Duration, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, LandingZoneDriftStatus, LandingZoneOperationStatus, LandingZoneOperationType,
    MAX_LANDING_ZONE_OPERATIONS,
};

use crate::landing_zone_lifecycle::{
    call, emulator_with_engine, manifest_governing, past_the_settle_span, Answer,
};
use crate::resource_tags::{apply_tags, read_tags, reported_tags, tags};

/// A landing zone ARN of the form this service mints, carried by the records a case appends
/// to the log directly: no landing zone was ever created under it, so a seeded record names
/// no resource a case asserts about.
const SEEDED_ARN: &str = "arn:aws:controltower:us-west-2:000000000000:landingzone/log-retention";

/// The version every row creates its landing zone at: inside the catalog and below its
/// newest, so an update has somewhere to move to.
const DEPLOYED_VERSION: &str = "3.0";

/// The region the manifest every row deploys governs.
const DEPLOYED_REGION: &str = "us-west-2";

/// Which asynchronous call parks the transition a row probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Started {
    Update,
    Reset,
    Delete,
}

/// Create the account's one landing zone and wait for its `CREATE` to become due, so the
/// call a row starts is not refused for an operation already in flight. Answers its ARN.
async fn settled_landing_zone(router: &MultiServiceRouter, case: &str) -> String {
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({
            "version": DEPLOYED_VERSION,
            "manifest": manifest_governing(&[DEPLOYED_REGION]),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected the create to return 200, got {} body {:?}",
        created.status, created.body
    );
    // Nothing sleeps server-side: the clock makes the create due, and the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;
    created.string("arn")
}

/// POST the call `started` names against `arn`, carrying `members` on top of the
/// `landingZoneIdentifier`.
async fn start(
    router: &MultiServiceRouter,
    started: Started,
    arn: &str,
    members: &serde_json::Value,
) -> Answer {
    let (path, mut body) = match started {
        Started::Update => (
            "/update-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        ),
        Started::Reset => (
            "/reset-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        ),
        Started::Delete => (
            "/delete-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn }),
        ),
    };
    if let Some(members) = members.as_object() {
        for (name, value) in members {
            body[name.as_str()] = value.clone();
        }
    }
    call(router, path, body).await
}

/// `GetLandingZone` for `identifier`: the whole answer, so a row can assert a refusal as
/// well as a reported landing zone.
async fn get_landing_zone(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-landingzone",
        serde_json::json!({ "landingZoneIdentifier": identifier }),
    )
    .await
}

/// The `landingZone` member of a `GetLandingZone` answer.
fn detail(answer: &Answer) -> serde_json::Value {
    answer.get("landingZone").clone()
}

/// A member of the reported `LandingZoneDetail`, as a `&str`.
fn member<'a>(detail: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    detail.get(name).and_then(serde_json::Value::as_str)
}

/// The reported `driftStatus.status`.
fn drift_status(detail: &serde_json::Value) -> Option<&str> {
    detail
        .get("driftStatus")
        .and_then(|summary| member(summary, "status"))
}

/// Whether `/get-landingzone-operation` still resolves `identifier`.
async fn operation_resolves(router: &MultiServiceRouter, identifier: &str) -> bool {
    let answer = call(
        router,
        "/get-landingzone-operation",
        serde_json::json!({ "operationIdentifier": identifier }),
    )
    .await;
    answer.status == 200
}

/// Drop `identifier` out of the landing zone operation log by driving the log to its
/// retention bound with every record in flight — the one case the bound evicts a record
/// whose own transition is still owed.
///
/// The seeded records start in the FUTURE, so they are the newest records in the log and
/// stay in flight however long the case takes: the record the bound drops once nothing is
/// terminal any more is therefore the oldest in-flight one, which is the record under test.
/// One record is appended at a time until `identifier` is gone, so the case does not depend
/// on how many records the calls before it left in the log.
fn evict_from_the_log(engine: &engine::InMemoryEngine, identifier: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let base = SystemTime::now() + Duration::from_secs(60);
    let mut seeded = 0usize;
    while store.landing_zone_operations.contains_key(identifier) {
        assert!(
            seeded <= MAX_LANDING_ZONE_OPERATIONS + 8,
            "seeding {seeded} records did not push {identifier} out of a log bounded at \
             {MAX_LANDING_ZONE_OPERATIONS}"
        );
        store.record_operation(
            LandingZoneOperationType::Update,
            SEEDED_ARN,
            base + Duration::from_millis(seeded as u64),
        );
        seeded += 1;
    }
}

/// Report the deployed landing zone as having diverged from its manifest.
///
/// Nothing in this crate drifts a landing zone yet, so a row whose subject is what a
/// deployment does to the reported drift status seeds the divergence into the store.
fn drift_the_landing_zone(engine: &engine::InMemoryEngine, arn: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let landing_zone = store
        .landing_zones
        .get_mut(arn)
        .expect("the landing zone the case created");
    landing_zone.drift_status = Some(LandingZoneDriftStatus::Drifted);
}

/// Fail the landing zone operation `identifier` names, at the instant the clock owes it its
/// terminal status.
///
/// Nothing in this crate fails a landing-zone operation yet, so a row whose subject is what
/// the clock does with a FAILED operation's parked transition seeds the failure into the
/// log.
fn fail_the_operation(engine: &engine::InMemoryEngine, identifier: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let record = store
        .landing_zone_operations
        .get_mut(identifier)
        .expect("the record of the operation the case started");
    record.status = LandingZoneOperationStatus::Failed;
    record.end_time = Some(record.due_at);
}

/// RULE: the transition an asynchronous call parked on a landing zone is reported and
/// applied from the RESOURCE, so dropping that call's record out of the bounded operation
/// log changes neither — the landing zone reads `PROCESSING` and refuses a second mutating
/// call with `ConflictException` (409) until the clock makes the transition due, and then
/// the clock applies exactly what was parked.
#[tokio::test]
async fn a_parked_transition_is_reported_and_applied_though_its_record_was_evicted() {
    // (case, the call that parks the transition, the members it carries, the version
    // deployed once the clock has made that transition due)
    let rows: Vec<(&str, Started, serde_json::Value, &str)> = vec![
        (
            "a configuration being deployed, its UPDATE record evicted",
            Started::Update,
            serde_json::json!({
                "version": "3.2",
                "manifest": manifest_governing(&["eu-west-1"]),
            }),
            "3.2",
        ),
        (
            "the deployed configuration being re-deployed, its RESET record evicted",
            Started::Reset,
            serde_json::json!({}),
            DEPLOYED_VERSION,
        ),
        (
            "a decommissioning still running, its DELETE record evicted",
            Started::Delete,
            serde_json::json!({}),
            DEPLOYED_VERSION,
        ),
    ];

    for (case, started, members, settled_version) in rows {
        // A router per row, so the rows cannot evict each other's records.
        let (router, engine) = emulator_with_engine();
        let arn = settled_landing_zone(&router, case).await;

        let answer = start(&router, started, &arn, &members).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the call that starts the transition expected 200, got {} body \
             {:?}",
            answer.status, answer.body
        );
        let identifier = answer.string("operationIdentifier");

        // The record is dropped while the transition it started is still owed.
        evict_from_the_log(&engine, &identifier);
        assert!(
            !operation_resolves(&router, &identifier).await,
            "case {case:?}: expected {identifier:?} to have been evicted from the log, but \
             get-landingzone-operation still resolves it"
        );

        // The transient state is the resource's own: a record the log no longer holds does
        // not make a landing zone still being changed read as settled, and does not let a
        // second call overwrite the transition the clock still owes.
        let reported = get_landing_zone(&router, &arn).await;
        let reported_detail = detail(&reported);
        assert_eq!(
            (reported.status, member(&reported_detail, "status")),
            (200, Some("PROCESSING")),
            "case {case:?}: expected the landing zone to read PROCESSING while its \
             transition is owed, got {:?} detail {reported_detail:?}",
            reported.status
        );
        assert_eq!(
            member(&reported_detail, "version"),
            Some(DEPLOYED_VERSION),
            "case {case:?}: expected the DEPLOYED version {DEPLOYED_VERSION:?} while the \
             transition is owed, got {reported_detail:?}"
        );

        let refused = call(
            &router,
            "/update-landingzone",
            serde_json::json!({ "landingZoneIdentifier": arn, "version": "3.3" }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: expected a second mutating call to be refused with 409 \
             ConflictException while the transition is owed, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );

        if started == Started::Delete {
            // `@idempotent` off the parked removal: the repeat reports that removal's own
            // identifier, which the log no longer holds.
            let repeated = start(&router, Started::Delete, &arn, &serde_json::json!({})).await;
            assert_eq!(
                (repeated.status, repeated.string("operationIdentifier")),
                (200, identifier.clone()),
                "case {case:?}: a repeated delete expected 200 and {identifier:?}, got {} \
                 body {:?}",
                repeated.status,
                repeated.body
            );
        }

        tokio::time::sleep(past_the_settle_span()).await;

        let settled = get_landing_zone(&router, &arn).await;
        if started == Started::Delete {
            assert_eq!(
                (settled.status, settled.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the decommissioning to have removed the landing \
                 zone (404 ResourceNotFoundException), got {:?} body {:?}",
                (settled.status, settled.error_code.as_deref()),
                settled.body
            );
            continue;
        }
        let settled_detail = detail(&settled);
        assert_eq!(
            (
                settled.status,
                member(&settled_detail, "status"),
                member(&settled_detail, "version"),
                drift_status(&settled_detail),
            ),
            (200, Some("ACTIVE"), Some(settled_version), Some("IN_SYNC")),
            "case {case:?}: expected the parked configuration to be deployed — ACTIVE at \
             {settled_version:?}, IN_SYNC — got {:?} detail {settled_detail:?}",
            settled.status
        );
    }
}

/// One row of the failed-operation table.
///
/// What the call asks for is stated ONCE, as the members below: both the request the row
/// sends and what the landing zone must report once that call is allowed to succeed are
/// derived from them.
struct FailureCase {
    case: &'static str,
    /// The call whose operation fails.
    started: Started,
    /// The `version` the call asks for, `None` when it carries none (a reset, a delete).
    requested_version: Option<&'static str>,
    /// The regions the manifest the call carries governs, `None` when it carries no
    /// manifest and the deployed one is carried forward.
    requested_manifest_regions: Option<&'static [&'static str]>,
    /// The `remediationTypes` the call carries, `None` when it carries none.
    requested_remediation_types: Option<&'static [&'static str]>,
    /// The drift status that successful call leaves: a deployment of a manifest the call
    /// supplied is in sync with it, while a call that supplies none carries the drift
    /// status forward, and a reset re-deploys the manifest the landing zone already has.
    succeeds_to_drift: &'static str,
}

impl FailureCase {
    /// The members this row's call carries on top of the `landingZoneIdentifier`.
    fn members(&self) -> serde_json::Value {
        let mut members = serde_json::json!({});
        if let Some(version) = self.requested_version {
            members["version"] = serde_json::json!(version);
        }
        if let Some(regions) = self.requested_manifest_regions {
            members["manifest"] = manifest_governing(regions);
        }
        if let Some(types) = self.requested_remediation_types {
            members["remediationTypes"] = serde_json::json!(types);
        }
        members
    }

    /// The version deployed once this row's call is allowed to succeed: a call that names
    /// no version deploys the one that is already there.
    fn succeeds_to_version(&self) -> &'static str {
        self.requested_version.unwrap_or(DEPLOYED_VERSION)
    }
}

/// The failed-operation table. Rows differ along which call parked the transition and, for
/// an update, which members of the configuration it carried — the version alone, a new
/// manifest, or `remediationTypes` (`RemediationType` has the single value
/// `INHERITANCE_DRIFT`, and the list's length is 1..1).
fn failure_cases() -> Vec<FailureCase> {
    vec![
        FailureCase {
            case: "a failed UPDATE of the version alone",
            started: Started::Update,
            requested_version: Some("3.2"),
            requested_manifest_regions: None,
            requested_remediation_types: None,
            succeeds_to_drift: "DRIFTED",
        },
        FailureCase {
            case: "a failed UPDATE carrying a new manifest",
            started: Started::Update,
            requested_version: Some("3.3"),
            requested_manifest_regions: Some(&["eu-west-1", "us-east-1"]),
            requested_remediation_types: None,
            succeeds_to_drift: "IN_SYNC",
        },
        FailureCase {
            case: "a failed UPDATE carrying remediationTypes",
            started: Started::Update,
            requested_version: Some("3.1"),
            requested_manifest_regions: None,
            requested_remediation_types: Some(&["INHERITANCE_DRIFT"]),
            succeeds_to_drift: "DRIFTED",
        },
        FailureCase {
            case: "a failed RESET",
            started: Started::Reset,
            requested_version: None,
            requested_manifest_regions: None,
            requested_remediation_types: None,
            succeeds_to_drift: "IN_SYNC",
        },
        FailureCase {
            case: "a failed DELETE",
            started: Started::Delete,
            requested_version: None,
            requested_manifest_regions: None,
            requested_remediation_types: None,
            succeeds_to_drift: "DRIFTED",
        },
    ]
}

/// RULE: a transition the clock has made due is applied only if the operation that started
/// it did not FAIL — a failed deployment deploys nothing, so the landing zone keeps the
/// version, manifest, remediation types and drift status it had, and a failed
/// decommissioning removes nothing, so the landing zone and its tags stay. The same call,
/// allowed to succeed, does apply the change.
#[tokio::test]
async fn a_failed_landing_zone_operation_applies_nothing() {
    let attached = tags(&[("Owner", "platform"), ("Cost-Centre", "4711")]);

    for row in failure_cases() {
        let case = row.case;
        // A router per row: the account holds one landing zone, and a row may remove it.
        let (router, engine) = emulator_with_engine();
        let arn = settled_landing_zone(&router, case).await;

        let tagged = apply_tags(&router, &arn, &attached).await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: tagging the landing zone expected 204, got {} body {:?}",
            tagged.status, tagged.body
        );

        // The landing zone has diverged from its manifest, so what a deployment does to the
        // reported drift status is observable.
        drift_the_landing_zone(&engine, &arn);
        let deployed = detail(&get_landing_zone(&router, &arn).await);
        assert_eq!(
            (
                member(&deployed, "version"),
                drift_status(&deployed),
                deployed.get("remediationTypes").cloned(),
            ),
            (Some(DEPLOYED_VERSION), Some("DRIFTED"), None),
            "case {case:?}: expected the deployed configuration to start at \
             {DEPLOYED_VERSION:?}, DRIFTED and with no remediationTypes, got {deployed:?}"
        );

        let members = row.members();
        let answer = start(&router, row.started, &arn, &members).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the call that starts the transition expected 200, got {} body \
             {:?}",
            answer.status, answer.body
        );
        let identifier = answer.string("operationIdentifier");

        // The operation fails; its transition stays parked until the clock makes it due.
        fail_the_operation(&engine, &identifier);

        let owed = get_landing_zone(&router, &arn).await;
        let owed_detail = detail(&owed);
        assert_eq!(
            (owed.status, member(&owed_detail, "status")),
            (200, Some("PROCESSING")),
            "case {case:?}: expected the landing zone to read PROCESSING while the \
             transition is owed, got {:?} detail {owed_detail:?}",
            owed.status
        );

        tokio::time::sleep(past_the_settle_span()).await;

        let after = get_landing_zone(&router, &arn).await;
        let after_detail = detail(&after);
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected the landing zone to survive the failed operation, got \
             {} body {:?}",
            after.status, after.body
        );
        assert_eq!(
            (
                member(&after_detail, "version"),
                after_detail.get("manifest").cloned(),
                after_detail.get("remediationTypes").cloned(),
                drift_status(&after_detail),
            ),
            (
                member(&deployed, "version"),
                deployed.get("manifest").cloned(),
                deployed.get("remediationTypes").cloned(),
                drift_status(&deployed),
            ),
            "case {case:?}: expected the failed operation to deploy nothing, leaving \
             {deployed:?}, got {after_detail:?}"
        );
        let kept = read_tags(&router, &arn).await;
        assert_eq!(
            (kept.status, reported_tags(&kept)),
            (200, attached.clone()),
            "case {case:?}: expected the failed operation to leave the tags alone, got {:?} \
             body {:?}",
            kept.status,
            kept.body
        );

        // Not vacuous: the SAME call, with its operation left to succeed, applies exactly
        // what the failed one did not.
        let repeated = start(&router, row.started, &arn, &members).await;
        assert_eq!(
            repeated.status, 200,
            "case {case:?}: the same call, once the failed operation is terminal, expected \
             200, got {} body {:?}",
            repeated.status, repeated.body
        );
        assert_ne!(
            repeated.string("operationIdentifier"),
            identifier,
            "case {case:?}: expected the repeated call to start a FRESH operation"
        );

        tokio::time::sleep(past_the_settle_span()).await;

        let succeeded = get_landing_zone(&router, &arn).await;
        if row.started == Started::Delete {
            assert_eq!(
                (succeeded.status, succeeded.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the succeeding decommissioning to remove the \
                 landing zone (404 ResourceNotFoundException), got {:?} body {:?}",
                (succeeded.status, succeeded.error_code.as_deref()),
                succeeded.body
            );
            let gone = read_tags(&router, &arn).await;
            assert_eq!(
                (gone.status, gone.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the tags of a decommissioned landing zone to be \
                 gone (404 ResourceNotFoundException), got {:?} tags {:?}",
                (gone.status, gone.error_code.as_deref()),
                reported_tags(&gone)
            );
            continue;
        }
        let succeeded_detail = detail(&succeeded);
        // What the succeeding call deploys is read off the row's request: the manifest it
        // carried, or the deployed one when it carried none.
        let expected_manifest = row.requested_manifest_regions.map_or_else(
            || deployed.get("manifest").cloned(),
            |regions| Some(manifest_governing(regions)),
        );
        let expected_remediation_types = row
            .requested_remediation_types
            .map(|types| serde_json::json!(types));
        assert_eq!(
            (
                succeeded.status,
                member(&succeeded_detail, "status"),
                member(&succeeded_detail, "version"),
                succeeded_detail.get("remediationTypes").cloned(),
                drift_status(&succeeded_detail),
            ),
            (
                200,
                Some("ACTIVE"),
                Some(row.succeeds_to_version()),
                expected_remediation_types.clone(),
                Some(row.succeeds_to_drift),
            ),
            "case {case:?}: expected the succeeding call to deploy {:?} with \
             remediationTypes {expected_remediation_types:?} and drift {:?}, got \
             {succeeded_detail:?}",
            row.succeeds_to_version(),
            row.succeeds_to_drift
        );
        assert_eq!(
            succeeded_detail.get("manifest").cloned(),
            expected_manifest,
            "case {case:?}: expected the succeeding call to deploy the manifest \
             {expected_manifest:?}, got {succeeded_detail:?}"
        );
    }
}

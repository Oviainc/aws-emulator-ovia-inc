// SPDX-License-Identifier: CC0-1.0

//! The transitions an asynchronous baseline call parks on its `EnabledBaseline`, end to
//! end over HTTP.
//!
//! The RULE under test: a transition an asynchronous call starts is parked ON the resource
//! it changes, with its own due instant, so what a reporting call reports and what the
//! clock applies depend on the resource alone — never on the bounded, self-evicting
//! operation log still holding the record of the call that started it.
//!
//! Asserted over a TABLE whose rows are what the rule generalises over: WHICH asynchronous
//! call parked the transition — `EnableBaseline` (an enablement being applied),
//! `UpdateEnabledBaseline` (a version being applied), `ResetEnabledBaseline` (a
//! configuration being re-applied) and `DisableBaseline` (a teardown) — each probed with
//! its own operation record dropped from the log while the transition it started is still
//! owed. Every row asserts the transient state through the wire, and then the state the
//! clock owes once the transition is due.
//!
//! The log's retention rule is the crate's own: `MAX_BASELINE_OPERATIONS` records, the
//! oldest TERMINAL one evicted first and the oldest overall only when every record is in
//! flight, read off
//! `service_..._awscontroltowerapis::state::{MAX_BASELINE_OPERATIONS, enforce_log_bound}`.
//! That last case is the one a record still owing a transition can be dropped in, and no
//! sequence of calls can reach it — an enablement carries at most one operation at a time,
//! so the wire cannot put a whole log's worth of baseline operations in flight at once —
//! which is why these rows seed the log straight into the store, the way the landing zone
//! suite's retention case does, and drive everything else through HTTP.
//!
//! The statuses and operation types come from
//! `smithy_query(query="shape", shape="EnablementStatus,BaselineOperationType")` on this
//! crate's `smithy-model/`: `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, and `ENABLE_BASELINE`
//! | `DISABLE_BASELINE` | `UPDATE_ENABLED_BASELINE` | `RESET_ENABLED_BASELINE`.

use std::time::{Duration, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, BaselineOperationType, MAX_BASELINE_OPERATIONS,
};

use crate::baseline_operations::{disable, enable, enablements};
use crate::enabled_control_lifecycle::entries;
use crate::landing_zone_lifecycle::{
    call, emulator_with_engine, past_the_settle_span, set_up_account, Answer,
};
use crate::resource_tags::{apply_tags, read_tags, reported_tags, tags};

/// The baseline this suite enables: `AWSControlTowerBaseline`, which offers the versions
/// the rows move between (`1.0` … `4.0`), read off
/// `service_..._awscontroltowerapis::state::BASELINE_CATALOG` — what `ListBaselines`
/// reports.
const BASELINE: &str = "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2";

/// The `enabledBaselineArn` the seeded log records carry: an ARN of the form this service
/// mints, under which no enablement was ever created, so a seeded record names no
/// resource a case asserts about.
const SEEDED_ARN: &str =
    "arn:aws:controltower:us-west-2:000000000000:enabledbaseline/log-retention";

/// Which asynchronous call parks the transition a row probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Started {
    Enable,
    Update,
    Reset,
    Disable,
}

/// POST `/enable-baseline` for a baseline/target pair at `version`.
async fn enable_pair(
    router: &MultiServiceRouter,
    target: &str,
    version: &str,
) -> Answer {
    call(
        router,
        "/enable-baseline",
        serde_json::json!({
            "baselineIdentifier": BASELINE,
            "baselineVersion": version,
            "targetIdentifier": target,
        }),
    )
    .await
}

/// POST `/get-enabled-baseline` and hand back the answer plus its
/// `enabledBaselineDetails` member.
async fn get_enabled_baseline(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    let details = answer.get("enabledBaselineDetails").clone();
    (answer, details)
}

/// A member of a JSON object, as a `&str`.
fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// `statusSummary.status` of an `EnabledBaselineDetails`.
fn status(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "status"))
}

/// `statusSummary.lastOperationIdentifier` of an `EnabledBaselineDetails`.
fn last_operation(details: &serde_json::Value) -> Option<&str> {
    details
        .get("statusSummary")
        .and_then(|summary| member(summary, "lastOperationIdentifier"))
}

/// The `arn` of every `ListEnabledBaselines` entry.
fn listed_arns(listed: &Answer) -> Vec<String> {
    entries(listed.get("enabledBaselines"))
        .iter()
        .map(|entry| member(entry, "arn").unwrap_or_default().to_string())
        .collect()
}

/// Whether `/get-baseline-operation` still resolves `identifier`.
async fn operation_resolves(router: &MultiServiceRouter, identifier: &str) -> bool {
    let answer = call(
        router,
        "/get-baseline-operation",
        serde_json::json!({ "operationIdentifier": identifier }),
    )
    .await;
    answer.status == 200
}

/// Drop `identifier` out of the baseline operation log by driving the log to its retention
/// bound with every record in flight — the one case the bound evicts a record whose own
/// transition is still owed.
///
/// The seeded records start in the FUTURE, so they are the newest records in the log and
/// stay in flight however long the case takes: the record the bound drops is therefore the
/// oldest in-flight one, which is the record under test. One record is appended at a time
/// until `identifier` is gone, so the case does not depend on how many records the calls
/// before it left in the log.
fn evict_from_the_log(engine: &engine::InMemoryEngine, identifier: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let base = SystemTime::now() + Duration::from_secs(60);
    let mut seeded = 0usize;
    while store.baseline_operations.contains_key(identifier) {
        assert!(
            seeded <= MAX_BASELINE_OPERATIONS + 8,
            "seeding {seeded} records did not push {identifier} out of a log bounded at \
             {MAX_BASELINE_OPERATIONS}"
        );
        store.record_baseline_operation(
            BaselineOperationType::EnableBaseline,
            SEEDED_ARN,
            base + Duration::from_millis(seeded as u64),
        );
        seeded += 1;
    }
}

/// RULE: the transition an asynchronous call parked on an `EnabledBaseline` is reported
/// and applied from the RESOURCE, so dropping that call's record out of the bounded
/// operation log changes neither: the enablement reads `UNDER_CHANGE` until the clock makes
/// the transition due, and then the clock applies exactly what was parked — the enablement
/// settles, the new version becomes the applied one, the re-application completes, the
/// teardown removes the resource.
#[tokio::test]
async fn a_parked_transition_is_reported_and_applied_though_its_record_was_evicted() {
    // (case, the call that parks the transition, the version the enablement must report
    // once the clock has made that transition due)
    let rows: Vec<(&str, Started, &str)> = vec![
        (
            "an enablement still being applied, its ENABLE_BASELINE record evicted",
            Started::Enable,
            "1.0",
        ),
        (
            "a version still being applied, its UPDATE_ENABLED_BASELINE record evicted",
            Started::Update,
            "4.0",
        ),
        (
            "a configuration being re-applied, its RESET_ENABLED_BASELINE record evicted",
            Started::Reset,
            "1.0",
        ),
        (
            "a teardown still running, its DISABLE_BASELINE record evicted",
            Started::Disable,
            "1.0",
        ),
    ];

    for (index, (case, started, settled_version)) in rows.into_iter().enumerate() {
        // A router per row, so the rows cannot evict each other's records.
        let (router, engine) = emulator_with_engine();
        set_up_account(&router).await;
        let target =
            format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-pt{index:02}-0001");

        let enabled = enable_pair(&router, &target, "1.0").await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable expected 200, got {} body {:?}",
            enabled.status, enabled.body
        );
        let arn = enabled.string("arn");
        let enable_operation = enabled.string("operationIdentifier");

        // The record under test: the enable's own for the first row, and for the others the
        // record of the call that acts on the enablement once the enable is due.
        let identifier = if started == Started::Enable {
            enable_operation
        } else {
            tokio::time::sleep(past_the_settle_span()).await;
            let answer = match started {
                Started::Update => {
                    call(
                        &router,
                        "/update-enabled-baseline",
                        serde_json::json!({
                            "enabledBaselineIdentifier": arn,
                            "baselineVersion": settled_version,
                        }),
                    )
                    .await
                }
                Started::Reset => {
                    call(
                        &router,
                        "/reset-enabled-baseline",
                        serde_json::json!({ "enabledBaselineIdentifier": arn }),
                    )
                    .await
                }
                Started::Disable => disable(&router, &arn).await,
                Started::Enable => unreachable!("the enable row is handled above"),
            };
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the call that starts the transition expected 200, got {} \
                 body {:?}",
                answer.status, answer.body
            );
            answer.string("operationIdentifier")
        };

        // The record is dropped while the transition it started is still owed.
        evict_from_the_log(&engine, &identifier);
        assert!(
            !operation_resolves(&router, &identifier).await,
            "case {case:?}: expected {identifier:?} to have been evicted from the log, but \
             get-baseline-operation still resolves it"
        );

        // The transient state is the resource's own: a record the log no longer holds does
        // not make an enablement still being changed read as settled.
        let (answer, details) = get_enabled_baseline(&router, &arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the enablement must still be reported while its transition is \
             owed, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (status(&details), last_operation(&details)),
            (Some("UNDER_CHANGE"), Some(identifier.as_str())),
            "case {case:?}: expected UNDER_CHANGE and {identifier:?} as the last \
             operation, got {details:?}"
        );

        tokio::time::sleep(past_the_settle_span()).await;

        let (answer, details) = get_enabled_baseline(&router, &arn).await;
        if started == Started::Disable {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the teardown to have removed the enablement (404 \
                 ResourceNotFoundException), got {:?} body {:?}",
                (answer.status, answer.error_code.as_deref()),
                answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the enablement to still be reported once its \
             transition is due, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (status(&details), member(&details, "baselineVersion")),
            (Some("SUCCEEDED"), Some(settled_version)),
            "case {case:?}: expected SUCCEEDED at version {settled_version:?} once the \
             transition is due, got {details:?}"
        );
    }
}

/// RULE (the teardown's blast radius): the removal `DisableBaseline` parks is what the
/// clock applies, so losing the `DISABLE_BASELINE` record out of the bounded log changes
/// nothing a caller sees — the enablement stops being reported by `GetEnabledBaseline` and
/// `ListEnabledBaselines`, its tags go with it, and its baseline/target pair is free to be
/// enabled again as a NEW resource. While the teardown is still owed the operation stays
/// `@idempotent`: a repeated call reports the identifier the teardown carries rather than
/// starting a second one.
///
/// Asserted over the enablement table the sibling `baseline_operations` suite drives
/// `EnableBaseline` with, so the rule holds whatever baseline, version, parameter list and
/// target the enablement carries.
#[tokio::test]
async fn a_teardown_whose_record_was_evicted_still_removes_the_enablement_and_its_tags() {
    let (router, engine) = emulator_with_engine();
    set_up_account(&router).await;
    let rows = enablements();
    let attached = tags(&[("Owner", "platform"), ("Cost-Centre", "4711")]);

    let mut enabled: Vec<(&'static str, String)> = Vec::new();
    for row in &rows {
        let answer = enable(&router, row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
        enabled.push((row.case, answer.string("arn")));
    }

    // Nothing sleeps server-side: the clock makes the enables due, and the wait is the
    // test's.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn) in &enabled {
        let tagged = apply_tags(&router, arn, &attached).await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: tagging the enablement expected 204, got {} body {:?}",
            tagged.status, tagged.body
        );
    }

    // Each row's teardown has its record evicted BEFORE the clock makes that teardown due,
    // so nothing in the log remembers it by the time it falls due.
    let mut teardowns: Vec<(&'static str, String, String)> = Vec::new();
    for (case, arn) in &enabled {
        let disabled = disable(&router, arn).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: teardown expected 200, got {} body {:?}",
            disabled.status, disabled.body
        );
        let identifier = disabled.string("operationIdentifier");
        evict_from_the_log(&engine, &identifier);
        assert!(
            !operation_resolves(&router, &identifier).await,
            "case {case:?}: expected {identifier:?} to have been evicted from the log, but \
             get-baseline-operation still resolves it"
        );

        // `@idempotent` off the parked teardown: the repeat reports that teardown's own
        // identifier, which the log no longer holds.
        let repeated = disable(&router, arn).await;
        assert_eq!(
            (repeated.status, repeated.string("operationIdentifier")),
            (200, identifier.clone()),
            "case {case:?}: a repeated teardown expected 200 and {identifier:?}, got {} \
             body {:?}",
            repeated.status,
            repeated.body
        );

        teardowns.push((case, arn.clone(), identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    let listed = call(&router, "/list-enabled-baselines", serde_json::json!({})).await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-baselines: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let still_listed = listed_arns(&listed);

    for (case, arn, identifier) in &teardowns {
        let (answer, details) = get_enabled_baseline(&router, arn).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the torn-down enablement to be gone (404 \
             ResourceNotFoundException), got {:?} details {details:?}",
            (answer.status, answer.error_code.as_deref())
        );
        assert!(
            !still_listed.contains(arn),
            "case {case:?}: expected {arn:?} not to be listed any more, got \
             {still_listed:?}"
        );

        // A tag entry never outlives the resource it tags.
        let read = read_tags(&router, arn).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the tags of a torn-down enablement to be gone (404 \
             ResourceNotFoundException), got {:?} tags {:?}",
            (read.status, read.error_code.as_deref()),
            reported_tags(&read)
        );

        // The teardown's own record is the one thing the eviction did lose: the removal it
        // owed was applied all the same.
        assert!(
            !operation_resolves(&router, identifier).await,
            "case {case:?}: expected the evicted {identifier:?} to stay unresolvable"
        );
    }

    // The pair is free again, and enables as a NEW resource.
    for (row, (case, arn, _)) in rows.iter().zip(&teardowns) {
        let reenabled = enable(&router, row).await;
        assert_eq!(
            reenabled.status, 200,
            "case {case:?}: enabling the pair again once its teardown is due expected 200, \
             got {} body {:?}",
            reenabled.status, reenabled.body
        );
        assert_ne!(
            &reenabled.string("arn"),
            arn,
            "case {case:?}: a re-enabled pair must mint a new ARN, got the torn-down {arn:?}"
        );
    }
}

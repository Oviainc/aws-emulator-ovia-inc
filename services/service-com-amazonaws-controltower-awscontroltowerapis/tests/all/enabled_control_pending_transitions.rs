// SPDX-License-Identifier: CC0-1.0

//! The teardown an asynchronous `DisableControl` parks on its `EnabledControl`, end to end
//! over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The RULE under test: a transition an asynchronous call parks on a resource is TAKEN the
//! moment that resource's own due instant passes, so the clock decides it exactly once,
//! from the terminal status its operation carries at that instant — never from a later
//! re-reading of the bounded, self-evicting operation log. Two consequences a caller sees,
//! and both are asserted here: a teardown whose own operation FAILED removes nothing, and
//! goes on removing nothing however the log is pruned afterwards; a teardown whose record
//! the log no longer holds removes the enablement and its tags all the same. Either way the
//! enablement stops carrying a teardown in flight once the clock has settled it, so it is
//! disablable again and ending its target's governance tears it down like any other.
//!
//! Asserted over a TABLE whose rows are what the rule generalises over: WHICH control the
//! teardown is of — read off `state::CONTROL_CATALOG`, covering both implementations the
//! catalog publishes and a control enabled with a configuration as well as ones that
//! declare none — crossed with what becomes of the teardown's record (its operation FAILED
//! with the record kept, FAILED with the record then evicted, and left in flight with the
//! record evicted before it falls due).
//!
//! Two things no sequence of calls can produce are seeded straight into the store, the way
//! the sibling `landing_zone_pending_transitions` and
//! `enabled_baseline_pending_transitions` suites' cases are: a control operation that
//! FAILED (nothing in this crate fails one yet) and a record dropped out of the log while
//! the transition it started is still owed (a control carries at most one operation at a
//! time, so the wire cannot put a whole log's worth in flight at once). Everything else is
//! driven through HTTP.
//!
//! The log's retention rule is the crate's own — `state::MAX_CONTROL_OPERATIONS` records,
//! the oldest TERMINAL one evicted first and the oldest overall only when every record is
//! in flight — which is why a FAILED record, being terminal, is a prime eviction candidate.
//! The statuses and operation types written out here come from
//! `smithy_query(query="shape", shape="EnablementStatus,DriftStatus,\
//! ControlOperationStatus,ControlOperationType")` on this crate's `smithy-model/`:
//! `SUCCEEDED` | `FAILED` | `UNDER_CHANGE`, `DRIFTED` | `IN_SYNC` | `NOT_CHECKING` |
//! `UNKNOWN`, `SUCCEEDED` | `FAILED` | `IN_PROGRESS` and `ENABLE_CONTROL` |
//! `DISABLE_CONTROL` | `UPDATE_ENABLED_CONTROL` | `RESET_ENABLED_CONTROL`; the refusal a
//! torn-down identifier earns from `smithy_query(query="operation",
//! operation="GetEnabledControl,ListTagsForResource")`, which model
//! `ResourceNotFoundException` (404).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    service_state, CatalogControl, ControlOperationStatus, ControlOperationType,
    CONTROL_CATALOG, MAX_CONTROL_OPERATIONS,
};

use crate::enabled_control_lifecycle::{
    configuration, control, drift_dimensions, enable_with, entries, last_operation, member, ou,
    status_summary_status,
};
use crate::governed_target_mandatory_controls::{
    baseline, enable_baseline, governance_baseline, list, listed_controls, mandatory,
};
use crate::in_flight_teardown_drift::{disable_control, get_enabled_control};
use crate::landing_zone_lifecycle::{
    call, emulator_with_engine, past_the_settle_span, set_up_account, Answer,
};
use crate::resource_tags::{apply_tags, read_tags, reported_tags, tags};

/// The identifiers the records a case appends to the log directly carry: a control, a
/// target and an enablement ARN of the forms this service handles, under which nothing was
/// ever enabled, so a seeded record names no resource a case asserts about.
const SEEDED_CONTROL: &str = "arn:aws:controltower:us-west-2::control/AWS-GR_LOG_RETENTION";
const SEEDED_TARGET: &str = "arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-ret0";
const SEEDED_ARN: &str =
    "arn:aws:controltower:us-west-2:000000000000:enabledcontrol/log-retention";

/// What becomes of the record of the teardown a row starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// The operation FAILED and its record stays in the log.
    FailedKept,
    /// The operation FAILED and its record is pushed out of the bounded log AFTER the clock
    /// has made the teardown due — the eviction a terminal record is first in line for, and
    /// the one that must not re-open a teardown the clock has already decided.
    FailedThenEvicted,
    /// The operation is left to succeed and its record is evicted while the teardown it
    /// started is still owed.
    EvictedInFlight,
}

impl Fate {
    /// Whether the teardown this fate describes is one the clock owes a removal for: a
    /// teardown whose own operation had failed by the time it fell due removed nothing.
    fn removes_the_enablement(self) -> bool {
        self == Self::EvictedInFlight
    }
}

/// One row of the teardown table.
struct Row {
    case: String,
    entry: &'static CatalogControl,
    target: String,
    /// The `parameters` member `EnableControl` is given; `None` omits it.
    parameters: Option<serde_json::Value>,
    fate: Fate,
}

/// The controls the rows enable: a caller-enablable control of each implementation the
/// catalog publishes, plus the one it publishes WITH enablement parameters, so the rule is
/// asserted over a control carrying a configuration as well as over ones that declare none.
fn subject_controls() -> Vec<&'static CatalogControl> {
    let enablable: Vec<&'static CatalogControl> =
        CONTROL_CATALOG.iter().filter(|entry| entry.enablable).collect();
    let config_rule = enablable
        .iter()
        .copied()
        .find(|entry| !entry.implemented_with_service_control_policy() && entry.parameters.is_empty())
        .expect("the catalog must publish an enablable Config-rule control with no parameters");
    let policy = enablable
        .iter()
        .copied()
        .find(|entry| entry.implemented_with_service_control_policy() && entry.parameters.is_empty())
        .expect("the catalog must publish an enablable policy control with no parameters");
    let parameterized = enablable
        .iter()
        .copied()
        .find(|entry| !entry.parameters.is_empty())
        .expect("the catalog must publish an enablable control that declares parameters");
    vec![config_rule, policy, parameterized]
}

/// The teardown table: every subject control crossed with every fate its teardown's record
/// can meet. Each row is its own control/target pair, because a control is enabled at most
/// once per target.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for entry in subject_controls() {
        // A `parameters` list names only keys the control declares, so a control that
        // declares none is enabled with no `parameters` member at all.
        let parameters = entry.parameters.first().map(|key| {
            serde_json::json!([{ "key": key, "value": ["us-west-2", "eu-west-1"] }])
        });
        for fate in [
            Fate::FailedKept,
            Fate::FailedThenEvicted,
            Fate::EvictedInFlight,
        ] {
            let index = rows.len();
            rows.push(Row {
                case: format!("{} with its teardown record {fate:?}", entry.name),
                entry,
                target: ou(&format!("pt{index:02}")),
                parameters: parameters.clone(),
                fate,
            });
        }
    }
    rows
}

/// The `/get-control-operation` answer for `identifier`, whatever it is.
async fn get_control_operation(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "/get-control-operation",
        serde_json::json!({ "operationIdentifier": identifier }),
    )
    .await
}

/// Fail the control operation `identifier` names, at the instant the clock owes it its
/// terminal status.
///
/// Nothing in this crate fails a control operation yet, so a row whose subject is what the
/// clock does with a FAILED operation's parked teardown seeds the failure into the log, the
/// way the sibling landing-zone suite does.
fn fail_the_operation(engine: &engine::InMemoryEngine, identifier: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let record = store
        .control_operations
        .get_mut(identifier)
        .expect("the record of the operation the case started");
    record.status = ControlOperationStatus::Failed;
    record.end_time = Some(record.due_at);
}

/// Drop `identifier` out of the control operation log by appending records until the log's
/// retention bound has pushed it out.
///
/// The seeded records start in the FUTURE, so they are in flight however long the case
/// takes and the bound never prefers one of them: what it evicts is the oldest TERMINAL
/// record, and then — once nothing is terminal — the oldest in-flight one. Either way the
/// record under test goes. One record is appended at a time until it is gone, so the case
/// does not depend on how many records the calls before it left in the log.
fn evict_from_the_log(engine: &engine::InMemoryEngine, identifier: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let base = SystemTime::now() + Duration::from_secs(60);
    let cap = MAX_CONTROL_OPERATIONS + store.control_operations.len() + 8;
    let mut seeded = 0usize;
    while store.control_operations.contains_key(identifier) {
        assert!(
            seeded <= cap,
            "seeding {seeded} records did not push {identifier} out of a log bounded at \
             {MAX_CONTROL_OPERATIONS}"
        );
        store.record_control_operation(
            ControlOperationType::EnableControl,
            SEEDED_CONTROL,
            SEEDED_TARGET,
            SEEDED_ARN,
            base + Duration::from_millis(seeded as u64),
        );
        seeded += 1;
    }
}

/// The `enabledControlDetails` of an answer that must have resolved.
fn details_of(case: &str, answer: &Answer) -> serde_json::Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: get-enabled-control expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer.get("enabledControlDetails").clone()
}

/// The `lastOperationIdentifier` a listing reports for each control, keyed by
/// `controlIdentifier`, beside the status reported with it.
fn listed_operations(listed: &Answer) -> BTreeMap<String, (String, String)> {
    entries(listed.get("enabledControls"))
        .iter()
        .map(|entry| {
            (
                member(entry, "controlIdentifier").unwrap_or_default().to_string(),
                (
                    last_operation(entry).unwrap_or_default().to_string(),
                    status_summary_status(entry).unwrap_or_default().to_string(),
                ),
            )
        })
        .collect()
}

/// POST `/list-enabled-controls` for one target.
async fn list_on(router: &MultiServiceRouter, target: &str) -> Answer {
    list(
        router,
        serde_json::json!({ "targetIdentifier": target, "maxResults": 100 }),
    )
    .await
}

/// RULE: the clock TAKES the teardown parked on an enabled control at that teardown's own
/// due instant and decides it once, from the status its operation carries then — so a
/// teardown whose operation FAILED leaves the enablement, its configuration and its tags
/// alone however the bounded operation log is pruned afterwards, while one the clock owes a
/// removal for removes the enablement and its tags even though the log no longer holds its
/// record. An enablement a failed teardown left behind is a resource again: the same call,
/// left to succeed, tears it down.
#[tokio::test]
async fn a_control_teardown_is_decided_once_from_the_resource_not_from_the_log() {
    let attached = tags(&[("Owner", "platform"), ("Cost-Centre", "4711")]);
    let rows = rows();
    assert!(
        rows.len() >= 6,
        "the table must span several controls and every fate a record can meet, got {} rows",
        rows.len()
    );

    // One emulator per row, so no row's log seeding can evict another row's records.
    struct Enabled {
        row: Row,
        router: MultiServiceRouter,
        engine: Arc<engine::InMemoryEngine>,
        arn: String,
        configured: Vec<serde_json::Value>,
    }

    let mut enabled: Vec<Enabled> = Vec::new();
    for row in rows {
        let (router, engine) = emulator_with_engine();
        set_up_account(&router).await;
        let (arn, _) = enable_with(
            &router,
            &control(row.entry.name),
            &row.target,
            row.parameters.clone(),
        )
        .await;
        let tagged = apply_tags(&router, &arn, &attached).await;
        assert_eq!(
            tagged.status, 204,
            "case {:?}: tagging the enablement expected 204, got {} body {:?}",
            row.case, tagged.status, tagged.body
        );
        let configured = configuration(row.parameters.as_ref().unwrap_or(&serde_json::Value::Null));
        enabled.push(Enabled {
            row,
            router,
            engine,
            arn,
            configured,
        });
    }

    // Nothing sleeps server-side: the clock makes the enables due, and the wait is the
    // test's. One wait for the whole table, since every row's enable was started before it.
    tokio::time::sleep(past_the_settle_span()).await;

    // Each row starts its teardown, and its record meets the fate the row names.
    let mut started: Vec<(Enabled, String)> = Vec::new();
    for subject in enabled {
        let case = subject.row.case.clone();
        let deployed = details_of(&case, &get_enabled_control(&subject.router, &subject.arn).await);
        assert_eq!(
            (
                status_summary_status(&deployed),
                drift_dimensions(&deployed),
                configuration(deployed.get("parameters").unwrap_or(&serde_json::Value::Null)),
            ),
            (
                Some("SUCCEEDED"),
                (Some("IN_SYNC"), Some("IN_SYNC"), Some("IN_SYNC")),
                subject.configured.clone(),
            ),
            "case {case:?}: expected a deployed enablement before the teardown, got \
             {deployed:?}"
        );

        let disabled = disable_control(&subject.router, &subject.arn).await;
        assert_eq!(
            disabled.status, 200,
            "case {case:?}: disable-control expected 200, got {} body {:?}",
            disabled.status, disabled.body
        );
        let identifier = disabled.string("operationIdentifier");

        match subject.row.fate {
            // The two failed rows fail their operation BEFORE the teardown falls due, so the
            // status the clock reads at that instant is FAILED; what distinguishes them is
            // what happens to the record afterwards.
            Fate::FailedKept | Fate::FailedThenEvicted => {
                fail_the_operation(&subject.engine, &identifier);
                let record = get_control_operation(&subject.router, &identifier).await;
                assert_eq!(
                    (
                        record.status,
                        member(record.get("controlOperation"), "status"),
                    ),
                    (200, Some("FAILED")),
                    "case {case:?}: expected the seeded record to read FAILED, got {:?} \
                     body {:?}",
                    record.status,
                    record.body
                );
            }
            Fate::EvictedInFlight => {
                evict_from_the_log(&subject.engine, &identifier);
                let record = get_control_operation(&subject.router, &identifier).await;
                assert_eq!(
                    (record.status, record.error_code.as_deref()),
                    (404, Some("ResourceNotFoundException")),
                    "case {case:?}: expected {identifier:?} to have been evicted from the \
                     log, got {:?} body {:?}",
                    (record.status, record.error_code.as_deref()),
                    record.body
                );
            }
        }
        started.push((subject, identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    // The teardown the clock has settled: removed, or decided to remove nothing — once.
    let mut survivors: Vec<Enabled> = Vec::new();
    for (subject, identifier) in started {
        let case = subject.row.case.clone();
        let reported = get_enabled_control(&subject.router, &subject.arn).await;
        let read = read_tags(&subject.router, &subject.arn).await;
        if subject.row.fate.removes_the_enablement() {
            assert_eq!(
                (reported.status, reported.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the teardown the clock owed to have removed the \
                 enablement (404 ResourceNotFoundException), got {:?} body {:?}",
                (reported.status, reported.error_code.as_deref()),
                reported.body
            );
            assert_eq!(
                (read.status, read.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected the tags of a torn-down enablement to be gone (404 \
                 ResourceNotFoundException), got {:?} tags {:?}",
                (read.status, read.error_code.as_deref()),
                reported_tags(&read)
            );
            continue;
        }

        let kept = details_of(&case, &reported);
        assert_eq!(
            (
                status_summary_status(&kept),
                last_operation(&kept),
                configuration(kept.get("parameters").unwrap_or(&serde_json::Value::Null)),
            ),
            (
                Some("SUCCEEDED"),
                Some(identifier.as_str()),
                subject.configured.clone(),
            ),
            "case {case:?}: expected the failed teardown to leave the enablement and its \
             configuration alone, got {kept:?}"
        );
        assert_eq!(
            (read.status, reported_tags(&read)),
            (200, attached.clone()),
            "case {case:?}: expected the failed teardown to leave the tags alone, got {:?} \
             tags {:?}",
            read.status,
            reported_tags(&read)
        );

        // The decision is the clock's, taken once: pruning the log afterwards cannot
        // re-open it. The FAILED record is terminal, so it is the first the bound drops.
        if subject.row.fate == Fate::FailedThenEvicted {
            evict_from_the_log(&subject.engine, &identifier);
            let record = get_control_operation(&subject.router, &identifier).await;
            assert_eq!(
                (record.status, record.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected {identifier:?} to have been evicted from the log, \
                 got {:?} body {:?}",
                (record.status, record.error_code.as_deref()),
                record.body
            );
            // Two reads, because each call settles the store: an enablement a teardown
            // decided to leave must survive every later settle, not just the next one.
            for read_number in 1..=2 {
                let survived = get_enabled_control(&subject.router, &subject.arn).await;
                let detail = details_of(&case, &survived);
                assert_eq!(
                    (
                        status_summary_status(&detail),
                        configuration(
                            detail.get("parameters").unwrap_or(&serde_json::Value::Null)
                        ),
                    ),
                    (Some("SUCCEEDED"), subject.configured.clone()),
                    "case {case:?}: read {read_number} after the record was evicted: a \
                     teardown already decided to remove nothing must not remove the \
                     enablement, got {detail:?}"
                );
            }
            let still_tagged = read_tags(&subject.router, &subject.arn).await;
            assert_eq!(
                (still_tagged.status, reported_tags(&still_tagged)),
                (200, attached.clone()),
                "case {case:?}: expected the tags to outlive the evicted record, got {:?} \
                 tags {:?}",
                still_tagged.status,
                reported_tags(&still_tagged)
            );
        }
        survivors.push(subject);
    }
    assert!(
        !survivors.is_empty(),
        "the table must leave at least one enablement a failed teardown did not remove"
    );

    // Not vacuous: the SAME call, with its operation left to succeed, tears down exactly
    // what the failed one did not — so a failed teardown leaves a resource rather than one
    // stuck mid-teardown.
    let mut repeated: Vec<(Enabled, String)> = Vec::new();
    for subject in survivors {
        let case = subject.row.case.clone();
        let again = disable_control(&subject.router, &subject.arn).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: disabling the enablement the failed teardown left expected 200, \
             got {} body {:?}",
            again.status, again.body
        );
        let identifier = again.string("operationIdentifier");
        let under_change = details_of(
            &case,
            &get_enabled_control(&subject.router, &subject.arn).await,
        );
        assert_eq!(
            (
                status_summary_status(&under_change),
                last_operation(&under_change),
            ),
            (Some("UNDER_CHANGE"), Some(identifier.as_str())),
            "case {case:?}: expected the repeated teardown to put the enablement \
             UNDER_CHANGE under its own operation, got {under_change:?}"
        );
        repeated.push((subject, identifier));
    }

    tokio::time::sleep(past_the_settle_span()).await;

    for (subject, identifier) in repeated {
        let case = subject.row.case.clone();
        let gone = get_enabled_control(&subject.router, &subject.arn).await;
        assert_eq!(
            (gone.status, gone.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the repeated teardown {identifier:?} to remove the \
             enablement (404 ResourceNotFoundException), got {:?} body {:?}",
            (gone.status, gone.error_code.as_deref()),
            gone.body
        );
        let read = read_tags(&subject.router, &subject.arn).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the tags to go with the enablement (404 \
             ResourceNotFoundException), got {:?} tags {:?}",
            (read.status, read.error_code.as_deref()),
            reported_tags(&read)
        );
    }
}

/// RULE (the same rule, through the service's OWN teardowns): because the clock takes a
/// control's parked teardown when it falls due, a control whose teardown failed carries
/// none afterwards — so the next withdrawal of its target's governance starts a fresh
/// teardown against it, exactly as the first one did, and the service-applied controls of an
/// ungoverned target are not left behind for the life of the process.
#[tokio::test]
async fn a_mandatory_control_whose_teardown_failed_is_torn_down_when_governance_ends_again() {
    let (router, engine) = emulator_with_engine();
    set_up_account(&router).await;
    let governance = governance_baseline();
    let version = governance.versions[0];
    let target = ou("gw01");
    // The controls AWS Control Tower applies to a target it governs itself, read off the
    // catalog rather than written out here.
    let mut applied: Vec<String> = mandatory()
        .into_iter()
        .map(|entry| control(entry.name))
        .collect();
    applied.sort();

    let enablement = enable_baseline(&router, &baseline(governance.id), version, &target).await;
    tokio::time::sleep(past_the_settle_span()).await;
    assert_eq!(
        listed_controls(&list_on(&router, &target).await),
        applied,
        "governing {target} must apply every mandatory catalog control to it"
    );

    // Governance ends: the service starts a teardown against each control it applied, and
    // every one of those operations FAILS.
    let withdrawn = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": enablement }),
    )
    .await;
    assert_eq!(
        withdrawn.status, 200,
        "disable-baseline expected 200, got {} body {:?}",
        withdrawn.status, withdrawn.body
    );
    let teardowns = listed_operations(&list_on(&router, &target).await);
    for identifier in &applied {
        let (operation, status) = teardowns
            .get(identifier)
            .unwrap_or_else(|| panic!("the listing must still report {identifier}"));
        assert_eq!(
            status, "UNDER_CHANGE",
            "control {identifier}: expected UNDER_CHANGE while its teardown is in flight, \
             got {status:?} under operation {operation:?}"
        );
        fail_the_operation(&engine, operation);
    }

    tokio::time::sleep(past_the_settle_span()).await;

    assert_eq!(
        listed_controls(&list_on(&router, &target).await),
        applied,
        "a teardown whose operation FAILED removed nothing, so every control the service \
         applied to {target} must still be reported"
    );

    // Governance is established again on the same target and then ended again. The controls
    // the first, failed withdrawal left behind must be torn down by this one.
    let re_enabled = enable_baseline(&router, &baseline(governance.id), version, &target).await;
    tokio::time::sleep(past_the_settle_span()).await;
    let withdrawn_again = call(
        &router,
        "/disable-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": re_enabled }),
    )
    .await;
    assert_eq!(
        withdrawn_again.status, 200,
        "the second disable-baseline expected 200, got {} body {:?}",
        withdrawn_again.status, withdrawn_again.body
    );

    let second = listed_operations(&list_on(&router, &target).await);
    for identifier in &applied {
        let (operation, status) = second
            .get(identifier)
            .unwrap_or_else(|| panic!("the listing must still report {identifier}"));
        let (failed, _) = teardowns
            .get(identifier)
            .unwrap_or_else(|| panic!("the first withdrawal must have reported {identifier}"));
        assert_eq!(
            status, "UNDER_CHANGE",
            "control {identifier}: expected the second withdrawal to start a teardown \
             against it, got {status:?} under operation {operation:?}"
        );
        assert_ne!(
            operation, failed,
            "control {identifier}: expected a FRESH teardown operation rather than the \
             failed {failed:?}"
        );
    }

    tokio::time::sleep(past_the_settle_span()).await;

    assert_eq!(
        listed_controls(&list_on(&router, &target).await),
        Vec::<String>::new(),
        "ending {target}'s governance again must leave none of the controls the service \
         applied to it"
    );
}

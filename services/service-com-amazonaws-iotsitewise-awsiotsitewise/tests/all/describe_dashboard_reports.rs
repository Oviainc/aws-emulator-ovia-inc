// SPDX-License-Identifier: CC0-1.0

//! `DescribeDashboard` is a REPORT, pinned over a table of stored dashboards and
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule this suite pins: a reporting read answers from the persisted record
//! the request addresses and writes nothing, so every member it reports is the
//! one the store holds at the instant of the call — including a record the store
//! gained or lost without any request of its own — repeated reads agree, reading
//! one dashboard cannot change what another reports, the answer carries only the
//! members the operation models, and an id resolving to no record is a modelled
//! refusal that leaves the store as it was.
//!
//! `dashboard_lifecycle` pins the create -> read -> update round trip, the
//! modelled bounds, the dashboard ceiling and the create's `clientToken`
//! idempotency; what is pinned HERE is that the read FOLLOWS THE STORE rather
//! than replaying what a create answered. It reuses that suite's request helpers
//! and the asset suite's HTTP harness rather than growing a second copy of
//! either.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    dashboard_arn, service_state, DashboardRecord,
};

use crate::asset_lifecycle::{error_code, router_with_engine, REGION};
use crate::dashboard_lifecycle::describe_dashboard;

/// Every member `DescribeDashboard` models in its output. Source:
/// `smithy_query(query="operation", operation="DescribeDashboard")` —
/// `dashboardId`, `dashboardArn`, `dashboardName`, `projectId`,
/// `dashboardDescription`, `dashboardDefinition`, `dashboardCreationDate`,
/// `dashboardLastUpdateDate`. Nothing else may appear on the wire, so neither a
/// dashboard's tags nor the `clientToken` its create carried may leak out of a
/// read.
const MODELLED_OUTPUT_MEMBERS: [&str; 8] = [
    "dashboardId",
    "dashboardArn",
    "dashboardName",
    "projectId",
    "dashboardDescription",
    "dashboardDefinition",
    "dashboardCreationDate",
    "dashboardLastUpdateDate",
];

/// The published maximum of `dashboardName`: len 1..256. Source:
/// `smithy_query(query="operation", operation="DescribeDashboard")` —
/// `dashboardName` targets `Name`.
const NAME_MAX: usize = 256;

/// A modelled `ID` built from `seed`: a lowercase hyphenated UUID that is not the
/// all-zero one the pattern refuses.
fn id_from(seed: u32) -> String {
    format!("{seed:08x}-1c2d-4e5f-8a9b-0c1d2e3f4a5b")
}

/// A wall-clock instant `secs` seconds after the epoch. Whole seconds on
/// purpose: the wire form of a `Timestamp` is epoch seconds, so a row's expected
/// instant is exact rather than rounded.
fn at_epoch(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// One row of the reporting table: a dashboard record the store holds, and the
/// label the assertions name it by.
#[derive(Debug, Clone)]
struct ReadCase {
    label: &'static str,
    record: DashboardRecord,
}

/// A definition literal of `length` characters, in the JSON form a Monitor
/// dashboard definition takes. `DashboardDefinition` is len 0..204800 with
/// pattern `^.+$`, so the shortest stored literal is one character.
fn definition_of(length: usize) -> String {
    let envelope = r#"{"widgets":[{"title":""}]}"#;
    let padding = length.saturating_sub(envelope.len());
    format!(r#"{{"widgets":[{{"title":"{}"}}]}}"#, "w".repeat(padding))
}

/// The rows: stored dashboards differing along everything a read reports — the
/// project they live in (two rows share one, so a read cannot be answering from
/// "the only dashboard in that project"), the form and length of the name, a
/// description present and absent, the size and form of the definition literal,
/// and a creation date equal to and earlier than the last-update date. Tags and
/// a spent `clientToken` ride along on some rows, because neither is a member of
/// this operation's output.
fn read_cases() -> Vec<ReadCase> {
    let project_a = id_from(0xa1);
    let project_b = id_from(0xb2);
    let same_instant = at_epoch(1_700_000_000);

    let rows: Vec<(&'static str, DashboardRecord)> = vec![
        (
            "required members only, shortest name and definition",
            DashboardRecord {
                id: id_from(1),
                arn: dashboard_arn(REGION, &id_from(1)),
                project_id: project_a.clone(),
                name: "D".to_string(),
                description: None,
                definition: "{".to_string(),
                creation_date: same_instant,
                last_update_date: same_instant,
                client_token: None,
                tags: HashMap::new(),
            },
        ),
        (
            "described dashboard, second in the same project, updated later",
            DashboardRecord {
                id: id_from(2),
                arn: dashboard_arn(REGION, &id_from(2)),
                project_id: project_a.clone(),
                name: "Line 3 — overview (v1.2)".to_string(),
                description: Some("the shop floor roll-up".to_string()),
                definition: definition_of(512),
                creation_date: at_epoch(1_700_000_001),
                last_update_date: at_epoch(1_700_086_401),
                client_token: Some("a-spent-create-token-0000000000000000".to_string()),
                tags: HashMap::from([("tier".to_string(), "gold".to_string())]),
            },
        ),
        (
            "name at its published maximum, another project, large definition",
            DashboardRecord {
                id: id_from(3),
                arn: dashboard_arn(REGION, &id_from(3)),
                project_id: project_b.clone(),
                name: "n".repeat(NAME_MAX),
                description: Some("d".repeat(2048)),
                definition: definition_of(100_000),
                creation_date: at_epoch(1_600_000_000),
                last_update_date: at_epoch(1_699_999_999),
                client_token: None,
                tags: HashMap::from([
                    ("env".to_string(), "test".to_string()),
                    ("owner".to_string(), "ops".to_string()),
                ]),
            },
        ),
        (
            "a definition that is not JSON at all, stored as the literal it is",
            DashboardRecord {
                id: id_from(4),
                arn: dashboard_arn(REGION, &id_from(4)),
                project_id: project_b,
                name: "not-json".to_string(),
                description: None,
                definition: "widgets: [one, two]".to_string(),
                creation_date: at_epoch(1_500_000_000),
                last_update_date: at_epoch(1_500_000_000),
                client_token: None,
                tags: HashMap::new(),
            },
        ),
    ];

    rows.into_iter()
        .map(|(label, record)| ReadCase { label, record })
        .collect()
}

/// Put `record` in the shared store, as the only writer of the dashboard map
/// would have left it.
fn put(shared: &engine::InMemoryEngine, record: &DashboardRecord) {
    let state = service_state(shared).expect("the IoT SiteWise store");
    state
        .lock()
        .dashboards
        .insert(record.id.clone(), record.clone());
}

/// Take `id` out of the shared store, answering whether it was there.
fn take(shared: &engine::InMemoryEngine, id: &str) -> bool {
    let state = service_state(shared).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store.dashboards.remove(id).is_some()
}

/// Every stored dashboard, rendered as a sorted, comparable list: what a read
/// must leave exactly as it found it. Spells the two instants out too, so a read
/// that re-stamped a record shows up here.
fn store_snapshot(shared: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(shared).expect("the IoT SiteWise store");
    let store = state.lock();
    let mut rows: Vec<String> = store
        .dashboards
        .values()
        .map(|record| {
            let mut tags: Vec<String> = record
                .tags
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            tags.sort();
            format!(
                "{id}|{arn}|{project}|{name}|{description:?}|{definition}|{created:?}|\
                 {updated:?}|{token:?}|{tags}",
                id = record.id,
                arn = record.arn,
                project = record.project_id,
                name = record.name,
                description = record.description,
                definition = record.definition,
                created = record.creation_date,
                updated = record.last_update_date,
                token = record.client_token,
                tags = tags.join(","),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Epoch seconds of `at`, the wire form of a modelled `Timestamp`.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant at or after the epoch")
        .as_secs_f64()
}

/// Assert `described` is the report of `record`, member by member, and that it
/// carries nothing the operation does not model.
fn assert_reports(label: &str, described: &Value, record: &DashboardRecord) {
    assert_eq!(
        (
            described["dashboardId"].as_str().unwrap_or_default(),
            described["dashboardArn"].as_str().unwrap_or_default(),
            described["projectId"].as_str().unwrap_or_default(),
            described["dashboardName"].as_str().unwrap_or_default(),
            described["dashboardDefinition"].as_str().unwrap_or_default(),
        ),
        (
            record.id.as_str(),
            record.arn.as_str(),
            record.project_id.as_str(),
            record.name.as_str(),
            record.definition.as_str(),
        ),
        "case {label:?}: the report must be the stored record, got {described}"
    );
    assert_eq!(
        described["dashboardDescription"].as_str(),
        record.description.as_deref(),
        "case {label:?}: the reported description must be the stored one, got {:?}",
        described["dashboardDescription"]
    );
    assert_eq!(
        (
            described["dashboardCreationDate"].as_f64(),
            described["dashboardLastUpdateDate"].as_f64(),
        ),
        (
            Some(epoch_secs(record.creation_date)),
            Some(epoch_secs(record.last_update_date)),
        ),
        "case {label:?}: the reported instants must be the stored ones, got {described}"
    );

    let modelled: HashSet<&str> = MODELLED_OUTPUT_MEMBERS.into_iter().collect();
    let reported: Vec<String> = described
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    let unmodelled: Vec<&String> = reported
        .iter()
        .filter(|member| !modelled.contains(member.as_str()))
        .collect();
    assert!(
        unmodelled.is_empty(),
        "case {label:?}: the report carries members this operation does not model: \
         {unmodelled:?}"
    );
}

#[tokio::test]
async fn describe_reports_the_stored_record_for_every_stored_shape() {
    let (router, shared) = router_with_engine();
    let cases = read_cases();
    for case in &cases {
        put(&shared, &case.record);
    }

    let before = store_snapshot(&shared);
    assert_eq!(
        before.len(),
        cases.len(),
        "expected {} stored dashboards before any read, got {}",
        cases.len(),
        before.len()
    );

    // First pass: each report must be the stored record.
    let mut first: Vec<Value> = Vec::new();
    for case in &cases {
        let (status, described) = describe_dashboard(&router, &case.record.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeDashboard expected 200, got {status} {described}",
            case.label
        );
        assert_reports(case.label, &described, &case.record);
        first.push(described);
    }

    // Second pass: the same reads in reverse order, several rounds. A report is
    // repeatable, and reading one dashboard cannot change what another reports.
    for round in 0..3 {
        for (at, case) in cases.iter().enumerate().rev() {
            let (status, again) = describe_dashboard(&router, &case.record.id).await;
            assert_eq!(
                status, 200,
                "case {:?}: round {round} expected 200, got {status} {again}",
                case.label
            );
            assert_eq!(
                again, first[at],
                "case {:?}: round {round} must report exactly what the first read did, \
                 got {again}",
                case.label
            );
        }
    }

    let after = store_snapshot(&shared);
    assert_eq!(
        after, before,
        "reads must change nothing a later caller sees, got {after:?}"
    );
}

#[tokio::test]
async fn describe_follows_the_store_rather_than_the_answer_a_read_gave_before() {
    let (router, shared) = router_with_engine();
    let cases = read_cases();
    for case in &cases {
        put(&shared, &case.record);
    }

    // Each row is rewritten in the store to the NEXT row's members — keeping its
    // own identity and provenance — so a read that replayed anything other than
    // the record it addresses reports the wrong one.
    let rewritten: Vec<DashboardRecord> = cases
        .iter()
        .enumerate()
        .map(|(at, case)| {
            let source = &cases[(at + 1) % cases.len()].record;
            DashboardRecord {
                name: source.name.clone(),
                description: source.description.clone(),
                definition: source.definition.clone(),
                last_update_date: source.last_update_date + Duration::from_secs(60),
                ..case.record.clone()
            }
        })
        .collect();

    for (case, record) in cases.iter().zip(&rewritten) {
        let (status, before) = describe_dashboard(&router, &case.record.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read before the rewrite expected 200, got {status} {before}",
            case.label
        );
        assert_reports(case.label, &before, &case.record);

        put(&shared, record);
        let (status, after) = describe_dashboard(&router, &case.record.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read after the rewrite expected 200, got {status} {after}",
            case.label
        );
        assert_reports(case.label, &after, record);
    }
}

#[tokio::test]
async fn an_id_the_store_does_not_hold_refuses_and_changes_nothing() {
    let (router, shared) = router_with_engine();
    let cases = read_cases();
    for case in &cases {
        put(&shared, &case.record);
    }

    let served_case = &cases[1];
    let (status, served) = describe_dashboard(&router, &served_case.record.id).await;
    assert_eq!(status, 200, "the served read got {status} {served}");

    // A record the store has LOST is a not-found refusal from the next read on,
    // because the read resolves the id against the store every time.
    let gone = cases[0].record.id.clone();
    assert!(
        take(&shared, &gone),
        "the store must have held the dashboard being removed"
    );

    let before = store_snapshot(&shared);
    for (label, requested) in [
        ("an id no dashboard was ever stored under", id_from(0xdead)),
        ("the id of a dashboard the store has lost", gone.clone()),
        ("a project's id, which is not a dashboard's", cases[1].record.project_id.clone()),
    ] {
        let (status, refused) = describe_dashboard(&router, &requested).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected a modelled not-found refusal, got {status} {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Dashboard with ID {requested} not found"),
            "case {label:?}: unexpected refusal message in {refused}"
        );
    }

    let after = store_snapshot(&shared);
    assert_eq!(
        after, before,
        "a refused read must change nothing a later caller sees, got {after:?}"
    );
    // And the dashboard the store does hold still reports exactly what it did
    // before those refusals.
    let (status, again) = describe_dashboard(&router, &served_case.record.id).await;
    assert_eq!(
        (status, &again),
        (200, &served),
        "the served read must be unaffected by the refused ones, got {status} {again}"
    );
}

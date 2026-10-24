// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListExecutions`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `ListExecutions` reports, from the shared store, the
//! executions recorded against the ONE target resource the request names, narrowed
//! by the `actionType` and `resolveTo` filters it carries, in a stable order its
//! `nextToken` resumes, each execution reporting its run as the clock has it —
//! still running until its run is due to end, its outcome and end time from then
//! on; an identifier that names no stored resource is a modelled refusal rather
//! than an empty page, and no listing changes what a later caller sees.
//!
//! `ExecuteAction` is the operation that records an execution and is still a
//! scaffold stub (its own issue,
//! `20261024T100803Z-iotsitewise-executeaction-stub.md`), so these suites record
//! the executions directly in the shared store this handler reads — the same store
//! a handler writes through, which is what `time_series` does for the alias
//! binding its own stub writer would make.
//!
//! The wire harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` already defines, re-used rather than copied.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ResolveTo, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ExecutionRecord, EXECUTION_STATE_COMPLETED, EXECUTION_STATE_FAILED,
    EXECUTION_STATE_RUNNING,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_catalog, router_with_engine,
};

/// Which seeded resource an execution is recorded against, or resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The first asset the fixture creates.
    AssetA,
    /// The second asset the fixture creates.
    AssetB,
    /// A computation model: `CreateComputationModel` is a scaffold stub, so no
    /// computation model is stored and this id names one that cannot exist.
    ComputationModel,
}

/// One row of the seeded execution table.
#[derive(Debug, Clone)]
struct ExecutionRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The id the execution is stored and reported under.
    id: &'static str,
    /// The resource the action was taken on.
    target: Target,
    /// The type of action executed, which the `actionType` filter matches.
    action_type: Option<&'static str>,
    /// How many seconds before the suite's anchor the execution started.
    started_ago: u64,
    /// How many seconds from the anchor the run is due to end: negative for a run
    /// already over, positive for one still in flight.
    settles_in: i64,
    /// The terminal state the run settles into.
    outcome: &'static str,
    /// The detailed resource the execution resolves to, when it resolves to one.
    resolve_to: Option<Target>,
    /// The version of the target resource the action ran against.
    target_resource_version: &'static str,
    /// The execution entity version, when the execution carries one.
    entity_version: Option<&'static str>,
}

/// An `actionType` at the published upper bound of its length: the member is a
/// `Name`, `len 1..256`. Source: `smithy_query(query="operation",
/// operation="ListExecutions")` — `actionType (Name, len 1..256, pattern
/// ^[^\u0000-\u001F\u007F]+$)`, a free string rather than an enum, so these rows
/// span the forms that pattern admits instead of a published catalog.
fn longest_action_type() -> String {
    "l".repeat(256)
}

/// The seeded executions: one table spanning the dimensions the listing rule
/// generalises over — which resource the execution targets, whether it carries an
/// `actionType` and in which of the forms the member admits, whether it resolves
/// to a detailed resource and which one, when it started (including two that
/// started in the SAME instant, whose order the id has to break), whether its run
/// is over, and which terminal state it settles into.
fn execution_table() -> Vec<ExecutionRow> {
    vec![
        ExecutionRow {
            case: "oldest on A, settled, completed, shortest action type",
            id: "aaaaaaa1-1111-4111-8111-111111111111",
            target: Target::AssetA,
            // The published lower bound of the `Name` length, 1.
            action_type: Some("a"),
            started_ago: 900,
            settles_in: -880,
            outcome: EXECUTION_STATE_COMPLETED,
            resolve_to: None,
            target_resource_version: "1",
            entity_version: None,
        },
        ExecutionRow {
            case: "A, settled, failed, longest action type, resolves to B",
            id: "aaaaaaa2-2222-4222-8222-222222222222",
            target: Target::AssetA,
            action_type: None, // replaced by `longest_action_type()` when seeded
            started_ago: 800,
            settles_in: -700,
            outcome: EXECUTION_STATE_FAILED,
            resolve_to: Some(Target::AssetB),
            target_resource_version: "2",
            entity_version: Some("3"),
        },
        ExecutionRow {
            case: "A, same start instant as the next row, non-ASCII action type",
            id: "aaaaaaa3-3333-4333-8333-333333333333",
            target: Target::AssetA,
            action_type: Some("ação-ünïcode"),
            started_ago: 600,
            settles_in: -500,
            outcome: EXECUTION_STATE_COMPLETED,
            resolve_to: Some(Target::AssetA),
            target_resource_version: "4",
            entity_version: None,
        },
        ExecutionRow {
            case: "A, same start instant as the previous row, no action type",
            id: "aaaaaaa4-4444-4444-8444-444444444444",
            target: Target::AssetA,
            action_type: None,
            started_ago: 600,
            settles_in: -500,
            outcome: EXECUTION_STATE_COMPLETED,
            resolve_to: Some(Target::AssetB),
            target_resource_version: "4",
            entity_version: Some("10"),
        },
        ExecutionRow {
            case: "A, still running",
            id: "aaaaaaa5-5555-4555-8555-555555555555",
            target: Target::AssetA,
            action_type: Some("acknowledge"),
            started_ago: 5,
            settles_in: 600,
            outcome: EXECUTION_STATE_COMPLETED,
            resolve_to: Some(Target::AssetA),
            target_resource_version: "9",
            entity_version: Some("7"),
        },
        ExecutionRow {
            case: "B, settled",
            id: "bbbbbbb1-1111-4111-8111-111111111111",
            target: Target::AssetB,
            action_type: Some("acknowledge"),
            started_ago: 400,
            settles_in: -390,
            outcome: EXECUTION_STATE_COMPLETED,
            resolve_to: Some(Target::AssetB),
            target_resource_version: "1",
            entity_version: None,
        },
        ExecutionRow {
            case: "recorded against a computation model, not an asset",
            id: "ccccccc1-1111-4111-8111-111111111111",
            target: Target::ComputationModel,
            action_type: Some("acknowledge"),
            started_ago: 300,
            settles_in: -200,
            outcome: EXECUTION_STATE_FAILED,
            resolve_to: Some(Target::AssetA),
            target_resource_version: "5",
            entity_version: Some("2"),
        },
    ]
}

/// The row whose `action_type` the table leaves for [`longest_action_type`].
const LONGEST_ACTION_ROW: &str = "aaaaaaa2-2222-4222-8222-222222222222";

/// A well-formed `ID` no stored resource carries. The modelled pattern refuses the
/// all-zero UUID, so a lookup has to reach the store to find this absent.
const ABSENT_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The `targetResourceId` of the computation-model row: a well-formed `ID` that
/// names a computation model, which no store holds while
/// `CreateComputationModel` is a scaffold stub.
const COMPUTATION_MODEL_ID: &str = "77777777-2222-4333-8444-555555555555";

/// The fixture: a router over a store holding two assets and the seeded
/// executions, plus what the suites need to address them.
struct Fixture {
    router: MultiServiceRouter,
    asset_a: String,
    asset_b: String,
    /// The `targetResourceId` of the computation-model row.
    computation_model: String,
    /// The anchor every seeded instant is measured from.
    anchor: SystemTime,
    rows: Vec<ExecutionRow>,
}

impl Fixture {
    /// The stored id of a seeded target.
    fn id_of(&self, target: Target) -> &str {
        match target {
            Target::AssetA => &self.asset_a,
            Target::AssetB => &self.asset_b,
            Target::ComputationModel => &self.computation_model,
        }
    }

    /// The `actionType` a row is seeded with.
    fn action_type_of(&self, row: &ExecutionRow) -> Option<String> {
        if row.id == LONGEST_ACTION_ROW {
            Some(longest_action_type())
        } else {
            row.action_type.map(str::to_string)
        }
    }

    /// When a row's execution started, as the epoch seconds the wire carries.
    fn start_of(&self, row: &ExecutionRow) -> i64 {
        epoch_seconds(self.anchor) - row.started_ago as i64
    }

    /// When a row's run is due to end, as the epoch seconds the wire carries.
    fn settles_at_of(&self, row: &ExecutionRow) -> i64 {
        epoch_seconds(self.anchor) + row.settles_in
    }

    /// The rows targeting `target`, in the order the listing must report them:
    /// oldest first, ties broken by execution id.
    fn ordered_rows(&self, target: Target) -> Vec<ExecutionRow> {
        let mut rows: Vec<ExecutionRow> = self
            .rows
            .iter()
            .filter(|row| row.target == target)
            .cloned()
            .collect();
        rows.sort_by(|left, right| {
            self.start_of(left)
                .cmp(&self.start_of(right))
                .then_with(|| left.id.cmp(right.id))
        });
        rows
    }
}

/// `at` as whole epoch seconds.
fn epoch_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the Unix epoch")
        .as_secs() as i64
}

/// Build the fixture: two assets created through the API, then the execution table
/// recorded in the shared store.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_a = create_asset(&router, "Boiler-A", &model).await;
    let asset_b = create_asset(&router, "Boiler-B", &model).await;
    let anchor = SystemTime::now();
    let rows = execution_table();

    let fixture = Fixture {
        router,
        asset_a,
        asset_b,
        computation_model: COMPUTATION_MODEL_ID.to_string(),
        anchor,
        rows,
    };

    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for row in &fixture.rows {
            let target_id = fixture.id_of(row.target).to_string();
            let target_resource = match row.target {
                Target::ComputationModel => TargetResource {
                    asset_id: None,
                    computation_model_id: Some(target_id),
                },
                _ => TargetResource {
                    asset_id: Some(target_id),
                    computation_model_id: None,
                },
            };
            store.executions.insert(
                row.id.to_string(),
                ExecutionRecord {
                    execution_id: row.id.to_string(),
                    action_type: fixture.action_type_of(row),
                    target_resource,
                    target_resource_version: row.target_resource_version.to_string(),
                    resolve_to: row.resolve_to.map(|resolved| ResolveTo {
                        asset_id: fixture.id_of(resolved).to_string(),
                    }),
                    execution_start_time: anchor - Duration::from_secs(row.started_ago),
                    settles_at: shifted(anchor, row.settles_in),
                    outcome: row.outcome.to_string(),
                    execution_entity_version: row.entity_version.map(str::to_string),
                },
            );
        }
    }

    fixture
}

/// `anchor` moved by `seconds`, which may be negative.
fn shifted(anchor: SystemTime, seconds: i64) -> SystemTime {
    if seconds < 0 {
        anchor - Duration::from_secs(seconds.unsigned_abs())
    } else {
        anchor + Duration::from_secs(seconds as u64)
    }
}

/// Dispatch one `ListExecutions` with `query` as its query string.
async fn list_executions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/executions?{query}"), None).await
}

/// The query string that lists the executions of one asset.
fn asset_query(asset_id: &str) -> String {
    format!("targetResourceType=ASSET&targetResourceId={asset_id}")
}

/// The `executionId`s of a listing body, in the order it reported them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["executionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an executionSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            summary["executionId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no executionId in {body}"))
                .to_string()
        })
        .collect()
}

/// Assert one reported summary carries exactly what its row was seeded with.
fn assert_summary(fixture: &Fixture, row: &ExecutionRow, summary: &Value) {
    let case = row.case;
    assert_eq!(
        summary["executionId"].as_str(),
        Some(row.id),
        "case {case:?}: unexpected executionId in {summary}"
    );
    assert_eq!(
        summary["actionType"].as_str().map(str::to_string),
        fixture.action_type_of(row),
        "case {case:?}: unexpected actionType in {summary}"
    );
    let expected_target = fixture.id_of(row.target);
    let reported_target = match row.target {
        Target::ComputationModel => summary["targetResource"]["computationModelId"].as_str(),
        _ => summary["targetResource"]["assetId"].as_str(),
    };
    assert_eq!(
        reported_target,
        Some(expected_target),
        "case {case:?}: unexpected targetResource in {summary}"
    );
    assert_eq!(
        summary["targetResourceVersion"].as_str(),
        Some(row.target_resource_version),
        "case {case:?}: unexpected targetResourceVersion in {summary}"
    );
    assert_eq!(
        summary["resolveTo"]["assetId"].as_str(),
        row.resolve_to.map(|resolved| fixture.id_of(resolved)),
        "case {case:?}: unexpected resolveTo in {summary}"
    );
    assert_eq!(
        summary["executionStartTime"].as_f64().map(|at| at as i64),
        Some(fixture.start_of(row)),
        "case {case:?}: unexpected executionStartTime in {summary}"
    );
    assert_eq!(
        summary["executionEntityVersion"].as_str(),
        row.entity_version,
        "case {case:?}: unexpected executionEntityVersion in {summary}"
    );

    // The run as the clock has it: a row whose end is still ahead of the anchor is
    // reported RUNNING with no end time, and one whose end has passed reports the
    // state it settled into and the instant it ended.
    let still_running = row.settles_in > 0;
    let expected_state = if still_running {
        EXECUTION_STATE_RUNNING
    } else {
        row.outcome
    };
    assert_eq!(
        summary["executionStatus"]["state"].as_str(),
        Some(expected_state),
        "case {case:?}: unexpected executionStatus in {summary}"
    );
    let expected_end = (!still_running).then(|| fixture.settles_at_of(row));
    assert_eq!(
        summary["executionEndTime"].as_f64().map(|at| at as i64),
        expected_end,
        "case {case:?}: unexpected executionEndTime in {summary}"
    );
}

#[tokio::test]
async fn a_listing_reports_every_stored_execution_of_the_target_resource_it_names() {
    let fixture = fixture().await;

    for target in [Target::AssetA, Target::AssetB] {
        let asset_id = fixture.id_of(target);
        let (status, body) = list_executions(&fixture.router, &asset_query(asset_id)).await;
        assert_eq!(
            status, 200,
            "case {target:?}: expected 200, got {status} {body}"
        );

        let expected = fixture.ordered_rows(target);
        let reported = reported_ids(&body);
        let expected_ids: Vec<String> = expected.iter().map(|row| row.id.to_string()).collect();
        assert_eq!(
            reported, expected_ids,
            "case {target:?}: a listing must report this target's executions oldest \
             first, ties broken by id, got {body}"
        );

        let summaries = body["executionSummaries"]
            .as_array()
            .expect("an executionSummaries list");
        for (row, summary) in expected.iter().zip(summaries) {
            assert_summary(&fixture, row, summary);
        }

        // A single page holds the whole listing, so there is nothing to resume from.
        assert!(
            body["nextToken"].is_null(),
            "case {target:?}: a complete page must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn the_action_type_and_resolve_to_filters_narrow_the_listing_to_their_matches() {
    let fixture = fixture().await;
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let asset_b = fixture.id_of(Target::AssetB).to_string();

    // Every filter form, each expected against the seeded table rather than a
    // hand-written answer: an `actionType` present on the target, one present only
    // on ANOTHER target, the longest admitted one, and a `resolveTo` reference to
    // each asset.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "actionType on this target",
            format!("{}&actionType=acknowledge", asset_query(&asset_a)),
            vec!["aaaaaaa5-5555-4555-8555-555555555555".to_string()],
        ),
        (
            "actionType of one character",
            format!("{}&actionType=a", asset_query(&asset_a)),
            vec!["aaaaaaa1-1111-4111-8111-111111111111".to_string()],
        ),
        (
            "actionType at the published length bound",
            format!(
                "{}&actionType={}",
                asset_query(&asset_a),
                longest_action_type()
            ),
            vec!["aaaaaaa2-2222-4222-8222-222222222222".to_string()],
        ),
        (
            "actionType no execution of this target carries",
            format!("{}&actionType=snooze", asset_query(&asset_a)),
            Vec::new(),
        ),
        (
            "resolveTo asset A",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_a}",
                asset_query(&asset_a)
            ),
            vec![
                "aaaaaaa3-3333-4333-8333-333333333333".to_string(),
                "aaaaaaa5-5555-4555-8555-555555555555".to_string(),
            ],
        ),
        (
            "resolveTo asset B",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={asset_b}",
                asset_query(&asset_a)
            ),
            vec![
                "aaaaaaa2-2222-4222-8222-222222222222".to_string(),
                "aaaaaaa4-4444-4444-8444-444444444444".to_string(),
            ],
        ),
        (
            "both filters at once",
            format!(
                "{}&actionType=acknowledge&resolveToResourceType=ASSET&resolveToResourceId={asset_b}",
                asset_query(&asset_a)
            ),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let (status, body) = list_executions(&fixture.router, &query).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: unexpected matches for {query}, got {body}"
        );
    }
}

#[tokio::test]
async fn paging_a_listing_walks_the_whole_order_once_and_resumes_where_it_stopped() {
    let fixture = fixture().await;
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let order: Vec<String> = fixture
        .ordered_rows(Target::AssetA)
        .iter()
        .map(|row| row.id.to_string())
        .collect();

    // Every page size from one row to one more than the listing holds, plus the
    // published upper bound of `maxResults` (range 1..250).
    for page_size in [1usize, 2, 3, order.len(), order.len() + 1, 250] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut query = format!("{}&maxResults={page_size}", asset_query(&asset_a));
            if let Some(token) = token.as_deref() {
                query.push_str(&format!("&nextToken={token}"));
            }
            let (status, body) = list_executions(&fixture.router, &query).await;
            assert_eq!(
                status, 200,
                "case {page_size}: expected 200, got {status} {body}"
            );
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page must hold at most maxResults rows, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= order.len() + 1,
                "case {page_size}: paging must terminate, walked {walked:?}"
            );

            match body["nextToken"].as_str() {
                Some(next) => {
                    // The token names the first row of the next page, hex-encoded —
                    // checked against the suite's own encoder, not the handler's.
                    assert_eq!(
                        next,
                        hex_token(&order[walked.len()]),
                        "case {page_size}: unexpected resume point, got {body}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case {page_size}: paging must walk the whole order exactly once"
        );
        let unique: HashSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {page_size}: paging must report no row twice, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_or_filter_that_names_nothing_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let before = list_executions(&fixture.router, &asset_query(&asset_a)).await;

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a target asset no store holds",
            asset_query(ABSENT_ID),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a computation-model target, which no store holds",
            format!(
                "targetResourceType=COMPUTATION_MODEL&targetResourceId={}",
                fixture.id_of(Target::ComputationModel)
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resolveTo asset no store holds",
            format!(
                "{}&resolveToResourceType=ASSET&resolveToResourceId={ABSENT_ID}",
                asset_query(&asset_a)
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resolveTo type with no id",
            format!("{}&resolveToResourceType=ASSET", asset_query(&asset_a)),
            400,
            "InvalidRequestException",
        ),
        (
            "a resolveTo id with no type",
            format!(
                "{}&resolveToResourceId={asset_a}",
                asset_query(&asset_a)
            ),
            400,
            "InvalidRequestException",
        ),
        (
            "a nextToken this operation never issued",
            format!("{}&nextToken=zzzz", asset_query(&asset_a)),
            400,
            "InvalidRequestException",
        ),
        (
            "a nextToken whose resume point is not in this enumeration",
            format!(
                "{}&nextToken={}",
                asset_query(&asset_a),
                hex_token(ABSENT_ID)
            ),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, query, status_wanted, code_wanted) in cases {
        let (status, body) = list_executions(&fixture.router, &query).await;
        assert_eq!(
            (status, error_code(&body)),
            (status_wanted, code_wanted),
            "case {case:?}: unexpected refusal for {query}, got {body}"
        );
    }

    // A listing reports; none of the calls above — refused or served — may change
    // what the next caller sees.
    let after = list_executions(&fixture.router, &asset_query(&asset_a)).await;
    assert_eq!(
        after, before,
        "a listing must not change what a later listing reports"
    );
}

#[tokio::test]
async fn an_execution_reports_its_outcome_only_once_its_run_is_due_to_have_ended() {
    let (router, engine) = router_with_engine();
    let model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_id = create_asset(&router, "Boiler-A", &model).await;

    // A run whose end falls due shortly after this call: the span is what makes the
    // RUNNING state observable before it, and the outcome observable after it.
    let settles_in = Duration::from_millis(300);
    let started = SystemTime::now();
    let settles_at = started + settles_in;
    let execution_id = "dddddddd-1111-4111-8111-111111111111";
    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        state.lock().executions.insert(
            execution_id.to_string(),
            ExecutionRecord {
                execution_id: execution_id.to_string(),
                action_type: Some("acknowledge".to_string()),
                target_resource: TargetResource {
                    asset_id: Some(asset_id.clone()),
                    computation_model_id: None,
                },
                target_resource_version: "1".to_string(),
                resolve_to: None,
                execution_start_time: started,
                settles_at,
                outcome: EXECUTION_STATE_FAILED.to_string(),
                execution_entity_version: None,
            },
        );
    }

    let (status, body) = list_executions(&router, &asset_query(&asset_id)).await;
    assert_eq!(status, 200, "in flight: expected 200, got {status} {body}");
    let summary = &body["executionSummaries"][0];
    assert_eq!(
        summary["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_RUNNING),
        "in flight: a run whose end is not yet due must still be running, got {body}"
    );
    assert!(
        summary["executionEndTime"].is_null(),
        "in flight: a run that has not ended reports no end time, got {body}"
    );

    tokio::time::sleep(settles_in + Duration::from_millis(100)).await;

    let (status, body) = list_executions(&router, &asset_query(&asset_id)).await;
    assert_eq!(status, 200, "settled: expected 200, got {status} {body}");
    let summary = &body["executionSummaries"][0];
    assert_eq!(
        summary["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_FAILED),
        "settled: a run whose end has fallen due must report its outcome, got {body}"
    );
    let reported_end = summary["executionEndTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("settled: no executionEndTime in {body}"));
    let expected_end = settles_at
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the Unix epoch")
        .as_secs_f64();
    assert!(
        (reported_end - expected_end).abs() < 0.01,
        "settled: the end time must be the instant the run was due to end, \
         expected {expected_end}, got {reported_end}"
    );
}

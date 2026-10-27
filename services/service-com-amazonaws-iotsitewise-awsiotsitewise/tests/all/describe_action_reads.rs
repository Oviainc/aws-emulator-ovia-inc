// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the read of ONE action, `DescribeAction`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rule this suite pins: a read of one action reports, from the shared store,
//! every member of the record its `actionId` addresses — the resource the action
//! was taken on (naming only the kind that resource is), the definition it ran,
//! the payload it carried, the instant it ran, and the detailed resource it
//! resolves to only when one was recorded — reports the SAME action the listing
//! summarises, writes nothing, and refuses an id no action is recorded under with
//! this service's not-found wording.
//!
//! The harness is re-used rather than copied: the action-side helpers
//! (`Target`, `create_action_targets`, `seed_action`, `target_query`,
//! `list_actions`, `ABSENT_ID`) are the `actions` suite's, the read request is
//! `execute_action`'s `describe_action`, the wire harness is `asset_lifecycle`'s
//! and the message reader is `hierarchies_etags_and_refusals`'. What this suite
//! adds is its own table — the stored forms a READ generalises over, which the
//! listing never reports: the payload, the execution instant, and an action
//! recorded against an asset.
//!
//! `execute_action` is the suite that covers the write path and the round trip
//! from it back through this read; `ExecuteAction` resolves an
//! `actionDefinitionId` only against the actions a computation model's
//! computation type publishes, so the asset-targeted rows below can only be
//! recorded through the store, as `seed_action` documents.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::actions::{
    create_action_targets, list_actions, seed_action, target_query, Target, ABSENT_ID,
};
use crate::asset_lifecycle::{error_code, router_with_engine};
use crate::execute_action::describe_action;
use crate::hierarchies_etags_and_refusals::error_message;

/// One row of the seeded action table: one stored action, and what a read of it
/// must report.
#[derive(Debug, Clone)]
struct ActionRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The id the action is stored under and read back by.
    id: &'static str,
    /// The action definition this action ran.
    definition: &'static str,
    /// The resource the action was taken on.
    target: Target,
    /// The detailed resource the action resolves to, when it resolves to one.
    resolve_to: Option<Target>,
    /// The `actionPayload.stringValue` the action carried.
    payload: String,
    /// The instant the action ran, as milliseconds since the epoch. Absolute
    /// rather than relative to the run, so the reported timestamp is checked
    /// against an exact expectation.
    ran_at_millis: u64,
}

/// The seeded actions: one table spanning the dimensions the read rule
/// generalises over — which resource the action was taken on and of which of the
/// two modelled kinds, whether it resolves to a detailed resource and whether
/// that is the target itself or another one, the form and size of the payload it
/// carried, and when it ran (before and after the instant the suite runs at, on a
/// whole second and on a fraction of one).
///
/// The payload forms span what `ActionPayloadString` admits: `smithy_query(
/// query="shape", shape="ActionPayloadString")` gives pattern
/// `^[^\u0000-\u001F\u007F]+$` and no length bound, so any non-empty run of
/// non-control characters — one character, punctuation-only JSON, non-ASCII text
/// and a long run.
fn action_table() -> Vec<ActionRow> {
    vec![
        ActionRow {
            case: "on asset A, resolving to nothing, the smallest JSON payload",
            id: "cacacaca-1111-4111-8111-111111111111",
            definition: "11111111-1111-4111-8111-111111111111",
            target: Target::AssetA,
            resolve_to: None,
            payload: "{}".to_string(),
            // A whole second, well before the instant this suite runs at.
            ran_at_millis: 1_600_000_000_000,
        },
        ActionRow {
            case: "on asset A, resolving to A itself, a non-ASCII payload",
            id: "cacacaca-2222-4222-8222-222222222222",
            definition: "22222222-2222-4222-8222-222222222222",
            target: Target::AssetA,
            resolve_to: Some(Target::AssetA),
            payload: "{\"setpoint\":42.5,\"unit\":\"°C\",\"note\":\"ünïcode ✓\"}".to_string(),
            // A fraction of a second: the reported timestamp keeps it.
            ran_at_millis: 1_700_000_000_500,
        },
        ActionRow {
            case: "on asset B, resolving to asset A, a one-character payload",
            id: "cacacaca-3333-4333-8333-333333333333",
            definition: "33333333-3333-4333-8333-333333333333",
            target: Target::AssetB,
            resolve_to: Some(Target::AssetA),
            payload: "x".to_string(),
            ran_at_millis: 1_750_000_123_250,
        },
        ActionRow {
            case: "on the computation model, resolving to asset B, a long payload",
            id: "cacacaca-4444-4444-8444-444444444444",
            definition: "44444444-4444-4444-8444-444444444444",
            target: Target::ComputationModel,
            resolve_to: Some(Target::AssetB),
            payload: format!("{{\"blob\":\"{}\"}}", "a".repeat(2048)),
            ran_at_millis: 1_760_000_000_750,
        },
        ActionRow {
            case: "on the computation model, resolving to nothing, running the same definition",
            id: "cacacaca-5555-4555-8555-555555555555",
            definition: "44444444-4444-4444-8444-444444444444",
            target: Target::ComputationModel,
            resolve_to: None,
            payload: "{\"mode\":\"REPLAY\"}".to_string(),
            // After the instant this suite runs at: a read reports the stored
            // instant rather than clamping it to now.
            ran_at_millis: 4_000_000_000_000,
        },
    ]
}

/// The router, the engine behind it, the ids of the seeded resources and the
/// seeded rows.
struct Fixture {
    router: smithy_server_runtime::MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    asset_a: String,
    asset_b: String,
    computation_model: String,
    rows: Vec<ActionRow>,
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
}

/// Build the fixture: the resources an action can be taken on, created through
/// the API, then the action table recorded in the shared store those handlers
/// write through.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let (asset_a, asset_b, computation_model) = create_action_targets(&router).await;

    let fixture = Fixture {
        router,
        engine,
        asset_a,
        asset_b,
        computation_model,
        rows: action_table(),
    };

    for row in &fixture.rows {
        seed_action(
            &fixture.engine,
            row.id,
            row.definition,
            row.target,
            fixture.id_of(row.target),
            &row.payload,
            row.resolve_to.map(|resolved| fixture.id_of(resolved)),
            UNIX_EPOCH + Duration::from_millis(row.ran_at_millis),
        );
    }

    fixture
}

/// Every stored action, as `(actionId, the record's own debug form)`, so a test
/// can state that a call wrote NOTHING rather than that one member it looked at
/// is unchanged.
fn stored_actions(engine: &Arc<engine::InMemoryEngine>) -> BTreeMap<String, String> {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let store = state.lock();
    store
        .actions
        .iter()
        .map(|(id, record)| (id.clone(), format!("{record:?}")))
        .collect()
}

/// Assert `described` reports exactly what `row` was recorded with.
fn assert_reports_row(fixture: &Fixture, row: &ActionRow, described: &Value) {
    let case = row.case;
    assert_eq!(
        described["actionId"].as_str(),
        Some(row.id),
        "case {case:?}: a read must report the action it was asked for, got {described}"
    );
    assert_eq!(
        described["actionDefinitionId"].as_str(),
        Some(row.definition),
        "case {case:?}: unexpected actionDefinitionId in {described}"
    );
    assert_eq!(
        described["actionPayload"]["stringValue"].as_str(),
        Some(row.payload.as_str()),
        "case {case:?}: a read must report the payload the action carried, got {}",
        described["actionPayload"]
    );

    let expected_target = fixture.id_of(row.target);
    let reported_target = match row.target {
        Target::ComputationModel => described["targetResource"]["computationModelId"].as_str(),
        _ => described["targetResource"]["assetId"].as_str(),
    };
    assert_eq!(
        reported_target,
        Some(expected_target),
        "case {case:?}: unexpected targetResource in {described}"
    );
    // The member the target's own kind does NOT occupy stays absent, so one
    // action names one resource rather than two.
    let other_member = match row.target {
        Target::ComputationModel => &described["targetResource"]["assetId"],
        _ => &described["targetResource"]["computationModelId"],
    };
    assert!(
        other_member.is_null(),
        "case {case:?}: a targetResource must name only its own kind, got {described}"
    );

    assert_eq!(
        described["resolveTo"]["assetId"].as_str(),
        row.resolve_to.map(|resolved| fixture.id_of(resolved)),
        "case {case:?}: unexpected resolveTo in {described}"
    );
    // `resolveTo` is the one OPTIONAL output member, so a row recorded without
    // one reports no member at all rather than an empty structure.
    if row.resolve_to.is_none() {
        assert!(
            described["resolveTo"].is_null(),
            "case {case:?}: an action resolving to nothing must report no resolveTo, got {described}"
        );
    }

    // `executionTime` is modelled a `Timestamp`, carried on the wire as epoch
    // seconds, and reports the instant the action RAN — including its fraction
    // of a second.
    let expected_seconds = row.ran_at_millis as f64 / 1000.0;
    let reported = described["executionTime"].as_f64();
    assert!(
        reported.is_some_and(|at| (at - expected_seconds).abs() < 1e-3),
        "case {case:?}: expected an executionTime of {expected_seconds} epoch seconds, got {:?}",
        described["executionTime"]
    );
}

#[tokio::test]
async fn a_read_reports_every_member_of_the_stored_action_its_id_addresses() {
    let fixture = fixture().await;

    for row in &fixture.rows {
        let case = row.case;
        let (status, described) = describe_action(&fixture.router, row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {described}"
        );
        assert_reports_row(&fixture, row, &described);
    }
}

#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    let before = stored_actions(&fixture.engine);

    let mut first_answers: Vec<Value> = Vec::new();
    for row in &fixture.rows {
        let (_, described) = describe_action(&fixture.router, row.id).await;
        first_answers.push(described);
    }

    // Reading every row again answers each one exactly as the first read did, and
    // the store is byte-for-byte what it was: this operation reports, it does not
    // mutate.
    for (row, first) in fixture.rows.iter().zip(&first_answers) {
        let case = row.case;
        let (status, again) = describe_action(&fixture.router, row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on the second read, got {status} {again}"
        );
        assert_eq!(
            &again, first,
            "case {case:?}: a second read must report the same action, got {again}"
        );
    }
    assert_eq!(
        stored_actions(&fixture.engine),
        before,
        "a read must write nothing"
    );
}

#[tokio::test]
async fn a_read_and_the_listing_report_one_action_rather_than_two_views_of_it() {
    let fixture = fixture().await;

    for target in [Target::AssetA, Target::AssetB, Target::ComputationModel] {
        let query = target_query(target, fixture.id_of(target));
        let (status, listed) = list_actions(&fixture.router, &query).await;
        assert_eq!(
            status, 200,
            "case {target:?}: expected 200, got {status} {listed}"
        );
        let summaries = listed["actionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {target:?}: no actionSummaries list in {listed}"))
            .clone();
        let expected = fixture
            .rows
            .iter()
            .filter(|row| row.target == target)
            .count();
        assert_eq!(
            summaries.len(),
            expected,
            "case {target:?}: expected {expected} summaries, got {listed}"
        );

        for summary in &summaries {
            let action_id = summary["actionId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {target:?}: a summary with no actionId in {listed}"))
                .to_string();
            let (status, described) = describe_action(&fixture.router, &action_id).await;
            assert_eq!(
                status, 200,
                "case {target:?}: expected 200 for {action_id}, got {status} {described}"
            );
            // Every identifier and reference the summary carries is what the read
            // of that same action reports.
            for member in [
                "actionId",
                "actionDefinitionId",
                "targetResource",
                "resolveTo",
            ] {
                assert_eq!(
                    described[member], summary[member],
                    "case {target:?}: {member} differs between the read and the summary of \
                     {action_id}: {} vs {}",
                    described[member], summary[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn an_action_id_no_action_is_recorded_under_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let before = stored_actions(&fixture.engine);

    // Every way a well-formed id can address no stored action: one nothing
    // minted, one that names a stored resource of each kind rather than an
    // action, and one that names an action DEFINITION an action ran.
    let asset_a = fixture.id_of(Target::AssetA).to_string();
    let computation_model = fixture.id_of(Target::ComputationModel).to_string();
    let definition = fixture.rows[0].definition.to_string();
    let cases: Vec<(&str, String)> = vec![
        ("an id no create minted", ABSENT_ID.to_string()),
        ("the id of a stored asset", asset_a),
        ("the id of a stored computation model", computation_model),
        ("the id of an action definition an action ran", definition),
    ];

    for (case, requested) in cases {
        let (status, answered) = describe_action(&fixture.router, &requested).await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: unexpected refusal for {requested}, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("Action with ID {requested} not found"),
            "case {case:?}: unexpected refusal wording, got {answered}"
        );
    }

    // `actionId` carries the modelled length 36..36 (`smithy_query(
    // query="operation", operation="DescribeAction")`), so an id outside that
    // length is a BAD REQUEST rather than an absent action. The bound is checked
    // on each side: one character short and one past it.
    for (case, requested) in [
        (
            "one character short of the modelled length",
            "cacacaca-1111-4111-8111-11111111111",
        ),
        (
            "one character past the modelled length",
            "cacacaca-1111-4111-8111-1111111111111",
        ),
    ] {
        let (status, answered) = describe_action(&fixture.router, requested).await;
        // The one client refusal this operation models for a request it cannot
        // make sense of: `smithy_query(query="operation",
        // operation="DescribeAction")` declares `InvalidRequestException` (HTTP
        // 400), so the code is pinned rather than matched against a set that an
        // unmodelled error shape would also satisfy.
        assert_eq!(
            (status, error_code(&answered)),
            (400, "InvalidRequestException"),
            "case {case:?}: unexpected refusal for {requested}, got {answered}"
        );
    }

    // No refusal wrote anything, and the actions the store does hold are still
    // readable exactly as they were recorded.
    assert_eq!(
        stored_actions(&fixture.engine),
        before,
        "a refused read must write nothing"
    );
    for row in &fixture.rows {
        let (status, described) = describe_action(&fixture.router, row.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 after the refusals, got {status} {described}",
            row.case
        );
        assert_reports_row(&fixture, row, &described);
    }
}

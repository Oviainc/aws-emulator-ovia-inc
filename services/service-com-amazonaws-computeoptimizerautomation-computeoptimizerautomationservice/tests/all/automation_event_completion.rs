// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the terminal status a STARTED automation event reaches.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! dispatched through the same [`MultiServiceRouter`] the emulator binary mounts
//! — through the harness `automation_rule_lifecycle` owns. The one thing these
//! cases reach into the store for is SEEDING a recommended action: Compute
//! Optimizer's analysis is what produces one, and that analysis is not part of
//! this API, so no wire call can create it. Everything asserted afterwards is
//! read back over the wire.
//!
//! The rule under test: an automation the service accepted and ran ends in the
//! modelled success status (`Complete`) with the instant it completed and NO
//! `eventStatusReason`, and every step it performed is terminal too, stamped in
//! the order the run performed them. `Failed` is reserved for an automation that
//! could not be carried out.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    call_ok, emulator_with_engine, member, string_member,
};

/// One recommended action, as Compute Optimizer's analysis would have produced
/// it. `recommended_action_id` satisfies the modelled `RecommendedActionId`
/// (`^[0-9A-Za-z]{16}$`) because a request names it.
fn recommended_action(
    id: &str,
    action_type: &str,
    volume: &str,
    savings: Option<EstimatedMonthlySavings>,
) -> RecommendedAction {
    RecommendedAction {
        recommended_action_id: Some(id.to_string()),
        resource_arn: Some(format!(
            "arn:aws:ec2:us-east-1:{}:volume/{volume}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        resource_id: Some(volume.to_string()),
        account_id: Some(engine::EMULATOR_ACCOUNT_ID.to_string()),
        region: Some("us-east-1".to_string()),
        // The model's only `ResourceType` value.
        resource_type: Some("EbsVolume".to_string()),
        recommended_action_type: Some(action_type.to_string()),
        estimated_monthly_savings: savings,
        ..Default::default()
    }
}

/// Put `action` in the store the mounted service reads from.
fn seed_action(engine: &Arc<engine::InMemoryEngine>, action: &RecommendedAction) {
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    let id = action
        .recommended_action_id
        .clone()
        .expect("a seeded action carries its id");
    service_state
        .lock()
        .recommended_actions
        .insert(id, action.clone());
}

/// `GetAutomationEvent` for `event_id`, requiring success.
async fn get_event(router: &MultiServiceRouter, event_id: &str, case: &str) -> Value {
    call_ok(
        router,
        "GetAutomationEvent",
        &json!({"eventId": event_id}),
        case,
    )
    .await
}

/// The steps `ListAutomationEventSteps` reports for `event_id`, in page order.
async fn listed_steps(router: &MultiServiceRouter, event_id: &str, case: &str) -> Vec<Value> {
    let listed = call_ok(
        router,
        "ListAutomationEventSteps",
        &json!({"eventId": event_id}),
        case,
    )
    .await;
    member(&listed, "automationEventSteps")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `eventId`s `ListAutomationEvents` reports for one `EventStatus` filter.
async fn ids_with_status(router: &MultiServiceRouter, status: &str, case: &str) -> Vec<String> {
    let listed = call_ok(
        router,
        "ListAutomationEvents",
        &json!({"filters": [{"name": "EventStatus", "values": [status]}]}),
        case,
    )
    .await;
    member(&listed, "automationEvents")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| string_member(event, "eventId"))
        .collect()
}

/// One row of the completion table: what to seed, how to start it and how many
/// steps the automation its type names performs.
struct Case {
    case: &'static str,
    action_id: &'static str,
    action_type: &'static str,
    volume: &'static str,
    estimate: Option<EstimatedMonthlySavings>,
    client_token: Option<&'static str>,
    step_count: usize,
}

/// One estimate, for the rows that carry one.
fn savings(before: f64, after: f64) -> EstimatedMonthlySavings {
    EstimatedMonthlySavings {
        currency: "USD".to_string(),
        before_discount_savings: before,
        after_discount_savings: after,
        savings_estimation_mode: "BeforeDiscount".to_string(),
    }
}

/// Every started automation ends in the modelled success status, whatever it
/// automates: the table spans both modelled `RecommendedActionType`s (whose step
/// sequences differ in length), an action with and without an estimate, and a
/// start with and without a client token. A completed event reports its
/// completion instant and no status reason, no started event ever reports the
/// failure status, and the status the event readers report is the one the
/// `EventStatus` filter selects on.
#[tokio::test]
async fn every_started_automation_settles_to_the_modelled_success_status() {
    let (router, engine) = emulator_with_engine();

    // (case, recommendedActionId, recommendedActionType, volume, estimate,
    //  clientToken, how many steps the type performs). The action types are the
    //  two the model declares
    //  (`smithy_query(query="shape",shape="RecommendedActionType")`), and their
    //  step sequences are the ones `StepType` admits: a snapshot-and-delete
    //  takes two steps, a volume-type upgrade one.
    let cases = vec![
        Case {
            case: "upgrade-no-estimate",
            action_id: "Upgrade0000000A1",
            action_type: "UpgradeEbsVolumeType",
            volume: "vol-00000000000000a1",
            estimate: None,
            client_token: None,
            step_count: 1,
        },
        Case {
            case: "upgrade-with-estimate-and-token",
            action_id: "Upgrade0000000B2",
            action_type: "UpgradeEbsVolumeType",
            volume: "vol-00000000000000b2",
            estimate: Some(savings(12.5, 10.0)),
            client_token: Some("completion-token-b2"),
            step_count: 1,
        },
        Case {
            case: "snapshot-delete-with-estimate",
            action_id: "Snapshot000000C3",
            action_type: "SnapshotAndDeleteUnattachedEbsVolume",
            volume: "vol-00000000000000c3",
            estimate: Some(savings(40.0, 33.25)),
            client_token: None,
            step_count: 2,
        },
        Case {
            case: "snapshot-delete-no-estimate-with-token",
            action_id: "Snapshot000000D4",
            action_type: "SnapshotAndDeleteUnattachedEbsVolume",
            volume: "vol-00000000000000d4",
            estimate: None,
            client_token: Some("completion-token-d4"),
            step_count: 2,
        },
    ];

    // Start every row, and check the status the start call answers with is the
    // non-terminal one: the run has not had time to finish.
    let mut started: Vec<(&str, String, usize)> = Vec::new();
    for row in &cases {
        let case = row.case;
        seed_action(
            &engine,
            &recommended_action(
                row.action_id,
                row.action_type,
                row.volume,
                row.estimate.clone(),
            ),
        );
        let mut request = json!({"recommendedActionId": row.action_id});
        if let Some(token) = row.client_token {
            request["clientToken"] = json!(token);
        }
        let answer = call_ok(&router, "StartAutomationEvent", &request, case).await;
        assert_eq!(
            string_member(&answer, "eventStatus"),
            "InProgress",
            "case {case:?}: a just-started automation is still running, got {answer}"
        );

        let event_id = string_member(&answer, "eventId");
        let running = get_event(&router, &event_id, case).await;
        assert_eq!(
            string_member(&running, "eventStatus"),
            "InProgress",
            "case {case:?}: the event reader agrees the run is not over, got {running}"
        );
        assert!(
            member(&running, "completedTimestamp").is_null(),
            "case {case:?}: a run that is not over has not completed, got {running}"
        );
        started.push((case, event_id, row.step_count));
    }

    // The terminal status the clock owes every one of them.
    tokio::time::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;

    for (case, event_id, step_count) in &started {
        let settled = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&settled, "eventStatus"),
            "Complete",
            "case {case:?}: a run that was carried out completes, got {settled}"
        );
        assert!(
            member(&settled, "eventStatusReason").is_null(),
            "case {case:?}: a completed run has nothing to explain, got {settled}"
        );
        let created = member(&settled, "createdTimestamp").as_f64();
        let completed = member(&settled, "completedTimestamp").as_f64();
        let (Some(created), Some(completed)) = (created, completed) else {
            panic!("case {case:?}: a completed run carries both instants, got {settled}");
        };
        assert!(
            completed >= created,
            "case {case:?}: a run cannot complete before it started, got {created} then {completed}"
        );

        // Reading it again changes nothing: the transition was the clock's, and
        // it has already been materialised.
        let again = get_event(&router, event_id, case).await;
        assert_eq!(
            (
                string_member(&again, "eventStatus"),
                member(&again, "completedTimestamp").as_f64()
            ),
            ("Complete".to_string(), Some(completed)),
            "case {case:?}: a second read reports the same settled event, got {again}"
        );

        // Every step the run performed is terminal, in run order, and the last
        // one ends exactly when the event did.
        let steps = listed_steps(&router, event_id, case).await;
        assert_eq!(
            steps.len(),
            *step_count,
            "case {case:?}: expected {step_count} steps for this event type, got {steps:?}"
        );
        let mut previous_end: Option<f64> = None;
        for (position, step) in steps.iter().enumerate() {
            assert_eq!(
                string_member(step, "stepStatus"),
                "Complete",
                "case {case:?}: step {position} of a completed run is complete, got {step}"
            );
            let start = member(step, "startTimestamp").as_f64();
            let end = member(step, "completedTimestamp").as_f64();
            let (Some(start), Some(end)) = (start, end) else {
                panic!("case {case:?}: step {position} ran, so it carries both instants, got {step}");
            };
            assert!(
                start <= end,
                "case {case:?}: step {position} cannot end before it starts, got {start} then {end}"
            );
            if let Some(previous_end) = previous_end {
                assert!(
                    previous_end <= start,
                    "case {case:?}: step {position} starts no earlier than its predecessor \
                     ended, got {previous_end} then {start}"
                );
            }
            previous_end = Some(end);
        }
        assert_eq!(
            previous_end,
            Some(completed),
            "case {case:?}: the run ends when its last step does, got {settled}"
        );
    }

    // What the readers report is what the `EventStatus` filter selects on: every
    // settled event is `Complete`, and none of them is `Failed`.
    let mut completed_ids = ids_with_status(&router, "Complete", "filter-complete").await;
    completed_ids.sort();
    let mut expected_ids: Vec<String> = started
        .iter()
        .map(|(_, event_id, _)| event_id.clone())
        .collect();
    expected_ids.sort();
    assert_eq!(
        completed_ids, expected_ids,
        "case \"filter-complete\": the Complete filter must select every settled event"
    );
    for refused_status in ["Failed", "Cancelled", "RollbackComplete"] {
        let case = format!("filter-{refused_status}");
        assert_eq!(
            ids_with_status(&router, refused_status, &case).await,
            Vec::<String>::new(),
            "case {case:?}: no automation this service ran reports {refused_status}"
        );
    }
}

/// An automation that is still running is in no terminal status at all: before
/// the run window has passed, the event is selected by the `InProgress` filter
/// and by neither terminal one, and its steps are the sequence with the first
/// one running.
#[tokio::test]
async fn a_run_that_is_not_over_is_in_no_terminal_status() {
    let (router, engine) = emulator_with_engine();
    let action = recommended_action(
        "Snapshot000000E5",
        "SnapshotAndDeleteUnattachedEbsVolume",
        "vol-00000000000000e5",
        Some(savings(7.0, 6.5)),
    );
    seed_action(&engine, &action);

    let answer = call_ok(
        &router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": "Snapshot000000E5"}),
        "running",
    )
    .await;
    let event_id = string_member(&answer, "eventId");

    assert_eq!(
        ids_with_status(&router, "InProgress", "running-filter").await,
        vec![event_id.clone()],
        "case \"running-filter\": a run that is not over is still in progress"
    );
    for terminal in ["Complete", "Failed"] {
        let case = format!("running-not-{terminal}");
        assert_eq!(
            ids_with_status(&router, terminal, &case).await,
            Vec::<String>::new(),
            "case {case:?}: a run that is not over has reached no terminal status"
        );
    }

    let steps = listed_steps(&router, &event_id, "running-steps").await;
    assert_eq!(
        steps
            .iter()
            .map(|step| string_member(step, "stepStatus"))
            .collect::<Vec<_>>(),
        vec!["InProgress".to_string(), "Ready".to_string()],
        "case \"running-steps\": the run is on its first step, got {steps:?}"
    );
}

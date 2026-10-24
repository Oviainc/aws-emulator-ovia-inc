// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RollbackAutomationEvent`.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! dispatched through the same [`MultiServiceRouter`] the emulator binary mounts
//! — through the harness `automation_rule_lifecycle` owns. The only things these
//! cases reach into the store for are the two things no wire call of this API
//! creates: a recommended action (Compute Optimizer's analysis produces one, and
//! that analysis is not part of this API) and an automation event in a status
//! this emulator's own runs never reach. Everything asserted afterwards is read
//! back over the wire.
//!
//! The rule under test: a rollback may be initiated only for an automation event
//! whose automation was carried out (the terminal success status); it puts that
//! event back into the non-terminal rollback-running status with the rollback's
//! own steps appended to the run, and the clock settles it to the modelled
//! rollback-success status once the rollback window has passed. An event in any
//! other status is refused as an invalid parameter value, and an `eventId` no
//! stored event carries is not found.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    EstimatedMonthlySavings, RecommendedAction, EVENT_STATUS_VALUES,
};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, StoredEvent,
};
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, enroll_caller, member, string_member,
    unenrolled_emulator_with_engine,
};

/// One recommended action, as Compute Optimizer's analysis would have produced
/// it. `recommended_action_id` satisfies the modelled `RecommendedActionId`
/// (`^[0-9A-Za-z]{16}$`) because a request names it.
fn recommended_action(id: &str, action_type: &str, volume: &str) -> RecommendedAction {
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
        estimated_monthly_savings: Some(EstimatedMonthlySavings {
            currency: "USD".to_string(),
            before_discount_savings: 18.0,
            after_discount_savings: 15.5,
            savings_estimation_mode: "BeforeDiscount".to_string(),
        }),
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

/// One stored automation event already in `event_status`, for the cases about
/// which statuses a rollback may start from: this emulator's own runs only ever
/// produce some of the modelled statuses, so the rest are seeded. `eventId` is
/// 16 characters of `[0-9A-Za-z]` (the model's `EventId` pattern) and
/// `resourceType` carries the single value the model's `ResourceType` enum
/// declares.
fn stored_event(index: usize, event_status: &str) -> StoredEvent {
    StoredEvent {
        event_id: format!("rbk{index:013}"),
        event_description: Some(format!("seeded event {index}")),
        event_type: "UpgradeEbsVolumeType".to_string(),
        event_status: event_status.to_string(),
        event_status_reason: None,
        resource_arn: None,
        resource_id: Some(format!("vol-{index:08}")),
        recommended_action_id: None,
        account_id: engine::EMULATOR_ACCOUNT_ID.to_string(),
        region: Some("us-east-1".to_string()),
        rule_id: None,
        resource_type: "EbsVolume".to_string(),
        created_timestamp: DateTime::from(std::time::SystemTime::now()),
        completed_timestamp: None,
        estimated_monthly_savings: None,
        steps: Vec::new(),
        // A seeded event is already in the status it names, so the clock owes it
        // no transition.
        pending: None,
    }
}

/// Put `event` in the store the mounted service reads from.
fn seed_event(engine: &Arc<engine::InMemoryEngine>, event: &StoredEvent) {
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    service_state.lock().events.insert(event.clone());
}

/// `RollbackAutomationEvent` for `event_id`, with an optional client token.
fn rollback_request(event_id: &str, client_token: Option<&str>) -> Value {
    let mut request = json!({"eventId": event_id});
    if let Some(token) = client_token {
        request["clientToken"] = json!(token);
    }
    request
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

/// One row of the rollback table: what to automate, whether the rollback carries
/// a client token, and how many steps the forward run and the rollback perform.
struct Case {
    case: &'static str,
    action_id: &'static str,
    action_type: &'static str,
    volume: &'static str,
    client_token: Option<&'static str>,
    forward_steps: usize,
    rollback_steps: usize,
}

/// A completed automation of ANY type can be rolled back, and the rollback runs
/// and then ends: the table spans both modelled `RecommendedActionType`s (whose
/// forward step sequences differ in length) and a rollback with and without a
/// client token. The rollback call answers with the non-terminal
/// rollback-running status, the event readers agree, the rollback's own steps
/// are appended to the ones the automation performed, and the clock settles the
/// event to the modelled rollback-success status.
#[tokio::test]
async fn a_completed_automation_rolls_back_and_then_settles() {
    let (router, engine) = emulator_with_engine();

    // (case, recommendedActionId, recommendedActionType, volume, clientToken,
    //  forward steps, rollback steps). The action types are the two the model
    //  declares (`smithy_query(query="shape",shape="RecommendedActionType")`):
    //  a snapshot-and-delete takes two steps forward and one back (the volume is
    //  recreated from the snapshot), a volume-type upgrade one each way.
    let cases = vec![
        Case {
            case: "upgrade-no-token",
            action_id: "Upgrade0000000R1",
            action_type: "UpgradeEbsVolumeType",
            volume: "vol-0000000000000r1",
            client_token: None,
            forward_steps: 1,
            rollback_steps: 1,
        },
        Case {
            case: "upgrade-with-token",
            action_id: "Upgrade0000000R2",
            action_type: "UpgradeEbsVolumeType",
            volume: "vol-0000000000000r2",
            client_token: Some("rollback-token-r2"),
            forward_steps: 1,
            rollback_steps: 1,
        },
        Case {
            case: "snapshot-delete-no-token",
            action_id: "Snapshot000000R3",
            action_type: "SnapshotAndDeleteUnattachedEbsVolume",
            volume: "vol-0000000000000r3",
            client_token: None,
            forward_steps: 2,
            rollback_steps: 1,
        },
        Case {
            case: "snapshot-delete-with-token",
            action_id: "Snapshot000000R4",
            action_type: "SnapshotAndDeleteUnattachedEbsVolume",
            volume: "vol-0000000000000r4",
            client_token: Some("rollback-token-r4"),
            forward_steps: 2,
            rollback_steps: 1,
        },
    ];

    // Start every row's automation and let the clock carry it to completion:
    // only a completed automation has anything to roll back.
    let mut started: Vec<(&Case, String)> = Vec::new();
    for row in &cases {
        seed_action(
            &engine,
            &recommended_action(row.action_id, row.action_type, row.volume),
        );
        let answer = call_ok(
            &router,
            "StartAutomationEvent",
            &json!({"recommendedActionId": row.action_id}),
            row.case,
        )
        .await;
        started.push((row, string_member(&answer, "eventId")));
    }
    tokio::time::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;

    let mut rolled_back: Vec<(&Case, String)> = Vec::new();
    for (row, event_id) in &started {
        let case = row.case;
        let completed = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&completed, "eventStatus"),
            "Complete",
            "case {case:?}: the automation has to be complete before it is rolled back, \
             got {completed}"
        );

        let answer = call_ok(
            &router,
            "RollbackAutomationEvent",
            &rollback_request(event_id, row.client_token),
            case,
        )
        .await;
        assert_eq!(
            (
                string_member(&answer, "eventId"),
                string_member(&answer, "eventStatus")
            ),
            (event_id.clone(), "RollbackInProgress".to_string()),
            "case {case:?}: a just-initiated rollback is still running on the event it names, \
             got {answer}"
        );

        // The event reader agrees, and the event is no longer complete: it is
        // running again, so it carries no completion instant.
        let running = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&running, "eventStatus"),
            "RollbackInProgress",
            "case {case:?}: the event reader agrees the rollback is running, got {running}"
        );
        assert!(
            member(&running, "completedTimestamp").is_null(),
            "case {case:?}: an event that is running again has not completed, got {running}"
        );

        // The rollback's steps are APPENDED to the ones the automation
        // performed: the forward steps stay terminal and the first rollback step
        // is the one the event is on.
        let steps = listed_steps(&router, event_id, case).await;
        assert_eq!(
            steps.len(),
            row.forward_steps + row.rollback_steps,
            "case {case:?}: expected the forward steps plus the rollback's, got {steps:?}"
        );
        for (position, step) in steps.iter().take(row.forward_steps).enumerate() {
            assert_eq!(
                string_member(step, "stepStatus"),
                "Complete",
                "case {case:?}: forward step {position} stays complete, got {step}"
            );
        }
        let first_rollback_step = &steps[row.forward_steps];
        assert_eq!(
            string_member(first_rollback_step, "stepStatus"),
            "InProgress",
            "case {case:?}: the rollback is on its first step, got {first_rollback_step}"
        );
        // Undoing an automation realises none of its estimated savings, while
        // the forward run that produced them reported them.
        assert!(
            member(first_rollback_step, "estimatedMonthlySavings").is_null(),
            "case {case:?}: a rollback step saves nothing, got {first_rollback_step}"
        );
        assert!(
            !member(&steps[row.forward_steps - 1], "estimatedMonthlySavings").is_null(),
            "case {case:?}: the forward run's last step reported the estimate, got {steps:?}"
        );
        // Every rollback step acts on the resource the event automated.
        assert_eq!(
            string_member(first_rollback_step, "resourceId"),
            row.volume,
            "case {case:?}: a rollback step acts on the event's resource, \
             got {first_rollback_step}"
        );

        // A retry carrying the same client token replays the first call instead
        // of starting a second rollback; a call without one finds the event in
        // no state a rollback starts from.
        if let Some(token) = row.client_token {
            let replay = call_ok(
                &router,
                "RollbackAutomationEvent",
                &rollback_request(event_id, Some(token)),
                case,
            )
            .await;
            assert_eq!(
                (
                    string_member(&replay, "eventId"),
                    string_member(&replay, "eventStatus")
                ),
                (event_id.clone(), "RollbackInProgress".to_string()),
                "case {case:?}: the same token replays the rollback it started, got {replay}"
            );
        }
        assert_refused(
            call(
                &router,
                "RollbackAutomationEvent",
                &rollback_request(event_id, None),
            )
            .await,
            400,
            "InvalidParameterValueException",
            case,
        );

        rolled_back.push((row, event_id.clone()));
    }

    // The terminal status the clock owes every rollback.
    tokio::time::sleep(state::EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;

    for (row, event_id) in &rolled_back {
        let case = row.case;
        let settled = get_event(&router, event_id, case).await;
        assert_eq!(
            string_member(&settled, "eventStatus"),
            "RollbackComplete",
            "case {case:?}: a rollback that was carried out completes, got {settled}"
        );
        assert!(
            member(&settled, "eventStatusReason").is_null(),
            "case {case:?}: a completed rollback has nothing to explain, got {settled}"
        );
        let created = member(&settled, "createdTimestamp").as_f64();
        let completed = member(&settled, "completedTimestamp").as_f64();
        let (Some(created), Some(completed)) = (created, completed) else {
            panic!("case {case:?}: a completed rollback carries both instants, got {settled}");
        };
        assert!(
            completed >= created,
            "case {case:?}: a rollback cannot end before its event started, got {created} \
             then {completed}"
        );

        // Reading it again changes nothing: the transition was the clock's.
        let again = get_event(&router, event_id, case).await;
        assert_eq!(
            (
                string_member(&again, "eventStatus"),
                member(&again, "completedTimestamp").as_f64()
            ),
            ("RollbackComplete".to_string(), Some(completed)),
            "case {case:?}: a second read reports the same settled event, got {again}"
        );

        // Every step of the whole run is terminal now, in run order, and the
        // last one ends exactly when the rollback did.
        let steps = listed_steps(&router, event_id, case).await;
        assert_eq!(
            steps.len(),
            row.forward_steps + row.rollback_steps,
            "case {case:?}: a settled rollback adds no further steps, got {steps:?}"
        );
        let mut previous_end: Option<f64> = None;
        for (position, step) in steps.iter().enumerate() {
            assert_eq!(
                string_member(step, "stepStatus"),
                "Complete",
                "case {case:?}: step {position} of a settled run is complete, got {step}"
            );
            let start = member(step, "startTimestamp").as_f64();
            let end = member(step, "completedTimestamp").as_f64();
            let (Some(start), Some(end)) = (start, end) else {
                panic!(
                    "case {case:?}: step {position} ran, so it carries both instants, got {step}"
                );
            };
            assert!(
                start <= end,
                "case {case:?}: step {position} cannot end before it starts, got {start} \
                 then {end}"
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
            "case {case:?}: the rollback ends when its last step does, got {settled}"
        );
    }

    // What the readers report is what the `EventStatus` filter selects on: every
    // rolled-back event reports the rollback's success status, and none of them
    // is reported as a completed automation any more.
    let mut selected = ids_with_status(&router, "RollbackComplete", "filter-rollback-complete").await;
    selected.sort();
    let mut expected: Vec<String> = rolled_back
        .iter()
        .map(|(_, event_id)| event_id.clone())
        .collect();
    expected.sort();
    assert_eq!(
        selected, expected,
        "case \"filter-rollback-complete\": the RollbackComplete filter must select every \
         rolled-back event"
    );
    assert_eq!(
        ids_with_status(&router, "Complete", "filter-complete").await,
        Vec::<String>::new(),
        "case \"filter-complete\": a rolled-back event is no longer a completed automation"
    );
}

/// A rollback starts from the completed status and from NO other: the table is
/// the model's whole `EventStatus` value set
/// (`smithy_query(query="shape",shape="EventStatus")`), one seeded event per
/// status, and every status but the terminal success one is refused as an
/// invalid parameter value. An `eventId` no stored event carries is not found.
#[tokio::test]
async fn a_rollback_starts_only_from_the_completed_status() {
    let (router, engine) = emulator_with_engine();

    for (index, status) in EVENT_STATUS_VALUES.iter().enumerate() {
        let event = stored_event(index, status);
        seed_event(&engine, &event);
        let case = format!("status-{status}");
        let answer = call(
            &router,
            "RollbackAutomationEvent",
            &rollback_request(&event.event_id, None),
        )
        .await;
        if *status == "Complete" {
            let (http_status, body) = answer;
            assert_eq!(
                http_status, 200,
                "case {case:?}: a completed automation can be rolled back, got {body}"
            );
            assert_eq!(
                string_member(&body, "eventStatus"),
                "RollbackInProgress",
                "case {case:?}: the rollback it started is running, got {body}"
            );
        } else {
            assert_refused(answer, 400, "InvalidParameterValueException", &case);
            // The refusal left the event exactly where it was.
            let unchanged = get_event(&router, &event.event_id, &case).await;
            assert_eq!(
                string_member(&unchanged, "eventStatus"),
                *status,
                "case {case:?}: a refused rollback changes nothing, got {unchanged}"
            );
        }
    }

    // An `eventId` in the modelled form that no stored event carries.
    assert_refused(
        call(
            &router,
            "RollbackAutomationEvent",
            &rollback_request("Absent0000000001", None),
        )
        .await,
        404,
        "ResourceNotFoundException",
        "absent-event",
    );
}

/// The service-wide enrollment gate is ahead of everything else: an account that
/// is not opted in is refused, nothing is rolled back, and the client token the
/// refused call carried is not consumed — so the same token works once the
/// account enrolls.
#[tokio::test]
async fn an_unenrolled_caller_rolls_nothing_back() {
    let (router, engine) = unenrolled_emulator_with_engine();
    let event = stored_event(0, "Complete");
    seed_event(&engine, &event);

    assert_refused(
        call(
            &router,
            "RollbackAutomationEvent",
            &rollback_request(&event.event_id, Some("gate-token")),
        )
        .await,
        403,
        "OptInRequiredException",
        "unenrolled",
    );

    enroll_caller(&engine);
    let unchanged = get_event(&router, &event.event_id, "unenrolled-then-enrolled").await;
    assert_eq!(
        string_member(&unchanged, "eventStatus"),
        "Complete",
        "case \"unenrolled-then-enrolled\": the refused call rolled nothing back, got {unchanged}"
    );
    let answer = call_ok(
        &router,
        "RollbackAutomationEvent",
        &rollback_request(&event.event_id, Some("gate-token")),
        "unenrolled-then-enrolled",
    )
    .await;
    assert_eq!(
        string_member(&answer, "eventStatus"),
        "RollbackInProgress",
        "case \"unenrolled-then-enrolled\": the token the refused call carried was not \
         consumed, got {answer}"
    );
}

/// One client token names one request: reusing it for a DIFFERENT event is a
/// parameter mismatch, and reusing a token another operation already used is a
/// token in use. Both are HTTP 409, and neither rolls anything back.
#[tokio::test]
async fn a_reused_client_token_is_refused() {
    let (router, engine) = emulator_with_engine();
    let first = stored_event(1, "Complete");
    let second = stored_event(2, "Complete");
    seed_event(&engine, &first);
    seed_event(&engine, &second);

    call_ok(
        &router,
        "RollbackAutomationEvent",
        &rollback_request(&first.event_id, Some("shared-token")),
        "mismatch-first",
    )
    .await;
    assert_refused(
        call(
            &router,
            "RollbackAutomationEvent",
            &rollback_request(&second.event_id, Some("shared-token")),
        )
        .await,
        409,
        "IdempotentParameterMismatchException",
        "mismatch-second",
    );
    let unchanged = get_event(&router, &second.event_id, "mismatch-second").await;
    assert_eq!(
        string_member(&unchanged, "eventStatus"),
        "Complete",
        "case \"mismatch-second\": a refused retry rolls nothing back, got {unchanged}"
    );

    // A token another operation already used.
    seed_action(
        &engine,
        &recommended_action(
            "Upgrade0000000R9",
            "UpgradeEbsVolumeType",
            "vol-0000000000000r9",
        ),
    );
    call_ok(
        &router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": "Upgrade0000000R9", "clientToken": "start-token"}),
        "token-in-use-start",
    )
    .await;
    let third = stored_event(3, "Complete");
    seed_event(&engine, &third);
    assert_refused(
        call(
            &router,
            "RollbackAutomationEvent",
            &rollback_request(&third.event_id, Some("start-token")),
        )
        .await,
        409,
        "IdempotencyTokenInUseException",
        "token-in-use",
    );
}

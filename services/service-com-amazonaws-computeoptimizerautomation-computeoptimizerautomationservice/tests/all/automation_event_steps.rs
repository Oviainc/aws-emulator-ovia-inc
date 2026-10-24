// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListAutomationEventSteps`.
//!
//! Every case drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: ComputeOptimizerAutomationService.ListAutomationEventSteps`,
//! dispatched through the same [`MultiServiceRouter`] the emulator binary
//! mounts, so a response that is not computed from persisted state fails here.
//!
//! The two ways an event reaches the store are both exercised. A STARTED event
//! comes from `StartAutomationEvent` over the wire (its recommended action is
//! seeded, because Compute Optimizer's analysis — which produces one — is not
//! part of this API), and that is what pins the steps an event type performs. A
//! SEEDED event is written into the store with steps of the case's own choosing,
//! which is what lets the pagination and retrieval-window cases span sizes and
//! instants no wire call can establish.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::RecommendedAction;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state::{
    self, StoredEvent, StoredStep, EVENT_RETRIEVAL_WINDOW, EVENT_SETTLEMENT_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator_with_engine, member, string_member,
};

// ---------------------------------------------------------------------------
// Seeding and reading back
// ---------------------------------------------------------------------------

/// One recommended action, as Compute Optimizer's analysis would have produced
/// it: `recommended_action_id` satisfies the modelled `RecommendedActionId`
/// (`^[0-9A-Za-z]{16}$`) because a request names it, and the estimate is the
/// one the event — and the step that realises it — reports.
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
        estimated_monthly_savings: Some(
            service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::EstimatedMonthlySavings {
                currency: "USD".to_string(),
                before_discount_savings: 12.5,
                after_discount_savings: 10.0,
                savings_estimation_mode: "BeforeDiscount".to_string(),
            },
        ),
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

/// Put `event` in the store the mounted service reads from.
fn seed_event(engine: &Arc<engine::InMemoryEngine>, event: &StoredEvent) {
    let service_state = state::service_state(&**engine).expect("the service state is reachable");
    service_state.lock().events.insert(event.clone());
}

/// One stored step, already terminal, so a seeded event owes the clock nothing.
fn stored_step(index: usize, step_type: &str, volume: &str) -> StoredStep {
    StoredStep {
        step_id: format!("stp{index:013}"),
        step_type: step_type.to_string(),
        step_status: state::STEP_STATUS_READY.to_string(),
        resource_id: Some(volume.to_string()),
        start_timestamp: None,
        completed_timestamp: None,
        estimated_monthly_savings: None,
    }
}

/// One stored event carrying `steps`, created at `created_secs`, in a terminal
/// status so the clock owes it no transition.
fn stored_event(index: usize, event_type: &str, created_secs: i64, steps: Vec<StoredStep>) -> StoredEvent {
    StoredEvent {
        event_id: format!("evs{index:013}"),
        event_description: Some(format!("seeded event {index}")),
        event_type: event_type.to_string(),
        event_status: "Complete".to_string(),
        event_status_reason: None,
        resource_arn: None,
        resource_id: Some(format!("vol-{index:08}")),
        recommended_action_id: None,
        account_id: engine::EMULATOR_ACCOUNT_ID.to_string(),
        region: Some("us-east-1".to_string()),
        rule_id: None,
        resource_type: "EbsVolume".to_string(),
        created_timestamp: DateTime::from_secs(created_secs),
        completed_timestamp: None,
        estimated_monthly_savings: None,
        steps,
        settles_at: None,
    }
}

/// Now, truncated to whole seconds, so a case can name an exact instant.
fn now_secs() -> i64 {
    DateTime::from(std::time::SystemTime::now()).secs()
}

/// `ListAutomationEventSteps` for `event_id`, requiring success.
async fn steps_ok(
    router: &MultiServiceRouter,
    body: &Value,
    case: &str,
) -> Value {
    call_ok(router, "ListAutomationEventSteps", body, case).await
}

/// The steps of one response, as a list of JSON objects.
fn listed_steps(body: &Value) -> Vec<Value> {
    member(body, "automationEventSteps")
        .as_array()
        .unwrap_or_else(|| panic!("automationEventSteps must be a list, got {body}"))
        .clone()
}

/// The `stepType`s of one response, in the order returned.
fn listed_step_types(body: &Value) -> Vec<String> {
    listed_steps(body)
        .iter()
        .map(|step| string_member(step, "stepType"))
        .collect()
}

/// The `stepId`s of one response, in the order returned.
fn listed_step_ids(body: &Value) -> Vec<String> {
    listed_steps(body)
        .iter()
        .map(|step| string_member(step, "stepId"))
        .collect()
}

/// `StepId` is modelled `^[0-9A-Za-z]{16}$`.
fn is_step_id(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `NextToken` is modelled `^[A-Za-z0-9+/=]+$`.
fn is_next_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// Start an automation for `action` over the wire and answer its `eventId`.
async fn start_event(router: &MultiServiceRouter, action_id: &str, case: &str) -> String {
    let started = call_ok(
        router,
        "StartAutomationEvent",
        &json!({"recommendedActionId": action_id}),
        case,
    )
    .await;
    string_member(&started, "eventId")
}

// ---------------------------------------------------------------------------
// The steps an event type performs
// ---------------------------------------------------------------------------

/// Every `EventType` reports the steps its automation performs, in the order it
/// performs them, each step projected from the stored event.
///
/// The expected sequences are written out here rather than read from the
/// service: `EventType` is `SnapshotAndDeleteUnattachedEbsVolume |
/// UpgradeEbsVolumeType` and `StepType` is `CreateEbsSnapshot | CreateEbsVolume
/// | DeleteEbsVolume | ModifyEbsVolume`
/// (`smithy_query(query="shape",shape="EventType,StepType")`), and an event
/// type's steps are the work its name states, in that order.
#[tokio::test]
async fn every_event_type_reports_the_steps_its_automation_performs() {
    // (recommendedActionType, the stepTypes its event performs, in order)
    let cases: Vec<(&str, Vec<&str>)> = vec![
        (
            "SnapshotAndDeleteUnattachedEbsVolume",
            vec!["CreateEbsSnapshot", "DeleteEbsVolume"],
        ),
        ("UpgradeEbsVolumeType", vec!["ModifyEbsVolume"]),
    ];

    for (index, (action_type, expected_types)) in cases.iter().enumerate() {
        let (router, engine) = emulator_with_engine();
        let action_id = format!("ActioN{index:010}");
        let volume = format!("vol-0a1b2c3d{index}");
        let action = recommended_action(&action_id, action_type, &volume);
        seed_action(&engine, &action);

        let event_id = start_event(&router, &action_id, action_type).await;
        let listed = steps_ok(
            &router,
            &json!({"eventId": event_id.clone()}),
            action_type,
        )
        .await;

        assert_eq!(
            &listed_step_types(&listed),
            expected_types,
            "case {action_type:?}: the event must report the steps its automation performs, in \
             order, got {listed}"
        );
        assert!(
            member(&listed, "nextToken").is_null(),
            "case {action_type:?}: a single page that holds every step carries no token, got \
             {listed}"
        );

        let steps = listed_steps(&listed);
        let last = steps.len() - 1;
        for (position, step) in steps.iter().enumerate() {
            let case = format!("{action_type}-step-{position}");
            assert_eq!(
                string_member(step, "eventId"),
                event_id,
                "case {case:?}: a step reports the event it belongs to, got {step}"
            );
            let step_id = string_member(step, "stepId");
            assert!(
                is_step_id(&step_id),
                "case {case:?}: expected a stepId matching ^[0-9A-Za-z]{{16}}$, got {step_id:?}"
            );
            assert_eq!(
                string_member(step, "resourceId"),
                volume,
                "case {case:?}: a step acts on the resource the event automates, got {step}"
            );
            // The event is still running: the step it is on is `InProgress` and
            // carries its start, and every step queued behind it is `Ready`
            // with no timestamp at all.
            let expected_status = if position == 0 { "InProgress" } else { "Ready" };
            assert_eq!(
                string_member(step, "stepStatus"),
                expected_status,
                "case {case:?}: expected stepStatus {expected_status}, got {step}"
            );
            assert_eq!(
                member(step, "startTimestamp").is_null(),
                position != 0,
                "case {case:?}: only the step the run has reached carries a startTimestamp, got \
                 {step}"
            );
            assert!(
                member(step, "completedTimestamp").is_null(),
                "case {case:?}: no step of a running event has completed, got {step}"
            );
            // The estimate is realised by the step that changes what the
            // resource costs: the last of the sequence.
            let savings = member(step, "estimatedMonthlySavings");
            if position == last {
                assert_eq!(
                    member(savings, "beforeDiscountSavings").as_f64(),
                    Some(12.5),
                    "case {case:?}: the realising step reports the event's estimate, got {step}"
                );
                assert_eq!(
                    string_member(savings, "currency"),
                    "USD",
                    "case {case:?}: the estimate keeps the action's currency, got {step}"
                );
            } else {
                assert!(
                    savings.is_null(),
                    "case {case:?}: a preparatory step realises no savings, got {step}"
                );
            }
        }

        // Read back twice: a step's id and type are stored with its event, so a
        // second read reports exactly the same ones.
        let again = steps_ok(&router, &json!({"eventId": event_id}), action_type).await;
        assert_eq!(
            listed_step_ids(&again),
            listed_step_ids(&listed),
            "case {action_type:?}: a step's id is stored, not minted per read, got {again}"
        );
    }
}

/// An event whose `eventType` is outside the modelled set has no known step
/// sequence, so it reports an empty page rather than a guessed step.
#[tokio::test]
async fn an_event_with_no_stored_steps_lists_none() {
    let (router, engine) = emulator_with_engine();
    let event = stored_event(1, "SomeFutureEventType", now_secs() - 60, Vec::new());
    seed_event(&engine, &event);

    let listed = steps_ok(
        &router,
        &json!({"eventId": event.event_id.clone()}),
        "no-steps",
    )
    .await;
    assert_eq!(
        listed_step_types(&listed),
        Vec::<String>::new(),
        "case {:?}: an event with no stored steps answers an empty page, got {listed}",
        "no-steps"
    );
    assert!(
        member(&listed, "nextToken").is_null(),
        "case {:?}: an empty page carries no token, got {listed}",
        "no-steps"
    );
}

// ---------------------------------------------------------------------------
// What the clock makes due
// ---------------------------------------------------------------------------

/// Once the event's terminal status falls due, the step the run was ON takes
/// that outcome and is stamped completed, while a step still queued behind it
/// keeps `Ready` and no timestamps: nothing claims work that never ran.
#[tokio::test]
async fn the_step_the_run_was_on_takes_the_events_outcome() {
    let (router, engine) = emulator_with_engine();
    let action = recommended_action(
        "SettleAction001A",
        "SnapshotAndDeleteUnattachedEbsVolume",
        "vol-0settle01",
    );
    seed_action(&engine, &action);
    let event_id = start_event(&router, "SettleAction001A", "settling").await;

    // The transition is due a stored span after the start call, so a reader
    // before it still sees the step the run is on.
    let running = steps_ok(&router, &json!({"eventId": event_id.clone()}), "before-due").await;
    assert_eq!(
        listed_steps(&running)
            .iter()
            .map(|step| string_member(step, "stepStatus"))
            .collect::<Vec<_>>(),
        vec!["InProgress".to_string(), "Ready".to_string()],
        "case {:?}: before the terminal status is due the run is on its first step, got {running}",
        "before-due"
    );

    tokio::time::sleep(EVENT_SETTLEMENT_DELAY + Duration::from_millis(250)).await;

    let settled = steps_ok(&router, &json!({"eventId": event_id.clone()}), "after-due").await;
    let steps = listed_steps(&settled);
    assert_eq!(
        steps.len(),
        2,
        "case {:?}: settling changes no step's identity, got {settled}",
        "after-due"
    );
    assert_eq!(
        string_member(&steps[0], "stepStatus"),
        "Failed",
        "case {:?}: the step the run was on takes the event's outcome, got {settled}",
        "after-due"
    );
    assert!(
        !member(&steps[0], "completedTimestamp").is_null(),
        "case {:?}: the step that ended is stamped completed, got {settled}",
        "after-due"
    );
    assert_eq!(
        string_member(&steps[1], "stepStatus"),
        "Ready",
        "case {:?}: a step that never started keeps its queued status, got {settled}",
        "after-due"
    );
    assert!(
        member(&steps[1], "startTimestamp").is_null()
            && member(&steps[1], "completedTimestamp").is_null(),
        "case {:?}: a step that never started carries no timestamp, got {settled}",
        "after-due"
    );

    // The event reader and the step reader agree on what the run reached.
    let event = call_ok(
        &router,
        "GetAutomationEvent",
        &json!({"eventId": event_id}),
        "after-due",
    )
    .await;
    assert_eq!(
        string_member(&event, "eventStatus"),
        "Failed",
        "case {:?}: the event itself reports the same outcome, got {event}",
        "after-due"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Paging walks an event's steps exactly once, in stored order, whatever the
/// page size, and a token is only honoured for the event it was issued for.
#[tokio::test]
async fn paging_walks_every_step_exactly_once() {
    let (router, engine) = emulator_with_engine();
    // Nine steps, cycling the modelled `StepType` values, so the walk is over a
    // sequence longer than any single page a small `maxResults` allows.
    let step_types = ["CreateEbsSnapshot", "DeleteEbsVolume", "CreateEbsVolume", "ModifyEbsVolume"];
    let steps: Vec<StoredStep> = (0..9)
        .map(|index| stored_step(index, step_types[index % step_types.len()], "vol-0paged01"))
        .collect();
    let event = stored_event(2, "SnapshotAndDeleteUnattachedEbsVolume", now_secs() - 600, steps.clone());
    seed_event(&engine, &event);
    // A second event, so a token issued for one cannot resume the other.
    let other = stored_event(3, "UpgradeEbsVolumeType", now_secs() - 600, vec![stored_step(99, "ModifyEbsVolume", "vol-0other01")]);
    seed_event(&engine, &other);

    let all: Vec<String> = steps.iter().map(|step| step.step_id.clone()).collect();

    // Page sizes spanning the model's `maxResults` range (1..=100): one at a
    // time, sizes that do and do not divide the sequence, its exact length, and
    // more than the event holds.
    for page_size in [1_usize, 2, 4, 9, 100] {
        let case = format!("page-size-{page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut body = json!({"eventId": event.event_id.clone(), "maxResults": page_size});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let listed = steps_ok(&router, &body, &case).await;
            let page = listed_step_ids(&listed);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed maxResults, got {} steps",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: the walk must terminate, it has taken {pages} pages"
            );
            match member(&listed, "nextToken").as_str() {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case {case:?}: a token must match the modelled NextToken pattern, got \
                         {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must report every step exactly once, in stored order"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_size.min(all.len())),
            "case {case:?}: no page beyond the last, and no token on it"
        );
    }

    // A page size the caller did not send: the default page holds every step.
    let listed = steps_ok(
        &router,
        &json!({"eventId": event.event_id.clone()}),
        "default-page-size",
    )
    .await;
    assert_eq!(
        listed_step_ids(&listed),
        all,
        "case {:?}: the default page must hold every step, got {listed}",
        "default-page-size"
    );

    // A token issued for one event, and one this service never issued, are both
    // refused rather than answered with a page of something else.
    let first = steps_ok(
        &router,
        &json!({"eventId": event.event_id.clone(), "maxResults": 1}),
        "token-reuse",
    )
    .await;
    let token = string_member(&first, "nextToken");
    for (case, body) in &[
        (
            "token-of-another-event",
            json!({"eventId": other.event_id.clone(), "nextToken": token.clone()}),
        ),
        (
            "foreign-token",
            json!({"eventId": event.event_id.clone(), "nextToken": "notatokenweissued"}),
        ),
    ] {
        assert_refused(
            call(&router, "ListAutomationEventSteps", body).await,
            400,
            "InvalidParameterValueException",
            case,
        );
    }

    // The same token on the event it was issued for resumes the walk.
    let resumed = steps_ok(
        &router,
        &json!({"eventId": event.event_id.clone(), "nextToken": token}),
        "token-resumes",
    )
    .await;
    assert_eq!(
        listed_step_ids(&resumed),
        all[1..].to_vec(),
        "case {:?}: the token must resume where the first page stopped, got {resumed}",
        "token-resumes"
    );
}

// ---------------------------------------------------------------------------
// Which events have listable steps
// ---------------------------------------------------------------------------

/// A well-formed `eventId` no stored event carries is the modelled
/// `ResourceNotFoundException` (404) — and so is an event created before the
/// retrieval window, whose steps are no longer listable.
#[tokio::test]
async fn steps_are_listable_only_for_a_reachable_event() {
    let (router, engine) = emulator_with_engine();
    let base = now_secs();
    let window = i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a representable window");
    let step = |index: usize| vec![stored_step(index, "ModifyEbsVolume", "vol-0window01")];

    // Just inside the far edge of the retrieval window, right at it, and past it.
    let inside = stored_event(4, "UpgradeEbsVolumeType", base - 3600, step(4));
    let edge = stored_event(5, "UpgradeEbsVolumeType", base - window + 60, step(5));
    let outside = stored_event(6, "UpgradeEbsVolumeType", base - window - 60, step(6));
    let long_outside = stored_event(7, "UpgradeEbsVolumeType", base - 5 * window, step(7));
    for event in [&inside, &edge, &outside, &long_outside] {
        seed_event(&engine, event);
    }

    // (case, eventId, the steps the call must report) — `None` means refused 404.
    let cases: Vec<(&str, String, Option<Vec<String>>)> = vec![
        (
            "inside-the-window",
            inside.event_id.clone(),
            Some(vec![inside.steps[0].step_id.clone()]),
        ),
        (
            "just-inside-the-edge",
            edge.event_id.clone(),
            Some(vec![edge.steps[0].step_id.clone()]),
        ),
        ("just-outside-the-edge", outside.event_id.clone(), None),
        ("far-outside", long_outside.event_id.clone(), None),
        // Well-formed `EventId`s (`^[0-9A-Za-z]{16}$`) no event carries,
        // differing in case, digits and letters.
        ("absent-uppercase", "AAAAAAAAAAAAAAAA".to_string(), None),
        ("absent-digits", "0000000000000000".to_string(), None),
        ("absent-mixed", "zZ9aA0bB1cC2dD3e".to_string(), None),
    ];

    for (case, event_id, expected) in &cases {
        match expected {
            Some(step_ids) => {
                let listed = steps_ok(&router, &json!({"eventId": event_id}), case).await;
                assert_eq!(
                    &listed_step_ids(&listed),
                    step_ids,
                    "case {case:?}: a reachable event reports its stored steps, got {listed}"
                );
            }
            None => assert_refused(
                call(&router, "ListAutomationEventSteps", &json!({"eventId": event_id})).await,
                404,
                "ResourceNotFoundException",
                case,
            ),
        }
    }
}

/// An `eventId` outside the modelled `^[0-9A-Za-z]{16}$` is refused on the
/// parameter, which is a different answer from "no such event", and an absent
/// one is refused the same way.
#[tokio::test]
async fn a_malformed_event_id_is_refused_on_the_parameter() {
    let (router, _engine) = emulator_with_engine();

    // (case, request) — each violates `EventId`'s pattern, or omits the
    // required member.
    let cases: Vec<(&str, Value)> = vec![
        ("too-short", json!({"eventId": "abc"})),
        ("too-long", json!({"eventId": "AAAAAAAAAAAAAAAAA"})),
        ("hyphen", json!({"eventId": "AAAAAAA-AAAAAAAA"})),
        ("empty", json!({"eventId": ""})),
        ("missing", json!({})),
    ];

    for (case, body) in &cases {
        let (status, parsed) = call(&router, "ListAutomationEventSteps", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request the model refuses must answer HTTP 400, got {status} and \
             {parsed}"
        );
    }
}

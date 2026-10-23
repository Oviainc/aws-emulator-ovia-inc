// SPDX-License-Identifier: CC0-1.0

//! `GetDeploymentTarget`: the one deployment target a deployment holds under the
//! id the request names, over the wire.
//!
//! RULE: the answer is COMPUTED from the stored deployment record at the instant
//! of the call — the target carries the `TargetStatus` the deployment's own
//! clock-owed progress gives it and, as its `lifecycleEvents`, the lifecycle
//! event hook results that were reported against that deployment, in the order
//! its compute platform runs those hooks; a deployment the clock has not started
//! yet has no target state to report at all.
//!
//! What a target reads back as across the compute platforms, the ordering the
//! batch reader answers in and the refusals both readers share are pinned in
//! [`super::deployment_targets`]; the cases here are the two dimensions that
//! suite holds still — the clock, and the hook results a deployment accumulates.
//!
//! The harness is the one [`super::codedeploy_lifecycle`] drives the rest of this
//! service with, and the fleet helpers are [`super::list_deployment_targets`]';
//! only the cases here are new.

use std::time::Instant;

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator, POLL_DEADLINE, POLL_INTERVAL};
use crate::deployment_targets::{carried, deployable};
use crate::list_deployment_targets::{deploy, force_status, group_with, register};

/// The lifecycle hooks of an Amazon ECS deployment, in the order
/// `PutLifecycleEventHookExecutionStatus` documents them
/// (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatus")`:
/// "For Amazon ECS deployments, the available lifecycle hooks are BeforeInstall,
/// AfterInstall, AfterAllowTestTraffic, BeforeAllowTraffic, and
/// AfterAllowTraffic").
const ECS_HOOKS: &[&str] = &[
    "BeforeInstall",
    "AfterInstall",
    "AfterAllowTestTraffic",
    "BeforeAllowTraffic",
    "AfterAllowTraffic",
];

/// What one `GetDeploymentTarget` call reported: the wire error code of a
/// refusal, or the `TargetStatus` the answered target carries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observed {
    Refused(String),
    Status(String),
}

/// Read `target_id` of `deployment_id`, answering what the call reported and —
/// when it answered a target — the instant that target last changed.
async fn observe(
    emulator: &Emulator,
    case: &str,
    deployment_id: &str,
    target_id: &str,
) -> (Observed, Option<f64>) {
    let (status, body) = emulator
        .call(
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": target_id}),
        )
        .await;
    if status != 200 {
        return (Observed::Refused(error_code(&body)), None);
    }
    let target = carried(case, &body["deploymentTarget"]).1;
    (
        Observed::Status(
            target["status"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ),
        target["lastUpdatedAt"].as_f64(),
    )
}

/// Read `target_id` of `deployment_id` and require a 200, answering the one
/// per-type member the target is carried in.
async fn target_of(emulator: &Emulator, case: &str, deployment_id: &str, target_id: &str) -> Value {
    let answered = emulator
        .ok(
            "GetDeploymentTarget",
            json!({"deploymentId": deployment_id, "targetId": target_id}),
        )
        .await;
    carried(case, &answered["deploymentTarget"]).1
}

/// Assert `target` carries exactly the lifecycle events `expected` names, as
/// `(lifecycle event name, status)` pairs in that order.
fn assert_events(case: &str, target: &Value, expected: &[(&str, &str)]) {
    if expected.is_empty() {
        assert!(
            target["lifecycleEvents"].is_null(),
            "case {case:?}: expected no lifecycle events on a target no hook result was \
             reported for, got {target:?}"
        );
        return;
    }
    let events = target["lifecycleEvents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        events.len(),
        expected.len(),
        "case {case:?}: expected one lifecycle event per reported hook result {expected:?}, \
         got {target:?}"
    );
    for (event, (name, status)) in events.iter().zip(expected) {
        assert_eq!(
            event["lifecycleEventName"].as_str(),
            Some(*name),
            "case {case:?}: expected the hook {name} its platform runs at this position, \
             got {event:?}"
        );
        assert_eq!(
            event["status"].as_str(),
            Some(*status),
            "case {case:?}: expected the {status} result that was reported for {name}, \
             got {event:?}"
        );
        assert!(
            event["endTime"].as_f64().is_some(),
            "case {case:?}: expected the instant the {name} event ended, got {event:?}"
        );
        // A report says what the event ENDED as and when; when it started is not
        // something a report carries, so there is nothing to report it from.
        assert!(
            event["startTime"].is_null(),
            "case {case:?}: expected no start instant for an event only a result was reported \
             for, got {event:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The clock
// ---------------------------------------------------------------------------

/// RULE: a target's reported state is its deployment's own clock-owed progress —
/// before the deployment starts there is no target state to report
/// (`DeploymentNotStartedException`), then the target is being deployed to, then
/// it finished with the deployment — and the instant the target last changed is
/// the instant the deployment last moved, so it is stable across reads of one
/// state and advances when the state does.
#[tokio::test]
async fn a_targets_state_walks_its_deployments_clock_owed_progress() {
    let case = "an EC2/on-premises deployment polled from the instant it was created";
    let emulator = Emulator::new();
    register(&emulator, "host-a", &[("Fleet", "deployable")]).await;
    group_with(&emulator, "clock-app", "Server", &deployable()).await;
    let deployment_id = deploy(&emulator, "clock-app").await;

    // The walk a target of a deployment nobody stops takes. Each step is owed by
    // the CLOCK and not by the poll, so a poll sees a prefix-consistent walk of
    // this order however fast the machine is — and never a step backwards.
    let walk = [
        Observed::Refused("DeploymentNotStartedException".to_string()),
        Observed::Status("InProgress".to_string()),
        Observed::Status("Succeeded".to_string()),
    ];
    // Every observation, as its index in the walk and the instant the target
    // reported it last changed at.
    let mut seen: Vec<(usize, Option<f64>)> = Vec::new();
    let mut reached = 0usize;
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (observed, last_updated) = observe(&emulator, case, &deployment_id, "host-a").await;
        let index = walk
            .iter()
            .position(|step| *step == observed)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected one of {walk:?}, got {observed:?}")
            });
        assert!(
            index >= reached,
            "case {case:?}: expected the target never to go back from {:?}, got {observed:?}",
            walk[reached]
        );
        reached = index;
        seen.push((index, last_updated));
        if reached + 1 == walk.len() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the walk to reach {:?} within {POLL_DEADLINE:?}, got \
             {seen:?}",
            walk[walk.len() - 1]
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Reading the target did not move it: the instant it reports is the one the
    // deployment moved at, so every read of one state answers the same instant,
    // and a later state's instant is later.
    let mut previous: Option<(usize, f64)> = None;
    for (index, last_updated) in &seen {
        let Some(instant) = last_updated else {
            assert_eq!(
                *index, 0,
                "case {case:?}: expected only the refused step to carry no instant, got {seen:?}"
            );
            continue;
        };
        if let Some((previous_index, previous_instant)) = previous {
            if previous_index == *index {
                assert_eq!(
                    *instant, previous_instant,
                    "case {case:?}: expected two reads of the {:?} target to answer the same \
                     instant, got {seen:?}",
                    walk[*index]
                );
            } else {
                assert!(
                    *instant > previous_instant,
                    "case {case:?}: expected the instant to advance with the state into {:?}, \
                     got {seen:?}",
                    walk[*index]
                );
            }
        }
        previous = Some((*index, *instant));
    }
    assert!(
        seen.iter().any(|(index, _)| *index + 1 == walk.len()),
        "case {case:?}: expected the walk to end on {:?}, got {seen:?}",
        walk[walk.len() - 1]
    );
}

// ---------------------------------------------------------------------------
// The lifecycle events a target accumulates
// ---------------------------------------------------------------------------

/// RULE: a target's `lifecycleEvents` are the lifecycle event hook results
/// reported against its deployment, one event per report, named for the hook its
/// compute platform runs at that position — so a target nothing was reported for
/// carries none, and a reported failure is the result the event carries and the
/// status the target then holds.
#[tokio::test]
async fn a_targets_lifecycle_events_are_the_hook_results_reported_for_its_deployment() {
    // Rows differ along the compute platform whose hooks a validation function
    // reports (and so along the target type and the id form), along HOW MANY
    // results were reported, and along the result reported.
    struct Case<'a> {
        case: &'a str,
        application: &'a str,
        platform: &'a str,
        extras: Value,
        fleet: &'a [&'a str],
        target_id: &'a str,
        /// The result each hook execution reports, in order; the hook each one
        /// lands on is its platform's, so `ECS_HOOKS` names them.
        reported: &'a [&'a str],
        /// The `TargetStatus` the target holds once every report was made.
        expected_status: &'a str,
    }
    let ecs_service = json!({"ecsServices": [{"clusterName": "prod", "serviceName": "web"}]});
    let cases: &[Case] = &[
        Case {
            case: "an Amazon ECS deployment whose every hook passed",
            application: "hooks-all-app",
            platform: "ECS",
            extras: ecs_service.clone(),
            fleet: &[],
            target_id: "prod:web",
            reported: &["Succeeded"; 5],
            expected_status: "InProgress",
        },
        Case {
            case: "an Amazon ECS deployment two hooks in",
            application: "hooks-some-app",
            platform: "ECS",
            extras: ecs_service.clone(),
            fleet: &[],
            target_id: "prod:web",
            reported: &["Succeeded", "Succeeded"],
            expected_status: "InProgress",
        },
        Case {
            case: "an Amazon ECS deployment whose first hook failed",
            application: "hooks-failed-app",
            platform: "ECS",
            extras: ecs_service.clone(),
            fleet: &[],
            target_id: "prod:web",
            reported: &["Failed"],
            // A validation function that reports a failure fails the deployment
            // it was validating, and a target of a failed deployment failed with
            // it.
            expected_status: "Failed",
        },
        Case {
            case: "an Amazon ECS deployment nothing was reported for",
            application: "hooks-none-app",
            platform: "ECS",
            extras: ecs_service.clone(),
            fleet: &[],
            target_id: "prod:web",
            reported: &[],
            expected_status: "InProgress",
        },
        Case {
            // EC2/on-premises lifecycle events are run by the CodeDeploy agent
            // rather than by a Lambda validation function, so nothing reports a
            // result for them and the target carries no event.
            case: "an EC2/on-premises deployment, whose hooks the agent runs",
            application: "hooks-agent-app",
            platform: "Server",
            extras: deployable(),
            fleet: &["host-a"],
            target_id: "host-a",
            reported: &[],
            expected_status: "InProgress",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for instance in case.fleet {
            register(&emulator, instance, &[("Fleet", "deployable")]).await;
        }
        group_with(&emulator, case.application, case.platform, &case.extras).await;
        let deployment_id = deploy(&emulator, case.application).await;
        // The deployment is held at `InProgress` so the row's reports are made
        // against a deployment that still has lifecycle events to report on,
        // whatever the machine's own timing: the clock's completion is what ends
        // them, and that is pinned by `lifecycle_event_hook_status`.
        force_status(&emulator, &deployment_id, "InProgress", true);

        let mut expected: Vec<(&str, &str)> = Vec::new();
        for (position, reported) in case.reported.iter().enumerate() {
            // Before this report, the target carries exactly the results already
            // reported — so each event appears with its report and not before.
            let target = target_of(&emulator, case.case, &deployment_id, case.target_id).await;
            assert_events(case.case, &target, &expected);

            emulator
                .ok(
                    "PutLifecycleEventHookExecutionStatus",
                    json!({
                        "deploymentId": deployment_id,
                        "lifecycleEventHookExecutionId": format!("hook-{position}"),
                        "status": reported,
                    }),
                )
                .await;
            expected.push((ECS_HOOKS[position], reported));
        }

        let target = target_of(&emulator, case.case, &deployment_id, case.target_id).await;
        assert_events(case.case, &target, &expected);
        assert_eq!(
            target["status"].as_str(),
            Some(case.expected_status),
            "case {:?}: expected the target status {}, got {target:?}",
            case.case,
            case.expected_status
        );
        assert_eq!(
            target["targetId"].as_str(),
            Some(case.target_id),
            "case {:?}: expected the target id asked for, got {target:?}",
            case.case
        );
        assert_eq!(
            target["deploymentId"].as_str(),
            Some(deployment_id.as_str()),
            "case {:?}: expected the id of the deployment it belongs to, got {target:?}",
            case.case
        );
    }
}

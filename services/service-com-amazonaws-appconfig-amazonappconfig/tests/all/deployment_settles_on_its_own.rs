// SPDX-License-Identifier: CC0-1.0

//! A deployment SETTLES: nobody has to stop it for it to reach a terminal state.
//!
//! The rule under test: a deployment is paced by the strategy it was started with, and
//! every stage that pacing schedules is owed by the CLOCK — so a deployment whose strategy
//! declares a rollout window is in flight when `StartDeployment` answers, a deployment
//! whose strategy also declares a final bake time bakes after that rollout, and either one
//! reaches `COMPLETE` on its own a bounded span later, without a caller stopping it; a
//! strategy that declares neither has nothing left to wait for, so its deployment is
//! already `COMPLETE` when the start answers
//! (`smithy_query(query="operation",operation="StartDeployment,GetDeployment")`: both
//! report `State` as a `DeploymentState`, `PercentageComplete`, `CompletedAt` and the
//! `EventLog` the stages write).
//!
//! The rows span what the rule generalises over: whether the strategy declares a rollout
//! window, whether it declares a final bake time, and the magnitude of each — both
//! published bounds of `MinutesBetween0And24Hours` (`range 0..1440`) and a value between
//! them — because a deployment must settle whatever the declared minutes are. Every
//! assertion names its case and prints what it read.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, router,
};
use crate::extension_action_points::{
    create_version, get_deployment, given_strategy, start_deployment,
};

/// How long a row waits for the deployment it started to settle before failing.
///
/// A ceiling on the WAIT, not a statement about the span a stage takes: a deployment that
/// has not settled by here is one nothing is moving, which is the defect this suite pins.
const SETTLE_BY: Duration = Duration::from_secs(60);

/// How often a row re-reads the deployment while waiting for it to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `DeploymentState` values a deployment stops at: "COMPLETE", a rollback that
/// finished, and a revert of one that had completed. The states this suite waits for.
const TERMINAL_STATES: &[&str] = &["COMPLETE", "ROLLED_BACK", "REVERTED"];

/// One row: the case, the strategy's declared rollout minutes and final bake minutes, the
/// state `StartDeployment` must answer, and the deployment-event types the stages the
/// strategy schedules must have logged by the time it settles.
struct Row {
    case: &'static str,
    duration_in_minutes: i32,
    bake_in_minutes: i32,
    state_at_start: &'static str,
    logged_events: &'static [&'static str],
}

/// Poll `GetDeployment` until the deployment reports a terminal state, and answer the
/// document that reported it. Fails the row — naming it and the last state read — when
/// nothing settles within [`SETTLE_BY`].
async fn settled(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    environment: &str,
    number: i32,
) -> Value {
    let deadline = Instant::now() + SETTLE_BY;
    loop {
        let read = get_deployment(router, application, environment, number).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetDeployment expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let document = read.json();
        let state = document
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if TERMINAL_STATES.contains(&state.as_str()) {
            return document;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: deployment {number} did not reach a terminal state within {}s; \
             last state {state:?}",
            SETTLE_BY.as_secs()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `EventType` of every entry in a deployment's event log.
fn logged(document: &Value) -> BTreeSet<String> {
    document
        .get("EventLog")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("EventType").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_deployment_reaches_a_terminal_state_whatever_pacing_its_strategy_declares() {
    let router = router();
    let application = create_application(&router, "settling-app").await;
    let profile = create_profile(&router, &application, "settling-profile").await;
    let seeded = create_version(
        &router,
        &application,
        &profile,
        "application/json",
        br#"{"enabled":true}"#,
    )
    .await;
    assert_eq!(
        seeded.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        seeded.status,
        String::from_utf8_lossy(&seeded.body)
    );

    let rows = [
        Row {
            case: "neither a rollout window nor a bake time",
            duration_in_minutes: 0,
            bake_in_minutes: 0,
            state_at_start: "COMPLETE",
            logged_events: &["DEPLOYMENT_STARTED", "DEPLOYMENT_COMPLETED"],
        },
        Row {
            case: "a rollout window and no bake time",
            duration_in_minutes: 1,
            bake_in_minutes: 0,
            state_at_start: "DEPLOYING",
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "a bake time and no rollout window",
            duration_in_minutes: 0,
            bake_in_minutes: 1,
            state_at_start: "BAKING",
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "a rollout window and a bake time",
            duration_in_minutes: 30,
            bake_in_minutes: 10,
            state_at_start: "DEPLOYING",
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
        Row {
            case: "both at the published upper bound of 1440 minutes",
            duration_in_minutes: 1440,
            bake_in_minutes: 1440,
            state_at_start: "DEPLOYING",
            logged_events: &[
                "DEPLOYMENT_STARTED",
                "PERCENTAGE_UPDATED",
                "BAKE_TIME_STARTED",
                "DEPLOYMENT_COMPLETED",
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One environment per row: an environment holds one deployment at a time.
        let environment =
            create_environment(&router, &application, &format!("settling-env-{index}")).await;
        let strategy = given_strategy(
            &router,
            &format!("settling-{index}"),
            row.duration_in_minutes,
            row.bake_in_minutes,
        )
        .await;
        let started = start_deployment(
            &router,
            &application,
            &environment,
            &serde_json::json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            started.status,
            201,
            "case {:?}: StartDeployment expected 201, got {} / {}",
            row.case,
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let start_document = started.json();
        assert_eq!(
            start_document.get("State").and_then(Value::as_str),
            Some(row.state_at_start),
            "case {:?}: expected State {:?} when the start answers, got {start_document}",
            row.case,
            row.state_at_start
        );
        // The declared minutes are the strategy's and are reported back as declared: the
        // pacing of a stage is not a value a reader of the deployment is told about.
        assert_eq!(
            start_document
                .get("DeploymentDurationInMinutes")
                .and_then(Value::as_i64),
            Some(i64::from(row.duration_in_minutes)),
            "case {:?}: expected the declared duration, got {start_document}",
            row.case
        );
        assert_eq!(
            start_document
                .get("FinalBakeTimeInMinutes")
                .and_then(Value::as_i64),
            Some(i64::from(row.bake_in_minutes)),
            "case {:?}: expected the declared bake time, got {start_document}",
            row.case
        );
        let number = start_document
            .get("DeploymentNumber")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: StartDeployment answered no DeploymentNumber, got \
                     {start_document}",
                    row.case
                )
            }) as i32;

        let document = settled(&router, row.case, &application, &environment, number).await;
        assert_eq!(
            document.get("State").and_then(Value::as_str),
            Some("COMPLETE"),
            "case {:?}: a deployment nobody stopped completes, got {document}",
            row.case
        );
        assert_eq!(
            document.get("PercentageComplete").and_then(Value::as_f64),
            Some(100.0),
            "case {:?}: a completed deployment reached every target, got {document}",
            row.case
        );
        assert!(
            document.get("CompletedAt").is_some(),
            "case {:?}: a completed deployment reports when it completed, got {document}",
            row.case
        );
        let events = logged(&document);
        for expected in row.logged_events {
            assert!(
                events.contains(*expected),
                "case {:?}: expected the stage {expected:?} on the event log, got {events:?}",
                row.case
            );
        }
        // A stage the strategy does not schedule logs nothing: no bake time, no bake event.
        if row.bake_in_minutes == 0 {
            assert!(
                !events.contains("BAKE_TIME_STARTED"),
                "case {:?}: a strategy with no bake time bakes nothing, got {events:?}",
                row.case
            );
        }

        // The environment is released by the deployment settling, so the next deployment
        // into it is accepted rather than refused as a conflict.
        let again = start_deployment(
            &router,
            &application,
            &environment,
            &serde_json::json!({
                "DeploymentStrategyId": strategy,
                "ConfigurationProfileId": profile,
                "ConfigurationVersion": "1",
            }),
        )
        .await;
        assert_eq!(
            again.status,
            201,
            "case {:?}: a settled environment accepts the next deployment, got {} / {}",
            row.case,
            again.status,
            String::from_utf8_lossy(&again.body)
        );
    }
}

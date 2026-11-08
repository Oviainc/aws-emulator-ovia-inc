// SPDX-License-Identifier: CC0-1.0

//! RULE: each entry of `Experiment$actions` carries its own `state` ({`status`, `reason`}),
//! `startTime` and `endTime`, and they report where the RUN is in that action's own span:
//! an action is `pending` until its span begins, `running` inside it, `completed` once it
//! has elapsed, and — for a run that ended before an action's span did — `stopped` for the
//! action that was live when the stop landed and `cancelled` for one that never began.
//!
//! `ExperimentAction` models `actionId`, `description`, `parameters`, `targets`,
//! `startAfter`, `state`, `startTime` and `endTime`, and `ExperimentActionStatus` is
//! `pending | initiating | running | completed | cancelled | stopping | stopped | failed |
//! skipped` (`smithy_query(query="shape",shape="ExperimentAction,ExperimentActionStatus")`),
//! so a client reads `experiment.actions.<name>.state.status` to see where one action is.
//! The span itself is the `startAfter` graph with each action's own `duration`, which is
//! what the experiment's run span is computed from — so the spans are the run, placed on
//! the emulator's compressed run clock.
//!
//! The observations differ along everything the rule generalises over: where in the run
//! they are taken (before it begins, inside the first action's span, after the run ended),
//! which action they are about (the one that runs first and the one that waits for it), and
//! how the run ENDED (elapsed, stopped mid-flight, or failed before any action ran — where
//! nothing was carried out, so no progress is claimed for anything). An action the
//! experiment options SKIP reports the `skipped` state instead, which
//! `experiment_options_decide_the_run.rs` owns.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, poll_until_final, start_experiment, stop_experiment,
    token,
};

/// The wait each action of the chained template carries. Compressed (one modelled minute
/// per second of real time) the two of them make a run of about ten seconds, so an
/// observation taken a second in lands inside the FIRST action's span.
const ACTION_DURATION: &str = "PT5M";

/// How far into the run the "inside the first action's span" observation is taken: past the
/// transition the start owes (half a second) and well inside the first action's own span.
const INSIDE_THE_FIRST_SPAN: Duration = Duration::from_millis(1_500);

/// How long a row waits for its run to reach a terminal status: the whole compressed run
/// window, the transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// A template whose two actions run one after the other: `second` waits for `first`, so the
/// run is the two spans end to end.
async fn chained_template(router: &MultiServiceRouter, label: &str, targets: Value) -> String {
    let created = create_template(
        router,
        json!({
            "clientToken": token(&format!("action-state-template-{label}")),
            "description": format!("per-action state: {label}"),
            "roleArn": "arn:aws:iam::000000000000:role/fis-action-state",
            "stopConditions": [{ "source": "none" }],
            "targets": targets,
            "actions": {
                "first": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": ACTION_DURATION }
                },
                "second": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": ACTION_DURATION },
                    "startAfter": ["first"]
                }
            }
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no template id in {:?}", created.body))
        .to_string()
}

/// Start an experiment from `template_id` and report the experiment the start answered
/// with, and its id.
async fn start(router: &MultiServiceRouter, label: &str, template_id: &str) -> (Value, String) {
    let answer = start_experiment(
        router,
        json!({
            "clientToken": token(&format!("action-state-start-{label}")),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {label:?}: expected the start to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    let experiment = answer.member("experiment").clone();
    let id = experiment["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no experiment id in {experiment:?}"))
        .to_string();
    (experiment, id)
}

/// The status action `name` reports on `experiment`.
fn action_status(case: &str, experiment: &Value, name: &str) -> String {
    experiment["actions"][name]["state"]["status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: action {name:?} reports no state.status: {experiment:?}")
        })
        .to_string()
}

/// Whether action `name` reports the time member `member` on `experiment`.
fn has_time(experiment: &Value, name: &str, member: &str) -> bool {
    experiment["actions"][name][member].is_number()
}

#[tokio::test]
async fn each_action_reports_where_the_run_is_in_its_own_span() {
    let router = emulator();
    let template_id = chained_template(&router, "elapsed", json!({})).await;
    let (accepted, id) = start(&router, "elapsed", &template_id).await;

    // (where the observation is taken, what `first` reports, what `second` reports)
    let mut observations: Vec<(&str, String, String)> = Vec::new();

    // Before the run begins: the start's own answer, where the experiment is still
    // `initiating` and neither action has begun.
    observations.push((
        "the start's own answer",
        action_status("before the run", &accepted, "first"),
        action_status("before the run", &accepted, "second"),
    ));
    for name in ["first", "second"] {
        for member in ["startTime", "endTime"] {
            assert!(
                !has_time(&accepted, name, member),
                "case before the run: an action that has not begun claims no {member}, \
                 got {:?}",
                accepted["actions"][name]
            );
        }
    }

    tokio::time::sleep(INSIDE_THE_FIRST_SPAN).await;
    let inside = get_experiment(&router, &id)
        .await
        .member("experiment")
        .clone();
    assert_eq!(
        inside["state"]["status"].as_str().unwrap_or_default(),
        "running",
        "case inside the first span: expected the experiment to be running, got {inside:?}"
    );
    observations.push((
        "inside the first action's span",
        action_status("inside the first span", &inside, "first"),
        action_status("inside the first span", &inside, "second"),
    ));
    assert!(
        has_time(&inside, "first", "startTime"),
        "case inside the first span: the live action reports when it started, got {:?}",
        inside["actions"]["first"]
    );
    assert!(
        !has_time(&inside, "first", "endTime"),
        "case inside the first span: an action still running claims no endTime, got {:?}",
        inside["actions"]["first"]
    );
    assert!(
        !has_time(&inside, "second", "startTime"),
        "case inside the first span: an action that waits for another claims no startTime, \
         got {:?}",
        inside["actions"]["second"]
    );

    let ended = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        ended["state"]["status"].as_str().unwrap_or_default(),
        "completed",
        "case after the run: expected the run to complete, got {ended:?}"
    );
    observations.push((
        "after the run ended",
        action_status("after the run", &ended, "first"),
        action_status("after the run", &ended, "second"),
    ));

    // The progression the rule states, asserted as a whole.
    let expected = [
        ("the start's own answer", "pending", "pending"),
        ("inside the first action's span", "running", "pending"),
        ("after the run ended", "completed", "completed"),
    ];
    for ((case, first, second), (expected_case, expected_first, expected_second)) in
        observations.iter().zip(expected)
    {
        assert_eq!(case, &expected_case, "the observations are out of order");
        assert_eq!(
            (first.as_str(), second.as_str()),
            (expected_first, expected_second),
            "case {case:?}: expected first={expected_first:?} and \
             second={expected_second:?}, got first={first:?} and second={second:?}"
        );
    }

    // And the spans are the run: the action that waits for another starts no earlier than
    // that one ended, and both lie inside the experiment's own window.
    let stamp = |name: &str, member: &str| -> f64 {
        ended["actions"][name][member].as_f64().unwrap_or_else(|| {
            panic!("case after the run: action {name:?} reports no {member}: {ended:?}")
        })
    };
    let run_start = ended["startTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case after the run: no startTime: {ended:?}"));
    let run_end = ended["endTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case after the run: no endTime: {ended:?}"));
    assert!(
        stamp("second", "startTime") >= stamp("first", "endTime"),
        "case after the run: expected second to start no earlier than first ended, \
         got {:?}",
        ended["actions"]
    );
    for name in ["first", "second"] {
        assert!(
            stamp(name, "startTime") >= run_start && stamp(name, "endTime") <= run_end,
            "case after the run: expected action {name:?} to lie inside the experiment's \
             own window ({run_start}..{run_end}), got {:?}",
            ended["actions"][name]
        );
    }
}

#[tokio::test]
async fn a_stop_mid_run_stops_the_live_action_and_cancels_the_one_that_never_began() {
    let router = emulator();
    let template_id = chained_template(&router, "stopped", json!({})).await;
    let (_, id) = start(&router, "stopped", &template_id).await;

    tokio::time::sleep(INSIDE_THE_FIRST_SPAN).await;
    let stopped = stop_experiment(&router, &id).await;
    assert_eq!(
        stopped.status, 200,
        "case stopped: expected the stop to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );

    let ended = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        ended["state"]["status"].as_str().unwrap_or_default(),
        "stopped",
        "case stopped: expected the run to be stopped, got {ended:?}"
    );
    assert_eq!(
        action_status("stopped", &ended, "first"),
        "stopped",
        "case stopped: expected the action the stop landed on to report stopped, got {:?}",
        ended["actions"]["first"]
    );
    assert_eq!(
        action_status("stopped", &ended, "second"),
        "cancelled",
        "case stopped: expected the action that never began to report cancelled, got {:?}",
        ended["actions"]["second"]
    );
    assert!(
        has_time(&ended, "first", "startTime"),
        "case stopped: the action the stop ended reports when it started, got {:?}",
        ended["actions"]["first"]
    );
    for member in ["startTime", "endTime"] {
        assert!(
            !has_time(&ended, "second", member),
            "case stopped: an action that never began claims no {member}, got {:?}",
            ended["actions"]["second"]
        );
    }
}

#[tokio::test]
async fn a_run_that_failed_before_any_action_began_claims_no_action_progress() {
    let router = emulator();
    // A target identified by TAG resolves to nothing here, and the documented default
    // `emptyTargetResolutionMode` (`fail`) ends such a run `failed` before any action is
    // carried out — so no action of it ever began, and none of them claims a progress.
    let targets = json!({
        "tagged": {
            "resourceType": "aws:iam:role",
            "resourceTags": { "Stage": "test" },
            "selectionMode": "ALL"
        }
    });
    let template_id = chained_template(&router, "failed", targets).await;
    let (_, id) = start(&router, "failed", &template_id).await;

    let ended = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
    assert_eq!(
        ended["state"]["status"].as_str().unwrap_or_default(),
        "failed",
        "case failed: expected the empty resolution to fail the run, got {ended:?}"
    );
    // The failure says which section of the template it is about, so a client can tell this
    // ending from a run that elapsed.
    assert_eq!(
        ended["state"]["error"]["location"].as_str(),
        Some("targets.tagged"),
        "case failed: expected the error to locate the target that resolved to nothing, \
         got {ended:?}"
    );
    for name in ["first", "second"] {
        assert!(
            ended["actions"][name]["state"].is_null(),
            "case failed: an action of a run that never got going claims no state, got {:?}",
            ended["actions"][name]
        );
        for member in ["startTime", "endTime"] {
            assert!(
                ended["actions"][name][member].is_null(),
                "case failed: an action that never began claims no {member}, got {:?}",
                ended["actions"][name]
            );
        }
    }
}

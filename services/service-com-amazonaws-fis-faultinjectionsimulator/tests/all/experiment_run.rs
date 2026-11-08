// SPDX-License-Identifier: CC0-1.0

//! What an experiment DOES between `StartExperiment` and its terminal status, end to end
//! over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. Two rules are under test:
//!
//! * RULE (the run): an experiment FIS starts runs. Whatever its actions are — a wait, an
//!   action that injects an API fault, an action that takes no `duration` at all — it is
//!   observable in `running` for the length of the longest path through its actions'
//!   durations and then reaches the terminal status its actions earned, with an `endTime`
//!   at the end of the run and the `startTime` the start stamped. An action's kind does
//!   not decide the outcome: nothing about the template refuses the run or ends it early.
//! * RULE (the pacing): that run is driven on a COMPRESSED clock, so each transient status
//!   is observable at least once and the terminal status becomes readable promptly — in
//!   particular, well before the modelled duration the actions named has elapsed in real
//!   time. A client (or a test) that starts an experiment and waits for it to finish must
//!   not have to block for the duration the template asked for, which for `aws:fis:wait`
//!   alone runs up to the documented 720 minutes.
//!
//! The rows differ along what those rules generalise over: which actions the template
//! carries (`aws:fis:wait`, `aws:fis:inject-api-internal-error`,
//! `aws:fis:inject-api-throttle-error`, `aws:cloudwatch:assert-alarm-state`), how many of
//! them there are and whether they are chained through `startAfter`, whether they name
//! targets, and the modelled duration they add up to (a single minute, two minutes, three
//! minutes through a chain, and none at all).
//!
//! What follows the run is asserted on the experiment the run left behind: `StopExperiment`
//! on an experiment in a final state is refused with `ValidationException` (HTTP 400) and a
//! message built from the request — it names that experiment — and changes nothing.
//!
//! # What "runs" means here
//!
//! The run this suite drives is the experiment's own LIFECYCLE — FIS's record of a run, on
//! a compressed clock — and nothing else. No action's effect on another service is
//! produced: no connection is opened, no process is spawned, nothing in the template is
//! interpreted, and a value the caller put in a parameter (a service name, an operation
//! name, an ARN) is read only as a span or as a resource name and is never dialled. That
//! limit, and what `completed` therefore does and does not claim, is stated in
//! `src/state.rs`'s module header; the last case below is the one that pins the parsing of
//! a caller's `duration` as TOTAL — a magnitude no `Duration` can hold names no span, so
//! the request is answered rather than aborted, and the run stays inside the compressed
//! window.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, send, start_experiment, stop_experiment, token,
    FALLBACK_REGION,
};

/// The window the emulator compresses a modelled run into, and the span it owes a
/// transition it chooses the length of. Spelled out here rather than imported from the
/// crate, so a wrong value in `src/state.rs` fails this suite.
const MIN_RUN_SPAN: Duration = Duration::from_secs(5);
/// See [`MIN_RUN_SPAN`].
const MAX_RUN_SPAN: Duration = Duration::from_secs(15);
/// See [`MIN_RUN_SPAN`].
const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// How long after the start the move to `running` must have been materialised: past
/// [`TRANSITION_DELAY`] and well inside [`MIN_RUN_SPAN`], so every row is still running
/// when it is read.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// How long a row may take to reach its terminal status: the whole compressed window, the
/// two transitions around it and a slack for a loaded machine. The pacing rule is what
/// makes this a bound at all — the modelled durations the rows name are minutes long.
const TERMINAL_BUDGET: Duration = Duration::from_secs(MAX_RUN_SPAN.as_secs() + 10);

/// A role ARN the emulator's IAM store holds no role for, which is what every row here
/// runs under: with no stored role there is nothing to show about its trust policy or its
/// permissions, so the run goes ahead. The permission rule is asserted against the real
/// IAM service in `core/router/tests/all/fis_execution_role_permissions.rs`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-run";

/// One row: a template, and the modelled length of the longest path through its actions.
struct Row {
    case: &'static str,
    /// The `targets` the template declares, for a row whose actions name targets.
    targets: Value,
    actions: Value,
    /// The seconds the row's actions add up to, as the `duration` parameters state them.
    /// The whole point of the pacing rule is that this is NOT how long the row takes.
    modelled: Duration,
}

/// The rows the two rules are asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "wait-of-one-minute",
            targets: json!({}),
            actions: json!({
                "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } }
            }),
            modelled: Duration::from_secs(60),
        },
        Row {
            // The action the recorded AWS run injects: an API fault against the calls made
            // with a targeted role. The experiment runs for the action's duration and ends
            // at the status the run earned, exactly as a wait-only one does.
            case: "api-internal-error-of-two-minutes",
            targets: json!({
                "roles": {
                    "resourceType": "aws:iam:role",
                    "resourceArns": ["arn:aws:iam::000000000000:role/fis-targeted"],
                    "selectionMode": "ALL"
                }
            }),
            actions: json!({
                "fault": {
                    "actionId": "aws:fis:inject-api-internal-error",
                    "parameters": {
                        "duration": "PT2M",
                        "service": "ec2",
                        "operations": "DescribeAvailabilityZones",
                        "percentage": "100"
                    },
                    "targets": { "Roles": "roles" }
                }
            }),
            modelled: Duration::from_secs(120),
        },
        Row {
            case: "a-throttle-fault-then-a-wait-chained",
            targets: json!({
                "roles": {
                    "resourceType": "aws:iam:role",
                    "resourceArns": ["arn:aws:iam::000000000000:role/fis-targeted"],
                    "selectionMode": "ALL"
                }
            }),
            actions: json!({
                "throttle": {
                    "actionId": "aws:fis:inject-api-throttle-error",
                    "parameters": {
                        "duration": "PT2M",
                        "service": "ec2",
                        "operations": "DescribeAvailabilityZones"
                    },
                    "targets": { "Roles": "roles" }
                },
                "afterwards": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": "PT1M" },
                    "startAfter": ["throttle"]
                }
            }),
            modelled: Duration::from_secs(180),
        },
        Row {
            // An action that takes no `duration` at all: it names no span, so the run is
            // the compressed floor — and it still passes through `running`.
            case: "assert-alarm-state-with-no-duration",
            targets: json!({}),
            actions: json!({
                "assert1": {
                    "actionId": "aws:cloudwatch:assert-alarm-state",
                    "parameters": {
                        "alarmArns": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:cpu-high",
                        "alarmStates": "INSUFFICIENT_DATA"
                    }
                }
            }),
            modelled: Duration::ZERO,
        },
    ]
}

/// The `startTime`/`endTime` an experiment reports, as epoch seconds.
fn stamp(experiment: &Value, member: &str) -> f64 {
    experiment[member]
        .as_f64()
        .unwrap_or_else(|| panic!("expected a {member} on {experiment:?}"))
}

#[tokio::test]
async fn a_started_experiment_runs_and_reaches_a_terminal_status_on_the_compressed_clock() {
    let router = emulator();
    let rows = rows();

    // Every row's experiment is started FIRST, so the rows' runs overlap instead of being
    // waited out one after another. The account's active-experiment quota is five, and
    // there are fewer rows than that.
    let mut experiments: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        let created = create_template(
            &router,
            json!({
                "clientToken": token(&format!("run-template-{}", row.case)),
                "description": format!("the run rule: {}", row.case),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": row.targets.clone(),
                "actions": row.actions.clone()
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the template to be created, got {} with {:?}",
            row.case, created.status, created.body
        );
        let template_id = created.member("experimentTemplate")["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("run-start-{}", row.case)),
                "experimentTemplateId": template_id
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected the start to be accepted, got {} with {:?}",
            row.case, started.status, started.body
        );
        let experiment = started.member("experiment").clone();
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "initiating",
            "case {:?}: expected a started experiment to be initiating, got {experiment:?}",
            row.case
        );
        let id = experiment["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no experiment id in {experiment:?}", row.case))
            .to_string();
        experiments.push((row, id));
    }

    // The transient `running` status is observable for EVERY row, whatever its actions
    // are: the run is at least the compressed floor long, so a client that polls a started
    // experiment sees it.
    tokio::time::sleep(PAST_TRANSITION).await;
    for (row, id) in &experiments {
        let answer = get_experiment(&router, id).await;
        let experiment = answer.member("experiment");
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "running",
            "case {:?}: expected the experiment to be running, got {:?}",
            row.case,
            answer.body
        );
        assert!(
            experiment["endTime"].is_null(),
            "case {:?}: a running experiment has not ended, got {:?}",
            row.case,
            answer.body
        );
    }

    // And each one reaches its terminal status inside the compressed window — which, for
    // every row, is sooner than the duration its actions modelled.
    for (row, id) in &experiments {
        let (experiment, elapsed) = poll_until_final(&router, id, TERMINAL_BUDGET).await;
        assert_eq!(
            experiment["state"]["status"].as_str().unwrap_or_default(),
            "completed",
            "case {:?}: expected the run to reach the status its actions earned, \
             got {experiment:?}",
            row.case
        );
        assert!(
            experiment["state"]["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {:?}: an experiment that ended says why, got {experiment:?}",
            row.case
        );
        // `ExperimentError` is the error information "when the action has failed", so a run
        // that completed reports none.
        assert!(
            experiment["state"]["error"].is_null(),
            "case {:?}: a completed experiment reports no error, got {experiment:?}",
            row.case
        );
        assert!(
            elapsed <= TERMINAL_BUDGET,
            "case {:?}: expected a terminal status within {TERMINAL_BUDGET:?}, got {elapsed:?}",
            row.case
        );
        // The pacing rule itself: the run is compressed, so the terminal status arrived
        // before the modelled duration would even have elapsed. The floor row models no
        // duration at all, so it is exempt from this comparison and bounded by the budget
        // above.
        if row.modelled > MIN_RUN_SPAN {
            assert!(
                elapsed < row.modelled,
                "case {:?}: expected the run to be compressed below its modelled \
                 {:?}, got {elapsed:?}",
                row.case,
                row.modelled
            );
        }

        // The stamps come from the record, not from whoever looked: the run ended at the
        // end of the compressed span after the start, and never before it began.
        let span = stamp(&experiment, "endTime") - stamp(&experiment, "startTime");
        assert!(
            span > 0.0,
            "case {:?}: expected endTime after startTime, got a span of {span}s \
             in {experiment:?}",
            row.case
        );
        let ceiling = (MAX_RUN_SPAN + TRANSITION_DELAY).as_secs_f64();
        assert!(
            span <= ceiling,
            "case {:?}: expected the reported run to be at most {ceiling}s, got {span}s \
             in {experiment:?}",
            row.case
        );
    }

    // What follows the run: a stop of an experiment in a final state is refused, with a
    // message naming that experiment, and leaves the record exactly as the run left it.
    for (row, id) in &experiments {
        let before = get_experiment(&router, id).await.member("experiment").clone();
        let refused = stop_experiment(&router, id).await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 stopping a completed experiment, got {} with {:?}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            refused.body
        );
        let message = refused.body["message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!("Cannot modify the state of an experiment in final state: {id}."),
            "case {:?}: expected the refusal to name the experiment, got {message:?}",
            row.case
        );
        let after = get_experiment(&router, id).await.member("experiment").clone();
        assert_eq!(
            after, before,
            "case {:?}: a refused stop must change nothing, got {after:?}",
            row.case
        );
    }
}

/// RULE: the run is the experiment's own, so the experiment an `actionsMode` of `skip-all`
/// starts still walks the lifecycle, and a second experiment started from the same template
/// runs independently of the first.
///
/// What that mode makes of the run itself — the actions it carries out, the durations it
/// spends and the failures it can earn — is asserted in
/// `experiment_options_decide_the_run.rs`; this case is about the two runs being two.
#[tokio::test]
async fn two_experiments_from_one_template_run_independently() {
    let router = emulator();

    let created = create_template(
        &router,
        json!({
            "clientToken": token("run-shared-template"),
            "description": "two experiments from one template",
            "roleArn": ROLE_ARN,
            "stopConditions": [{ "source": "none" }],
            "targets": {},
            "actions": {
                "fault": {
                    "actionId": "aws:fis:inject-api-internal-error",
                    "parameters": {
                        "duration": "PT1M",
                        "service": "ec2",
                        "operations": "DescribeAvailabilityZones"
                    }
                }
            }
        }),
    )
    .await;
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The rows differ along the `actionsMode` the start chooses, which is the one option
    // `StartExperiment` sets: the documented default and its other value.
    let mut ids = Vec::new();
    for (case, actions_mode) in [("run-all", "run-all"), ("skip-all", "skip-all")] {
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("run-shared-start-{case}")),
                "experimentTemplateId": template_id,
                "experimentOptions": { "actionsMode": actions_mode }
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the start to be accepted, got {} with {:?}",
            started.status, started.body
        );
        let experiment = started.member("experiment");
        assert_eq!(
            experiment["experimentOptions"]["actionsMode"]
                .as_str()
                .unwrap_or_default(),
            actions_mode,
            "case {case:?}: expected the experiment to carry the mode it was started with, \
             got {experiment:?}"
        );
        // And the mode is more than an echo: it decides whether the run carries the action
        // out at all, which the action's own state is what says — `skipped` for the action
        // nothing carries out, and the start of the run's own progression for the one it
        // does.
        let expected_status = if actions_mode == "skip-all" {
            "skipped"
        } else {
            "pending"
        };
        assert_eq!(
            experiment["actions"]["fault"]["state"]["status"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the action's state to say whether the run carries it \
             out, got {experiment:?}"
        );
        ids.push((
            case,
            experiment["id"].as_str().unwrap_or_default().to_string(),
        ));
    }
    assert_ne!(
        ids[0].1, ids[1].1,
        "two starts of one template are two experiments, got {ids:?}"
    );

    // One experiment's run is not the other's: stopping the first leaves the second
    // running, and only the stopped one carries a stop reason.
    let stopped = stop_experiment(&router, &ids[0].1).await;
    assert_eq!(
        stopped.status, 200,
        "expected the stop of the first experiment to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let first = get_experiment(&router, &ids[0].1).await;
    assert_eq!(
        first.member("experiment")["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "stopped",
        "expected the stopped experiment to have ended, got {:?}",
        first.body
    );
    let second = get_experiment(&router, &ids[1].1).await;
    assert_eq!(
        second.member("experiment")["state"]["status"]
            .as_str()
            .unwrap_or_default(),
        "running",
        "expected the other experiment to be running still, got {:?}",
        second.body
    );

    // And the template is untouched by either run: `ListExperiments` scoped to it reports
    // both experiments, which is the one record of what was started from it.
    let listed = send(
        &router,
        "GET",
        &format!("/experiments?experimentTemplateId={template_id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    let count = listed.member("experiments").as_array().map(Vec::len);
    assert_eq!(
        count,
        Some(2),
        "expected both experiments to be listed for the template, got {:?}",
        listed.body
    );

    let stopped = stop_experiment(&router, &ids[1].1).await;
    assert_eq!(
        stopped.status, 200,
        "expected the stop of the second experiment to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );
}

/// RULE: a `duration` this service cannot read as a fixed, representable span names no
/// span at all, so the action contributes nothing to the run and the experiment still runs
/// to a terminal status — the request is ANSWERED either way.
///
/// `aws:fis:wait`'s `duration` is range-checked when the template is created, but every
/// other action's is not (each action publishes its own range, which this service does not
/// hold), so the value reaches the run plan as the caller sent it. The rows differ along
/// why the value names no span: a magnitude past what a `Duration` can hold, in seconds and
/// in days and in every field at once; a span whose length depends on the calendar; and a
/// value that is not a duration at all.
#[tokio::test]
async fn a_duration_that_names_no_representable_span_is_answered_and_not_run() {
    let router = emulator();

    let rows: [(&str, &str); 6] = [
        ("more-seconds-than-a-duration-holds", "PT99999999999999999999S"),
        ("more-days-than-a-duration-holds", "P99999999999999999999D"),
        (
            "every-field-at-that-magnitude",
            "P99999999999999999999DT99999999999999999999H99999999999999999999M99999999999999999999S",
        ),
        ("a-calendar-span", "P1Y"),
        ("not-a-duration-at-all", "forever"),
        ("a-negative-span", "-PT5M"),
    ];

    for chunk in rows.chunks(3) {
        // Started in groups, so the rows' runs overlap rather than being waited out one
        // after another, and no group exceeds the account's active-experiment quota.
        let mut started_rows: Vec<(&str, String)> = Vec::new();
        for (case, duration) in chunk {
            let created = create_template(
                &router,
                json!({
                    "clientToken": token(&format!("unreadable-template-{case}")),
                    "description": format!("an unreadable duration: {case}"),
                    "roleArn": ROLE_ARN,
                    "stopConditions": [{ "source": "none" }],
                    "targets": {},
                    "actions": {
                        "fault": {
                            "actionId": "aws:fis:inject-api-internal-error",
                            "parameters": {
                                "duration": duration,
                                "service": "ec2",
                                "operations": "DescribeAvailabilityZones"
                            }
                        }
                    }
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: this service holds no range for a non-wait action's duration, \
                 so the create is accepted; got {} with {:?}",
                created.status, created.body
            );
            let template_id = created.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let started = start_experiment(
                &router,
                json!({
                    "clientToken": token(&format!("unreadable-start-{case}")),
                    "experimentTemplateId": template_id
                }),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected the start to be answered, got {} with {:?}",
                started.status, started.body
            );
            started_rows.push((
                case,
                started.member("experiment")["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }

        // The value named no span, so the run is the compressed floor: each experiment is
        // observably running and then reaches its terminal status inside the window,
        // instead of being unbounded.
        tokio::time::sleep(PAST_TRANSITION).await;
        for (case, id) in &started_rows {
            let running = get_experiment(&router, id).await;
            assert_eq!(
                running.member("experiment")["state"]["status"]
                    .as_str()
                    .unwrap_or_default(),
                "running",
                "case {case:?}: expected the experiment to be running, got {:?}",
                running.body
            );
        }
        for (case, id) in &started_rows {
            let (experiment, elapsed) = poll_until_final(&router, id, TERMINAL_BUDGET).await;
            assert_eq!(
                experiment["state"]["status"].as_str().unwrap_or_default(),
                "completed",
                "case {case:?}: expected the run to reach a terminal status, got {experiment:?}"
            );
            assert!(
                elapsed <= TERMINAL_BUDGET,
                "case {case:?}: expected a terminal status within {TERMINAL_BUDGET:?}, \
                 got {elapsed:?}"
            );
        }
    }
}

/// Poll `GetExperiment` until the experiment reaches a terminal status or `budget` runs
/// out, and report the experiment as it was last read beside how long that took.
async fn poll_until_final(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    budget: Duration,
) -> (Value, Duration) {
    const INTERVAL: Duration = Duration::from_millis(100);
    let began = Instant::now();
    loop {
        let experiment = get_experiment(router, id).await.member("experiment").clone();
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if ["completed", "failed", "stopped", "cancelled"].contains(&status.as_str())
            || began.elapsed() >= budget
        {
            return (experiment, began.elapsed());
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

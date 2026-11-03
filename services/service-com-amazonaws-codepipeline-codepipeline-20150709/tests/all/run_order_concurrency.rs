// SPDX-License-Identifier: CC0-1.0

//! What `ActionDeclaration$runOrder` ("the order in which actions are run") means for
//! WHEN each action of a stage runs, as `ListActionExecutions` reports it.
//!
//! The rule under test: the actions of one stage that share a run order run
//! CONCURRENTLY — they start at the same instant and none of them waits for a sibling,
//! so their `[startTime, lastUpdateTime]` windows overlap — while an action of a higher
//! run order starts only once EVERY action of the run order before it has finished, so
//! its window overlaps none of theirs. Which run order an action belongs to is the one
//! it declares, defaulted to 1 where it declares none.
//!
//! The rows differ along what the rule generalises over: how many actions share a run
//! order, how many run orders a stage declares, whether the order is declared or
//! defaulted, and which values the orders take (contiguous or not, starting at 1 or not,
//! and both published bounds of `ActionRunOrder`'s `@range(min: 1, max: 999)`).
//!
//! The harness is the execution plane's own (`tests/all/execution_engine.rs`), so every
//! assertion here goes through the router, over the wire the service speaks.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, source_stage, wait_for_terminal, Emulator,
};

/// The run order an action that declares none runs in: `ActionDeclaration$runOrder`
/// documents "The default is 1".
const DEFAULT_RUN_ORDER: i64 = 1;

/// One action's reported window: its name, its `startTime` and its `lastUpdateTime`,
/// the last two in epoch seconds.
type Window = (String, f64, f64);

/// How far apart two instants may be and still count as the SAME one.
///
/// The actions of one run order are started from one stored instant, so the values
/// reported for them are equal; this is only the slack of carrying them as JSON numbers
/// of epoch seconds, and it is two orders of magnitude below the span that separates two
/// run orders (`state::execution::ACTION_STEP`, 300 ms).
const SAME_INSTANT: f64 = 0.001;

/// The least a later run order may start after the previous one finished.
///
/// A step of the walk becomes due `state::execution::ACTION_STEP` (300 ms) after the step
/// before it finished; this is that span with slack for the rounding of a JSON epoch
/// number.
const STEP_FLOOR: f64 = 0.25;

/// One deploy action of the acting stage: it delivers the source artifact to its own
/// object, so each action of the stage performs real work of its own.
fn deploy_action(name: &str, run_order: Option<i64>) -> Value {
    let mut action = json!({
        "name": name,
        "actionTypeId": {
            "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
        },
        "configuration": {
            "BucketName": "deploy-bucket",
            "ObjectKey": format!("delivered/{name}"),
            "Extract": "false",
        },
        "inputArtifacts": [{"name": "SourceArtifact"}],
    });
    if let Some(run_order) = run_order {
        action["runOrder"] = json!(run_order);
    }
    action
}

/// The name this suite gives the action declared at `position`.
fn action_name(position: usize) -> String {
    format!("Dep{position}")
}

/// `startTime` and `lastUpdateTime`, in epoch seconds, of every action execution of
/// `execution`, by action name.
async fn windows(
    emulator: &Emulator,
    pipeline: &str,
    execution: &str,
) -> BTreeMap<String, (f64, f64)> {
    let listed = emulator
        .ok(
            "ListActionExecutions",
            &json!({
                "pipelineName": pipeline,
                "filter": {"pipelineExecutionId": execution},
            }),
        )
        .await;
    let mut collected = BTreeMap::new();
    for detail in listed["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = detail["actionName"].as_str().unwrap_or_default().to_string();
        let start = detail["startTime"].as_f64().unwrap_or_else(|| {
            panic!("an action execution reports when it started, got {detail}")
        });
        let end = detail["lastUpdateTime"].as_f64().unwrap_or_else(|| {
            panic!("an action execution reports when it last changed, got {detail}")
        });
        let status = detail["status"].as_str().unwrap_or_default();
        assert_eq!(
            status, "Succeeded",
            "the action {name} must have performed its work, got {detail}"
        );
        collected.insert(name, (start, end));
    }
    collected
}

#[tokio::test]
async fn the_actions_of_one_run_order_run_together_and_a_later_one_waits_for_them_all() {
    /// One row: the case and the `runOrder` each action of the acting stage declares
    /// (`None` for an action that declares none).
    type Case = (&'static str, Vec<Option<i64>>);

    let cases: Vec<Case> = vec![
        ("two actions declaring no run order", vec![None, None]),
        ("two actions sharing run order 1", vec![Some(1), Some(1)]),
        (
            "three actions sharing run order 2",
            vec![Some(2), Some(2), Some(2)],
        ),
        ("one action per run order", vec![Some(1), Some(2)]),
        (
            "a shared run order, then a later one",
            vec![Some(1), Some(1), Some(2)],
        ),
        (
            "a defaulted and a declared order 1 share it, then a later one",
            vec![None, Some(1), Some(2)],
        ),
        (
            "run orders need not be contiguous nor start at 1",
            vec![Some(7), Some(7), Some(9)],
        ),
        (
            "ActionRunOrder's published bounds (@range min 1, max 999)",
            vec![Some(1), Some(999)],
        ),
    ];

    for (index, (case, orders)) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("run-order-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"the source bytes")
            .await;

        let actions: Vec<Value> = orders
            .iter()
            .enumerate()
            .map(|(position, order)| deploy_action(&action_name(position), *order))
            .collect();
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        {"name": "Deploy", "actions": actions},
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start reports an execution id"))
            .to_string();

        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: every action's work is work this engine performs, got {answer}"
        );

        // The reported windows, grouped by the run order each action belongs to.
        let reported = windows(&emulator, &pipeline, &execution).await;
        let mut groups: BTreeMap<i64, Vec<Window>> = BTreeMap::new();
        for (position, order) in orders.iter().enumerate() {
            let name = action_name(position);
            let (start, end) = *reported.get(&name).unwrap_or_else(|| {
                panic!("case {case:?}: the action {name} has no record, got {reported:?}")
            });
            groups
                .entry(order.unwrap_or(DEFAULT_RUN_ORDER))
                .or_default()
                .push((name, start, end));
        }

        // Every action delivered its own object: the walk ran the work of each one and
        // not only of the one that happened to be reached first.
        for position in 0..orders.len() {
            let key = format!("delivered/{}", action_name(position));
            let (status, bytes) = emulator.get_object("deploy-bucket", &key).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {key} must have been delivered, got {status}"
            );
            assert_eq!(
                bytes, b"the source bytes",
                "case {case:?}: {key} must carry the source bytes"
            );
        }

        // Within one run order: one start instant for all of them, and windows that
        // overlap, because they run concurrently.
        for (order, members) in &groups {
            for (name, start, end) in members {
                assert!(
                    *end >= *start,
                    "case {case:?}: run order {order}: the window of {name} ends before it \
                     starts, got {start} to {end}"
                );
            }
            let earliest = members.iter().map(|(_, start, _)| *start).fold(f64::MAX, f64::min);
            for (name, start, _) in members {
                assert!(
                    (*start - earliest).abs() <= SAME_INSTANT,
                    "case {case:?}: run order {order}: {name} must start with the rest of its \
                     run order (at {earliest}), got {start}"
                );
            }
            for (name, start, end) in members {
                for (other, other_start, other_end) in members {
                    assert!(
                        *start <= *other_end && *other_start <= *end,
                        "case {case:?}: run order {order}: the windows of {name} ({start} to \
                         {end}) and {other} ({other_start} to {other_end}) must overlap"
                    );
                }
            }
        }

        // Between run orders: the later one starts a step after the LAST of the earlier
        // one finished, so no window of the two overlaps.
        let ordered: Vec<(i64, Vec<Window>)> = groups.into_iter().collect();
        for pair in ordered.windows(2) {
            let (earlier_order, earlier) = &pair[0];
            let (later_order, later) = &pair[1];
            let finished = earlier
                .iter()
                .map(|(_, _, end)| *end)
                .fold(f64::MIN, f64::max);
            for (name, start, _) in later {
                assert!(
                    *start - finished >= STEP_FLOOR,
                    "case {case:?}: run order {later_order}: {name} must start at least \
                     {STEP_FLOOR}s after run order {earlier_order} finished (at {finished}), \
                     got {start}"
                );
            }
            for (name, _, end) in earlier {
                for (other, other_start, _) in later {
                    assert!(
                        *other_start > *end,
                        "case {case:?}: the window of {other} (run order {later_order}, from \
                         {other_start}) must not overlap {name} (run order {earlier_order}, \
                         until {end})"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_shared_run_order_starts_its_actions_closer_together_than_two_run_orders_do() {
    // Two pipelines declaring the SAME two actions and differing only in whether the two
    // share a run order: the shared one starts them together, the split one a step apart.
    let mut gaps: BTreeMap<&str, f64> = BTreeMap::new();
    for (case, orders) in [
        ("shared run order", vec![Some(1), Some(1)]),
        ("one action per run order", vec![Some(1), Some(2)]),
    ] {
        let emulator = Emulator::new();
        let pipeline = format!("gap-{}", case.replace(' ', "-"));
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket("source-bucket").await;
        emulator.bucket("deploy-bucket").await;
        emulator
            .put_object("source-bucket", "source.zip", b"the source bytes")
            .await;

        let actions: Vec<Value> = orders
            .iter()
            .enumerate()
            .map(|(position, order)| deploy_action(&action_name(position), *order))
            .collect();
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    json!([
                        source_stage("source-bucket", "source.zip", "SourceArtifact"),
                        {"name": "Deploy", "actions": actions},
                    ]),
                    None,
                ),
            )
            .await;
        let started = emulator
            .ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start reports an execution id"))
            .to_string();
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: every action's work is work this engine performs, got {answer}"
        );

        let reported = windows(&emulator, &pipeline, &execution).await;
        let starts: Vec<f64> = (0..orders.len())
            .map(|position| {
                let name = action_name(position);
                reported
                    .get(&name)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the action {name} has no record, got {reported:?}")
                    })
                    .0
            })
            .collect();
        let spread = starts.iter().fold(f64::MIN, |most, start| most.max(*start))
            - starts.iter().fold(f64::MAX, |least, start| least.min(*start));
        gaps.insert(case, spread);
    }

    let shared = gaps["shared run order"];
    let split = gaps["one action per run order"];
    assert!(
        shared <= SAME_INSTANT,
        "case \"shared run order\": two actions of one run order start together, got them \
         {shared}s apart"
    );
    assert!(
        shared < split,
        "two actions of one run order must start closer together than two run orders do: \
         got {shared}s shared against {split}s split"
    );
    assert!(
        split >= STEP_FLOOR,
        "case \"one action per run order\": a later run order starts at least {STEP_FLOOR}s \
         after the one before it, got {split}s"
    );
}

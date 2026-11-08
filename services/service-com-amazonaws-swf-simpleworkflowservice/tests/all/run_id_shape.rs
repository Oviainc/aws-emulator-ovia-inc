// SPDX-License-Identifier: CC0-1.0

//! The shape of the `runId` `StartWorkflowExecution` mints, end to end over the
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: a `runId` is an opaque, server-generated id of the
//! class Amazon SWF's run ids are written in — base64 text, i.e. drawn from
//! `A-Z a-z 0-9 + / =` and decodable as standard base64 over enough bytes to be
//! unguessable — inside the modelled `WorkflowRunId` length (string, @length 1..64, read
//! from `smithy_query(query="shape", shape="com.amazonaws.swf#WorkflowRunId")`). No run
//! id is a value the caller supplied, no two runs share one, and the id a start returns
//! is the id every reader of that run reports.
//!
//! Rows vary what the rule generalises over: the domain, the shape of the `workflowId`
//! (plain, punctuated, non-ASCII, at the modelled maximum), how many runs are started,
//! and whether the id is read from the start, from a describe or from the history's
//! events — ids minted for different runs in the same instant included.

use std::collections::BTreeSet;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{full_defaults, seeded, string_at, Emulator};

/// `WorkflowRunId`'s modelled bounds (string, @length 1..64).
const MIN_RUN_ID_LEN: usize = 1;
const MAX_RUN_ID_LEN: usize = 64;

/// The fewest bytes an opaque, server-generated id is built over: fewer would make it
/// guessable, which is what "opaque" rules out.
const MIN_RUN_ID_BYTES: usize = 16;

/// The workflow type every start below uses.
const TYPE_NAME: &str = "orders";
const TYPE_VERSION: &str = "1.0";

/// The characters standard base64 is written in — the class Amazon SWF's run ids belong
/// to.
fn is_base64_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '=')
}

/// Assert `run_id` satisfies the class and the modelled length.
fn assert_run_id_shape(case: &str, run_id: &str) {
    assert!(
        (MIN_RUN_ID_LEN..=MAX_RUN_ID_LEN).contains(&run_id.chars().count()),
        "case {case:?}: a runId lies inside WorkflowRunId's {MIN_RUN_ID_LEN}..{MAX_RUN_ID_LEN} \
         length, got {} characters in {run_id:?}",
        run_id.chars().count()
    );
    assert!(
        run_id.chars().all(is_base64_character),
        "case {case:?}: a runId is base64 text, got {run_id:?}"
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(run_id)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: a runId decodes as standard base64, got {run_id:?}: {error}")
        });
    assert!(
        decoded.len() >= MIN_RUN_ID_BYTES,
        "case {case:?}: a runId is built over at least {MIN_RUN_ID_BYTES} opaque bytes, \
         got {} in {run_id:?}",
        decoded.len()
    );
}

/// One start a run id is minted for.
struct StartCase {
    case: &'static str,
    domain: &'static str,
    workflow_id: String,
}

fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "plain id",
            domain: "run-ids",
            workflow_id: "wf-1".to_string(),
        },
        StartCase {
            case: "punctuated id",
            domain: "run-ids",
            workflow_id: "orders-2026-01-01#42".to_string(),
        },
        StartCase {
            case: "non-ASCII id in another domain",
            domain: "run-ids-second",
            workflow_id: "注文-ワークフロー".to_string(),
        },
        StartCase {
            case: "id at the modelled maximum",
            domain: "run-ids-second",
            // `WorkflowId` is string, @length 1..256.
            workflow_id: "w".repeat(256),
        },
    ]
}

#[tokio::test]
async fn every_minted_run_id_has_the_modelled_shape_and_is_unique() {
    let emulator = Emulator::new();
    for domain in ["run-ids", "run-ids-second"] {
        seeded(
            &emulator,
            domain,
            domain,
            (TYPE_NAME, TYPE_VERSION),
            full_defaults("tasks", "TERMINATE"),
        )
        .await;
    }

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for case in start_cases() {
        let answer = emulator
            .ok(
                case.case,
                "StartWorkflowExecution",
                &json!({
                    "domain": case.domain,
                    "workflowId": case.workflow_id,
                    "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
                }),
            )
            .await;
        let run_id = string_at(&answer, "/runId");
        assert_run_id_shape(case.case, &run_id);
        assert!(
            !run_id.contains(&case.workflow_id) && !run_id.contains(case.domain),
            "case {:?}: a runId is generated, not derived from the request, got {run_id:?}",
            case.case
        );
        assert!(
            seen.insert(run_id.clone()),
            "case {:?}: no two runs share a runId, got {run_id:?} twice",
            case.case
        );

        // The id a start returns is the id every reader of that run reports.
        let described = emulator
            .ok(
                case.case,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": case.domain,
                    "execution": { "workflowId": case.workflow_id, "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/execution/runId"),
            run_id,
            "case {:?}: the described run carries the minted id, got {described:?}",
            case.case
        );
        let history = emulator
            .ok(
                case.case,
                "GetWorkflowExecutionHistory",
                &json!({
                    "domain": case.domain,
                    "execution": { "workflowId": case.workflow_id, "runId": run_id },
                }),
            )
            .await;
        let events = history
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: expected events, got {history:?}", case.case));
        assert!(
            !events.is_empty(),
            "case {:?}: the run the id names has a history, got {history:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn run_ids_minted_back_to_back_are_all_distinct_and_well_shaped() {
    // Many starts in one domain in immediate succession: the id of each is its own, so
    // nothing is derived from a value two starts share (the domain, the clock second).
    let emulator = Emulator::new();
    let domain = "run-id-burst";
    seeded(
        &emulator,
        domain,
        domain,
        (TYPE_NAME, TYPE_VERSION),
        full_defaults("tasks", "TERMINATE"),
    )
    .await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for ordinal in 0..16 {
        let case = format!("burst run {ordinal}");
        let answer = emulator
            .ok(
                &case,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": format!("burst-{ordinal}"),
                    "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
                }),
            )
            .await;
        let run_id = string_at(&answer, "/runId");
        assert_run_id_shape(&case, &run_id);
        assert!(
            seen.insert(run_id.clone()),
            "case {case:?}: every start mints a distinct runId, got {run_id:?} twice"
        );
    }
    assert_eq!(
        seen.len(),
        16,
        "16 starts mint 16 distinct run ids, got {}",
        seen.len()
    );
}

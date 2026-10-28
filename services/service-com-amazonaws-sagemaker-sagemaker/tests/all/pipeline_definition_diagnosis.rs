// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the DIAGNOSIS `CreatePipeline` refuses a defective pipeline
//! definition with.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! `CreatePipeline` compiles the definition synchronously, so each DEFECT CLASS carries
//! its own reason: a caller (and the SDK error handling built on it) tells a cycle from a
//! dangling `DependsOn` from a bad parameter reference by that reason alone. The tests
//! assert the RULE over a TABLE spanning the classes, with the variable parts of each
//! message taken from the request, rather than one recorded call.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{error_code, message, Emulator};
use super::pipeline_execution::definition_declaring;
use super::pipeline_lifecycle::{condition_step, create_body, definition, step, token};

/// A `Fail` step named `name`, depending on `depends_on` — the step kind that ENDS a run,
/// so nothing may wait on it. `pipeline_lifecycle::step` is the `Lambda` step another
/// step may depend on.
fn fail_step(name: &str, depends_on: &[&str]) -> Value {
    let mut built = json!({
        "Name": name,
        "Type": "Fail",
        "Arguments": { "ErrorMessage": format!("{name} stopped the pipeline") },
    });
    if !depends_on.is_empty() {
        built["DependsOn"] = json!(depends_on);
    }
    built
}

/// `declaration` with a `DependsOn` list added, so a case can make any step kind — a
/// condition step included — wait on other steps.
fn depending_on(mut declaration: Value, depends_on: &[&str]) -> Value {
    declaration["DependsOn"] = json!(depends_on);
    declaration
}

/// A `Lambda` step named `name` whose arguments reference `reference` through the
/// schema's `{"Get": …}` property reference, nested the way a step's arguments carry one.
fn referencing_step(name: &str, reference: &str) -> Value {
    json!({
        "Name": name,
        "Type": "Lambda",
        "Arguments": {
            "FunctionArn": "arn:aws:lambda:us-east-1:000000000000:function:f",
            "Payload": { "Values": [{ "Get": reference }] },
        },
    })
}

/// RULE: a definition whose step DAG does not resolve is refused 400
/// `ValidationException` with the diagnosis of ITS defect class — a step depending on
/// itself, a `DependsOn` naming a step the definition does not declare, a `DependsOn`
/// whose target is a step kind nothing can wait on, or a cycle — and the variable parts
/// of that diagnosis are the step names the request sent.
#[tokio::test]
async fn each_step_graph_defect_class_carries_its_own_diagnosis() {
    let emulator = Emulator::new();

    // Rows span the defect classes, and within each class the step count, the graph
    // shape, the declaration order, the step kind of the target and whether the steps are
    // nested in a condition step's branches.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "self-dependency-alone",
            json!([step("Loop", &["Loop"])]),
            "Step: Loop can not depend on itself.".to_string(),
        ),
        (
            "self-dependency-beside-resolving-steps",
            json!([
                step("Head", &[]),
                step("Middle", &["Head"]),
                step("Knot", &["Head", "Knot"])
            ]),
            "Step: Knot can not depend on itself.".to_string(),
        ),
        (
            "self-dependency-in-a-condition-branch",
            json!([condition_step(
                "Gate",
                json!([step("Branch", &["Branch"])]),
                json!([]),
            )]),
            "Step: Branch can not depend on itself.".to_string(),
        ),
        (
            "unknown-dependency",
            json!([step("Only", &["Absent"])]),
            "Step: Only depends on unknown Step: Absent".to_string(),
        ),
        (
            "unknown-dependency-among-declared-ones",
            json!([
                step("First", &[]),
                step("Second", &["First", "NeverDeclared"])
            ]),
            "Step: Second depends on unknown Step: NeverDeclared".to_string(),
        ),
        (
            "unknown-dependency-in-a-condition-branch",
            json!([condition_step(
                "Gate",
                json!([step("Branch", &["Missing"])]),
                json!([]),
            )]),
            "Step: Branch depends on unknown Step: Missing".to_string(),
        ),
        (
            "fail-step-target",
            json!([fail_step("Stop", &[]), step("After", &["Stop"])]),
            "Step: After can not depends on FailStep: Stop".to_string(),
        ),
        (
            "fail-step-target-declared-after-the-waiter",
            json!([
                step("Lead", &[]),
                step("After", &["Lead", "Stop"]),
                fail_step("Stop", &["Lead"])
            ]),
            "Step: After can not depends on FailStep: Stop".to_string(),
        ),
        (
            "condition-step-target",
            json!([
                condition_step("Gate", json!([step("Branch", &[])]), json!([])),
                step("After", &["Gate"])
            ]),
            "Step: After can not depends on ConditionStep: Gate".to_string(),
        ),
        (
            "two-step-cycle",
            json!([step("Ping", &["Pong"]), step("Pong", &["Ping"])]),
            "Detected cycle in the DAG.".to_string(),
        ),
        (
            "three-step-cycle",
            json!([
                step("One", &["Three"]),
                step("Two", &["One"]),
                step("Three", &["Two"])
            ]),
            "Detected cycle in the DAG.".to_string(),
        ),
        (
            "cycle-reached-from-an-acyclic-prefix",
            json!([
                step("Entry", &["Ring1"]),
                step("Ring1", &["Ring2"]),
                step("Ring2", &["Ring1"])
            ]),
            "Detected cycle in the DAG.".to_string(),
        ),
        (
            "cycle-across-a-condition-branch",
            json!([
                condition_step("Gate", json!([step("Branch", &["Tail"])]), json!([])),
                step("Tail", &["Branch"])
            ]),
            "Detected cycle in the DAG.".to_string(),
        ),
    ];

    for (case, steps, expected) in cases {
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(case, &definition(steps), &token(case)),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {body:?}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case}: expected the diagnosis {expected:?}, got {body:?}"
        );

        // The definition was compiled, not stored: nothing is readable under the name.
        let (status, body) = emulator
            .call("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: a refused create must persist nothing, got {status} {body:?}"
        );
    }
}

/// RULE: a property reference to one of the pipeline's own parameters resolves against
/// the parameters the DEFINITION declares — a reference naming one it does not declare is
/// refused 400 `ValidationException` naming that reference as written, whatever the
/// declared set and wherever in the definition the reference sits, while a reference to a
/// declared parameter is accepted.
#[tokio::test]
async fn a_reference_to_an_undeclared_parameter_names_the_reference_it_cannot_resolve() {
    let emulator = Emulator::new();

    // Rows differ along the declared parameter set (none, one, several) and the reference
    // the step makes — including one that differs from a declared name only in case, and
    // one whose prefix addresses something other than a parameter.
    let unresolved: Vec<(&str, Vec<&str>, &str, String)> = vec![
        (
            "declares-nothing",
            vec![],
            "Parameters.Alpha",
            "Unknown property reference [Parameters.Alpha].".to_string(),
        ),
        (
            "declares-another-name",
            vec!["Alpha"],
            "Parameters.Beta",
            "Unknown property reference [Parameters.Beta].".to_string(),
        ),
        (
            "declares-several-other-names",
            vec!["Alpha", "Beta", "Gamma"],
            "Parameters.Delta",
            "Unknown property reference [Parameters.Delta].".to_string(),
        ),
        (
            "differs-from-a-declared-name-only-in-case",
            vec!["Alpha"],
            "Parameters.alpha",
            "Unknown property reference [Parameters.alpha].".to_string(),
        ),
    ];

    for (case, declared, reference, expected) in unresolved {
        let body = create_body(
            case,
            &definition_declaring(&declared, json!([referencing_step("Invoke", reference)])),
            &token(case),
        );
        let (status, parsed) = emulator.call("CreatePipeline", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: expected 400 ValidationException, got {status} {parsed:?}"
        );
        assert_eq!(
            message(&parsed),
            expected,
            "case {case}: expected the diagnosis {expected:?}, got {parsed:?}"
        );
        let (status, parsed) = emulator
            .call("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ResourceNotFound"),
            "case {case}: a refused create must persist nothing, got {status} {parsed:?}"
        );
    }

    // And the same references RESOLVE once the definition declares what they name, so
    // the refusal above is about resolution and not about the reference's form. The last
    // row references a step's own property rather than a parameter, which the parameter
    // resolution does not judge at all.
    let resolving: Vec<(&str, Vec<&str>, &str)> = vec![
        ("declared-single-parameter", vec!["Alpha"], "Parameters.Alpha"),
        (
            "declared-among-several",
            vec!["Alpha", "Beta", "Gamma"],
            "Parameters.Gamma",
        ),
        (
            "not-a-parameter-reference",
            vec![],
            "Steps.Other.OutputParameters",
        ),
    ];

    for (case, declared, reference) in resolving {
        let body = create_body(
            case,
            &definition_declaring(&declared, json!([referencing_step("Invoke", reference)])),
            &token(case),
        );
        let (status, parsed) = emulator.call("CreatePipeline", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a reference the definition resolves is accepted, got {status} \
             {parsed:?}"
        );
    }
}

/// RULE: a `DependsOn` edge waits for a step that CAN complete, so a step of any other
/// kind may be waited on and the two kinds nothing can wait on may themselves depend on
/// others — the refusal is about the edge's TARGET, not about the step that declares it.
#[tokio::test]
async fn only_the_target_of_a_dependency_decides_whether_the_edge_resolves() {
    let emulator = Emulator::new();

    // Rows: a fail step depending on a step that completes, a condition step depending on
    // one, a chain of both, and a condition step's branch step depending on an outside
    // step.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "fail-step-depends-on-a-lambda-step",
            json!([step("Lead", &[]), fail_step("Stop", &["Lead"])]),
        ),
        (
            "condition-step-depends-on-a-lambda-step",
            json!([
                step("Lead", &[]),
                depending_on(
                    condition_step("Gate", json!([step("Branch", &[])]), json!([])),
                    &["Lead"],
                ),
            ]),
        ),
        (
            "both-kinds-depend-on-the-same-step",
            json!([
                step("Lead", &[]),
                fail_step("Stop", &["Lead"]),
                depending_on(
                    condition_step("Gate", json!([step("Branch", &["Lead"])]), json!([])),
                    &["Lead"],
                ),
            ]),
        ),
    ];

    for (case, steps) in accepted {
        let (status, body) = emulator
            .call(
                "CreatePipeline",
                &create_body(case, &definition(steps), &token(case)),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: a step kind that completes may be waited on, got {status} \
             {body:?}"
        );
    }
}

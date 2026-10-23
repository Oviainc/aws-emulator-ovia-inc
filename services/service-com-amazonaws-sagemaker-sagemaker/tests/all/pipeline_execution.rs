// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's pipeline-execution slice:
//! `StartPipelineExecution` and `DescribePipelineExecution`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a start
//! records one execution of the stored pipeline under an ARN that extends the pipeline's
//! own, and the reader replays every member the start supplied; a start the stored
//! pipeline cannot support is refused and persists nothing; a repeated
//! `ClientRequestToken` completes the start no more than once; the execution's terminal
//! status is moved by the CLOCK, not by a read; and a pipeline's executions go with the
//! pipeline when it is removed.

use std::time::Duration;

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};
use super::pipeline_lifecycle::{create_body, step, token, DEFINITION_VERSION};

/// Long enough to pass `state::PIPELINE_EXECUTION_ENDS_AFTER` (400 ms).
const PAST_EXECUTION_DUE: Duration = Duration::from_millis(800);

/// Long enough to pass `state::PIPELINE_DELETED_AFTER` (300 ms).
const PAST_REMOVAL_DUE: Duration = Duration::from_millis(600);

/// The parameter names the fixture definition declares — what a run may assign values
/// to, per `Parameter$Name`'s doc ("This parameter name must match a named parameter in
/// the pipeline definition").
const DECLARED_PARAMETERS: [&str; 3] = ["Alpha", "Beta", "Gamma"];

/// A pipeline definition in the published schema version declaring `parameters` and
/// `steps`.
fn definition_declaring(parameters: &[&str], steps: Value) -> String {
    let declared: Vec<Value> = parameters
        .iter()
        .map(|name| json!({ "Name": name, "Type": "String", "DefaultValue": "unset" }))
        .collect();
    json!({
        "Version": DEFINITION_VERSION,
        "Metadata": {},
        "Parameters": declared,
        "Steps": steps,
    })
    .to_string()
}

/// The definition every pipeline in this suite is created with: two steps named
/// `Prepare` and `Train`, and the three declared parameters above.
fn fixture_definition() -> String {
    definition_declaring(
        &DECLARED_PARAMETERS,
        json!([step("Prepare", &[]), step("Train", &["Prepare"])]),
    )
}

/// Create the fixture pipeline under `name`, answering its `PipelineArn`.
async fn pipeline(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreatePipeline",
            &create_body(name, &fixture_definition(), &token(name)),
        )
        .await;
    str_field(&created, "PipelineArn")
}

/// A `StartPipelineExecution` body naming `name` with `request_token`, plus whatever
/// optional members `extra` carries.
fn start_body(name: &str, request_token: &str, extra: &Value) -> Value {
    let mut body = json!({ "PipelineName": name, "ClientRequestToken": request_token });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member.as_str()] = value.clone();
        }
    }
    body
}

/// RULE: a start records ONE execution of the pipeline it names, under a
/// `PipelineExecutionArn` that extends the pipeline's own ARN with a server-generated
/// `execution/<id>` segment, reports it `Executing`, stamps the pipeline's `LastRunTime`,
/// and the reader replays every member the start supplied — whichever optional members
/// that was, and whether the start named the pipeline by name or by ARN.
#[tokio::test]
async fn a_start_records_an_execution_the_reader_replays() {
    let emulator = Emulator::new();
    let parallelism = json!({ "MaxParallelExecutionSteps": 7 });
    let selective = json!({ "SelectedSteps": [{ "StepName": "Train" }] });

    // Rows differ along which optional members the start carried, and how many
    // parameters it assigned (none, one, all three declared).
    let cases: Vec<(&str, Value)> = vec![
        ("no-optional-members", json!({})),
        (
            "display-name",
            json!({ "PipelineExecutionDisplayName": "nightly-run" }),
        ),
        (
            "description",
            json!({ "PipelineExecutionDescription": "the nightly training run" }),
        ),
        (
            "one-parameter",
            json!({ "PipelineParameters": [{ "Name": "Beta", "Value": "2" }] }),
        ),
        (
            "every-declared-parameter",
            json!({
                "PipelineParameters": DECLARED_PARAMETERS
                    .iter()
                    .enumerate()
                    .map(|(index, name)| json!({ "Name": name, "Value": index.to_string() }))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "parallelism-override",
            json!({ "ParallelismConfiguration": parallelism }),
        ),
        (
            "mlflow-experiment",
            json!({ "MlflowExperimentName": "pipeline-experiment" }),
        ),
        (
            "selective-execution",
            json!({ "SelectiveExecutionConfig": selective }),
        ),
        ("the-only-pipeline-version", json!({ "PipelineVersionId": 1 })),
        (
            "every-optional-member",
            json!({
                "PipelineExecutionDisplayName": "full-run",
                "PipelineExecutionDescription": "every optional member at once",
                "PipelineParameters": [{ "Name": "Alpha", "Value": "1" }],
                "ParallelismConfiguration": parallelism,
                "MlflowExperimentName": "full-experiment",
                "SelectiveExecutionConfig": selective,
                "PipelineVersionId": 1,
            }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, extra) in cases {
        let pipeline_arn = pipeline(&emulator, case).await;
        // The pipeline has not run yet, so the reader reports no `LastRunTime`.
        let described = emulator
            .ok("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert!(
            described.get("LastRunTime").is_none(),
            "case {case}: a pipeline nobody ran reports no LastRunTime, got {described:?}"
        );

        // Half the rows name the pipeline by ARN, half by name: `PipelineNameOrArn`
        // denotes the same pipeline either way.
        let named_by_arn = minted.len().is_multiple_of(2);
        let identifier = if named_by_arn {
            pipeline_arn.clone()
        } else {
            case.to_string()
        };
        let started = emulator
            .ok(
                "StartPipelineExecution",
                &start_body(&identifier, &token(case), &extra),
            )
            .await;
        let execution_arn = str_field(&started, "PipelineExecutionArn");
        let prefix = format!("{pipeline_arn}/execution/");
        assert!(
            execution_arn.starts_with(&prefix),
            "case {case}: the execution ARN extends the pipeline's own, expected the \
             prefix {prefix}, got {execution_arn}"
        );
        assert!(
            !execution_arn[prefix.len()..].is_empty(),
            "case {case}: the execution ARN carries an id, got {execution_arn}"
        );
        assert!(
            !minted.contains(&execution_arn),
            "case {case}: every start mints its own execution ARN, got the already \
             minted {execution_arn}"
        );
        minted.push(execution_arn.clone());

        let described = emulator
            .ok(
                "DescribePipelineExecution",
                &json!({ "PipelineExecutionArn": execution_arn }),
            )
            .await;
        assert_eq!(
            (
                str_field(&described, "PipelineArn"),
                str_field(&described, "PipelineExecutionArn"),
                str_field(&described, "PipelineExecutionStatus"),
            ),
            (pipeline_arn.clone(), execution_arn.clone(), "Executing".to_string()),
            "case {case}: the reader replays the started execution, got {described:?}"
        );
        assert_eq!(
            described.get("PipelineVersionId").and_then(Value::as_i64),
            Some(1),
            "case {case}: the run is of the pipeline's one version, got {described:?}"
        );
        assert!(
            described.get("FailureReason").is_none(),
            "case {case}: an Executing run has not failed, got {described:?}"
        );
        assert_eq!(
            described.get("CreationTime"),
            described.get("LastModifiedTime"),
            "case {case}: nothing has modified the run since it started, got {described:?}"
        );
        assert_eq!(
            described
                .pointer("/CreatedBy/IamIdentity/Arn")
                .and_then(Value::as_str),
            Some(format!("arn:aws:iam::{ACCOUNT}:user/AKIDEXAMPLE").as_str()),
            "case {case}: the run records the identity that started it, got {described:?}"
        );

        // Every member the start supplied is replayed, and a member it did not supply is
        // ABSENT rather than empty.
        for member in [
            "PipelineExecutionDisplayName",
            "PipelineExecutionDescription",
            "ParallelismConfiguration",
            "SelectiveExecutionConfig",
        ] {
            assert_eq!(
                described.get(member),
                extra.get(member),
                "case {case}: {member} round-trips, got {described:?}"
            );
        }
        assert_eq!(
            described
                .pointer("/MLflowConfig/MlflowExperimentName")
                .and_then(Value::as_str),
            extra.get("MlflowExperimentName").and_then(Value::as_str),
            "case {case}: the MLflow experiment name round-trips, got {described:?}"
        );

        // And the pipeline now reports when it was last run.
        let described_pipeline = emulator
            .ok("DescribePipeline", &json!({ "PipelineName": case }))
            .await;
        assert!(
            described_pipeline.get("LastRunTime").is_some(),
            "case {case}: starting a run stamps the pipeline's LastRunTime, got \
             {described_pipeline:?}"
        );
    }
}

/// RULE: an execution's terminal status is moved by the CLOCK — it reports `Executing`
/// until the move falls due, then the modelled failure state with a reason, whatever
/// number of times a caller read it in between, and the terminal record is stable across
/// later reads.
#[tokio::test]
async fn the_clock_moves_a_run_out_of_executing_and_a_read_does_not() {
    let emulator = Emulator::new();

    // Rows differ along how many times the caller polls BEFORE the move is due: a read
    // must make no difference to the result.
    for polls in [0_usize, 1, 3] {
        let case = format!("polled-{polls}-times");
        pipeline(&emulator, &case).await;
        let started = emulator
            .ok(
                "StartPipelineExecution",
                &start_body(&case, &token(&case), &json!({})),
            )
            .await;
        let execution_arn = str_field(&started, "PipelineExecutionArn");
        let read = json!({ "PipelineExecutionArn": execution_arn });

        for poll in 0..polls {
            let described = emulator.ok("DescribePipelineExecution", &read).await;
            assert_eq!(
                str_field(&described, "PipelineExecutionStatus"),
                "Executing",
                "case {case}/poll {poll}: a run reports Executing until its move is due, \
                 got {described:?}"
            );
        }

        tokio::time::sleep(PAST_EXECUTION_DUE).await;
        let settled = emulator.ok("DescribePipelineExecution", &read).await;
        assert_eq!(
            str_field(&settled, "PipelineExecutionStatus"),
            "Failed",
            "case {case}: past the due instant the run reports the modelled failure \
             state, got {settled:?}"
        );
        assert!(
            !str_field(&settled, "FailureReason").is_empty(),
            "case {case}: a failed run says why, got {settled:?}"
        );
        assert_ne!(
            settled.get("LastModifiedTime"),
            settled.get("CreationTime"),
            "case {case}: the move stamps LastModifiedTime, got {settled:?}"
        );

        let again = emulator.ok("DescribePipelineExecution", &read).await;
        assert_eq!(
            again, settled,
            "case {case}: the terminal record is stable across reads, got {again:?}"
        );
    }
}

/// RULE: a start the stored pipeline cannot support is REFUSED with the modelled
/// failure and persists nothing — an unknown pipeline or pipeline version is not found,
/// and a parameter, a selected step or a reference execution the stored pipeline does
/// not account for is a bad request.
#[tokio::test]
async fn a_start_the_stored_pipeline_cannot_support_is_refused() {
    let emulator = Emulator::new();
    let runnable_arn = pipeline(&emulator, "runnable").await;
    // A second pipeline, so a reference execution of ANOTHER pipeline is addressable.
    pipeline(&emulator, "other").await;
    let other_execution = str_field(
        &emulator
            .ok(
                "StartPipelineExecution",
                &start_body("other", &token("other"), &json!({})),
            )
            .await,
        "PipelineExecutionArn",
    );

    // The modelled bound on `SelectedStepList` (len 1..50), read from the module that
    // restates it rather than repeated as a literal here.
    let max_selected = service_com_amazonaws_sagemaker_sagemaker::state::MAX_SELECTED_STEPS;
    let one_past_the_bound: Vec<Value> = (0..=max_selected)
        .map(|index| json!({ "StepName": format!("Step{index}") }))
        .collect();

    // Rows differ along WHAT the stored pipeline cannot support, each an instance of the
    // rule; the version rows also walk one past the only published version.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "selected-steps-one-past-the-modelled-bound",
            start_body(
                "runnable",
                &token("j"),
                &json!({ "SelectiveExecutionConfig": { "SelectedSteps": one_past_the_bound } }),
            ),
            "ValidationException",
        ),
        (
            "no-selected-steps",
            start_body(
                "runnable",
                &token("k"),
                &json!({ "SelectiveExecutionConfig": { "SelectedSteps": [] } }),
            ),
            "ValidationException",
        ),
        (
            "selected-step-named-twice",
            start_body(
                "runnable",
                &token("l"),
                &json!({
                    "SelectiveExecutionConfig": {
                        "SelectedSteps": [{ "StepName": "Train" }, { "StepName": "Train" }],
                    },
                }),
            ),
            "ValidationException",
        ),
        (
            "unknown-pipeline-name",
            start_body("never-created", &token("a"), &json!({})),
            "ResourceNotFound",
        ),
        (
            "unknown-pipeline-arn",
            start_body(
                &format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:pipeline/never-created"),
                &token("b"),
                &json!({}),
            ),
            "ResourceNotFound",
        ),
        (
            "version-one-past-the-only-one",
            start_body("runnable", &token("c"), &json!({ "PipelineVersionId": 2 })),
            "ResourceNotFound",
        ),
        (
            "version-far-past-the-only-one",
            start_body("runnable", &token("d"), &json!({ "PipelineVersionId": 9999 })),
            "ResourceNotFound",
        ),
        (
            "parameter-the-definition-does-not-declare",
            start_body(
                "runnable",
                &token("e"),
                &json!({ "PipelineParameters": [{ "Name": "Delta", "Value": "4" }] }),
            ),
            "ValidationException",
        ),
        (
            "declared-parameter-assigned-twice",
            start_body(
                "runnable",
                &token("f"),
                &json!({
                    "PipelineParameters": [
                        { "Name": "Alpha", "Value": "1" },
                        { "Name": "Alpha", "Value": "2" },
                    ],
                }),
            ),
            "ValidationException",
        ),
        (
            "selected-step-the-definition-does-not-declare",
            start_body(
                "runnable",
                &token("g"),
                &json!({
                    "SelectiveExecutionConfig": {
                        "SelectedSteps": [{ "StepName": "Evaluate" }],
                    },
                }),
            ),
            "ValidationException",
        ),
        (
            "reference-execution-nobody-started",
            start_body(
                "runnable",
                &token("h"),
                &json!({
                    "SelectiveExecutionConfig": {
                        "SelectedSteps": [{ "StepName": "Train" }],
                        "SourcePipelineExecutionArn":
                            format!("{runnable_arn}/execution/0123456789abcdef"),
                    },
                }),
            ),
            "ResourceNotFound",
        ),
        (
            "reference-execution-of-another-pipeline",
            start_body(
                "runnable",
                &token("i"),
                &json!({
                    "SelectiveExecutionConfig": {
                        "SelectedSteps": [{ "StepName": "Train" }],
                        "SourcePipelineExecutionArn": other_execution,
                    },
                }),
            ),
            "ResourceNotFound",
        ),
    ];

    for (case, body, expected_code) in cases {
        let (status, parsed) = emulator.call("StartPipelineExecution", &body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, expected_code),
            "case {case}: expected 400 {expected_code}, got {status} {parsed:?}"
        );

        // Nothing was started: the pipeline still reports no run.
        let described = emulator
            .ok("DescribePipeline", &json!({ "PipelineName": "runnable" }))
            .await;
        assert!(
            described.get("LastRunTime").is_none(),
            "case {case}: a refused start must persist nothing, got {described:?}"
        );
    }

    // At the bound, a selection the definition can satisfy is ACCEPTED: the refusals
    // above are about the bound and the definition, not about selective execution.
    let wide_definition = definition_declaring(
        &[],
        json!((0..max_selected)
            .map(|index| step(&format!("Step{index}"), &[]))
            .collect::<Vec<Value>>()),
    );
    emulator
        .ok(
            "CreatePipeline",
            &create_body("wide", &wide_definition, &token("wide")),
        )
        .await;
    let at_the_bound: Vec<Value> = (0..max_selected)
        .map(|index| json!({ "StepName": format!("Step{index}") }))
        .collect();
    let started = emulator
        .ok(
            "StartPipelineExecution",
            &start_body(
                "wide",
                &token("wide-run"),
                &json!({ "SelectiveExecutionConfig": { "SelectedSteps": at_the_bound } }),
            ),
        )
        .await;
    assert!(
        !str_field(&started, "PipelineExecutionArn").is_empty(),
        "case selected-steps-at-the-modelled-bound: a selection the definition satisfies \
         is accepted, got {started:?}"
    );

    // A pipeline whose delete is in flight is no longer runnable: the start conflicts
    // with the removal already under way. Asserted straight after the delete, while the
    // pipeline still reports the transient `Deleting`.
    pipeline(&emulator, "doomed").await;
    emulator
        .ok(
            "DeletePipeline",
            &json!({ "PipelineName": "doomed", "ClientRequestToken": token("delete-doomed") }),
        )
        .await;
    let (status, parsed) = emulator
        .call(
            "StartPipelineExecution",
            &start_body("doomed", &token("start-doomed"), &json!({})),
        )
        .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ConflictException"),
        "case pipeline-being-deleted: expected 400 ConflictException, got {status} {parsed:?}"
    );
}

/// RULE: `ClientRequestToken` completes a start no more than once — a retry answers the
/// execution that call started and changes nothing — while a different token, and the
/// same token under another pipeline's name, each start their own run.
#[tokio::test]
async fn a_repeated_start_completes_no_more_than_once() {
    let emulator = Emulator::new();
    let first_arn = pipeline(&emulator, "first").await;
    let second_arn = pipeline(&emulator, "second").await;
    let shared = token("one-start");

    let started = emulator
        .ok(
            "StartPipelineExecution",
            &start_body(
                "first",
                &shared,
                &json!({ "PipelineExecutionDisplayName": "the-first-run" }),
            ),
        )
        .await;
    let execution_arn = str_field(&started, "PipelineExecutionArn");

    // Rows: the retry of the same call, the retry carrying different members, a
    // genuinely second start (another token), and the same token on another pipeline.
    let cases: Vec<(&str, &str, String, Value, bool)> = vec![
        ("same-token", "first", shared.clone(), json!({}), true),
        (
            "same-token-different-display-name",
            "first",
            shared.clone(),
            json!({ "PipelineExecutionDisplayName": "a-different-name" }),
            true,
        ),
        (
            "different-token",
            "first",
            token("another-start"),
            json!({}),
            false,
        ),
        (
            "same-token-another-pipeline",
            "second",
            shared.clone(),
            json!({}),
            false,
        ),
    ];

    for (case, name, request_token, extra, is_retry) in cases {
        let started = emulator
            .ok(
                "StartPipelineExecution",
                &start_body(name, &request_token, &extra),
            )
            .await;
        let answered = str_field(&started, "PipelineExecutionArn");
        if is_retry {
            assert_eq!(
                answered, execution_arn,
                "case {case}: a retry answers the first call's execution, got {started:?}"
            );
        } else {
            assert_ne!(
                answered, execution_arn,
                "case {case}: a start with its own token starts its own run, got {started:?}"
            );
            let owner = if name == "first" {
                &first_arn
            } else {
                &second_arn
            };
            assert!(
                answered.starts_with(&format!("{owner}/execution/")),
                "case {case}: the run belongs to the pipeline it was started on, got \
                 {answered}"
            );
        }

        // Whatever the answer, the first execution is still the one the first call
        // recorded: no retry rewrote it.
        let described = emulator
            .ok(
                "DescribePipelineExecution",
                &json!({ "PipelineExecutionArn": execution_arn }),
            )
            .await;
        assert_eq!(
            str_field(&described, "PipelineExecutionDisplayName"),
            "the-first-run",
            "case {case}: a retry leaves the recorded run as the first call wrote it, \
             got {described:?}"
        );
    }
}

/// RULE: an execution belongs to the pipeline that ran it — it is resolvable while that
/// pipeline exists, including while its delete is in flight, and gone once the clock
/// owes the pipeline's removal, so a pipeline recreated under the freed name inherits no
/// run.
#[tokio::test]
async fn a_pipelines_executions_go_with_the_pipeline() {
    let emulator = Emulator::new();
    pipeline(&emulator, "cascade").await;
    let execution_arn = str_field(
        &emulator
            .ok(
                "StartPipelineExecution",
                &start_body("cascade", &token("cascade"), &json!({})),
            )
            .await,
        "PipelineExecutionArn",
    );
    let read = json!({ "PipelineExecutionArn": execution_arn });

    emulator
        .ok(
            "DeletePipeline",
            &json!({ "PipelineName": "cascade", "ClientRequestToken": token("delete-cascade") }),
        )
        .await;
    // The removal is not due yet, so the run is still resolvable.
    let described = emulator.ok("DescribePipelineExecution", &read).await;
    assert_eq!(
        str_field(&described, "PipelineExecutionArn"),
        str_field(&read, "PipelineExecutionArn"),
        "case delete-in-flight: the run is still resolvable, got {described:?}"
    );

    tokio::time::sleep(PAST_REMOVAL_DUE).await;
    let (status, parsed) = emulator.call("DescribePipelineExecution", &read).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFound"),
        "case past-removal: a removed pipeline's run is refused, got {status} {parsed:?}"
    );

    // The name is free again, and the recreated pipeline inherits no run.
    pipeline(&emulator, "cascade").await;
    let described = emulator
        .ok("DescribePipeline", &json!({ "PipelineName": "cascade" }))
        .await;
    assert!(
        described.get("LastRunTime").is_none(),
        "case recreated: a pipeline recreated under the freed name has not run, got \
         {described:?}"
    );
    let (status, parsed) = emulator.call("DescribePipelineExecution", &read).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ResourceNotFound"),
        "case recreated: the deleted pipeline's run stays gone, got {status} {parsed:?}"
    );
}

/// RULE: a start carries at most one value per parameter the stored DEFINITION declares,
/// so what a run may assign follows the definition rather than the request — asserted
/// over definitions declaring different parameter sets.
#[tokio::test]
async fn what_a_run_may_assign_follows_the_stored_definition() {
    let emulator = Emulator::new();

    // Rows differ along the parameter set the definition declares: none, one, and a set
    // whose names none of the other rows share.
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("declares-none", vec![]),
        ("declares-one", vec!["Only"]),
        ("declares-many", vec!["First", "Second", "Third", "Fourth"]),
    ];

    for (case, declared) in cases {
        let definition = definition_declaring(&declared, json!([step("Prepare", &[])]));
        emulator
            .ok(
                "CreatePipeline",
                &create_body(case, &definition, &token(case)),
            )
            .await;

        // Every declared name is assignable, together, in one start.
        let assignments: Vec<Value> = declared
            .iter()
            .map(|name| json!({ "Name": name, "Value": format!("{name}-value") }))
            .collect();
        let started = emulator
            .ok(
                "StartPipelineExecution",
                &start_body(
                    case,
                    &token(case),
                    &json!({ "PipelineParameters": assignments }),
                ),
            )
            .await;
        assert!(
            !str_field(&started, "PipelineExecutionArn").is_empty(),
            "case {case}: assigning every declared parameter is accepted, got {started:?}"
        );

        // A name the definition does not declare is refused, whatever the row declares.
        let (status, parsed) = emulator
            .call(
                "StartPipelineExecution",
                &start_body(
                    case,
                    &token(&format!("{case}-undeclared")),
                    &json!({ "PipelineParameters": [{ "Name": "Undeclared", "Value": "x" }] }),
                ),
            )
            .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ValidationException"),
            "case {case}: a parameter the definition does not declare is refused, got \
             {status} {parsed:?}"
        );
    }
}

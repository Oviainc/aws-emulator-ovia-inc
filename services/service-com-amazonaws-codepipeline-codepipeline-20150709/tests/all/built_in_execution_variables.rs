// SPDX-License-Identifier: CC0-1.0

//! The VARIABLES an execution resolves an action's `#{namespace.key}` references
//! against, end to end over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: every reference in an action's `configuration` is resolved
//! against the variables the EXECUTION carries, and those include CodePipeline's own
//! built-ins — `#{codepipeline.PipelineExecutionId}` is published for every execution of
//! every pipeline without the declaration declaring anything — so the action's work is
//! performed with the resolved value, the worker of a custom action is served the
//! resolved value, and `ListActionExecutions` reports it as the action's
//! `resolvedConfiguration`. A reference the execution carries no value for is left
//! exactly as written, which is what keeps a typo observable instead of silently empty.
//!
//! The rows span what the rule generalises over: a reference alone and inside
//! surrounding text, the same reference twice, a built-in beside a declared pipeline
//! variable, a name the `codepipeline` namespace does not publish, and no reference at
//! all — read back through the two readers of a configuration (the engine that performs
//! the action, and the job a worker is handed).
//!
//! The harness comes from `tests/all/execution_engine.rs`, so these pipelines run their
//! real actions through the emulated S3 service and every assertion goes through the
//! router.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, wait_for_terminal, Emulator, WAIT,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "variables-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The bucket a deploy action delivers into.
const DEPLOY_BUCKET: &str = "variables-deploy";

/// The artifact the source action produces and the deploy action consumes.
const ARTIFACT: &str = "SourceArtifact";

/// The bytes the source object carries, so a delivery is observable.
const PAYLOAD: &[u8] = b"the source bytes";

/// The reference every row of the table below is about: the id of the execution
/// resolving it, which CodePipeline publishes for every pipeline.
const EXECUTION_ID_REFERENCE: &str = "#{codepipeline.PipelineExecutionId}";

/// The path form of an object key: a key may carry characters a URI reserves — a
/// reference left as written carries `#`, which starts a fragment — so the key is
/// percent-encoded for the request path, exactly as a client's own SDK encodes it.
fn encoded(key: &str) -> String {
    key.replace('%', "%25")
        .replace('#', "%23")
        .replace('{', "%7B")
        .replace('}', "%7D")
}

/// Create the buckets one of these pipelines needs and write its source object.
async fn seed(emulator: &Emulator, pipeline: &str) {
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator.put_object(SOURCE_BUCKET, SOURCE_KEY, PAYLOAD).await;
}

/// Start one execution of `pipeline` and answer its id.
async fn start(emulator: &Emulator, pipeline: &str) -> String {
    emulator
        .ok("StartPipelineExecution", &json!({"name": pipeline}))
        .await["pipelineExecutionId"]
        .as_str()
        .expect("the start reports an execution id")
        .to_string()
}

/// The action execution `ListActionExecutions` reports for one action of one execution.
async fn action_execution(
    emulator: &Emulator,
    pipeline: &str,
    execution: &str,
    action: &str,
) -> Value {
    let page = emulator
        .ok(
            "ListActionExecutions",
            &json!({
                "pipelineName": pipeline,
                "filter": {"pipelineExecutionId": execution},
            }),
        )
        .await;
    page["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|detail| detail["actionName"] == json!(action))
        .cloned()
        .unwrap_or_else(|| panic!("no action execution for {action} of {execution}: {page}"))
}

/// RULE, through the engine that performs an action: the key an S3 deploy action
/// delivers under is its `ObjectKey` with every reference the execution carries a value
/// for replaced, and `ListActionExecutions` reports that same text as the action's
/// `resolvedConfiguration`.
#[tokio::test]
async fn an_actions_configuration_is_resolved_against_the_executions_variables() {
    // (case, the ObjectKey declared, how the delivered key is built from the execution
    //  id, whether the configuration has a resolved form at all)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, String, fn(&str) -> String, bool)> = vec![
        (
            "the reference alone",
            EXECUTION_ID_REFERENCE.to_string(),
            |execution| execution.to_string(),
            true,
        ),
        (
            "the reference inside a path",
            format!("deployed/{EXECUTION_ID_REFERENCE}.zip"),
            |execution| format!("deployed/{execution}.zip"),
            true,
        ),
        (
            "the same reference twice",
            format!("{EXECUTION_ID_REFERENCE}/{EXECUTION_ID_REFERENCE}.zip"),
            |execution| format!("{execution}/{execution}.zip"),
            true,
        ),
        (
            "a declared pipeline variable beside the built-in",
            format!("#{{variables.Env}}/{EXECUTION_ID_REFERENCE}.zip"),
            |execution| format!("staging/{execution}.zip"),
            true,
        ),
        (
            // The `codepipeline` namespace publishes the execution id; a name it does
            // not publish is a value the execution carries nothing for, so the text
            // stays as written and the delivery lands under it.
            "a name the codepipeline namespace does not publish",
            "deployed/#{codepipeline.NoSuchVariable}.zip".to_string(),
            |_| "deployed/#{codepipeline.NoSuchVariable}.zip".to_string(),
            false,
        ),
        (
            "no reference at all",
            "deployed/plain.zip".to_string(),
            |_| "deployed/plain.zip".to_string(),
            true,
        ),
    ];

    for (row, (case, object_key, delivered, resolvable)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        // The pipeline name is short because the artifact bucket is derived from it and
        // an S3 bucket name is at most 63 characters.
        let pipeline = format!("vars-{row}");
        seed(&emulator, &pipeline).await;
        let mut body = declaration(
            &pipeline,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, &object_key, false),
            ]),
            None,
        );
        // `PipelineDeclaration$variables` is a V2 member, and one row references one of
        // its variables beside the built-in.
        body["pipeline"]["pipelineType"] = json!("V2");
        body["pipeline"]["variables"] = json!([{"name": "Env", "defaultValue": "staging"}]);
        emulator.ok("CreatePipeline", &body).await;

        let execution = start(&emulator, &pipeline).await;
        let answer = wait_for_terminal(&emulator, &pipeline, &execution).await;
        assert_eq!(
            answer["pipelineExecution"]["status"],
            json!("Succeeded"),
            "case {case:?}: expected the execution to deliver and succeed, got {answer}"
        );

        let expected_key = delivered(&execution);
        let (status, bytes) = emulator
            .get_object(DEPLOY_BUCKET, &encoded(&expected_key))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delivery under {expected_key:?}, got {status}"
        );
        assert_eq!(
            bytes, PAYLOAD,
            "case {case:?}: the delivered bytes must be the source bytes"
        );
        // The declared text is not the key: a resolved reference is no longer there.
        if expected_key != object_key {
            let (status, _) = emulator
                .get_object(DEPLOY_BUCKET, &encoded(&object_key))
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: nothing is delivered under the UNRESOLVED key \
                 {object_key:?}, got {status}"
            );
        }

        let detail = action_execution(&emulator, &pipeline, &execution, "Dep").await;
        let expected_resolved = match resolvable {
            true => json!(expected_key),
            // A reference this emulator cannot resolve leaves the configuration with no
            // resolved form at all, so the member is absent rather than half-resolved.
            false => Value::Null,
        };
        assert_eq!(
            detail["input"]["resolvedConfiguration"]["ObjectKey"], expected_resolved,
            "case {case:?}: unexpected resolvedConfiguration, got {detail}"
        );
        assert_eq!(
            detail["input"]["configuration"]["ObjectKey"],
            json!(object_key),
            "case {case:?}: the declared configuration is reported as declared, got {detail}"
        );
    }
}

/// RULE, through the other reader of a configuration: the job a custom action's worker
/// is handed carries the action's configuration with the same references resolved, so
/// the worker performs the work the execution asked for rather than one carrying raw
/// template text.
#[tokio::test]
async fn a_custom_actions_job_carries_the_resolved_configuration() {
    // (case, the configuration value declared, how the value the worker is served is
    //  built from the execution id)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, String, fn(&str) -> String)> = vec![
        (
            "the reference alone",
            EXECUTION_ID_REFERENCE.to_string(),
            |execution| execution.to_string(),
            ),
        (
            "the reference inside surrounding text",
            format!("build-{EXECUTION_ID_REFERENCE}"),
            |execution| format!("build-{execution}"),
        ),
        (
            "no reference at all",
            "build-fixed".to_string(),
            |_| "build-fixed".to_string(),
        ),
    ];

    for (row, (case, configured, served)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("job-vars-{row}");
        seed(&emulator, &pipeline).await;
        emulator
            .ok(
                "CreateCustomActionType",
                &json!({
                    "category": "Build",
                    "provider": "VariablesWorker",
                    "version": "1",
                    "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                    "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
                }),
            )
            .await;
        let body = declaration(
            &pipeline,
            json!([
                source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                json!({
                    "name": "Build",
                    "actions": [{
                        "name": "Work",
                        "actionTypeId": {
                            "category": "Build",
                            "owner": "Custom",
                            "provider": "VariablesWorker",
                            "version": "1",
                        },
                        "configuration": {"Tag": configured},
                        "inputArtifacts": [{"name": ARTIFACT}],
                    }],
                }),
            ]),
            None,
        );
        emulator.ok("CreatePipeline", &body).await;
        let execution = start(&emulator, &pipeline).await;

        // The job the execution queues when it reaches the custom action, polled for the
        // way its worker polls for it.
        let deadline = Instant::now() + WAIT;
        let mut job = Value::Null;
        while Instant::now() < deadline {
            let answer = emulator
                .ok(
                    "PollForJobs",
                    &json!({
                        "actionTypeId": {
                            "category": "Build",
                            "owner": "Custom",
                            "provider": "VariablesWorker",
                            "version": "1",
                        },
                        "maxBatchSize": 10,
                    }),
                )
                .await;
            if let Some(found) = answer["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|candidate| {
                    candidate["data"]["pipelineContext"]["pipelineExecutionId"] == json!(execution)
                })
            {
                job = found.clone();
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            job["id"].is_string(),
            "case {case:?}: the execution's custom action must queue a job within {WAIT:?}, \
             got {job}"
        );
        assert_eq!(
            job["data"]["actionConfiguration"]["configuration"]["Tag"],
            json!(served(&execution)),
            "case {case:?}: the worker must be served the resolved configuration, got {job}"
        );
    }
}

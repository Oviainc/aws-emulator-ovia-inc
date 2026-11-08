// SPDX-License-Identifier: CC0-1.0

//! A CodePipeline action of category `Build` provided by `CodeBuild` INVOKES that project,
//! and is settled from the build CodeBuild ran.
//!
//! The rule under test: **CodePipeline does not perform a `CodeBuild` action's work itself —
//! it starts a build of the project the action's `ProjectName` names, passing the
//! environment variables the action declares, and the action then follows THAT build: it
//! reports the build as its external execution and takes its outcome from the build's own
//! terminal status. A project the account does not hold is a declaration the pipeline cannot
//! run, which the action reports as a configuration error with no build started.**
//!
//! Three crates have to agree for that to hold (CodePipeline drives the action, CodeBuild
//! runs the build, Amazon S3 holds the pipeline's artifacts), so this suite lives here.
//!
//! What a build ENDS as is CodeBuild's own business and not this suite's: this emulator runs
//! no build container, so a build it accepts reaches the build-system failure status that
//! service reports for one (`Build$buildStatus`), and the assertions below hold the ACTION
//! to whatever the build reported — the state the build ended in, in CodePipeline's own
//! wording — rather than to a particular outcome. What they do pin is that the action is no
//! longer refused by the pipeline itself: the pipeline starts a real build, names it, and
//! says nothing about this emulator in the detail a client reads.
//!
//! The rows differ along whether the project the action names exists, and along the
//! environment variables the action declares.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region every call of this suite is signed for.
const REGION: &str = "us-east-1";

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model.
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// CodeBuild's `X-Amz-Target` prefix, from that service's own model.
const CODEBUILD_TARGET_PREFIX: &str = "CodeBuild_20161006";

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// How long a wait for one execution's next state may take before the suite calls it
/// stalled.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// How often a wait re-reads the reporting operation it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The trust policy a pipeline role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy a build project's service role carries.
const CODEBUILD_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codebuild.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
struct Emulator {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    fn send(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// One awsJson1_1 operation of `signing_name`, under `prefix`.
    fn json_rpc(
        &self,
        signing_name: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let (status, payload) = self.send(
            signing_name,
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", &format!("{prefix}.{operation}")),
            ],
            serde_json::to_vec(body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|_| {
                json!({ "unparsed": String::from_utf8_lossy(&payload).into_owned() })
            })
        };
        (status, parsed)
    }

    /// One CodePipeline operation that has to succeed.
    fn cp_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) =
            self.json_rpc("codepipeline", CODEPIPELINE_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: codepipeline {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One CodeBuild operation that has to succeed.
    fn cb_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) =
            self.json_rpc("codebuild", CODEBUILD_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: codebuild {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One IAM action (awsQuery, API version 2010-05-08).
    fn iam(&self, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (key, value) in members {
            body.push('&');
            body.push_str(&form_encode(key));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, payload) = self.send(
            "iam",
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// Create a role carrying `trust`, and answer its ARN.
    fn role(&self, name: &str, trust: &str) -> String {
        let (status, body) = self.iam(
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", trust)],
        );
        assert_eq!(
            status, 200,
            "CreateRole {name}: expected 200, got {status} with body {body}"
        );
        element(&body, "Arn")
    }

    /// Create a bucket and put the pipeline's source object into it.
    fn bucket_with_source(&self, case: &str, bucket: &str, key: &str, body: &[u8]) {
        let (status, payload) = self.send("s3", "PUT", &format!("/{bucket}"), &[], Vec::new());
        assert_eq!(
            status,
            200,
            "case {case:?}: CreateBucket {bucket} must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
        let (status, payload) =
            self.send("s3", "PUT", &format!("/{bucket}/{key}"), &[], body.to_vec());
        assert_eq!(
            status,
            200,
            "case {case:?}: PutObject {key} must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    /// The builds CodeBuild holds for `project`, newest first.
    fn builds_of(&self, case: &str, project: &str) -> Vec<String> {
        let answer = self.cb_ok(
            case,
            "ListBuildsForProject",
            &json!({ "projectName": project }),
        );
        answer["ids"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect()
    }

    /// One build as `BatchGetBuilds` reports it.
    fn build(&self, case: &str, id: &str) -> Value {
        let answer = self.cb_ok(case, "BatchGetBuilds", &json!({ "ids": [id] }));
        answer["builds"]
            .as_array()
            .and_then(|builds| builds.first())
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: CodeBuild must hold the build {id}"))
    }

    /// The state of one action of one stage, as `GetPipelineState` reports it.
    fn action_state(&self, case: &str, pipeline: &str, stage: &str, action: &str) -> Value {
        let answer = self.cp_ok(case, "GetPipelineState", &json!({ "name": pipeline }));
        answer["stageStates"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|state| state["stageName"].as_str() == Some(stage))
            .and_then(|state| {
                state["actionStates"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|state| state["actionName"].as_str() == Some(action))
            })
            .unwrap_or_else(|| panic!("case {case:?}: no state for {stage}/{action}"))
    }

    /// Start an execution of `pipeline` and answer its id.
    fn start(&self, case: &str, pipeline: &str) -> String {
        let answer = self.cp_ok(case, "StartPipelineExecution", &json!({ "name": pipeline }));
        answer["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start must report an id"))
            .to_string()
    }

    /// Wait until `id` reaches a terminal status, and answer it.
    fn settled(&self, case: &str, pipeline: &str, id: &str) -> String {
        settle(case, "a terminal execution status", || {
            let answer = self.cp_ok(
                case,
                "GetPipelineExecution",
                &json!({ "pipelineName": pipeline, "pipelineExecutionId": id }),
            );
            let status = answer["pipelineExecution"]["status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if TERMINAL.contains(&status.as_str()) {
                Ok(status)
            } else {
                Err(status)
            }
        })
    }

    /// Wait until every execution of `pipeline` has reached a terminal status.
    fn drained(&self, case: &str, pipeline: &str) {
        settle(case, "every execution settled", || {
            let answer = self.cp_ok(
                case,
                "ListPipelineExecutions",
                &json!({ "pipelineName": pipeline }),
            );
            let pending: Vec<Value> = answer["pipelineExecutionSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|summary| {
                    !TERMINAL.contains(&summary["status"].as_str().unwrap_or_default())
                })
                .collect();
            if pending.is_empty() {
                Ok(())
            } else {
                Err(format!("{pending:?}"))
            }
        });
    }
}

/// Poll `read` until it answers `Ok`, or fail naming the case and the last state observed.
fn settle<T>(case: &str, what: &str, mut read: impl FnMut() -> Result<T, String>) -> T {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    loop {
        match read() {
            Ok(value) => return value,
            Err(observed) => {
                assert!(
                    Instant::now() < deadline,
                    "case {case:?}: {what} was not reached within {SETTLE_TIMEOUT:?}; last \
                     observed {observed}"
                );
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
}

/// A pipeline whose Source stage reads one object and whose Build stage invokes `project`
/// with `environment` as the action's declared environment variables.
fn declaration(
    name: &str,
    role_arn: &str,
    bucket: &str,
    project: &str,
    environment: Option<&str>,
) -> Value {
    let mut configuration = json!({ "ProjectName": project });
    if let Some(environment) = environment {
        configuration["EnvironmentVariables"] = Value::String(environment.to_string());
    }
    json!({
        "name": name,
        "roleArn": role_arn,
        "artifactStore": { "type": "S3", "location": bucket },
        "stages": [
            {
                "name": "Source",
                "actions": [{
                    "name": "Src",
                    "actionTypeId": {
                        "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
                    },
                    "configuration": {
                        "S3Bucket": bucket,
                        "S3ObjectKey": "source.zip",
                        "PollForSourceChanges": "false",
                    },
                    "outputArtifacts": [{ "name": "Src" }],
                    "runOrder": 1,
                }],
            },
            {
                "name": "Build",
                "actions": [{
                    "name": "Build",
                    "actionTypeId": {
                        "category": "Build",
                        "owner": "AWS",
                        "provider": "CodeBuild",
                        "version": "1",
                    },
                    "configuration": configuration,
                    "inputArtifacts": [{ "name": "Src" }],
                    "runOrder": 1,
                }],
            },
        ],
    })
}

/// The Build action really invokes the project: CodeBuild holds exactly one build of it,
/// started with the environment variables the action declared, and the action reports that
/// build as its external execution and settles from the build's own terminal status —
/// saying nothing about this emulator in the detail a client reads.
#[test]
fn a_build_action_starts_a_build_of_its_project_and_settles_from_it() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let case = "a Build action naming a project of this account";
        let bucket = "cp-build-action";
        let project = "cp-build-project";
        let pipeline = "cp-build-pipeline";

        emulator.bucket_with_source(case, bucket, "source.zip", b"the source bytes");
        let pipeline_role = emulator.role("cp-build-pipeline-role", PIPELINE_TRUST);
        let project_role = emulator.role("cp-build-project-role", CODEBUILD_TRUST);
        emulator.cb_ok(
            case,
            "CreateProject",
            &json!({
                "name": project,
                "serviceRole": project_role,
                "source": { "type": "CODEPIPELINE", "buildspec": "version: 0.2" },
                "artifacts": { "type": "CODEPIPELINE" },
                "environment": {
                    "type": "LINUX_CONTAINER",
                    "image": "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                    "computeType": "BUILD_GENERAL1_SMALL",
                },
            }),
        );

        let declared_environment = json!([
            { "name": "IMAGE_TAG", "value": "release", "type": "PLAINTEXT" },
        ])
        .to_string();
        emulator.cp_ok(
            case,
            "CreatePipeline",
            &json!({
                "pipeline": declaration(
                    pipeline,
                    &pipeline_role,
                    bucket,
                    project,
                    Some(&declared_environment),
                ),
            }),
        );
        emulator.drained(case, pipeline);

        // Exactly one build per run of the action: the pipeline's own first execution.
        let after_create = emulator.builds_of(case, project);
        assert_eq!(
            after_create.len(),
            1,
            "case {case:?}: the Build action must have started one build of the project, got \
             {after_create:?}"
        );

        let execution = emulator.start(case, pipeline);
        let status = emulator.settled(case, pipeline, &execution);
        let builds = emulator.builds_of(case, project);
        assert_eq!(
            builds.len(),
            2,
            "case {case:?}: each run of the action starts one build, got {builds:?}"
        );

        let state = emulator.action_state(case, pipeline, "Build", "Build");
        let latest = &state["latestExecution"];
        let external = latest["externalExecutionId"].as_str().unwrap_or_default();
        assert!(
            builds.iter().any(|id| id == external),
            "case {case:?}: the action must report the build it started as its external \
             execution, got {latest} against the builds {builds:?}"
        );

        // The build CodeBuild holds for the action was started with what the action
        // declared: the project it names, and the environment variables of the action.
        let build = emulator.build(case, external);
        assert_eq!(
            build["projectName"].as_str(),
            Some(project),
            "case {case:?}: the build must be a build of the project the action named, got \
             {build}"
        );
        let variables = build["environment"]["environmentVariables"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            variables.iter().any(|variable| {
                variable["name"].as_str() == Some("IMAGE_TAG")
                    && variable["value"].as_str() == Some("release")
            }),
            "case {case:?}: the action's declared environment variables must reach the \
             build, got {variables:?}"
        );

        // The action's outcome is the BUILD's: whatever status that build ended in is what
        // the action and the execution report, and the detail a client reads names the
        // build's state rather than this emulator.
        let build_status = build["buildStatus"].as_str().unwrap_or_default();
        let action_status = latest["status"].as_str().unwrap_or_default();
        let detail = latest["errorDetails"]["message"]
            .as_str()
            .or_else(|| latest["summary"].as_str())
            .unwrap_or_default();
        if build_status == "SUCCEEDED" {
            assert_eq!(
                action_status, "Succeeded",
                "case {case:?}: a build that succeeded must settle the action Succeeded, got \
                 {latest} for a build {build}"
            );
            assert_eq!(
                status, "Succeeded",
                "case {case:?}: and the execution with it, got {status:?}"
            );
        } else {
            assert_eq!(
                action_status, "Failed",
                "case {case:?}: a build that did not succeed must settle the action Failed, \
                 got {latest} for a build {build}"
            );
            assert_eq!(
                detail,
                format!("Build terminated with state: {build_status}"),
                "case {case:?}: the action's detail must be the state the build ended in, got \
                 {detail:?}"
            );
            assert_eq!(
                status, "Failed",
                "case {case:?}: and the execution with it, got {status:?}"
            );
        }
        assert!(
            !detail.contains("emulator"),
            "case {case:?}: the detail a client reads must not announce the emulator, got \
             {detail:?}"
        );
    });
}

/// A Build action naming a project the account does not hold cannot be run as declared:
/// the action fails as a CONFIGURATION error, and no build is started anywhere.
#[test]
fn a_build_action_naming_no_project_fails_as_a_configuration_error() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let case = "a Build action naming a project nothing created";
        let bucket = "cp-build-absent";
        let pipeline = "cp-build-absent-pipeline";
        let project = "cp-build-never-created";

        emulator.bucket_with_source(case, bucket, "source.zip", b"the source bytes");
        let pipeline_role = emulator.role("cp-build-absent-role", PIPELINE_TRUST);
        emulator.cp_ok(
            case,
            "CreatePipeline",
            &json!({
                "pipeline": declaration(pipeline, &pipeline_role, bucket, project, None),
            }),
        );
        emulator.drained(case, pipeline);

        let state = emulator.action_state(case, pipeline, "Build", "Build");
        let latest = &state["latestExecution"];
        assert_eq!(
            latest["status"].as_str(),
            Some("Failed"),
            "case {case:?}: the action must fail, got {latest}"
        );
        assert_eq!(
            latest["errorDetails"]["code"].as_str(),
            Some("ConfigurationError"),
            "case {case:?}: a project the account does not hold is a configuration error, got \
             {latest}"
        );

        // And CodeBuild holds no build for a project it has none of: the refusal came from
        // that service, with nothing started.
        let (status, listed) = emulator.json_rpc(
            "codebuild",
            CODEBUILD_TARGET_PREFIX,
            "ListBuildsForProject",
            &json!({ "projectName": project }),
        );
        assert_eq!(
            status, 400,
            "case {case:?}: CodeBuild must still hold no such project, got {status} with \
             {listed}"
        );
    });
}

/// Run `body` on a thread with the stack the whole-router assembly needs.
fn on_a_sized_thread(body: impl FnOnce() + Send + 'static) {
    let handle = std::thread::Builder::new()
        .stack_size(BOOT_STACK_SIZE)
        .spawn(body)
        .expect("the sized thread starts");
    handle.join().expect("the suite's thread does not panic");
}

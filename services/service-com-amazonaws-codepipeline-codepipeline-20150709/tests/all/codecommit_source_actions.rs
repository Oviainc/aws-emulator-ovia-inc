// SPDX-License-Identifier: CC0-1.0

//! What a pipeline whose source lives in a CodeCommit repository DOES, end to end over
//! the wire each service speaks, with CodePipeline, CodeCommit, Lambda and S3 mounted on
//! one engine — so the execution plane reaches each of them the way the assembled
//! emulator does.
//!
//! The rules under test, each asserted over a SET of inputs rather than one case:
//!
//! * creating a pipeline RUNS it once: the pipeline carries exactly one execution from
//!   the moment it exists, and that execution's `trigger.triggerType` is the
//!   `CreatePipeline` member of `TriggerType`, whatever the pipeline declares;
//! * a `Source` action whose provider is `CodeCommit` resolves the branch its
//!   configuration names to that branch's TIP commit and produces the repository's files
//!   at that commit as its output artifact: the commit id is the revision the execution
//!   ran, the external execution the action reports, and the bytes a later action
//!   consumes;
//! * a branch the service cannot resolve is a declaration this pipeline cannot source
//!   from: the action fails with the `ConfigurationError` failure type and a summary
//!   saying why, produces no artifact, fails its execution, and the stage after it never
//!   starts — so no action execution exists for it and its state carries no
//!   `latestExecution`;
//! * an action whose provider is `Lambda` performs its work by INVOKING the function its
//!   configuration names, so the action reaches a terminal status reporting that function
//!   as its external execution, and what it reports is what the invocation answered.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for CodePipeline.
const PIPELINE_TARGET: &str = "CodePipeline_20150709";

/// The `X-Amz-Target` prefix CodeCommit's OPERATIONS.md records.
const COMMIT_TARGET: &str = "CodeCommit_20150413";

/// A `roleArn` satisfying `RoleArn`'s modelled pattern, for both the pipeline and the
/// Lambda function. IAM is not mounted here, so no role lookup has an answer and the
/// declarations are accepted (only a definitive "no such role" refuses one).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/cp";

/// How long a test waits for the clock-owed steps of one execution: each step is due
/// `state::execution::ACTION_STEP` (300 ms) after the one before it.
const WAIT: Duration = Duration::from_secs(20);

/// The `FailureType` an action carries when its declaration cannot be executed at all.
const CONFIGURATION_ERROR: &str = "ConfigurationError";

/// One emulator: the shared engine with CodePipeline, S3, CodeCommit and Lambda on it,
/// and the router that dispatches to all four.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsJson1_1 call to the service `signing_name` claims, under `target`.
    async fn json_call(
        &self,
        signing_name: &str,
        target: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.us-east-1.amazonaws.com/"))
            .header("host", format!("{signing_name}.us-east-1.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// One CodePipeline operation.
    async fn cp(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.json_call("codepipeline", PIPELINE_TARGET, operation, body)
            .await
    }

    /// One CodePipeline operation that must succeed.
    async fn cp_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.cp(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// One CodeCommit operation that must succeed.
    async fn cc_ok(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self
            .json_call("codecommit", COMMIT_TARGET, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
        response
    }

    /// One Lambda call: the service speaks restJson1, so each operation has its own
    /// method and URI.
    async fn lambda(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let payload = if body.is_null() {
            bytes::Bytes::new()
        } else {
            bytes::Bytes::from(body.to_string())
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://lambda.us-east-1.amazonaws.com{uri}"))
            .header("host", "lambda.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(payload)
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{method} {uri}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// One S3 request, path style, the way the emulated S3 service routes it.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
            .header("host", "s3.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), body.to_vec())
    }

    /// Create a bucket.
    async fn bucket(&self, name: &str) {
        let (status, body) = self.s3("PUT", &format!("/{name}"), b"").await;
        assert_eq!(
            status,
            200,
            "the bucket {name:?} must be created, got {status} with {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Read an object: its status and bytes.
    async fn object(&self, bucket: &str, key: &str) -> (u16, Vec<u8>) {
        self.s3("GET", &format!("/{bucket}/{key}"), b"").await
    }

    /// Create a repository holding `files` on `branch`, and answer the id of the commit
    /// that branch's tip ends up at. With no files the repository is created empty, so
    /// it has no branch at all and the answer is `None`.
    async fn repository(
        &self,
        name: &str,
        branch: &str,
        files: &[(&str, &[u8])],
    ) -> Option<String> {
        self.cc_ok("CreateRepository", &json!({"repositoryName": name}))
            .await;
        let mut tip: Option<String> = None;
        for (path, content) in files {
            let mut body = json!({
                "repositoryName": name,
                "branchName": branch,
                "filePath": path,
                // `fileContent` is a blob, which awsJson1_1 carries base64-encoded.
                "fileContent": base64::engine::general_purpose::STANDARD.encode(content),
                "commitMessage": format!("add {path}"),
            });
            if let Some(parent) = &tip {
                body["parentCommitId"] = json!(parent);
            }
            let answer = self.cc_ok("PutFile", &body).await;
            tip = Some(
                answer["commitId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("PutFile reports the commit it made, got {answer}"))
                    .to_string(),
            );
        }
        tip
    }

    /// Create a Lambda function and bring it to `Active`, which is the state an
    /// invocation needs.
    async fn function(&self, name: &str) {
        let (status, created) = self
            .lambda(
                "POST",
                "/2015-03-31/functions",
                &json!({
                    "FunctionName": name,
                    "Role": ROLE_ARN,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    // The deployment package is a blob, carried base64-encoded.
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD
                            .encode(b"a deployment package"),
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateFunction {name:?} must succeed, got {status} with {created}"
        );
        let deadline = Instant::now() + WAIT;
        let uri = format!("/2015-03-31/functions/{name}/configuration");
        loop {
            let (status, configuration) = self.lambda("GET", &uri, &Value::Null).await;
            assert_eq!(
                status, 200,
                "GetFunctionConfiguration {name:?} must succeed, got {configuration}"
            );
            if configuration["State"] == json!("Active") {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the function {name:?} never became Active, last {configuration}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// A `Source` action reading `branch` of `repository`, producing `artifact`.
fn code_commit_source(repository: &str, branch: &str, artifact: &str) -> Value {
    json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": {
                "category": "Source",
                "owner": "AWS",
                "provider": "CodeCommit",
                "version": "1",
            },
            "configuration": {"RepositoryName": repository, "BranchName": branch},
            "outputArtifacts": [{"name": artifact}],
            "namespace": "SourceVariables",
        }],
    })
}

/// A `Deploy` stage that extracts `artifact` into `bucket`, so every file of the source
/// revision is delivered as its own object.
fn extracting_deploy(artifact: &str, bucket: &str) -> Value {
    json!({
        "name": "Deploy",
        "actions": [{
            "name": "Dep",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {"BucketName": bucket, "Extract": "true"},
            "inputArtifacts": [{"name": artifact}],
        }],
    })
}

/// A manual-approval stage: the non-`Source` stage a declaration has to carry
/// (`state::validate_declaration`) when the row under test delivers nothing.
fn approval_gate() -> Value {
    json!({
        "name": "Gate",
        "actions": [{
            "name": "Approve",
            "actionTypeId": {
                "category": "Approval", "owner": "AWS", "provider": "Manual", "version": "1"
            },
        }],
    })
}

/// A `Deploy` stage whose action invokes the Lambda function `function`.
fn lambda_deploy(artifact: &str, function: &str) -> Value {
    json!({
        "name": "Deploy",
        "actions": [{
            "name": "Dep",
            "actionTypeId": {
                "category": "Invoke", "owner": "AWS", "provider": "Lambda", "version": "1"
            },
            "configuration": {"FunctionName": function, "UserParameters": "launch"},
            "inputArtifacts": [{"name": artifact}],
        }],
    })
}

/// A pipeline declaration named `name`, writing its artifacts to `artifact_store`.
fn declaration(name: &str, artifact_store: &str, stages: Vec<Value>) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": artifact_store},
        "stages": stages,
    }})
}

/// The summaries `ListPipelineExecutions` reports for `pipeline`, newest first.
async fn summaries(emulator: &Emulator, pipeline: &str) -> Vec<Value> {
    let listed = emulator
        .cp_ok("ListPipelineExecutions", &json!({"pipelineName": pipeline}))
        .await;
    listed["pipelineExecutionSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The one execution a freshly created pipeline carries: its summary.
async fn only_summary(emulator: &Emulator, pipeline: &str) -> Value {
    let listed = summaries(emulator, pipeline).await;
    assert_eq!(
        listed.len(),
        1,
        "creating {pipeline} runs it exactly once, got {listed:?}"
    );
    listed[0].clone()
}

/// Poll `ListPipelineExecutions` until the pipeline's newest execution reaches a
/// terminal status, and answer its summary.
async fn wait_for_terminal(emulator: &Emulator, pipeline: &str) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let listed = summaries(emulator, pipeline).await;
        let newest = listed.first().cloned().unwrap_or(Value::Null);
        let status = newest["status"].as_str().unwrap_or_default().to_string();
        if matches!(
            status.as_str(),
            "Succeeded" | "Failed" | "Stopped" | "Superseded" | "Cancelled"
        ) {
            return newest;
        }
        assert!(
            Instant::now() < deadline,
            "pipeline {pipeline}: no execution reached a terminal status within {WAIT:?}, \
             last {newest}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The action executions `ListActionExecutions` reports for `pipeline`.
async fn action_executions(emulator: &Emulator, pipeline: &str) -> Vec<Value> {
    let page = emulator
        .cp_ok("ListActionExecutions", &json!({"pipelineName": pipeline}))
        .await;
    page["actionExecutionDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The action execution reported for the action `action`, if there is one.
async fn action_execution(emulator: &Emulator, pipeline: &str, action: &str) -> Option<Value> {
    action_executions(emulator, pipeline)
        .await
        .into_iter()
        .find(|detail| detail["actionName"] == json!(action))
}

/// RULE: creating a pipeline runs it once, and that run's trigger is the
/// `CreatePipeline` member of `TriggerType`.
///
/// The rows differ along what the rule generalises over: what the pipeline does with the
/// revision it fetched, which provider its source action names, and whether that source
/// can be resolved at all — none of which decides whether the creation runs the pipeline.
#[tokio::test]
async fn creating_a_pipeline_runs_it_once_with_the_create_pipeline_trigger() {
    // (case, whether the second stage deploys the artifact, whether the source is seeded)
    let rows: Vec<(&str, bool, bool)> = vec![
        ("a deploying pipeline, a source that resolves", true, true),
        ("a deploying pipeline, a source that does not", true, false),
        ("a gated pipeline, a source that resolves", false, true),
        ("a gated pipeline, a source that does not", false, false),
    ];

    for (index, (case, deploys, seeded)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("created-{index}");
        let artifacts = format!("{pipeline}-artifacts");
        let delivered = format!("{pipeline}-delivered");
        let repository = format!("{pipeline}-repo");
        emulator.bucket(&artifacts).await;
        emulator.bucket(&delivered).await;
        let files: Vec<(&str, &[u8])> = if seeded {
            vec![("etl.py", b"print(\"etl\")\n")]
        } else {
            Vec::new()
        };
        emulator.repository(&repository, "main", &files).await;

        let mut stages = vec![code_commit_source(&repository, "main", "SourceArtifact")];
        if deploys {
            stages.push(extracting_deploy("SourceArtifact", &delivered));
        } else {
            stages.push(approval_gate());
        }
        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(&pipeline, &artifacts, stages),
            )
            .await;

        let summary = only_summary(&emulator, &pipeline).await;
        assert_eq!(
            summary["trigger"]["triggerType"],
            json!("CreatePipeline"),
            "case {case:?}: the run the creation started carries the CreatePipeline \
             trigger, got {summary}"
        );
        assert!(
            summary["pipelineExecutionId"]
                .as_str()
                .is_some_and(|id| id.len() == 36),
            "case {case:?}: the run carries a pipeline execution id, got {summary}"
        );

        // A start of the same pipeline is a DIFFERENT trigger, so the trigger reported
        // above is the creation's rather than a constant.
        emulator
            .cp_ok("StartPipelineExecution", &json!({"name": pipeline}))
            .await;
        let after = summaries(&emulator, &pipeline).await;
        let triggers: Vec<&str> = after
            .iter()
            .map(|summary| {
                summary["trigger"]["triggerType"]
                    .as_str()
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            triggers,
            vec!["StartPipelineExecution", "CreatePipeline"],
            "case {case:?}: each execution records what started it, got {after:?}"
        );
    }
}

/// RULE: a `CodeCommit` source action resolves the branch its configuration names to
/// that branch's tip commit and produces the repository's files at that commit as its
/// output artifact — so the commit id is the revision the execution ran and the external
/// execution the action reports, and the bytes a later action consumes are those files.
///
/// The rows differ along what the rule generalises over: the branch's name, how many
/// files the revision holds, their paths (including one under a folder) and their
/// content, and how many commits the branch's history has.
#[tokio::test]
async fn a_code_commit_source_action_runs_the_tip_of_the_branch_it_names() {
    // (case, branch, the files the branch holds)
    /// One row: the case, the branch the revision lives on, and the files it holds.
    type Row = (&'static str, &'static str, Vec<(&'static str, &'static [u8])>);

    let rows: Vec<Row> = vec![
        (
            "one file on main",
            "main",
            vec![("etl.py", b"print(\"etl\")\n" as &[u8])],
        ),
        (
            "three files, one of them under a folder",
            "develop",
            vec![
                ("etl.py", b"print(\"etl\")\n" as &[u8]),
                ("README.md", b"# the pipeline's source\n" as &[u8]),
                ("jobs/load.py", b"load()\n" as &[u8]),
            ],
        ),
        (
            "a branch whose name is not the default one",
            "release-1.0",
            vec![
                ("app.txt", b"the application\n" as &[u8]),
                ("app.txt.sha", b"0123456789\n" as &[u8]),
            ],
        ),
    ];

    for (index, (case, branch, files)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("sourced-{index}");
        let artifacts = format!("{pipeline}-artifacts");
        let delivered = format!("{pipeline}-delivered");
        let repository = format!("{pipeline}-repo");
        emulator.bucket(&artifacts).await;
        emulator.bucket(&delivered).await;
        let tip = emulator
            .repository(&repository, branch, &files)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the branch must have a tip"));

        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    &artifacts,
                    vec![
                        code_commit_source(&repository, branch, "SourceArtifact"),
                        extracting_deploy("SourceArtifact", &delivered),
                    ],
                ),
            )
            .await;

        let settled = wait_for_terminal(&emulator, &pipeline).await;
        assert_eq!(
            settled["status"],
            json!("Succeeded"),
            "case {case:?}: the run the creation started succeeded, got {settled}"
        );
        assert_eq!(
            settled["sourceRevisions"],
            json!([{"actionName": "Src", "revisionId": tip}]),
            "case {case:?}: the revision the execution ran is the branch tip, got {settled}"
        );

        let source = action_execution(&emulator, &pipeline, "Src")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the source action ran"));
        assert_eq!(
            source["status"],
            json!("Succeeded"),
            "case {case:?}: the source action performed its work, got {source}"
        );
        assert_eq!(
            source["output"]["executionResult"]["externalExecutionId"],
            json!(tip),
            "case {case:?}: the external execution is the commit it read, got {source}"
        );
        assert_eq!(
            source["output"]["outputArtifacts"][0]["s3location"]["bucket"],
            json!(artifacts),
            "case {case:?}: the artifact was produced into the artifact store, got {source}"
        );
        assert_eq!(
            source["output"]["outputVariables"]["CommitId"],
            json!(tip),
            "case {case:?}: the action publishes the commit it resolved, got {source}"
        );

        // The bytes really moved: the deploy stage extracted the artifact, so every file
        // of the revision is delivered as its own object with its own content.
        for (path, content) in &files {
            let (status, bytes) = emulator.object(&delivered, path).await;
            assert_eq!(
                (status, bytes.as_slice()),
                (200, *content),
                "case {case:?}: the file {path} of the revision must be delivered"
            );
        }
    }
}

/// RULE: a branch the service cannot resolve is a declaration the pipeline cannot source
/// from, so the source action fails with the `ConfigurationError` failure type and a
/// summary saying why, produces no artifact, fails its execution, and the stage after it
/// never starts.
///
/// The rows differ along every way the configured revision can be unreachable: no branch
/// at all, a branch of another name, and a repository that does not exist.
#[tokio::test]
async fn a_source_action_that_cannot_resolve_its_branch_fails_with_a_configuration_error() {
    // (case, the branch the repository holds — none for an empty repository, the branch
    //  the declaration names, whether the repository is created at all)
    let rows: Vec<(&str, Option<&str>, &str, bool)> = vec![
        ("a repository with no branches", None, "main", true),
        (
            "a repository whose only branch is another",
            Some("develop"),
            "main",
            true,
        ),
        ("a repository that does not exist", None, "main", false),
    ];

    for (index, (case, seeded_branch, configured, created)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let pipeline = format!("unsourceable-{index}");
        let artifacts = format!("{pipeline}-artifacts");
        let delivered = format!("{pipeline}-delivered");
        let repository = format!("{pipeline}-repo");
        emulator.bucket(&artifacts).await;
        emulator.bucket(&delivered).await;
        if created {
            let files: Vec<(&str, &[u8])> = match seeded_branch {
                Some(_) => vec![("etl.py", b"print(\"etl\")\n")],
                None => Vec::new(),
            };
            emulator
                .repository(&repository, seeded_branch.unwrap_or("main"), &files)
                .await;
        }

        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    &artifacts,
                    vec![
                        code_commit_source(&repository, configured, "SourceArtifact"),
                        extracting_deploy("SourceArtifact", &delivered),
                    ],
                ),
            )
            .await;

        let settled = wait_for_terminal(&emulator, &pipeline).await;
        assert_eq!(
            (
                settled["status"].clone(),
                settled["trigger"]["triggerType"].clone()
            ),
            (json!("Failed"), json!("CreatePipeline")),
            "case {case:?}: the run the creation started failed at its source, got {settled}"
        );
        assert_eq!(
            settled["sourceRevisions"],
            json!([]),
            "case {case:?}: no revision was resolved, so the list is empty, got {settled}"
        );

        let source = action_execution(&emulator, &pipeline, "Src")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the source action was reached"));
        assert_eq!(
            (
                source["status"].clone(),
                source["output"]["executionResult"]["errorDetails"]["code"].clone()
            ),
            (json!("Failed"), json!(CONFIGURATION_ERROR)),
            "case {case:?}: the action failed because its declaration cannot be run, got \
             {source}"
        );
        assert!(
            source["output"]["executionResult"]["externalExecutionSummary"]
                .as_str()
                .is_some_and(|summary| !summary.is_empty()),
            "case {case:?}: a failed action says what happened, got {source}"
        );
        assert_eq!(
            source["output"]["outputArtifacts"],
            json!([]),
            "case {case:?}: nothing was produced, so no artifact is reported, got {source}"
        );

        // The stage after the source never started, so it has no action execution at all
        // and its action's state carries no execution either.
        assert!(
            action_execution(&emulator, &pipeline, "Dep").await.is_none(),
            "case {case:?}: the deploy action never started, so it has no execution, got {:?}",
            action_executions(&emulator, &pipeline).await
        );
        let state = emulator
            .cp_ok("GetPipelineState", &json!({"name": pipeline}))
            .await;
        let deploy = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|stage| stage["stageName"] == json!("Deploy"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the deploy stage is reported, got {state}"));
        assert!(
            deploy["actionStates"][0].get("latestExecution").is_none(),
            "case {case:?}: an action no execution reached carries no latestExecution, got \
             {deploy}"
        );
        let (status, _) = emulator.object(&delivered, "etl.py").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a failed source delivers nothing downstream"
        );
    }
}

/// RULE: an action whose provider is `Lambda` performs its work by INVOKING the function
/// its configuration names: the action reaches a terminal status that reports that
/// function as its external execution, and what it reports is the outcome of that
/// invocation — a function the service does not hold cannot be invoked as declared
/// (`ConfigurationError`), and a function it does hold is invoked and the action reports
/// what the invocation answered.
///
/// Both rows run behind a CodeCommit source action that succeeds, so each also pins that
/// the stage after a source action that DID resolve its revision is reached.
#[tokio::test]
async fn a_lambda_action_performs_its_work_by_invoking_the_function_it_names() {
    // (case, whether the function exists)
    for (index, (case, exists)) in [
        ("a function the service holds", true),
        ("a function it does not", false),
    ]
    .into_iter()
    .enumerate()
    {
        let emulator = Emulator::new();
        let pipeline = format!("invoking-{index}");
        let artifacts = format!("{pipeline}-artifacts");
        let repository = format!("{pipeline}-repo");
        let function = format!("{pipeline}-launcher");
        emulator.bucket(&artifacts).await;
        let tip = emulator
            .repository(&repository, "main", &[("etl.py", b"print(\"etl\")\n")])
            .await
            .expect("the branch has a tip");
        if exists {
            emulator.function(&function).await;
        }

        emulator
            .cp_ok(
                "CreatePipeline",
                &declaration(
                    &pipeline,
                    &artifacts,
                    vec![
                        code_commit_source(&repository, "main", "SourceArtifact"),
                        lambda_deploy("SourceArtifact", &function),
                    ],
                ),
            )
            .await;

        wait_for_terminal(&emulator, &pipeline).await;

        // The source action ran its revision, which is what let the execution reach the
        // invoking action at all.
        let source = action_execution(&emulator, &pipeline, "Src")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the source action ran"));
        assert_eq!(
            (
                source["status"].clone(),
                source["output"]["executionResult"]["externalExecutionId"].clone()
            ),
            (json!("Succeeded"), json!(tip)),
            "case {case:?}: the source action read the branch tip, got {source}"
        );

        let deploy = action_execution(&emulator, &pipeline, "Dep")
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the invoking action was reached"));
        let status = deploy["status"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            deploy["output"]["executionResult"]["externalExecutionId"],
            json!(function),
            "case {case:?}: the external execution is the function it invoked, got {deploy}"
        );
        let error = &deploy["output"]["executionResult"]["errorDetails"];
        if !exists {
            // A function the service does not hold cannot be invoked as the declaration
            // states it, so this row has ONE admissible outcome.
            assert_eq!(
                (status.clone(), error["code"].clone()),
                ("Failed".to_string(), json!(CONFIGURATION_ERROR)),
                "case {case:?}: a function that is not there cannot be invoked, got {deploy}"
            );
        } else {
            // A function the service does holds IS invoked, and the action reports what
            // the invocation answered — which is an outcome either way.
            assert!(
                matches!(status.as_str(), "Succeeded" | "Failed"),
                "case {case:?}: the invoking action reaches an outcome, got {deploy}"
            );
            if status == "Failed" {
                assert_eq!(
                    error["code"],
                    json!("JobFailed"),
                    "case {case:?}: an invocation that did not run the function is the \
                     run's own failure, got {deploy}"
                );
            }
        }
        if status == "Failed" {
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(&function)),
                "case {case:?}: a failed invocation says which function it was, got {deploy}"
            );
        }
    }
}

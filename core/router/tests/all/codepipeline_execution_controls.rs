// SPDX-License-Identifier: CC0-1.0

//! CodePipeline's execution plane and its operational CONTROLS, driven end to end over the
//! assembled router: a started execution runs every stage of the pipeline to a terminal
//! `PipelineExecutionStatus`, a disabled stage transition withholds exactly one boundary
//! until it is enabled again, `RetryStageExecution` re-runs a failed stage, and
//! `RollbackStage` re-runs a stage with the artifacts of the execution it targets.
//!
//! The rule under test: **an execution's progress is a function of the stored declaration,
//! the artifacts in the artifact store and the CLOCK — so every action of a stage is
//! performed, in run order, against the services that own the work (Amazon S3 here), an
//! execution is never held out of the pipeline because another one is in flight (a STAGE
//! takes one execution at a time), and each operational control changes only what it names:
//! a disabled transition holds entry to its own stage while every earlier stage still
//! completes, a retry re-runs the failed stage of the latest execution, and a rollback
//! re-runs one stage with the target execution's artifacts.**
//!
//! Three crates have to agree for that to hold (CodePipeline drives it, Amazon S3 holds the
//! source objects, artifacts and delivered objects, IAM holds the pipeline role), so this
//! suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: how many stages a pipeline
//! declares, how many actions one stage declares and at which run orders, which boundary is
//! held, what an earlier execution is waiting for, which `StageRetryMode` a retry asks for,
//! and which execution a rollback targets.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. Nothing here sleeps for an execution to finish: it
//! POLLS the reporting operations, exactly as a client does, because every step of an
//! execution is materialised by the call that comes after the clock made it due.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model (service shape
/// `CodePipeline_20150709`).
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The Region every call of this suite is signed for. One Region keeps `CreateBucket`
/// free of a `LocationConstraint`, which is not what is under test here.
const REGION: &str = "us-east-1";

/// How long a wait for one execution's next state may take before the suite calls it
/// stalled. Every step of an execution falls due a few hundred milliseconds after the one
/// before it, so a pipeline of a handful of actions settles in well under a second; this is
/// a generous ceiling, not an expectation.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// How often a wait re-reads the reporting operation it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a "nothing moved" assertion watches a held execution. Longer than several of
/// the engine's per-step spans, so a boundary that was going to be crossed would have been.
const HELD_WINDOW: Duration = Duration::from_millis(1_500);

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 4] = ["Succeeded", "Failed", "Stopped", "Superseded"];

/// The trust policy a pipeline role carries.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

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
            // A MULTI-thread runtime: an action's work is performed through in-process
            // calls into other services of this same router, and the archive decoding a
            // deploy action does runs on the blocking pool.
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    /// Drive one request and answer its status, headers and body.
    fn send(
        &self,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, http::HeaderMap, Vec<u8>) {
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
        let headers = response.headers().clone();
        (status, headers, response.into_body().to_vec())
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
        let (status, _, payload) = self.send(
            "iam",
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// One awsJson1_1 CodePipeline operation.
    fn cp(&self, operation: &str, body: Value) -> (u16, Value) {
        let (status, _, payload) = self.send(
            "codepipeline",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", &format!("{TARGET_PREFIX}.{operation}")),
            ],
            serde_json::to_vec(&body).expect("the request body serializes"),
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
    fn cp_ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, answer) = self.cp(operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One S3 request against a bucket or object path.
    fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let (status, _, payload) = self.send("s3", method, uri, &[], body.to_vec());
        (status, payload)
    }

    /// Create the role a pipeline declares, and answer its ARN.
    fn role(&self, name: &str) -> String {
        let (status, body) = self.iam(
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        );
        assert_eq!(
            status, 200,
            "CreateRole for {name} must answer 200, got {status} with body {body}"
        );
        element(&body, "Arn")
    }

    /// Create a bucket and put the source object into it.
    fn bucket_with_source(&self, case: &str, bucket: &str, key: &str, body: &[u8]) {
        let (status, answer) = self.s3("PUT", &format!("/{bucket}"), b"");
        assert_eq!(
            status,
            200,
            "case {case:?}: CreateBucket {bucket} must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&answer)
        );
        self.put_object(case, bucket, key, body);
    }

    fn put_object(&self, case: &str, bucket: &str, key: &str, body: &[u8]) {
        let (status, answer) = self.s3("PUT", &format!("/{bucket}/{key}"), body);
        assert_eq!(
            status,
            200,
            "case {case:?}: PutObject s3://{bucket}/{key} must answer 200, got {status} with \
             {:?}",
            String::from_utf8_lossy(&answer)
        );
    }

    /// The bytes of one object, or `None` where S3 holds no such object.
    fn object(&self, bucket: &str, key: &str) -> Option<Vec<u8>> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), b"");
        (status == 200).then_some(body)
    }

    /// One execution's reported status.
    fn execution_status(&self, case: &str, pipeline: &str, id: &str) -> String {
        let answer = self.cp_ok(
            case,
            "GetPipelineExecution",
            json!({ "pipelineName": pipeline, "pipelineExecutionId": id }),
        );
        answer["pipelineExecution"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Every action of one execution as `(stageName, actionName, status)`, as
    /// `ListActionExecutions` reports it.
    fn action_executions(&self, case: &str, pipeline: &str, id: &str) -> Vec<(String, String, String)> {
        let answer = self.cp_ok(
            case,
            "ListActionExecutions",
            json!({ "pipelineName": pipeline, "filter": { "pipelineExecutionId": id } }),
        );
        answer["actionExecutionDetails"]
            .as_array()
            .map(|details| {
                details
                    .iter()
                    .map(|detail| {
                        (
                            detail["stageName"].as_str().unwrap_or_default().to_string(),
                            detail["actionName"].as_str().unwrap_or_default().to_string(),
                            detail["status"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Start an execution of `pipeline` and answer its id.
    fn start(&self, case: &str, pipeline: &str) -> String {
        let answer = self.cp_ok(case, "StartPipelineExecution", json!({ "name": pipeline }));
        answer["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the start must report an id, got {answer}"))
            .to_string()
    }

    /// Wait until one execution reaches a terminal status, and answer it.
    fn settled(&self, case: &str, pipeline: &str, id: &str) -> String {
        settle(case, &format!("a terminal status for execution {id}"), || {
            let status = self.execution_status(case, pipeline, id);
            if TERMINAL.contains(&status.as_str()) {
                Ok(status)
            } else {
                Err(format!(
                    "status {status:?} with actions {:?}",
                    self.action_executions(case, pipeline, id)
                ))
            }
        })
    }

    /// Wait until every execution of `pipeline` has reached a terminal status.
    ///
    /// `CreatePipeline` starts an execution of its own, so a scenario that wants to observe
    /// ITS OWN execution waits for that one to finish first — in `QUEUED` execution mode a
    /// pipeline runs one execution at a time.
    fn drained(&self, case: &str, pipeline: &str) {
        settle(case, "every earlier execution settled", || {
            let answer = self.cp_ok(case, "ListPipelineExecutions", json!({ "pipelineName": pipeline }));
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

    /// Create a pipeline from the declaration, and wait for the execution `CreatePipeline`
    /// starts of its own to settle.
    fn pipeline(&self, case: &str, declaration: Value) {
        let name = declaration["name"].as_str().expect("a named declaration").to_string();
        self.cp_ok(case, "CreatePipeline", json!({ "pipeline": declaration }));
        self.drained(case, &name);
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

/// An S3 source action: it reads one object and produces the output artifact.
fn source_action(name: &str, bucket: &str, key: &str, output: &str) -> Value {
    json!({
        "name": name,
        "actionTypeId": { "category": "Source", "owner": "AWS", "provider": "S3", "version": "1" },
        "configuration": { "S3Bucket": bucket, "S3ObjectKey": key, "PollForSourceChanges": "false" },
        "outputArtifacts": [{ "name": output }],
        "runOrder": 1,
    })
}

/// An S3 deploy action: it reads the input artifact and delivers it as one object.
fn deploy_action(name: &str, bucket: &str, key: &str, input: &str, run_order: u32) -> Value {
    json!({
        "name": name,
        "actionTypeId": { "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1" },
        "configuration": { "BucketName": bucket, "ObjectKey": key, "Extract": "false" },
        "inputArtifacts": [{ "name": input }],
        "runOrder": run_order,
    })
}

/// A pipeline declaration over `stages`, with `bucket` as its artifact store.
fn declaration(name: &str, role_arn: &str, bucket: &str, stages: Vec<Value>) -> Value {
    json!({
        "name": name,
        "roleArn": role_arn,
        "artifactStore": { "type": "S3", "location": bucket },
        "stages": stages,
    })
}

/// A started execution runs EVERY stage of the declaration, in run order, and reaches
/// `Succeeded` with each declared delivery performed.
fn an_execution_runs_every_declared_action(emulator: &Emulator, role: &str) {
    // (case, the stages, the delivered keys the execution must have written)
    let rows: Vec<(&str, Vec<Value>, Vec<&str>)> = vec![
        (
            "two stages, one deploy action",
            vec![
                json!({ "name": "Source", "actions": [source_action("Src", "cp-run-a", "source.zip", "Art")] }),
                json!({ "name": "Deploy", "actions": [deploy_action("Dep", "cp-run-a", "out/one.zip", "Art", 1)] }),
            ],
            vec!["out/one.zip"],
        ),
        (
            "two actions of one stage at different run orders",
            vec![
                json!({ "name": "Source", "actions": [source_action("Src", "cp-run-b", "source.zip", "Art")] }),
                json!({ "name": "Deploy", "actions": [
                    deploy_action("First", "cp-run-b", "out/first.zip", "Art", 1),
                    deploy_action("Second", "cp-run-b", "out/second.zip", "Art", 2),
                ] }),
            ],
            vec!["out/first.zip", "out/second.zip"],
        ),
        (
            "three stages",
            vec![
                json!({ "name": "Source", "actions": [source_action("Src", "cp-run-c", "source.zip", "Art")] }),
                json!({ "name": "DeployA", "actions": [deploy_action("Dep", "cp-run-c", "out/a.zip", "Art", 1)] }),
                json!({ "name": "DeployB", "actions": [deploy_action("Dep", "cp-run-c", "out/b.zip", "Art", 1)] }),
            ],
            vec!["out/a.zip", "out/b.zip"],
        ),
    ];

    for (index, (case, stages, delivered)) in rows.into_iter().enumerate() {
        let bucket = format!("cp-run-{}", (b'a' + index as u8) as char);
        let pipeline = format!("cp-run-{index}");
        let source = format!("the source bytes of row {index}").into_bytes();
        emulator.bucket_with_source(case, &bucket, "source.zip", &source);
        emulator.pipeline(case, declaration(&pipeline, role, &bucket, stages));

        let execution = emulator.start(case, &pipeline);
        let status = emulator.settled(case, &pipeline, &execution);
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: the execution must run every stage to Succeeded, got {status:?} \
             with actions {:?}",
            emulator.action_executions(case, &pipeline, &execution)
        );
        for key in delivered {
            assert_eq!(
                emulator.object(&bucket, key).as_deref(),
                Some(source.as_slice()),
                "case {case:?}: the deploy action must have delivered s3://{bucket}/{key}"
            );
        }
        let reported = emulator.action_executions(case, &pipeline, &execution);
        assert!(
            !reported.is_empty()
                && reported
                    .iter()
                    .all(|(_, _, status)| status == "Succeeded"),
            "case {case:?}: every action of the execution must be reported Succeeded, got \
             {reported:?}"
        );
    }
}

/// A disabled INBOUND transition withholds entry to exactly the stage it names: every
/// earlier stage of the execution still completes, the execution stays `InProgress`, and
/// enabling the transition lets it run to `Succeeded`.
fn a_disabled_transition_holds_only_its_own_boundary(emulator: &Emulator, role: &str) {
    // (case, the held stage, the stages that must complete while it is held)
    let rows: [(&str, &str, &[&str]); 2] = [
        ("the boundary into the second stage", "DeployA", &["Source"]),
        (
            "the boundary into the third stage",
            "DeployB",
            &["Source", "DeployA"],
        ),
    ];

    for (index, (case, held, before)) in rows.into_iter().enumerate() {
        let bucket = format!("cp-hold-{index}");
        let pipeline = format!("cp-hold-{index}");
        let source = format!("the held source bytes of row {index}").into_bytes();
        emulator.bucket_with_source(case, &bucket, "source.zip", &source);
        emulator.pipeline(
            case,
            declaration(
                &pipeline,
                role,
                &bucket,
                vec![
                    json!({ "name": "Source", "actions": [source_action("Src", &bucket, "source.zip", "Art")] }),
                    json!({ "name": "DeployA", "actions": [deploy_action("Dep", &bucket, "out/a.zip", "Art", 1)] }),
                    json!({ "name": "DeployB", "actions": [deploy_action("Dep", &bucket, "out/b.zip", "Art", 1)] }),
                ],
            ),
        );

        emulator.cp_ok(
            case,
            "DisableStageTransition",
            json!({
                "pipelineName": pipeline,
                "stageName": held,
                "transitionType": "Inbound",
                "reason": "held by the suite",
            }),
        );

        let execution = emulator.start(case, &pipeline);

        // Every stage BEFORE the held boundary completes: the hold is on entering one
        // stage, not on the work already running.
        settle(case, "every stage before the held boundary Succeeded", || {
            let reported = emulator.action_executions(case, &pipeline, &execution);
            let complete = before.iter().all(|stage| {
                reported
                    .iter()
                    .any(|(reported_stage, _, status)| reported_stage == stage && status == "Succeeded")
            });
            if complete {
                Ok(())
            } else {
                Err(format!("{reported:?}"))
            }
        });

        // And the execution PARKS there: the held stage does not start, and the execution
        // stays InProgress for as long as the hold lasts.
        let watch = Instant::now() + HELD_WINDOW;
        while Instant::now() < watch {
            let status = emulator.execution_status(case, &pipeline, &execution);
            assert_eq!(
                status, "InProgress",
                "case {case:?}: a held execution stays InProgress, got {status:?} with actions \
                 {:?}",
                emulator.action_executions(case, &pipeline, &execution)
            );
            let reported = emulator.action_executions(case, &pipeline, &execution);
            assert!(
                !reported.iter().any(|(stage, _, _)| stage == held),
                "case {case:?}: the held stage {held} must not have started, got {reported:?}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }

        emulator.cp_ok(
            case,
            "EnableStageTransition",
            json!({ "pipelineName": pipeline, "stageName": held, "transitionType": "Inbound" }),
        );

        let status = emulator.settled(case, &pipeline, &execution);
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: enabling the transition must let the execution finish, got \
             {status:?} with actions {:?}",
            emulator.action_executions(case, &pipeline, &execution)
        );
        for key in ["out/a.zip", "out/b.zip"] {
            assert_eq!(
                emulator.object(&bucket, key).as_deref(),
                Some(source.as_slice()),
                "case {case:?}: the released execution must have delivered s3://{bucket}/{key}"
            );
        }
    }
}

/// `RetryStageExecution` re-runs the failed stage of the latest execution: once the reason
/// it failed is gone, the retried stage completes and the execution reaches `Succeeded`.
fn retrying_a_failed_stage_runs_it_again(emulator: &Emulator, role: &str) {
    // (case, the retryMode asked for)
    for (index, (case, retry_mode)) in [
        ("retrying only the failed actions", "FAILED_ACTIONS"),
        ("retrying every action of the stage", "ALL_ACTIONS"),
    ]
    .into_iter()
    .enumerate()
    {
        let bucket = format!("cp-retry-{index}");
        let pipeline = format!("cp-retry-{index}");
        let source = format!("the retried source bytes of row {index}").into_bytes();
        // The bucket exists but the source object does NOT, so the source action fails.
        let (status, answer) = emulator.s3("PUT", &format!("/{bucket}"), b"");
        assert_eq!(
            status,
            200,
            "case {case:?}: CreateBucket must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&answer)
        );
        emulator.pipeline(
            case,
            declaration(
                &pipeline,
                role,
                &bucket,
                vec![
                    json!({ "name": "Source", "actions": [source_action("Src", &bucket, "source.zip", "Art")] }),
                    json!({ "name": "Deploy", "actions": [deploy_action("Dep", &bucket, "out/retried.zip", "Art", 1)] }),
                ],
            ),
        );

        let execution = emulator.start(case, &pipeline);
        let status = emulator.settled(case, &pipeline, &execution);
        assert_eq!(
            status, "Failed",
            "case {case:?}: an execution whose source object is missing must fail, got \
             {status:?} with actions {:?}",
            emulator.action_executions(case, &pipeline, &execution)
        );
        assert_eq!(
            emulator.object(&bucket, "out/retried.zip"),
            None,
            "case {case:?}: a failed execution delivers nothing"
        );

        // The reason it failed is gone, so the retried stage can complete.
        emulator.put_object(case, &bucket, "source.zip", &source);
        emulator.cp_ok(
            case,
            "RetryStageExecution",
            json!({
                "pipelineName": pipeline,
                "pipelineExecutionId": execution,
                "stageName": "Source",
                "retryMode": retry_mode,
            }),
        );

        let status = emulator.settled(case, &pipeline, &execution);
        assert_eq!(
            status, "Succeeded",
            "case {case:?}: the retried stage must carry the execution to Succeeded, got \
             {status:?} with actions {:?}",
            emulator.action_executions(case, &pipeline, &execution)
        );
        assert_eq!(
            emulator.object(&bucket, "out/retried.zip").as_deref(),
            Some(source.as_slice()),
            "case {case:?}: the retried execution must have delivered the artifact"
        );
    }
}

/// `RollbackStage` re-runs one stage with the ARTIFACTS of the execution it targets, so the
/// state that execution left is restored.
fn rolling_a_stage_back_restores_the_targets_state(emulator: &Emulator, role: &str) {
    let case = "rolling the deploy stage back to the first execution";
    let bucket = "cp-rollback";
    let pipeline = "cp-rollback";
    let first = b"the first revision".as_slice();
    let second = b"the second revision".as_slice();
    emulator.bucket_with_source(case, bucket, "source.zip", first);
    emulator.pipeline(
        case,
        declaration(
            pipeline,
            role,
            bucket,
            vec![
                json!({ "name": "Source", "actions": [source_action("Src", bucket, "source.zip", "Art")] }),
                json!({ "name": "Deploy", "actions": [deploy_action("Dep", bucket, "out/live.zip", "Art", 1)] }),
            ],
        ),
    );

    let earlier = emulator.start(case, pipeline);
    let status = emulator.settled(case, pipeline, &earlier);
    assert_eq!(
        status, "Succeeded",
        "case {case:?}: the first execution must succeed, got {status:?}"
    );
    assert_eq!(
        emulator.object(bucket, "out/live.zip").as_deref(),
        Some(first),
        "case {case:?}: the first execution delivers the first revision"
    );

    emulator.put_object(case, bucket, "source.zip", second);
    let later = emulator.start(case, pipeline);
    let status = emulator.settled(case, pipeline, &later);
    assert_eq!(
        status, "Succeeded",
        "case {case:?}: the second execution must succeed, got {status:?}"
    );
    assert_eq!(
        emulator.object(bucket, "out/live.zip").as_deref(),
        Some(second),
        "case {case:?}: the second execution delivers the second revision"
    );

    let answer = emulator.cp_ok(
        case,
        "RollbackStage",
        json!({
            "pipelineName": pipeline,
            "stageName": "Deploy",
            "targetPipelineExecutionId": earlier,
        }),
    );
    let rollback = answer["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the rollback must report an id, got {answer}"))
        .to_string();
    assert_ne!(
        rollback, earlier,
        "case {case:?}: a rollback is an execution of its own"
    );

    let status = emulator.settled(case, pipeline, &rollback);
    assert_eq!(
        status, "Succeeded",
        "case {case:?}: the rollback execution must succeed, got {status:?} with actions {:?}",
        emulator.action_executions(case, pipeline, &rollback)
    );
    assert_eq!(
        emulator.object(bucket, "out/live.zip").as_deref(),
        Some(first),
        "case {case:?}: the rollback must restore what the target execution deployed"
    );
    let described = emulator.cp_ok(
        case,
        "GetPipelineExecution",
        json!({ "pipelineName": pipeline, "pipelineExecutionId": rollback }),
    );
    assert_eq!(
        described["pipelineExecution"]["executionType"].as_str(),
        Some("ROLLBACK"),
        "case {case:?}: the rollback reports itself as one, got {described}"
    );
}

/// An execution started while an earlier one is parked at a manual approval is NOT held
/// out of the pipeline: it runs the stages whose turn is free — its source stage first —
/// and waits only at the boundary of the stage the parked execution holds.
fn a_second_execution_runs_while_the_first_waits_for_an_approval(emulator: &Emulator, role: &str) {
    let case = "a second execution while the first waits for an approval";
    let bucket = "cp-parked";
    let pipeline = "cp-parked";
    let source = b"the parked source bytes".as_slice();
    let (status, answer) = emulator.s3("PUT", &format!("/{bucket}"), b"");
    assert_eq!(
        status,
        200,
        "case {case:?}: CreateBucket must answer 200, got {status} with {:?}",
        String::from_utf8_lossy(&answer)
    );
    emulator.put_object(case, bucket, "source.zip", source);
    // NOT drained: `CreatePipeline` starts an execution of its own, and this scenario is
    // about what a SECOND execution does while that one is parked at the approval.
    emulator.cp_ok(
        case,
        "CreatePipeline",
        json!({ "pipeline": declaration(
            pipeline,
            role,
            bucket,
            vec![
                json!({ "name": "Source", "actions": [source_action("Src", bucket, "source.zip", "Art")] }),
                json!({ "name": "Approve", "actions": [{
                    "name": "Gate",
                    "actionTypeId": { "category": "Approval", "owner": "AWS", "provider": "Manual", "version": "1" },
                    "runOrder": 1,
                }] }),
                json!({ "name": "Deploy", "actions": [deploy_action("Dep", bucket, "out/parked.zip", "Art", 1)] }),
            ],
        ) }),
    );

    // The pipeline's own execution parks at the approval.
    settle(case, "the first execution parked at the approval", || {
        let state = emulator.cp_ok(case, "GetPipelineState", json!({ "name": pipeline }));
        let token = state["stageStates"]
            .as_array()
            .and_then(|stages| {
                stages
                    .iter()
                    .find(|stage| stage["stageName"] == json!("Approve"))
                    .and_then(|stage| stage["actionStates"][0]["latestExecution"]["token"].as_str())
                    .map(str::to_string)
            });
        token.ok_or_else(|| format!("{state}"))
    });

    let second = emulator.start(case, pipeline);
    settle(case, "the second execution's Source stage Succeeded", || {
        let reported = emulator.action_executions(case, pipeline, &second);
        if reported
            .iter()
            .any(|(stage, _, status)| stage == "Source" && status == "Succeeded")
        {
            Ok(())
        } else {
            Err(format!(
                "actions {reported:?}, status {:?}",
                emulator.execution_status(case, pipeline, &second)
            ))
        }
    });
}

#[test]
fn codepipeline_executions_and_their_controls_run_to_a_terminal_status() {
    std::thread::Builder::new()
        .name("codepipeline-execution-controls".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::new();
            let role = emulator.role("cp-execution-controls");
            a_second_execution_runs_while_the_first_waits_for_an_approval(&emulator, &role);
            an_execution_runs_every_declared_action(&emulator, &role);
            a_disabled_transition_holds_only_its_own_boundary(&emulator, &role);
            retrying_a_failed_stage_runs_it_again(&emulator, &role);
            rolling_a_stage_back_restores_the_targets_state(&emulator, &role);
        })
        .expect("the suite's thread starts")
        .join()
        .expect("the suite's thread runs to completion");
}

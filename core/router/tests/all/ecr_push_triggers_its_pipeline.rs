// SPDX-License-Identifier: CC0-1.0

//! An image PUSH to Amazon ECR starts the CodePipeline pipeline an EventBridge rule
//! targets, and the execution it starts sources that image.
//!
//! The rule under test: **a push to an Amazon ECR repository publishes an `ECR Image
//! Action` event onto the account's default event bus (source `aws.ecr`, with the
//! repository, the action type, the image's digest and its tag in the event's `detail`); a
//! rule whose `EventPattern` matches it delivers the event to its targets, and a
//! CodePipeline target is delivered to by starting one execution of that pipeline under the
//! role the target names — so a rule whose filter does not match the push starts nothing,
//! and a target role that does not allow `codepipeline:StartPipelineExecution` starts
//! nothing while a correctly permissioned sibling rule still does. The execution that runs
//! then resolves the image its Amazon ECR source action names and records that image's
//! DIGEST as the action's source revision.**
//!
//! Five crates have to agree for that to hold (Amazon ECR publishes, EventBridge routes and
//! delivers, IAM holds the target role, CodePipeline runs the execution and Amazon S3 holds
//! its artifacts), so this suite lives here: a crate-local test can only ever see one side
//! of it.
//!
//! The rows differ along what the rule generalises over: which tag the push carries against
//! the tag the rule filters on, how many pushes are made (one image per push, each with its
//! own digest), whether the repository the push names is the one the pattern names, and
//! whether the target's role carries the grant the invocation needs.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. Nothing here sleeps for an execution to finish: it
//! POLLS the reporting operations, exactly as a client does.

use std::collections::BTreeSet;
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

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model.
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// Amazon ECR's `X-Amz-Target` prefix, from that service's own model.
const ECR_TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// EventBridge's `X-Amz-Target` prefix, from that service's own model.
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// How long a wait for one execution's next state may take before the suite calls it
/// stalled. A generous ceiling, not an expectation.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// How often a wait re-reads the reporting operation it polls.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a "nothing was started" assertion watches the pipeline. Longer than several of
/// the execution engine's per-step spans and of the delivery path, so an execution that was
/// going to be started would have been.
const NOTHING_STARTED_WINDOW: Duration = Duration::from_millis(1_500);

/// The trust policy a pipeline role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy the role an EventBridge target names carries: EventBridge invokes the
/// target AS that role, so the role has to let that service assume it.
const EVENTS_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"events.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

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
            // A MULTI-thread runtime: a push delivers its event through in-process calls
            // into other services of this same router, and an action's work runs there
            // too.
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    /// Drive one request and answer its status and body.
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
    fn json_rpc(&self, signing_name: &str, prefix: &str, operation: &str, body: &Value) -> (u16, Value) {
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

    /// One Amazon ECR operation that has to succeed.
    fn ecr_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) = self.json_rpc("ecr", ECR_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: ecr {operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// One EventBridge operation that has to succeed.
    fn events_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) = self.json_rpc("events", EVENTS_TARGET_PREFIX, operation, body);
        assert_eq!(
            status, 200,
            "case {case:?}: events {operation} must answer 200, got {status} with {answer}"
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

    /// Attach an inline policy to `role` that allows `action` on `resource`.
    fn allow(&self, role: &str, action: &str, resource: &str) {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}],
        })
        .to_string();
        let (status, body) = self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", role),
                ("PolicyName", "allow-start"),
                ("PolicyDocument", &document),
            ],
        );
        assert_eq!(
            status, 200,
            "PutRolePolicy {role}: expected 200, got {status} with body {body}"
        );
    }

    /// Create a bucket for a pipeline's artifact store and its deliveries.
    fn bucket(&self, name: &str) {
        let (status, payload) = self.send("s3", "PUT", &format!("/{name}"), &[], Vec::new());
        assert_eq!(
            status,
            200,
            "CreateBucket {name}: expected 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    /// The bytes of one object, or `None` where S3 holds no such object.
    fn object(&self, bucket: &str, key: &str) -> Option<Vec<u8>> {
        let (status, body) = self.send("s3", "GET", &format!("/{bucket}/{key}"), &[], Vec::new());
        (status == 200).then_some(body)
    }

    /// Push one image manifest under `tag`, and answer the digest the registry stored it
    /// under.
    fn push(&self, case: &str, repository: &str, manifest: &str, tag: &str) -> String {
        let pushed = self.ecr_ok(
            case,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
        );
        pushed["image"]["imageId"]["imageDigest"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the push must report a digest, got {pushed}"))
            .to_string()
    }

    /// The ids of every execution of `pipeline`, as `ListPipelineExecutions` reports them.
    fn execution_ids(&self, case: &str, pipeline: &str) -> BTreeSet<String> {
        let answer = self.cp_ok(
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        answer["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|summary| summary["pipelineExecutionId"].as_str().map(str::to_string))
            .collect()
    }

    /// One execution's summary, as `ListPipelineExecutions` reports it — which is where
    /// `sourceRevisions` lives (`PipelineExecution` itself carries `artifactRevisions`).
    fn execution_summary(&self, case: &str, pipeline: &str, id: &str) -> Value {
        let answer = self.cp_ok(
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        answer["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| summary["pipelineExecutionId"].as_str() == Some(id))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for the execution {id}"))
    }

    /// One execution as `GetPipelineExecution` reports it.
    fn execution(&self, case: &str, pipeline: &str, id: &str) -> Value {
        let answer = self.cp_ok(
            case,
            "GetPipelineExecution",
            &json!({ "pipelineName": pipeline, "pipelineExecutionId": id }),
        );
        answer["pipelineExecution"].clone()
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
                .filter(|summary| !TERMINAL.contains(&summary["status"].as_str().unwrap_or_default()))
                .collect();
            if pending.is_empty() {
                Ok(())
            } else {
                Err(format!("{pending:?}"))
            }
        });
    }

    /// Wait until an execution of `pipeline` appears outside `baseline`, and answer its id.
    fn started_execution(&self, case: &str, pipeline: &str, baseline: &BTreeSet<String>) -> String {
        settle(case, "a new pipeline execution", || {
            let ids = self.execution_ids(case, pipeline);
            match ids.difference(baseline).next() {
                Some(id) => Ok(id.clone()),
                None => Err(format!("only the baseline executions {ids:?}")),
            }
        })
    }

    /// Create a pipeline whose Source action sources `repository` at `tag` and whose Deploy
    /// action delivers the image detail it produced to `bucket`.
    fn pipeline(&self, case: &str, name: &str, role: &str, bucket: &str, repository: &str, tag: &str) {
        let declaration = json!({
            "name": name,
            "roleArn": role,
            "artifactStore": { "type": "S3", "location": bucket },
            "stages": [
                {
                    "name": "Source",
                    "actions": [{
                        "name": "Image",
                        "actionTypeId": {
                            "category": "Source",
                            "owner": "AWS",
                            "provider": "ECR",
                            "version": "1",
                        },
                        "configuration": { "RepositoryName": repository, "ImageTag": tag },
                        "outputArtifacts": [{ "name": "ImageArt" }],
                        "runOrder": 1,
                    }],
                },
                {
                    "name": "Deploy",
                    "actions": [{
                        "name": "Detail",
                        "actionTypeId": {
                            "category": "Deploy",
                            "owner": "AWS",
                            "provider": "S3",
                            "version": "1",
                        },
                        "configuration": {
                            "BucketName": bucket,
                            "ObjectKey": format!("delivered/{name}.zip"),
                            "Extract": "false",
                        },
                        "inputArtifacts": [{ "name": "ImageArt" }],
                        "runOrder": 1,
                    }],
                },
            ],
        });
        self.cp_ok(case, "CreatePipeline", &json!({ "pipeline": declaration }));
    }

    /// Put a rule matching a push of `tag` to `repository` on the default bus, with
    /// `pipeline_arn` as its target under `role_arn`.
    fn rule_for_push(
        &self,
        case: &str,
        rule: &str,
        repository: &str,
        tag: &str,
        pipeline_arn: &str,
        role_arn: &str,
    ) {
        let pattern = json!({
            "source": ["aws.ecr"],
            "detail-type": ["ECR Image Action"],
            "detail": {
                "action-type": ["PUSH"],
                "repository-name": [repository],
                "image-tag": [tag],
            },
        })
        .to_string();
        self.events_ok(
            case,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": pattern }),
        );
        let targeted = self.events_ok(
            case,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{ "Id": "to-pipeline", "Arn": pipeline_arn, "RoleArn": role_arn }],
            }),
        );
        assert_eq!(
            targeted["FailedEntryCount"].as_i64(),
            Some(0),
            "case {case:?}: the pipeline target must be accepted, got {targeted}"
        );
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

/// The ARN a pipeline of `name` carries, which is what a rule's target names it by.
fn pipeline_arn(name: &str) -> String {
    format!("arn:aws:codepipeline:{REGION}:{ACCOUNT}:{name}")
}

/// The digest a registry addresses a manifest by: `sha256:<hex>` over the manifest bytes,
/// computed here independently of the services under test.
fn manifest_digest(manifest: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(manifest.as_bytes())))
}

/// A manifest whose bytes — and so whose digest — are the row's own.
fn manifest_of(marker: &str) -> String {
    json!({"schemaVersion": 2, "marker": marker}).to_string()
}

/// The source revision the execution recorded for its `Image` action, as the execution's
/// SUMMARY reports it (`PipelineExecutionSummary$sourceRevisions` is "a list of the source
/// artifact revisions that initiated a pipeline execution").
fn source_revision(summary: &Value) -> Option<String> {
    summary["sourceRevisions"]
        .as_array()?
        .iter()
        .find(|revision| revision["actionName"].as_str() == Some("Image"))?["revisionId"]
        .as_str()
        .map(str::to_string)
}

/// The revision the execution reports for the artifact its source action produced
/// (`PipelineExecution$artifactRevisions`).
fn artifact_revision(execution: &Value) -> Option<String> {
    execution["artifactRevisions"]
        .as_array()?
        .iter()
        .find(|revision| revision["name"].as_str() == Some("ImageArt"))?["revisionId"]
        .as_str()
        .map(str::to_string)
}

/// A push whose tag the rule's `image-tag` filter names starts ONE execution of the
/// targeted pipeline, and that execution sources the pushed image: its Source action's
/// source revision is the digest the push stored, and the detail it produced is delivered
/// by the stage after it. A push whose tag the filter does not name starts nothing.
#[test]
fn a_matched_push_starts_the_pipeline_and_records_the_pushed_image_digest() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let case = "a matched push";
        let repository = "orders";
        let tag = "release";
        let bucket = "ecr-trigger-artifacts";
        let pipeline = "ecr-trigger";

        emulator.bucket(bucket);
        emulator.ecr_ok(
            case,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        );
        let pipeline_role = emulator.role("ecr-pipeline-role", PIPELINE_TRUST);
        emulator.pipeline(case, pipeline, &pipeline_role, bucket, repository, tag);
        // `CreatePipeline` starts an execution of its own, which runs before any image is
        // pushed: it is part of the baseline, whatever it ended as.
        emulator.drained(case, pipeline);

        let target_role = emulator.role("ecr-events-role", EVENTS_TRUST);
        emulator.allow(
            "ecr-events-role",
            "codepipeline:StartPipelineExecution",
            &pipeline_arn(pipeline),
        );
        emulator.rule_for_push(
            case,
            "ecr-push-rule",
            repository,
            tag,
            &pipeline_arn(pipeline),
            &target_role,
        );

        // (case, the tag the push carries, whether the rule's filter matches it)
        let rows: [(&str, &str, bool); 4] = [
            ("the first push of the matched tag", tag, true),
            ("a tag the rule does not name", "canary", false),
            ("a second push of the matched tag, with other bytes", tag, true),
            ("a tag that merely starts like the matched one", "release-2", false),
        ];

        let mut baseline = emulator.execution_ids(case, pipeline);
        for (index, (row, pushed_tag, matched)) in rows.into_iter().enumerate() {
            let digest = emulator.push(
                row,
                repository,
                &manifest_of(&format!("{row}-{index}")),
                pushed_tag,
            );
            assert_eq!(
                digest,
                manifest_digest(&manifest_of(&format!("{row}-{index}"))),
                "case {row:?}: the registry must store the manifest under the digest of its \
                 own bytes, got {digest}"
            );

            if !matched {
                // Nothing the rule does not match starts an execution: the set of
                // executions is unchanged after a window in which one would have appeared.
                std::thread::sleep(NOTHING_STARTED_WINDOW);
                let ids = emulator.execution_ids(row, pipeline);
                assert_eq!(
                    ids, baseline,
                    "case {row:?}: a push the rule's filter does not match must start no \
                     execution, got {ids:?} against the baseline {baseline:?}"
                );
                continue;
            }

            let started = emulator.started_execution(row, pipeline, &baseline);
            // The revision is the execution's from the START: an execution is started FOR
            // a revision of its source, so it reports that revision as soon as it exists —
            // before the stage that produces the artifact from it has run.
            let at_start = emulator.execution(row, pipeline, &started);
            assert_eq!(
                artifact_revision(&at_start).as_deref(),
                Some(digest.as_str()),
                "case {row:?}: the execution must report the revision it was started for \
                 ({digest}) as soon as it exists, got {at_start}"
            );
            emulator.drained(row, pipeline);
            let execution = emulator.execution(row, pipeline, &started);
            assert_eq!(
                execution["status"].as_str(),
                Some("Succeeded"),
                "case {row:?}: the execution the push started must run every stage, got \
                 {execution}"
            );
            assert_eq!(
                artifact_revision(&execution).as_deref(),
                Some(digest.as_str()),
                "case {row:?}: the artifact the source action produced must carry the digest \
                 the push stored ({digest}), got {execution}"
            );
            let summary = emulator.execution_summary(row, pipeline, &started);
            assert_eq!(
                source_revision(&summary).as_deref(),
                Some(digest.as_str()),
                "case {row:?}: the Source action's source revision must be the digest the \
                 push stored ({digest}), got {summary}"
            );
            assert!(
                emulator
                    .object(bucket, &format!("delivered/{pipeline}.zip"))
                    .is_some_and(|object| object.starts_with(b"PK")),
                "case {row:?}: the stage after the source must have delivered the archive \
                 the source action produced"
            );
            baseline = emulator.execution_ids(row, pipeline);
        }
    });
}

/// A target role that does NOT allow `codepipeline:StartPipelineExecution` starts no
/// execution, while a correctly permissioned sibling rule matching the same push does —
/// which is what makes the two configurations distinguishable.
#[test]
fn a_target_role_without_the_start_permission_starts_nothing() {
    on_a_sized_thread(|| {
        let emulator = Emulator::new();
        let case = "a target role without the grant";
        let repository = "payments";
        let tag = "live";
        let bucket = "ecr-permission-artifacts";

        emulator.bucket(bucket);
        emulator.ecr_ok(
            case,
            "CreateRepository",
            &json!({ "repositoryName": repository }),
        );
        let pipeline_role = emulator.role("ecr-permission-pipeline-role", PIPELINE_TRUST);

        // Two pipelines, so each rule's delivery is observable on its own: the one whose
        // target role carries the grant, and the one whose role does not.
        let permissioned = "ecr-permissioned";
        let withheld = "ecr-withheld";
        for pipeline in [permissioned, withheld] {
            emulator.pipeline(case, pipeline, &pipeline_role, bucket, repository, tag);
            emulator.drained(case, pipeline);
        }

        let granting = emulator.role("ecr-events-granting", EVENTS_TRUST);
        emulator.allow(
            "ecr-events-granting",
            "codepipeline:StartPipelineExecution",
            &pipeline_arn(permissioned),
        );
        // The same trust, so EventBridge may assume it — and no policy allowing the call
        // the invocation makes.
        let withholding = emulator.role("ecr-events-withholding", EVENTS_TRUST);

        emulator.rule_for_push(
            case,
            "ecr-permissioned-rule",
            repository,
            tag,
            &pipeline_arn(permissioned),
            &granting,
        );
        emulator.rule_for_push(
            case,
            "ecr-withheld-rule",
            repository,
            tag,
            &pipeline_arn(withheld),
            &withholding,
        );

        let permissioned_baseline = emulator.execution_ids(case, permissioned);
        let withheld_baseline = emulator.execution_ids(case, withheld);
        emulator.push(case, repository, &manifest_of("permission-control"), tag);

        // The positive control: the correctly permissioned rule started an execution.
        let started = emulator.started_execution(case, permissioned, &permissioned_baseline);
        assert!(
            !started.is_empty(),
            "case {case:?}: the correctly permissioned rule must start an execution"
        );
        // And the rule whose role withholds the call started nothing, after a window in
        // which it would have.
        std::thread::sleep(NOTHING_STARTED_WINDOW);
        let ids = emulator.execution_ids(case, withheld);
        assert_eq!(
            ids, withheld_baseline,
            "case {case:?}: a target role that allows no codepipeline:StartPipelineExecution \
             must start no execution, got {ids:?} against the baseline {withheld_baseline:?}"
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

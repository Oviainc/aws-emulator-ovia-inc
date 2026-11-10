// SPDX-License-Identifier: CC0-1.0

//! A CodePipeline execution's state changes reach the SQS queue an EventBridge rule
//! targets, scoped to the pipeline the rule's pattern names.
//!
//! The RULE under test: **while an execution runs, every state a pipeline execution, a
//! stage of it or an action of that stage MOVES INTO is published once onto the account's
//! default EventBridge bus under `source` `aws.codepipeline` with the `detail-type` of the
//! thing that moved — `CodePipeline Pipeline Execution State Change`, `CodePipeline Stage
//! Execution State Change` or `CodePipeline Action Execution State Change` — carrying the
//! pipeline's ARN in `resources`, the caller's account and Region on the envelope, and the
//! moved resource's identity (`pipeline`, `execution-id`, `stage`, `action`) with its new
//! `state` in `detail`; EventBridge routes each by its CONTENT, so a rule whose
//! `EventPattern` names those detail types AND one `detail.pipeline` delivers one message
//! per matched event to its SQS queue target and delivers nothing for any other
//! pipeline.** The states are the modelled statuses under the event schema's names
//! (`smithy_query(query="shape", shape="com.amazonaws.codepipeline#PipelineExecutionStatus")`
//! and its stage and action siblings): a resource that begins reports `STARTED`, one that
//! finishes reports `SUCCEEDED` or `FAILED`.
//!
//! This runs the path a client runs it over: four services (CodePipeline publishes while it
//! drives the run, Amazon S3 holds the artifacts, EventBridge routes, Amazon SQS holds what
//! arrived) reached through the WHOLE emulator — the same `build_router` service set the
//! binary serves — so the in-process publication is claimed and routed by whichever mounted
//! model a client's own `events`-scoped call would reach, not by a hand-picked pair of
//! services. Nothing calls CodePipeline between the create and the arrival of the last
//! message, so an event that arrives was published by the service driving the run on its
//! own rather than as a side effect of being polled.
//!
//! The rows differ along what the rule generalises over: HOW MANY stages the run passes
//! through, WHETHER it succeeds or fails, and WHICH of the three detail types the rule's
//! pattern names. Every row also creates a TWIN pipeline of the same shape, which runs at
//! the same time and whose own transitions must reach the queue NOT AT ALL — the negative
//! half of the same rule, and the half only a `detail.pipeline` pattern can state.
//!
//! What is NOT asserted here, because it is asserted elsewhere:
//! `services/service-com-amazonaws-codepipeline-codepipeline-20150709/tests/all/state_change_events.rs`
//! pins the same publication against EventBridge's own routing log over a table of pipeline
//! shapes, and
//! `services/service-com-amazonaws-eventbridge-awsevents/tests/all/queue_target_policy_authorization.rs`
//! pins what a queue WITHHOLDING its resource policy does. What only this suite reaches is
//! the pair those leave out: routing a CodePipeline event by the pipeline named inside its
//! `detail`, over the whole mounted service set.

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

/// The `source` and the three `detail-type` values CodePipeline publishes its execution
/// events under, spelled out here rather than imported from the service crate, so a wrong
/// constant in the implementation cannot make this suite pass.
const EVENT_SOURCE: &str = "aws.codepipeline";
const PIPELINE_STATE_CHANGE: &str = "CodePipeline Pipeline Execution State Change";
const STAGE_STATE_CHANGE: &str = "CodePipeline Stage Execution State Change";
const ACTION_STATE_CHANGE: &str = "CodePipeline Action Execution State Change";

/// The bus an Amazon Web Services service's own events go to: the events a service
/// generates go to the account's default event bus, which is the bus a rule carrying no
/// `EventBusName` is created on.
const DEFAULT_BUS: &str = "default";

/// The service principal EventBridge delivers to an SQS target as, which is what the
/// queue's resource policy admits.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The trust policy the pipeline role carries.
const PIPELINE_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The bucket the source object lives in, and its key.
const SOURCE_BUCKET: &str = "cp-events-source";
const SOURCE_KEY: &str = "source.zip";

/// The bucket every deploy action of this suite delivers to.
const DEPLOY_BUCKET: &str = "cp-events-deploys";

/// The artifact the source action produces and the deploy actions consume.
const ARTIFACT: &str = "SourceArtifact";

/// The `PipelineExecutionStatus` values an execution never leaves.
const TERMINAL: [&str; 5] = [
    "Succeeded",
    "Failed",
    "Stopped",
    "Superseded",
    "Cancelled",
];

/// How long a row waits for the transitions the run owes to arrive on the queue, or for an
/// execution to settle, before it calls the path stalled: far above the span one transition
/// takes, and driven off what has ARRIVED rather than off a fixed sleep, so a slow machine
/// delays a row instead of breaking it.
const DEADLINE: Duration = Duration::from_secs(40);

/// One poll inside [`DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a row leaves the queue alone before it reads it a last time: longer than the
/// span any transition above took to arrive, so an empty read is evidence the twin's
/// transitions were not delivered rather than evidence of having looked too early.
const QUIET_SPAN: Duration = Duration::from_millis(750);

/// One delivered event, reduced to WHICH resource moved into WHICH state: the detail type,
/// the stage and action named in the `detail` (empty for a level that names neither) and the
/// state. This is the shape the rule is about, so it is what the expected sequence states.
type Moved = (String, String, String, String);

/// The `Moved` of one delivered envelope.
fn moved(envelope: &Value) -> Moved {
    let member = |name: &str| {
        envelope["detail"][name]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    (
        envelope["detail-type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        member("stage"),
        member("action"),
        member("state"),
    )
}

/// The `Moved` the expected sequence states.
fn expect(detail_type: &str, stage: &str, action: &str, state: &str) -> Moved {
    (
        detail_type.to_string(),
        stage.to_string(),
        action.to_string(),
        state.to_string(),
    )
}

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

    /// One request against the service whose signing name is `signing_name`, which is what
    /// routes it. The signature is never checked: the router above runs with request
    /// authentication off.
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

    /// One `X-Amz-Target` RPC operation of `signing_name`, which must answer 200.
    fn rpc_ok(
        &self,
        case: &str,
        signing_name: &str,
        prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let (status, payload) = self.send(
            signing_name,
            "POST",
            "/",
            &[
                (
                    "content-type",
                    &format!("application/x-amz-json-{json_version}"),
                ),
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
        assert_eq!(
            status, 200,
            "case {case:?}: {signing_name} {operation} must answer 200, got {status} with \
             {parsed}"
        );
        parsed
    }

    /// One CodePipeline operation that has to succeed.
    fn cp(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc_ok(
            case,
            "codepipeline",
            CODEPIPELINE_TARGET_PREFIX,
            "1.1",
            operation,
            body,
        )
    }

    /// One EventBridge operation that has to succeed.
    fn events(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc_ok(case, "events", "AWSEvents", "1.1", operation, body)
    }

    /// One Amazon SQS operation that has to succeed.
    fn sqs(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc_ok(case, "sqs", "AmazonSQS", "1.0", operation, body)
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

    /// Create the pipeline role, and answer its ARN.
    fn role(&self, name: &str) -> String {
        let (status, body) = self.iam(
            "CreateRole",
            &[
                ("RoleName", name),
                ("AssumeRolePolicyDocument", PIPELINE_TRUST),
            ],
        );
        assert_eq!(
            status, 200,
            "CreateRole {name}: expected 200, got {status} with body {body}"
        );
        element(&body, "Arn")
    }

    /// Create a bucket.
    fn bucket(&self, case: &str, bucket: &str) {
        let (status, payload) = self.send("s3", "PUT", &format!("/{bucket}"), &[], Vec::new());
        assert_eq!(
            status,
            200,
            "case {case:?}: CreateBucket {bucket} must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    /// Put one object.
    fn put_object(&self, case: &str, bucket: &str, key: &str, body: &[u8]) {
        let (status, payload) =
            self.send("s3", "PUT", &format!("/{bucket}/{key}"), &[], body.to_vec());
        assert_eq!(
            status,
            200,
            "case {case:?}: PutObject {bucket}/{key} must answer 200, got {status} with {:?}",
            String::from_utf8_lossy(&payload)
        );
    }

    /// A queue called `name`, created through SQS itself and admitting EventBridge to send
    /// to it on behalf of `rule_arn`; answers the url `ReceiveMessage` takes and the ARN a
    /// rule's target names it by.
    fn queue(&self, case: &str, name: &str, rule_arn: &str) -> (String, String) {
        let created = self.sqs(case, "CreateQueue", &json!({ "QueueName": name }));
        let url = string(case, &created, "/QueueUrl");
        let attributes = self.sqs(
            case,
            "GetQueueAttributes",
            &json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
        );
        let queue_arn = string(case, &attributes, "/Attributes/QueueArn");
        // The grant a rule's queue target needs: EventBridge makes that target's
        // `sqs:SendMessage` as its own service principal and relies on a resource-based
        // policy for a queue, so a queue that has granted it nothing receives nothing.
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowEventBridge",
                "Effect": "Allow",
                "Principal": { "Service": EVENTS_PRINCIPAL },
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
            }],
        })
        .to_string();
        self.sqs(
            case,
            "SetQueueAttributes",
            &json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
        );
        (url, queue_arn)
    }

    /// Every message now waiting on the queue at `url`, as the parsed envelopes their
    /// bodies carry.
    fn drain(&self, case: &str, url: &str) -> Vec<Value> {
        let received = self.sqs(
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        );
        received["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| {
                serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                    .unwrap_or(Value::Null)
            })
            .collect()
    }

    /// Collect envelopes off the queue at `url` until `wanted` of them have arrived or
    /// [`DEADLINE`] passes; answers everything that did arrive, in the order it arrived.
    fn collect(&self, case: &str, url: &str, wanted: usize) -> Vec<Value> {
        let deadline = Instant::now() + DEADLINE;
        let mut arrived: Vec<Value> = Vec::new();
        while arrived.len() < wanted && Instant::now() < deadline {
            arrived.extend(self.drain(case, url));
            if arrived.len() < wanted {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
        arrived
    }

    /// The executions of `pipeline`, as `ListPipelineExecutions` summarises them.
    fn executions_of(&self, case: &str, pipeline: &str) -> Vec<Value> {
        let listed = self.cp(
            case,
            "ListPipelineExecutions",
            &json!({ "pipelineName": pipeline }),
        );
        listed["pipelineExecutionSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Wait until `pipeline` holds exactly one execution and it has reached a terminal
    /// status; answers that execution's id and status.
    fn settled_run(&self, case: &str, pipeline: &str) -> (String, String) {
        let deadline = Instant::now() + DEADLINE;
        let mut last = Vec::new();
        while Instant::now() < deadline {
            last = self.executions_of(case, pipeline);
            if let [summary] = last.as_slice() {
                let status = summary["status"].as_str().unwrap_or_default();
                if TERMINAL.contains(&status) {
                    return (
                        summary["pipelineExecutionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        status.to_string(),
                    );
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        panic!(
            "case {case:?}: {pipeline} never came to rest on exactly one settled execution; \
             last saw {last:?}"
        );
    }

    /// The ARN `GetPipeline` reports for `pipeline`.
    fn pipeline_arn(&self, case: &str, pipeline: &str) -> String {
        let answer = self.cp(case, "GetPipeline", &json!({ "name": pipeline }));
        string(case, &answer, "/metadata/pipelineArn")
    }
}

/// A string member of a response, or a panic naming the case and what came back.
fn string(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// A pipeline reading one S3 object and delivering the artifact it produces to one S3 key
/// per deploy stage.
fn declaration(name: &str, role_arn: &str, artifacts: &str, deploys: &[&str]) -> Value {
    let mut stages = vec![json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": {
                "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
            },
            "configuration": {
                "S3Bucket": SOURCE_BUCKET,
                "S3ObjectKey": SOURCE_KEY,
                "PollForSourceChanges": "false",
            },
            "outputArtifacts": [{ "name": ARTIFACT }],
            "runOrder": 1,
        }],
    })];
    for stage in deploys {
        stages.push(json!({
            "name": stage,
            "actions": [{
                "name": "Dep",
                "actionTypeId": {
                    "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1",
                },
                "configuration": {
                    "BucketName": DEPLOY_BUCKET,
                    "ObjectKey": format!("{name}/{stage}.zip"),
                    "Extract": "false",
                },
                "inputArtifacts": [{ "name": ARTIFACT }],
                "runOrder": 1,
            }],
        }));
    }
    json!({
        "pipeline": {
            "name": name,
            "roleArn": role_arn,
            "artifactStore": { "type": "S3", "location": artifacts },
            "stages": stages,
        },
    })
}

/// What one row runs, filters on, and must therefore see.
struct Row {
    case: &'static str,
    /// The deploy stages that follow the source stage.
    deploys: &'static [&'static str],
    /// Whether the source object the run reads is there.
    source_present: bool,
    /// Whether the rule's pattern also names the ACTION detail type.
    match_actions: bool,
}

impl Row {
    /// The detail types this row's rule matches.
    fn detail_types(&self) -> Vec<&'static str> {
        let mut types = vec![PIPELINE_STATE_CHANGE, STAGE_STATE_CHANGE];
        if self.match_actions {
            types.push(ACTION_STATE_CHANGE);
        }
        types
    }

    /// Every `(detail-type, stage, action, state)` this row's run owes the rule, in the
    /// order the transitions happen: the execution starts, then each stage it reaches
    /// starts (with its action, when the pattern names action events) and finishes, then the
    /// execution finishes.
    fn expected(&self) -> Vec<Moved> {
        let terminal = match self.source_present {
            true => "SUCCEEDED",
            false => "FAILED",
        };
        let mut expected = vec![expect(PIPELINE_STATE_CHANGE, "", "", "STARTED")];
        let mut stages: Vec<(&str, &str)> = vec![("Source", "Src")];
        // A run whose source action failed never reaches the stages beyond it, so they move
        // into nothing and owe no event.
        if self.source_present {
            stages.extend(self.deploys.iter().map(|stage| (*stage, "Dep")));
        }
        let last = stages.len().saturating_sub(1);
        for (index, (stage, action)) in stages.into_iter().enumerate() {
            let outcome = match (self.source_present, index == last) {
                (false, true) => "FAILED",
                _ => "SUCCEEDED",
            };
            expected.push(expect(STAGE_STATE_CHANGE, stage, "", "STARTED"));
            if self.match_actions {
                expected.push(expect(ACTION_STATE_CHANGE, stage, action, "STARTED"));
                expected.push(expect(ACTION_STATE_CHANGE, stage, action, outcome));
            }
            expected.push(expect(STAGE_STATE_CHANGE, stage, "", outcome));
        }
        expected.push(expect(PIPELINE_STATE_CHANGE, "", "", terminal));
        expected
    }
}

/// RULE: each state a pipeline execution, a stage of it or an action of that stage moves
/// into is published under `aws.codepipeline` with that level's detail type and reaches the
/// SQS target of every rule whose pattern matches it — and no transition of a pipeline the
/// pattern does not name reaches it at all.
#[test]
fn every_execution_and_stage_state_change_reaches_the_matching_rules_queue() {
    on_a_sized_thread(|| {
        let rows = [
            Row {
                case: "a two-stage run that succeeds, matched on the pipeline and stage \
                       detail types",
                deploys: &["Deploy"],
                source_present: true,
                match_actions: false,
            },
            Row {
                case: "a three-stage run that succeeds",
                deploys: &["DeployOne", "DeployTwo"],
                source_present: true,
                match_actions: false,
            },
            Row {
                case: "a run whose source object is missing, so it fails at its first stage",
                deploys: &["Deploy"],
                source_present: false,
                match_actions: false,
            },
            Row {
                case: "a rule whose pattern also names the action detail type",
                deploys: &["Deploy"],
                source_present: true,
                match_actions: true,
            },
        ];

        for (index, row) in rows.iter().enumerate() {
            let case = row.case;
            let emulator = Emulator::new();
            let graded = format!("cp-events-graded-{index}");
            let twin = format!("cp-events-twin-{index}");
            let rule = format!("cp-events-rule-{index}");

            // The rule, then the queue it targets: the queue's policy names the rule, so
            // the rule's ARN has to exist before the queue admits it.
            let put = emulator.events(
                case,
                "PutRule",
                &json!({
                    "Name": &rule,
                    "EventBusName": DEFAULT_BUS,
                    "State": "ENABLED",
                    "EventPattern": json!({
                        "source": [EVENT_SOURCE],
                        "detail-type": row.detail_types(),
                        "detail": { "pipeline": [&graded] },
                    }).to_string(),
                }),
            );
            let rule_arn = string(case, &put, "/RuleArn");
            let (queue_url, queue_arn) =
                emulator.queue(case, &format!("cp-events-queue-{index}"), &rule_arn);
            let targeted = emulator.events(
                case,
                "PutTargets",
                &json!({
                    "Rule": &rule,
                    "EventBusName": DEFAULT_BUS,
                    "Targets": [{ "Id": "queue", "Arn": &queue_arn }],
                }),
            );
            assert_eq!(
                targeted["FailedEntryCount"],
                json!(0),
                "case {case:?}: every target must be accepted, got {targeted}"
            );

            // The buckets the two runs read from and deliver to, and the source object —
            // written BEFORE either pipeline exists, so the run each creation starts sees
            // whatever this row says about it.
            emulator.bucket(case, SOURCE_BUCKET);
            emulator.bucket(case, DEPLOY_BUCKET);
            let graded_artifacts = format!("cp-events-graded-{index}-artifacts");
            let twin_artifacts = format!("cp-events-twin-{index}-artifacts");
            emulator.bucket(case, &graded_artifacts);
            emulator.bucket(case, &twin_artifacts);
            if row.source_present {
                emulator.put_object(case, SOURCE_BUCKET, SOURCE_KEY, b"the source bytes");
            }
            let role = emulator.role(&format!("cp-events-role-{index}"));

            // The twin runs first and alongside: its own transitions are published too, and
            // the rule's pattern is the only thing that keeps them off this queue.
            emulator.cp(
                case,
                "CreatePipeline",
                &declaration(&twin, &role, &twin_artifacts, row.deploys),
            );
            emulator.cp(
                case,
                "CreatePipeline",
                &declaration(&graded, &role, &graded_artifacts, row.deploys),
            );

            // From here until the queue holds the last of them, no CodePipeline operation is
            // called at all: what arrives was published by the service driving the run.
            let expected = row.expected();
            let delivered = emulator.collect(case, &queue_url, expected.len());
            let found: Vec<Moved> = delivered.iter().map(moved).collect();
            assert_eq!(
                found,
                expected,
                "case {case:?}: expected one delivered message per state change, in order, \
                 got {found:#?} from {delivered:#?}"
            );

            // Both runs have come to rest, so every transition either owed has happened.
            let (graded_execution, graded_status) = emulator.settled_run(case, &graded);
            let (twin_execution, twin_status) = emulator.settled_run(case, &twin);
            let graded_arn = emulator.pipeline_arn(case, &graded);
            let expected_status = match row.source_present {
                true => "Succeeded",
                false => "Failed",
            };
            for (pipeline, status) in [(&graded, &graded_status), (&twin, &twin_status)] {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: {pipeline} must reach {expected_status}, got {status}"
                );
            }

            // Each delivered message is the whole envelope AWS hands a target, and it names
            // the resource that moved: this pipeline, this execution, this account and
            // Region, and the pipeline's ARN as the resource the event is about.
            for envelope in &delivered {
                for (pointer, expected) in [
                    ("/source", json!(EVENT_SOURCE)),
                    ("/account", json!(engine::EMULATOR_ACCOUNT_ID)),
                    ("/region", json!(REGION)),
                    ("/resources", json!([&graded_arn])),
                    ("/detail/pipeline", json!(&graded)),
                    ("/detail/execution-id", json!(&graded_execution)),
                    ("/detail/version", json!(1)),
                ] {
                    let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
                    assert_eq!(
                        actual, expected,
                        "case {case:?}: expected {pointer} to be {expected}, got {actual} in \
                         {envelope}"
                    );
                }
                assert!(
                    envelope["id"].as_str().is_some_and(|id| !id.is_empty()),
                    "case {case:?}: expected the envelope to carry an event id, got {envelope}"
                );
                assert!(
                    envelope["time"].as_str().is_some_and(|at| at.ends_with('Z')),
                    "case {case:?}: expected the envelope to state when the resource moved, \
                     got {envelope}"
                );
                let body = envelope.to_string();
                assert!(
                    !body.contains(twin.as_str()) && !body.contains(twin_execution.as_str()),
                    "case {case:?}: no delivered envelope may name the twin pipeline, got \
                     {envelope}"
                );
            }

            // The negative half: the twin ran to the same terminal status, so it made the
            // same transitions — and a rule whose pattern names only the graded pipeline
            // received none of them, over a further span longer than any delivery above
            // took.
            std::thread::sleep(QUIET_SPAN);
            let extra = emulator.drain(case, &queue_url);
            let extra_shapes: Vec<Moved> = extra.iter().map(moved).collect();
            assert!(
                extra.is_empty(),
                "case {case:?}: the queue must hold nothing beyond this pipeline's own \
                 transitions, got {extra_shapes:?}"
            );
        }
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

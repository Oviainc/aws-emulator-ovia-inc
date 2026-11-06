// SPDX-License-Identifier: CC0-1.0

//! CodePipeline publishes one EventBridge event per state change of an execution, of
//! each of its stages and of each of their actions.
//!
//! The RULE under test: while an execution runs, every state a pipeline execution, a
//! stage of it or an action of that stage MOVES INTO is published once onto the account's
//! default EventBridge bus as `source` `aws.codepipeline` with the `detail-type` of the
//! thing that moved — `CodePipeline Pipeline Execution State Change`, `CodePipeline Stage
//! Execution State Change` or `CodePipeline Action Execution State Change` — carrying the
//! pipeline's ARN in `resources` and that resource's identity and new state in the event
//! `detail`. EventBridge then routes it: a rule whose `EventPattern` matches delivers one
//! message per event to its SQS queue target, which a client reads with `ReceiveMessage`,
//! and a rule that does not match receives nothing.
//!
//! The states are the modelled statuses mapped onto the event schema's own names
//! (`smithy_query(query="shape", shape="PipelineExecutionStatus")` and its stage and
//! action siblings): an execution or stage that begins reports `STARTED`, one that
//! finishes reports `SUCCEEDED` or `FAILED`.
//!
//! The rule is asserted over a TABLE of pipelines that differ along what it generalises
//! over — HOW MANY stages the run passes through and WHETHER the run succeeds or fails —
//! because the rule is about every transition of every resource, not about one shape of
//! pipeline. Nothing in these tests calls CodePipeline after the start: the events are
//! read from the queue the rule targets, so an event that arrives was published by the
//! service driving the execution on its own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::execution_engine::{
    artifact_bucket, declaration, deploy_stage, source_stage, zip_of, WAIT,
};

/// The `source` and the three `detail-type` values CodePipeline publishes its execution
/// events under, spelled out here rather than imported from the implementation, so a
/// wrong constant in the service cannot make a test pass.
const EVENT_SOURCE: &str = "aws.codepipeline";
const PIPELINE_STATE_CHANGE: &str = "CodePipeline Pipeline Execution State Change";
const STAGE_STATE_CHANGE: &str = "CodePipeline Stage Execution State Change";
const ACTION_STATE_CHANGE: &str = "CodePipeline Action Execution State Change";

/// The rule that matches a CodePipeline event, and the rule that must not.
const MATCHING_RULE: &str = "codepipeline-state-changes";
const OTHER_RULE: &str = "unrelated-events";

/// The Region every request in this suite is scoped to.
const REGION: &str = "us-east-1";

/// The bucket the source object lives in, and its key.
const SOURCE_BUCKET: &str = "events-source";
const SOURCE_KEY: &str = "source.zip";

/// The bucket the deploy actions deliver to.
const DEPLOY_BUCKET: &str = "events-deploys";

/// The artifact the source action produces and the deploy actions consume.
const ARTIFACT: &str = "SourceArtifact";

/// How long one poll of the rule's queue waits before reading again.
const POLL: Duration = Duration::from_millis(25);

/// One emulator: CodePipeline, S3, EventBridge and SQS mounted on ONE shared engine, with
/// the router that dispatches to all four.
///
/// EventBridge and SQS are mounted BESIDE CodePipeline because the execution plane
/// publishes its state changes onto the default event bus through the in-process callback
/// and a rule on that bus delivers them to its SQS queue target: with neither mounted
/// there is nowhere to publish, so the path under test would not be exercised at all.
/// Each service claims only the requests scoped to its own signing name, so no
/// CodePipeline request below is routed any differently for their presence.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
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
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// The EventBridge store mounted beside CodePipeline: the buses, their rules and the
    /// routing log `PutEvents` appends what it routed to.
    fn event_bus_store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState,
    > {
        self.engine.state()
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", &host)
            .header("authorization", authorization)
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: the response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// Call one CodePipeline operation.
    async fn cp(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc(
                "codepipeline",
                "CodePipeline_20150709",
                "1.1",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Call one EventBridge operation.
    async fn events(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc("events", "AWSEvents", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Call one SQS operation.
    async fn sqs(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.rpc("sqs", "AmazonSQS", "1.0", operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One S3 request, path style, the way the emulated S3 service routes it.
    async fn s3(&self, method: &str, uri: &str, headers: &[(&str, &str)], body: &[u8]) -> u16 {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a valid request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Create a versioned bucket — what a CodePipeline source bucket is.
    async fn bucket(&self, name: &str) {
        let status = self.s3("PUT", &format!("/{name}"), &[], b"").await;
        assert_eq!(status, 200, "the bucket {name:?} must be created, got {status}");
        let status = self
            .s3(
                "PUT",
                &format!("/{name}?versioning"),
                &[("content-type", "application/xml")],
                b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
            )
            .await;
        assert_eq!(
            status, 200,
            "versioning must be enabled on {name:?}, got {status}"
        );
    }

    /// Write an object.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) {
        let status = self.s3("PUT", &format!("/{bucket}/{key}"), &[], body).await;
        assert_eq!(
            status, 200,
            "writing s3://{bucket}/{key} must succeed, got {status}"
        );
    }
}

/// An SQS queue called `name`, carrying the grant a rule's target needs — EventBridge
/// makes the target's `sqs:SendMessage` as its own service principal and "relies on
/// resource-based policies" for a queue (`PutTargets`), so a queue that has granted it
/// nothing receives nothing. Answers its url and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator.sqs("CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = emulator
        .sqs(
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    emulator
        .sqs(
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
    (url, arn)
}

/// Install the two rules every row runs against: one whose pattern matches a
/// CodePipeline event, one whose pattern cannot. Each gets its own queue as its target,
/// so a delivery is attributable to the rule that matched; answers the two queue urls,
/// the matching one first.
async fn given_rules(emulator: &Emulator) -> (String, String) {
    let (matching_url, matching_arn) = given_queue(emulator, "codepipeline-events").await;
    let (other_url, other_arn) = given_queue(emulator, "unrelated-events").await;
    let rows = [
        (
            MATCHING_RULE,
            json!({ "source": [EVENT_SOURCE] }).to_string(),
            matching_arn,
        ),
        (
            OTHER_RULE,
            // Another service's events: nothing CodePipeline publishes can match it.
            json!({ "source": ["aws.codebuild"] }).to_string(),
            other_arn,
        ),
    ];
    for (rule, pattern, queue) in rows {
        emulator
            .events(
                "PutRule",
                &json!({ "Name": rule, "EventPattern": pattern, "State": "ENABLED" }),
            )
            .await;
        emulator
            .events(
                "PutTargets",
                &json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": queue }] }),
            )
            .await;
    }
    (matching_url, other_url)
}

/// One event, projected onto what this suite asserts.
#[derive(Debug, Clone)]
struct Published {
    source: String,
    detail_type: String,
    resources: Vec<String>,
    detail: Value,
    matched_rules: Vec<String>,
}

/// Every event EventBridge has routed for `execution`, oldest first, read from the
/// routing log `PutEvents` appends to.
fn routed_for(emulator: &Emulator, execution: &str) -> Vec<Published> {
    emulator
        .event_bus_store()
        .lock()
        .delivered
        .iter()
        .map(|event| Published {
            source: event.source.clone().unwrap_or_default(),
            detail_type: event.detail_type.clone().unwrap_or_default(),
            resources: event.resources.clone(),
            detail: event
                .detail
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or(Value::Null),
            matched_rules: event.matched_rules.clone(),
        })
        .filter(|event| event.detail["execution-id"] == json!(execution))
        .collect()
}

/// `(detail-type, stage, action, state)` for one event, as the table states them.
fn shape_of(detail_type: &str, detail: &Value) -> (String, String, String, String) {
    (
        detail_type.to_string(),
        detail["stage"].as_str().unwrap_or_default().to_string(),
        detail["action"].as_str().unwrap_or_default().to_string(),
        detail["state"].as_str().unwrap_or_default().to_string(),
    )
}

/// Read every message waiting on the queue at `url`, as parsed JSON bodies.
async fn receive(emulator: &Emulator, url: &str) -> Vec<Value> {
    let answer = emulator
        .sqs(
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
    answer["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let raw = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(raw).unwrap_or(Value::Null)
        })
        .collect()
}

/// Poll the rule's queue — and NOTHING else — until the terminal pipeline-level event of
/// `execution` has been delivered, answering every message the queue held for it.
///
/// No CodePipeline operation is called while this waits, so every message it collects was
/// published by the service driving the execution on its own.
async fn delivered_until_terminal(
    emulator: &Emulator,
    url: &str,
    execution: &str,
    terminal: &str,
    case: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + WAIT;
    let mut collected: Vec<Value> = Vec::new();
    while Instant::now() < deadline {
        collected.extend(receive(emulator, url).await);
        let reached = collected.iter().any(|message| {
            message["detail"]["execution-id"] == json!(execution)
                && message["detail-type"] == json!(PIPELINE_STATE_CHANGE)
                && message["detail"]["state"] == json!(terminal)
        });
        if reached {
            return collected
                .into_iter()
                .filter(|message| message["detail"]["execution-id"] == json!(execution))
                .collect();
        }
        tokio::time::sleep(POLL).await;
    }
    panic!(
        "case {case:?}: the rule's queue never received the {terminal} pipeline event of \
         {execution}; it holds {collected:#?}"
    );
}

/// The ARN `GetPipeline` reports for `pipeline`.
async fn pipeline_arn(emulator: &Emulator, pipeline: &str) -> String {
    let answer = emulator.cp("GetPipeline", &json!({ "name": pipeline })).await;
    answer["metadata"]["pipelineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetPipeline must report the ARN of {pipeline}: {answer}"))
        .to_string()
}

/// One row of the table: a pipeline shape, whether its source object exists, and the
/// state every resource of the run therefore moves into.
struct Row {
    /// What this row is about, named in every assertion.
    case: &'static str,
    /// The stages that follow the source stage, as `(stage name, delivered key, whether
    /// the deploy extracts the artifact)`.
    deploys: Vec<(&'static str, &'static str, bool)>,
    /// Whether the source object the run reads is there.
    source_present: bool,
}

impl Row {
    /// The expected `(detail-type, stage, action, state)` sequence of this run, in the
    /// order the transitions happen: the execution starts, then each stage it reaches
    /// starts with its action and finishes with it, then the execution finishes.
    fn expected(&self) -> Vec<(String, String, String, String)> {
        let terminal = match self.source_present {
            true => "SUCCEEDED",
            false => "FAILED",
        };
        let mut expected = vec![shape_of(
            PIPELINE_STATE_CHANGE,
            &json!({ "state": "STARTED" }),
        )];
        let mut stages: Vec<(&str, &str)> = vec![("Source", "Src")];
        // A run whose source action failed never reaches the stages beyond it, so they
        // move into nothing and owe no event.
        if self.source_present {
            stages.extend(self.deploys.iter().map(|(stage, _, _)| (*stage, "Dep")));
        }
        let last = stages.len().saturating_sub(1);
        for (index, (stage, action)) in stages.into_iter().enumerate() {
            let outcome = match (self.source_present, index == last) {
                (false, true) => "FAILED",
                _ => "SUCCEEDED",
            };
            for (detail_type, action_name, state) in [
                (STAGE_STATE_CHANGE, "", "STARTED"),
                (ACTION_STATE_CHANGE, action, "STARTED"),
                (ACTION_STATE_CHANGE, action, outcome),
                (STAGE_STATE_CHANGE, "", outcome),
            ] {
                expected.push(shape_of(
                    detail_type,
                    &json!({ "stage": stage, "action": action_name, "state": state }),
                ));
            }
        }
        expected.push(shape_of(
            PIPELINE_STATE_CHANGE,
            &json!({ "state": terminal }),
        ));
        expected
    }
}

#[tokio::test]
async fn every_execution_stage_and_action_state_change_reaches_the_rules_queue() {
    let rows = [
        Row {
            case: "a two-stage pipeline whose deploy extracts the artifact",
            deploys: vec![("Deploy", "extracted", true)],
            source_present: true,
        },
        Row {
            case: "a two-stage pipeline that succeeds",
            deploys: vec![("Deploy", "two/one.zip", false)],
            source_present: true,
        },
        Row {
            case: "a three-stage pipeline that succeeds",
            deploys: vec![
                ("DeployOne", "three/one.zip", false),
                ("DeployTwo", "three/two.zip", false),
            ],
            source_present: true,
        },
        Row {
            case: "a two-stage pipeline whose source object is missing",
            deploys: vec![("Deploy", "missing/one.zip", false)],
            source_present: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let (queue, other_queue) = given_rules(&emulator).await;
        let pipeline = format!("events-pipe-{index}");
        emulator.bucket(&artifact_bucket(&pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator.bucket(DEPLOY_BUCKET).await;

        let mut stages = vec![source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT)];
        for (stage, key, extract) in &row.deploys {
            stages.push(deploy_stage(stage, ARTIFACT, DEPLOY_BUCKET, key, *extract));
        }
        emulator
            .cp(
                "CreatePipeline",
                &declaration(&pipeline, json!(stages), None),
            )
            .await;
        let arn = pipeline_arn(&emulator, &pipeline).await;

        // The source object appears only now, so the run the creation started has
        // already failed at its source action: the events asserted below are filtered to
        // the execution started next.
        if row.source_present {
            let archive = zip_of(&[("payload.txt", b"the deployed bytes".as_slice())]);
            emulator
                .put_object(SOURCE_BUCKET, SOURCE_KEY, &archive)
                .await;
        }

        let started = emulator
            .cp("StartPipelineExecution", &json!({ "name": &pipeline }))
            .await;
        let execution = started["pipelineExecutionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no pipelineExecutionId in {started}"))
            .to_string();

        let expected = row.expected();
        let terminal = expected
            .last()
            .map(|(_, _, _, state)| state.clone())
            .unwrap_or_default();
        // From here until the queue holds the terminal event, no CodePipeline operation
        // is called at all.
        let delivered =
            delivered_until_terminal(&emulator, &queue, &execution, &terminal, case).await;

        let delivered_shapes: Vec<(String, String, String, String)> = delivered
            .iter()
            .map(|message| {
                shape_of(
                    message["detail-type"].as_str().unwrap_or_default(),
                    &message["detail"],
                )
            })
            .collect();
        assert_eq!(
            delivered_shapes, expected,
            "case {case:?}: expected one delivered message per state change, in order, \
             got {delivered_shapes:#?}"
        );
        for message in &delivered {
            assert_eq!(
                message["source"],
                json!(EVENT_SOURCE),
                "case {case:?}: every delivered envelope must carry the CodePipeline \
                 source, got {message}"
            );
            assert_eq!(
                message["detail"]["pipeline"],
                json!(&pipeline),
                "case {case:?}: every event must name its pipeline, got {message}"
            );
            assert_eq!(
                message["resources"],
                json!([&arn]),
                "case {case:?}: every event must carry the pipeline's ARN in resources, \
                 got {message}"
            );
        }

        // The same events as EventBridge routed them: matched by the rule whose pattern
        // names this source, and by no other rule.
        let routed = routed_for(&emulator, &execution);
        let routed_shapes: Vec<(String, String, String, String)> = routed
            .iter()
            .map(|event| shape_of(&event.detail_type, &event.detail))
            .collect();
        assert_eq!(
            routed_shapes, expected,
            "case {case:?}: expected one routed event per state change, in order, got \
             {routed_shapes:#?}"
        );
        for event in &routed {
            assert_eq!(
                event.source, EVENT_SOURCE,
                "case {case:?}: expected source {EVENT_SOURCE}, got {:?}",
                event.source
            );
            assert_eq!(
                event.resources,
                vec![arn.clone()],
                "case {case:?}: expected the pipeline's ARN in resources, got {:?}",
                event.resources
            );
            assert_eq!(
                event.matched_rules,
                vec![MATCHING_RULE.to_string()],
                "case {case:?}: expected only {MATCHING_RULE} to match, got {:?}",
                event.matched_rules
            );
        }

        let unrelated = receive(&emulator, &other_queue).await;
        assert!(
            unrelated.is_empty(),
            "case {case:?}: the rule whose pattern names another service's source must \
             receive nothing, got {unrelated:#?}"
        );
    }
}

/// A state change is published ONCE: reading the execution again publishes no second
/// event for a status that has not moved since.
///
/// This is the half of the rule a diff could get wrong in the other direction — every
/// reporting call re-reads the same statuses — so it is asserted over several reads of a
/// finished execution.
#[tokio::test]
async fn a_state_change_is_published_once_however_often_the_execution_is_read() {
    let emulator = Emulator::new();
    let (queue, _) = given_rules(&emulator).await;
    let pipeline = "events-once";
    emulator.bucket(&artifact_bucket(pipeline)).await;
    emulator.bucket(SOURCE_BUCKET).await;
    emulator.bucket(DEPLOY_BUCKET).await;
    emulator
        .put_object(
            SOURCE_BUCKET,
            SOURCE_KEY,
            &zip_of(&[("payload.txt", b"once".as_slice())]),
        )
        .await;
    emulator
        .cp(
            "CreatePipeline",
            &declaration(
                pipeline,
                json!([
                    source_stage(SOURCE_BUCKET, SOURCE_KEY, ARTIFACT),
                    deploy_stage("Deploy", ARTIFACT, DEPLOY_BUCKET, "once/one.zip", false),
                ]),
                None,
            ),
        )
        .await;
    let started = emulator
        .cp("StartPipelineExecution", &json!({ "name": pipeline }))
        .await;
    let execution = started["pipelineExecutionId"]
        .as_str()
        .expect("the start must report an execution id")
        .to_string();

    let delivered = delivered_until_terminal(
        &emulator,
        &queue,
        &execution,
        "SUCCEEDED",
        "published once",
    )
    .await;
    let settled = routed_for(&emulator, &execution).len();
    assert_eq!(
        settled,
        delivered.len(),
        "case \"published once\": every routed event must have been delivered, got \
         {settled} routed and {} delivered",
        delivered.len()
    );

    // Three reads of the finished execution, each of which re-reads every status.
    for read in 0..3 {
        emulator
            .cp(
                "GetPipelineExecution",
                &json!({ "pipelineName": pipeline, "pipelineExecutionId": &execution }),
            )
            .await;
        emulator.cp("GetPipelineState", &json!({ "name": pipeline })).await;
        let now = routed_for(&emulator, &execution).len();
        assert_eq!(
            now, settled,
            "case \"published once\": read {read} of a finished execution must publish \
             nothing further, got {now} events where {settled} were published"
        );
    }
}

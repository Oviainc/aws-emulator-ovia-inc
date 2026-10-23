// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AWS Batch job lifecycle, over the wire.
//!
//! Every case drives real HTTP (restJson1) requests through the router the emulator
//! mounts, so what is asserted is the response a client gets — not an internal call.
//!
//! The tests assert RULES, each over a set of inputs:
//!
//! * anything the model documents as "name or ARN" resolves both ways, and a job
//!   definition also by `<name>:<revision>`;
//! * a create persists what was submitted and a `Describe*` reads it back unchanged;
//!   re-creating a name fails rather than overwriting;
//! * `RegisterJobDefinition`'s revision is a per-name counter and earlier revisions stay
//!   describable;
//! * `containerOverrides` are merged into the definition's `containerProperties` the way
//!   AWS documents (replace scalars, merge `environment`/`resourceRequirements` by
//!   name/type);
//! * a job's status only ever moves FORWARD through the modelled `JobStatus` order, with
//!   time and never because somebody read it, and a terminal status is final;
//! * `ListJobs` honours the documented filters and the documented default (`RUNNING`
//!   only when no status is given);
//! * paging with `maxResults`/`nextToken` sees every item exactly once.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::{state, EmulatedService};

/// A SigV4 credential scope naming `batch` in `us-east-1`: what the router routes on,
/// and the region every ARN minted for these requests must carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/us-east-1/batch/aws4_request, SignedHeaders=host, Signature=0000";

/// The region of the credential scope above.
pub(crate) const REGION: &str = "us-east-1";

/// The `JobStatus` values, in the order AWS Batch moves a job through them. Source: the
/// model's `com.amazonaws.batch#JobStatus` enum
/// (`smithy_query(query="shape", shape="JobStatus")`), which lists them in exactly this
/// order: SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED — the last
/// two being the terminal pair, so both get the same (final) rank here.
const JOB_STATUS_ORDER: &[(&str, u8)] = &[
    ("SUBMITTED", 0),
    ("PENDING", 1),
    ("RUNNABLE", 2),
    ("STARTING", 3),
    ("RUNNING", 4),
    ("SUCCEEDED", 5),
    ("FAILED", 5),
];

/// The rank of a status in [`JOB_STATUS_ORDER`].
fn status_rank(status: &str) -> u8 {
    JOB_STATUS_ORDER
        .iter()
        .find(|(name, _)| *name == status)
        .map(|(_, rank)| *rank)
        .unwrap_or_else(|| panic!("status {status:?} is not a modelled JobStatus"))
}

/// One emulator: the batch service mounted on one shared engine, reached the way a
/// client reaches it.
///
/// EventBridge and SQS are mounted BESIDE Batch on that same engine, because Batch
/// publishes a `Batch Job State Change` event onto the account's default event bus on
/// every job transition (`state::publish_job_state_changes`), reaching EventBridge
/// through the in-process callback, and a rule on that bus delivers the event to its SQS
/// queue targets: with neither mounted there is nowhere to publish, so the publication
/// path would not be exercised at all. Each claims only requests scoped to its own
/// signing name (`events`, `sqs`), so no Batch request below is routed any differently
/// for their presence.
pub(crate) struct Batch {
    router: smithy_server_runtime::MultiServiceRouter,
    /// The SAME engine the mounted service holds, so a test can read or seed the store
    /// the handlers read and write.
    engine: Arc<engine::InMemoryEngine>,
}

impl Batch {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = smithy_server_runtime::MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// This emulator's persisted store, reached exactly as a handler reaches it
    /// (`state::service_state`) — for a test that must seed a record whose creating
    /// operation is still a scaffold.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<state::ComAmazonawsBatchAwsbatchv20160810State> {
        state::service_state(&*self.engine).expect("the batch store is reachable")
    }

    /// The EventBridge store mounted beside Batch: the buses, their rules, and the
    /// routing log `PutEvents` appends what it routed to.
    pub(crate) fn event_bus_store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState,
    > {
        self.engine.state()
    }

    /// Call one EventBridge operation the way a client does: `awsJson1_1`, `POST /`
    /// with `X-Amz-Target: AWSEvents.<Operation>` and an `events`-scoped credential.
    pub(crate) async fn events(&self, operation: &str, body: Value) -> (u16, Value) {
        self.rpc("events", "AWSEvents", "1.1", operation, body).await
    }

    /// Call one SQS operation the way a client does: `awsJson1_0`, `POST /` with
    /// `X-Amz-Target: AmazonSQS.<Operation>` and an `sqs`-scoped credential.
    pub(crate) async fn sqs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.rpc("sqs", "AmazonSQS", "1.0", operation, body).await
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let credential = format!(
            "AKIDTEST/20240101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
             Signature=0000"
        );
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", &host)
            .header("content-type", format!("application/x-amz-json-{json_version}"))
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential={credential}"),
            )
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let decoded = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
            })
        };
        (status, decoded)
    }

    /// POST `uri` with `body` as JSON; answer the status and the decoded body.
    pub(crate) async fn call(&self, uri: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://batch.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("batch.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let decoded = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
            })
        };
        (status, decoded)
    }

    /// POST `uri`, requiring a 200.
    pub(crate) async fn ok(&self, uri: &str, body: Value) -> Value {
        let (status, decoded) = self.call(uri, body).await;
        assert_eq!(status, 200, "case {uri}: expected 200, got {status} {decoded}");
        decoded
    }
}

/// `arn:aws:batch:us-east-1:<account>:<resource>`.
pub(crate) fn arn(resource: &str) -> String {
    format!(
        "arn:aws:batch:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `message` of an error response. Shared by every suite in this target.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// An `UNMANAGED`, `ENABLED` compute environment called `name`; answers its ARN.
pub(crate) async fn compute_environment(batch: &Batch, name: &str) -> String {
    let body = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": name, "type": "UNMANAGED", "state": "ENABLED"}),
        )
        .await;
    body["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A queue called `name` over the compute environment `environment`; answers its ARN.
pub(crate) async fn job_queue(batch: &Batch, name: &str, environment: &str) -> String {
    let body = batch
        .ok(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": name,
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": environment}],
            }),
        )
        .await;
    body["jobQueueArn"].as_str().unwrap_or_default().to_string()
}

/// A `container` job definition called `name` carrying `container_properties`; answers
/// its ARN and revision.
pub(crate) async fn job_definition(
    batch: &Batch,
    name: &str,
    container_properties: Value,
) -> (String, i64) {
    let body = batch
        .ok(
            "/v1/registerjobdefinition",
            json!({
                "jobDefinitionName": name,
                "type": "container",
                "containerProperties": container_properties,
            }),
        )
        .await;
    (
        body["jobDefinitionArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        body["revision"].as_i64().unwrap_or_default(),
    )
}

/// The wire error code of an error response, with any namespace prefix dropped
/// the way an SDK client drops it: the single copy this crate's suites share.
pub(crate) fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>");
    raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string()
}

/// The smallest container properties a definition can carry.
pub(crate) fn container_properties() -> Value {
    json!({
        "image": "busybox",
        "command": ["echo", "hi"],
        "resourceRequirements": [
            {"type": "VCPU", "value": "1"},
            {"type": "MEMORY", "value": "512"},
        ],
    })
}

/// The one `JobDetail` for `job_id`.
pub(crate) async fn describe_job(batch: &Batch, job_id: &str) -> Value {
    let body = batch.ok("/v1/describejobs", json!({"jobs": [job_id]})).await;
    let jobs = body["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        jobs.len(),
        1,
        "case describe {job_id}: expected exactly one JobDetail, got {body}"
    );
    jobs[0].clone()
}

/// How long to wait for every clock-owed job transition to fall due. Each one takes
/// [`state::JOB_PHASE_MILLIS`]; a job with no dependencies makes two (SUBMITTED ->
/// RUNNABLE -> terminal), and the margin covers a slow machine.
async fn wait_for_terminal() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// The lifecycle the issue names, end to end: compute environment -> queue -> job
/// definition -> submit -> describe to a terminal state -> terminate a second job, with
/// every body checked rather than just the status code.
#[tokio::test]
async fn job_submission_lifecycle_round_trips_through_persisted_state() {
    let batch = Batch::new();

    // 1. The compute environment is minted once and reported identically afterwards.
    let environment_arn = compute_environment(&batch, "ce-demo").await;
    assert_eq!(
        environment_arn,
        arn("compute-environment/ce-demo"),
        "case create compute environment: expected the minted ARN, got {environment_arn:?}"
    );

    // Re-creating the same name fails instead of overwriting.
    let (status, body) = batch
        .call(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-demo", "type": "UNMANAGED"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case duplicate compute environment: expected 400, got {status} {body}"
    );

    // A MANAGED environment without computeResources is refused.
    let (status, body) = batch
        .call(
            "/v1/createcomputeenvironment",
            json!({"computeEnvironmentName": "ce-managed", "type": "MANAGED"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case MANAGED without computeResources: expected 400, got {status} {body}"
    );

    // Straight after the create, the environment is still in the transient status the
    // clock owes a move out of.
    let created = batch
        .ok(
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": ["ce-demo"]}),
        )
        .await;
    assert_eq!(
        created["computeEnvironments"][0]["status"].as_str(),
        Some(state::STATUS_CREATING),
        "case describe straight after create: expected CREATING, got {created}"
    );

    // 2. The queue stores the environment NORMALISED to its ARN.
    let queue_arn = job_queue(&batch, "q-demo", "ce-demo").await;
    assert_eq!(
        queue_arn,
        arn("job-queue/q-demo"),
        "case create job queue: expected the minted ARN, got {queue_arn:?}"
    );
    let queues = batch
        .ok("/v1/describejobqueues", json!({"jobQueues": ["q-demo"]}))
        .await;
    let queue = &queues["jobQueues"][0];
    assert_eq!(
        queue["computeEnvironmentOrder"][0]["computeEnvironment"]
            .as_str()
            .unwrap_or_default(),
        environment_arn,
        "case describe job queue: the order must report the environment ARN, got {queue}"
    );
    assert_eq!(
        queue["priority"].as_i64(),
        Some(1),
        "case describe job queue: expected the submitted priority, got {queue}"
    );
    assert_eq!(
        queue["state"].as_str(),
        Some("ENABLED"),
        "case describe job queue: state defaults to ENABLED, got {queue}"
    );

    // A queue over an unknown environment is refused.
    let (status, body) = batch
        .call(
            "/v1/createjobqueue",
            json!({
                "jobQueueName": "q-nope",
                "priority": 1,
                "computeEnvironmentOrder": [{"order": 1, "computeEnvironment": "no-such-ce"}],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case queue over an unknown environment: expected 400, got {status} {body}"
    );

    // 3. Revisions count up per name, and both stay ACTIVE and describable.
    let (first_arn, first_revision) =
        job_definition(&batch, "jd-demo", container_properties()).await;
    let (second_arn, second_revision) =
        job_definition(&batch, "jd-demo", container_properties()).await;
    assert_eq!(
        (first_revision, second_revision),
        (1, 2),
        "case re-register: revisions must count up, got {first_revision} then {second_revision}"
    );
    assert_eq!(
        (first_arn.as_str(), second_arn.as_str()),
        (
            arn("job-definition/jd-demo:1").as_str(),
            arn("job-definition/jd-demo:2").as_str()
        ),
        "case re-register: the ARN must embed the revision"
    );
    let definitions = batch
        .ok(
            "/v1/describejobdefinitions",
            json!({"jobDefinitionName": "jd-demo"}),
        )
        .await;
    let listed = definitions["jobDefinitions"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        listed.len(),
        2,
        "case describe job definitions: both revisions must stay describable, got {definitions}"
    );
    for definition in &listed {
        assert_eq!(
            definition["status"].as_str(),
            Some("ACTIVE"),
            "case describe job definitions: every revision is ACTIVE, got {definition}"
        );
        assert_eq!(
            definition["containerProperties"]["image"].as_str(),
            Some("busybox"),
            "case describe job definitions: the stored properties come back, got {definition}"
        );
    }

    // 4. Submit resolves the queue and the definition against persisted state.
    let submitted = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "j1",
                "jobQueue": "q-demo",
                "jobDefinition": "jd-demo",
                "containerOverrides": {"command": ["echo", "overridden"]},
                "tags": {"team": "batch"},
            }),
        )
        .await;
    let job_id = submitted["jobId"].as_str().unwrap_or_default().to_string();
    assert!(
        !job_id.is_empty(),
        "case submit: expected a job id, got {submitted}"
    );
    assert_eq!(
        submitted["jobArn"].as_str(),
        Some(arn(&format!("job/{job_id}")).as_str()),
        "case submit: the job ARN must carry the minted id, got {submitted}"
    );
    assert_eq!(
        submitted["jobName"].as_str(),
        Some("j1"),
        "case submit: expected the submitted name, got {submitted}"
    );

    // 5. Describe reports exactly what was submitted, from the stored record.
    let detail = describe_job(&batch, &job_id).await;
    assert_eq!(
        detail["jobQueue"].as_str(),
        Some(queue_arn.as_str()),
        "case describe: the queue is reported as its ARN, got {detail}"
    );
    assert_eq!(
        detail["jobDefinition"].as_str(),
        Some(second_arn.as_str()),
        "case describe: a bare name resolves to the latest ACTIVE revision, got {detail}"
    );
    assert_eq!(
        detail["status"].as_str(),
        Some(state::JOB_SUBMITTED),
        "case describe straight after submit: the transient status is observable, got {detail}"
    );
    assert_eq!(
        detail["container"]["command"],
        json!(["echo", "overridden"]),
        "case describe: containerOverrides.command replaces the definition's, got {detail}"
    );
    assert_eq!(
        detail["container"]["image"].as_str(),
        Some("busybox"),
        "case describe: the definition's image survives the override, got {detail}"
    );
    assert_eq!(
        detail["tags"]["team"].as_str(),
        Some("batch"),
        "case describe: the submitted tags come back, got {detail}"
    );
    assert!(
        detail["createdAt"].as_i64().unwrap_or_default() > 0,
        "case describe: createdAt is epoch milliseconds, got {detail}"
    );

    // An unknown queue or definition is refused rather than answered with an id.
    for (case, body) in [
        (
            "unknown queue",
            json!({"jobName": "x", "jobQueue": "nope", "jobDefinition": "jd-demo"}),
        ),
        (
            "unknown definition",
            json!({"jobName": "x", "jobQueue": "q-demo", "jobDefinition": "nope"}),
        ),
        (
            "unknown definition revision",
            json!({"jobName": "x", "jobQueue": "q-demo", "jobDefinition": "jd-demo:99"}),
        ),
        (
            "unknown dependency",
            json!({
                "jobName": "x",
                "jobQueue": "q-demo",
                "jobDefinition": "jd-demo",
                "dependsOn": [{"jobId": "00000000-0000-4000-8000-000000000000"}],
            }),
        ),
    ] {
        let (status, decoded) = batch.call("/v1/submitjob", body).await;
        assert_eq!(
            status, 400,
            "case submit with an {case}: expected 400, got {status} {decoded}"
        );
    }

    // 6. The clock, not the reader, moves the job to a terminal status.
    wait_for_terminal().await;
    // ... and by the same clock the environment and the queue have become VALID.
    for (case, uri, request, field) in [
        (
            "compute environment",
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": ["ce-demo"]}),
            "computeEnvironments",
        ),
        (
            "job queue",
            "/v1/describejobqueues",
            json!({"jobQueues": ["q-demo"]}),
            "jobQueues",
        ),
    ] {
        let decoded = batch.ok(uri, request).await;
        assert_eq!(
            decoded[field][0]["status"].as_str(),
            Some(state::STATUS_VALID),
            "case {case} after its creation span: expected VALID, got {decoded}"
        );
    }
    let detail = describe_job(&batch, &job_id).await;
    let terminal = detail["status"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        status_rank(&terminal),
        5,
        "case describe after the lifecycle span: expected a terminal status, got {detail}"
    );
    assert!(
        !detail["statusReason"].as_str().unwrap_or_default().is_empty(),
        "case terminal job: statusReason must say why, got {detail}"
    );
    assert!(
        detail["stoppedAt"].as_i64().unwrap_or_default() > 0,
        "case terminal job: stoppedAt must be set, got {detail}"
    );
    // A terminal status is final, and a second read answers identically.
    let again = describe_job(&batch, &job_id).await;
    assert_eq!(
        again["status"].as_str(),
        detail["status"].as_str(),
        "case re-describe a terminal job: a read must not change it, got {again}"
    );
    assert_eq!(
        again["stoppedAt"], detail["stoppedAt"],
        "case re-describe a terminal job: stoppedAt must be stable, got {again}"
    );

    // 7. Terminating a second job is observed by the following describe.
    let second = batch
        .ok(
            "/v1/submitjob",
            json!({"jobName": "j2", "jobQueue": &queue_arn, "jobDefinition": &first_arn}),
        )
        .await;
    let second_id = second["jobId"].as_str().unwrap_or_default().to_string();
    let terminated = batch
        .ok(
            "/v1/terminatejob",
            json!({"jobId": &second_id, "reason": "stop it"}),
        )
        .await;
    // TerminateJob models no output members, so the body is the empty object.
    assert_eq!(
        terminated,
        json!({}),
        "case terminate: TerminateJob has no output members, got {terminated}"
    );
    let detail = describe_job(&batch, &second_id).await;
    assert_eq!(
        detail["status"].as_str(),
        Some(state::JOB_FAILED),
        "case terminate: expected FAILED, got {detail}"
    );
    assert_eq!(
        detail["statusReason"].as_str(),
        Some("stop it"),
        "case terminate: the caller's reason is the statusReason, got {detail}"
    );
    assert!(
        detail["stoppedAt"].as_i64().unwrap_or_default() > 0,
        "case terminate: stoppedAt must be set, got {detail}"
    );
    assert_eq!(
        detail["isCancelled"].as_bool(),
        Some(true),
        "case terminate before STARTING: the job is cancelled, got {detail}"
    );
    assert_eq!(
        detail["jobDefinition"].as_str(),
        Some(first_arn.as_str()),
        "case submit by definition ARN: the named revision is kept, got {detail}"
    );

    // Terminating a job id the service holds none for is ACCEPTED, and fabricates
    // nothing: the empty 200 the operation models, and the id is still unknown to
    // DescribeJobs below.
    let (status, body) = batch
        .call(
            "/v1/terminatejob",
            json!({"jobId": "does-not-exist", "reason": "x"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case terminate an unknown job: expected the empty 200, got {status} {body}"
    );

    // 8. An unknown job id is omitted from DescribeJobs, not an error.
    let body = batch
        .ok(
            "/v1/describejobs",
            json!({"jobs": ["does-not-exist", &job_id]}),
        )
        .await;
    let jobs = body["jobs"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        jobs.len(),
        1,
        "case describe with an unknown id: the unknown one is omitted, got {body}"
    );

    // 9. ListJobs reports the queue's jobs for an explicit status, and honours the
    //    documented default of RUNNING only.
    let listed = batch
        .ok(
            "/v1/listjobs",
            json!({"jobQueue": "q-demo", "jobStatus": state::JOB_FAILED}),
        )
        .await;
    let summaries = listed["jobSummaryList"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        2,
        "case list FAILED jobs: expected both jobs of the queue, got {listed}"
    );
    assert_eq!(
        summaries[0]["createdAt"]
            .as_i64()
            .unwrap_or_default()
            .cmp(&summaries[1]["createdAt"].as_i64().unwrap_or_default()),
        std::cmp::Ordering::Greater,
        "case list: the most recent job comes first, got {listed}"
    );
    let defaulted = batch
        .ok("/v1/listjobs", json!({"jobQueue": "q-demo"}))
        .await;
    assert_eq!(
        defaulted["jobSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case list without jobStatus: only RUNNING jobs are returned, got {defaulted}"
    );
}

/// RULE: a member the model documents as "name or ARN" resolves both ways, and a job
/// definition reference also resolves as `<name>:<revision>`. One row per form.
#[tokio::test]
async fn names_and_arns_resolve_both_ways() {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-both").await;
    let queue_arn = job_queue(&batch, "q-both", &environment_arn).await;
    let (first_arn, _) = job_definition(&batch, "jd-both", container_properties()).await;
    let (second_arn, _) = job_definition(&batch, "jd-both", container_properties()).await;

    // Each row: the reference submitted, and the job definition ARN it must resolve to.
    let rows: &[(&str, &str, &str)] = &[
        ("queue name + bare definition name", "q-both", "jd-both"),
        ("queue ARN + bare definition name", &queue_arn, "jd-both"),
        ("queue name + name:revision", "q-both", "jd-both:1"),
        ("queue ARN + name:revision", &queue_arn, "jd-both:2"),
        ("queue name + definition ARN", "q-both", &first_arn),
        ("queue ARN + definition ARN", &queue_arn, &second_arn),
    ];
    for (case, queue, definition) in rows {
        let expected = match *definition {
            "jd-both" => second_arn.as_str(),
            "jd-both:1" => first_arn.as_str(),
            "jd-both:2" => second_arn.as_str(),
            other => other,
        };
        let submitted = batch
            .ok(
                "/v1/submitjob",
                json!({"jobName": "resolve", "jobQueue": queue, "jobDefinition": definition}),
            )
            .await;
        let job_id = submitted["jobId"].as_str().unwrap_or_default().to_string();
        let detail = describe_job(&batch, &job_id).await;
        assert_eq!(
            detail["jobQueue"].as_str(),
            Some(queue_arn.as_str()),
            "case {case:?}: the queue must resolve to its ARN, got {detail}"
        );
        assert_eq!(
            detail["jobDefinition"].as_str(),
            Some(expected),
            "case {case:?}: expected {expected}, got {detail}"
        );
    }

    // The same two forms read a compute environment back.
    for (case, reference) in [
        ("by name", "ce-both"),
        ("by ARN", environment_arn.as_str()),
    ] {
        let body = batch
            .ok(
                "/v1/describecomputeenvironments",
                json!({"computeEnvironments": [reference]}),
            )
            .await;
        assert_eq!(
            body["computeEnvironments"][0]["computeEnvironmentArn"].as_str(),
            Some(environment_arn.as_str()),
            "case describe compute environment {case}: got {body}"
        );
        assert_eq!(
            body["computeEnvironments"][0]["type"].as_str(),
            Some("UNMANAGED"),
            "case describe compute environment {case}: the stored type comes back, got {body}"
        );
    }

    // A reference naming nothing is omitted, not an error.
    for (case, uri, body) in [
        (
            "compute environment",
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": ["no-such-thing"]}),
        ),
        (
            "job queue",
            "/v1/describejobqueues",
            json!({"jobQueues": ["no-such-thing"]}),
        ),
    ] {
        let decoded = batch.ok(uri, body).await;
        let items = match case {
            "compute environment" => &decoded["computeEnvironments"],
            _ => &decoded["jobQueues"],
        };
        assert_eq!(
            items.as_array().map(Vec::len),
            Some(0),
            "case describe an unknown {case}: expected an empty list, got {decoded}"
        );
    }
}

/// One row of the container-override merge table: the overrides submitted, then what
/// the effective container must report for each member the merge touches.
struct MergeCase {
    case: &'static str,
    overrides: Value,
    command: Value,
    vcpus: Option<i64>,
    memory: Option<i64>,
    environment: Value,
    resource_requirements: Value,
}

/// RULE: the container a job gets is the definition's `containerProperties` with
/// `containerOverrides` applied — scalars replaced, `environment` and
/// `resourceRequirements` merged by name and by type.
#[tokio::test]
async fn container_overrides_merge_into_the_definition() {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-merge").await;
    let queue_arn = job_queue(&batch, "q-merge", &environment_arn).await;
    job_definition(
        &batch,
        "jd-merge",
        json!({
            "image": "busybox",
            "command": ["definition", "command"],
            "vcpus": 1,
            "memory": 128,
            "environment": [{"name": "KEEP", "value": "yes"}, {"name": "REPLACE", "value": "old"}],
            "resourceRequirements": [
                {"type": "VCPU", "value": "1"},
                {"type": "MEMORY", "value": "512"},
            ],
        }),
    )
    .await;

    let rows = vec![
        MergeCase {
            case: "no overrides keeps the definition",
            overrides: json!(null),
            command: json!(["definition", "command"]),
            vcpus: Some(1),
            memory: Some(128),
            environment: json!([{"name": "KEEP", "value": "yes"}, {"name": "REPLACE", "value": "old"}]),
            resource_requirements: json!([{"type": "VCPU", "value": "1"}, {"type": "MEMORY", "value": "512"}]),
        },
        MergeCase {
            case: "command is replaced wholesale",
            overrides: json!({"command": ["override"]}),
            command: json!(["override"]),
            vcpus: Some(1),
            memory: Some(128),
            environment: json!([{"name": "KEEP", "value": "yes"}, {"name": "REPLACE", "value": "old"}]),
            resource_requirements: json!([{"type": "VCPU", "value": "1"}, {"type": "MEMORY", "value": "512"}]),
        },
        MergeCase {
            case: "vcpus and memory are replaced",
            overrides: json!({"vcpus": 4, "memory": 2048}),
            command: json!(["definition", "command"]),
            vcpus: Some(4),
            memory: Some(2048),
            environment: json!([{"name": "KEEP", "value": "yes"}, {"name": "REPLACE", "value": "old"}]),
            resource_requirements: json!([{"type": "VCPU", "value": "1"}, {"type": "MEMORY", "value": "512"}]),
        },
        MergeCase {
            case: "environment merges by name",
            overrides: json!({"environment": [{"name": "REPLACE", "value": "new"}, {"name": "ADDED", "value": "1"}]}),
            command: json!(["definition", "command"]),
            vcpus: Some(1),
            memory: Some(128),
            environment: json!([
                {"name": "KEEP", "value": "yes"},
                {"name": "REPLACE", "value": "new"},
                {"name": "ADDED", "value": "1"},
            ]),
            resource_requirements: json!([{"type": "VCPU", "value": "1"}, {"type": "MEMORY", "value": "512"}]),
        },
        MergeCase {
            case: "resourceRequirements merge by type",
            overrides: json!({"resourceRequirements": [{"type": "MEMORY", "value": "1024"}, {"type": "GPU", "value": "2"}]}),
            command: json!(["definition", "command"]),
            vcpus: Some(1),
            memory: Some(128),
            environment: json!([{"name": "KEEP", "value": "yes"}, {"name": "REPLACE", "value": "old"}]),
            resource_requirements: json!([
                {"type": "VCPU", "value": "1"},
                {"type": "MEMORY", "value": "1024"},
                {"type": "GPU", "value": "2"},
            ]),
        },
    ];

    for row in &rows {
        let MergeCase {
            case,
            overrides,
            command,
            vcpus,
            memory,
            environment,
            resource_requirements: requirements,
        } = row;
        let mut request = json!({
            "jobName": "merge",
            "jobQueue": &queue_arn,
            "jobDefinition": "jd-merge",
        });
        if !overrides.is_null() {
            request["containerOverrides"] = overrides.clone();
        }
        let submitted = batch.ok("/v1/submitjob", request).await;
        let job_id = submitted["jobId"].as_str().unwrap_or_default().to_string();
        let detail = describe_job(&batch, &job_id).await;
        let container = &detail["container"];
        assert_eq!(
            &container["command"], command,
            "case {case:?}: expected command {command}, got {container}"
        );
        assert_eq!(
            container["vcpus"].as_i64(),
            *vcpus,
            "case {case:?}: expected vcpus {vcpus:?}, got {container}"
        );
        assert_eq!(
            container["memory"].as_i64(),
            *memory,
            "case {case:?}: expected memory {memory:?}, got {container}"
        );
        assert_eq!(
            &container["environment"], environment,
            "case {case:?}: expected environment {environment}, got {container}"
        );
        assert_eq!(
            &container["resourceRequirements"], requirements,
            "case {case:?}: expected resourceRequirements {requirements}, got {container}"
        );
    }
}

/// RULE (invariant): a job's status only ever moves FORWARD through the modelled
/// `JobStatus` order, it moves with the clock rather than with a read, and a terminal
/// status is final. Checked over a job with no dependencies and a job that depends on
/// it, polled repeatedly.
#[tokio::test]
async fn job_status_only_moves_forward_and_never_because_of_a_read() {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-progress").await;
    let queue_arn = job_queue(&batch, "q-progress", &environment_arn).await;
    job_definition(&batch, "jd-progress", container_properties()).await;

    let first = batch
        .ok(
            "/v1/submitjob",
            json!({"jobName": "p1", "jobQueue": &queue_arn, "jobDefinition": "jd-progress"}),
        )
        .await;
    let first_id = first["jobId"].as_str().unwrap_or_default().to_string();
    let dependent = batch
        .ok(
            "/v1/submitjob",
            json!({
                "jobName": "p2",
                "jobQueue": &queue_arn,
                "jobDefinition": "jd-progress",
                "dependsOn": [{"jobId": &first_id}],
            }),
        )
        .await;
    let dependent_id = dependent["jobId"].as_str().unwrap_or_default().to_string();

    for (case, job_id) in [("independent", &first_id), ("dependent", &dependent_id)] {
        // Two reads back to back: a read must not advance anything.
        let before = describe_job(&batch, job_id).await;
        let after = describe_job(&batch, job_id).await;
        assert_eq!(
            before["status"].as_str(),
            after["status"].as_str(),
            "case {case} read twice: a read must not move the status, got {after}"
        );
        assert_eq!(
            before["status"].as_str(),
            Some(state::JOB_SUBMITTED),
            "case {case} straight after submit: expected SUBMITTED, got {before}"
        );
    }

    // Poll both jobs to the end, checking the rank never goes backwards.
    let mut ranks = [0_u8, 0_u8];
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    for _ in 0..12 {
        for (index, job_id) in [&first_id, &dependent_id].into_iter().enumerate() {
            let detail = describe_job(&batch, job_id).await;
            let status = detail["status"].as_str().unwrap_or_default().to_string();
            let rank = status_rank(&status);
            assert!(
                rank >= ranks[index],
                "case poll job {index}: status went backwards to {status:?}, got {detail}"
            );
            ranks[index] = rank;
        }
        if ranks.iter().all(|rank| *rank == 5) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(span / 2)).await;
    }
    assert_eq!(
        ranks,
        [5, 5],
        "case poll to the end: both jobs must reach a terminal status, got {ranks:?}"
    );

    let dependent_detail = describe_job(&batch, &dependent_id).await;
    assert!(
        !dependent_detail["statusReason"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case dependent job: a terminal job says why, got {dependent_detail}"
    );
    // Nothing ran, so no attempt and no exit code are reported for it.
    assert!(
        dependent_detail["attempts"].is_null(),
        "case dependent job: no attempt is invented for work nothing ran, got {dependent_detail}"
    );
    assert!(
        dependent_detail["container"]["exitCode"].is_null(),
        "case dependent job: no exit code is invented, got {dependent_detail}"
    );
}

/// RULE: `ListJobs` selects exactly one of `jobQueue`/`arrayJobId`/`multiNodeJobId`, and
/// applies the documented filters — one at a time, with `jobStatus` ignored except
/// alongside `SHARE_IDENTIFIER`.
#[tokio::test]
async fn list_jobs_honours_the_documented_selectors_and_filters() {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-list").await;
    let queue_arn = job_queue(&batch, "q-list", &environment_arn).await;
    let (alpha_arn, _) = job_definition(&batch, "jd-alpha", container_properties()).await;
    job_definition(&batch, "jd-beta", container_properties()).await;

    let mut ids = Vec::new();
    for (name, definition, share) in [
        ("Report-alpha", "jd-alpha", "team-a"),
        ("report-beta", "jd-beta", "team-b"),
    ] {
        let submitted = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": name,
                    "jobQueue": &queue_arn,
                    "jobDefinition": definition,
                    "shareIdentifier": share,
                }),
            )
            .await;
        ids.push(submitted["jobId"].as_str().unwrap_or_default().to_string());
    }
    // The two submits need not land in the same millisecond (a submit also publishes the
    // job's state change), so a `BEFORE_CREATED_AT` bound meant to cover BOTH jobs is
    // taken from the LATER of the two instants rather than from the first job's.
    let created_at = describe_job(&batch, &ids[0]).await["createdAt"]
        .as_i64()
        .unwrap_or_default();
    let last_created_at = describe_job(&batch, &ids[1]).await["createdAt"]
        .as_i64()
        .unwrap_or_default();

    // Each row: the filters sent, and the job names the answer must contain.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "JOB_NAME exact",
            json!([{"name": "JOB_NAME", "values": ["Report-alpha"]}]),
            vec!["Report-alpha"],
        ),
        (
            "JOB_NAME is case-insensitive",
            json!([{"name": "JOB_NAME", "values": ["report-ALPHA"]}]),
            vec!["Report-alpha"],
        ),
        (
            "JOB_NAME with a trailing asterisk matches a prefix",
            json!([{"name": "JOB_NAME", "values": ["report*"]}]),
            vec!["Report-alpha", "report-beta"],
        ),
        (
            "JOB_DEFINITION by name matches any revision",
            json!([{"name": "JOB_DEFINITION", "values": ["jd-alpha"]}]),
            vec!["Report-alpha"],
        ),
        (
            "JOB_DEFINITION by ARN matches that revision",
            json!([{"name": "JOB_DEFINITION", "values": [alpha_arn.as_str()]}]),
            vec!["Report-alpha"],
        ),
        (
            "JOB_DEFINITION with a trailing asterisk matches a prefix",
            json!([{"name": "JOB_DEFINITION", "values": ["jd-*"]}]),
            vec!["Report-alpha", "report-beta"],
        ),
        (
            "BEFORE_CREATED_AT keeps jobs created at or before the bound",
            json!([{"name": "BEFORE_CREATED_AT", "values": [(last_created_at + 1).to_string()]}]),
            vec!["Report-alpha", "report-beta"],
        ),
        (
            "BEFORE_CREATED_AT excludes later jobs",
            json!([{"name": "BEFORE_CREATED_AT", "values": [(created_at - 1).to_string()]}]),
            vec![],
        ),
        (
            "AFTER_CREATED_AT keeps jobs created at or after the bound",
            json!([{"name": "AFTER_CREATED_AT", "values": [created_at.to_string()]}]),
            vec!["Report-alpha", "report-beta"],
        ),
    ];
    for (case, filters, expected) in rows {
        let body = batch
            .ok(
                "/v1/listjobs",
                json!({"jobQueue": "q-list", "filters": filters}),
            )
            .await;
        let mut names: Vec<String> = body["jobSummaryList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["jobName"].as_str().unwrap_or_default().to_string())
            .collect();
        names.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        wanted.sort();
        assert_eq!(
            names, wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }

    // SHARE_IDENTIFIER is the one filter that still honours jobStatus.
    let body = batch
        .ok(
            "/v1/listjobs",
            json!({
                "jobQueue": "q-list",
                "jobStatus": state::JOB_SUBMITTED,
                "filters": [{"name": "SHARE_IDENTIFIER", "values": ["team-a"]}],
            }),
        )
        .await;
    assert_eq!(
        body["jobSummaryList"][0]["jobName"].as_str(),
        Some("Report-alpha"),
        "case SHARE_IDENTIFIER with jobStatus: expected the one job of that share, got {body}"
    );
    let body = batch
        .ok(
            "/v1/listjobs",
            json!({
                "jobQueue": "q-list",
                "jobStatus": state::JOB_SUCCEEDED,
                "filters": [{"name": "SHARE_IDENTIFIER", "values": ["team-a"]}],
            }),
        )
        .await;
    assert_eq!(
        body["jobSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case SHARE_IDENTIFIER with a non-matching jobStatus: expected nothing, got {body}"
    );

    // Refusals: no selector, two selectors, two filters, an unknown filter name, an
    // unparseable timestamp. A filter TARGET the service does not hold is not among them:
    // it is an empty page (asserted below), and only a filter-SELECTION error is refused.
    for (case, body) in [
        ("no selector", json!({})),
        (
            "two selectors",
            json!({"jobQueue": "q-list", "arrayJobId": ids[0].as_str()}),
        ),
        (
            "two filters",
            json!({
                "jobQueue": "q-list",
                "filters": [
                    {"name": "JOB_NAME", "values": ["a"]},
                    {"name": "SHARE_IDENTIFIER", "values": ["b"]},
                ],
            }),
        ),
        (
            "an unknown filter name",
            json!({"jobQueue": "q-list", "filters": [{"name": "NOPE", "values": ["a"]}]}),
        ),
        (
            "an unparseable timestamp",
            json!({
                "jobQueue": "q-list",
                "filters": [{"name": "AFTER_CREATED_AT", "values": ["not-a-number"]}],
            }),
        ),
    ] {
        let (status, decoded) = batch.call("/v1/listjobs", body).await;
        assert_eq!(
            status, 400,
            "case list with {case}: expected 400, got {status} {decoded}"
        );
    }

    // A filter target that names nothing stored selects nothing: HTTP 200 with an empty
    // `jobSummaryList`, never a refusal.
    for (case, body) in [
        ("an unknown queue", json!({"jobQueue": "no-such-queue"})),
        ("an unknown array parent", json!({"arrayJobId": "no-such-job"})),
        (
            "an unknown multi-node parent",
            json!({"multiNodeJobId": "no-such-job"}),
        ),
    ] {
        let listed = batch.ok("/v1/listjobs", body).await;
        assert_eq!(
            listed["jobSummaryList"].as_array().map(Vec::len),
            Some(0),
            "case list with {case}: expected an empty list, got {listed}"
        );
    }

    // A job that exists but is no array parent has no children to list.
    let body = batch
        .ok("/v1/listjobs", json!({"arrayJobId": ids[0].as_str()}))
        .await;
    assert_eq!(
        body["jobSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case list the children of a job that has none: expected an empty list, got {body}"
    );
}

/// RULE (invariant): paging with `maxResults`/`nextToken` sees every item exactly once,
/// and a token that names no page is refused.
#[tokio::test]
async fn paging_sees_every_item_exactly_once() {
    let batch = Batch::new();
    let mut expected = Vec::new();
    for index in 0..3 {
        let (arn, _) = job_definition(&batch, &format!("jd-page-{index}"), container_properties()).await;
        expected.push(arn);
    }
    expected.sort();

    for page_size in [1, 2, 3, 7] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..6 {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let body = batch.ok("/v1/describejobdefinitions", request).await;
            let page = body["jobDefinitions"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case maxResults {page_size}: a page must not exceed it, got {body}"
            );
            for definition in &page {
                seen.push(
                    definition["jobDefinitionArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            token = body["nextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case maxResults {page_size}: paging must terminate, left token {token:?}"
        );
        seen.sort();
        assert_eq!(
            seen, expected,
            "case maxResults {page_size}: every revision exactly once, got {seen:?}"
        );
    }

    let (status, body) = batch
        .call(
            "/v1/describejobdefinitions",
            json!({"nextToken": "not-a-token"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case an invalid nextToken: expected 400, got {status} {body}"
    );
}

/// How a job's dependency reached a terminal status, and when.
#[derive(Debug, Clone, Copy)]
enum DependencyEnd {
    /// The clock ends it (nothing runs the container, so it fails on its own).
    ByClock,
    /// `TerminateJob` ends it while the dependent is still `SUBMITTED`.
    TerminatedWhileDependentIsSubmitted,
    /// `TerminateJob` ends it after the dependent has already reached `PENDING` — the case
    /// where the dependent is out of the clock's due index and must be woken by the
    /// dependency finishing.
    TerminatedWhileDependentIsPending,
}

/// Poll `job_id` until it reaches a terminal status, and answer its last `JobDetail`.
async fn poll_until_terminal(batch: &Batch, job_id: &str) -> Value {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    let mut detail = describe_job(batch, job_id).await;
    for _ in 0..12 {
        let status = detail["status"].as_str().unwrap_or_default();
        if status == state::JOB_SUCCEEDED || status == state::JOB_FAILED {
            return detail;
        }
        tokio::time::sleep(Duration::from_millis(span / 2)).await;
        detail = describe_job(batch, job_id).await;
    }
    detail
}

/// RULE: a job waiting on another is released by the dependency reaching a terminal
/// status, whatever ROUTE took it there — the clock or `TerminateJob` — and whether or not
/// the waiting job had already left `SUBMITTED`; a dependency that failed fails the job
/// that waited on it, with the reason AWS Batch gives.
#[tokio::test]
async fn a_dependent_job_is_released_however_its_dependency_ended() {
    for case in [
        DependencyEnd::ByClock,
        DependencyEnd::TerminatedWhileDependentIsSubmitted,
        DependencyEnd::TerminatedWhileDependentIsPending,
    ] {
        let batch = Batch::new();
        let environment_arn = compute_environment(&batch, "ce-depends").await;
        let queue_arn = job_queue(&batch, "q-depends", &environment_arn).await;
        job_definition(&batch, "jd-depends", container_properties()).await;

        let dependency = batch
            .ok(
                "/v1/submitjob",
                json!({"jobName": "d1", "jobQueue": &queue_arn, "jobDefinition": "jd-depends"}),
            )
            .await;
        let dependency_id = dependency["jobId"].as_str().unwrap_or_default().to_string();
        let dependent = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": "d2",
                    "jobQueue": &queue_arn,
                    "jobDefinition": "jd-depends",
                    "dependsOn": [{"jobId": &dependency_id, "type": "SEQUENTIAL"}],
                }),
            )
            .await;
        let dependent_id = dependent["jobId"].as_str().unwrap_or_default().to_string();

        let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
        // What the dependency's own `statusReason` must say once it is terminal. For a
        // terminated row it is the caller's reason — unless the dependency's own
        // clock-owed failure got there first on a loaded machine, which makes that row
        // behave like the ByClock one, so the row is read off observed state rather than
        // assumed from the wall clock.
        let mut expected_dependency_reason = state::NO_CONTAINER_RUNTIME_REASON;
        match case {
            DependencyEnd::ByClock => {}
            DependencyEnd::TerminatedWhileDependentIsSubmitted => {
                let before = describe_job(&batch, &dependency_id).await;
                if before["status"].as_str() != Some(state::JOB_FAILED) {
                    expected_dependency_reason = "stop the dependency";
                }
                batch
                    .ok(
                        "/v1/terminatejob",
                        json!({"jobId": &dependency_id, "reason": "stop the dependency"}),
                    )
                    .await;
            }
            DependencyEnd::TerminatedWhileDependentIsPending => {
                // Driven off OBSERVED state, not a fixed sleep: poll until the dependent
                // reports PENDING — where it waits with nothing owed by the clock — so a
                // slow machine delays the case instead of breaking it.
                let mut waiting = describe_job(&batch, &dependent_id).await;
                for _ in 0..40 {
                    let status = waiting["status"].as_str().unwrap_or_default();
                    if status != state::JOB_SUBMITTED {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis((span / 10).max(1))).await;
                    waiting = describe_job(&batch, &dependent_id).await;
                }
                assert_eq!(
                    waiting["status"].as_str(),
                    Some(state::JOB_PENDING),
                    "case {case:?}: the dependent must be waiting in PENDING first, got {waiting}"
                );
                let before = describe_job(&batch, &dependency_id).await;
                if before["status"].as_str() != Some(state::JOB_FAILED) {
                    expected_dependency_reason = "stop the dependency";
                }
                batch
                    .ok(
                        "/v1/terminatejob",
                        json!({"jobId": &dependency_id, "reason": "stop the dependency"}),
                    )
                    .await;
            }
        }

        let detail = poll_until_terminal(&batch, &dependent_id).await;
        assert_eq!(
            detail["status"].as_str(),
            Some(state::JOB_FAILED),
            "case {case:?}: a job whose dependency failed must reach FAILED, got {detail}"
        );
        assert_eq!(
            detail["statusReason"].as_str(),
            Some(state::DEPENDENT_JOB_FAILED_REASON),
            "case {case:?}: expected the documented dependency reason, got {detail}"
        );
        assert!(
            detail["stoppedAt"].as_i64().unwrap_or_default() > 0,
            "case {case:?}: a terminal job carries stoppedAt, got {detail}"
        );
        // `JobDetail$startedAt` is @required, so a value is always serialized; nothing
        // ran, so it is the 0 sentinel rather than an invented start instant.
        assert_eq!(
            detail["startedAt"].as_i64(),
            Some(0),
            "case {case:?}: no startedAt is invented for work nothing ran, got {detail}"
        );
        assert!(
            detail["attempts"].is_null(),
            "case {case:?}: no attempt is invented for work nothing ran, got {detail}"
        );

        // And the dependency itself ended the way this row observed it ending.
        let dependency_detail = describe_job(&batch, &dependency_id).await;
        assert_eq!(
            dependency_detail["statusReason"].as_str(),
            Some(expected_dependency_reason),
            "case {case:?}: the dependency's own reason, got {dependency_detail}"
        );
    }
}

/// RULE: every identifier this service mints has the shape AWS Batch gives it — a
/// version-4 UUID, `8-4-4-4-12` lowercase hex with the version nibble `4` and the variant
/// bits `10` (RFC 4122 §4.4) — and no two ids it mints are equal, whatever the resource.
#[tokio::test]
async fn every_minted_id_is_a_distinct_version_4_uuid() {
    let batch = Batch::new();
    let environment_arn = compute_environment(&batch, "ce-ids").await;
    let queue_arn = job_queue(&batch, "q-ids", &environment_arn).await;
    job_definition(&batch, "jd-ids", container_properties()).await;

    let mut ids = Vec::new();
    // The compute environment's own `uuid`, read back through DescribeComputeEnvironments.
    let described = batch
        .ok(
            "/v1/describecomputeenvironments",
            json!({"computeEnvironments": ["ce-ids"]}),
        )
        .await;
    ids.push(
        described["computeEnvironments"][0]["uuid"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    // And one id per submitted job.
    for index in 0..5 {
        let body = batch
            .ok(
                "/v1/submitjob",
                json!({
                    "jobName": format!("id-{index}"),
                    "jobQueue": &queue_arn,
                    "jobDefinition": "jd-ids",
                }),
            )
            .await;
        ids.push(body["jobId"].as_str().unwrap_or_default().to_string());
    }

    for id in &ids {
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "case id {id:?}: expected the 8-4-4-4-12 UUID grouping"
        );
        assert!(
            groups
                .iter()
                .all(|group| group.chars().all(|digit| digit.is_ascii_hexdigit()
                    && !digit.is_ascii_uppercase())),
            "case id {id:?}: expected lowercase hex digits only"
        );
        assert_eq!(
            groups[2].chars().next(),
            Some('4'),
            "case id {id:?}: expected version 4 in the third group"
        );
        assert!(
            matches!(groups[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "case id {id:?}: expected the RFC 4122 variant bits in the fourth group"
        );
    }
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case {ids:?}: every minted id must be distinct"
    );
}

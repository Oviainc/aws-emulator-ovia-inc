// SPDX-License-Identifier: CC0-1.0

//! The two halves of what a `CreateAgentRuntime` owes, asserted TOGETHER on one emulator:
//! the pre-condition it resolves before it writes anything, and the side effect a served
//! create leaves behind.
//!
//! The rule under test: a create that stores an agent runtime's `codeConfiguration`
//! artifact resolves the object-storage location it names BEFORE anything is written — a
//! location object storage holds no object at is refused `ValidationException` (HTTP 400)
//! and leaves NO trace at all, neither a runtime a listing reports nor the log group a
//! served create vends — while a create that is served both records the runtime and
//! provisions the runtime's managed CloudWatch Logs group, named for the `agentRuntimeId`
//! it minted, with no retention configured, and that group outlives
//! `DeleteAgentRuntime`.
//!
//! The suites in `code_artifact_object.rs` and `runtime_vended_log_group.rs` each pin one
//! half against one of the two services involved. This suite mounts BOTH object storage
//! and CloudWatch Logs beside the control plane on ONE engine — what the assembled
//! emulator does (`core/router`'s `build_router` registers every service crate) — so what
//! a refusal has to leave undone and what a success has to leave behind are read through
//! the front doors of the services that own them, in the same run.
//!
//! The rows span what the rule generalises over: the form of the location (a flat key, a
//! nested key, and a prefix an object is stored under), how much of it object storage
//! holds (the object, the bucket alone, nothing at all), and the Region the request is
//! signed for. Every refusal row then stores the exact object the request named and
//! re-sends the BYTE-IDENTICAL create, which must be served — so the refusal is pinned to
//! the absent object and to nothing else about the request.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol as agentcore;

/// The prefix CloudWatch Logs holds an AgentCore Runtime's vended logs under, and the
/// endpoint suffix the group a create vends carries. Spelled out here rather than read off
/// the implementation, so a drift in either is a failing test.
const RUNTIME_LOG_GROUP_PREFIX: &str = "/aws/bedrock-agentcore/runtimes/";
const DEFAULT_ENDPOINT: &str = "DEFAULT";

/// The Region object storage places a bucket created with no `LocationConstraint` in, which
/// is the one Region whose create carries no constraint.
const DEFAULT_BUCKET_REGION: &str = "us-east-1";

/// A `runtime` value of `AgentManagedRuntimeType` and an `entryPoint` within
/// `EntryPoints`' bound (`smithy_query(query="shape", shape="CodeConfiguration")`).
const RUNTIME: &str = "PYTHON_3_12";

/// The execution role every row names. No IAM is mounted, so nothing is known about the
/// role and the role gate refuses nothing: what each row turns on is the code object.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentRuntimeRole";

/// The first bytes of a zip archive — a deployment package is one, and nothing in the
/// emulator interprets its contents.
const ZIP: &[u8] = b"PK\x03\x04";

/// The emulator the rows run against: the control plane, with object storage and
/// CloudWatch Logs beside it on the one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // `default()` registers no authenticator, so the suite exercises routing and the
        // handlers rather than this process's SigV4 keys.
        let mut router = MultiServiceRouter::default();
        router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// One `restJson1` request to the control plane, signed for `region` — which is what
    /// the router routes on and what the handler reads the Region from.
    async fn control(&self, method: &str, uri: &str, region: &str, body: Option<&Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261024/{region}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    async fn create_runtime(&self, region: &str, body: &Value) -> Answer {
        self.control("PUT", "/runtimes/", region, Some(body)).await
    }

    async fn get_runtime(&self, region: &str, id: &str) -> Answer {
        self.control("GET", &format!("/runtimes/{id}/"), region, None)
            .await
    }

    async fn delete_runtime(&self, region: &str, id: &str) -> Answer {
        self.control("DELETE", &format!("/runtimes/{id}/"), region, None)
            .await
    }

    /// The `agentRuntimeName` of every runtime the control plane reports, in the order
    /// reported.
    async fn listed_runtime_names(&self, case: &str, region: &str) -> Vec<String> {
        let answer = self.control("POST", "/runtimes/", region, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from ListAgentRuntimes, got {} and body {}",
            answer.status, answer.body
        );
        answer.body["agentRuntimes"]
            .as_array()
            .map(|runtimes| {
                runtimes
                    .iter()
                    .filter_map(|runtime| runtime["agentRuntimeName"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One path-style request to object storage's front door. Object storage is a REST
    /// service: the bucket and the key live in the URI and the body is the object's own
    /// bytes. The credential scope names `s3`, which is what the router claims on.
    async fn s3(&self, method: &str, region: &str, path: &str, body: &[u8]) -> u16 {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261024/{region}/s3/aws4_request, \
             SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Create `bucket` in `region`. A create for the Region object storage places an
    /// unconstrained bucket in carries no `LocationConstraint`; every other Region's does.
    async fn create_bucket(&self, region: &str, bucket: &str) {
        let configuration = if region == DEFAULT_BUCKET_REGION {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <LocationConstraint>{region}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            )
        };
        let created = self
            .s3("PUT", region, &format!("/{bucket}"), configuration.as_bytes())
            .await;
        assert!(
            created == 200 || created == 409,
            "seeding s3://{bucket} in {region}: expected 200 or 409 from CreateBucket, got \
             {created}"
        );
    }

    /// Store a deployment package at `key` in `bucket`, creating the bucket first, so a
    /// request that resolves that location finds the object there. Both writes go over
    /// object storage's own front door, so the object a create reads is one object storage
    /// really holds.
    async fn put_object(&self, region: &str, bucket: &str, key: &str) {
        self.create_bucket(region, bucket).await;
        let stored = self
            .s3("PUT", region, &format!("/{bucket}/{key}"), ZIP)
            .await;
        assert_eq!(
            stored, 200,
            "seeding s3://{bucket}/{key} in {region}: expected 200 from PutObject, got {stored}"
        );
    }

    /// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`) signed for
    /// `region`. The credential scope names `logs`, which is what the router claims on.
    async fn logs(&self, region: &str, operation: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261024/{region}/logs/aws4_request, \
             SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://logs.{region}.amazonaws.com/"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Logs_20140328.{operation}"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    /// Every log group CloudWatch Logs reports under `prefix` in `region`, as a client
    /// tailing a runtime reads them.
    async fn log_groups(&self, case: &str, region: &str, prefix: &str) -> Vec<Value> {
        let answer = self
            .logs(
                region,
                "DescribeLogGroups",
                &json!({"logGroupNamePrefix": prefix}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeLogGroups, got {} and body {}",
            answer.status, answer.body
        );
        answer.body["logGroups"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The `logGroupName` of every group reported under `prefix`, in the order reported.
    async fn log_group_names(&self, case: &str, region: &str, prefix: &str) -> Vec<String> {
        self.log_groups(case, region, prefix)
            .await
            .iter()
            .filter_map(|group| group["logGroupName"].as_str().map(str::to_string))
            .collect()
    }
}

/// One answer from the emulator: its status and its JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Self { status, body }
    }

    /// The error code `restJson1` and `awsJson1_1` both carry in `__type`.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<no __type>")
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn id(&self, case: &str) -> String {
        self.body["agentRuntimeId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a served create reports an agentRuntimeId, got body {}",
                    self.body
                )
            })
            .to_string()
    }
}

/// How much of the code object storage holds before the first create of a row runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    /// The bucket exists and holds an object the location resolves to.
    Object,
    /// The bucket exists but holds no object at or under the location.
    BucketOnly,
    /// Object storage holds no such bucket.
    NoBucket,
}

/// One row: where the request says the runtime's code is, what object storage holds, and
/// where the request is signed.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    /// Within `AgentRuntimeName`'s `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` pattern.
    name: &'static str,
    bucket: &'static str,
    /// `S3Location$prefix` — the value the request carries.
    prefix: &'static str,
    /// The key object storage holds the package at once the location resolves.
    key: &'static str,
    stored: Stored,
}

/// The rows the rule is asserted over: the three forms of the location, each of the three
/// things object storage can hold for it, and a Region per row that is not always the
/// emulator's default.
const ROWS: &[Row] = &[
    Row {
        case: "an object at the flat key the location names",
        region: "us-east-1",
        name: "flat_key_agent",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        key: "deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "an object under the prefix the location names",
        region: "us-west-2",
        name: "prefix_agent",
        bucket: "agentcore-artifacts",
        prefix: "agents/py/",
        key: "agents/py/deployment_package.zip",
        stored: Stored::Object,
    },
    Row {
        case: "a nested key the bucket does not hold",
        region: "us-west-2",
        name: "missing_key_agent",
        bucket: "agentcore-artifacts",
        prefix: "agents/2026/deployment_package.zip",
        key: "agents/2026/deployment_package.zip",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a flat key the bucket does not hold",
        region: "eu-west-2",
        name: "missing_flat_key_agent",
        bucket: "agentcore-artifacts",
        prefix: "deployment_package.zip",
        key: "deployment_package.zip",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a prefix nothing is stored under",
        region: "us-east-1",
        name: "missing_prefix_agent",
        bucket: "agentcore-artifacts",
        prefix: "agents/node/",
        key: "agents/node/deployment_package.zip",
        stored: Stored::BucketOnly,
    },
    Row {
        case: "a bucket object storage does not hold",
        region: "eu-west-2",
        name: "missing_bucket_agent",
        bucket: "agentcore-absent-bucket",
        prefix: "deployment_package.zip",
        key: "deployment_package.zip",
        stored: Stored::NoBucket,
    },
];

impl Row {
    /// Whether the first create of this row names an object storage already holds.
    fn resolves(&self) -> bool {
        self.stored == Stored::Object
    }

    /// Store as much of the code as the row says object storage holds before the create.
    async fn seed(&self, emulator: &Emulator) {
        match self.stored {
            Stored::Object => emulator.put_object(self.region, self.bucket, self.key).await,
            // The bucket is seeded by storing an object the location does not resolve to.
            Stored::BucketOnly => {
                emulator
                    .put_object(self.region, self.bucket, "other/package.zip")
                    .await;
            }
            Stored::NoBucket => {}
        }
    }

    /// The create request this row sends — the same bytes before and after the object is
    /// stored.
    fn create_request(&self) -> Value {
        json!({
            "agentRuntimeName": self.name,
            "agentRuntimeArtifact": {"codeConfiguration": {
                "code": {"s3": {"bucket": self.bucket, "prefix": self.prefix}},
                "runtime": RUNTIME,
                "entryPoint": ["main.py"],
            }},
            "roleArn": ROLE_ARN,
            "networkConfiguration": {"networkMode": "PUBLIC"},
        })
    }
}

/// This instant in epoch milliseconds, which is what CloudWatch Logs timestamps an event
/// with. An event is ingested only inside the window the batch constraints allow around
/// now, so the event the suite writes is stamped from the clock rather than with a fixed
/// value that would age out of it.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("this process runs after the epoch")
            .as_millis(),
    )
    .expect("epoch milliseconds fit an i64")
}

/// The log group a client derives for the runtime `agent_runtime_id`.
fn vended_group(agent_runtime_id: &str) -> String {
    format!("{RUNTIME_LOG_GROUP_PREFIX}{agent_runtime_id}-{DEFAULT_ENDPOINT}")
}

/// Assert `answer` is this service's refusal of a code object that is not there: HTTP 400
/// `ValidationException`, the sentence the refusal carries, and the path through the
/// artifact union to the member that failed.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException".to_string()),
        "case {case:?}: expected HTTP 400 ValidationException for a code object that is not \
         there, got {} and body {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"],
        json!("S3 object not found in bucket"),
        "case {case:?}: unexpected refusal message, got body {}",
        answer.body
    );
    assert_eq!(
        answer.body["fieldList"][0]["name"],
        json!("agentRuntimeArtifact.codeConfiguration.code.s3"),
        "case {case:?}: the refusal must name the member that failed, got body {}",
        answer.body
    );
}

/// Assert exactly one group exists under the name a client derives for `id`, and that it
/// carries NO `retentionInDays` — a vended group's events never expire.
async fn assert_vended_group(emulator: &Emulator, case: &str, region: &str, id: &str) {
    let expected = vended_group(id);
    let groups = emulator.log_groups(case, region, &expected).await;
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected exactly the runtime's vended group {expected:?}, got {groups:?}"
    );
    assert_eq!(
        groups[0]["logGroupName"],
        json!(expected),
        "case {case:?}: expected the vended group {expected:?}, got {}",
        groups[0]
    );
    assert!(
        groups[0].get("retentionInDays").is_none(),
        "case {case:?}: a vended runtime log group carries no retention, so its events never \
         expire, got {}",
        groups[0]
    );
}

/// A create refused for a code object that is not there leaves NOTHING behind: no runtime
/// any listing reports, and no vended log group in CloudWatch Logs. The byte-identical
/// create is then served once that exact object is stored, and the group it vends is
/// there — and stays there when the runtime is deleted.
#[tokio::test]
async fn a_create_writes_nothing_until_its_code_object_resolves() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        row.seed(&emulator).await;

        // Nothing is vended before the create: the group is the create's own effect.
        let before = emulator
            .log_group_names(case, row.region, RUNTIME_LOG_GROUP_PREFIX)
            .await;
        assert!(
            before.is_empty(),
            "case {case:?}: no runtime log group may exist before any create, got {before:?}"
        );

        let mut served = emulator.create_runtime(row.region, &row.create_request()).await;
        if !row.resolves() {
            assert_refused(case, &served);
            // Refused BEFORE anything is written: the runtime is in no listing...
            let listed = emulator.listed_runtime_names(case, row.region).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused create records no runtime, got {listed:?}"
            );
            // ...and no group was vended for a runtime that was never minted.
            let groups = emulator
                .log_group_names(case, row.region, RUNTIME_LOG_GROUP_PREFIX)
                .await;
            assert!(
                groups.is_empty(),
                "case {case:?}: a refused create vends no log group, got {groups:?}"
            );

            // The same request, once object storage really holds the object it names, is
            // served: what was refused is the absent object and nothing else.
            emulator.put_object(row.region, row.bucket, row.key).await;
            served = emulator.create_runtime(row.region, &row.create_request()).await;
        }

        assert_eq!(
            served.status, 202,
            "case {case:?}: expected HTTP 202 from CreateAgentRuntime once the code object is \
             stored, got {} and body {}",
            served.status, served.body
        );
        let id = served.id(case);

        // The round trip: the runtime reads back under the very id the group's name is
        // derived from...
        let read = emulator.get_runtime(row.region, &id).await;
        assert_eq!(
            (read.status, read.body["agentRuntimeId"].clone()),
            (200, json!(id)),
            "case {case:?}: GetAgentRuntime must report the created runtime, got {} and body {}",
            read.status,
            read.body
        );
        let listed = emulator.listed_runtime_names(case, row.region).await;
        assert_eq!(
            listed,
            vec![row.name.to_string()],
            "case {case:?}: exactly the served runtime is listed, got {listed:?}"
        );
        // ...and its group is the only one vended under the runtimes prefix.
        assert_vended_group(&emulator, case, row.region, &id).await;
        let groups = emulator
            .log_group_names(case, row.region, RUNTIME_LOG_GROUP_PREFIX)
            .await;
        assert_eq!(
            groups,
            vec![vended_group(&id)],
            "case {case:?}: one served create vends exactly one group, got {groups:?}"
        );

        // Deleting the runtime does not cascade into CloudWatch Logs: the logs of a
        // runtime that is gone are what an operator diagnosing its last session reads.
        let deleted = emulator.delete_runtime(row.region, &id).await;
        assert_eq!(
            deleted.status, 202,
            "case {case:?}: expected HTTP 202 from DeleteAgentRuntime, got {} and body {}",
            deleted.status, deleted.body
        );
        let read = emulator.get_runtime(row.region, &id).await;
        assert_eq!(
            (read.status, read.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: a deleted runtime is not found, got {} and body {}",
            read.status,
            read.body
        );
        assert_vended_group(&emulator, case, row.region, &id).await;
    }
}

/// A group a client writes the runtime's output into is the group the create vended, and
/// the events in it survive the runtime: `DeleteAgentRuntime` removes neither the group
/// nor what is stored in it.
#[tokio::test]
async fn the_events_in_a_vended_group_outlive_the_runtime() {
    let case = "events written into the vended group";
    let region = "us-east-1";
    let emulator = Emulator::new();
    let row = ROWS[0];
    emulator.put_object(region, row.bucket, row.key).await;

    let created = emulator.create_runtime(region, &row.create_request()).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected HTTP 202 from CreateAgentRuntime, got {} and body {}",
        created.status, created.body
    );
    let id = created.id(case);
    let group = vended_group(&id);

    // A stream in the group the create vended, and one event in it. The group has to exist
    // already for either call to be served, which is what makes them a read of the
    // create's side effect.
    let stream = format!("{id}-runtime-logs");
    let created_stream = emulator
        .logs(
            region,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": stream}),
        )
        .await;
    assert_eq!(
        created_stream.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLogStream in the vended group, got {} and \
         body {}",
        created_stream.status, created_stream.body
    );
    let put = emulator
        .logs(
            region,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": stream,
                "logEvents": [{"timestamp": now_millis(), "message": "agent started"}],
            }),
        )
        .await;
    assert_eq!(
        put.status, 200,
        "case {case:?}: expected HTTP 200 from PutLogEvents in the vended group, got {} and body \
         {}",
        put.status, put.body
    );

    let deleted = emulator.delete_runtime(region, &id).await;
    assert_eq!(
        deleted.status, 202,
        "case {case:?}: expected HTTP 202 from DeleteAgentRuntime, got {} and body {}",
        deleted.status, deleted.body
    );

    // The group is still there, with no retention still...
    assert_vended_group(&emulator, case, region, &id).await;
    // ...and so is the event it holds.
    let events = emulator
        .logs(
            region,
            "FilterLogEvents",
            &json!({"logGroupName": group}),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "case {case:?}: expected HTTP 200 from FilterLogEvents, got {} and body {}",
        events.status, events.body
    );
    let messages: Vec<String> = events.body["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        messages,
        vec!["agent started".to_string()],
        "case {case:?}: the events of a deleted runtime's group are retained, got {messages:?}"
    );
}

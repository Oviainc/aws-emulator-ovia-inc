// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a DISTRIBUTED `Map` state's Map Run does besides
//! iterating: it writes its results where the state's `ResultWriter` says, it starts
//! its child workflow executions as the state machine's execution role, and a redrive
//! of its parent re-runs exactly the items that did not succeed.
//!
//! Driven over every service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions, S3 and IAM mounted on ONE engine — the
//! shape `core/router/src/lib.rs` mounts them in — so the objects a run writes are the
//! objects S3's own handlers stored and the policies a child start is judged against
//! are the documents IAM's own handlers wrote.
//!
//! The rules under test:
//!
//! 1. a Distributed `Map` state that declares a `ResultWriter` WRITES its run's
//!    results to the S3 location that writer names — one object per processing status
//!    that has results, and a `manifest.json` naming each of them — and the state puts
//!    out the Map Run's ARN and where the manifest was written rather than the results
//!    themselves;
//! 2. its child workflow executions are started by Step Functions as the state
//!    machine's execution role, so a role whose policies do not allow
//!    `states:StartExecution` on that state machine is a Map Run that FAILS having
//!    started none of them;
//! 3. redriving the parent of a Map Run that failed re-runs only the items whose child
//!    execution did not succeed: the ones that did keep the executions they ran as, so
//!    their work is not repeated, and the parent reports the redrive it took.
//!
//! Each table spans what its rule generalises over: for rule 1 the dataset the items
//! came from, how many of them there are, how they turned out and whether the writer
//! names a prefix; for rule 2 what the role's policies say about the action; for rule 3
//! which of the items failed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::RUNTIME_ERROR;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::execution_data_encryption::{form_encode, xml_element};
use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::trust_document;

/// Step Functions' wire facts, from its model's own `service_info`: the service shape
/// the `X-Amz-Target` prefix is, its `sigv4Name`, and the `awsJson` version its
/// protocol declares.
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
const SFN_SIGNING_NAME: &str = "states";
const SFN_JSON_VERSION: &str = "1.0";

/// S3's signing name, which is what routes a request to it
/// (`smithy_query(query="service_info")` on its model: `sigv4Name` and
/// `endpointPrefix` are both `s3`).
const S3_SIGNING_NAME: &str = "s3";

/// The access-key id every request below is signed with.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// How long a test waits for an execution to leave `RUNNING`.
const PATIENCE: Duration = Duration::from_secs(20);

/// The bucket the datasets below are read from and the results are written to.
pub(crate) const BUCKET: &str = "map-results";

/// The object one JSON dataset is held under.
const DATASET_KEY: &str = "datasets/items.json";

/// The manifest's own object name, as the language names it.
const MANIFEST: &str = "manifest.json";

/// Step Functions, S3 and IAM over ONE engine.
///
/// `pub(crate)`, not private: `tests/all/s3_sdk_task_integration.rs` drives the same
/// three services, and `tests/all.rs` is one crate root, so the wiring and the three
/// wire protocols are stated here once.
pub(crate) struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one Step Functions operation, requiring a 200.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{SFN_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{SFN_SIGNING_NAME}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{SFN_JSON_VERSION}"),
            )
            .header("x-amz-target", format!("{SFN_TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {operation}: expected 200, got {status} with body {parsed:?}"
        );
        parsed
    }

    /// Put one S3 request on the wire, PATH-STYLE against the Region's own endpoint.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let host = format!("{S3_SIGNING_NAME}.{REGION}.amazonaws.com");
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{S3_SIGNING_NAME}/\
             aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", authorization)
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// One awsQuery call to IAM, signed with the same credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/iam/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {action}: expected 200 from IAM, got {status} with {text}"
        );
        text
    }

    /// An IAM role carrying `policy` as its inline policy, by its own ARN.
    pub(crate) async fn role(&self, name: &str, policy: &Value) -> String {        let created = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name.to_string()),
                    ("AssumeRolePolicyDocument", trust_document().to_string()),
                ],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
        self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
        arn
    }

    /// A bucket in this Region.
    pub(crate) async fn bucket(&self, name: &str) {
        let configuration = format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        );
        let (status, body) = self
            .s3("PUT", &format!("/{name}"), configuration.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the bucket {name:?} must be created first, got {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    /// One object, stored through S3's own `PutObject`.
    pub(crate) async fn object(&self, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{BUCKET}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the object {key:?} must be stored first, got {status} with body {:?}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// What S3 holds under `key`, read back through its own `GetObject`; `None` for an
    /// object nothing stored.
    pub(crate) async fn stored(&self, key: &str) -> Option<String> {
        let (status, body) = self.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        match status {
            200 => Some(String::from_utf8_lossy(&body).to_string()),
            _ => None,
        }
    }

    /// The JSON document S3 holds under `key`.
    async fn stored_json(&self, key: &str, case: &str) -> Value {
        let text = self
            .stored(key)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected an object at {key:?}"));
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("case {case:?}: {key:?} is not JSON: {error} ({text})"))
    }

    /// Create a state machine under `role_arn` and return its ARN.
    pub(crate) async fn machine(&self, name: &str, definition: &Value, role_arn: &str) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": role_arn,
                }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Start one execution and report it once it has left `RUNNING`.
    pub(crate) async fn ran(&self, machine_arn: &str, input: &Value, case: &str) -> (String, Value) {
        let started = self
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine_arn, "input": input.to_string() }),
            )
            .await;
        let execution_arn = string(&started, "executionArn");
        let described = self.settled(&execution_arn, case).await;
        (execution_arn, described)
    }

    /// Poll one execution until it is no longer `RUNNING`.
    async fn settled(&self, execution_arn: &str, case: &str) -> Value {
        let began = Instant::now();
        loop {
            let described = self
                .sfn("DescribeExecution", &json!({ "executionArn": execution_arn }))
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return described;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described:?}"
            );
            ::tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The ARN of the one Map Run `execution_arn` started.
    async fn map_run_arn(&self, execution_arn: &str, case: &str) -> String {
        let listed = self
            .sfn("ListMapRuns", &json!({ "executionArn": execution_arn }))
            .await;
        let runs = listed
            .get("mapRuns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            runs.len(),
            1,
            "case {case:?}: expected exactly one Map Run, got {runs:?}"
        );
        string(&runs[0], "mapRunArn")
    }

    /// Every child workflow execution of one Map Run, as `DescribeExecution` reports
    /// it, keyed by the `id` member of the input its item carried.
    async fn children(&self, map_run_arn: &str) -> Map<String, Value> {
        let listed = self
            .sfn(
                "ListExecutions",
                &json!({ "mapRunArn": map_run_arn, "maxResults": 100 }),
            )
            .await;
        let mut children = Map::new();
        for child in listed
            .get("executions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let arn = string(&child, "executionArn");
            let described = self
                .sfn("DescribeExecution", &json!({ "executionArn": arn }))
                .await;
            let input: Value = described
                .get("input")
                .and_then(Value::as_str)
                .map(|text| serde_json::from_str(text).expect("a child's input is JSON"))
                .unwrap_or(Value::Null);
            let id = input
                .get("id")
                .map(ToString::to_string)
                .unwrap_or_else(|| arn.clone());
            children.insert(id, described);
        }
        children
    }

    /// `DescribeMapRun` for one Map Run.
    async fn map_run(&self, map_run_arn: &str) -> Value {
        self.sfn("DescribeMapRun", &json!({ "mapRunArn": map_run_arn }))
            .await
    }

    /// How many events of one execution's history carry `event_type`.
    async fn events_of(&self, execution_arn: &str, event_type: &str) -> usize {
        let answered = self
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution_arn, "maxResults": 1000 }),
            )
            .await;
        answered
            .get("events")
            .and_then(Value::as_array)
            .map_or(0, |events| {
                events
                    .iter()
                    .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
                    .count()
            })
    }
}

/// The `ItemProcessor` every row runs: an item carrying `"ok": true` succeeds with a
/// document of its own, and one carrying `false` fails at a `Fail` state.
fn gate_processor() -> Value {
    json!({
        "ProcessorConfig": { "Mode": "DISTRIBUTED" },
        "StartAt": "Check",
        "States": {
            "Check": {
                "Type": "Choice",
                "Choices": [{ "Variable": "$.ok", "BooleanEquals": true, "Next": "Keep" }],
                "Default": "Boom",
            },
            "Keep": {
                "Type": "Pass",
                "Parameters": { "id.$": "$.id", "done": true },
                "End": true,
            },
            "Boom": { "Type": "Fail", "Error": "ItemRejected", "Cause": "the item said so" },
        },
    })
}

/// A `ResultWriter` over the test bucket, under `prefix` when one is named.
fn result_writer(prefix: Option<&str>) -> Value {
    let mut parameters = json!({ "Bucket": BUCKET });
    if let Some(prefix) = prefix {
        parameters
            .as_object_mut()
            .expect("the parameters are an object")
            .insert("Prefix".to_string(), json!(prefix));
    }
    json!({
        "Resource": "arn:aws:states:::s3:putObject",
        "Parameters": parameters,
    })
}

/// An `ItemReader` over the one JSON object the dataset is held in.
fn json_dataset() -> Value {
    json!({
        "Resource": "arn:aws:states:::s3:getObject",
        "ReaderConfig": { "InputType": "JSON" },
        "Parameters": { "Bucket": BUCKET, "Key": DATASET_KEY },
    })
}

/// A Distributed `Map` state over `reader` (or over the execution's own input when it
/// names none), writing its results through `writer`.
fn mapping(reader: Option<Value>, writer: Option<Value>, tolerance: Option<Value>) -> Value {
    let mut state = json!({
        "Type": "Map",
        "ItemProcessor": gate_processor(),
        "End": true,
    });
    let members = state.as_object_mut().expect("the state is an object");
    if let Some(reader) = reader {
        members.insert("ItemReader".to_string(), reader);
    }
    if let Some(writer) = writer {
        members.insert("ResultWriter".to_string(), writer);
    }
    if let Some(tolerance) = tolerance {
        members.insert("ToleratedFailurePercentage".to_string(), tolerance);
    }
    json!({ "StartAt": "Fan", "States": { "Fan": state } })
}

/// `count` items, every one of which succeeds except the indices in `failing`.
fn items(count: usize, failing: &[usize]) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "id": index, "ok": !failing.contains(&index) }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Rule 1: the results are written where the ResultWriter says
// ---------------------------------------------------------------------------

/// One row of the writer table: how many items ran, which of them failed, where the
/// dataset came from and what prefix the writer names.
struct WriterCase {
    case: &'static str,
    count: usize,
    failing: &'static [usize],
    /// Whether the items are read from an S3 dataset rather than from the execution's
    /// own input: the writer must write the run's results either way.
    from_dataset: bool,
    prefix: Option<&'static str>,
}

/// A Distributed Map's `ResultWriter` writes the run's results to S3 and the state puts
/// out where they were written.
///
/// The rows differ along what the rule generalises over: the number of items, how they
/// turned out (all succeeded, some failed inside the state's tolerance), where the
/// items came from (the execution's input, an S3 dataset) and whether the destination
/// names a prefix.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_writer_writes_the_runs_results_and_its_manifest() {
    let rows: &[WriterCase] = &[
        WriterCase {
            case: "one item, under a prefix",
            count: 1,
            failing: &[],
            from_dataset: false,
            prefix: Some("runs"),
        },
        WriterCase {
            case: "several items, no prefix at all",
            count: 4,
            failing: &[],
            from_dataset: false,
            prefix: None,
        },
        WriterCase {
            case: "some items failed inside the tolerance",
            count: 5,
            failing: &[1, 3],
            from_dataset: false,
            prefix: Some("runs/mixed"),
        },
        WriterCase {
            case: "the items came from an S3 dataset",
            count: 3,
            failing: &[2],
            from_dataset: true,
            prefix: Some("runs/dataset"),
        },
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    // A role this account holds, trusts this service to assume and allows to start the
    // child executions of a Map Run: what the run WRITING its results is judged under,
    // so the rows below turn on the writer rather than on the grant (which the table
    // after this one is about).
    let role_arn = chain.role("WriterRole", &every_action()).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let all_items = items(row.count, row.failing);
        let (reader, input) = match row.from_dataset {
            true => {
                chain.object(DATASET_KEY, &all_items.to_string()).await;
                (Some(json_dataset()), json!({}))
            }
            false => (None, all_items.clone()),
        };
        // Every failure is tolerated, so the state SUCCEEDS whichever items failed and
        // the manifest is what says how they turned out.
        let definition = mapping(reader, Some(result_writer(row.prefix)), Some(json!(100)));
        let machine = chain
            .machine(&format!("writer-{index}"), &definition, &role_arn)
            .await;
        let (execution, described) = chain.ran(&machine, &input, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected SUCCEEDED, got {described:?}"
        );

        // What the state put out: the Map Run and where the manifest is, not the
        // results themselves.
        let output: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
        let map_run_arn = chain.map_run_arn(&execution, case).await;
        assert_eq!(
            output.get("MapRunArn").and_then(Value::as_str),
            Some(map_run_arn.as_str()),
            "case {case:?}: the output names the Map Run, got {output:?}"
        );
        let details = output
            .get("ResultWriterDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected ResultWriterDetails in {output:?}"));
        assert_eq!(
            details.get("Bucket").and_then(Value::as_str),
            Some(BUCKET),
            "case {case:?}: the output names the destination bucket, got {output:?}"
        );
        let manifest_key = details
            .get("Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a manifest key in {output:?}"));
        let expected_folder = match row.prefix {
            Some(prefix) => format!(
                "{}/{}/",
                prefix.trim_end_matches('/'),
                map_run_arn.rsplit(':').next().unwrap_or_default()
            ),
            None => format!("{}/", map_run_arn.rsplit(':').next().unwrap_or_default()),
        };
        assert_eq!(
            manifest_key,
            format!("{expected_folder}{MANIFEST}"),
            "case {case:?}: the manifest is written under the run's own folder, got \
             {manifest_key:?}"
        );

        // The manifest itself, read back out of S3.
        let manifest = chain.stored_json(manifest_key, case).await;
        assert_eq!(
            (
                manifest.get("DestinationBucket").and_then(Value::as_str),
                manifest.get("MapRunArn").and_then(Value::as_str),
            ),
            (Some(BUCKET), Some(map_run_arn.as_str())),
            "case {case:?}: the manifest names the bucket and the Map Run, got {manifest:?}"
        );
        let files = manifest
            .get("ResultFiles")
            .unwrap_or_else(|| panic!("case {case:?}: expected ResultFiles in {manifest:?}"));
        let succeeded = row.count - row.failing.len();
        let expected: &[(&str, usize)] = &[
            ("SUCCEEDED", succeeded),
            ("FAILED", row.failing.len()),
            // No item of a run this emulator ran is left unstarted, so the status that
            // counts those is reported with no file at all.
            ("PENDING", 0),
        ];
        for (status, records) in expected {
            let listed = files
                .get(status)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a {status} list in {manifest:?}")
                });
            if *records == 0 {
                assert!(
                    listed.is_empty(),
                    "case {case:?}: {status} has no results, got {listed:?}"
                );
                continue;
            }
            assert_eq!(
                listed.len(),
                1,
                "case {case:?}: expected one {status} result file, got {listed:?}"
            );
            let key = string(&listed[0], "Key");
            assert_eq!(
                key,
                format!("{expected_folder}{status}_0.json"),
                "case {case:?}: the {status} results are written beside the manifest, got {key:?}"
            );
            let body = chain
                .stored(&key)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected an object at {key:?}"));
            assert_eq!(
                listed[0].get("Size").and_then(Value::as_u64),
                Some(body.len() as u64),
                "case {case:?}: the manifest reports the object's own size, got {:?} for {} \
                 bytes",
                listed[0],
                body.len()
            );
            let holds: Vec<Value> =
                serde_json::from_str(&body).expect("a result file holds a JSON array");
            assert_eq!(
                holds.len(),
                *records,
                "case {case:?}: expected {records} {status} records, got {holds:?}"
            );
            // Every record is one of the run's own child workflow executions, as that
            // execution reports itself.
            let children = chain.children(&map_run_arn).await;
            for record in &holds {
                let arn = string(record, "ExecutionArn");
                let child = children
                    .values()
                    .find(|child| child.get("executionArn").and_then(Value::as_str) == Some(&arn))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: {arn} is not a child of this Map Run")
                    });
                assert_eq!(
                    (
                        record.get("Status").and_then(Value::as_str),
                        record.get("Input").and_then(Value::as_str),
                        record.get("Output").and_then(Value::as_str),
                    ),
                    (
                        Some(*status),
                        child.get("input").and_then(Value::as_str),
                        child.get("output").and_then(Value::as_str),
                    ),
                    "case {case:?}: the record is the child's own report, got {record:?} for \
                     {child:?}"
                );
                if *status == "FAILED" {
                    assert_eq!(
                        record.get("Error").and_then(Value::as_str),
                        Some("ItemRejected"),
                        "case {case:?}: a failed record names the failure, got {record:?}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the children are started as the execution role
// ---------------------------------------------------------------------------

/// One row of the authorization table: what the role's policy says, and whether that
/// lets the Map Run start its children.
struct GrantCase {
    case: &'static str,
    /// The statements the role carries, built against the state machine's own ARN.
    statements: fn(&str) -> Value,
    allowed: bool,
}

/// A Distributed Map starts its child workflow executions only when the execution
/// role's policies allow `states:StartExecution` on the state machine; a role they do
/// not is a Map Run that fails having started none.
///
/// The rows differ along what the rule generalises over: a grant on the state machine
/// itself, a wildcard grant, a grant over another resource, no statement about the
/// action at all, and an explicit deny over a grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_distributed_map_starts_no_child_without_the_start_execution_grant() {
    let rows: &[GrantCase] = &[
        GrantCase {
            case: "a grant on the state machine itself",
            statements: |machine| json!([allow("states:StartExecution", machine)]),
            allowed: true,
        },
        GrantCase {
            case: "a wildcard grant",
            statements: |_machine| json!([allow("states:*", "*")]),
            allowed: true,
        },
        GrantCase {
            case: "no statement about the action",
            statements: |machine| json!([allow("states:DescribeExecution", machine)]),
            allowed: false,
        },
        GrantCase {
            case: "a grant over another state machine",
            statements: |_machine| {
                json!([allow(
                    "states:StartExecution",
                    "arn:aws:states:us-west-2:000000000000:stateMachine:elsewhere"
                )])
            },
            allowed: false,
        },
        GrantCase {
            case: "an explicit deny over a grant",
            statements: |machine| {
                json!([
                    allow("states:StartExecution", machine),
                    {
                        "Effect": "Deny",
                        "Action": "states:StartExecution",
                        "Resource": machine,
                    },
                ])
            },
            allowed: false,
        },
    ];

    let chain = Chain::new();
    let input = items(2, &[]);
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // The role is created BEFORE the state machine it is about, so the policy is
        // written over the ARN the machine will carry — the one the Map Run's own child
        // starts are taken on.
        let name = format!("grant-{index}");
        let machine_arn = format!(
            "arn:aws:states:{REGION}:000000000000:stateMachine:{name}"
        );
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": (row.statements)(&machine_arn),
        });
        let role_arn = chain.role(&format!("MapRole{index}"), &policy).await;
        let definition = mapping(None, None, None);
        let created = chain.machine(&name, &definition, &role_arn).await;
        assert_eq!(
            created, machine_arn,
            "case {case:?}: the policy must be written over the machine's own ARN"
        );
        let (execution, described) = chain.ran(&created, &input, case).await;
        let map_run_arn = chain.map_run_arn(&execution, case).await;
        let children = chain.children(&map_run_arn).await;
        let map_run = chain.map_run(&map_run_arn).await;

        if row.allowed {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: a granted role runs the map, got {described:?}"
            );
            assert_eq!(
                children.len(),
                2,
                "case {case:?}: every item ran as a child execution, got {children:?}"
            );
            continue;
        }
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(RUNTIME_ERROR)),
            "case {case:?}: a denied role fails the execution, got {described:?}"
        );
        let cause = described.get("cause").and_then(Value::as_str).unwrap_or("");
        assert!(
            cause.contains("not authorized to perform: states:StartExecution"),
            "case {case:?}: the cause is the refusal the start came back with, got {cause:?}"
        );
        assert!(
            children.is_empty(),
            "case {case:?}: a denied Map Run starts no child, got {children:?}"
        );
        assert_eq!(
            map_run.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: the Map Run itself failed, got {map_run:?}"
        );
        assert_eq!(
            map_run
                .get("itemCounts")
                .and_then(|counts| counts.get("total"))
                .and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a denied Map Run counted no item, got {map_run:?}"
        );
    }
}

/// A write that could not be made fails the state with `States.ResultWriterFailed`,
/// which the state's own `Retry` answers for exactly as it answers for its items'
/// failures: the attempt is run again, and the state ends on the last one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_write_fails_the_state_and_is_retried() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("RetryRole", &every_action()).await;
    // (case, the retriers the state declares, how many attempts the state makes)
    let rows: &[(&str, Value, usize)] = &[
        ("no Retry at all", json!([]), 1),
        (
            "a Retry naming the write's own error",
            json!([{ "ErrorEquals": ["States.ResultWriterFailed"], "MaxAttempts": 2, "IntervalSeconds": 0 }]),
            3,
        ),
        (
            "a Retry over every error",
            json!([{ "ErrorEquals": ["States.ALL"], "MaxAttempts": 1, "IntervalSeconds": 0 }]),
            2,
        ),
    ];
    for (index, (case, retry, attempts)) in rows.iter().enumerate() {
        // The destination bucket does not exist, so the write S3 answers is a refusal.
        let mut state = json!({
            "Type": "Map",
            "ItemProcessor": gate_processor(),
            "ResultWriter": {
                "Resource": "arn:aws:states:::s3:putObject",
                "Parameters": { "Bucket": "no-such-bucket", "Prefix": "runs" },
            },
            "End": true,
        });
        if !retry.as_array().is_some_and(Vec::is_empty) {
            state
                .as_object_mut()
                .expect("the state is an object")
                .insert("Retry".to_string(), retry.clone());
        }
        let definition = json!({ "StartAt": "Fan", "States": { "Fan": state } });
        let machine = chain
            .machine(&format!("write-fails-{index}"), &definition, &role_arn)
            .await;
        let (execution, described) = chain.ran(&machine, &items(2, &[]), case).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some("States.ResultWriterFailed")),
            "case {case:?}: a write that could not be made fails the state, got {described:?}"
        );
        assert_eq!(
            chain.events_of(&execution, "MapStateStarted").await,
            *attempts,
            "case {case:?}: the state made {attempts} attempt(s), each iterating its items"
        );
    }
}

/// An `Allow` statement over one action and one resource.
pub(crate) fn allow(action: &str, resource: &str) -> Value {
    json!({ "Effect": "Allow", "Action": action, "Resource": resource })
}

/// A `ResultWriter` writes the results of a MAP RUN, and an INLINE `Map` starts none:
/// a state that declares one without a distributed processor is not run for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_map_is_not_run_for_a_result_writer() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("InlineRole", &every_action()).await;
    let definition = json!({
        "StartAt": "Fan",
        "States": {
            "Fan": {
                "Type": "Map",
                // No `ProcessorConfig`, so the processor is INLINE and starts no Map Run.
                "ItemProcessor": {
                    "StartAt": "Keep",
                    "States": { "Keep": { "Type": "Pass", "End": true } },
                },
                "ResultWriter": result_writer(Some("inline")),
                "End": true,
            },
        },
    });
    let machine = chain.machine("inline-writer", &definition, &role_arn).await;
    let (_execution, described) = chain
        .ran(&machine, &items(2, &[]), "an inline Map with a ResultWriter")
        .await;
    assert_eq!(
        (
            described.get("status").and_then(Value::as_str),
            described.get("error").and_then(Value::as_str),
        ),
        (Some("FAILED"), Some(RUNTIME_ERROR)),
        "an inline Map is not run for a ResultWriter, got {described:?}"
    );
    assert!(
        described
            .get("cause")
            .and_then(Value::as_str)
            .is_some_and(|cause| cause.contains("'ResultWriter'")),
        "the cause says what was not run, got {described:?}"
    );
}

/// A policy allowing every action of this service on every resource: what a role that
/// is not what a row is about carries, so nothing it does is refused.
pub(crate) fn every_action() -> Value {
    json!({ "Version": "2012-10-17", "Statement": [allow("states:*", "*")] })
}

// ---------------------------------------------------------------------------
// Rule 3: a redrive re-runs only the items that did not succeed
// ---------------------------------------------------------------------------

/// Redriving the parent of a failed Map Run re-runs exactly the items whose child
/// execution did not succeed: the ones that did keep the executions they ran as, so
/// their work is not repeated.
///
/// The rows differ along which items failed: the first, the last, and several
/// scattered through the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redrive_reruns_only_the_items_that_did_not_succeed() {
    let rows: &[(&str, usize, &[usize])] = &[
        ("the first item", 3, &[0]),
        ("the last item", 4, &[3]),
        ("three of nine", 9, &[2, 4, 7]),
    ];

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("RedriveRole", &every_action()).await;
    for (index, (case, count, failing)) in rows.iter().enumerate() {
        let key = format!("datasets/redrive-{index}.json");
        chain.object(&key, &items(*count, failing).to_string()).await;
        let reader = json!({
            "Resource": "arn:aws:states:::s3:getObject",
            "ReaderConfig": { "InputType": "JSON" },
            "Parameters": { "Bucket": BUCKET, "Key": key },
        });
        // No tolerance: the run FAILS on its failed items, which is what makes the
        // execution redrivable at all.
        let definition = mapping(Some(reader), Some(result_writer(Some("redriven"))), None);
        let machine = chain
            .machine(&format!("redrive-{index}"), &definition, &role_arn)
            .await;
        let (execution, described) = chain.ran(&machine, &json!({}), case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: a run with failed items fails, got {described:?}"
        );
        let map_run_arn = chain.map_run_arn(&execution, case).await;
        let before = chain.children(&map_run_arn).await;
        assert_eq!(
            before.len(),
            *count,
            "case {case:?}: every item ran as a child execution, got {before:?}"
        );
        // The results of a run that FAILED are written too, under the run's own folder,
        // which is where the redriven attempt writes them again.
        let folder = format!(
            "redriven/{}/",
            map_run_arn.rsplit(':').next().unwrap_or_default()
        );
        let first_records = succeeded_records(&chain, &folder, case).await;
        for position in 0..*count {
            let child = &before[&position.to_string()];
            let expected = match failing.contains(&position) {
                true => "FAILED",
                false => "SUCCEEDED",
            };
            assert_eq!(
                child.get("status").and_then(Value::as_str),
                Some(expected),
                "case {case:?}: item {position} ended {expected}, got {child:?}"
            );
        }

        // The items are read again on the redriven attempt, so the dataset the failed
        // items came from is put right first — which is what a caller redriving a run
        // does.
        chain.object(&key, &items(*count, &[]).to_string()).await;
        let redriven = chain
            .sfn("RedriveExecution", &json!({ "executionArn": execution }))
            .await;
        assert!(
            redriven.get("redriveDate").is_some(),
            "case {case:?}: the redrive answers the date it happened, got {redriven:?}"
        );
        let described = chain.settled(&execution, case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: the redriven run succeeds, got {described:?}"
        );
        assert_eq!(
            described.get("redriveCount").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: the execution reports the redrive it took, got {described:?}"
        );
        assert!(
            described.get("redriveDate").is_some(),
            "case {case:?}: the execution reports when it was redriven, got {described:?}"
        );

        // The same Map Run, counting the same items: the ones that had succeeded kept
        // the executions they ran as, and only the ones that had failed ran again.
        let after = chain.children(&map_run_arn).await;
        assert_eq!(
            after.len(),
            *count,
            "case {case:?}: the run still has one child per item, got {after:?}"
        );
        for position in 0..*count {
            let position = position.to_string();
            let (was, now) = (&before[&position], &after[&position]);
            let (was_arn, now_arn) = (
                was.get("executionArn").and_then(Value::as_str),
                now.get("executionArn").and_then(Value::as_str),
            );
            assert_eq!(
                now.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: item {position} succeeded on the redriven attempt, got {now:?}"
            );
            if was.get("status").and_then(Value::as_str) == Some("SUCCEEDED") {
                assert_eq!(
                    (was_arn, was.get("startDate"), was.get("output")),
                    (now_arn, now.get("startDate"), now.get("output")),
                    "case {case:?}: item {position} had succeeded, so its execution is \
                     untouched: was {was:?}, now {now:?}"
                );
            } else {
                assert_ne!(
                    was_arn, now_arn,
                    "case {case:?}: item {position} had failed, so it ran as a new execution: \
                     was {was:?}, now {now:?}"
                );
            }
        }
        let map_run = chain.map_run(&map_run_arn).await;
        let counts = map_run
            .get("itemCounts")
            .unwrap_or_else(|| panic!("case {case:?}: expected itemCounts in {map_run:?}"));
        assert_eq!(
            (
                map_run.get("status").and_then(Value::as_str),
                counts.get("succeeded").and_then(Value::as_i64),
                counts.get("failed").and_then(Value::as_i64),
                map_run.get("redriveCount").and_then(Value::as_i64),
            ),
            (
                Some("SUCCEEDED"),
                Some(*count as i64),
                Some(0),
                Some(1)
            ),
            "case {case:?}: the redriven Map Run counts every item succeeded, got {map_run:?} \
             with {counts:?}"
        );

        // The results were written again under the SAME run's folder, because the
        // redriven attempt is the same Map Run.
        let output: Value = described
            .get("output")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str(text).expect("the output is JSON"))
            .unwrap_or_else(|| panic!("case {case:?}: expected an output in {described:?}"));
        let manifest_key = output
            .pointer("/ResultWriterDetails/Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a manifest key in {output:?}"));
        let manifest = chain.stored_json(manifest_key, case).await;
        let written = manifest
            .pointer("/ResultFiles/SUCCEEDED/0/Key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a result file in {manifest:?}"));
        let holds: Vec<Value> = serde_json::from_str(
            &chain
                .stored(written)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: expected an object at {written:?}")),
        )
        .expect("a result file holds a JSON array");
        assert_eq!(
            holds.len(),
            *count,
            "case {case:?}: the rewritten results hold every item of the run, got {holds:?}"
        );
        // A kept child's record does not move either: the execution it ran as is
        // untouched, so what the results report about it — the dates it ran between and
        // the output it produced — is what the attempt that ran it recorded.
        let second_records = succeeded_records(&chain, &folder, case).await;
        for (arn, first) in &first_records {
            let again = second_records
                .get(arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} is not in the rewritten results"));
            assert_eq!(
                (
                    first.get("StartDate"),
                    first.get("StopDate"),
                    first.get("Output"),
                ),
                (
                    again.get("StartDate"),
                    again.get("StopDate"),
                    again.get("Output"),
                ),
                "case {case:?}: the kept child {arn} reports what it always did, was \
                 {first:?}, now {again:?}"
            );
        }
    }
}

/// The records the `SUCCEEDED` result file of the run written under `folder` holds,
/// keyed by the execution each one is about.
async fn succeeded_records(chain: &Chain, folder: &str, case: &str) -> Map<String, Value> {
    let manifest = chain
        .stored_json(&format!("{folder}{MANIFEST}"), case)
        .await;
    let Some(key) = manifest
        .pointer("/ResultFiles/SUCCEEDED/0/Key")
        .and_then(Value::as_str)
    else {
        return Map::new();
    };
    let body = chain
        .stored(key)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: expected an object at {key:?}"));
    let holds: Vec<Value> = serde_json::from_str(&body).expect("a result file holds a JSON array");
    holds
        .into_iter()
        .map(|record| (string(&record, "ExecutionArn"), record))
        .collect()
}

/// An item a redrive keeps is identified by the DOCUMENT its iteration was handed, so a
/// dataset whose items moved between the attempts still keeps each item's own
/// execution rather than the one that sat at its position.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_item_is_matched_by_its_own_document() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("MovedItemsRole", &every_action()).await;
    let key = "datasets/moved.json";
    // Three items, the middle one failing.
    chain.object(key, &items(3, &[1]).to_string()).await;
    let definition = mapping(
        Some(json!({
            "Resource": "arn:aws:states:::s3:getObject",
            "ReaderConfig": { "InputType": "JSON" },
            "Parameters": { "Bucket": BUCKET, "Key": key },
        })),
        None,
        None,
    );
    let case = "the items moved between the attempts";
    let machine = chain.machine("moved-items", &definition, &role_arn).await;
    let (execution, described) = chain.ran(&machine, &json!({}), case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: the failed item fails the run, got {described:?}"
    );
    let map_run_arn = chain.map_run_arn(&execution, case).await;
    let before = chain.children(&map_run_arn).await;

    // The same three items, in the REVERSE order, with the failed one put right: the
    // two that had succeeded are the same items, at different positions.
    let reversed = Value::Array(
        (0..3usize)
            .rev()
            .map(|id| json!({ "id": id, "ok": true }))
            .collect(),
    );
    chain.object(key, &reversed.to_string()).await;
    chain
        .sfn("RedriveExecution", &json!({ "executionArn": execution }))
        .await;
    let described = chain.settled(&execution, case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: the redriven run succeeds, got {described:?}"
    );
    let after = chain.children(&map_run_arn).await;
    for position in 0..3usize {
        let position = position.to_string();
        let (was, now) = (&before[position.as_str()], &after[position.as_str()]);
        let kept = was.get("executionArn") == now.get("executionArn");
        assert_eq!(
            kept,
            was.get("status").and_then(Value::as_str) == Some("SUCCEEDED"),
            "case {case:?}: item {position} kept its own execution exactly if it had \
             succeeded, wherever the dataset now holds it: was {was:?}, now {now:?}"
        );
        if kept {
            assert_eq!(
                now.get("input"),
                was.get("input"),
                "case {case:?}: the execution item {position} kept is the one it ran as, over \
                 the very document it was handed, got {now:?}"
            );
        }
    }
}

/// A redrive matches the Map Run its state is a second attempt of by that state's own
/// LABEL, so a Distributed Map state AFTER another one is the same Map Run again —
/// which is what keeps its children, and its ARN, across the redrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_distributed_map_is_the_same_map_run_when_redriven() {
    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("SecondMapRole", &every_action()).await;
    // Two Distributed Map states, one after the other: the first succeeds and the
    // second fails on an item of its own, so the redrive continues from the second —
    // whose Map Run is the execution's SECOND, not its first.
    chain.object("datasets/first.json", &items(2, &[]).to_string()).await;
    chain
        .object("datasets/second.json", &items(3, &[1]).to_string())
        .await;
    let reader = |key: &str| {
        json!({
            "Resource": "arn:aws:states:::s3:getObject",
            "ReaderConfig": { "InputType": "JSON" },
            "Parameters": { "Bucket": BUCKET, "Key": key },
        })
    };
    let definition = json!({
        "StartAt": "Alpha",
        "States": {
            "Alpha": {
                "Type": "Map",
                "ItemReader": reader("datasets/first.json"),
                "ItemProcessor": gate_processor(),
                "Next": "Beta",
            },
            "Beta": {
                "Type": "Map",
                "ItemReader": reader("datasets/second.json"),
                "ItemProcessor": gate_processor(),
                "End": true,
            },
        },
    });
    let case = "a second Distributed Map redriven";
    let machine = chain.machine("second-map", &definition, &role_arn).await;
    let (execution, described) = chain.ran(&machine, &json!({}), case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("FAILED"),
        "case {case:?}: the second map's failed item fails the execution, got {described:?}"
    );
    let runs = chain
        .sfn("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let listed = runs
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        2,
        "case {case:?}: each Distributed Map state started a Map Run, got {listed:?}"
    );
    let (first_arn, second_arn) = (
        string(&listed[0], "mapRunArn"),
        string(&listed[1], "mapRunArn"),
    );
    let before = chain.children(&second_arn).await;

    // The dataset the failed item came from is put right, and the execution redriven.
    chain
        .object("datasets/second.json", &items(3, &[]).to_string())
        .await;
    chain
        .sfn("RedriveExecution", &json!({ "executionArn": execution }))
        .await;
    let described = chain.settled(&execution, case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {case:?}: the redriven run succeeds, got {described:?}"
    );
    let runs = chain
        .sfn("ListMapRuns", &json!({ "executionArn": execution }))
        .await;
    let listed: Vec<String> = runs
        .get("mapRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|run| string(run, "mapRunArn"))
        .collect();
    assert_eq!(
        listed,
        vec![first_arn.clone(), second_arn.clone()],
        "case {case:?}: both Map Runs are still the execution's, under the ARNs they had"
    );
    let after = chain.children(&second_arn).await;
    for position in 0..3usize {
        let position = position.to_string();
        let (was, now) = (&before[position.as_str()], &after[position.as_str()]);
        assert_eq!(
            now.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: item {position} succeeded on the redriven attempt, got {now:?}"
        );
        let same = was.get("executionArn") == now.get("executionArn");
        assert_eq!(
            same,
            was.get("status").and_then(Value::as_str) == Some("SUCCEEDED"),
            "case {case:?}: item {position} ran again exactly if it had not succeeded: was \
             {was:?}, now {now:?}"
        );
    }
}

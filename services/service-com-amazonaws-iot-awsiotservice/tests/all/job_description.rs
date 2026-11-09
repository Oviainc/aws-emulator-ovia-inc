// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DescribeJob`: the job registered under a `jobId` is
//! reported from the stored record, and an id no creator recorded is the modelled 404.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /jobs/{jobId}`, `GET /jobs/{jobId}` and `DELETE /jobs/{jobId}` signed
//! for the `iot` signing name — so what is asserted is what a client reads back through
//! the modelled surface, not the state struct.
//!
//! The rule under test: an identifier a create hands back addresses a record in the
//! plane that owns it, so `DescribeJob` answers 200 for the `jobId` a job was created
//! under, reporting the identity, the targets, the selection, the timeout, the
//! parameters and the document source that create persisted, plus the progress its
//! executions have made — and a `jobId` no record holds is the modelled
//! `ResourceNotFoundException` (HTTP 404), never a success and never a 501.
//!
//! The table spans what the rule generalises over — whether the job carries a
//! description, a `targetSelection` (and which), an in-progress timeout, substitution
//! parameters, an inline document or a `documentSource`, and how many devices it
//! targets — rather than one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// The `JobStatus` a job that is rolling out to its targets carries — one of the
/// model's `JobStatus` values (`IN_PROGRESS`, `CANCELED`, `COMPLETED`,
/// `DELETION_IN_PROGRESS`, `SCHEDULED`).
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("/message").unwrap_or_default()
    }

    /// The `job` member of a `DescribeJob` answer.
    fn job(&self) -> &Value {
        self.body.get("job").unwrap_or(&Value::Null)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_job(&self, job_id: &str, request: &Value) -> Reply {
        self.call("PUT", &format!("/jobs/{job_id}"), Some(request))
            .await
    }

    async fn describe_job(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/jobs/{job_id}"), None).await
    }

    async fn delete_job(&self, job_id: &str) -> Reply {
        self.call("DELETE", &format!("/jobs/{job_id}?force=true"), None)
            .await
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// The job ARN this plane mints for `job_id` — `Job$jobArn` is "an ARN identifying the
/// job with format arn:aws:iot:region:account:job/jobId".
fn job_arn(job_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:job/{job_id}")
}

/// One row of the table: a job to create, and what a read of it must report.
struct JobCase {
    case: &'static str,
    job_id: &'static str,
    /// The devices the job targets, by thing name.
    things: Vec<&'static str>,
    description: Option<&'static str>,
    target_selection: Option<&'static str>,
    document: Option<&'static str>,
    document_source: Option<&'static str>,
    in_progress_timeout_minutes: Option<i64>,
    parameters: Vec<(&'static str, &'static str)>,
}

/// RULE: `DescribeJob` reports the job registered under that `jobId` from the stored
/// record — every member its creator persisted, plus the progress counted from its
/// executions.
#[tokio::test]
async fn a_created_job_reads_back_member_for_member() {
    let emulator = Emulator::new();

    let cases = vec![
        JobCase {
            case: "one device, nothing but a document",
            job_id: "job-minimal",
            things: vec!["device-a"],
            description: None,
            target_selection: None,
            document: Some(r#"{"operation":"reboot"}"#),
            document_source: None,
            in_progress_timeout_minutes: None,
            parameters: Vec::new(),
        },
        JobCase {
            case: "a snapshot job with a description and a timeout",
            job_id: "job-snapshot",
            things: vec!["device-b"],
            description: Some("reboot the fleet"),
            target_selection: Some("SNAPSHOT"),
            document: Some(r#"{"operation":"reboot"}"#),
            document_source: None,
            in_progress_timeout_minutes: Some(5),
            parameters: Vec::new(),
        },
        JobCase {
            case: "a continuous job over three devices",
            job_id: "job-continuous",
            things: vec!["device-c", "device-d", "device-e"],
            description: None,
            target_selection: Some("CONTINUOUS"),
            document: Some(r#"{"operation":"install"}"#),
            document_source: None,
            in_progress_timeout_minutes: Some(10_080),
            parameters: Vec::new(),
        },
        JobCase {
            case: "a job whose document carries substitution parameters",
            job_id: "job-parameters",
            things: vec!["device-f"],
            description: Some("install what the parameters name"),
            target_selection: Some("SNAPSHOT"),
            document: Some(r#"{"operation":"${aws:iot:parameter:action}"}"#),
            document_source: None,
            in_progress_timeout_minutes: None,
            parameters: vec![("action", "install"), ("retries", "2")],
        },
        JobCase {
            case: "a job created from a documentSource",
            job_id: "job-document-source",
            things: vec!["device-g"],
            description: None,
            target_selection: None,
            document: None,
            document_source: Some("https://s3.us-east-1.amazonaws.com/updates/payload.json"),
            in_progress_timeout_minutes: Some(1),
            parameters: Vec::new(),
        },
    ];

    for case in &cases {
        let targets: Vec<String> = case.things.iter().map(|thing| thing_arn(thing)).collect();
        let mut request = json!({ "targets": targets });
        if let Some(description) = case.description {
            request["description"] = json!(description);
        }
        if let Some(target_selection) = case.target_selection {
            request["targetSelection"] = json!(target_selection);
        }
        if let Some(document) = case.document {
            request["document"] = json!(document);
        }
        if let Some(document_source) = case.document_source {
            request["documentSource"] = json!(document_source);
        }
        if let Some(minutes) = case.in_progress_timeout_minutes {
            request["timeoutConfig"] = json!({ "inProgressTimeoutInMinutes": minutes });
        }
        if !case.parameters.is_empty() {
            request["documentParameters"] = Value::Object(
                case.parameters
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }

        let created = emulator.create_job(case.job_id, &request).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the job must be creatable, got {} {:?}",
            case.case, created.status, created.body
        );

        let read = emulator.describe_job(case.job_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, read.status, read.body
        );

        // The identity the create reported is the identity the read answers with.
        assert_eq!(
            (read.text("/job/jobId"), read.text("/job/jobArn")),
            (
                Some(case.job_id.to_string()),
                created.text("/jobArn").or_else(|| Some(job_arn(case.job_id)))
            ),
            "case {:?}: expected the created job's id and ARN, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.text("/job/jobArn"),
            Some(job_arn(case.job_id)),
            "case {:?}: expected the modelled job ARN format, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.text("/job/status"),
            Some(STATUS_IN_PROGRESS.to_string()),
            "case {:?}: a job that is rolling out is {STATUS_IN_PROGRESS}, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.body
                .pointer("/job/targets")
                .and_then(Value::as_array)
                .map(|entries| entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<String>>()),
            Some(targets.clone()),
            "case {:?}: expected the targets the job was created with, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.text("/job/description").as_deref(),
            case.description,
            "case {:?}: expected the created description, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.text("/job/targetSelection").as_deref(),
            case.target_selection,
            "case {:?}: expected the created targetSelection, got {:?}",
            case.case,
            read.job()
        );
        assert_eq!(
            read.text("/documentSource").as_deref(),
            case.document_source,
            "case {:?}: expected the created documentSource, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.body
                .pointer("/job/timeoutConfig/inProgressTimeoutInMinutes")
                .and_then(Value::as_i64),
            case.in_progress_timeout_minutes,
            "case {:?}: expected the created timeoutConfig, got {:?}",
            case.case,
            read.job()
        );
        let expected_parameters: BTreeMap<String, String> = case
            .parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        let reported_parameters: BTreeMap<String, String> = read
            .body
            .pointer("/job/documentParameters")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value.as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported_parameters, expected_parameters,
            "case {:?}: expected the created documentParameters, got {:?}",
            case.case,
            read.job()
        );
        assert!(
            read.body.pointer("/job/createdAt").is_some(),
            "case {:?}: a job reports when it was created, got {:?}",
            case.case,
            read.job()
        );

        // The progress is COUNTED from the executions the create queued: one QUEUED
        // execution per targeted device, each device listed as a processing target.
        let devices = i64::try_from(case.things.len()).expect("a handful of devices");
        assert_eq!(
            read.body
                .pointer("/job/jobProcessDetails/numberOfQueuedThings")
                .and_then(Value::as_i64),
            Some(devices),
            "case {:?}: expected one queued thing per target, got {:?}",
            case.case,
            read.job()
        );
        let mut processing: Vec<String> = read
            .body
            .pointer("/job/jobProcessDetails/processingTargets")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        processing.sort();
        let mut expected_processing: Vec<String> =
            case.things.iter().map(|thing| (*thing).to_string()).collect();
        expected_processing.sort();
        assert_eq!(
            processing, expected_processing,
            "case {:?}: expected the targeted devices as processing targets, got {:?}",
            case.case,
            read.job()
        );
    }
}

/// RULE: a `jobId` no job was created under is the modelled `ResourceNotFoundException`
/// (HTTP 404) — never a 200 reporting a job that does not exist, and never a 501.
#[tokio::test]
async fn an_unknown_job_id_is_a_404() {
    let emulator = Emulator::new();

    let created = emulator
        .create_job(
            "job-that-exists",
            &json!({
                "targets": [thing_arn("device-a")],
                "document": r#"{"operation":"reboot"}"#,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the job must be creatable, got {} {:?}",
        created.status, created.body
    );

    // An id nobody used, one differing only in case, one differing in a separator, and
    // (below) the id of a job that has been deleted: none of them names a job.
    let unknown = ["job-that-exists-not", "JOB-THAT-EXISTS", "job_other", "x"];
    for job_id in unknown {
        let reply = emulator.describe_job(job_id).await;
        assert_eq!(
            reply.status, 404,
            "case {job_id:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {job_id:?}: expected ResourceNotFoundException, got {:?}",
            reply.error_code
        );
        assert_eq!(
            reply.message(),
            format!("Job {job_id} cannot be found."),
            "case {job_id:?}: expected the refusal to name the job, got {:?}",
            reply.body
        );
    }

    // Round trip the other way: a deleted job is a job no read finds.
    let deleted = emulator.delete_job("job-that-exists").await;
    assert_eq!(
        deleted.status, 200,
        "case \"a forced delete\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = emulator.describe_job("job-that-exists").await;
    assert_eq!(
        gone.status, 404,
        "case \"a deleted job\": expected 404, got {} {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a deleted job\": expected ResourceNotFoundException, got {:?}",
        gone.error_code
    );
}

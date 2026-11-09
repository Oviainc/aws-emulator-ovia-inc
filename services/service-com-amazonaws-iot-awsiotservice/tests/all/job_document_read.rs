// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `GetJobDocument`: a job's document is reported back in
//! the view the caller asks for.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `PUT /jobs/{jobId}` and `GET /jobs/{jobId}/job-document` signed for the
//! `iot` signing name — so the document a job was created with is read back through
//! the modelled surface, not out of the state struct.
//!
//! The rule under test: `GetJobDocument` reports the document of the job registered
//! under that `jobId`, with every substitution parameter the job carries resolved to
//! its value unless `beforeSubstitution` asks for the document as written; a
//! placeholder the job has no value for is reported as written, a job that has no
//! document reports none, and a `jobId` no job was created under is the modelled 404.
//!
//! The table spans what the rule generalises over — how many placeholders a document
//! carries and whether they resolve, the form (JSON, opaque text, empty) and size of
//! the document, and which view the request selects — rather than one recorded
//! request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    MAX_JOB_DOCUMENT_BYTES, MAX_JOB_DOCUMENT_PARAMETERS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::JobExecutionStore;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

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
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can read the SHARED job-execution store the control
/// plane writes the device-facing copy of the document into.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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

    /// `GetJobDocument` with the `beforeSubstitution` query parameter left out, which
    /// is the modelled default (`false`).
    async fn job_document(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/jobs/{job_id}/job-document"), None)
            .await
    }

    /// `GetJobDocument` with `beforeSubstitution` spelled out.
    async fn job_document_before(&self, job_id: &str, before_substitution: bool) -> Reply {
        self.call(
            "GET",
            &format!("/jobs/{job_id}/job-document?beforeSubstitution={before_substitution}"),
            None,
        )
        .await
    }

    /// The document the SHARED store holds for `thing_name`'s execution of a job: what
    /// the device is handed.
    fn execution_document(&self, thing_name: &str, job_id: &str) -> Option<String> {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .executions_for(thing_name)
            .iter()
            .find(|record| record.job_id == job_id)
            .and_then(|record| record.job_document.clone())
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// A substitution placeholder as AWS IoT's job-document substitution writes it: the
/// parameter's key inside `${aws:iot:parameter:…}`.
fn placeholder(key: &str) -> String {
    format!("${{aws:iot:parameter:{key}}}")
}

/// One row of the document table: a job to create, and the two views of its document.
struct DocumentCase {
    case: &'static str,
    job_id: &'static str,
    /// The `document` the job is created with, when the row carries one.
    document: Option<String>,
    /// The `documentSource` the job is created with, when the row carries one.
    document_source: Option<&'static str>,
    /// The `documentParameters` the job is created with.
    parameters: Vec<(&'static str, String)>,
    /// What the resolved view must report (`None`: no `document` member at all).
    resolved: Option<String>,
    /// What the `beforeSubstitution` view must report.
    before: Option<String>,
}

/// RULE: `GetJobDocument` reports the document of the job registered under that
/// `jobId`, resolved against the job's substitution parameters unless the caller asks
/// for the document before substitution.
#[tokio::test]
async fn a_job_document_is_reported_in_the_view_the_request_selects() {
    let emulator = Emulator::new();

    let plain = r#"{"operation":"reboot"}"#.to_string();
    let opaque = "reboot now, then report in".to_string();
    let one_parameter = format!(r#"{{"operation":"{}"}}"#, placeholder("action"));
    let repeated = format!(
        r#"{{"operation":"{}","retry":"{}"}}"#,
        placeholder("action"),
        placeholder("action")
    );
    let partly_unknown = format!(
        r#"{{"operation":"{}","url":"{}"}}"#,
        placeholder("action"),
        placeholder("artifact")
    );
    // The published maximum of `JobDocument` (len 0..32768), exactly.
    let filler = "x".repeat(MAX_JOB_DOCUMENT_BYTES - r#"{"payload":""}"#.len());
    let at_maximum = format!(r#"{{"payload":"{filler}"}}"#);

    let cases: Vec<DocumentCase> = vec![
        DocumentCase {
            case: "a document with no parameters reads back verbatim in both views",
            job_id: "job-plain",
            document: Some(plain.clone()),
            document_source: None,
            parameters: Vec::new(),
            resolved: Some(plain.clone()),
            before: Some(plain.clone()),
        },
        DocumentCase {
            case: "a document that is not JSON is still a document",
            job_id: "job-opaque",
            document: Some(opaque.clone()),
            document_source: None,
            parameters: Vec::new(),
            resolved: Some(opaque.clone()),
            before: Some(opaque.clone()),
        },
        DocumentCase {
            case: "an empty document is a document",
            job_id: "job-empty",
            document: Some(String::new()),
            document_source: None,
            parameters: Vec::new(),
            resolved: Some(String::new()),
            before: Some(String::new()),
        },
        DocumentCase {
            case: "one placeholder resolves to its parameter",
            job_id: "job-one-parameter",
            document: Some(one_parameter.clone()),
            document_source: None,
            parameters: vec![("action", "install".to_string())],
            resolved: Some(r#"{"operation":"install"}"#.to_string()),
            before: Some(one_parameter.clone()),
        },
        DocumentCase {
            case: "a placeholder used twice resolves at every occurrence",
            job_id: "job-repeated-parameter",
            document: Some(repeated.clone()),
            document_source: None,
            parameters: vec![("action", "install".to_string())],
            resolved: Some(r#"{"operation":"install","retry":"install"}"#.to_string()),
            before: Some(repeated.clone()),
        },
        DocumentCase {
            case: "a placeholder with no parameter is reported as written",
            job_id: "job-unknown-parameter",
            document: Some(partly_unknown.clone()),
            document_source: None,
            parameters: vec![("action", "install".to_string())],
            resolved: Some(format!(
                r#"{{"operation":"install","url":"{}"}}"#,
                placeholder("artifact")
            )),
            before: Some(partly_unknown.clone()),
        },
        DocumentCase {
            case: "a parameter whose value is itself brace-free text resolves once",
            job_id: "job-url-parameter",
            document: Some(format!(r#"{{"url":"{}"}}"#, placeholder("artifact"))),
            document_source: None,
            parameters: vec![(
                "artifact",
                "https://example.invalid/firmware-2.0.bin".to_string(),
            )],
            resolved: Some(r#"{"url":"https://example.invalid/firmware-2.0.bin"}"#.to_string()),
            before: Some(format!(r#"{{"url":"{}"}}"#, placeholder("artifact"))),
        },
        DocumentCase {
            case: "a document at the published maximum length",
            job_id: "job-at-maximum",
            document: Some(at_maximum.clone()),
            document_source: None,
            parameters: Vec::new(),
            resolved: Some(at_maximum.clone()),
            before: Some(at_maximum.clone()),
        },
        DocumentCase {
            case: "a job created from documentSource has no document here",
            job_id: "job-document-source",
            document: None,
            document_source: Some("https://s3.us-east-1.amazonaws.com/updates/payload.json"),
            parameters: Vec::new(),
            resolved: None,
            before: None,
        },
    ];

    for case in &cases {
        let thing_name = format!("device-for-{}", case.job_id);
        let mut request = json!({ "targets": [thing_arn(&thing_name)] });
        if let Some(document) = &case.document {
            request["document"] = json!(document);
        }
        if let Some(document_source) = case.document_source {
            request["documentSource"] = json!(document_source);
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

        for (before_substitution, expected) in [(false, &case.resolved), (true, &case.before)] {
            let reply = emulator
                .job_document_before(case.job_id, before_substitution)
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?} (beforeSubstitution={before_substitution}): expected 200, got {} {:?}",
                case.case, reply.status, reply.body
            );
            assert_eq!(
                reply.text("document").as_ref(),
                expected.as_ref(),
                "case {:?} (beforeSubstitution={before_substitution}): expected {:?}, got {:?}",
                case.case,
                expected,
                reply.text("document")
            );
        }

        // The flag left out is the modelled default (`false`), so it must report the
        // resolved view.
        let defaulted = emulator.job_document(case.job_id).await;
        assert_eq!(
            defaulted.text("document").as_ref(),
            case.resolved.as_ref(),
            "case {:?}: beforeSubstitution left out must report the resolved view, got {:?}",
            case.case,
            defaulted.body
        );

        // Round trip: the copy the device is handed is the resolved document too.
        assert_eq!(
            emulator.execution_document(&thing_name, case.job_id),
            case.resolved,
            "case {:?}: the targeted device's execution must carry the resolved document",
            case.case
        );
    }
}

/// RULE: a `jobId` no job was created under is the modelled `ResourceNotFoundException`
/// (HTTP 404), whichever view is asked for — never a 200 with no document.
#[tokio::test]
async fn an_unknown_job_id_is_a_404_in_either_view() {
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

    // A deleted job, a job id nobody ever used, and one that differs from a live job's
    // id only in case: none of them names a job.
    let unknown = ["job-that-exists-not", "JOB-THAT-EXISTS", "job_other"];
    for job_id in unknown {
        for before in [false, true] {
            let reply = emulator.job_document_before(job_id, before).await;
            assert_eq!(
                reply.status, 404,
                "case {job_id:?} (beforeSubstitution={before}): expected 404, got {} {:?}",
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
    }
}

/// RULE: a job whose document the request's parameters cannot be resolved into — more
/// parameters than one job may carry, or a resolved document past what `JobDocument`
/// may hold — is refused at creation, so no job exists whose document a reader cannot
/// report.
#[tokio::test]
async fn a_document_that_cannot_be_resolved_is_refused_before_the_job_exists() {
    let emulator = Emulator::new();

    // One parameter past the ceiling on how many a job may carry.
    let too_many: Vec<(String, String)> = (0..=MAX_JOB_DOCUMENT_PARAMETERS)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    let too_many_reply = emulator
        .create_job(
            "job-too-many-parameters",
            &json!({
                "targets": [thing_arn("device-b")],
                "document": format!(r#"{{"operation":"{}"}}"#, placeholder("key-0")),
                "documentParameters": Value::Object(
                    too_many
                        .iter()
                        .map(|(key, value)| (key.clone(), json!(value)))
                        .collect(),
                ),
            }),
        )
        .await;
    assert_eq!(
        too_many_reply.status, 410,
        "case \"one parameter past the ceiling\": expected 410, got {} {:?}",
        too_many_reply.status, too_many_reply.body
    );
    assert_eq!(
        too_many_reply.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"one parameter past the ceiling\": expected LimitExceededException, got {:?}",
        too_many_reply.error_code
    );

    // A document that fits, with a parameter whose value expands it past the maximum.
    let padding = "x".repeat(4_000);
    let oversized_reply = emulator
        .create_job(
            "job-oversized-resolution",
            &json!({
                "targets": [thing_arn("device-c")],
                "document": format!(r#"{{"pad":"{padding}","blob":"{}"}}"#, placeholder("blob")),
                // The published maximum of `ParameterValue` (len 1..30720).
                "documentParameters": { "blob": "y".repeat(30_720) },
            }),
        )
        .await;
    assert_eq!(
        oversized_reply.status, 400,
        "case \"a resolved document past the maximum\": expected 400, got {} {:?}",
        oversized_reply.status, oversized_reply.body
    );
    assert_eq!(
        oversized_reply.error_code.as_deref(),
        Some("InvalidRequestException"),
        "case \"a resolved document past the maximum\": expected InvalidRequestException, got {:?}",
        oversized_reply.error_code
    );

    // Neither refusal created a job, so neither id reads back a document.
    for job_id in ["job-too-many-parameters", "job-oversized-resolution"] {
        let reply = emulator.job_document(job_id).await;
        assert_eq!(
            reply.status, 404,
            "case {job_id:?}: a refused create must leave no job, got {} {:?}",
            reply.status, reply.body
        );
    }
}

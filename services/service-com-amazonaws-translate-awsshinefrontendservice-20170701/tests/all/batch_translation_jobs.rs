// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the asynchronous batch translation jobs:
//! `StartTextTranslationJob`, `DescribeTextTranslationJob` and
//! `StopTextTranslationJob`.
//!
//! Every case drives the production `MultiServiceRouter` with TWO services on
//! ONE shared engine — this service (`awsJson1_1`, `POST /` with
//! `X-Amz-Target`) and Amazon S3, where a job's input documents live and its
//! translations are written — so what is asserted is what a signing AWS client
//! would see: routing, constraint validation, the handlers, the in-process
//! cross-service read and write, and serialization.
//!
//! The rules under test:
//!
//! * a submitted job translates every document under its input location into
//!   each of its target languages, writes each translation into the output
//!   location's job folder, and reports the documents it read, translated and
//!   could not process;
//! * a job's status is the one the clock owes it — `SUBMITTED`, then
//!   `IN_PROGRESS`, then the status its own outcome reached — and its end time
//!   is reported only once it has ended;
//! * a stop request on a job that has not ended yet puts it into
//!   `STOP_REQUESTED` and then `STOPPED`, and a job that ended first keeps the
//!   status it reached.
//!
//! The suites are TABLES whose rows vary what those rules generalise over: the
//! number of documents, the number and identity of the target languages,
//! whether the source language is named or determined from the document, the
//! form of the two locations, the terminology applied, and the document that
//! cannot be processed.
//!
//! What a translated document CONTAINS is asserted against `TranslateText` for
//! the same text and pair rather than against a literal: one transform serves
//! both, so the two answering alike is the property, whatever that transform
//! renders.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service's operations are named under
/// (OPERATIONS.md, "Wire protocol").
const TARGET_PREFIX: &str = "AWSShineFrontendService_20170701";

/// The Region every request is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN and every job folder carries
/// (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The bucket the documents are uploaded to and the translations are written
/// to. Its name satisfies `S3Uri`'s modelled pattern.
const BUCKET: &str = "translate-batch-documents";

/// A `DataAccessRoleArn` satisfying the member's modelled pattern.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/TranslateBatchRole";

/// The only input content type this emulator's batch data plane reads
/// (`InputDataConfig$ContentType`).
const CONTENT_TYPE: &str = "text/plain";

/// The statuses a job reports before it has ended (`smithy_query
/// shape=JobStatus`).
const TRANSIENT_STATUSES: &[&str] = &["SUBMITTED", "IN_PROGRESS", "STOP_REQUESTED"];

/// How long a case waits for a job to reach a terminal status before calling it
/// stuck. Far above the emulator's own phase span, so a slow machine does not
/// fail the suite.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(40);

/// How long one phase of a job's lifecycle takes in this emulator
/// (`state::batch_translation::JOB_PHASE`), as seconds. A submitted job ends
/// two phases after it was submitted.
const PHASE_SECONDS: f64 = 3.0;

/// One emulator: this service and Amazon S3 on ONE shared engine, which is what
/// the assembled router does.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are verified by the emulator, never by a service; this
        // suite is about behaviour, so it signs with a fixed scope and verifies
        // nothing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_translate_awsshinefrontendservice_20170701::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One translate call (`AWSShineFrontendService_20170701.<Operation>` on
    /// `POST /`).
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let payload = Bytes::from(serde_json::to_vec(body).expect("the request body serializes"));
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", authorization("translate"))
            .header("host", format!("translate.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A translate call that must succeed.
    async fn call_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed}"
        );
        parsed
    }

    /// One path-style request to the emulator's S3 front door: the bucket and
    /// key live in the URI and the body is the object's bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization("s3"))
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }

    /// Create [`BUCKET`], which every case's documents live in.
    async fn create_bucket(&self) {
        let (status, _) = self.s3("PUT", &format!("/{BUCKET}"), &[]).await;
        assert!(
            status == 200 || status == 409,
            "CreateBucket {BUCKET}: expected 200 or 409, got {status}"
        );
    }

    /// Upload one object.
    async fn upload(&self, key: &str, body: &[u8]) {
        let (status, _) = self.s3("PUT", &format!("/{BUCKET}/{key}"), body).await;
        assert_eq!(status, 200, "PutObject {key}: expected 200, got {status}");
    }

    /// Read one object back, or `None` when the bucket holds none under that
    /// key.
    async fn download(&self, key: &str) -> Option<String> {
        let (status, body) = self.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        (status == 200).then(|| String::from_utf8_lossy(&body).to_string())
    }

    /// `DescribeTextTranslationJob`'s properties for `job_id`.
    async fn describe(&self, job_id: &str, case: &str) -> Value {
        self.call_ok(
            "DescribeTextTranslationJob",
            &json!({ "JobId": job_id }),
            case,
        )
        .await["TextTranslationJobProperties"]
            .clone()
    }

    /// Poll `DescribeTextTranslationJob` until the job leaves the transient
    /// statuses, and return its properties.
    ///
    /// Every status seen on the way has to be a transient one, and a status may
    /// never go backwards, so a job reported terminal immediately — which a
    /// submitted batch job never is — fails the case.
    async fn await_terminal(&self, job_id: &str, case: &str) -> Value {
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        let mut seen: Vec<String> = Vec::new();
        loop {
            let properties = self.describe(job_id, case).await;
            let status = properties["JobStatus"].as_str().unwrap_or("").to_string();
            if seen.last() != Some(&status) {
                seen.push(status.clone());
            }
            if !TRANSIENT_STATUSES.contains(&status.as_str()) {
                assert!(
                    !status.is_empty(),
                    "case {case:?}: expected a status, got {properties}"
                );
                let distinct: BTreeSet<&String> = seen.iter().collect();
                assert_eq!(
                    distinct.len(),
                    seen.len(),
                    "case {case:?}: expected each status once in order, got {seen:?}"
                );
                return properties;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the job never left {status:?} (seen {seen:?})"
            );
            ::tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names
/// `signing_name` — the routing signal the router reads first.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

/// The error code `awsJson1_1` puts in `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` member of a failure or of a job's properties.
fn message(body: &Value) -> String {
    for member in ["message", "Message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

/// A `JobDetails` count, or `-1` when the member is absent.
fn count_at(properties: &Value, member: &str) -> i64 {
    properties
        .pointer(&format!("/JobDetails/{member}"))
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// The request body of one submission.
fn submission(case: &JobCase, token: &str, index: usize) -> Value {
    let mut body = json!({
        "JobName": format!("batch-{index}"),
        "ClientToken": token,
        "DataAccessRoleArn": ROLE_ARN,
        "SourceLanguageCode": case.source,
        "TargetLanguageCodes": case.targets,
        "InputDataConfig": {
            "S3Uri": format!("s3://{BUCKET}/{}", case.input_prefix(index)),
            "ContentType": CONTENT_TYPE,
        },
        "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/{}", case.output_prefix(index)) },
    });
    if let Some(terminology) = case.terminology {
        body["TerminologyNames"] = json!([terminology]);
    }
    body
}

/// One row of the batch-job table.
struct JobCase {
    name: &'static str,
    /// The documents uploaded under the job's input location, as
    /// `(file name, text)`.
    documents: &'static [(&'static str, &'static str)],
    /// A document uploaded beside them whose bytes are not text, when the row
    /// exercises a document the job cannot process.
    binary_document: Option<&'static str>,
    source: &'static str,
    targets: &'static [&'static str],
    /// The custom terminology the job applies, when the row exercises one.
    terminology: Option<&'static str>,
    /// Whether the input location names the folder with a trailing delimiter,
    /// which a caller may write either way.
    trailing_delimiter: bool,
    expected_status: &'static str,
    expected_translated: i64,
    expected_errors: i64,
}

impl JobCase {
    /// The key prefix this row's documents are uploaded under.
    fn input_folder(&self, index: usize) -> String {
        format!("case-{index}/input/")
    }

    /// The `S3Uri` suffix the submission names the input location by.
    fn input_prefix(&self, index: usize) -> String {
        let folder = self.input_folder(index);
        if self.trailing_delimiter {
            folder
        } else {
            folder.trim_end_matches('/').to_string()
        }
    }

    /// The `S3Uri` suffix the submission names the output location by.
    fn output_prefix(&self, index: usize) -> String {
        format!("case-{index}/output")
    }
}

/// The rows: the documents, the languages, the locations and the terminology a
/// batch job generalises over, with the recorded case — a job that completes
/// successfully — among them.
const JOB_CASES: &[JobCase] = &[
    JobCase {
        name: "one document into one target language",
        documents: &[("greeting.txt", "hello world")],
        binary_document: None,
        source: "en",
        targets: &["fr"],
        terminology: None,
        trailing_delimiter: true,
        expected_status: "COMPLETED",
        expected_translated: 1,
        expected_errors: 0,
    },
    JobCase {
        name: "three documents into three target languages",
        documents: &[
            ("first.txt", "hello world"),
            ("second.txt", "the river is wide"),
            ("nested/third.txt", "good morning"),
        ],
        binary_document: None,
        source: "en",
        targets: &["fr", "de", "ja"],
        terminology: None,
        trailing_delimiter: true,
        expected_status: "COMPLETED",
        expected_translated: 3,
        expected_errors: 0,
    },
    JobCase {
        name: "input location named without a trailing delimiter",
        documents: &[("only.txt", "hello world")],
        binary_document: None,
        source: "en",
        targets: &["es"],
        terminology: None,
        trailing_delimiter: false,
        expected_status: "COMPLETED",
        expected_translated: 1,
        expected_errors: 0,
    },
    JobCase {
        name: "a custom terminology applied to every document",
        documents: &[("terms.txt", "hello world"), ("more.txt", "hello again")],
        binary_document: None,
        source: "en",
        targets: &["fr"],
        terminology: Some("batch-terms"),
        trailing_delimiter: true,
        expected_status: "COMPLETED",
        expected_translated: 2,
        expected_errors: 0,
    },
    JobCase {
        name: "source language determined from each document",
        documents: &[(
            "detected.txt",
            "The quick brown fox jumps over the lazy dog near the river bank at dawn.",
        )],
        binary_document: None,
        source: "auto",
        targets: &["fr"],
        terminology: None,
        trailing_delimiter: true,
        expected_status: "COMPLETED",
        expected_translated: 1,
        expected_errors: 0,
    },
    JobCase {
        name: "a document the job cannot read beside one it can",
        documents: &[("readable.txt", "hello world")],
        binary_document: Some("unreadable.txt"),
        source: "en",
        targets: &["fr"],
        terminology: None,
        trailing_delimiter: true,
        expected_status: "COMPLETED_WITH_ERROR",
        expected_translated: 1,
        expected_errors: 1,
    },
];

/// A batch job translates every document under its input location into each
/// target language, writes each translation into the output location's job
/// folder, and reports what it processed.
#[tokio::test]
async fn batch_jobs_translate_every_document_into_every_target_language() {
    let harness = Harness::new();
    harness.create_bucket().await;
    // One stored terminology, for the row that applies one: English `hello`
    // translates to `bonjour` in French.
    harness
        .call_ok(
            "ImportTerminology",
            &json!({
                "Name": "batch-terms",
                "MergeStrategy": "OVERWRITE",
                "TerminologyData": {
                    "File": aws_smithy_types::base64::encode("en,fr\nhello,bonjour\n".as_bytes()),
                    "Format": "CSV",
                },
            }),
            "terminology fixture",
        )
        .await;

    for (index, case) in JOB_CASES.iter().enumerate() {
        let name = case.name;
        let folder = case.input_folder(index);
        for (file, text) in case.documents {
            harness
                .upload(&format!("{folder}{file}"), text.as_bytes())
                .await;
        }
        if let Some(file) = case.binary_document {
            // Bytes that are not text at all, so the document is not the
            // `text/plain` the job declared.
            harness
                .upload(&format!("{folder}{file}"), &[0xf0, 0x9f, 0x00, 0xff, 0xfe])
                .await;
        }

        let submitted = harness
            .call_ok(
                "StartTextTranslationJob",
                &submission(case, &format!("token-case-{index}"), index),
                name,
            )
            .await;
        let job_id = submitted["JobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !job_id.is_empty() && job_id.len() <= 32,
            "case {name:?}: expected a JobId of at most 32 characters, got {submitted}"
        );
        assert_eq!(
            submitted["JobStatus"].as_str().unwrap_or_default(),
            "SUBMITTED",
            "case {name:?}: expected a submitted job, got {submitted}"
        );

        let properties = harness.await_terminal(&job_id, name).await;
        assert_eq!(
            properties["JobStatus"].as_str().unwrap_or_default(),
            case.expected_status,
            "case {name:?}: expected the job to end {}, got {properties}",
            case.expected_status
        );
        let expected_inputs =
            case.documents.len() as i64 + i64::from(case.binary_document.is_some());
        assert_eq!(
            count_at(&properties, "InputDocumentsCount"),
            expected_inputs,
            "case {name:?}: expected {expected_inputs} input documents, got {properties}"
        );
        assert_eq!(
            count_at(&properties, "TranslatedDocumentsCount"),
            case.expected_translated,
            "case {name:?}: expected {} translated documents, got {properties}",
            case.expected_translated
        );
        assert_eq!(
            count_at(&properties, "DocumentsWithErrorsCount"),
            case.expected_errors,
            "case {name:?}: expected {} documents with errors, got {properties}",
            case.expected_errors
        );
        // The configuration is reported back from the stored record, so a read
        // answers with what was submitted.
        assert_eq!(
            properties["SourceLanguageCode"].as_str().unwrap_or_default(),
            case.source,
            "case {name:?}: expected the submitted source language, got {properties}"
        );
        assert_eq!(
            properties["TargetLanguageCodes"],
            json!(case.targets),
            "case {name:?}: expected the submitted target languages, got {properties}"
        );
        assert_eq!(
            properties
                .pointer("/InputDataConfig/S3Uri")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            format!("s3://{BUCKET}/{}", case.input_prefix(index)),
            "case {name:?}: expected the submitted input location, got {properties}"
        );
        if case.expected_errors > 0 {
            let reported = message(&properties);
            assert!(
                reported.contains(case.binary_document.unwrap_or_default()),
                "case {name:?}: expected the message to name the document it could not \
                 process, got {reported:?}"
            );
        } else {
            assert!(
                properties.get("Message").is_none(),
                "case {name:?}: expected no message on a job without errors, got {properties}"
            );
        }

        // Every (document, target language) pair is in the output location,
        // holding exactly what the synchronous translation of that text and
        // pair answers — one transform serves both.
        for (file, text) in case.documents {
            for target in case.targets {
                let key = format!(
                    "{}/{ACCOUNT}-TranslateText-{job_id}/{target}.{file}",
                    case.output_prefix(index)
                );
                let written = harness.download(&key).await;
                let mut synchronous = json!({
                    "Text": text,
                    "SourceLanguageCode": case.source,
                    "TargetLanguageCode": target,
                });
                if let Some(terminology) = case.terminology {
                    synchronous["TerminologyNames"] = json!([terminology]);
                }
                let expected = harness
                    .call_ok("TranslateText", &synchronous, name)
                    .await["TranslatedText"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    written.as_deref(),
                    Some(expected.as_str()),
                    "case {name:?}: expected s3://{BUCKET}/{key} to hold the translation \
                     {expected:?}, got {written:?}"
                );
            }
        }
        if let Some(file) = case.binary_document {
            let key = format!(
                "{}/{ACCOUNT}-TranslateText-{job_id}/{}.{file}",
                case.output_prefix(index),
                case.targets.first().copied().unwrap_or_default()
            );
            assert_eq!(
                harness.download(&key).await,
                None,
                "case {name:?}: expected nothing written for a document the job could not \
                 process"
            );
        }
    }
}

/// A job whose input location holds nothing it can translate does not report a
/// success: it ends `FAILED`, carrying the reason, and writes nothing.
#[tokio::test]
async fn a_job_that_cannot_be_carried_out_ends_failed() {
    let harness = Harness::new();
    harness.create_bucket().await;

    // The rows differ in WHY the job has nothing to translate: an input folder
    // this bucket holds no object under, and a bucket that does not exist at
    // all.
    for (case, s3_uri) in [
        ("an empty input folder", format!("s3://{BUCKET}/empty/")),
        (
            "an input bucket that does not exist",
            "s3://translate-batch-absent/input/".to_string(),
        ),
    ] {
        let submitted = harness
            .call_ok(
                "StartTextTranslationJob",
                &json!({
                    "ClientToken": format!("token-{}", case.len()),
                    "DataAccessRoleArn": ROLE_ARN,
                    "SourceLanguageCode": "en",
                    "TargetLanguageCodes": ["fr"],
                    "InputDataConfig": { "S3Uri": s3_uri, "ContentType": CONTENT_TYPE },
                    "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/failed-output/") },
                }),
                case,
            )
            .await;
        let job_id = submitted["JobId"].as_str().unwrap_or_default().to_string();
        let properties = harness.await_terminal(&job_id, case).await;
        assert_eq!(
            properties["JobStatus"].as_str().unwrap_or_default(),
            "FAILED",
            "case {case:?}: expected the job to end FAILED, got {properties}"
        );
        assert_eq!(
            count_at(&properties, "TranslatedDocumentsCount"),
            0,
            "case {case:?}: expected no translated documents, got {properties}"
        );
        let reported = message(&properties);
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected the job to explain why it failed, got {properties}"
        );
    }
}

/// A job's status is the one the clock owes it, and its end time is reported
/// only once it has ended — two phases after it was submitted.
#[tokio::test]
async fn a_jobs_status_and_end_time_follow_the_clock() {
    let harness = Harness::new();
    harness.create_bucket().await;
    harness.upload("clock/input/one.txt", b"hello world").await;

    let case = "clock";
    let job_id = harness
        .call_ok(
            "StartTextTranslationJob",
            &json!({
                "ClientToken": "token-clock",
                "DataAccessRoleArn": ROLE_ARN,
                "SourceLanguageCode": "en",
                "TargetLanguageCodes": ["fr"],
                "InputDataConfig": {
                    "S3Uri": format!("s3://{BUCKET}/clock/input/"),
                    "ContentType": CONTENT_TYPE,
                },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/clock/output/") },
            }),
            case,
        )
        .await["JobId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Straight after the submission the job is still in a transient status and
    // has no end time: the span is never zero.
    let submitted = harness.describe(&job_id, case).await;
    let status = submitted["JobStatus"].as_str().unwrap_or_default();
    assert!(
        TRANSIENT_STATUSES.contains(&status),
        "case {case:?}: expected a transient status straight after the submission, got {status}"
    );
    assert!(
        submitted.get("EndTime").is_none(),
        "case {case:?}: expected no end time before the job ended, got {submitted}"
    );
    let submitted_time = submitted["SubmittedTime"].as_f64().unwrap_or_default();
    assert!(
        submitted_time > 0.0,
        "case {case:?}: expected a submitted time, got {submitted}"
    );

    let ended = harness.await_terminal(&job_id, case).await;
    assert_eq!(
        ended["JobStatus"].as_str().unwrap_or_default(),
        "COMPLETED",
        "case {case:?}: expected the job to complete, got {ended}"
    );
    let end_time = ended["EndTime"].as_f64().unwrap_or_default();
    assert!(
        end_time - submitted_time >= 2.0 * PHASE_SECONDS,
        "case {case:?}: expected the job to end at least {} seconds after it was submitted, \
         got {} ({ended})",
        2.0 * PHASE_SECONDS,
        end_time - submitted_time
    );

    // The read reports; it does not move the job on. A second read of an ended
    // job answers exactly as the first did.
    let again = harness.describe(&job_id, case).await;
    assert_eq!(
        again, ended,
        "case {case:?}: expected a second read to report the same job, got {again}"
    );
}

/// A stop request on a job that has not ended yet puts it into
/// `STOP_REQUESTED` and then `STOPPED`; a job that ended first keeps the status
/// it reached.
#[tokio::test]
async fn stopping_a_job_transitions_it_to_stopped() {
    let harness = Harness::new();
    harness.create_bucket().await;
    harness.upload("stop/input/one.txt", b"hello world").await;

    let start = |token: &str| {
        json!({
            "ClientToken": token,
            "DataAccessRoleArn": ROLE_ARN,
            "SourceLanguageCode": "en",
            "TargetLanguageCodes": ["fr"],
            "InputDataConfig": {
                "S3Uri": format!("s3://{BUCKET}/stop/input/"),
                "ContentType": CONTENT_TYPE,
            },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/stop/output/") },
        })
    };

    // Row 1: stopped while it is still running.
    let case = "stopped in flight";
    let (status, submitted) = harness
        .call("StartTextTranslationJob", &start("token-stop-running"))
        .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {submitted}");
    let job_id = submitted["JobId"].as_str().unwrap_or_default().to_string();
    let stopped = harness
        .call_ok(
            "StopTextTranslationJob",
            &json!({ "JobId": job_id }),
            case,
        )
        .await;
    assert_eq!(
        stopped["JobStatus"].as_str().unwrap_or_default(),
        "STOP_REQUESTED",
        "case {case:?}: expected the stop to be requested, got {stopped}"
    );
    assert_eq!(
        stopped["JobId"].as_str().unwrap_or_default(),
        job_id,
        "case {case:?}: expected the stop to report the job it stopped, got {stopped}"
    );
    // A repeated stop does not move the instant the first one made STOPPED due.
    let repeated = harness
        .call_ok(
            "StopTextTranslationJob",
            &json!({ "JobId": job_id }),
            case,
        )
        .await;
    assert!(
        ["STOP_REQUESTED", "STOPPED"]
            .contains(&repeated["JobStatus"].as_str().unwrap_or_default()),
        "case {case:?}: expected a repeated stop to report the stopping job, got {repeated}"
    );
    let ended = harness.await_terminal(&job_id, case).await;
    assert_eq!(
        ended["JobStatus"].as_str().unwrap_or_default(),
        "STOPPED",
        "case {case:?}: expected the job to reach STOPPED, got {ended}"
    );
    assert!(
        ended["EndTime"].as_f64().unwrap_or_default() > 0.0,
        "case {case:?}: expected a stopped job to report when it ended, got {ended}"
    );

    // Row 2: stopped after it ended, which keeps the status it reached.
    let case = "stopped after it completed";
    let (status, submitted) = harness
        .call("StartTextTranslationJob", &start("token-stop-completed"))
        .await;
    assert_eq!(status, 200, "case {case:?}: expected 200, got {submitted}");
    let job_id = submitted["JobId"].as_str().unwrap_or_default().to_string();
    let completed = harness.await_terminal(&job_id, case).await;
    assert_eq!(
        completed["JobStatus"].as_str().unwrap_or_default(),
        "COMPLETED",
        "case {case:?}: expected the job to complete first, got {completed}"
    );
    let stopped = harness
        .call_ok(
            "StopTextTranslationJob",
            &json!({ "JobId": job_id }),
            case,
        )
        .await;
    assert_eq!(
        stopped["JobStatus"].as_str().unwrap_or_default(),
        "COMPLETED",
        "case {case:?}: expected a completed job to keep its status, got {stopped}"
    );
    assert_eq!(
        harness.describe(&job_id, case).await["JobStatus"]
            .as_str()
            .unwrap_or_default(),
        "COMPLETED",
        "case {case:?}: expected the stop to leave the completed job as it was"
    );
}

/// `ClientToken` identifies the submission: submitting one token twice is one
/// job, and two tokens are two jobs.
#[tokio::test]
async fn one_client_token_starts_one_job() {
    let harness = Harness::new();
    harness.create_bucket().await;
    harness
        .upload("idempotent/input/one.txt", b"hello world")
        .await;

    let body = |token: &str| {
        json!({
            "ClientToken": token,
            "DataAccessRoleArn": ROLE_ARN,
            "SourceLanguageCode": "en",
            "TargetLanguageCodes": ["fr"],
            "InputDataConfig": {
                "S3Uri": format!("s3://{BUCKET}/idempotent/input/"),
                "ContentType": CONTENT_TYPE,
            },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/idempotent/output/") },
        })
    };

    let case = "one token twice";
    let first = harness
        .call_ok("StartTextTranslationJob", &body("token-once"), case)
        .await;
    let second = harness
        .call_ok("StartTextTranslationJob", &body("token-once"), case)
        .await;
    assert_eq!(
        first["JobId"], second["JobId"],
        "case {case:?}: expected one job for one client token, got {first} and {second}"
    );

    let case = "a second token";
    let other = harness
        .call_ok("StartTextTranslationJob", &body("token-twice"), case)
        .await;
    assert_ne!(
        first["JobId"], other["JobId"],
        "case {case:?}: expected a second token to start a second job, got {other}"
    );
}

/// The term matching the batch plane and the synchronous operations share is
/// bounded in the work it does, not only in the answer it produces: a
/// terminology whose terms all share a long prefix with the text does not make
/// one request compare without limit
/// (`state::MAX_MATCH_COMPARISON_BYTES`).
#[tokio::test]
async fn a_terminology_of_long_common_prefix_terms_stays_bounded() {
    let harness = Harness::new();

    // 256 terms (the most one terminology holds), each 2000 characters of the
    // same prefix diverging only at its last character, so every attempt the
    // scan makes against them is as long as the table allows.
    let prefix = "a".repeat(1999);
    let mut file = String::from("en,fr\n");
    for index in 0..256u32 {
        // The terms differ only in their last character, cycling the letters
        // of the alphabet, so none of them is a prefix of another.
        let suffix = char::from_digit(index % 36, 36).unwrap_or('z');
        file.push_str(&format!("{prefix}{suffix},bonjour{index}\n"));
    }
    harness
        .call_ok(
            "ImportTerminology",
            &json!({
                "Name": "long-terms",
                "MergeStrategy": "OVERWRITE",
                "TerminologyData": {
                    "File": aws_smithy_types::base64::encode(file.as_bytes()),
                    "Format": "CSV",
                },
            }),
            "long terms fixture",
        )
        .await;

    // Text made of word-start positions each followed by that same prefix, so
    // every position attempts every term over its whole length.
    let text = format!("{} ", &prefix[..999]).repeat(9);
    let case = "a terminology of long common-prefix terms";
    let started = Instant::now();
    let (status, parsed) = harness
        .call(
            "TranslateText",
            &json!({
                "Text": text,
                "SourceLanguageCode": "en",
                "TargetLanguageCode": "fr",
                "TerminologyNames": ["long-terms"],
            }),
        )
        .await;
    // Either the translation or the service's own size refusal — never a hang:
    // the comparison bound stops the scan the way the output ceiling does.
    assert!(
        status == 200 || status == 400,
        "case {case:?}: expected the request to be answered, got {status} {parsed}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "case {case:?}: expected the bounded scan to answer promptly, took {:?}",
        started.elapsed()
    );
}

/// One row of the refusal table: the submission, and the status and error class
/// it is refused with.
struct RefusalCase {
    name: &'static str,
    operation: &'static str,
    /// What the submission overrides on the valid body below.
    patch: fn(&mut Value),
    status: u16,
    code: &'static str,
}

/// The refusals: a pair this service does not translate, a member it cannot act
/// on, and a resource this account does not hold.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        name: "a target language Amazon Translate does not translate into",
        operation: "StartTextTranslationJob",
        patch: |body| body["TargetLanguageCodes"] = json!(["xq"]),
        status: 400,
        code: "UnsupportedLanguagePairException",
    },
    RefusalCase {
        name: "a source language Amazon Translate does not translate from",
        operation: "StartTextTranslationJob",
        patch: |body| body["SourceLanguageCode"] = json!("xq"),
        status: 400,
        code: "UnsupportedLanguagePairException",
    },
    RefusalCase {
        name: "a target language that is the source language",
        operation: "StartTextTranslationJob",
        patch: |body| body["TargetLanguageCodes"] = json!(["fr", "en"]),
        status: 400,
        code: "UnsupportedLanguagePairException",
    },
    RefusalCase {
        name: "more target languages than a job translates into",
        operation: "StartTextTranslationJob",
        patch: |body| {
            body["TargetLanguageCodes"] = json!([
                "fr", "de", "es", "it", "ja", "ko", "pt", "nl", "sv", "pl", "da"
            ]);
        },
        status: 400,
        code: "InvalidParameterValueException",
    },
    RefusalCase {
        name: "a document format this batch plane does not read",
        operation: "StartTextTranslationJob",
        patch: |body| body["InputDataConfig"]["ContentType"] = json!("text/html"),
        status: 400,
        code: "InvalidParameterValueException",
    },
    RefusalCase {
        name: "more custom terminologies than a job applies",
        operation: "StartTextTranslationJob",
        patch: |body| body["TerminologyNames"] = json!(["first", "second"]),
        status: 400,
        code: "InvalidParameterValueException",
    },
    RefusalCase {
        name: "a custom terminology this account does not hold",
        operation: "StartTextTranslationJob",
        patch: |body| body["TerminologyNames"] = json!(["absent-terminology"]),
        status: 404,
        code: "ResourceNotFoundException",
    },
    RefusalCase {
        name: "a parallel data resource this account does not hold",
        operation: "StartTextTranslationJob",
        patch: |body| body["ParallelDataNames"] = json!(["absent-parallel-data"]),
        status: 404,
        code: "ResourceNotFoundException",
    },
    RefusalCase {
        name: "describing a job id no submission started",
        operation: "DescribeTextTranslationJob",
        patch: |body| *body = json!({ "JobId": "0123456789abcdef0123456789abcdef" }),
        status: 404,
        code: "ResourceNotFoundException",
    },
    RefusalCase {
        name: "stopping a job id no submission started",
        operation: "StopTextTranslationJob",
        patch: |body| *body = json!({ "JobId": "0123456789abcdef0123456789abcdef" }),
        status: 404,
        code: "ResourceNotFoundException",
    },
];

/// Each refusal is answered with the modelled error class and status, and never
/// with a 501 or a success.
#[tokio::test]
async fn submissions_the_service_cannot_carry_out_are_refused() {
    let harness = Harness::new();
    harness.create_bucket().await;

    for case in REFUSAL_CASES {
        let mut body = json!({
            "ClientToken": "token-refused",
            "DataAccessRoleArn": ROLE_ARN,
            "SourceLanguageCode": "en",
            "TargetLanguageCodes": ["fr"],
            "InputDataConfig": {
                "S3Uri": format!("s3://{BUCKET}/refused/input/"),
                "ContentType": CONTENT_TYPE,
            },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/refused/output/") },
        });
        (case.patch)(&mut body);
        let name = case.name;
        let (status, parsed) = harness.call(case.operation, &body).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected HTTP {}, got {status} {parsed}",
            case.status
        );
        assert_eq!(
            error_code(&parsed),
            case.code,
            "case {name:?}: expected {}, got {parsed}",
            case.code
        );
        assert!(
            !message(&parsed).is_empty(),
            "case {name:?}: expected the refusal to say why, got {parsed}"
        );
    }
}

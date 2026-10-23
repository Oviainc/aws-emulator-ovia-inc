// SPDX-License-Identifier: CC0-1.0

//! The transcription-job lifecycle, end to end over the wire: `StartTranscriptionJob`
//! -> `GetTranscriptionJob` -> `ListTranscriptionJobs` -> `DeleteTranscriptionJob`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Long enough for every clock-owed transition of a job started during the test to
/// have fallen due: a queued job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans
/// of `crate::state::JOB_TRANSITION_DELAY` (500 ms each).
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// One emulator: the service mounted on a single shared engine behind the router.
///
/// `pub(crate)`, like [`Reply`] and [`dispatch`] below it: the vocabulary suite in this
/// same test crate drives its requests through this harness instead of standing up a
/// second one. The router is behind an `Arc` so a test that needs several concurrent
/// callers can hand each one a clone.
pub(crate) struct Transcribe {
    pub(crate) router: Arc<MultiServiceRouter>,
}

/// One answer: the HTTP status, the `Content-Type` it was sent with and the parsed
/// JSON body.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) body: Value,
}

impl Transcribe {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_transcribe_transcribe::EmulatedService::new(engine),
        );
        Self {
            router: Arc::new(router),
        }
    }

    /// Call one operation in the service's own wire envelope (awsJson1_1: the
    /// operation in `X-Amz-Target`, the input as the JSON body).
    pub(crate) async fn call(&self, operation: &str, input: Value) -> Reply {
        self.call_in_region(operation, input, None).await
    }

    /// The same call, signed for `region` when one is given, so the handler sees a
    /// SigV4 credential scope.
    pub(crate) async fn call_in_region(
        &self,
        operation: &str,
        input: Value,
        region: Option<&str>,
    ) -> Reply {
        dispatch(&self.router, operation, input, region).await
    }
}

/// One awsJson1_1 call through `router`, signed for `region` when one is given.
///
/// A free function rather than only a method, so a test driving several concurrent
/// callers can hand each one a cloned `Arc<MultiServiceRouter>`.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    operation: &str,
    input: Value,
    region: Option<&str>,
) -> Reply {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Transcribe.{operation}"))
        .header("content-type", "application/x-amz-json-1.1");
    if let Some(region) = region {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/transcribe/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        );
    }
    let request = builder
        .body(bytes::Bytes::from(
            serde_json::to_vec(&input).expect("serializable input"),
        ))
        .expect("valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({e}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    Reply {
        status,
        content_type,
        body,
    }
}

impl Reply {
    /// The body of a successful call.
    pub(crate) fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert a successful call that answers an explicit status.
    ///
    /// Every operation of this awsJson1_1 service answers 200 on success (the model's
    /// `@http` `code` is inert on an RPC protocol), so this is 200 everywhere; it
    /// stays as its own assertion so a row that means to pin a status says so.
    pub(crate) fn ok_status(&self, case: &str, expected: u16) {
        assert_eq!(
            self.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {:?}",
            self.status, self.body
        );
    }

    /// Assert this answer is the modelled refusal `code` with HTTP `status`.
    pub(crate) fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            self.status, self.body
        );
        assert_eq!(
            self.error_code(),
            code,
            "case {case:?}: expected the modelled {code}, got {:?}",
            self.body
        );
    }

    /// One string member of a successful body.
    pub(crate) fn member(&self, case: &str, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a string {name} in the answer, got {:?}",
                    self.body
                )
            })
            .to_string()
    }

    /// One timestamp member of a successful body, as the epoch seconds awsJson1_1
    /// renders a `DateTime` as.
    pub(crate) fn timestamp(&self, case: &str, name: &str) -> f64 {
        self.body
            .get(name)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a numeric {name} in the answer, got {:?}",
                    self.body
                )
            })
    }

    /// The awsJson error code (`__type`), or `""` when the body carries none.
    pub(crate) fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .map_or("", |code| code.rsplit(['#', ':']).next().unwrap_or(""))
    }

    /// The error body's message, whichever casing this envelope spells the member in
    /// (`Message` on the modelled shapes, `message` on the framework's synthetic
    /// ones).
    pub(crate) fn error_message(&self, case: &str) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a message in the error body, got {:?}",
                    self.body
                )
            })
            .to_string()
    }
}

/// The members `StartTranscriptionJob` accepts that the `TranscriptionJob` shape also
/// carries, so a stored job must read them back unchanged.
const ECHOED_MEMBERS: &[&str] = &[
    "TranscriptionJobName",
    "LanguageCode",
    "MediaSampleRateHertz",
    "MediaFormat",
    "Media",
    "Settings",
    "ModelSettings",
    "JobExecutionSettings",
    "ContentRedaction",
    "IdentifyLanguage",
    "IdentifyMultipleLanguages",
    "LanguageOptions",
    "Tags",
];

/// The class of job a `StartTranscriptionJob` request can describe, one row per axis
/// the service's rules generalise over: the media URI scheme, the media format, the
/// sample rate (each published bound of `MediaSampleRateHertz`), how the language is
/// specified, where the output goes, how many tags and subtitle formats are asked for,
/// and whether execution is deferred.
fn start_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal: s3 media, no output bucket",
            json!({
                "TranscriptionJobName": "minimal-job",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                "LanguageCode": "en-US"
            }),
        ),
        (
            "customer bucket, output key naming a file, two tags",
            json!({
                "TranscriptionJobName": "tagged.job-1",
                "Media": {"MediaFileUri": "s3://example-media/meeting.mp3"},
                "LanguageCode": "de-DE",
                "MediaFormat": "mp3",
                "OutputBucketName": "example-output",
                "OutputKey": "transcripts/meeting.json",
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 4},
                "Tags": [{"Key": "team", "Value": "research"}, {"Key": "cost", "Value": "7"}]
            }),
        ),
        (
            "https media, mp4, lowest published sample rate",
            json!({
                "TranscriptionJobName": "https_job",
                "Media": {"MediaFileUri": "https://example.invalid/media/clip.mp4"},
                "LanguageCode": "ja-JP",
                "MediaFormat": "mp4",
                "MediaSampleRateHertz": 8000
            }),
        ),
        (
            "highest published sample rate, subtitles in two formats",
            json!({
                "TranscriptionJobName": "subtitled-job",
                "Media": {"MediaFileUri": "s3://example-media/lecture.wav"},
                "LanguageCode": "es-US",
                "MediaFormat": "wav",
                "MediaSampleRateHertz": 48000,
                "OutputBucketName": "example-output",
                "Subtitles": {"Formats": ["vtt", "srt"], "OutputStartIndex": 1}
            }),
        ),
        (
            "automatic language identification instead of a language code",
            json!({
                "TranscriptionJobName": "identify-job",
                "Media": {"MediaFileUri": "s3://example-media/call.ogg"},
                "MediaFormat": "ogg",
                "IdentifyLanguage": true,
                "LanguageOptions": ["en-US", "fr-FR"]
            }),
        ),
        (
            "deferred execution, so the job is queued first",
            json!({
                "TranscriptionJobName": "queued-job",
                "Media": {"MediaFileUri": "s3://example-media/batch.amr"},
                "LanguageCode": "en-GB",
                "MediaFormat": "amr",
                "JobExecutionSettings": {
                    "AllowDeferredExecution": true,
                    "DataAccessRoleArn": "arn:aws:iam::000000000000:role/transcribe-access"
                }
            }),
        ),
    ]
}

#[tokio::test]
async fn a_started_job_reads_back_exactly_as_it_was_started() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        let started = transcribe
            .call("StartTranscriptionJob", request.clone())
            .await;
        let job = started.ok(case)["TranscriptionJob"].clone();

        let queued = request
            .get("JobExecutionSettings")
            .and_then(|settings| settings.get("AllowDeferredExecution"))
            == Some(&Value::Bool(true));
        let expected_status = if queued { "QUEUED" } else { "IN_PROGRESS" };
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!(expected_status),
            "case {case:?}: a job just started must be {expected_status}, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert!(
            job["CreationTime"].is_number(),
            "case {case:?}: CreationTime must be set, got {:?}",
            job["CreationTime"]
        );
        assert!(
            job["Transcript"].is_null(),
            "case {case:?}: a job that has not completed has no transcript, got {:?}",
            job["Transcript"]
        );

        let name = request["TranscriptionJobName"].clone();
        let fetched = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": name.clone()}),
            )
            .await;
        let fetched = fetched.ok(case)["TranscriptionJob"].clone();

        for member in ECHOED_MEMBERS {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    fetched.get(*member),
                    Some(sent),
                    "case {case:?}: GetTranscriptionJob must return the stored {member}, got {:?}",
                    fetched.get(*member)
                );
            }
        }
        assert_eq!(
            fetched["CreationTime"], job["CreationTime"],
            "case {case:?}: CreationTime must be stable across reads, got {:?} then {:?}",
            job["CreationTime"], fetched["CreationTime"]
        );

        // `Subtitles` is accepted as `Subtitles` and returned as `SubtitlesOutput`,
        // so it is checked member by member rather than by the echo table.
        if let Some(subtitles) = request.get("Subtitles") {
            assert_eq!(
                fetched["Subtitles"]["Formats"], subtitles["Formats"],
                "case {case:?}: the requested subtitle formats must be returned, got {:?}",
                fetched["Subtitles"]["Formats"]
            );
            assert_eq!(
                fetched["Subtitles"]["OutputStartIndex"],
                subtitles
                    .get("OutputStartIndex")
                    .cloned()
                    .unwrap_or(json!(0)),
                "case {case:?}: OutputStartIndex must be the requested value (default 0), got {:?}",
                fetched["Subtitles"]["OutputStartIndex"]
            );
        }
    }
}

#[tokio::test]
async fn a_job_completes_once_its_processing_span_has_elapsed() {
    let transcribe = Transcribe::new();

    // Two rows: a job that starts processing immediately, and one whose execution
    // the request deferred, so it is queued before it starts.
    let cases: &[(&str, bool)] = &[("immediate", false), ("deferred", true)];
    for (case, deferred) in cases {
        let mut request = json!({
            "TranscriptionJobName": format!("lifecycle-{case}"),
            "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
            "LanguageCode": "en-US",
            "OutputBucketName": "example-output"
        });
        if *deferred {
            request["JobExecutionSettings"] = json!({
                "AllowDeferredExecution": true,
                "DataAccessRoleArn": "arn:aws:iam::000000000000:role/transcribe-access"
            });
        }

        let started = transcribe.call("StartTranscriptionJob", request).await;
        let job = started.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!(if *deferred { "QUEUED" } else { "IN_PROGRESS" }),
            "case {case:?}: unexpected status just after the start call, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert_eq!(
            job["StartTime"].is_null(),
            *deferred,
            "case {case:?}: a queued job has no StartTime and a processing one does, got {:?}",
            job["StartTime"]
        );
        assert!(
            job["CompletionTime"].is_null(),
            "case {case:?}: a job that has not completed has no CompletionTime, got {:?}",
            job["CompletionTime"]
        );
    }

    // Nothing sleeps inside the service: the transition is due at an instant stored
    // on the record, so the test simply waits past it.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (case, _) in cases {
        let fetched = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": format!("lifecycle-{case}")}),
            )
            .await;
        let job = fetched.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the job must read back COMPLETED once its span elapsed, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert_eq!(
            job["Transcript"]["TranscriptFileUri"],
            json!(format!("s3://example-output/lifecycle-{case}.json")),
            "case {case:?}: a completed job carries its transcript URI, got {:?}",
            job["Transcript"]
        );
        let start = job["StartTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a completed job has a StartTime, got {:?}",
                job["StartTime"]
            )
        });
        let completion = job["CompletionTime"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a completed job has a CompletionTime, got {:?}",
                job["CompletionTime"]
            )
        });
        assert!(
            completion >= start,
            "case {case:?}: CompletionTime must not precede StartTime, got {completion} < {start}"
        );

        // A completed job is terminal: a later read reports the same instants.
        let again = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": format!("lifecycle-{case}")}),
            )
            .await;
        assert_eq!(
            again.ok(case)["TranscriptionJob"]["CompletionTime"],
            job["CompletionTime"],
            "case {case:?}: CompletionTime must be stable across reads, got {:?}",
            again.body["TranscriptionJob"]["CompletionTime"]
        );
    }
}

#[tokio::test]
async fn the_output_location_follows_the_documented_output_key_rule() {
    let transcribe = Transcribe::new();
    let account = engine::emulator_account_id();

    // Rows 2-5 are the four examples in the model doc for
    // `StartTranscriptionJobRequest$OutputKey`; row 1 is its stated default ("the
    // same as the name you specified for your transcription job") and row 6 is the
    // service-managed bucket of `Transcript$TranscriptFileUri`.
    let cases = vec![
        OutputCase {
            name: "job-a",
            bucket: Some("doc-example-bucket"),
            key: None,
            uri: "s3://doc-example-bucket/job-a.json".to_string(),
            location: "CUSTOMER_BUCKET",
        },
        OutputCase {
            name: "job-b",
            bucket: Some("doc-example-bucket"),
            key: Some("my-transcript.json"),
            uri: "s3://doc-example-bucket/my-transcript.json".to_string(),
            location: "CUSTOMER_BUCKET",
        },
        OutputCase {
            name: "my-first-transcription",
            bucket: Some("doc-example-bucket"),
            key: Some("my-transcript"),
            uri: "s3://doc-example-bucket/my-transcript/my-first-transcription.json".to_string(),
            location: "CUSTOMER_BUCKET",
        },
        OutputCase {
            name: "job-c",
            bucket: Some("doc-example-bucket"),
            key: Some("test-files/my-transcript.json"),
            uri: "s3://doc-example-bucket/test-files/my-transcript.json".to_string(),
            location: "CUSTOMER_BUCKET",
        },
        OutputCase {
            name: "job-d",
            bucket: Some("doc-example-bucket"),
            key: Some("test-files/my-transcript"),
            uri: "s3://doc-example-bucket/test-files/my-transcript/job-d.json".to_string(),
            location: "CUSTOMER_BUCKET",
        },
        OutputCase {
            name: "service-managed-job",
            bucket: None,
            key: None,
            uri: format!(
                "https://s3.us-east-1.amazonaws.com/aws-transcribe-us-east-1-prod/{account}/service-managed-job.json"
            ),
            location: "SERVICE_BUCKET",
        },
    ];

    for case in &cases {
        let mut request = json!({
            "TranscriptionJobName": case.name,
            "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
            "LanguageCode": "en-US"
        });
        if let Some(bucket) = case.bucket {
            request["OutputBucketName"] = json!(bucket);
        }
        if let Some(key) = case.key {
            request["OutputKey"] = json!(key);
        }
        transcribe
            .call("StartTranscriptionJob", request)
            .await
            .ok(case.name);
    }

    std::thread::sleep(PAST_EVERY_TRANSITION);

    for case in &cases {
        let name = case.name;
        let fetched = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        let job = fetched.ok(name)["TranscriptionJob"].clone();
        assert_eq!(
            job["Transcript"]["TranscriptFileUri"],
            json!(case.uri),
            "case {name:?}: expected transcript at {}, got {:?}",
            case.uri,
            job["Transcript"]["TranscriptFileUri"]
        );
    }

    // `OutputLocationType` is reported by the list projection, and follows from
    // whether the request named an output bucket.
    let listed = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    let summaries = listed.ok("list every job")["TranscriptionJobSummaries"]
        .as_array()
        .expect("TranscriptionJobSummaries must be a list")
        .clone();
    for case in &cases {
        let name = case.name;
        let summary = summaries
            .iter()
            .find(|summary| summary["TranscriptionJobName"] == json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: the job must appear in the listing"));
        assert_eq!(
            summary["OutputLocationType"],
            json!(case.location),
            "case {name:?}: expected OutputLocationType {}, got {:?}",
            case.location,
            summary["OutputLocationType"]
        );
    }
}

/// One row of [`the_output_location_follows_the_documented_output_key_rule`]: what the
/// request asked for, and where the transcript must therefore be.
struct OutputCase {
    name: &'static str,
    bucket: Option<&'static str>,
    key: Option<&'static str>,
    uri: String,
    location: &'static str,
}

#[tokio::test]
async fn re_using_a_job_name_is_a_conflict() {
    let transcribe = Transcribe::new();
    let request = json!({
        "TranscriptionJobName": "demo-job",
        "Media": {"MediaFileUri": "s3://example-media/audio.mp3"},
        "LanguageCode": "en-US",
        "MediaFormat": "mp3",
        "OutputBucketName": "example-output"
    });

    transcribe
        .call("StartTranscriptionJob", request.clone())
        .await
        .ok("first start");

    // The same name again, and the same name with a different body: the name alone
    // decides.
    let mut different = request.clone();
    different["LanguageCode"] = json!("fr-FR");
    for (case, retry) in [("identical request", request), ("different body", different)] {
        let conflict = transcribe.call("StartTranscriptionJob", retry).await;
        assert_eq!(
            conflict.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            conflict.body
        );
    }
}

#[tokio::test]
async fn a_request_the_service_cannot_serve_is_refused_and_stores_nothing() {
    let transcribe = Transcribe::new();

    // Each row is refused with a 4xx and a modelled error code: the first three by
    // this service's own rules, the rest by the model's constraint traits at the
    // request boundary.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "Media without a MediaFileUri",
            json!({
                "TranscriptionJobName": "no-media",
                "Media": {},
                "LanguageCode": "en-US"
            }),
            400,
            "BadRequestException",
        ),
        (
            "no language specification at all",
            json!({
                "TranscriptionJobName": "no-language",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"}
            }),
            400,
            "BadRequestException",
        ),
        (
            "both a language code and language identification",
            json!({
                "TranscriptionJobName": "two-languages",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                "LanguageCode": "en-US",
                "IdentifyLanguage": true
            }),
            400,
            "BadRequestException",
        ),
        (
            "job name outside the modelled pattern",
            json!({
                "TranscriptionJobName": "has spaces",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                "LanguageCode": "en-US"
            }),
            400,
            "ValidationException",
        ),
        (
            "media URI outside the modelled pattern",
            json!({
                "TranscriptionJobName": "bad-uri",
                "Media": {"MediaFileUri": "ftp://example-media/audio.flac"},
                "LanguageCode": "en-US"
            }),
            400,
            "ValidationException",
        ),
        (
            "sample rate below the modelled range",
            json!({
                "TranscriptionJobName": "slow-sample",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                "LanguageCode": "en-US",
                "MediaSampleRateHertz": 7999
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in &cases {
        let refused = transcribe.call("StartTranscriptionJob", request.clone()).await;
        assert_eq!(
            refused.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {:?}",
            refused.body
        );
    }

    let listed = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        listed.ok("list after refusals")["TranscriptionJobSummaries"],
        json!([]),
        "a refused StartTranscriptionJob must store nothing, got {:?}",
        listed.body
    );

    // A NextToken this service did not hand out is refused, rather than silently
    // listing from the top or answering an empty page: neither an unparseable
    // cursor nor a well-formed one naming a sequence that cannot exist (creation
    // sequences start at 1) is accepted.
    for token in ["not-a-cursor", "0|demo-job", "|", "-1|demo-job"] {
        let bad_token = transcribe
            .call("ListTranscriptionJobs", json!({"NextToken": token}))
            .await;
        assert_eq!(
            bad_token.status, 400,
            "case {token:?}: a NextToken this service did not write must be refused \
             with HTTP 400, got {} with body {:?}",
            bad_token.status, bad_token.body
        );
        assert_eq!(
            bad_token.error_code(),
            "BadRequestException",
            "case {token:?}: expected BadRequestException, got {:?}",
            bad_token.body
        );
    }
}

#[tokio::test]
async fn the_service_managed_transcript_uri_is_in_the_signed_region() {
    let transcribe = Transcribe::new();
    let account = engine::emulator_account_id();

    // A region the credential scope names is used; a scope value that is not
    // shaped like a Region identifier is not reflected into the URI, and the
    // emulator's default region is used instead.
    let cases: &[(&str, &str)] = &[
        ("eu-west-1", "eu-west-1"),
        ("ap-southeast-3", "ap-southeast-3"),
        ("us-gov-west-1", "us-gov-west-1"),
        ("us-east-1.attacker.invalid", "us-east-1"),
        ("", "us-east-1"),
        ("UPPER-CASE-1", "us-east-1"),
    ];

    for (index, (scope_region, _)) in cases.iter().enumerate() {
        transcribe
            .call_in_region(
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": format!("region-job-{index}"),
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                    "LanguageCode": "en-US"
                }),
                Some(scope_region),
            )
            .await
            .ok(scope_region);
    }

    // One wait for every row, not one per row.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (index, (scope_region, expected_region)) in cases.iter().enumerate() {
        let name = format!("region-job-{index}");
        let fetched = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": name.clone()}),
            )
            .await;
        let job = fetched.ok(scope_region)["TranscriptionJob"].clone();
        assert_eq!(
            job["Transcript"]["TranscriptFileUri"],
            json!(format!(
                "https://s3.{expected_region}.amazonaws.com/aws-transcribe-{expected_region}-prod/{account}/{name}.json"
            )),
            "case {scope_region:?}: expected a transcript URI in {expected_region}, got {:?}",
            job["Transcript"]["TranscriptFileUri"]
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_by_the_account_job_quota() {
    let transcribe = Transcribe::new();

    let start = |name: String| {
        let request = json!({
            "TranscriptionJobName": name,
            "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
            "LanguageCode": "en-US"
        });
        transcribe.call("StartTranscriptionJob", request)
    };

    // Filling the quota succeeds job for job.
    for index in 0..100 {
        let name = format!("quota-{index}");
        let reply = start(name.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: a job within the quota must be accepted, got {} with body {:?}",
            reply.status, reply.body
        );
    }

    // The next one is refused with the error this operation models for a quota,
    // and nothing is stored for it.
    let over = start("quota-100".to_string()).await;
    assert_eq!(
        over.status, 400,
        "a start beyond the quota must be HTTP 400, got {} with body {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error_code(),
        "LimitExceededException",
        "a start beyond the quota must be a LimitExceededException, got {:?}",
        over.body
    );
    let missing = transcribe
        .call(
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": "quota-100"}),
        )
        .await;
    assert_eq!(
        missing.status, 400,
        "the refused job must not have been stored, got {} with body {:?}",
        missing.status, missing.body
    );

    // Deleting a job makes room again: the quota bounds what is STORED, it is not
    // a one-way door. The wait is what makes the job deletable: a job the service is
    // still working on is in a non-terminal state, which `DeleteTranscriptionJob`
    // refuses.
    std::thread::sleep(PAST_EVERY_TRANSITION);
    transcribe
        .call(
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": "quota-0"}),
        )
        .await
        .ok_status("delete a completed job to free quota", 200);
    let after_delete = start("quota-100".to_string()).await;
    assert_eq!(
        after_delete.status, 200,
        "a start after a delete freed quota must be accepted, got {} with body {:?}",
        after_delete.status, after_delete.body
    );
}

#[tokio::test]
async fn an_unknown_job_is_not_found_and_delete_removes_a_known_one() {
    let transcribe = Transcribe::new();

    let missing = transcribe
        .call(
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": "never-started"}),
        )
        .await;
    assert_eq!(
        missing.status, 400,
        "an unknown job must be HTTP 400, got {} with body {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code(),
        "BadRequestException",
        "an unknown job must be a BadRequestException, got {:?}",
        missing.body
    );

    transcribe
        .call(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "demo-job",
                "Media": {"MediaFileUri": "s3://example-media/audio.mp3"},
                "LanguageCode": "en-US",
                "MediaFormat": "mp3"
            }),
        )
        .await
        .ok("start the job to delete");

    // A job the service is still working on is in a non-terminal state, and a delete
    // of such a resource is refused (`BadRequestException`: "This can occur when the
    // entity you're trying to delete doesn't exist or if it's in a non-terminal state
    // (such as IN PROGRESS)").
    let while_in_progress = transcribe
        .call(
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": "demo-job"}),
        )
        .await;
    assert_eq!(
        while_in_progress.status, 400,
        "deleting a job that is still being processed must be HTTP 400, got {} with \
         body {:?}",
        while_in_progress.status, while_in_progress.body
    );
    assert_eq!(
        while_in_progress.error_code(),
        "BadRequestException",
        "deleting a job that is still being processed must be a BadRequestException, \
         got {:?}",
        while_in_progress.body
    );

    // Once every clock-owed transition has fallen due the job is COMPLETED — a
    // terminal state — and the same delete goes through.
    std::thread::sleep(PAST_EVERY_TRANSITION);
    let deleted = transcribe
        .call(
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": "demo-job"}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTranscriptionJob answers this awsJson1_1 service's success status 200, \
         got {} with body {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        json!({}),
        "DeleteTranscriptionJob answers no members, got {:?}",
        deleted.body
    );

    let after_delete = transcribe
        .call(
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": "demo-job"}),
        )
        .await;
    assert_eq!(
        after_delete.status, 400,
        "a deleted job must be HTTP 400, got {} with body {:?}",
        after_delete.status, after_delete.body
    );

    let listed = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        listed.ok("list after the delete")["TranscriptionJobSummaries"],
        json!([]),
        "a deleted job must be gone from the listing, got {:?}",
        listed.body
    );

    // Deleting what is not there is refused with the error this operation models
    // for an entity that does not exist.
    let again = transcribe
        .call(
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": "demo-job"}),
        )
        .await;
    assert_eq!(
        again.status, 400,
        "deleting an unknown job must be HTTP 400, got {} with body {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "BadRequestException",
        "deleting an unknown job must be a BadRequestException, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn listing_is_newest_first_and_honours_every_filter_and_page_size() {
    let transcribe = Transcribe::new();

    // Seven jobs, in a known creation order, with names that differ in case so the
    // case-insensitive `JobNameContains` search has something to find.
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let name = if index % 2 == 0 {
            format!("alpha-{index}")
        } else {
            format!("BETA-{index}")
        };
        transcribe
            .call(
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": name.clone(),
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                    "LanguageCode": "en-US"
                }),
            )
            .await
            .ok(&name);
        created.push(name);
    }

    let names_of = |summaries: &Value| -> Vec<String> {
        summaries
            .as_array()
            .expect("TranscriptionJobSummaries must be a list")
            .iter()
            .map(|summary| summary["TranscriptionJobName"].as_str().unwrap().to_string())
            .collect()
    };

    // Newest first: the reverse of the creation order.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();
    let all = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        names_of(&all.ok("unpaged listing")["TranscriptionJobSummaries"]),
        newest_first,
        "the listing must be newest first, got {:?}",
        all.body["TranscriptionJobSummaries"]
    );

    // Every page size pages through exactly the same sequence, with no job
    // repeated and none missed.
    for page_size in [1_u64, 2, 3, 7] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = transcribe.call("ListTranscriptionJobs", request).await;
            let body = page.ok(&case);
            let page_names = names_of(&body["TranscriptionJobSummaries"]);
            assert!(
                page_names.len() as u64 <= page_size,
                "case {case:?}: a page must hold at most {page_size} jobs, got {:?}",
                page_names
            );
            seen.extend(page_names);
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= newest_first.len(),
                "case {case:?}: paging must terminate, got {:?}",
                seen
            );
        }
        assert_eq!(
            seen, newest_first,
            "case {case:?}: paging must walk every job once, newest first, got {:?}",
            seen
        );
    }

    // `JobNameContains` is a case-insensitive substring search.
    let contains_cases: &[(&str, Vec<String>)] = &[
        (
            "beta",
            created
                .iter()
                .rev()
                .filter(|name| name.starts_with("BETA"))
                .cloned()
                .collect(),
        ),
        (
            "BETA",
            created
                .iter()
                .rev()
                .filter(|name| name.starts_with("BETA"))
                .cloned()
                .collect(),
        ),
        ("alpha-4", vec!["alpha-4".to_string()]),
        ("zeta", Vec::new()),
    ];
    for (needle, expected) in contains_cases {
        let filtered = transcribe
            .call(
                "ListTranscriptionJobs",
                json!({"JobNameContains": needle, "MaxResults": 100}),
            )
            .await;
        assert_eq!(
            &names_of(&filtered.ok(needle)["TranscriptionJobSummaries"]),
            expected,
            "case {needle:?}: unexpected JobNameContains result, got {:?}",
            filtered.body["TranscriptionJobSummaries"]
        );
    }

    // The `Status` filter selects only jobs in that status, is echoed back, and the
    // four statuses together still cover every job. Waiting past every transition
    // first keeps the partition from being read mid-transition, and the assertion
    // is on the UNION of the names rather than on a sum, so it cannot depend on
    // where the clock is between two of the calls.
    std::thread::sleep(PAST_EVERY_TRANSITION);
    let mut covered: Vec<String> = Vec::new();
    for status in ["QUEUED", "IN_PROGRESS", "COMPLETED", "FAILED"] {
        let filtered = transcribe
            .call(
                "ListTranscriptionJobs",
                json!({"Status": status, "MaxResults": 100}),
            )
            .await;
        let body = filtered.ok(status);
        assert_eq!(
            body["Status"],
            json!(status),
            "case {status:?}: the Status filter must be echoed back, got {:?}",
            body["Status"]
        );
        let summaries = body["TranscriptionJobSummaries"]
            .as_array()
            .expect("TranscriptionJobSummaries must be a list");
        for summary in summaries {
            assert_eq!(
                summary["TranscriptionJobStatus"],
                json!(status),
                "case {status:?}: the filter must return only jobs in that status, got {:?}",
                summary["TranscriptionJobStatus"]
            );
        }
        covered.extend(names_of(&body["TranscriptionJobSummaries"]));
    }
    covered.sort();
    covered.dedup();
    let mut expected_names = newest_first.clone();
    expected_names.sort();
    assert_eq!(
        covered, expected_names,
        "the four statuses together must cover every job, got {covered:?}"
    );

    // An unfiltered listing with no MaxResults returns the documented default page
    // of 5, with a cursor to the rest.
    let defaulted = transcribe.call("ListTranscriptionJobs", json!({})).await;
    let body = defaulted.ok("default page size");
    assert_eq!(
        names_of(&body["TranscriptionJobSummaries"]).len(),
        5,
        "the documented default page size is 5, got {:?}",
        body["TranscriptionJobSummaries"]
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated page must carry a NextToken, got {:?}",
        body["NextToken"]
    );
}

// SPDX-License-Identifier: CC0-1.0

//! What a finished job's reported output location SERVES, end to end over the wire:
//! a start, then the family's reader polled until the job is finished, then a plain
//! object-store GET of every location that reader reported.
//!
//! THE RULE THESE ROWS ASSERT: a job the service reports finished has its output
//! WRITTEN at the location it reports, so a client that follows that URI fetches the
//! document — `Transcript$TranscriptFileUri` is "The Amazon S3 location of your
//! transcript. You can use this URI to access or download your transcript"
//! (`smithy_query(query="doc", shape="Transcript", member="TranscriptFileUri")`), and
//! every other output member of this service documents its own location the same way
//! (`SubtitlesOutput$SubtitleFileUris`, `MedicalTranscript$TranscriptFileUri`,
//! `MedicalScribeOutput$TranscriptFileUri` / `ClinicalDocumentUri`). The location is
//! owed its document when the job FINISHES and not before, and what is stored there is
//! not replaced by a later read.
//!
//! The rows differ along what the rule generalises over: which family started the job
//! (all four of them), where the request directed the output (a caller-named bucket, a
//! key naming a file, a key naming a folder, a Call Analytics `OutputLocation` in each
//! of its documented forms, and the service-managed location a request that named none
//! gets), how many locations the job therefore reports (redaction asks for a second
//! transcript, each requested subtitle format for another file), and which document
//! kind sits at each one (a transcript, a clinical note, a WebVTT file, a SubRip file).
//!
//! The emulator mounted here holds the Amazon S3 service beside Transcribe on ONE
//! shared engine, which is what the assembled router does, and the media every row
//! names is staged in it first — reusing the harness the media-resolution suite stands
//! up (`crate::media_object_resolution`: its emulator, its Transcribe call, its staged
//! media and its raw path-style object-store request) rather than building a second
//! one. One Region serves every call, the one that harness signs for; which Region a
//! record belongs to is another suite's subject (`crate::regional_resource_scoping`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::media_object_resolution::{call, emulator, given_staged_media, s3, BUCKET, STAGED_KEYS};

/// How long a poll of the family's reader waits between two calls, and how many calls
/// it makes before giving up: together comfortably past the two spans of
/// `crate::state::JOB_TRANSITION_DELAY` a queued job needs, even on a slow machine.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
const POLLS_FOR_FINISHED: usize = 60;

/// The statuses of `TranscriptionJobStatus` a job is still being worked on in
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`); the other two are
/// terminal.
const UNFINISHED: &[&str] = &["QUEUED", "IN_PROGRESS"];

/// The role a Medical Scribe start names (`DataAccessRoleArn`, a required member).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// The media every row's start is pointed at: an object the staged bucket really holds.
fn media() -> Value {
    json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])})
}

/// Which family a row drives: the start, the reader, and the members each spells the
/// job and its output with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// `StartTranscriptionJob` -> `GetTranscriptionJob`.
    Transcription,
    /// `StartMedicalTranscriptionJob` -> `GetMedicalTranscriptionJob`.
    Medical,
    /// `StartCallAnalyticsJob` -> `GetCallAnalyticsJob`.
    CallAnalytics,
    /// `StartMedicalScribeJob` -> `GetMedicalScribeJob`.
    Scribe,
}

impl Family {
    fn start(self) -> &'static str {
        match self {
            Self::Transcription => "StartTranscriptionJob",
            Self::Medical => "StartMedicalTranscriptionJob",
            Self::CallAnalytics => "StartCallAnalyticsJob",
            Self::Scribe => "StartMedicalScribeJob",
        }
    }

    fn get(self) -> &'static str {
        match self {
            Self::Transcription => "GetTranscriptionJob",
            Self::Medical => "GetMedicalTranscriptionJob",
            Self::CallAnalytics => "GetCallAnalyticsJob",
            Self::Scribe => "GetMedicalScribeJob",
        }
    }

    fn name_member(self) -> &'static str {
        match self {
            Self::Transcription => "TranscriptionJobName",
            Self::Medical => "MedicalTranscriptionJobName",
            Self::CallAnalytics => "CallAnalyticsJobName",
            Self::Scribe => "MedicalScribeJobName",
        }
    }

    /// The member of a successful read that carries the job.
    fn job_member(self) -> &'static str {
        match self {
            Self::Transcription => "TranscriptionJob",
            Self::Medical => "MedicalTranscriptionJob",
            Self::CallAnalytics => "CallAnalyticsJob",
            Self::Scribe => "MedicalScribeJob",
        }
    }

    /// The member of the job that carries its status.
    fn status_member(self) -> &'static str {
        match self {
            Self::Scribe => "MedicalScribeJobStatus",
            Self::CallAnalytics => "CallAnalyticsJobStatus",
            Self::Transcription | Self::Medical => "TranscriptionJobStatus",
        }
    }

    /// The smallest valid start request of this family, before a row's own members are
    /// merged in.
    fn start_request(self, name: &str) -> Value {
        match self {
            Self::Transcription => json!({
                "TranscriptionJobName": name,
                "LanguageCode": "en-US",
                "Media": media(),
            }),
            Self::Medical => json!({
                "MedicalTranscriptionJobName": name,
                "LanguageCode": "en-US",
                "Media": media(),
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION",
            }),
            Self::CallAnalytics => json!({
                "CallAnalyticsJobName": name,
                "Media": media(),
            }),
            Self::Scribe => json!({
                "MedicalScribeJobName": name,
                "Media": media(),
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2},
            }),
        }
    }

    /// Every output location the finished job reports, read off the job the reader
    /// answered with.
    fn reported_locations(self, job: &Value) -> Vec<String> {
        let mut locations = Vec::new();
        match self {
            Self::Scribe => {
                for member in ["TranscriptFileUri", "ClinicalDocumentUri"] {
                    if let Some(uri) = job["MedicalScribeOutput"][member].as_str() {
                        locations.push(uri.to_string());
                    }
                }
            }
            _ => {
                for member in ["TranscriptFileUri", "RedactedTranscriptFileUri"] {
                    if let Some(uri) = job["Transcript"][member].as_str() {
                        locations.push(uri.to_string());
                    }
                }
            }
        }
        if let Some(uris) = job["Subtitles"]["SubtitleFileUris"].as_array() {
            locations.extend(uris.iter().filter_map(Value::as_str).map(str::to_string));
        }
        locations
    }
}

/// One row: a job, where its start directs the output, and how many locations the
/// finished job must therefore report.
struct Row {
    /// What this row varies, named in every assertion it fails.
    case: &'static str,
    family: Family,
    name: &'static str,
    /// The members merged into the family's smallest start request.
    output: fn() -> Value,
    /// The buckets the row's start names, which the CALLER creates before starting the
    /// job — exactly as the real service requires. A row that named none leaves this
    /// empty: the service-managed bucket is the service's own to stand up.
    buckets: &'static [&'static str],
    /// How many output locations the finished job reports, so a row cannot pass by
    /// reporting none.
    locations: usize,
}

/// The bucket the rows that name one of their own direct their output to.
const OUTPUT_BUCKET: &str = "example-job-output";

/// The rows. They span all four start operations, every documented shape of an output
/// location, both redaction outputs and each subtitle format the model enumerates
/// (`smithy_query(query="shape", shape="SubtitleFormat")`: vtt | srt).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a transcription job whose OutputKey names the output file",
            family: Family::Transcription,
            name: "named-file",
            output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "transcripts/chosen.json"}),
            buckets: &[OUTPUT_BUCKET],
            locations: 1,
        },
        Row {
            case: "a transcription job whose OutputKey names a folder",
            family: Family::Transcription,
            name: "named-folder",
            output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "transcripts/"}),
            buckets: &[OUTPUT_BUCKET],
            locations: 1,
        },
        Row {
            case: "a transcription job that named no bucket, so the service-managed \
                   location answers",
            family: Family::Transcription,
            name: "service-managed",
            output: || json!({}),
            buckets: &[],
            locations: 1,
        },
        Row {
            case: "a transcription job asking for both subtitle formats beside its \
                   transcript",
            family: Family::Transcription,
            name: "subtitled",
            output: || {
                json!({
                    "OutputBucketName": OUTPUT_BUCKET,
                    "Subtitles": {"Formats": ["vtt", "srt"]},
                })
            },
            buckets: &[OUTPUT_BUCKET],
            locations: 3,
        },
        Row {
            case: "a transcription job naming one subtitle format more than once, which \
                   is one file reported twice",
            family: Family::Transcription,
            name: "subtitled-repeated",
            output: || {
                json!({
                    "OutputBucketName": OUTPUT_BUCKET,
                    "OutputKey": "repeated/",
                    "Subtitles": {"Formats": ["vtt", "vtt", "srt"]},
                })
            },
            buckets: &[OUTPUT_BUCKET],
            locations: 4,
        },
        Row {
            case: "a transcription job asking for a redacted transcript beside the \
                   unredacted one, and one subtitle file",
            family: Family::Transcription,
            name: "redacted-and-unredacted",
            output: || {
                json!({
                    "OutputBucketName": OUTPUT_BUCKET,
                    "OutputKey": "both/",
                    "Subtitles": {"Formats": ["vtt"]},
                    "ContentRedaction": {
                        "RedactionType": "PII",
                        "RedactionOutput": "redacted_and_unredacted",
                    },
                })
            },
            buckets: &[OUTPUT_BUCKET],
            locations: 3,
        },
        Row {
            case: "a transcription job asking for the redacted transcript alone, in the \
                   service-managed location",
            family: Family::Transcription,
            name: "redacted-only",
            output: || {
                json!({
                    "ContentRedaction": {"RedactionType": "PII", "RedactionOutput": "redacted"},
                })
            },
            buckets: &[],
            locations: 1,
        },
        Row {
            case: "a medical transcription job, whose OutputBucketName is required",
            family: Family::Medical,
            name: "medical",
            output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "medical/"}),
            buckets: &[OUTPUT_BUCKET],
            locations: 1,
        },
        Row {
            case: "a Call Analytics job that named no output location",
            family: Family::CallAnalytics,
            name: "analytics-service-managed",
            output: || json!({}),
            buckets: &[],
            locations: 1,
        },
        Row {
            case: "a Call Analytics job whose OutputLocation names a folder",
            family: Family::CallAnalytics,
            name: "analytics-folder",
            output: || json!({"OutputLocation": format!("s3://{OUTPUT_BUCKET}/calls/")}),
            buckets: &[OUTPUT_BUCKET],
            locations: 1,
        },
        Row {
            case: "a Call Analytics job whose OutputLocation names the output file",
            family: Family::CallAnalytics,
            name: "analytics-named-file",
            output: || {
                json!({"OutputLocation": format!("s3://{OUTPUT_BUCKET}/calls/chosen-call.json")})
            },
            buckets: &[OUTPUT_BUCKET],
            locations: 1,
        },
        Row {
            case: "a Call Analytics job asking for a redacted transcript beside the \
                   unredacted one",
            family: Family::CallAnalytics,
            name: "analytics-redacted",
            output: || {
                json!({
                    "OutputLocation": format!("s3://{OUTPUT_BUCKET}/redacted-calls/"),
                    "Settings": {
                        "ContentRedaction": {
                            "RedactionType": "PII",
                            "RedactionOutput": "redacted_and_unredacted",
                        },
                    },
                })
            },
            buckets: &[OUTPUT_BUCKET],
            locations: 2,
        },
        Row {
            case: "a Medical Scribe job, which reports a transcript and a clinical note",
            family: Family::Scribe,
            name: "scribe",
            output: || json!({"OutputBucketName": OUTPUT_BUCKET}),
            buckets: &[OUTPUT_BUCKET],
            locations: 2,
        },
    ]
}

/// The start request of a row: the family's smallest valid one with the row's own
/// output members merged in.
fn start_request(row: &Row) -> Value {
    let mut request = row.family.start_request(row.name);
    let Value::Object(output) = (row.output)() else {
        panic!("case {:?}: a row's output members are an object", row.case);
    };
    for (member, value) in output {
        request[member] = value;
    }
    request
}

/// The path a client resolves a reported output URI to: everything from the bucket on,
/// whether the URI is the `s3://BUCKET/KEY` of a caller-named location or the
/// path-style `https://s3.REGION.amazonaws.com/BUCKET/KEY` of a service-managed one.
fn object_path(case: &str, uri: &str) -> String {
    if let Some(rest) = uri.strip_prefix("s3://") {
        return format!("/{rest}");
    }
    let rest = uri
        .split_once("://")
        .unwrap_or_else(|| panic!("case {case:?}: a reported location is a URI, got {uri:?}"))
        .1;
    let at = rest
        .find('/')
        .unwrap_or_else(|| panic!("case {case:?}: a reported location names a path, got {uri:?}"));
    rest[at..].to_string()
}

/// Create the buckets a row's start directs its output to, as the caller must before
/// starting the job.
async fn given_output_buckets(router: &MultiServiceRouter, row: &Row) {
    for bucket in row.buckets {
        let (status, body) = s3(router, "PUT", &format!("/{bucket}"), &[]).await;
        assert!(
            // 409 is the bucket already being there, which an earlier row created.
            status == 200 || status == 409,
            "case {:?}: the output bucket {bucket} must exist, got HTTP {status} with \
             body {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
    }
}

/// Poll the family's reader until the job has FINISHED, and answer the job it reported.
async fn finished_job(router: &MultiServiceRouter, row: &Row) -> Value {
    let request = json!({row.family.name_member(): row.name});
    for _ in 0..POLLS_FOR_FINISHED {
        let reply = call(router, row.family.get(), request.clone()).await;
        let job = reply.ok(row.case)[row.family.job_member()].clone();
        let status = job[row.family.status_member()]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {:?}: a read reports a status, got {job:?}", row.case)
            })
            .to_string();
        if !UNFINISHED.contains(&status.as_str()) {
            assert_eq!(
                status, "COMPLETED",
                "case {:?}: a job the service found nothing wrong with must complete, \
                 got {status:?} in {job:?}",
                row.case
            );
            return job;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("case {:?}: the job never finished", row.case);
}

/// Assert the document at `body` is the one the extension of `path` names, and that it
/// carries the job's own identity rather than anything invented.
fn assert_document(case: &str, name: &str, path: &str, body: &[u8]) {
    let text = std::str::from_utf8(body)
        .unwrap_or_else(|error| panic!("case {case:?}: {path} is not UTF-8: {error}"));
    if path.ends_with(".vtt") {
        // A WebVTT file is identified by its own first line.
        assert!(
            text.starts_with("WEBVTT"),
            "case {case:?}: the WebVTT file at {path} must begin with its signature, \
             got {text:?}"
        );
        return;
    }
    if path.ends_with(".srt") {
        // A SubRip file is a sequence of cues and nothing else, so one with no caption
        // holds nothing.
        assert!(
            text.trim().is_empty(),
            "case {case:?}: the SubRip file at {path} holds no caption, got {text:?}"
        );
        return;
    }
    let document: Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("case {case:?}: {path} is not JSON ({error}): {text:?}"));
    if path.ends_with("summary.json") {
        // The clinical note: no section, because no conversation was transcribed.
        let sections = document["ClinicalDocumentation"]["Sections"].as_array();
        assert_eq!(
            sections.map(Vec::len),
            Some(0),
            "case {case:?}: the clinical note at {path} must hold no section, got \
             {document:?}"
        );
        return;
    }
    assert_eq!(
        document["jobName"].as_str(),
        Some(name),
        "case {case:?}: the transcript at {path} names the job it is the output of, got \
         {document:?}"
    );
    assert_eq!(
        document["status"].as_str(),
        Some("COMPLETED"),
        "case {case:?}: the transcript at {path} reports the status the job finished in, \
         got {document:?}"
    );
    assert_eq!(
        document["results"]["transcripts"][0]["transcript"].as_str(),
        Some(""),
        "case {case:?}: no audio was decoded, so the transcript at {path} holds no \
         words, got {document:?}"
    );
}

/// Every location a finished job reports serves the document it names.
#[tokio::test]
async fn every_reported_output_location_serves_its_document() {
    let router = emulator();
    given_staged_media(&router).await;

    for row in &rows() {
        given_output_buckets(&router, row).await;
        call(&router, row.family.start(), start_request(row))
            .await
            .ok(row.case);
        let job = finished_job(&router, row).await;

        let locations = row.family.reported_locations(&job);
        assert_eq!(
            locations.len(),
            row.locations,
            "case {:?}: expected {} reported output location(s), got {locations:?}",
            row.case,
            row.locations
        );
        for location in &locations {
            let path = object_path(row.case, location);
            let (status, body) = s3(&router, "GET", &path, &[]).await;
            assert_eq!(
                status, 200,
                "case {:?}: the reported location {location:?} must serve its document, \
                 got HTTP {status} with body {:?}",
                row.case,
                String::from_utf8_lossy(&body)
            );
            assert_document(row.case, row.name, &path, &body);
        }
    }
}

/// The location is owed its document when the job FINISHES: while the service is still
/// working on the job it reports no location and nothing is stored where the finished
/// job will report one.
#[tokio::test]
async fn an_unfinished_job_has_written_nothing() {
    let router = emulator();
    given_staged_media(&router).await;
    let row = Row {
        case: "a job read straight after it was started",
        family: Family::Transcription,
        name: "still-running",
        output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "pending/out.json"}),
        buckets: &[OUTPUT_BUCKET],
        locations: 1,
    };
    given_output_buckets(&router, &row).await;
    call(&router, row.family.start(), start_request(&row))
        .await
        .ok(row.case);

    // The caller acts straight after the start, so the job is still being worked on.
    let reply = call(
        &router,
        row.family.get(),
        json!({row.family.name_member(): row.name}),
    )
    .await;
    let job = reply.ok(row.case)[row.family.job_member()].clone();
    let status = job[row.family.status_member()].as_str().unwrap_or_default();
    assert!(
        UNFINISHED.contains(&status),
        "case {:?}: a job read straight after the start is still being worked on, got \
         {status:?}",
        row.case
    );
    assert!(
        row.family.reported_locations(&job).is_empty(),
        "case {:?}: an unfinished job reports no output location, got {job:?}",
        row.case
    );

    // The location the finished job will report holds nothing yet.
    let path = format!("/{OUTPUT_BUCKET}/pending/out.json");
    let (status, body) = s3(&router, "GET", &path, &[]).await;
    assert_eq!(
        status, 404,
        "case {:?}: nothing is stored at {path} while the job runs, got HTTP {status} \
         with body {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );

    // Once the job has finished, the same location serves its transcript.
    let job = finished_job(&router, &row).await;
    let locations = row.family.reported_locations(&job);
    assert_eq!(
        locations,
        vec![format!("s3://{OUTPUT_BUCKET}/pending/out.json")],
        "case {:?}: the finished job reports the location its request directed the \
         output to, got {locations:?}",
        row.case
    );
    let (status, body) = s3(&router, "GET", &path, &[]).await;
    assert_eq!(
        status, 200,
        "case {:?}: the finished job's location serves its transcript, got HTTP \
         {status} with body {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
    assert_document(row.case, row.name, &path, &body);
}

/// An output location of the `http(s)` form that names a HOST rather than this
/// emulator's object store addresses no object of it, so nothing is written for it — in
/// particular not into a bucket named after its first path segment, which the caller
/// never named and no reader of this service reports.
#[tokio::test]
async fn an_output_location_naming_a_host_writes_nothing() {
    let router = emulator();
    given_staged_media(&router).await;
    let row = Row {
        case: "a Call Analytics job whose OutputLocation names a host",
        family: Family::CallAnalytics,
        name: "hosted-output",
        output: || json!({"OutputLocation": "https://host.example/some/call.json"}),
        buckets: &[],
        locations: 1,
    };
    call(&router, row.family.start(), start_request(&row))
        .await
        .ok(row.case);
    let job = finished_job(&router, &row).await;

    // The job reports the location its request named, as it always has.
    let locations = row.family.reported_locations(&job);
    assert_eq!(
        locations,
        vec!["https://host.example/some/call.json".to_string()],
        "case {:?}: the job reports the location its request named, got {locations:?}",
        row.case
    );
    // Nothing was written under its first path segment read as a bucket.
    let (status, body) = s3(&router, "GET", "/some/call.json", &[]).await;
    assert_eq!(
        status, 404,
        "case {:?}: no object may be written for a location that names a host, got \
         HTTP {status} with body {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
}

/// A READ does not rewrite what the location holds: the document is stored once, when
/// the job finishes, so whatever the location holds afterwards is what a client last
/// put there.
#[tokio::test]
async fn reading_a_finished_job_again_leaves_the_stored_document_alone() {
    let router = emulator();
    given_staged_media(&router).await;
    let row = Row {
        case: "a finished job read twice",
        family: Family::Transcription,
        name: "read-twice",
        output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "twice/out.json"}),
        buckets: &[OUTPUT_BUCKET],
        locations: 1,
    };
    given_output_buckets(&router, &row).await;
    call(&router, row.family.start(), start_request(&row))
        .await
        .ok(row.case);
    finished_job(&router, &row).await;

    // What the caller puts at the reported location afterwards is the caller's.
    let path = format!("/{OUTPUT_BUCKET}/twice/out.json");
    let replacement = br#"{"jobName":"replaced-by-the-caller"}"#;
    let (status, body) = s3(&router, "PUT", &path, replacement).await;
    assert_eq!(
        status, 200,
        "case {:?}: the caller may overwrite the object, got HTTP {status} with body \
         {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );

    // Reading the job again reports the same location and leaves it as the caller left
    // it.
    let job = finished_job(&router, &row).await;
    let locations = row.family.reported_locations(&job);
    assert_eq!(
        locations.len(),
        1,
        "case {:?}: the job still reports its one output location, got {locations:?}",
        row.case
    );
    let (status, body) = s3(&router, "GET", &path, &[]).await;
    assert_eq!(
        status, 200,
        "case {:?}: the location still serves an object, got HTTP {status}",
        row.case
    );
    assert_eq!(
        body.as_ref(),
        replacement.as_slice(),
        "case {:?}: a read must not rewrite the stored document, got {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
}

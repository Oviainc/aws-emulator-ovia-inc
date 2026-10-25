// SPDX-License-Identifier: CC0-1.0

//! What a COMPLETED transcription job reports as the location of each transcript it
//! was asked to produce, end to end over the wire: `StartTranscriptionJob` ->
//! `GetTranscriptionJob` / `ListTranscriptionJobs`.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: a finished job reports the Amazon S3 location of each
//! transcript its request asked the service to produce and of no other — a redacted
//! location (`Transcript.RedactedTranscriptFileUri`) exactly when redaction was asked
//! for, and the unredacted one (`Transcript.TranscriptFileUri`) except where only the
//! redacted transcript was asked for — with the two at distinct keys in whatever
//! bucket and folder the request directed its output to.
//!
//! Each test asserts that over a table spanning the axis the rule generalises over:
//! every value of the model's `RedactionOutput` enum and the absent-redaction case,
//! crossed with every documented form of the output location, plus a deferred job that
//! reaches the same end through an extra status transition.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::ops::types::REDACTION_OUTPUT_VALUES;
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::transcription_jobs::Transcribe;

/// Long enough for every clock-owed transition of a job started during the test to
/// have fallen due: a deferred job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans
/// of `crate::state::JOB_TRANSITION_DELAY` (500 ms each).
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// One row of the table: a start request, and which of the two transcript locations
/// the job it starts must report once it has completed.
struct Row {
    case: String,
    request: Value,
    unredacted: bool,
    redacted: bool,
}

/// The `ContentRedaction` a row asks for, and which transcripts that asks the service
/// to produce.
///
/// Built from the model's own enum (`REDACTION_OUTPUT_VALUES`, from
/// `smithy_query(query="shape", shape="RedactionOutput")`) rather than a hand-written
/// list, so every value the model carries is exercised: `redacted` means "Amazon
/// Transcribe creates only a redacted transcript", and the other value means it
/// "creates a redacted and an unredacted transcript (as two separate files)" (model
/// doc for `ContentRedaction$RedactionOutput`).
fn redaction_cases() -> Vec<(String, Value, bool, bool)> {
    let mut cases = vec![(
        "no redaction asked for".to_string(),
        Value::Null,
        true,
        false,
    )];
    for output in REDACTION_OUTPUT_VALUES {
        cases.push((
            format!("RedactionOutput {output}"),
            json!({"RedactionType": "PII", "RedactionOutput": output}),
            *output != "redacted",
            true,
        ));
    }
    cases
}

/// Every row of the table: each redaction case against each documented output-location
/// form, plus one deferred-execution row per redaction case.
fn rows() -> Vec<Row> {
    // The output-location forms `StartTranscriptionJobRequest$OutputBucketName` and
    // `$OutputKey` document: no bucket at all (the service-managed location of
    // `Transcript$TranscriptFileUri`), a bucket alone, a bucket with a key naming a
    // folder, and a bucket with a key naming the output file.
    let locations: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("service-managed location", None, None),
        ("a bucket alone", Some("example-output"), None),
        (
            "a key naming a folder",
            Some("example-output"),
            Some("transcripts/"),
        ),
        (
            "a key naming the output file",
            Some("example-output"),
            Some("transcripts/chosen-name.json"),
        ),
    ];

    let mut rows = Vec::new();
    for (location_index, (location_case, bucket, key)) in locations.iter().enumerate() {
        for (redaction_index, (redaction_case, redaction, unredacted, redacted)) in
            redaction_cases().iter().enumerate()
        {
            let mut request = json!({
                "TranscriptionJobName":
                    format!("redaction-{location_index}-{redaction_index}"),
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "LanguageCode": "en-US"
            });
            if let Some(bucket) = bucket {
                request["OutputBucketName"] = json!(bucket);
            }
            if let Some(key) = key {
                request["OutputKey"] = json!(key);
            }
            if !redaction.is_null() {
                request["ContentRedaction"] = redaction.clone();
            }
            rows.push(Row {
                case: format!("{location_case}, {redaction_case}"),
                request,
                unredacted: *unredacted,
                redacted: *redacted,
            });
        }
    }

    // A job whose execution the request deferred reaches `COMPLETED` through an extra
    // transition, and must report the same locations when it gets there.
    for (redaction_index, (redaction_case, redaction, unredacted, redacted)) in
        redaction_cases().iter().enumerate()
    {
        let mut request = json!({
            "TranscriptionJobName": format!("redaction-deferred-{redaction_index}"),
            "Media": {"MediaFileUri": "s3://example-media/call.flac"},
            "LanguageCode": "en-US",
            "OutputBucketName": "example-output",
            "JobExecutionSettings": {
                "AllowDeferredExecution": true,
                "DataAccessRoleArn": "arn:aws:iam::111122223333:role/TranscribeAccess"
            }
        });
        if !redaction.is_null() {
            request["ContentRedaction"] = redaction.clone();
        }
        rows.push(Row {
            case: format!("deferred execution, {redaction_case}"),
            request,
            unredacted: *unredacted,
            redacted: *redacted,
        });
    }
    rows
}

/// The name a row's request gives its job.
fn job_name(request: &Value) -> String {
    request["TranscriptionJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// Start every row, wait out the one span they share, and answer each row's completed
/// job beside its case name.
async fn completed_jobs(transcribe: &Transcribe, rows: &[Row]) -> Vec<Value> {
    for row in rows {
        transcribe
            .call("StartTranscriptionJob", row.request.clone())
            .await
            .ok(&row.case);
    }
    // One wait for every row, not one per row: nothing sleeps inside the service.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    let mut jobs = Vec::new();
    for row in rows {
        let fetched = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": job_name(&row.request)}),
            )
            .await;
        let job = fetched.ok(&row.case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {:?}: the processing span has elapsed, so the job must be COMPLETED, \
             got {:?}",
            row.case,
            job["TranscriptionJobStatus"]
        );
        jobs.push(job);
    }
    jobs
}

#[tokio::test]
async fn a_completed_job_reports_the_transcripts_its_redaction_settings_asked_for() {
    let transcribe = Transcribe::new();
    let rows = rows();
    let jobs = completed_jobs(&transcribe, &rows).await;

    for (row, job) in rows.iter().zip(&jobs) {
        let case = &row.case;
        let transcript = job["Transcript"].clone();
        assert_eq!(
            transcript["TranscriptFileUri"].is_string(),
            row.unredacted,
            "case {case:?}: an unredacted transcript location is reported exactly where \
             an unredacted transcript was asked for, got {transcript:?}"
        );
        assert_eq!(
            transcript["RedactedTranscriptFileUri"].is_string(),
            row.redacted,
            "case {case:?}: a redacted transcript location is reported exactly where \
             redaction was asked for, got {transcript:?}"
        );

        // Whichever locations it reports, each one is a value of the member's own
        // modelled shape (`Uri`, `@pattern "^(s3://|http(s*)://).+$"`).
        for member in ["TranscriptFileUri", "RedactedTranscriptFileUri"] {
            if let Some(uri) = transcript[member].as_str() {
                assert!(
                    uri.starts_with("s3://") || uri.starts_with("http://")
                        || uri.starts_with("https://"),
                    "case {case:?}: {member} must be a modelled Uri, got {uri:?}"
                );
                assert!(
                    uri.ends_with(".json"),
                    "case {case:?}: {member} names a transcript file, got {uri:?}"
                );
            }
        }

        let Some(redacted_uri) = transcript["RedactedTranscriptFileUri"].as_str() else {
            continue;
        };
        if let Some(unredacted_uri) = transcript["TranscriptFileUri"].as_str() {
            // "two separate files": two distinct keys in the same folder, which is
            // where the request directed the job's output.
            assert_ne!(
                redacted_uri, unredacted_uri,
                "case {case:?}: the two transcripts are separate files, got \
                 {redacted_uri:?} for both"
            );
            let (redacted_folder, _) = redacted_uri
                .rsplit_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: an output URI has a folder"));
            let (unredacted_folder, _) = unredacted_uri
                .rsplit_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: an output URI has a folder"));
            assert_eq!(
                redacted_folder, unredacted_folder,
                "case {case:?}: both transcripts go where the request directed its \
                 output, got {redacted_uri:?} beside {unredacted_uri:?}"
            );
        }

        // A read does not change what a later read sees.
        let again = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": job_name(&row.request)}),
            )
            .await;
        assert_eq!(
            again.ok(case)["TranscriptionJob"], *job,
            "case {case:?}: a second read must report the same job"
        );
    }
}

#[tokio::test]
async fn the_redacted_transcript_sits_beside_the_unredacted_one_in_the_requested_bucket() {
    let transcribe = Transcribe::new();
    let account = engine::emulator_account_id();

    // The spelling, pinned per output-location form on jobs that asked for both
    // transcripts: the redacted file is the unredacted file's name under its own key,
    // in the caller's bucket and folder when the request named one and in the
    // service-managed location otherwise.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a bucket alone",
            json!({
                "TranscriptionJobName": "both-bucket",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "LanguageCode": "en-US",
                "OutputBucketName": "example-output",
                "ContentRedaction": {
                    "RedactionType": "PII",
                    "RedactionOutput": "redacted_and_unredacted"
                }
            }),
            json!({
                "TranscriptFileUri": "s3://example-output/both-bucket.json",
                "RedactedTranscriptFileUri": "s3://example-output/redacted-both-bucket.json"
            }),
        ),
        (
            "a key naming a folder",
            json!({
                "TranscriptionJobName": "both-folder",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "LanguageCode": "en-US",
                "OutputBucketName": "example-output",
                "OutputKey": "transcripts/",
                "ContentRedaction": {
                    "RedactionType": "PII",
                    "RedactionOutput": "redacted_and_unredacted",
                    "PiiEntityTypes": ["NAME", "PHONE"]
                }
            }),
            json!({
                "TranscriptFileUri": "s3://example-output/transcripts/both-folder.json",
                "RedactedTranscriptFileUri":
                    "s3://example-output/transcripts/redacted-both-folder.json"
            }),
        ),
        (
            "a key naming the output file",
            json!({
                "TranscriptionJobName": "both-named",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "LanguageCode": "en-US",
                "OutputBucketName": "example-output",
                "OutputKey": "transcripts/chosen-name.json",
                "ContentRedaction": {
                    "RedactionType": "PII",
                    "RedactionOutput": "redacted_and_unredacted"
                }
            }),
            json!({
                "TranscriptFileUri": "s3://example-output/transcripts/chosen-name.json",
                "RedactedTranscriptFileUri":
                    "s3://example-output/transcripts/redacted-chosen-name.json"
            }),
        ),
        (
            "the service-managed location, redacted output only",
            json!({
                "TranscriptionJobName": "redacted-only",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "LanguageCode": "en-US",
                "ContentRedaction": {"RedactionType": "PII", "RedactionOutput": "redacted"}
            }),
            json!({
                "RedactedTranscriptFileUri": format!(
                    "https://s3.{DEFAULT_REGION}.amazonaws.com/\
                     aws-transcribe-{DEFAULT_REGION}-prod/{account}/redacted-redacted-only.json"
                )
            }),
        ),
    ];

    for (case, request, _) in &rows {
        transcribe
            .call("StartTranscriptionJob", request.clone())
            .await
            .ok(case);
    }
    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (case, request, expected) in &rows {
        let fetched = transcribe
            .call(
                "GetTranscriptionJob",
                json!({"TranscriptionJobName": job_name(request)}),
            )
            .await;
        let job = fetched.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            &job["Transcript"], expected,
            "case {case:?}: expected the transcript locations {expected:?}, got {:?}",
            job["Transcript"]
        );
        // The summary of the same job echoes the redaction it was started with, so a
        // client that found the job in a listing asks Get for its locations.
        let listed = transcribe
            .call(
                "ListTranscriptionJobs",
                json!({"JobNameContains": job_name(request), "MaxResults": 100}),
            )
            .await;
        let summaries = listed.ok(case)["TranscriptionJobSummaries"].clone();
        let summary = summaries
            .as_array()
            .expect("TranscriptionJobSummaries must be a list")
            .iter()
            .find(|summary| summary["TranscriptionJobName"] == json!(job_name(request)))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the job must appear in the listing"));
        assert_eq!(
            summary["ContentRedaction"], request["ContentRedaction"],
            "case {case:?}: the summary must report the redaction the job was started \
             with, got {:?}",
            summary["ContentRedaction"]
        );
    }
}

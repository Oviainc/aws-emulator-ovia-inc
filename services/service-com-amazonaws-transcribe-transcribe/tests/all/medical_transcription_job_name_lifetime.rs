// SPDX-License-Identifier: CC0-1.0

//! How long a MEDICAL transcription job's NAME stays taken, end to end over the wire:
//! `StartMedicalTranscriptionJob` -> `GetMedicalTranscriptionJob` ->
//! `StartMedicalTranscriptionJob` again.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: a job name is taken by the stored RECORD, not by the work —
//! "A unique name, chosen by you... must be unique within an Amazon Web Services
//! account. If you try to create a new job with the same name as an existing job, you
//! get a ConflictException error" (model doc for
//! `StartMedicalTranscriptionJobRequest$MedicalTranscriptionJobName`,
//! `smithy_query(query="doc", shape="StartMedicalTranscriptionJobRequest",
//! member="MedicalTranscriptionJobName")`) — so a second start under that name is
//! refused for as long as the record exists, including after the job has reached its
//! terminal status and released its concurrency slot, whatever the retry's body says;
//! the refusal stores nothing, leaves the stored record exactly as it was, and leaves
//! every other name startable. The existing medical-job suite pins the conflict while a
//! job is still in flight; what this suite adds is that finishing the job does not give
//! the name back.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::Transcribe;

/// Long enough for the `IN_PROGRESS -> COMPLETED` transition of a job started during
/// the test to have fallen due: one span of `crate::state::JOB_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The status a medical transcription job the clock has finished with reports, so a job
/// in it is done and its concurrency slot is free
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`).
const FINISHED_STATUS: &str = "COMPLETED";

/// How this service words "that name is already taken": the resource KIND, never the
/// name the caller asked about.
const NAME_TAKEN: &str = "The requested job name already exists. Use a different job name.";

/// One row: a job started under `name`, and a retry under the SAME name whose body
/// differs along the axis the row is named for. The name alone decides, so every row
/// must be refused identically.
struct Row {
    case: &'static str,
    /// The start that takes the name.
    first: Value,
    /// The start that asks for the name again once the first job has finished.
    second: Value,
}

/// The rows, spanning the retry forms the rule generalises over: a byte-identical
/// retry, and retries differing in media file and format, in `Type`, in output
/// location and sample rate, and in tag count.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a byte-identical retry",
            first: json!({
                "MedicalTranscriptionJobName": "name-lifetime-identical",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "MediaFormat": "wav",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
            second: json!({
                "MedicalTranscriptionJobName": "name-lifetime-identical",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "MediaFormat": "wav",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        },
        Row {
            case: "a retry naming another media file in another format",
            first: json!({
                "MedicalTranscriptionJobName": "name.lifetime_media",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/visit.flac"},
                "MediaFormat": "flac",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
            second: json!({
                "MedicalTranscriptionJobName": "name.lifetime_media",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "https://example.invalid/media/other.webm"},
                "MediaFormat": "webm",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
        },
        Row {
            case: "a retry of the other Type",
            first: json!({
                "MedicalTranscriptionJobName": "n",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/notes.mp3"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
            second: json!({
                "MedicalTranscriptionJobName": "n",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/notes.mp3"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
        },
        Row {
            case: "a retry asking for another output location and sample rate",
            first: json!({
                "MedicalTranscriptionJobName": "name-lifetime-output",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/visit.mp4"},
                "MediaFormat": "mp4",
                "MediaSampleRateHertz": 16_000,
                "OutputBucketName": "example-medical-output",
                "OutputKey": "clinic/visits/",
                "Settings": {"ChannelIdentification": true},
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
            second: json!({
                "MedicalTranscriptionJobName": "name-lifetime-output",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/visit.mp4"},
                "MediaFormat": "mp4",
                "MediaSampleRateHertz": 48_000,
                "OutputBucketName": "example-output.bucket",
                "OutputKey": "elsewhere.json",
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2},
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
        },
        Row {
            case: "a retry carrying tags where the first carried none",
            first: json!({
                "MedicalTranscriptionJobName": "name-lifetime-tags-9",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.m4a"},
                "MediaFormat": "m4a",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
            second: json!({
                "MedicalTranscriptionJobName": "name-lifetime-tags-9",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.m4a"},
                "MediaFormat": "m4a",
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION",
                "Tags": [{"Key": "clinic", "Value": "cardiology"}]
            }),
        },
    ]
}

/// The job name a start request asks for.
fn job_name(request: &Value) -> String {
    request["MedicalTranscriptionJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// `GetMedicalTranscriptionJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> crate::transcription_jobs::Reply {
    transcribe
        .call(
            "GetMedicalTranscriptionJob",
            json!({"MedicalTranscriptionJobName": name}),
        )
        .await
}

#[tokio::test]
async fn a_name_held_by_a_finished_medical_job_is_still_a_conflict() {
    let transcribe = Transcribe::new();

    for row in rows() {
        let started = transcribe
            .call("StartMedicalTranscriptionJob", row.first.clone())
            .await;
        let job = started.ok(row.case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a job just started must be IN_PROGRESS, got {:?}",
            job["TranscriptionJobStatus"],
            case = row.case
        );
    }

    // One wait for every row, not one per row: the transition the clock owes each job
    // falls due at the same span after its own start.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let name = job_name(&row.first);

        // The job is done: its slot is free, and the record — and so its name — is not.
        let finished = get(&transcribe, &name).await;
        let before = finished.ok(case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            before["TranscriptionJobStatus"],
            json!(FINISHED_STATUS),
            "case {case:?}: the processing span has elapsed, so the job must have \
             reached its terminal status, got {:?}",
            before["TranscriptionJobStatus"]
        );

        let conflict = transcribe
            .call("StartMedicalTranscriptionJob", row.second.clone())
            .await;
        conflict.refused(case, 400, "ConflictException");
        let message = conflict.error_message(case);
        assert_eq!(
            message, NAME_TAKEN,
            "case {case:?}: unexpected conflict message, got {message:?}"
        );

        // The refused start stored nothing: the record still answers with what the
        // FIRST request asked for, down to the instant it was created.
        let after = get(&transcribe, &name).await;
        let after = after.ok(case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            after, before,
            "case {case:?}: a refused start must leave the stored job exactly as it \
             was, got {after:?}"
        );
        for member in ["Media", "MediaFormat", "Type", "MediaSampleRateHertz", "Tags"] {
            let asked = row.first.get(member).cloned().unwrap_or(Value::Null);
            if asked.is_null() {
                continue;
            }
            assert_eq!(
                after[member], asked,
                "case {case:?}: the stored {member} must still be the first request's, \
                 got {:?}",
                after[member]
            );
        }

        // The refusal is scoped to the NAME: the very body just refused is accepted
        // under a name nothing has taken, and reads back as its own job.
        let free_name = format!("after-the-conflict-{index}");
        let mut retry = row.second.clone();
        retry["MedicalTranscriptionJobName"] = json!(free_name);
        let accepted = transcribe
            .call("StartMedicalTranscriptionJob", retry.clone())
            .await;
        assert_eq!(
            accepted.ok(case)["MedicalTranscriptionJob"]["TranscriptionJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a start under a free name must be accepted, got {:?}",
            accepted.body
        );

        let fetched = get(&transcribe, &free_name).await;
        let job = fetched.ok(case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            job["Media"], retry["Media"],
            "case {case:?}: the new job must report the media it was started with, \
             got {:?}",
            job["Media"]
        );
        assert_eq!(
            job["MedicalTranscriptionJobName"],
            json!(free_name),
            "case {case:?}: the new job must report its own name, got {:?}",
            job["MedicalTranscriptionJobName"]
        );
    }
}

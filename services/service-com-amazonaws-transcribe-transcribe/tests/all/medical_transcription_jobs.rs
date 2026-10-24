// SPDX-License-Identifier: CC0-1.0

//! The MEDICAL transcription-job family (Amazon Transcribe Medical), end to end over
//! the wire: `StartMedicalTranscriptionJob` -> `GetMedicalTranscriptionJob`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler bodies.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: a medical transcription job is a stored record of what the
//! start request asked for, `IN_PROGRESS` from the moment it is started and moved to
//! `COMPLETED` by the CLOCK, readable by name through `GetMedicalTranscriptionJob`
//! exactly as it was stored — with the transcript reported at the location the
//! documented `OutputKey` rule puts it in the bucket the request named — while a name
//! this account never started is refused and a second start under a live name is a
//! conflict. Each test asserts that over a table spanning the request class (media URI
//! scheme and format, each published `MediaSampleRateHertz` bound, every form of
//! `OutputKey`, both `Type` values, PHI content identification, the optional settings,
//! encryption and tag count), not over one row.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::Transcribe;

/// Long enough for the `IN_PROGRESS -> COMPLETED` transition of a job started during
/// the test to have fallen due: one span of `crate::state::JOB_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The members `StartMedicalTranscriptionJob` accepts that the
/// `MedicalTranscriptionJob` shape also carries, so a stored job must read them back
/// unchanged (`smithy_query(query="shape", shape="MedicalTranscriptionJob")`).
const ECHOED_MEMBERS: &[&str] = &[
    "MedicalTranscriptionJobName",
    "LanguageCode",
    "MediaSampleRateHertz",
    "MediaFormat",
    "Media",
    "Settings",
    "ContentIdentificationType",
    "Specialty",
    "Type",
    "Tags",
];

/// The class of job a `StartMedicalTranscriptionJob` request can describe, one row per
/// axis the service's rules generalise over: the media URI scheme and format, each
/// published bound of `MediaSampleRateHertz` (`@range(min: 16000, max: 48000)`), each
/// form `OutputKey` can take (absent, a file name, a prefix, a prefix with a trailing
/// separator), both values of `Type` (`CONVERSATION | DICTATION`), whether PHI content
/// identification is asked for, which optional settings are set, whether the output is
/// KMS-encrypted, and how many tags are attached.
fn start_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a dictation at the lowest published sample rate, no OutputKey, no tags",
            json!({
                "MedicalTranscriptionJobName": "medical-minimal",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "MediaFormat": "wav",
                "MediaSampleRateHertz": 16_000,
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
        (
            "a conversation at the highest published sample rate, an OutputKey naming a \
             file, PHI identification, speaker partitioning, KMS encryption, two tags",
            json!({
                "MedicalTranscriptionJobName": "medical.job_2",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "https://example.invalid/media/visit.mp4"},
                "MediaFormat": "mp4",
                "MediaSampleRateHertz": 48_000,
                "OutputBucketName": "example-output.bucket",
                "OutputKey": "my-transcript.json",
                "OutputEncryptionKMSKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "KMSEncryptionContext": {"department": "cardiology"},
                "ContentIdentificationType": "PHI",
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 2,
                    "VocabularyName": "clinic-terms"
                },
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION",
                "Tags": [{"Key": "clinic", "Value": "cardiology"}, {"Key": "cost", "Value": "7"}]
            }),
        ),
        (
            "a conversation with an OutputKey naming a prefix, channel identification \
             and one tag",
            json!({
                "MedicalTranscriptionJobName": "medical_channels-3",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.flac"},
                "MediaFormat": "flac",
                "OutputBucketName": "example-medical-output",
                "OutputKey": "my-transcript",
                "Settings": {"ChannelIdentification": true},
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
        ),
        (
            "a dictation with an OutputKey naming a prefix with a trailing separator, \
             alternative transcriptions, no media format",
            json!({
                "MedicalTranscriptionJobName": "m",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/notes.mp3"},
                "OutputBucketName": "example-medical-output",
                "OutputKey": "clinic/visits/",
                "Settings": {"ShowAlternatives": true, "MaxAlternatives": 3},
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
    ]
}

/// The job name of a start request.
fn job_name(request: &Value) -> String {
    request["MedicalTranscriptionJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// Where a completed job's transcript is reported, per the documented `OutputKey`
/// rule: "The default name for your transcription output is the same as the name you
/// specified for your medical transcription job", an `OutputKey` naming a file is used
/// as given, and an `OutputKey` naming a prefix holds a file named for the job (model
/// doc for `StartMedicalTranscriptionJobRequest$OutputKey`). `OutputBucketName` is
/// required for this family, so the transcript is always in the caller's bucket.
fn expected_transcript_uri(request: &Value) -> String {
    let bucket = request["OutputBucketName"]
        .as_str()
        .expect("every row names its output bucket");
    let name = job_name(request);
    match request.get("OutputKey").and_then(Value::as_str) {
        None => format!("s3://{bucket}/{name}.json"),
        Some(key) if key.ends_with(".json") => format!("s3://{bucket}/{key}"),
        Some(key) => format!("s3://{bucket}/{prefix}/{name}.json", prefix = key.trim_end_matches('/')),
    }
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
async fn a_started_medical_transcription_job_reads_back_exactly_as_it_was_started() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        let started = transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await;
        let job = started.ok(case)["MedicalTranscriptionJob"].clone();

        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a job just started must be IN_PROGRESS, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert!(
            job["CreationTime"].is_number() && job["StartTime"].is_number(),
            "case {case:?}: CreationTime and StartTime must be set, got {:?} and {:?}",
            job["CreationTime"],
            job["StartTime"]
        );
        assert!(
            job["CompletionTime"].is_null() && job["Transcript"].is_null(),
            "case {case:?}: a job still processing has no completion time and no \
             transcript, got {:?} and {:?}",
            job["CompletionTime"],
            job["Transcript"]
        );
        assert!(
            job["FailureReason"].is_null(),
            "case {case:?}: an accepted job has no FailureReason, got {:?}",
            job["FailureReason"]
        );

        let name = job_name(&request);
        let fetched = get(&transcribe, &name).await;
        let fetched = fetched.ok(case)["MedicalTranscriptionJob"].clone();

        for member in ECHOED_MEMBERS {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    fetched.get(*member),
                    Some(sent),
                    "case {case:?}: GetMedicalTranscriptionJob must return the stored \
                     {member}, got {:?}",
                    fetched.get(*member)
                );
            }
        }
        // A job carrying no tags reports no `Tags` at all: `TagList` is modelled
        // `length {min 1, max 200}`, so an empty list is not a value of that shape.
        if request.get("Tags").is_none() {
            assert!(
                fetched["Tags"].is_null(),
                "case {case:?}: a job started with no tags must report no Tags, got {:?}",
                fetched["Tags"]
            );
        }
        // A member the request left out is absent from the answer rather than
        // defaulted: nothing is invented for it.
        for member in ["MediaFormat", "MediaSampleRateHertz", "Settings"] {
            if request.get(member).is_none() {
                assert!(
                    fetched[member].is_null(),
                    "case {case:?}: {member} was not asked for, so it must be absent, \
                     got {:?}",
                    fetched[member]
                );
            }
        }
        assert_eq!(
            fetched["CreationTime"], job["CreationTime"],
            "case {case:?}: CreationTime must be stable across reads, got {:?} then {:?}",
            job["CreationTime"], fetched["CreationTime"]
        );
    }
}

#[tokio::test]
async fn a_medical_transcription_job_completes_once_its_processing_span_has_elapsed() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await
            .ok(case);
    }

    // One wait for every row, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in start_requests() {
        let name = job_name(&request);

        let fetched = get(&transcribe, &name).await;
        let job = fetched.ok(case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the processing span has elapsed, so the job must be \
             COMPLETED, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert!(
            job["CompletionTime"].is_number(),
            "case {case:?}: a completed job reports a CompletionTime, got {:?}",
            job["CompletionTime"]
        );
        assert_eq!(
            job["Transcript"],
            json!({"TranscriptFileUri": expected_transcript_uri(&request)}),
            "case {case:?}: unexpected Transcript, got {:?}",
            job["Transcript"]
        );

        // A read does not change what a later read sees: the completion instant and
        // the reported transcript stay as they were.
        let again = get(&transcribe, &name).await;
        let again = again.ok(case)["MedicalTranscriptionJob"].clone();
        assert_eq!(
            again, job,
            "case {case:?}: a second read must report the same job, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_medical_transcription_job_this_account_never_started_is_refused() {
    let transcribe = Transcribe::new();

    // The name forms `TranscriptionJobName`'s pattern allows (`^[0-9a-zA-Z._-]+$`),
    // including both published length bounds, none of them ever started here.
    let names = [
        "never-started",
        "a",
        "never.started_2",
        "9",
        &"n".repeat(200),
    ];
    for name in names {
        let reply = get(&transcribe, name).await;
        let case = format!("GetMedicalTranscriptionJob {name:?}");
        reply.refused(&case, 400, "BadRequestException");
        // The service's own not-found text names the KIND of resource and never
        // echoes the name the caller asked about.
        let message = reply.error_message(&case);
        assert_eq!(
            message,
            "The requested job couldn't be found. Check the job name and try your \
             request again.",
            "case {case:?}: unexpected not-found message, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_start_whose_members_cannot_hold_together_is_refused() {
    let transcribe = Transcribe::new();

    // One row per rule the operation documents that the model's constraint traits
    // cannot express: a `Media` naming no media file, and a language other than the
    // one language medical transcription runs in. The language rows span the
    // service-wide `LanguageCode` enum this member targets — a different English
    // locale, another language entirely — none of which is en-US.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a Media that names no media file",
            json!({
                "MedicalTranscriptionJobName": "refused-1",
                "LanguageCode": "en-US",
                "Media": {},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
        (
            "a British English job",
            json!({
                "MedicalTranscriptionJobName": "refused-2",
                "LanguageCode": "en-GB",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
        (
            "a US Spanish job",
            json!({
                "MedicalTranscriptionJobName": "refused-3",
                "LanguageCode": "es-US",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION"
            }),
        ),
        (
            "a German job",
            json!({
                "MedicalTranscriptionJobName": "refused-4",
                "LanguageCode": "de-DE",
                "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
    ];

    for (case, request) in &rows {
        let refused = transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await;
        refused.refused(case, 400, "BadRequestException");

        // A refused start stores nothing, so the name it asked for is unknown.
        get(&transcribe, &job_name(request))
            .await
            .refused(case, 400, "BadRequestException");
    }

    // A member outside a constraint the model DOES express is refused by the boundary
    // validator, before the handler runs: `MedicalMediaSampleRateHertz` is
    // `@range(min: 16000, max: 48000)`, so one past each published bound is a
    // ValidationException.
    for hertz in [15_999, 48_001] {
        let case = format!("MediaSampleRateHertz {hertz}");
        transcribe
            .call(
                "StartMedicalTranscriptionJob",
                json!({
                    "MedicalTranscriptionJobName": "refused-range",
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
                    "MediaSampleRateHertz": hertz,
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                }),
            )
            .await
            .refused(&case, 400, "ValidationException");
        get(&transcribe, "refused-range")
            .await
            .refused(&case, 400, "BadRequestException");
    }
}

#[tokio::test]
async fn a_second_start_under_a_live_job_name_is_a_conflict() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await
            .ok(case);

        // "must be unique within an Amazon Web Services account. If you try to create
        // a new job with the same name as an existing job, you get a
        // ConflictException error."
        let conflict = transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await;
        conflict.refused(case, 400, "ConflictException");

        // The first job is untouched by the refused second start.
        let fetched = get(&transcribe, &job_name(&request)).await;
        assert_eq!(
            fetched.ok(case)["MedicalTranscriptionJob"]["Media"],
            request["Media"],
            "case {case:?}: a refused start must not rewrite the stored job, got {:?}",
            fetched.body
        );
    }
}

/// The medical transcription jobs are a family of their OWN: the ARN resource type
/// `medical-transcription-job` is distinct from `transcription-job`, so a name may be
/// in use in both families at once and each read answers its own record.
#[tokio::test]
async fn a_medical_job_and_a_transcription_job_may_share_a_name() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        let name = job_name(&request);
        transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await
            .ok(case);
        transcribe
            .call(
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": name.clone(),
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/not-medical.wav"}
                }),
            )
            .await
            .ok(case);

        let medical = get(&transcribe, &name).await;
        assert_eq!(
            medical.ok(case)["MedicalTranscriptionJob"]["Media"],
            request["Media"],
            "case {case:?}: the medical read must answer the medical record, got {:?}",
            medical.body
        );
        let plain = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        assert_eq!(
            plain.ok(case)["TranscriptionJob"]["Media"],
            json!({"MediaFileUri": "s3://example-media/not-medical.wav"}),
            "case {case:?}: the non-medical read must answer its own record, got {:?}",
            plain.body
        );
    }
}

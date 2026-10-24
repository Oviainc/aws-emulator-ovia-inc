// SPDX-License-Identifier: CC0-1.0

//! The Medical Scribe (AWS HealthScribe) job family, end to end over the wire:
//! `StartMedicalScribeJob` -> `GetMedicalScribeJob` -> `DeleteMedicalScribeJob`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time; this suite adds only what is specific to the Medical Scribe family.
//!
//! THE RULE UNDER TEST: a Medical Scribe job is a stored record whose status the clock
//! moves from `IN_PROGRESS` to `COMPLETED`, readable by name until it is deleted, and
//! deletable exactly once and only once it is in a terminal status — after which the
//! name is unknown to a read and free for a new start. Each test asserts that over a
//! table spanning the request class (media URI scheme, speaker partitioning vs channel
//! identification, each published `MaxSpeakerLabels` bound, vocabulary filtering,
//! clinical-note template, encryption, tag count, patient context), not over one row.

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

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// The members `StartMedicalScribeJob` accepts that the `MedicalScribeJob` shape also
/// carries, so a stored job must read them back unchanged
/// (`smithy_query(query="shape", shape="MedicalScribeJob")`).
const ECHOED_MEMBERS: &[&str] = &[
    "MedicalScribeJobName",
    "Media",
    "Settings",
    "DataAccessRoleArn",
    "ChannelDefinitions",
    "Tags",
];

/// The class of job a `StartMedicalScribeJob` request can describe, one row per axis
/// the service's rules generalise over: the media URI scheme, which of the two
/// speaker-partitioning modes is chosen, each published bound of `MaxSpeakerLabels`
/// (`@range(min: 2, max: 30)`), whether a custom vocabulary and filter are applied,
/// which clinical-note template is asked for, whether the output is KMS-encrypted,
/// how many tags are attached, and whether a patient context is supplied.
fn start_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "speaker labels at the lowest published MaxSpeakerLabels, no tags, no context",
            json!({
                "MedicalScribeJobName": "scribe-minimal",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
            }),
        ),
        (
            "speaker labels at the highest published MaxSpeakerLabels, vocabulary filter, \
             note template, KMS encryption, two tags, patient context",
            json!({
                "MedicalScribeJobName": "scribe.job_2",
                "Media": {"MediaFileUri": "https://example.invalid/media/visit.mp4"},
                "OutputBucketName": "example-output.bucket",
                "DataAccessRoleArn": "arn:aws-us-gov:iam::111122223333:role/clinic/Scribe",
                "OutputEncryptionKMSKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "KMSEncryptionContext": {"department": "cardiology"},
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 30,
                    "VocabularyName": "clinic-terms",
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "mask",
                    // "BIRP" is a value of `MedicalScribeNoteTemplate`
                    // (`smithy_query(query="shape", shape="MedicalScribeNoteTemplate")`),
                    // not the default-looking HISTORY_AND_PHYSICAL.
                    "ClinicalNoteGenerationSettings": {"NoteTemplate": "BIRP"}
                },
                "Tags": [{"Key": "clinic", "Value": "cardiology"}, {"Key": "cost", "Value": "7"}],
                "MedicalScribeContext": {"PatientContext": {"Pronouns": "THEY_THEM"}}
            }),
        ),
        (
            "channel identification with the two channel definitions it requires",
            json!({
                "MedicalScribeJobName": "scribe_channels-3",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ChannelIdentification": true},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ],
                "Tags": [{"Key": "clinic", "Value": "primary-care"}]
            }),
        ),
    ]
}

/// The job name of a start request.
fn job_name(request: &Value) -> String {
    request["MedicalScribeJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// `GetMedicalScribeJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> crate::transcription_jobs::Reply {
    transcribe
        .call("GetMedicalScribeJob", json!({"MedicalScribeJobName": name}))
        .await
}

/// `DeleteMedicalScribeJob` for `name`.
async fn delete(transcribe: &Transcribe, name: &str) -> crate::transcription_jobs::Reply {
    transcribe
        .call(
            "DeleteMedicalScribeJob",
            json!({"MedicalScribeJobName": name}),
        )
        .await
}

#[tokio::test]
async fn a_started_medical_scribe_job_reads_back_exactly_as_it_was_started() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        let started = transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await;
        let job = started.ok(case)["MedicalScribeJob"].clone();

        assert_eq!(
            job["MedicalScribeJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a job just started must be IN_PROGRESS, got {:?}",
            job["MedicalScribeJobStatus"]
        );
        // `MedicalScribeLanguageCode` has the single value `en-US`, and the start
        // request carries no language member, so that is the language of every job.
        assert_eq!(
            job["LanguageCode"],
            json!("en-US"),
            "case {case:?}: expected the only modelled LanguageCode en-US, got {:?}",
            job["LanguageCode"]
        );
        assert!(
            job["CreationTime"].is_number() && job["StartTime"].is_number(),
            "case {case:?}: CreationTime and StartTime must be set, got {:?} and {:?}",
            job["CreationTime"],
            job["StartTime"]
        );
        assert!(
            job["CompletionTime"].is_null() && job["MedicalScribeOutput"].is_null(),
            "case {case:?}: a job still processing has no completion time and no output, \
             got {:?} and {:?}",
            job["CompletionTime"],
            job["MedicalScribeOutput"]
        );
        assert!(
            job["FailureReason"].is_null(),
            "case {case:?}: an accepted job has no FailureReason, got {:?}",
            job["FailureReason"]
        );
        assert_eq!(
            job["MedicalScribeContextProvided"],
            json!(request.get("MedicalScribeContext").is_some()),
            "case {case:?}: MedicalScribeContextProvided must say whether the request \
             carried a context, got {:?}",
            job["MedicalScribeContextProvided"]
        );

        let name = job_name(&request);
        let fetched = get(&transcribe, &name).await;
        let fetched = fetched.ok(case)["MedicalScribeJob"].clone();

        for member in ECHOED_MEMBERS {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    fetched.get(*member),
                    Some(sent),
                    "case {case:?}: GetMedicalScribeJob must return the stored {member}, got {:?}",
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
        assert_eq!(
            fetched["CreationTime"], job["CreationTime"],
            "case {case:?}: CreationTime must be stable across reads, got {:?} then {:?}",
            job["CreationTime"], fetched["CreationTime"]
        );
    }
}

#[tokio::test]
async fn a_medical_scribe_job_completes_once_its_processing_span_has_elapsed() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);
    }

    // One wait for every row, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in start_requests() {
        let name = job_name(&request);
        let bucket = request["OutputBucketName"]
            .as_str()
            .expect("every row names its output bucket")
            .to_string();

        let fetched = get(&transcribe, &name).await;
        let job = fetched.ok(case)["MedicalScribeJob"].clone();
        assert_eq!(
            job["MedicalScribeJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the processing span has elapsed, so the job must be COMPLETED, \
             got {:?}",
            job["MedicalScribeJobStatus"]
        );
        assert!(
            job["CompletionTime"].is_number(),
            "case {case:?}: a completed job reports a CompletionTime, got {:?}",
            job["CompletionTime"]
        );
        // Both output URIs are in the bucket the request named, under a prefix named
        // for the job (see `crate::state::medical_scribe_output_uri`).
        assert_eq!(
            job["MedicalScribeOutput"],
            json!({
                "TranscriptFileUri": format!("s3://{bucket}/{name}/transcript.json"),
                "ClinicalDocumentUri": format!("s3://{bucket}/{name}/summary.json")
            }),
            "case {case:?}: unexpected MedicalScribeOutput, got {:?}",
            job["MedicalScribeOutput"]
        );

        // A read does not change what a later read sees: the completion instant and
        // the reported output stay as they were.
        let again = get(&transcribe, &name).await;
        let again = again.ok(case)["MedicalScribeJob"].clone();
        assert_eq!(
            again, job,
            "case {case:?}: a second read must report the same job, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_terminal_medical_scribe_job_is_deletable_exactly_once() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);
    }
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in start_requests() {
        let name = job_name(&request);

        let deleted = delete(&transcribe, &name).await;
        deleted.ok_status(case, 200);

        // The record is gone: a read of the name it was stored under is the service's
        // not-found refusal, and so is a second delete.
        get(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");
        delete(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");

        // Deleting is what makes the name free again, so the same request starts a
        // new job under it.
        let restarted = transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await;
        assert_eq!(
            restarted.ok(case)["MedicalScribeJob"]["MedicalScribeJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: the deleted name must be startable again, got {:?}",
            restarted.body
        );
    }
}

#[tokio::test]
async fn a_medical_scribe_job_the_service_is_still_working_on_is_not_deletable() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        let name = job_name(&request);
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);

        // Straight after the start the job is IN_PROGRESS — a non-terminal status, so
        // the delete is refused and the record survives it.
        let refused = delete(&transcribe, &name).await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert!(
            message.contains("IN_PROGRESS"),
            "case {case:?}: the refusal must name the state it refused for, got {message:?}"
        );

        let survived = get(&transcribe, &name).await;
        assert_eq!(
            survived.ok(case)["MedicalScribeJob"]["MedicalScribeJobName"],
            json!(name.clone()),
            "case {case:?}: a refused delete must leave the job stored, got {:?}",
            survived.body
        );
    }

    // Once the clock owes them COMPLETED, the same deletes succeed.
    std::thread::sleep(PAST_THE_TRANSITION);
    for (case, request) in start_requests() {
        delete(&transcribe, &job_name(&request))
            .await
            .ok_status(case, 200);
    }
}

#[tokio::test]
async fn a_medical_scribe_job_this_account_never_started_is_refused() {
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
        for operation in ["GetMedicalScribeJob", "DeleteMedicalScribeJob"] {
            let reply = transcribe
                .call(operation, json!({"MedicalScribeJobName": name}))
                .await;
            let case = format!("{operation} {name:?}");
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
}

#[tokio::test]
async fn a_start_whose_members_cannot_hold_together_is_refused() {
    let transcribe = Transcribe::new();

    // One row per combination the operation documents but the model's constraint
    // traits cannot express.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a Media that names no media file",
            json!({
                "MedicalScribeJobName": "refused-1",
                "Media": {},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
            }),
        ),
        (
            "neither speaker labels nor channel identification",
            json!({
                "MedicalScribeJobName": "refused-2",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {}
            }),
        ),
        (
            "both speaker labels and channel identification",
            json!({
                "MedicalScribeJobName": "refused-3",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 2,
                    "ChannelIdentification": true
                },
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ]
            }),
        ),
        (
            "speaker labels without MaxSpeakerLabels",
            json!({
                "MedicalScribeJobName": "refused-4",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true}
            }),
        ),
        (
            "channel identification without the channel definitions",
            json!({
                "MedicalScribeJobName": "refused-5",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ChannelIdentification": true}
            }),
        ),
        (
            "channel definitions without channel identification",
            json!({
                "MedicalScribeJobName": "refused-6",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ]
            }),
        ),
        (
            "a vocabulary filter without its filter method",
            json!({
                "MedicalScribeJobName": "refused-7",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 2,
                    "VocabularyFilterName": "blocked-terms"
                }
            }),
        ),
        (
            "two channel definitions naming the same channel",
            json!({
                "MedicalScribeJobName": "refused-8",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ChannelIdentification": true},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 0, "ParticipantRole": "PATIENT"}
                ]
            }),
        ),
        (
            "two channel definitions naming the same participant role",
            json!({
                "MedicalScribeJobName": "refused-9",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ChannelIdentification": true},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "CLINICIAN"}
                ]
            }),
        ),
    ];

    for (case, request) in &rows {
        let refused = transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await;
        refused.refused(case, 400, "BadRequestException");

        // A refused start stores nothing, so the name it asked for is unknown.
        delete(&transcribe, &job_name(request))
            .await
            .refused(case, 400, "BadRequestException");
    }

    // A member outside a constraint the model DOES express is refused by the boundary
    // validator, before the handler runs: `MaxSpeakers` is `@range(min: 2, max: 30)`,
    // so one past each published bound is a ValidationException.
    for speakers in [1, 31] {
        let case = format!("MaxSpeakerLabels {speakers}");
        transcribe
            .call(
                "StartMedicalScribeJob",
                json!({
                    "MedicalScribeJobName": "refused-range",
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": speakers}
                }),
            )
            .await
            .refused(&case, 400, "ValidationException");
    }
}

#[tokio::test]
async fn a_second_start_under_a_live_job_name_is_a_conflict() {
    let transcribe = Transcribe::new();

    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);

        // "must be unique within an Amazon Web Services account. If you try to create
        // a new job with the same name as an existing job, you get a
        // ConflictException error."
        let conflict = transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await;
        conflict.refused(case, 400, "ConflictException");

        // The first job is untouched by the refused second start.
        let fetched = get(&transcribe, &job_name(&request)).await;
        assert_eq!(
            fetched.ok(case)["MedicalScribeJob"]["Media"],
            request["Media"],
            "case {case:?}: a refused start must not rewrite the stored job, got {:?}",
            fetched.body
        );
    }
}

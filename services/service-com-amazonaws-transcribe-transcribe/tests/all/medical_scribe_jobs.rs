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
//! The name a job is read, started and deleted by is the one the start request
//! spelled, case included, so spellings differing only in letter case name different
//! jobs.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_NOTE_TEMPLATE;

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
///
/// `Settings` is not among them: its `ClinicalNoteGenerationSettings.NoteTemplate`
/// documents a default, which the job is RECORDED with, so the settings read back are
/// the requested ones with that default materialised — asserted on its own below, the
/// way the transcription suite asserts `SubtitlesOutput.OutputStartIndex`.
const ECHOED_MEMBERS: &[&str] = &[
    "MedicalScribeJobName",
    "Media",
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

/// The smallest start request that names `name` and `media_uri`, for the rows that
/// vary only the job NAME.
fn start_request(name: &str, media_uri: &str) -> Value {
    json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": media_uri},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
    })
}

/// Pairs of job names that differ from each other ONLY in letter case, one row per
/// form `TranscriptionJobName` allows (`@pattern ^[0-9a-zA-Z._-]+$`,
/// `@length(min: 1, max: 200)`): a plain name, a name carrying the pattern's
/// punctuation and digits, a single character, and a name at the published upper
/// length bound.
fn case_variant_names() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "a plain name, lowercase vs uppercase",
            "scribe-case".to_string(),
            "SCRIBE-CASE".to_string(),
        ),
        (
            "a name with the pattern's punctuation and digits, differing in one letter",
            "scribe.case_7".to_string(),
            "scribe.Case_7".to_string(),
        ),
        (
            "a single-character name, the lowest published length bound",
            "c".to_string(),
            "C".to_string(),
        ),
        (
            "a 200-character name, the highest published length bound, differing in its \
             last letter",
            format!("{}a", "s".repeat(199)),
            format!("{}A", "s".repeat(199)),
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

/// Create the custom resources the rows of [`start_requests`] NAME, so each row
/// applies a custom vocabulary and vocabulary filter this account holds.
///
/// A start may only name custom resources that exist — "The name of the custom
/// vocabulary you want to include in your Medical Scribe request", and the same
/// sentence for the filter (`smithy_query(query="shape",
/// shape="MedicalScribeSettings")`) — so a suite that starts a job naming them creates
/// them first.
async fn create_named_resources(transcribe: &Transcribe) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "clinic-terms",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok("CreateVocabulary clinic-terms");
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "blocked-terms",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok("CreateVocabularyFilter blocked-terms");
    // A start may only apply a custom vocabulary once its build has finished: it is
    // READY that makes one usable, and a create leaves it PENDING.
    transcribe.vocabulary_ready("clinic-terms", None).await;
}

#[tokio::test]
async fn a_started_medical_scribe_job_reads_back_exactly_as_it_was_started() {
    let transcribe = Transcribe::new();
    create_named_resources(&transcribe).await;

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
        // Every settings member reads back as the request sent it, except the
        // clinical-note template: "The default is HISTORY_AND_PHYSICAL" (model doc for
        // `ClinicalNoteGenerationSettings$NoteTemplate`), and a documented default is
        // materialised on the record, so a start that named no template reads back the
        // one its job's note will be summarised with.
        let mut expected_settings = request["Settings"].clone();
        let requested_template = request["Settings"]["ClinicalNoteGenerationSettings"]
            ["NoteTemplate"]
            .as_str()
            .unwrap_or(DEFAULT_NOTE_TEMPLATE)
            .to_string();
        expected_settings
            .as_object_mut()
            .expect("every row's Settings are a JSON object")
            .insert(
                "ClinicalNoteGenerationSettings".to_string(),
                json!({"NoteTemplate": requested_template}),
            );
        assert_eq!(
            fetched["Settings"], expected_settings,
            "case {case:?}: expected the recorded settings {expected_settings:?}, got {:?}",
            fetched["Settings"]
        );

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
    create_named_resources(&transcribe).await;

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
    create_named_resources(&transcribe).await;

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
    create_named_resources(&transcribe).await;

    for (case, request) in start_requests() {
        let name = job_name(&request);
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);

        // Straight after the start the job is IN_PROGRESS — a non-terminal status, so
        // the delete is refused and the record survives it. The refusal is the job
        // families' own fixed sentence for that condition, which carries neither the
        // name asked about nor the status the job is in.
        let refused = delete(&transcribe, &name).await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message, "The job couldn't be deleted. Make sure it exists and isn't pending.",
            "case {case:?}: expected the job family's non-terminal delete refusal, got \
             {message:?}"
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
    create_named_resources(&transcribe).await;

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

/// A job is identified by its name EXACTLY as the start request spelled it
/// ("Job names are case sensitive",
/// `smithy_query(query="doc", shape="GetMedicalScribeJobRequest", member="MedicalScribeJobName")`),
/// so two spellings differing only in letter case are two different jobs: a read of
/// one never reports the other, starting the second spelling is no conflict with the
/// first, and deleting one leaves the other stored.
#[tokio::test]
async fn medical_scribe_job_names_differing_only_in_case_are_different_jobs() {
    const FIRST_MEDIA: &str = "s3://example-media/first-spelling.flac";
    const SECOND_MEDIA: &str = "s3://example-media/second-spelling.wav";

    let transcribe = Transcribe::new();

    for (case, first, second) in case_variant_names() {
        transcribe
            .call("StartMedicalScribeJob", start_request(&first, FIRST_MEDIA))
            .await
            .ok(case);

        // Only the first spelling has been started, so the other name is unknown.
        get(&transcribe, &second)
            .await
            .refused(case, 400, "BadRequestException");

        // ... and starting it is therefore not a second start of the same job.
        transcribe
            .call("StartMedicalScribeJob", start_request(&second, SECOND_MEDIA))
            .await
            .ok(case);

        // Each spelling reads back its own record, named exactly as it was started.
        for (name, media) in [(&first, FIRST_MEDIA), (&second, SECOND_MEDIA)] {
            let fetched = get(&transcribe, name).await;
            let job = fetched.ok(case)["MedicalScribeJob"].clone();
            assert_eq!(
                job["MedicalScribeJobName"],
                json!(name),
                "case {case:?}: expected the job named {name:?}, got {:?}",
                job["MedicalScribeJobName"]
            );
            assert_eq!(
                job["Media"]["MediaFileUri"],
                json!(media),
                "case {case:?}: the job named {name:?} must report its own media {media:?}, \
                 got {:?}",
                job["Media"]["MediaFileUri"]
            );
        }
    }

    // One wait for every row, so each job is terminal and therefore deletable.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, first, second) in case_variant_names() {
        delete(&transcribe, &first).await.ok_status(case, 200);

        // Deleting one spelling removes only that record.
        get(&transcribe, &first)
            .await
            .refused(case, 400, "BadRequestException");
        let survivor = get(&transcribe, &second).await;
        assert_eq!(
            survivor.ok(case)["MedicalScribeJob"]["Media"]["MediaFileUri"],
            json!(SECOND_MEDIA),
            "case {case:?}: deleting {first:?} must leave {second:?} stored, got {:?}",
            survivor.body
        );
    }
}

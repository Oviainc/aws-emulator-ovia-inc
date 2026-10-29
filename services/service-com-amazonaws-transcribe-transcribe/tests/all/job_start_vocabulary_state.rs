// SPDX-License-Identifier: CC0-1.0

//! The STATE a custom vocabulary has to be in for a job to apply it, end to end over
//! the wire.
//!
//! THE RULE UNDER TEST: a start may apply a custom vocabulary only once the service has
//! finished building it successfully — `READY` is that state ("If the state is READY,
//! you can use the custom vocabulary", `smithy_query(query="shape",
//! shape="VocabularyState")`) — so a start naming one whose build is still running
//! (`PENDING`) or did not produce any terms (`FAILED`) is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores no job, while the same start, once the
//! named vocabulary has settled `READY`, is served.
//!
//! The rule is asserted over a table spanning what it generalises over: every start
//! operation whose settings can name a custom vocabulary, each of the member paths that
//! carries such a name (the job's own `Settings` and the per-language
//! `LanguageIdSettings` entry), both states that are not `READY`, how the vocabulary's
//! terms were to be sourced (the terms the request listed vs a file), and the name forms
//! the member's `@pattern`/`@length` admit.
//!
//! A custom vocabulary FILTER is the counter-case the same table pins: that family
//! models no processing state anywhere, so a held filter is applicable the moment it is
//! created.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::transcription_jobs::{not_usable_message as state_refusal, Reply, Transcribe};

/// A `DataAccessRoleArn` of the form the member's `@pattern` pins, for the operations
/// that model one `@required`.
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// One start operation, and one member path of it that names a custom vocabulary.
struct Reference {
    case: &'static str,
    /// The start operation.
    start: &'static str,
    /// The start request applying the custom vocabulary named by the second argument to
    /// the job named by the first.
    request: fn(&str, &str) -> Value,
    /// The operation that reads a job of this family back, and the member it names the
    /// job by — so a refused start can be shown to have stored nothing.
    read: &'static str,
    name_member: &'static str,
}

/// Every member path of every start operation that names a custom vocabulary of the
/// (non-medical) custom vocabulary family: the job's own `Settings`, and the
/// per-language `LanguageIdSettings` entry of the two operations that model one.
fn references() -> Vec<Reference> {
    vec![
        Reference {
            case: "StartTranscriptionJob Settings.VocabularyName",
            start: "StartTranscriptionJob",
            request: |job, vocabulary| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                    "Settings": {"VocabularyName": vocabulary}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
        },
        Reference {
            case: "StartTranscriptionJob LanguageIdSettings[en-US].VocabularyName",
            start: "StartTranscriptionJob",
            request: |job, vocabulary| {
                json!({
                    "TranscriptionJobName": job,
                    "IdentifyLanguage": true,
                    "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                    "LanguageIdSettings": {"en-US": {"VocabularyName": vocabulary}}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
        },
        Reference {
            case: "StartCallAnalyticsJob Settings.VocabularyName",
            start: "StartCallAnalyticsJob",
            request: |job, vocabulary| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                    "Settings": {"VocabularyName": vocabulary}
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
        },
        Reference {
            case: "StartCallAnalyticsJob Settings.LanguageIdSettings[en-US].VocabularyName",
            start: "StartCallAnalyticsJob",
            request: |job, vocabulary| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/call.mp3"},
                    "Settings": {
                        "LanguageOptions": ["en-US", "es-US"],
                        "LanguageIdSettings": {"en-US": {"VocabularyName": vocabulary}}
                    }
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
        },
        Reference {
            case: "StartMedicalScribeJob Settings.VocabularyName with speaker partitioning",
            start: "StartMedicalScribeJob",
            request: |job, vocabulary| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {
                        "ShowSpeakerLabels": true,
                        "MaxSpeakerLabels": 2,
                        "VocabularyName": vocabulary
                    }
                })
            },
            read: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
        },
        Reference {
            case: "StartMedicalScribeJob Settings.VocabularyName with channel identification",
            start: "StartMedicalScribeJob",
            request: |job, vocabulary| {
                json!({
                    "MedicalScribeJobName": job,
                    "Media": {"MediaFileUri": "s3://example-media/visit.mp4"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": {
                        "ChannelIdentification": true,
                        "VocabularyName": vocabulary
                    },
                    "ChannelDefinitions": [
                        {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                        {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                    ]
                })
            },
            read: "GetMedicalScribeJob",
            name_member: "MedicalScribeJobName",
        },
    ]
}

/// The name forms the members' shared `@pattern` (`^[0-9a-zA-Z._-]+$`) admits, including
/// both published `@length` bounds (1..200) — one per row, so no row is pinned to one
/// spelling.
fn vocabulary_names() -> Vec<String> {
    vec![
        "clinic-terms".to_string(),
        "a".to_string(),
        "terms.v2_9".to_string(),
        "9".to_string(),
        "Z".repeat(200),
        "mixed.Case-99_terms".to_string(),
    ]
}

/// How this service refuses a start that names a custom vocabulary it cannot apply: the
/// name the request asked for and the state this account holds that vocabulary in.
fn not_usable_message(name: &str, state: &str) -> String {
    state_refusal("vocabulary", name, state, "READY")
}

/// `CreateVocabulary` for `name` from the terms the request LISTS, which settles `READY`.
async fn create_from_phrases(transcribe: &Transcribe, name: &str) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok(&format!("CreateVocabulary {name:?} from Phrases"));
}

/// `CreateVocabulary` for `name` from a FILE, the alternative source of terms
/// (`smithy_query(query="operation", operation="CreateVocabulary")`).
async fn create_from_file(transcribe: &Transcribe, name: &str) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": name,
                "LanguageCode": "en-US",
                "VocabularyFileUri": "s3://DOC-EXAMPLE-BUCKET/terms.txt"
            }),
        )
        .await
        .ok(&format!("CreateVocabulary {name:?} from a file"));
}

/// Read the job named `name` back through the family's own reader.
async fn read_job(transcribe: &Transcribe, reference: &Reference, name: &str) -> Reply {
    transcribe
        .call(reference.read, json!({reference.name_member: name}))
        .await
}

#[tokio::test]
async fn a_start_naming_a_vocabulary_still_being_built_is_refused_and_served_once_it_is_ready() {
    for (row, (reference, vocabulary)) in references()
        .iter()
        .zip(vocabulary_names().iter())
        .enumerate()
    {
        // A fresh emulator per row, so a vocabulary one row built cannot be what makes
        // the next row's start succeed.
        let transcribe = Transcribe::new();
        let case = reference.case;

        // Just created, the vocabulary is still being built, so no job may apply it.
        create_from_phrases(&transcribe, vocabulary).await;
        let pending_job = format!("pending-reference-{row}");
        let refused = transcribe
            .call(reference.start, (reference.request)(&pending_job, vocabulary))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_usable_message(vocabulary, "PENDING"),
            "case {case:?}: the refusal must name the state the vocabulary is in, got \
             {message:?}"
        );

        // A refused start stores nothing, so the job name it asked for is unknown.
        read_job(&transcribe, reference, &pending_job)
            .await
            .refused(case, 400, "BadRequestException");

        // Once the build has finished successfully, the very same start is served and
        // the job reads back.
        transcribe.vocabulary_ready(vocabulary, None).await;
        let ready_job = format!("ready-reference-{row}");
        transcribe
            .call(reference.start, (reference.request)(&ready_job, vocabulary))
            .await
            .ok(case);
        read_job(&transcribe, reference, &ready_job).await.ok(case);
    }
}

#[tokio::test]
async fn a_start_naming_a_vocabulary_whose_build_did_not_produce_terms_is_refused() {
    // The names are taken from the end of the table, so this test's rows are not the
    // ones the PENDING test uses.
    let names: Vec<String> = vocabulary_names().into_iter().rev().collect();

    for (row, (reference, vocabulary)) in references().iter().zip(names.iter()).enumerate() {
        let transcribe = Transcribe::new();
        let case = reference.case;

        // A vocabulary whose terms were to be READ from a file: its build settles in the
        // model's failure state, which is not a state a job may apply it in either.
        create_from_file(&transcribe, vocabulary).await;
        let settled = transcribe.vocabulary_settled(vocabulary, None).await;
        assert_eq!(
            settled, "FAILED",
            "case {case:?}: a vocabulary whose terms were not loaded must settle FAILED, \
             got {settled:?}"
        );

        let job = format!("failed-reference-{row}");
        let refused = transcribe
            .call(reference.start, (reference.request)(&job, vocabulary))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_usable_message(vocabulary, "FAILED"),
            "case {case:?}: the refusal must name the state the vocabulary is in, got \
             {message:?}"
        );
        read_job(&transcribe, reference, &job)
            .await
            .refused(case, 400, "BadRequestException");

        // The failure is terminal: a second start, after the state could only have been
        // re-read, is refused in the same words.
        let again = transcribe
            .call(reference.start, (reference.request)(&job, vocabulary))
            .await;
        again.refused(case, 400, "BadRequestException");
        let repeated = again.error_message(case);
        assert_eq!(
            repeated,
            not_usable_message(vocabulary, "FAILED"),
            "case {case:?}: a failed build does not become usable by being read, got \
             {repeated:?}"
        );
    }
}

/// The counter-case: a custom vocabulary FILTER models no processing state anywhere, so
/// a start may apply one the moment the account holds it — there is no state to wait
/// for.
#[tokio::test]
async fn a_held_vocabulary_filter_is_applicable_as_soon_as_it_is_created() {
    // One row per start operation that can name a filter, differing in the filter method
    // applied (every value of `VocabularyFilterMethod`).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "StartTranscriptionJob Settings.VocabularyFilterName",
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "filter-transcription",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
                "Settings": {
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "remove"
                }
            }),
        ),
        (
            "StartCallAnalyticsJob Settings.VocabularyFilterName",
            "StartCallAnalyticsJob",
            json!({
                "CallAnalyticsJobName": "filter-call-analytics",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "Settings": {
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "mask"
                }
            }),
        ),
        (
            "StartMedicalScribeJob Settings.VocabularyFilterName",
            "StartMedicalScribeJob",
            json!({
                "MedicalScribeJobName": "filter-medical-scribe",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 3,
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "tag"
                }
            }),
        ),
    ];

    for (case, start, request) in rows {
        let transcribe = Transcribe::new();
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
            .ok(case);
        transcribe.call(start, request).await.ok(case);
    }
}

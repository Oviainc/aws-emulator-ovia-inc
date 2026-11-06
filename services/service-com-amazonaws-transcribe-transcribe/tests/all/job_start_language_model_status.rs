// SPDX-License-Identifier: CC0-1.0

//! The STATUS a custom language model has to be in for a job to apply it, end to end
//! over the wire.
//!
//! THE RULE UNDER TEST: a start may apply a custom language model only once the service
//! has finished training it successfully — `COMPLETED` is that status ("When the status
//! displays as COMPLETED the model is ready for use", `smithy_query(query="doc",
//! shape="LanguageModel", member="ModelStatus")`) — so a start naming one whose
//! training is still running (`IN_PROGRESS`) is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores no job, while the same start, once the
//! named model has settled `COMPLETED`, is served and reads the model setting back.
//!
//! The rule is asserted over a table spanning what it generalises over: every start
//! operation whose settings can name a custom language model, each of the member paths
//! that carries such a name (the job's own `ModelSettings` / `Settings` and the
//! per-language `LanguageIdSettings` entry), the model's language and base model, and
//! the name forms `ModelName`'s own `@pattern`/`@length` admit.
//!
//! The second test is the other end of the same rule: a model whose CREATE was refused
//! — because the object store said its training data could not be read — is not held at
//! all, so a start naming it is the family's not-found refusal rather than a state one.
//! Those rows need an object store to ground the refusal in, so they run on the
//! S3-mounted emulator of `crate::language_model_training_data` (the way the assembled
//! emulator runs the two services), where the rows whose data IS staged are the
//! counter-case: a model the store answered for settles `COMPLETED` and the very same
//! start is served.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::language_model_training_data::{
    call, create_bucket, create_request, emulator, stage, TRAINING_FILE,
    UNREADABLE_S3_URI_MESSAGE,
};
use crate::language_models::{language_model_request, PAST_THE_TRANSITION};
use crate::transcription_jobs::{not_usable_message as state_refusal, Reply, Transcribe};

/// The bucket the rows of the second test stage their media in — and under whose
/// prefixes the training data of the models whose creates must be REFUSED is
/// deliberately NOT staged.
const BUCKET: &str = "transcribe-clm-start-status";

/// The media key every row of the S3-mounted half names: a start resolves its
/// `Media.MediaFileUri` against the same object store, so the file has to be there for
/// the row to be decided by the model's status and not by the media rule.
const MEDIA_KEY: &str = "media/audio.flac";

/// One start operation, and one member path of it that names a custom language model.
struct Reference {
    case: &'static str,
    /// The start operation.
    start: &'static str,
    /// The start request applying the custom language model named by the second
    /// argument to the job named by the first, with the media at the third.
    request: fn(&str, &str, &str) -> Value,
    /// The operation that reads a job of this family back, and the member it names the
    /// job by — so a refused start can be shown to have stored nothing.
    read: &'static str,
    name_member: &'static str,
    /// The member of the answer the job itself sits under.
    job_member: &'static str,
    /// Where the stored job reports the model name it applies.
    reported: fn(&Value) -> &Value,
    /// The language and base model the model this row names is trained for. The
    /// languages are entries of the model's own `CLMLanguageCode` enum and the base
    /// models the whole `BaseModelName` enum (`smithy_query(query="shape",
    /// shape="CLMLanguageCode,BaseModelName")`); a language mismatch is not an error
    /// ("There are no errors or warnings associated with a language mismatch" — the
    /// model's doc for `ModelSettings$LanguageModelName`), so each row names the
    /// language it identifies.
    language: &'static str,
    base_model: &'static str,
}

/// Every member path of every start operation that names a custom language model: the
/// job's own `ModelSettings` (`StartTranscriptionJob`) or `Settings`
/// (`StartCallAnalyticsJob`), and the per-language `LanguageIdSettings` entry of both
/// (`smithy_query(query="shape", shape="ModelSettings,CallAnalyticsJobSettings,\
/// LanguageIdSettings")`).
fn references() -> Vec<Reference> {
    vec![
        Reference {
            case: "StartTranscriptionJob ModelSettings.LanguageModelName",
            start: "StartTranscriptionJob",
            request: |job, model, media| {
                json!({
                    "TranscriptionJobName": job,
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": media},
                    "ModelSettings": {"LanguageModelName": model}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            reported: |job| &job["ModelSettings"]["LanguageModelName"],
            language: "en-US",
            base_model: "WideBand",
        },
        Reference {
            case: "StartTranscriptionJob LanguageIdSettings[hi-IN].LanguageModelName",
            start: "StartTranscriptionJob",
            request: |job, model, media| {
                json!({
                    "TranscriptionJobName": job,
                    "IdentifyLanguage": true,
                    "LanguageOptions": ["hi-IN", "en-US"],
                    "Media": {"MediaFileUri": media},
                    "LanguageIdSettings": {"hi-IN": {"LanguageModelName": model}}
                })
            },
            read: "GetTranscriptionJob",
            name_member: "TranscriptionJobName",
            job_member: "TranscriptionJob",
            reported: |job| &job["LanguageIdSettings"]["hi-IN"]["LanguageModelName"],
            language: "hi-IN",
            base_model: "NarrowBand",
        },
        Reference {
            case: "StartCallAnalyticsJob Settings.LanguageModelName",
            start: "StartCallAnalyticsJob",
            request: |job, model, media| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": media},
                    "Settings": {"LanguageModelName": model}
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            reported: |job| &job["Settings"]["LanguageModelName"],
            language: "es-US",
            base_model: "WideBand",
        },
        Reference {
            case: "StartCallAnalyticsJob Settings.LanguageIdSettings[en-GB].LanguageModelName",
            start: "StartCallAnalyticsJob",
            request: |job, model, media| {
                json!({
                    "CallAnalyticsJobName": job,
                    "Media": {"MediaFileUri": media},
                    "Settings": {
                        "LanguageOptions": ["en-GB", "en-AU"],
                        "LanguageIdSettings": {"en-GB": {"LanguageModelName": model}}
                    }
                })
            },
            read: "GetCallAnalyticsJob",
            name_member: "CallAnalyticsJobName",
            job_member: "CallAnalyticsJob",
            reported: |job| &job["Settings"]["LanguageIdSettings"]["en-GB"]["LanguageModelName"],
            language: "en-GB",
            base_model: "NarrowBand",
        },
    ]
}

/// The name forms `ModelName`'s own `@pattern` (`^[0-9a-zA-Z._-]+$`) admits, including
/// both published `@length` bounds (1..200, `smithy_query(query="shape",
/// shape="ModelName")`) — one per row, so no row is pinned to one spelling.
fn model_names() -> Vec<String> {
    vec![
        "support-calls".to_string(),
        "a".to_string(),
        "Hindi.Calls_v2".to_string(),
        "M".repeat(200),
    ]
}

/// How this service refuses a start that names a custom language model it cannot apply:
/// the name the request asked for, the status this account holds that model in, and the
/// one status the model's own documentation makes it usable in.
fn not_usable_message(name: &str, status: &str) -> String {
    state_refusal("language model", name, status, "COMPLETED")
}

/// The media URI a row of the no-object-store half names: nothing answers for it, and a
/// location no object store answered for refuses nothing.
const UNRESOLVED_MEDIA: &str = "s3://example-media/audio.flac";

/// Read the job named `name` back through the family's own reader.
async fn read_job(transcribe: &Transcribe, reference: &Reference, name: &str) -> Reply {
    transcribe
        .call(reference.read, json!({reference.name_member: name}))
        .await
}

#[tokio::test]
async fn a_start_naming_a_model_still_training_is_refused_and_served_once_it_completes() {
    for (row, (reference, model)) in references().iter().zip(model_names().iter()).enumerate() {
        // A fresh emulator per row, so a model one row trained cannot be what makes the
        // next row's start succeed.
        let transcribe = Transcribe::new();
        let case = reference.case;

        transcribe
            .call(
                "CreateLanguageModel",
                language_model_request(model, reference.language, reference.base_model),
            )
            .await
            .ok(&format!("CreateLanguageModel {model:?}"));

        // Just created, the model is still training, so no job may apply it.
        let training_job = format!("training-model-{row}");
        let refused = transcribe
            .call(
                reference.start,
                (reference.request)(&training_job, model, UNRESOLVED_MEDIA),
            )
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            not_usable_message(model, "IN_PROGRESS"),
            "case {case:?}: the refusal must name the status the model is in, got \
             {message:?}"
        );

        // A refused start stores nothing, so the job name it asked for is unknown.
        read_job(&transcribe, reference, &training_job)
            .await
            .refused(case, 400, "BadRequestException");

        // Once the training has finished successfully, the very same start is served and
        // the stored job reports the model it applies.
        transcribe.language_model_completed(model, None).await;
        let completed_job = format!("completed-model-{row}");
        transcribe
            .call(
                reference.start,
                (reference.request)(&completed_job, model, UNRESOLVED_MEDIA),
            )
            .await
            .ok(case);
        let stored = read_job(&transcribe, reference, &completed_job).await;
        let job = stored.ok(case)[reference.job_member].clone();
        let applied = (reference.reported)(&job);
        assert_eq!(
            applied,
            &json!(model),
            "case {case:?}: the stored job must report the model it applies, got \
             {applied:?}"
        );
    }
}

#[tokio::test]
async fn a_start_naming_a_model_whose_create_was_refused_finds_no_model() {
    // Transcribe with the object store beside it, so a create's training data is
    // resolved against what that store answers.
    let emulator = emulator();
    create_bucket(&emulator, BUCKET).await;
    stage(&emulator, BUCKET, MEDIA_KEY, TRAINING_FILE).await;
    let media = format!("s3://{BUCKET}/{MEDIA_KEY}");

    // The names are taken from the end of the table, so this test's rows are not the
    // ones the IN_PROGRESS test uses.
    let names: Vec<String> = model_names().into_iter().rev().collect();
    let references = references();

    // Two creates per row, differing in nothing but whether the data they name is
    // staged: one is served, one is refused.
    for (row, (reference, model)) in references.iter().zip(names.iter()).enumerate() {
        let staged_key = format!("staged-{row}/corpus.txt");
        stage(&emulator, BUCKET, &staged_key, TRAINING_FILE).await;
        let usable = format!("usable-{row}");
        call(
            &emulator,
            "CreateLanguageModel",
            create_request(&usable, &format!("s3://{BUCKET}/staged-{row}/"), None),
        )
        .await
        .ok(reference.case);
        let refused = call(
            &emulator,
            "CreateLanguageModel",
            create_request(model, &format!("s3://{BUCKET}/not-staged-{row}/"), None),
        )
        .await;
        refused.refused(reference.case, 400, "BadRequestException");
        let message = refused.error_message(reference.case);
        assert_eq!(
            message, UNREADABLE_S3_URI_MESSAGE,
            "case {:?}: the create must be refused for the location it cannot read, \
             got {message:?}",
            reference.case
        );
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for (row, (reference, model)) in references.iter().zip(names.iter()).enumerate() {
        let case = reference.case;
        let usable = format!("usable-{row}");

        // What the store answered, as the family's own reader reports it: one model,
        // and nothing at all under the refused name.
        let described = call(
            &emulator,
            "DescribeLanguageModel",
            json!({"ModelName": usable}),
        )
        .await
        .ok(case)["LanguageModel"]
            .clone();
        assert_eq!(
            described["ModelStatus"],
            json!("COMPLETED"),
            "case {case:?}: the model {usable:?} must settle COMPLETED, got \
             {described:?}"
        );
        call(
            &emulator,
            "DescribeLanguageModel",
            json!({"ModelName": model}),
        )
        .await
        .refused(case, 400, "BadRequestException");

        // A model this account does not hold is not one a job may name, and the
        // refusal is the family's own not-found sentence rather than a state one.
        let unheld_job = format!("unheld-model-{row}");
        let refused = call(
            &emulator,
            reference.start,
            (reference.request)(&unheld_job, model, &media),
        )
        .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert_eq!(
            message,
            crate::transcription_job_request_rules::not_found_message("language model"),
            "case {case:?}: the refusal must be the family's not-found sentence, got \
             {message:?}"
        );

        // A refused start stores nothing, so the job name it asked for is unknown.
        call(
            &emulator,
            reference.read,
            json!({reference.name_member: unheld_job}),
        )
        .await
        .refused(case, 400, "BadRequestException");

        // The counter-case through the very same member path: the model the store
        // answered for is COMPLETED, so the same start applying it is served.
        let usable_job = format!("usable-model-{row}");
        call(
            &emulator,
            reference.start,
            (reference.request)(&usable_job, &usable, &media),
        )
        .await
        .ok(case);
        let stored = call(
            &emulator,
            reference.read,
            json!({reference.name_member: usable_job}),
        )
        .await;
        let job = stored.ok(case)[reference.job_member].clone();
        let applied = (reference.reported)(&job);
        assert_eq!(
            applied,
            &json!(usable),
            "case {case:?}: the stored job must report the model it applies, got \
             {applied:?}"
        );
    }
}

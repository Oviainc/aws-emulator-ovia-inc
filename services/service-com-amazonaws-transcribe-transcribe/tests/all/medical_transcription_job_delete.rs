// SPDX-License-Identifier: CC0-1.0

//! `DeleteMedicalTranscriptionJob`, end to end over the wire:
//! `StartMedicalTranscriptionJob` -> `DeleteMedicalTranscriptionJob` ->
//! `GetMedicalTranscriptionJob` / `ListMedicalTranscriptionJobs` /
//! `ListTagsForResource`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULES UNDER TEST:
//!
//! * deleting a stored medical transcription job the service has finished with REMOVES
//!   its record: the call answers this RPC protocol's success status with no members,
//!   the job is then neither readable by name nor present in the listing, its tag set
//!   is gone with it, a second delete of the same name is refused, and the name is free
//!   for a new start again;
//! * a job the service is still working on — one in a non-terminal status — is NOT
//!   deletable: the call is refused with the error the operation models for an entity
//!   that "doesn't exist or … is in a non-terminal state (such as IN PROGRESS)"
//!   (`smithy_query(query="operation", operation="DeleteMedicalTranscriptionJob")`), and
//!   the record survives the refusal unchanged;
//! * a name THIS family holds no job under is refused the same way, whichever other
//!   family holds a resource of that name, and that other resource is untouched.
//!
//! Each test asserts that over a table spanning the start class (media URI scheme and
//! format, each published `MediaSampleRateHertz` bound, every documented form of
//! `OutputKey`, both `Type` values, PHI content identification, the optional settings,
//! encryption, tag count, and each published bound of the job-name length), not over
//! one row.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_transcribe_transcribe::state::DEFAULT_REGION;

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the `IN_PROGRESS -> COMPLETED` transition of a job started during
/// the test to have fallen due: one span of `crate::state::JOB_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The ARN `<type>` segment that names this family, so a tag set can be read back
/// through the standard tagging API.
const RESOURCE_TYPE: &str = "medical-transcription-job";

/// The modelled client error this service answers for a job it holds no record of, and
/// the status its awsJson1_1 wire protocol puts on every client error (both pinned by
/// `crate::client_error_wire_status`).
const ABSENT_CODE: &str = "BadRequestException";
/// See [`ABSENT_CODE`].
const CLIENT_ERROR_STATUS: u16 = 400;

/// The status a medical transcription job the clock has finished with reports
/// (`smithy_query(query="shape", shape="TranscriptionJobStatus")`), so a job in it is
/// done and deletable.
const FINISHED_STATUS: &str = "COMPLETED";

/// The status a job the service is still working on reports — a non-terminal one, so a
/// delete of such a job is refused.
const RUNNING_STATUS: &str = "IN_PROGRESS";

/// The class of job a `StartMedicalTranscriptionJob` request can describe, one row per
/// axis this family's rules generalise over: the media URI scheme and format, each
/// published bound of `MediaSampleRateHertz` (`@range(min: 16000, max: 48000)`), each
/// form `OutputKey` can take (absent, a file name, a prefix with a trailing separator),
/// both values of `Type` (`CONVERSATION | DICTATION`), whether PHI content
/// identification is asked for, which optional settings are set, whether the output is
/// KMS-encrypted, and how many tags are attached. Plus one row at each published bound
/// of the name member, which `TranscriptionJobName` models as
/// `length {min 1, max 200}` / `pattern ^[0-9a-zA-Z._-]+$`
/// (`smithy_query(query="operation", operation="DeleteMedicalTranscriptionJob")`).
fn start_requests() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "a dictation at the lowest published sample rate, no OutputKey, no tags"
                .to_string(),
            json!({
                "MedicalTranscriptionJobName": "delete-medical-minimal",
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
             file, PHI identification, speaker partitioning, KMS encryption, two tags"
                .to_string(),
            json!({
                "MedicalTranscriptionJobName": "delete.medical_2",
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
                    "VocabularyName": "delete-clinic-terms"
                },
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION",
                "Tags": [{"Key": "clinic", "Value": "cardiology"}, {"Key": "cost", "Value": "7"}]
            }),
        ),
        (
            "a conversation with channel identification, an OutputKey naming a prefix \
             with a trailing separator and one tag"
                .to_string(),
            json!({
                "MedicalTranscriptionJobName": "delete_medical-3",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.flac"},
                "MediaFormat": "flac",
                "OutputBucketName": "example-medical-output",
                "OutputKey": "clinic/visits/",
                "Settings": {"ChannelIdentification": true},
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION",
                "Tags": [{"Key": "clinic", "Value": "neurology"}]
            }),
        ),
        (
            "a dictation with alternative transcriptions and no media format".to_string(),
            json!({
                "MedicalTranscriptionJobName": "delete-medical-4",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/notes.mp3"},
                "OutputBucketName": "example-medical-output",
                "Settings": {"ShowAlternatives": true, "MaxAlternatives": 3},
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION"
            }),
        ),
    ];
    for name in ["m".to_string(), "z".repeat(200)] {
        rows.push((
            format!("a job name at the length bound {}", name.len()),
            named_job(&name),
        ));
    }
    rows
}

/// A start request for a job named `name`, minimal apart from the name.
fn named_job(name: &str) -> Value {
    json!({
        "MedicalTranscriptionJobName": name,
        "LanguageCode": "en-US",
        "Media": {"MediaFileUri": "s3://example-media/dictation.wav"},
        "OutputBucketName": "example-medical-output",
        "Specialty": "PRIMARYCARE",
        "Type": "DICTATION"
    })
}

/// The job name a start request asks for.
fn job_name(request: &Value) -> String {
    request["MedicalTranscriptionJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// Every media location the rows name as an object of the emulator's own Amazon S3:
/// with the object store mounted, a start whose media it does not hold is refused, so
/// what the rows name is staged.
const STAGED_MEDIA: &[&str] = &[
    "s3://example-media/dictation.wav",
    "s3://example-media/two-channel.flac",
    "s3://example-media/notes.mp3",
];

/// An emulator holding everything the rows name: the media each start is pointed at,
/// and every custom medical vocabulary a row applies, built from a table file the object
/// store serves and waited out until it is `READY`.
///
/// A start may only name a custom medical vocabulary this account holds AND has finished
/// building (the `medical_job_start_vocabulary_state` suite asserts that rule), and this
/// family's terms come only from its table file — so these rows need the object store
/// beside the service.
async fn given_what_the_rows_name() -> Transcribe {
    let transcribe = Transcribe::with_object_store();
    transcribe.given_media_objects(STAGED_MEDIA).await;
    for (_, request) in start_requests() {
        let Some(name) = request["Settings"]["VocabularyName"].as_str() else {
            continue;
        };
        transcribe.medical_vocabulary_ready(name).await;
    }
    transcribe
}

/// `GetMedicalTranscriptionJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call(
            "GetMedicalTranscriptionJob",
            json!({"MedicalTranscriptionJobName": name}),
        )
        .await
}

/// `DeleteMedicalTranscriptionJob` for `name`.
async fn delete(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call(
            "DeleteMedicalTranscriptionJob",
            json!({"MedicalTranscriptionJobName": name}),
        )
        .await
}

/// The names `ListMedicalTranscriptionJobs` reports, over a page big enough for every
/// row this suite starts.
async fn listed_names(transcribe: &Transcribe, case: &str) -> Vec<String> {
    let reply = transcribe
        .call("ListMedicalTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    reply.ok(case)["MedicalTranscriptionJobSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a summary list, got {:?}",
                reply.body
            )
        })
        .iter()
        .map(|summary| {
            summary["MedicalTranscriptionJobName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary names its job, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// The ARN of this family's job named `name`, as `ListTagsForResource` takes it — built
/// through the framework's own ARN helper, so no test names a partition or an account
/// literal.
fn arn(name: &str) -> String {
    engine::emulator_arn(
        "transcribe",
        DEFAULT_REGION,
        &format!("{RESOURCE_TYPE}/{name}"),
    )
}

#[tokio::test]
async fn a_finished_medical_job_is_deleted_and_one_still_being_processed_is_refused() {
    let transcribe = given_what_the_rows_name().await;
    let rows = start_requests();

    for (case, request) in &rows {
        transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await
            .ok(case);
    }

    // Every row is IN_PROGRESS from the moment it is started — a non-terminal status —
    // so a delete of it is refused rather than performed, with the job families' own
    // fixed sentence for that condition (which carries no request or state value).
    for (case, request) in &rows {
        let name = job_name(request);
        let refused = delete(&transcribe, &name).await;
        refused.refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        let message = refused.error_message(case);
        assert_eq!(
            message, "The job couldn't be deleted. Make sure it exists and isn't pending.",
            "case {case:?}: expected the job family's non-terminal delete refusal, got \
             {message:?}"
        );

        // The refusal left the record exactly where it was.
        let fetched = get(&transcribe, &name).await;
        let status = fetched.ok(case)["MedicalTranscriptionJob"]["TranscriptionJobStatus"].clone();
        assert_eq!(
            status,
            json!(RUNNING_STATUS),
            "case {case:?}: a refused delete must leave the job stored and unchanged, \
             got {status:?}"
        );
    }

    let mut stored = listed_names(&transcribe, "listing before the deletes").await;
    stored.sort();
    let mut started: Vec<String> = rows.iter().map(|(_, request)| job_name(request)).collect();
    started.sort();
    assert_eq!(
        stored, started,
        "every refused delete must leave its job in the listing, got {stored:?}"
    );

    // One wait for every row: once the processing span has elapsed each job is
    // COMPLETED — a terminal status — and the same delete goes through.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in &rows {
        let name = job_name(request);
        let finished = get(&transcribe, &name).await;
        assert_eq!(
            finished.ok(case)["MedicalTranscriptionJob"]["TranscriptionJobStatus"],
            json!(FINISHED_STATUS),
            "case {case:?}: the processing span has elapsed, so the job must be \
             {FINISHED_STATUS}, got {:?}",
            finished.body
        );

        let deleted = delete(&transcribe, &name).await;
        deleted.ok_status(case, 200);
        assert!(
            matches!(&deleted.body, Value::Null)
                || matches!(&deleted.body, Value::Object(members) if members.is_empty()),
            "case {case:?}: DeleteMedicalTranscriptionJob answers no members, got {:?}",
            deleted.body
        );

        // The record is gone: this family's reader refuses the name, the listing no
        // longer holds it, and the tag set the start stored went with the record.
        get(&transcribe, &name)
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        let remaining = listed_names(&transcribe, case).await;
        assert!(
            !remaining.contains(&name),
            "case {case:?}: a deleted job must be gone from the listing, got {remaining:?}"
        );
        transcribe
            .call("ListTagsForResource", json!({"ResourceArn": arn(&name)}))
            .await
            .refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);

        // Deleting what is no longer there is the not-found refusal, not a second
        // success: this operation is not idempotent.
        let again = delete(&transcribe, &name).await;
        again.refused(case, CLIENT_ERROR_STATUS, ABSENT_CODE);
        let message = again.error_message(case);
        assert_eq!(
            message,
            "The requested job couldn't be found. Check the job name and try your \
             request again.",
            "case {case:?}: unexpected not-found message, got {message:?}"
        );
    }

    let empty = listed_names(&transcribe, "listing after the deletes").await;
    assert!(
        empty.is_empty(),
        "every job was deleted, so the listing must be empty, got {empty:?}"
    );

    // A deleted name is free again: a start under it is accepted rather than refused as
    // a conflict with the record the delete removed.
    for (case, request) in &rows {
        let restarted = transcribe
            .call("StartMedicalTranscriptionJob", request.clone())
            .await;
        let status =
            restarted.ok(case)["MedicalTranscriptionJob"]["TranscriptionJobStatus"].clone();
        assert_eq!(
            status,
            json!(RUNNING_STATUS),
            "case {case:?}: the deleted name must be free for a new start, got {status:?}"
        );
    }
}

#[tokio::test]
async fn a_name_this_family_holds_no_medical_job_under_is_refused() {
    let transcribe = Transcribe::new();

    // A name nothing at all was ever created under, in each form the name member's
    // pattern allows, including both published length bounds.
    for name in [
        "never-started",
        "a",
        "never.started_2",
        "9",
        &"n".repeat(200),
    ] {
        let case = format!("DeleteMedicalTranscriptionJob {name:?}");
        let refused = delete(&transcribe, name).await;
        refused.refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);

        // The refused delete MINTED nothing: this family's reader still does not find
        // the name.
        get(&transcribe, name)
            .await
            .refused(&case, CLIENT_ERROR_STATUS, ABSENT_CODE);
    }

    // A name ANOTHER family holds a resource under is not a medical transcription job:
    // each family is keyed on its own store, so the delete refuses and that other
    // resource is untouched. One row per other family whose own start/create takes a
    // name of this shape.
    let shared = "delete-shared-name";
    let others: Vec<(&str, Value, &str, &str)> = vec![
        (
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": shared,
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.mp3"}
            }),
            "GetTranscriptionJob",
            "TranscriptionJobName",
        ),
        (
            "StartCallAnalyticsJob",
            json!({
                "CallAnalyticsJobName": shared,
                "Media": {"MediaFileUri": "s3://example-media/call.flac"}
            }),
            "GetCallAnalyticsJob",
            "CallAnalyticsJobName",
        ),
        (
            "CreateVocabulary",
            json!({
                "VocabularyName": shared,
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
            "GetVocabulary",
            "VocabularyName",
        ),
    ];
    for (create, request, _, _) in &others {
        transcribe
            .call(create, request.clone())
            .await
            .ok(&format!("{create} under the shared name"));
    }

    delete(&transcribe, shared).await.refused(
        "a name only other families hold a resource under",
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );

    // Now the medical transcription job of that same name, deleted once it is
    // finished: the delete reaches only this family's record.
    transcribe
        .call("StartMedicalTranscriptionJob", named_job(shared))
        .await
        .ok("start the medical transcription job of the shared name");
    std::thread::sleep(PAST_THE_TRANSITION);
    delete(&transcribe, shared)
        .await
        .ok_status("delete the medical transcription job of the shared name", 200);

    get(&transcribe, shared).await.refused(
        "the deleted medical transcription job",
        CLIENT_ERROR_STATUS,
        ABSENT_CODE,
    );
    for (_, _, read, name_member) in &others {
        transcribe
            .call(read, json!({(*name_member): shared}))
            .await
            .ok(&format!(
                "{read} must still answer after the medical delete"
            ));
    }

    // A name one past the member's modelled length is a boundary violation refused
    // before the handler runs, not this operation's not-found error.
    delete(&transcribe, &"n".repeat(201)).await.refused(
        "a name one past the length bound",
        CLIENT_ERROR_STATUS,
        "ValidationException",
    );
}

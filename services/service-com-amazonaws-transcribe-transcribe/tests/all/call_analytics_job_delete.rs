// SPDX-License-Identifier: CC0-1.0

//! `DeleteCallAnalyticsJob`, end to end over the wire.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call, the request table and the family's read helpers are the
//! Call Analytics job suite's (`crate::call_analytics_jobs`), reused rather than stood
//! up a second time.
//!
//! THE RULES UNDER TEST:
//!
//! * deleting a stored job the service has finished with REMOVES its record: the call
//!   answers this RPC protocol's success status with no members, the job is then
//!   neither readable by name nor present in the listing, a second delete of the same
//!   name is refused, and the name is free for a new start again;
//! * a job the service is still working on — one in a non-terminal status — is NOT
//!   deletable: the call is refused with the error the operation models for an entity
//!   that "doesn't exist or … is in a non-terminal state (such as IN PROGRESS)", and
//!   the record survives the refusal unchanged;
//! * a name no record of THIS family exists under is refused the same way, whichever
//!   other family holds a resource of that name.
//!
//! Each test asserts that over a table spanning the request class (media member and
//! URI scheme, every documented output-location form, language options, vocabulary and
//! filter settings, content redaction, summarization, channel definitions, tag count,
//! and each published bound of the job-name length), not over one row.

use serde_json::{json, Value};

use crate::call_analytics_jobs::{
    create_named_resources, get, job_name, list, names_of, start_requests, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::{Reply, Transcribe};

/// `DeleteCallAnalyticsJob` for `name`.
async fn delete(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call(
            "DeleteCallAnalyticsJob",
            json!({"CallAnalyticsJobName": name}),
        )
        .await
}

/// A start request for a job named `name`, minimal apart from the name.
fn named_job(name: &str) -> Value {
    json!({
        "CallAnalyticsJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/call.flac"}
    })
}

/// The rows this suite deletes: the family's own request table, plus one row at each
/// published bound of the name member (`CallAnalyticsJobName` is modelled
/// `length {min 1, max 200}` —
/// `smithy_query(query="operation", operation="DeleteCallAnalyticsJob")`), which that
/// table does not span.
fn delete_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = start_requests()
        .into_iter()
        .map(|(case, request)| (case.to_string(), request))
        .collect();
    for name in ["a".to_string(), "z".repeat(200)] {
        rows.push((
            format!("job name at the length bound {}", name.len()),
            named_job(&name),
        ));
    }
    rows
}

#[tokio::test]
async fn a_finished_job_is_deleted_and_one_still_being_processed_is_refused() {
    let transcribe = Transcribe::new();
    let rows = delete_rows();

    // A row may only name a custom vocabulary, filter or language model this account
    // holds, so the suite creates what its settings name before starting anything.
    create_named_resources(&transcribe).await;

    for (case, request) in &rows {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .ok(case);
    }

    // Every row is IN_PROGRESS from the moment it is started — a non-terminal status —
    // and a delete of such a job is refused rather than performed
    // (`BadRequestException`: "This can occur when the entity you're trying to delete
    // doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)").
    for (case, request) in &rows {
        let name = job_name(request);
        delete(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");

        // The refusal left the record exactly where it was.
        let fetched = get(&transcribe, &name).await;
        let status = fetched.ok(case)["CallAnalyticsJob"]["CallAnalyticsJobStatus"].clone();
        assert_eq!(
            status,
            json!("IN_PROGRESS"),
            "case {case:?}: a refused delete must leave the job stored and unchanged, \
             got {status:?}"
        );
    }

    let before = list(&transcribe, json!({"MaxResults": 100})).await;
    let mut stored = names_of(&before.ok("listing before the deletes")["CallAnalyticsJobSummaries"]);
    stored.sort();
    let mut started: Vec<String> = rows.iter().map(|(_, request)| job_name(request)).collect();
    started.sort();
    assert_eq!(
        stored, started,
        "every refused delete must leave its job in the listing, got {stored:?}"
    );

    // Once the processing span has elapsed each job is COMPLETED — a terminal status —
    // and the same delete goes through.
    std::thread::sleep(PAST_THE_TRANSITION);
    for (case, request) in &rows {
        let name = job_name(request);
        let deleted = delete(&transcribe, &name).await;
        deleted.ok_status(case, 200);
        assert_eq!(
            deleted.body,
            json!({}),
            "case {case:?}: DeleteCallAnalyticsJob answers no members, got {:?}",
            deleted.body
        );

        // The record is gone: the family's reader refuses the name, and the listing no
        // longer holds it.
        get(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");
        let listed = list(&transcribe, json!({"MaxResults": 100})).await;
        let remaining = names_of(&listed.ok(case)["CallAnalyticsJobSummaries"]);
        assert!(
            !remaining.contains(&name),
            "case {case:?}: a deleted job must be gone from the listing, got {remaining:?}"
        );

        // Deleting what is no longer there is the not-found refusal, not a second
        // success: this operation is not idempotent.
        delete(&transcribe, &name)
            .await
            .refused(case, 400, "BadRequestException");
    }

    let after = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        after.ok("listing after the deletes")["CallAnalyticsJobSummaries"],
        json!([]),
        "every job was deleted, so the listing must be empty, got {:?}",
        after.body
    );

    // A deleted name is free again: a start under it is accepted rather than refused
    // as a conflict with the record the delete removed.
    for (case, request) in &rows {
        let restarted = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        let status = restarted.ok(case)["CallAnalyticsJob"]["CallAnalyticsJobStatus"].clone();
        assert_eq!(
            status,
            json!("IN_PROGRESS"),
            "case {case:?}: the deleted name must be free for a new start, got {status:?}"
        );
    }
}

#[tokio::test]
async fn a_name_this_family_holds_no_job_under_is_refused() {
    let transcribe = Transcribe::new();

    // A name nothing at all was ever created under.
    delete(&transcribe, "never-started")
        .await
        .refused("a name no resource exists under", 400, "BadRequestException");

    // A name ANOTHER family holds a resource under is not a Call Analytics job: each
    // family is keyed on its own store, so the delete refuses and that other resource
    // is untouched. One row per family whose own start/create takes a name of this
    // shape.
    let shared = "shared-name";
    transcribe
        .call(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": shared,
                "Media": {"MediaFileUri": "s3://example-media/audio.mp3"},
                "LanguageCode": "en-US"
            }),
        )
        .await
        .ok("start the transcription job of the same name");
    transcribe
        .call(
            "StartMedicalTranscriptionJob",
            json!({
                "MedicalTranscriptionJobName": shared,
                "Media": {"MediaFileUri": "s3://example-media/visit.mp3"},
                "LanguageCode": "en-US",
                "Specialty": "PRIMARYCARE",
                "Type": "CONVERSATION",
                "OutputBucketName": "example-output"
            }),
        )
        .await
        .ok("start the medical transcription job of the same name");

    delete(&transcribe, shared).await.refused(
        "a name only other families hold a resource under",
        400,
        "BadRequestException",
    );

    for (case, operation, member) in [
        (
            "the transcription job of the same name",
            "GetTranscriptionJob",
            "TranscriptionJobName",
        ),
        (
            "the medical transcription job of the same name",
            "GetMedicalTranscriptionJob",
            "MedicalTranscriptionJobName",
        ),
    ] {
        transcribe
            .call(operation, json!({member: shared}))
            .await
            .ok(case);
    }

    // Now the Call Analytics job of that same name, deleted once it is finished: the
    // delete reaches only this family's record.
    transcribe
        .call("StartCallAnalyticsJob", named_job(shared))
        .await
        .ok("start the Call Analytics job of the same name");
    std::thread::sleep(PAST_THE_TRANSITION);
    delete(&transcribe, shared)
        .await
        .ok_status("delete the Call Analytics job of the shared name", 200);

    get(&transcribe, shared).await.refused(
        "the deleted Call Analytics job",
        400,
        "BadRequestException",
    );
    for (case, operation, member) in [
        (
            "the transcription job survives the Call Analytics delete",
            "GetTranscriptionJob",
            "TranscriptionJobName",
        ),
        (
            "the medical transcription job survives the Call Analytics delete",
            "GetMedicalTranscriptionJob",
            "MedicalTranscriptionJobName",
        ),
    ] {
        transcribe
            .call(operation, json!({member: shared}))
            .await
            .ok(case);
    }

    // A name past the member's modelled length is a boundary violation refused before
    // the handler runs, not this operation's not-found error.
    delete(&transcribe, &"n".repeat(201))
        .await
        .refused("a name one past the length bound", 400, "ValidationException");
}

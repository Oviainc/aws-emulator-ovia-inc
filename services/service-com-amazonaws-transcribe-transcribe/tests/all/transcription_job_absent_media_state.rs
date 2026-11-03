// SPDX-License-Identifier: CC0-1.0

//! What the transcription-job family holds AFTER a start whose media named no object:
//! nothing, so every later call about that job name answers as it does for a name no
//! start ever used.
//!
//! THE RULE UNDER TEST: a start is refused before anything is persisted when the media
//! it was pointed at is no object of the object store — either a URI of the S3 scheme
//! that addresses no object at all (a bucket with no key under it), which needs no store
//! to judge, or an address the store itself reports it holds nothing at. Because the
//! refusal precedes the record, the job NEVER EXISTS: the family's listing reports no
//! summary for that name, and the single-job read and the delete are refused with the
//! family's absent-entity wording — never with anything about the state of a job, which
//! is a different fact about a different (existing) resource.
//!
//! The rows differ along what the rule generalises over: which part of the S3 address
//! fails to resolve (an absent bucket, a key nothing was written to, a key that only
//! prefixes a stored one, a key that only extends one) and whether the URI addresses an
//! object of the scheme at all (no key, an empty key, an empty bucket). The follow-up
//! calls vary too: the listing unfiltered, the listing narrowed by `JobNameContains`,
//! the read, and the delete.
//!
//! The last two tests keep the absence honest. The CONTROL drives the same calls over a
//! start the store does answer for — so the job is listed, reads back, and its delete
//! reports its STATE while the delete of a name nothing stored reports its ABSENCE. And
//! with no object store mounted at all, an address the emulator simply cannot ask about
//! refuses nothing, while a URI that addresses no object of the scheme is still refused:
//! a refusal is grounded in the request's own URI or in what the store answered, never
//! in an assumption.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::media_object_resolution::{
    call, emulator, emulator_without_the_object_store, given_staged_media, ABSENT_BUCKET, BUCKET,
    PAST_THE_TRANSITION, STAGED_KEYS,
};
use crate::transcription_jobs::Reply;

/// The refusal a start whose media names no S3 object is answered with, as the service
/// words it for that error class.
///
/// `pub(crate)` so the Call Analytics start suite
/// (`crate::call_analytics_job_start_preconditions`) asserts the same wording from one
/// home rather than restating it.
pub(crate) const MEDIA_NAMES_NO_OBJECT: &str =
    "The URI that you provided doesn't point to an S3 object. Make sure that the object \
     exists and try your request again.";

/// The refusal every call about a transcription job the service does not hold is
/// answered with, as the service words it for an absent job.
const JOB_NOT_FOUND: &str =
    "The requested job couldn't be found. Check the job name and try your request again.";

/// One row: the media URI a start is pointed at, and the job name it would have stored.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// A job name of the modelled pattern (`^[0-9a-zA-Z._-]+$`).
    name: &'static str,
    /// The `Media.MediaFileUri` the start carries.
    uri: fn() -> String,
}

/// Rows whose media the object store does not hold: the start is refused, so the name
/// names nothing afterwards.
fn no_such_object() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket the store does not hold",
            name: "absent-bucket",
            uri: || format!("s3://{ABSENT_BUCKET}/dictation.wav"),
        },
        Row {
            case: "a key nothing was ever written to",
            name: "absent-key",
            uri: || format!("s3://{BUCKET}/never-uploaded.flac"),
        },
        Row {
            case: "a key that only prefixes a stored one",
            name: "key.prefixes_a_stored_one",
            uri: || format!("s3://{BUCKET}/clinic/visits"),
        },
        Row {
            case: "a key that only extends a stored one",
            name: "key-extends-a-stored-one",
            uri: || format!("s3://{BUCKET}/{}.bak", STAGED_KEYS[0]),
        },
    ]
}

/// Rows whose URI addresses no object OF THE S3 SCHEME, which the request itself
/// settles: the media member is the Amazon S3 location of a media FILE, and both of its
/// documented examples name a bucket and a key
/// (`smithy_query(query="doc", shape="Media", member="MediaFileUri")`), so a bucket with
/// no key under it, an empty key or an empty bucket names no media file for any store to
/// hold.
fn addresses_no_object() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket with no key under it",
            name: "bucket-with-no-key",
            uri: || format!("s3://{BUCKET}"),
        },
        Row {
            case: "a bucket with an empty key under it",
            name: "bucket-with-an-empty-key",
            uri: || format!("s3://{BUCKET}/"),
        },
        Row {
            case: "the scheme with no bucket",
            name: "scheme-with-no-bucket",
            uri: || format!("s3:///{}", STAGED_KEYS[0]),
        },
    ]
}

/// Every row whose start must be refused with nothing stored.
fn refused_rows() -> Vec<Row> {
    let mut rows = no_such_object();
    rows.extend(addresses_no_object());
    rows
}

/// Start one transcription job of the smallest valid shape over `uri`.
async fn start(emulator: &MultiServiceRouter, name: &str, uri: &str) -> Reply {
    call(
        emulator,
        "StartTranscriptionJob",
        json!({
            "TranscriptionJobName": name,
            "LanguageCode": "en-US",
            "Media": {"MediaFileUri": uri}
        }),
    )
    .await
}

/// The job names one `ListTranscriptionJobs` answer reports, in the order it reports
/// them.
async fn listed_names(emulator: &MultiServiceRouter, case: &str, input: Value) -> Vec<String> {
    let listed = call(emulator, "ListTranscriptionJobs", input).await;
    let body = listed.ok(case);
    body["TranscriptionJobSummaries"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row["TranscriptionJobName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_refused_start_leaves_no_job_for_a_later_list_read_or_delete() {
    let emulator = emulator();
    given_staged_media(&emulator).await;

    for row in refused_rows() {
        let uri = (row.uri)();
        let answered = start(&emulator, row.name, &uri).await;
        answered.refused(row.case, 400, "BadRequestException");
        let message = answered.error_message(row.case);
        assert_eq!(
            message, MEDIA_NAMES_NO_OBJECT,
            "case {:?}: expected the service's wording for a media URI that names no \
             object, got {message:?}",
            row.case
        );

        // The listing has no summary for the name, whether it is asked for the whole
        // family or narrowed to that very name.
        for (how, input) in [
            ("unfiltered", json!({"MaxResults": 100})),
            (
                "narrowed by JobNameContains",
                json!({"JobNameContains": row.name, "MaxResults": 100}),
            ),
        ] {
            let names = listed_names(&emulator, row.case, input).await;
            assert!(
                !names.iter().any(|listed| listed == row.name),
                "case {:?}: the listing {how} must report no job for a refused start, \
                 got {names:?}",
                row.case
            );
        }

        // The single-job read reports the job as absent and carries no job.
        let read = call(
            &emulator,
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": row.name}),
        )
        .await;
        read.refused(row.case, 400, "BadRequestException");
        let message = read.error_message(row.case);
        assert_eq!(
            message, JOB_NOT_FOUND,
            "case {:?}: expected the absent-job wording from the read, got {message:?}",
            row.case
        );
        assert!(
            read.body["TranscriptionJob"].is_null(),
            "case {:?}: a refused start leaves no job to read, got {:?}",
            row.case,
            read.body
        );

        // And the delete reports the SAME fact — the job is not there — rather than
        // anything about the state of a job that was never created.
        let deleted = call(
            &emulator,
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": row.name}),
        )
        .await;
        deleted.refused(row.case, 400, "BadRequestException");
        let message = deleted.error_message(row.case);
        assert_eq!(
            message, JOB_NOT_FOUND,
            "case {:?}: expected the delete to report the job as absent, got {message:?}",
            row.case
        );
    }

    // No refused start reached the family's listing at all.
    let names = listed_names(&emulator, "the family listing", json!({"MaxResults": 100})).await;
    assert!(
        names.is_empty(),
        "no refused start may reach the listing, got {names:?}"
    );
}

#[tokio::test]
async fn a_start_the_object_store_answers_for_is_listed_read_and_deletable() {
    let emulator = emulator();
    given_staged_media(&emulator).await;

    let case = "a start over an object the store holds";
    let name = "stored-media-job";
    let uri = format!("s3://{BUCKET}/{}", STAGED_KEYS[0]);
    start(&emulator, name, &uri).await.ok_status(case, 200);

    // The listing reports it, both unfiltered and narrowed to the name — the same two
    // questions the refused rows answer with nothing.
    for (how, input) in [
        ("unfiltered", json!({"MaxResults": 100})),
        (
            "narrowed by JobNameContains",
            json!({"JobNameContains": name, "MaxResults": 100}),
        ),
    ] {
        let names = listed_names(&emulator, case, input).await;
        assert_eq!(
            names,
            vec![name.to_string()],
            "case {case:?}: the listing {how} must report the stored job, got {names:?}"
        );
    }

    // It reads back by name.
    let read = call(
        &emulator,
        "GetTranscriptionJob",
        json!({"TranscriptionJobName": name}),
    )
    .await;
    let job = &read.ok(case)["TranscriptionJob"];
    assert_eq!(
        job["TranscriptionJobName"], name,
        "case {case:?}: the stored job must read back under its name, got {job:?}"
    );

    // A delete while the service is still working on it is refused with the job
    // families' own fixed sentence for that condition: a different fact from absence,
    // about a job that does exist, and one the sentence states without naming the job
    // or the status it is in.
    let refused = call(
        &emulator,
        "DeleteTranscriptionJob",
        json!({"TranscriptionJobName": name}),
    )
    .await;
    refused.refused(case, 400, "BadRequestException");
    let message = refused.error_message(case);
    assert_eq!(
        message, "The job couldn't be deleted. Make sure it exists and isn't pending.",
        "case {case:?}: expected the job family's non-terminal delete refusal, got \
         {message:?}"
    );

    // Once the job has settled, the delete succeeds — and then the name names nothing
    // again, so the read and a second delete report absence, exactly as they do for a
    // start that was refused.
    std::thread::sleep(PAST_THE_TRANSITION);
    call(
        &emulator,
        "DeleteTranscriptionJob",
        json!({"TranscriptionJobName": name}),
    )
    .await
    .ok_status(case, 200);

    for operation in ["GetTranscriptionJob", "DeleteTranscriptionJob"] {
        let answered = call(
            &emulator,
            operation,
            json!({"TranscriptionJobName": name}),
        )
        .await;
        answered.refused(operation, 400, "BadRequestException");
        let message = answered.error_message(operation);
        assert_eq!(
            message, JOB_NOT_FOUND,
            "case {operation:?}: expected the absent-job wording after the delete, got \
             {message:?}"
        );
    }
    let names = listed_names(&emulator, case, json!({"MaxResults": 100})).await;
    assert!(
        names.is_empty(),
        "case {case:?}: a deleted job reaches no listing, got {names:?}"
    );
}

#[tokio::test]
async fn with_no_object_store_a_uri_that_addresses_no_object_is_still_refused() {
    // A refusal grounded in the request's own URI stands whatever is mounted: such a
    // URI names no media file for any store to hold.
    let emulator = emulator_without_the_object_store();

    for row in addresses_no_object() {
        let uri = (row.uri)();
        let answered = start(&emulator, row.name, &uri).await;
        answered.refused(row.case, 400, "BadRequestException");
        let message = answered.error_message(row.case);
        assert_eq!(
            message, MEDIA_NAMES_NO_OBJECT,
            "case {:?}: expected the refusal to stand with no store mounted, got \
             {message:?}",
            row.case
        );
        let read = call(
            &emulator,
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": row.name}),
        )
        .await;
        read.refused(row.case, 400, "BadRequestException");
    }

    // An address this emulator merely cannot ASK about refuses nothing: the start is
    // accepted and the job is stored.
    for row in no_such_object() {
        let uri = (row.uri)();
        start(&emulator, row.name, &uri)
            .await
            .ok_status(row.case, 200);
        let read = call(
            &emulator,
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": row.name}),
        )
        .await;
        let job = &read.ok(row.case)["TranscriptionJob"];
        assert_eq!(
            job["TranscriptionJobName"], row.name,
            "case {:?}: with no store to ask, the job must be stored, got {job:?}",
            row.case
        );
    }
}

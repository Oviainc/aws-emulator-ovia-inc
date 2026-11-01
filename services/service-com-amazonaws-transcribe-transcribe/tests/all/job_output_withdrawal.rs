// SPDX-License-Identifier: CC0-1.0

//! What a job's DELETE does to the documents the job's reported output locations
//! served, end to end over the wire: a start, the family's reader polled until the job
//! is finished, a plain object-store GET of every location it reports, then the
//! family's delete and the same GETs again.
//!
//! THE RULE THESE ROWS ASSERT: removing the record of a resource withdraws the file
//! this service wrote for it into a bucket of its OWN, and leaves untouched every file
//! it wrote into a bucket the REQUEST named, which belongs to the caller. The
//! vocabulary family already closes that loop — a delete withdraws the file its
//! `DownloadUri` named (`crate::vocabulary_download_isolation`) — and the service-managed
//! branch of a job's output location names the very same bucket
//! (`crate::state::service_managed_bucket`), so a job's delete owes its documents the
//! same withdrawal. A caller-named bucket is the caller's: "you must first upload your
//! media file into an Amazon S3 bucket", and the bucket a start directs its output to
//! exists because the caller created it, so nothing in it is this service's to remove.
//!
//! The rows differ along what the rule generalises over: which family started the job
//! (each of the four), whose bucket the output went to (this service's own, because the
//! start named none, against one the request named in each form that family documents),
//! how many locations the job therefore reports (a redacted transcript beside the
//! unredacted one, a subtitle file per requested format), and which document kind sits
//! at each (a transcript, a WebVTT file, a SubRip file, a clinical note).
//!
//! The harness is the job-output suite's (`crate::job_output_objects`: its row, its
//! families, its polling and its reported-location reader) over the media-resolution
//! suite's emulator, reused rather than stood up a third time.

use serde_json::json;

use crate::job_output_objects::{
    assert_document, finished_job, given_output_buckets, object_path, start_request, Family, Row,
    OUTPUT_BUCKET,
};
use crate::media_object_resolution::{call, emulator, given_staged_media};

/// The Region every call of this harness is signed for
/// (`crate::media_object_resolution`), and so the Region whose service-managed bucket
/// this service's own files live in.
const REGION: &str = "us-east-1";

/// The bucket this service's OWN files live in for [`REGION`], as
/// `crate::state::service_managed_bucket` spells it — the one a start that named no
/// output location of its own directs the transcript to.
fn service_managed_bucket() -> String {
    format!("aws-transcribe-{REGION}-prod")
}

/// The operation that deletes a job of this family.
fn delete_operation(family: Family) -> &'static str {
    match family {
        Family::Transcription => "DeleteTranscriptionJob",
        Family::Medical => "DeleteMedicalTranscriptionJob",
        Family::CallAnalytics => "DeleteCallAnalyticsJob",
        Family::Scribe => "DeleteMedicalScribeJob",
    }
}

/// One row: a finished job, and how many of the locations it reports are this service's
/// own to withdraw against how many are the caller's to keep. Both counts are stated, so
/// a row can neither pass by reporting nothing nor by having every location land in one
/// kind of bucket.
struct Case {
    row: Row,
    /// Locations in this service's own bucket, which the delete must withdraw.
    withdrawn: usize,
    /// Locations in a bucket the request named, which the delete must leave alone.
    kept: usize,
}

/// The rows. Every family appears, each with the output placements its own model admits:
/// `OutputBucketName` is `@required` for a medical transcription job and for a Medical
/// Scribe job (`smithy_query(query="operation",
/// operation="StartMedicalTranscriptionJob,StartMedicalScribeJob")`), so those two can
/// only ever write into the caller's bucket, while a transcription job and a Call
/// Analytics job write into this service's own whenever the start named no location.
fn cases() -> Vec<Case> {
    vec![
        Case {
            row: Row {
                case: "a transcription job that named no bucket, so its transcript is \
                       this service's own file",
                family: Family::Transcription,
                name: "withdraw-service-managed",
                output: || json!({}),
                buckets: &[],
                locations: 1,
            },
            withdrawn: 1,
            kept: 0,
        },
        Case {
            row: Row {
                case: "a transcription job whose service-managed output is a transcript \
                       and both subtitle formats",
                family: Family::Transcription,
                name: "withdraw-subtitled",
                output: || json!({"Subtitles": {"Formats": ["vtt", "srt"]}}),
                buckets: &[],
                locations: 3,
            },
            withdrawn: 3,
            kept: 0,
        },
        Case {
            row: Row {
                case: "a transcription job whose service-managed output is a redacted \
                       transcript beside the unredacted one",
                family: Family::Transcription,
                name: "withdraw-redacted",
                output: || {
                    json!({
                        "ContentRedaction": {
                            "RedactionType": "PII",
                            "RedactionOutput": "redacted_and_unredacted",
                        },
                    })
                },
                locations: 2,
                buckets: &[],
            },
            withdrawn: 2,
            kept: 0,
        },
        Case {
            row: Row {
                case: "a transcription job whose OutputBucketName names the caller's \
                       bucket",
                family: Family::Transcription,
                name: "keep-caller-bucket",
                output: || {
                    json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "kept/out.json"})
                },
                buckets: &[OUTPUT_BUCKET],
                locations: 1,
            },
            withdrawn: 0,
            kept: 1,
        },
        Case {
            row: Row {
                case: "a transcription job whose OutputKey names a folder in the \
                       caller's bucket, asking for a subtitle file too",
                family: Family::Transcription,
                name: "keep-caller-folder",
                output: || {
                    json!({
                        "OutputBucketName": OUTPUT_BUCKET,
                        "OutputKey": "kept-folder/",
                        "Subtitles": {"Formats": ["vtt"]},
                    })
                },
                buckets: &[OUTPUT_BUCKET],
                locations: 2,
            },
            withdrawn: 0,
            kept: 2,
        },
        Case {
            row: Row {
                case: "a Call Analytics job that named no OutputLocation",
                family: Family::CallAnalytics,
                name: "withdraw-analytics",
                output: || json!({}),
                buckets: &[],
                locations: 1,
            },
            withdrawn: 1,
            kept: 0,
        },
        Case {
            row: Row {
                case: "a Call Analytics job whose service-managed output is a redacted \
                       transcript beside the unredacted one",
                family: Family::CallAnalytics,
                name: "withdraw-analytics-redacted",
                output: || {
                    json!({
                        "Settings": {
                            "ContentRedaction": {
                                "RedactionType": "PII",
                                "RedactionOutput": "redacted_and_unredacted",
                            },
                        },
                    })
                },
                buckets: &[],
                locations: 2,
            },
            withdrawn: 2,
            kept: 0,
        },
        Case {
            row: Row {
                case: "a Call Analytics job whose OutputLocation names a file in the \
                       caller's bucket",
                family: Family::CallAnalytics,
                name: "keep-analytics-file",
                output: || {
                    json!({"OutputLocation": format!("s3://{OUTPUT_BUCKET}/kept-calls/call.json")})
                },
                buckets: &[OUTPUT_BUCKET],
                locations: 1,
            },
            withdrawn: 0,
            kept: 1,
        },
        Case {
            row: Row {
                case: "a medical transcription job, whose OutputBucketName is required",
                family: Family::Medical,
                name: "keep-medical",
                output: || json!({"OutputBucketName": OUTPUT_BUCKET, "OutputKey": "kept-medical/"}),
                buckets: &[OUTPUT_BUCKET],
                locations: 1,
            },
            withdrawn: 0,
            kept: 1,
        },
        Case {
            row: Row {
                case: "a Medical Scribe job, which reports a transcript and a clinical \
                       note in the caller's bucket",
                family: Family::Scribe,
                name: "keep-scribe",
                output: || json!({"OutputBucketName": OUTPUT_BUCKET}),
                buckets: &[OUTPUT_BUCKET],
                locations: 2,
            },
            withdrawn: 0,
            kept: 2,
        },
    ]
}

/// Whether the location `uri` addresses a file in a bucket of THIS SERVICE's, which is
/// what decides whose the file is.
fn is_service_managed(case: &str, uri: &str) -> bool {
    object_path(case, uri).starts_with(&format!("/{}/", service_managed_bucket()))
}

/// Start the row's job, poll it to finished, and answer every location it reports —
/// asserting first that each one really serves its document, so a row cannot pass
/// because nothing was ever written.
async fn served_locations(
    router: &smithy_server_runtime::MultiServiceRouter,
    row: &Row,
) -> Vec<String> {
    given_output_buckets(router, row).await;
    call(router, row.family.start(), start_request(row))
        .await
        .ok(row.case);
    let job = finished_job(router, row).await;

    let locations = row.family.reported_locations(&job);
    assert_eq!(
        locations.len(),
        row.locations,
        "case {:?}: expected {} reported output location(s), got {locations:?}",
        row.case,
        row.locations
    );
    for location in &locations {
        let path = object_path(row.case, location);
        let (status, body) = crate::media_object_resolution::s3(router, "GET", &path, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the reported location {location:?} must serve its document \
             before the delete, got HTTP {status} with body {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );
        assert_document(row.case, row.name, &path, &body);
    }
    locations
}

/// `Delete<Family>Job` for the row's job.
async fn delete(router: &smithy_server_runtime::MultiServiceRouter, row: &Row) {
    call(
        router,
        delete_operation(row.family),
        json!({row.family.name_member(): row.name}),
    )
    .await
    .ok_status(row.case, 200);
}

/// A delete withdraws every document this service wrote into a bucket of its own for the
/// job, and leaves every document it wrote into a bucket the request named exactly where
/// it is.
#[tokio::test]
async fn a_job_delete_withdraws_this_services_own_documents_and_keeps_the_callers() {
    let router = emulator();
    given_staged_media(&router).await;

    for case in &cases() {
        let row = &case.row;
        let locations = served_locations(&router, row).await;

        let own = locations
            .iter()
            .filter(|location| is_service_managed(row.case, location))
            .count();
        assert_eq!(
            (own, locations.len() - own),
            (case.withdrawn, case.kept),
            "case {:?}: expected {} location(s) in this service's own bucket and {} in \
             the caller's, got {locations:?}",
            row.case,
            case.withdrawn,
            case.kept
        );

        delete(&router, row).await;

        // The record is gone: the family's reader no longer reports the job at all.
        call(
            &router,
            row.family.get(),
            json!({row.family.name_member(): row.name}),
        )
        .await
        .refused(row.case, 400, "BadRequestException");

        for location in &locations {
            let path = object_path(row.case, location);
            let (status, body) =
                crate::media_object_resolution::s3(&router, "GET", &path, &[]).await;
            if is_service_managed(row.case, location) {
                assert_eq!(
                    status, 404,
                    "case {:?}: the deleted job's own file at {location:?} must be \
                     withdrawn, got HTTP {status} with body {:?}",
                    row.case,
                    String::from_utf8_lossy(&body)
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {:?}: the file in the caller's bucket at {location:?} is the \
                     caller's to keep, got HTTP {status} with body {:?}",
                    row.case,
                    String::from_utf8_lossy(&body)
                );
                assert_document(row.case, row.name, &path, &body);
            }
        }
    }
}

/// A delete withdraws only the documents of the job it names: another job's own files,
/// in the same service-managed bucket, are still served afterwards — and a job started
/// under the freed name gets its own document at the same location again, so the
/// withdrawal is not a tombstone.
#[tokio::test]
async fn a_delete_withdraws_only_the_documents_of_its_own_job() {
    let router = emulator();
    given_staged_media(&router).await;

    let deleted = Row {
        case: "the transcription job that is deleted",
        family: Family::Transcription,
        name: "isolation-deleted",
        output: || json!({}),
        buckets: &[],
        locations: 1,
    };
    let kept = Row {
        case: "the transcription job beside it",
        family: Family::Transcription,
        name: "isolation-kept",
        output: || json!({}),
        buckets: &[],
        locations: 1,
    };
    let analytics = Row {
        case: "the Call Analytics job beside it",
        family: Family::CallAnalytics,
        name: "isolation-analytics",
        output: || json!({}),
        buckets: &[],
        locations: 1,
    };

    let deleted_locations = served_locations(&router, &deleted).await;
    let kept_locations = served_locations(&router, &kept).await;
    let analytics_locations = served_locations(&router, &analytics).await;
    assert_ne!(
        deleted_locations, kept_locations,
        "case {:?}: two jobs must report different locations, both got \
         {deleted_locations:?}",
        deleted.case
    );

    delete(&router, &deleted).await;

    for location in &deleted_locations {
        let path = object_path(deleted.case, location);
        let (status, body) = crate::media_object_resolution::s3(&router, "GET", &path, &[]).await;
        assert_eq!(
            status, 404,
            "case {:?}: the deleted job's own file at {location:?} must be withdrawn, \
             got HTTP {status} with body {:?}",
            deleted.case,
            String::from_utf8_lossy(&body)
        );
    }
    for (row, locations) in [(&kept, &kept_locations), (&analytics, &analytics_locations)] {
        for location in locations {
            let path = object_path(row.case, location);
            let (status, body) =
                crate::media_object_resolution::s3(&router, "GET", &path, &[]).await;
            assert_eq!(
                status, 200,
                "case {:?}: another job's file at {location:?} survives the delete, got \
                 HTTP {status} with body {:?}",
                row.case,
                String::from_utf8_lossy(&body)
            );
            assert_document(row.case, row.name, &path, &body);
        }
    }

    // The name is free again, and the job started under it owes the same location its
    // own document.
    let restarted = served_locations(&router, &deleted).await;
    assert_eq!(
        restarted, deleted_locations,
        "case {:?}: a job restarted under the freed name reports the same location, got \
         {restarted:?}",
        deleted.case
    );
}

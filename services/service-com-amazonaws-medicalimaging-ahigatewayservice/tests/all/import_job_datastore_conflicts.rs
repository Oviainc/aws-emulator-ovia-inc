// SPDX-License-Identifier: CC0-1.0

//! The two rules a DICOM import job in flight puts on the rest of this service, and the
//! form a refusal about a resource identifier the request URI carries takes.
//!
//! RULE 1 — a data store with a DICOM import job still RUNNING is not deletable: an
//! import writes image sets into the store, so `DeleteDatastore` answers the modelled
//! `ConflictException` (HTTP 409) with one fixed sentence, leaves the store exactly as it
//! was (`GetDatastore` still reports `ACTIVE`), and admits the identical call once every
//! job of that store has reached a terminal `jobStatus`. The rule is about RUNNING jobs
//! and nothing else, so it is consulted BEFORE the store's remaining image sets are.
//!
//! RULE 2 — a data store that has not settled into `ACTIVE` refuses work on it, and on
//! its contents, with ONE sentence AWS words the same for every operation that refuses on
//! that rule, carrying neither the data store's identifier nor the status it was in.
//!
//! RULE 3 — a `ValidationException` about a member this service's model binds to the
//! request URI's PATH (`@httpLabel`) states the member path and the requirement and
//! WITHHOLDS the rejected value: the value is the URI the caller addressed the request to
//! rather than something a member of the request held. Every `@httpLabel` of this service
//! is covered, not only the one a corpus happens to record; a member of a nested payload
//! that shares a label's name still echoes its value, which `crate::relational_refusal_values`
//! pins from the other side.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`,
//! `crate::image_set_copy`): real requests through `MultiServiceRouter::dispatch`, with
//! the state struct touched only to SEED an image set and never to assert.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_medicalimaging_ahigatewayservice::state::{
    IMPORT_JOB_COMPLETION_DELAY, IMPORT_JOB_IN_PROGRESS_DELAY,
};

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};
use crate::image_set_copy::seed_image_set;

/// The sentence AWS refuses the delete of a data store with a running import job with.
/// Entirely fixed text: no identifier, no count of the jobs in flight.
const RUNNING_IMPORT_JOBS: &str = "Cannot delete a datastore with running import jobs";

/// The sentence AWS refuses a call on a data store that has not settled into `ACTIVE`
/// with. The whole sentence is fixed text.
const DATASTORE_NOT_ACTIVE: &str = "The datastore is not active.";

/// The sentence this service refuses the delete of a store that still holds image sets
/// with — the sibling 409 of the same operation, which the running-job rule is checked
/// ahead of.
const STILL_HOLDS_IMAGE_SETS: &str = "The data store still contains image sets, which \
                                      must all be deleted before the data store can be \
                                      deleted.";

/// The `IAM` role a submit names (`RoleArn` is
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$` —
/// `smithy_query(query="operation",operation="StartDICOMImportJob")`). No IAM is mounted
/// beside this service here, so nothing is established about the role and the submit is
/// served; `aws_emulator_router`'s `medical_imaging_dicom_import` suite is where the
/// resolution of a role against real IAM records is asserted.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// The body `StartDICOMImportJob` takes.
fn import_body(client_token: &str, job_name: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "clientToken": client_token,
        "dataAccessRoleArn": ROLE_ARN,
        "inputS3Uri": "s3://imaging-in/studies/",
        "outputS3Uri": "s3://imaging-out/results/",
    });
    if let Some(name) = job_name {
        body["jobName"] = json!(name);
    }
    body
}

/// Assert one refusal: the status and error code the model declares, the exact sentence,
/// and that the answer reflects none of `identifiers` back.
fn assert_refusal(
    case: &str,
    answer: &Answer,
    status: u16,
    error_code: &str,
    expected_message: &str,
    identifiers: &[&str],
) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, error_code,
        "case {case:?}: expected error code {error_code}, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the refusal text {expected_message:?}, got {message:?}"
    );
    let body = answer.body.to_string();
    for identifier in identifiers {
        assert!(
            !body.contains(identifier),
            "case {case:?}: the refusal reflected {identifier:?} back, body {body:?}"
        );
    }
}

/// One row of the running-import-job table.
#[derive(Debug)]
struct RunningCase {
    case: &'static str,
    /// The region the store is created in and every call below is addressed to.
    region: &'static str,
    /// How many import jobs are submitted before the delete arrives.
    jobs: usize,
    /// Whether each submitted job carries a `jobName`.
    named: bool,
    /// How many image sets the store is seeded with, which the running-job rule is
    /// checked AHEAD of.
    seeded_image_sets: usize,
    /// How long after the submits the delete is sent, as a fraction of the job's window:
    /// `0` is immediately (the job still `SUBMITTED`) and `1` is after the move to
    /// `IN_PROGRESS` has fallen due.
    after_in_progress: bool,
}

/// RULE 1: `DeleteDatastore` is refused 409 `ConflictException` with one fixed sentence
/// while any import job of the store is still running, the store is left `ACTIVE`, and
/// the identical call is admitted once the jobs have settled — unless the store holds
/// image sets, which is the sibling refusal the running-job rule is checked ahead of.
///
/// The rows differ along what the rule generalises over: how many jobs are in flight,
/// which non-terminal status they are in when the delete arrives (`SUBMITTED` and
/// `IN_PROGRESS`), whether the jobs carry a name, which region the store lives in, and
/// whether the store also holds image sets.
#[tokio::test]
async fn a_running_import_job_refuses_the_datastore_delete_with_one_sentence() {
    let rows = [
        RunningCase {
            case: "one job, still SUBMITTED",
            region: "us-east-1",
            jobs: 1,
            named: false,
            seeded_image_sets: 0,
            after_in_progress: false,
        },
        RunningCase {
            case: "one named job, now IN_PROGRESS",
            region: "us-east-1",
            jobs: 1,
            named: true,
            seeded_image_sets: 0,
            after_in_progress: true,
        },
        RunningCase {
            case: "several jobs in flight at once",
            region: "eu-west-1",
            jobs: 3,
            named: true,
            seeded_image_sets: 0,
            after_in_progress: false,
        },
        RunningCase {
            case: "a job in flight over a store that also holds image sets",
            region: "ap-southeast-2",
            jobs: 2,
            named: false,
            seeded_image_sets: 2,
            after_in_progress: true,
        },
    ];

    let mut texts: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let case = row.case;
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore_in(row.region, "running.jobs", "running-jobs-token")
            .await;
        let mut seeded: Vec<String> = Vec::new();
        for _ in 0..row.seeded_image_sets {
            seeded.push(seed_image_set(&emulator, &datastore_id, 1));
        }
        let mut job_ids: Vec<String> = Vec::new();
        for job in 0..row.jobs {
            let body = import_body(&format!("running-{job}"), row.named.then_some("run.job"));
            let answer = emulator
                .call_in(
                    row.region,
                    "POST",
                    &format!("/startDICOMImportJob/datastore/{datastore_id}"),
                    Some(body),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 submitting job {job}, got {answer:?}"
            );
            job_ids.push(answer.string(&["jobId"]));
        }
        if row.after_in_progress {
            // Between the two due instants: the job has left `SUBMITTED` and has not yet
            // reached its terminal status.
            tokio::time::sleep((IMPORT_JOB_IN_PROGRESS_DELAY + IMPORT_JOB_COMPLETION_DELAY) / 2)
                .await;
        }

        // The jobs really are still running when the delete arrives, read off the wire.
        for job_id in &job_ids {
            let read = emulator
                .call_in(
                    row.region,
                    "GET",
                    &format!("/getDICOMImportJob/datastore/{datastore_id}/job/{job_id}"),
                    None,
                )
                .await;
            let status = read.string(&["jobProperties", "jobStatus"]);
            assert!(
                status == "SUBMITTED" || status == "IN_PROGRESS",
                "case {case:?}: expected the job to still be running, got {status:?}"
            );
        }

        let refused = emulator
            .call_in(
                row.region,
                "DELETE",
                &format!("/datastore/{datastore_id}"),
                None,
            )
            .await;
        let mut withheld: Vec<&str> = vec![datastore_id.as_str()];
        withheld.extend(job_ids.iter().map(String::as_str));
        withheld.extend(seeded.iter().map(String::as_str));
        assert_refusal(
            case,
            &refused,
            409,
            "ConflictException",
            RUNNING_IMPORT_JOBS,
            &withheld,
        );
        texts.insert(refused.string(&["message"]));

        // The refused delete left the data store exactly as it was.
        let read = emulator
            .call_in(
                row.region,
                "GET",
                &format!("/datastore/{datastore_id}"),
                None,
            )
            .await;
        assert_eq!(
            read.string(&["datastoreProperties", "datastoreStatus"]),
            "ACTIVE",
            "case {case:?}: expected the refused delete to leave the store ACTIVE, got {read:?}"
        );

        // Once every job has settled the identical call is admitted — or, where the store
        // holds image sets, refused by THAT rule instead, which is what shows the
        // running-job rule was the one answering above.
        tokio::time::sleep(SETTLE_MULTIPLE * IMPORT_JOB_COMPLETION_DELAY).await;
        let again = emulator
            .call_in(
                row.region,
                "DELETE",
                &format!("/datastore/{datastore_id}"),
                None,
            )
            .await;
        if row.seeded_image_sets == 0 {
            assert_eq!(
                again.status, 200,
                "case {case:?}: expected HTTP 200 once the jobs settled, got {again:?}"
            );
            assert_eq!(
                again.string(&["datastoreStatus"]),
                "DELETING",
                "case {case:?}: expected DELETING once the jobs settled, got {again:?}"
            );
        } else {
            assert_refusal(
                case,
                &again,
                409,
                "ConflictException",
                STILL_HOLDS_IMAGE_SETS,
                &[datastore_id.as_str()],
            );
        }
    }

    assert_eq!(
        texts.len(),
        1,
        "the running-import-job refusal must be ONE sentence across every row, got {texts:?}"
    );
}

/// RULE 2: every operation that refuses a data store which has not settled into `ACTIVE`
/// answers AWS's one sentence for it.
///
/// The rows span the operations that refuse on that rule — a submit into the store, a
/// copy of one of its image sets, a delete of one — and both non-`ACTIVE` statuses a
/// store reaches while the clock still owes it something: `CREATING` on the way to
/// `ACTIVE`, and `DELETING` on the way to gone.
#[tokio::test]
async fn the_not_active_conflict_is_awss_one_sentence_for_every_operation() {
    // A well-formed `ImageSetId` (`^[0-9a-z]{32}$`) the store holds nothing under: the
    // store's own status is what refuses these calls, ahead of anything it holds.
    let image_set_id = "0".repeat(32);
    // (operation, method, URI template, body)
    let calls: [(&str, &str, String, Option<serde_json::Value>); 3] = [
        (
            "StartDICOMImportJob",
            "POST",
            "/startDICOMImportJob/datastore/{id}".to_string(),
            Some(import_body("not-active-token", None)),
        ),
        (
            "CopyImageSet",
            "POST",
            format!("/datastore/{{id}}/imageSet/{image_set_id}/copyImageSet"),
            Some(json!({ "sourceImageSet": { "latestVersionId": "1" } })),
        ),
        (
            "DeleteImageSet",
            "POST",
            format!("/datastore/{{id}}/imageSet/{image_set_id}/deleteImageSet"),
            None,
        ),
    ];

    let mut texts: BTreeSet<String> = BTreeSet::new();
    for settled in [false, true] {
        for (operation, method, template, body) in &calls {
            let case = if settled {
                format!("{operation} against a DELETING store")
            } else {
                format!("{operation} against a CREATING store")
            };
            let emulator = Emulator::new();
            let datastore_id = if settled {
                // A store whose deletion is in flight: it has settled out of ACTIVE the
                // other way, and the same sentence refuses work on it.
                let id = emulator
                    .active_datastore("not.active", "not-active-store")
                    .await;
                let deleted = emulator.delete_datastore(&id).await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: expected HTTP 200 starting the delete, got {deleted:?}"
                );
                id
            } else {
                // Read back with no wait at all, so the store is still CREATING.
                let created = emulator
                    .create_datastore(json!({
                        "datastoreName": "not.active",
                        "clientToken": "not-active-store",
                    }))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected HTTP 200 from the create, got {created:?}"
                );
                created.string(&["datastoreId"])
            };

            let uri = template.replace("{id}", &datastore_id);
            let refused = emulator.call(method, &uri, body.clone()).await;
            assert_refusal(
                &case,
                &refused,
                409,
                "ConflictException",
                DATASTORE_NOT_ACTIVE,
                &[datastore_id.as_str()],
            );
            texts.insert(refused.string(&["message"]));
        }
    }

    assert_eq!(
        texts.len(),
        1,
        "the not-active refusal must be ONE sentence across every operation and status, \
         got {texts:?}"
    );
}

/// One row of the URI-label table.
struct LabelCase {
    case: &'static str,
    method: &'static str,
    /// The URI, with `{store}` standing for the data store the request addresses and
    /// `{bad}` for the illegal label value.
    template: &'static str,
    body: Option<serde_json::Value>,
    /// The validation path the clause must name.
    member: &'static str,
    /// The requirement half of the clause.
    requirement: &'static str,
    /// The illegal value the label carries.
    offending: &'static str,
}

/// `ImageSetId` and `DatastoreId` share one `@pattern`
/// (`smithy_query(query="shape",shape="DatastoreId,ImageSetId")`: `^[0-9a-z]{32}$`),
/// quoted here without the anchors the service strips.
const ID_PATTERN_REQUIREMENT: &str = "Member must satisfy regular expression pattern: [0-9a-z]{32}";

/// RULE 3: a refusal about an `@httpLabel` states the member and the requirement and
/// carries no value.
///
/// The rows span every `@httpLabel` identifier of this service other than the two the
/// sibling `crate::error_message_texts` table already covers (`datastoreId`, `jobId`):
/// `imageSetId` as four different operations bind it, and `sourceImageSetId`, which only
/// the copy binds — crossed with the operations' methods (GET, POST, DELETE) and with
/// illegal values of different shapes.
#[tokio::test]
async fn a_refusal_about_a_uri_label_withholds_the_value() {
    let rows = [
        LabelCase {
            case: "GetImageSet",
            method: "POST",
            template: "/datastore/{store}/imageSet/{bad}/getImageSet",
            body: None,
            member: "imageSetId",
            requirement: ID_PATTERN_REQUIREMENT,
            offending: "not-an-image-set-id!!",
        },
        LabelCase {
            case: "GetImageSetMetadata",
            method: "POST",
            template: "/datastore/{store}/imageSet/{bad}/getImageSetMetadata",
            body: None,
            member: "imageSetId",
            requirement: ID_PATTERN_REQUIREMENT,
            offending: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        },
        LabelCase {
            case: "DeleteImageSet",
            method: "POST",
            template: "/datastore/{store}/imageSet/{bad}/deleteImageSet",
            body: None,
            member: "imageSetId",
            requirement: ID_PATTERN_REQUIREMENT,
            offending: "short",
        },
        LabelCase {
            case: "ListImageSetVersions",
            method: "POST",
            template: "/datastore/{store}/imageSet/{bad}/listImageSetVersions",
            body: None,
            member: "imageSetId",
            requirement: ID_PATTERN_REQUIREMENT,
            offending: "0123456789abcdef0123456789abcdefg",
        },
        LabelCase {
            case: "CopyImageSet",
            method: "POST",
            template: "/datastore/{store}/imageSet/{bad}/copyImageSet",
            // The payload is `@required`, so the request carries a well-formed one: what
            // is refused is the label, not the body.
            body: Some(json!({ "sourceImageSet": { "latestVersionId": "1" } })),
            member: "sourceImageSetId",
            requirement: ID_PATTERN_REQUIREMENT,
            offending: "not-a-source-image-set-id!!",
        },
    ];

    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("labels", "labels-token").await;

    for row in &rows {
        let case = row.case;
        let uri = row
            .template
            .replace("{store}", &datastore_id)
            .replace("{bad}", row.offending);
        let answer = emulator.call(row.method, &uri, row.body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let message = answer.string(&["message"]);
        let expected = format!(
            "1 validation error detected: {} failed to satisfy constraint: {}",
            row.member, row.requirement
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains("Value "),
            "case {case:?}: expected no echoed value, got {message:?}"
        );
        assert!(
            !message.contains(row.offending),
            "case {case:?}: expected the refused value to be withheld, got {message:?}"
        );
    }
}

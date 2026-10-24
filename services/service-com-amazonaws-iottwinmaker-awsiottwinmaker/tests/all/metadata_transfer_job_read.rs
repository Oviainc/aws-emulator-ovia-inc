// SPDX-License-Identifier: CC0-1.0

//! `GetMetadataTransferJob` as a READ: which ids it reports a job for, which it
//! refuses and how, and that reporting a job changes nothing a later caller
//! sees.
//!
//! The lifecycle suite beside this one
//! (`tests/all/metadata_transfer_job.rs`) pins what a read reports for a job
//! its create persisted and when the clock settles that job's outcome. These
//! cases pin the read's own ADDRESSING rule instead, over the whole class of
//! values its `metadataTransferJobId` label can carry:
//!
//! * every id the modelled `Id` admits addresses the job stored under it —
//!   whatever its length and whatever characters the pattern lets it use;
//! * an id the model admits that this account does not hold is the modelled
//!   `ResourceNotFoundException` (HTTP 404), naming the job that was looked
//!   for;
//! * an id outside the modelled `Id` never reaches the lookup at all and is
//!   refused with HTTP 400 `ValidationException`;
//! * and a read — served or refused — mints no job, removes none and leaves
//!   every stored job exactly as it was.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`), so
//! the route, the `@httpLabel` binding, the status code and the serialized body
//! are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    metadata_transfer_job_not_found_message, metadata_transfer_job_role,
    METADATA_TRANSFER_JOB_STATE_ERROR, METADATA_TRANSFER_JOB_STATE_VALIDATING,
};

use crate::harness::{metadata_transfer_job_arn, workspace_arn, Emulator};

/// The workspace the destination of every transfer created here names.
const WORKSPACE: &str = "readWorkspace";

/// The published bound of the modelled `Id` the `metadataTransferJobId` label
/// targets: `@length(min: 1, max: 128)`, from
/// `smithy_query(query="shape",shape="Id")`.
const ID_MAX: usize = 128;

/// A create request transferring an S3 document into [`WORKSPACE`] under `id`.
fn create_request(id: &str) -> Value {
    json!({
        "metadataTransferJobId": id,
        "sources": [{
            "type": "s3",
            "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/entities.json"},
        }],
        "destination": {
            "type": "iottwinmaker",
            "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
        },
    })
}

/// The ids the modelled `Id` ADMITS, spanning what its constraint generalises
/// over: the shortest and the longest value it allows, and each character class
/// its pattern
/// `^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$` permits in each position (both
/// from `smithy_query(query="shape",shape="Id")`).
fn admitted_ids() -> Vec<(&'static str, String)> {
    vec![
        // The pattern needs a first character AND a trailing alphanumeric, so
        // two characters is the shortest value it admits even though
        // `@length`'s minimum is one.
        ("shortest the pattern admits", "ab".to_string()),
        ("leading underscore", "_importJob".to_string()),
        ("digits only", "0123456789".to_string()),
        ("embedded hyphens", "job-into-workspace".to_string()),
        ("embedded underscores", "job_into_workspace".to_string()),
        ("mixed case", "JobIntoWorkspaceX".to_string()),
        // Exactly at the published maximum length.
        ("at the max length", "a".repeat(ID_MAX)),
    ]
}

/// RULE: an id the modelled `Id` admits addresses the metadata transfer job
/// stored under it, whatever its length or the characters the pattern lets it
/// use, and the read reports that job's own identity, ends, role and state.
#[tokio::test]
async fn every_id_the_model_admits_addresses_its_own_job() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case transfers into must exist"
    );

    for (case, id) in admitted_ids() {
        let created = emulator.create_metadata_transfer_job(create_request(&id)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
        );

        let read = emulator.get_metadata_transfer_job(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetMetadataTransferJob, got {read:?}"
        );
        assert_eq!(
            (
                read.member("metadataTransferJobId").as_str(),
                read.member("arn").as_str(),
            ),
            (
                Some(id.as_str()),
                Some(metadata_transfer_job_arn(&id).as_str()),
            ),
            "case {case:?}: the read must report the job addressed by this id, got {read:?}"
        );
        // The ends of the transfer come back verbatim, and the role is the
        // service's own: the create accepts none.
        let request = create_request(&id);
        assert_eq!(
            (read.member("sources"), read.member("destination")),
            (&request["sources"], &request["destination"]),
            "case {case:?}: expected the created ends, got {read:?}"
        );
        assert_eq!(
            read.member("metadataTransferJobRole").as_str(),
            Some(metadata_transfer_job_role().as_str()),
            "case {case:?}: expected the service's transfer role, got {read:?}"
        );
        // The job was just accepted, so it is in the non-terminal state its
        // create answered with or — if its outcome has already fallen due — the
        // terminal state the clock owes it; never anything else.
        let state = read.member("status")["state"].as_str();
        assert!(
            state == Some(METADATA_TRANSFER_JOB_STATE_VALIDATING)
                || state == Some(METADATA_TRANSFER_JOB_STATE_ERROR),
            "case {case:?}: expected the accepted or the settled state, got {read:?}"
        );
        assert!(
            read.member("creationDateTime")
                .as_f64()
                .is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected an epoch-seconds creationDateTime, got {read:?}"
        );
        // Nothing transferred, so there is no count and no report to name.
        assert!(
            read.member("progress").is_null() && read.member("reportUrl").is_null(),
            "case {case:?}: a job that transferred nothing reports no progress and no \
             reportUrl, got {read:?}"
        );
    }
}

/// RULE: a read naming an id the modelled `Id` admits but this account does not
/// hold is refused with the modelled `ResourceNotFoundException` (HTTP 404)
/// naming the job that was looked for; a read naming an id outside the modelled
/// `Id` — past its published length bound, or outside its published pattern —
/// never reaches the lookup and is refused with HTTP 400 `ValidationException`.
#[tokio::test]
async fn a_read_refuses_an_unheld_id_and_an_id_outside_the_model() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the held job transfers into must exist"
    );
    let held = "heldJob";
    assert_eq!(
        emulator
            .create_metadata_transfer_job(create_request(held))
            .await
            .status,
        200,
        "the one job this account holds must be created"
    );

    // Each row takes the id either inside the modelled `Id` but outside this
    // account's jobs, or one step past a published bound of `Id` itself —
    // `@length(1..128)` and `^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$`.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "well formed and not held",
            "absentJob".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "not held, at the max length",
            "z".repeat(ID_MAX),
            404,
            "ResourceNotFoundException",
        ),
        (
            "not held, differing from a held id by one character",
            "heldJo".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "one past the max length",
            "a".repeat(ID_MAX + 1),
            400,
            "ValidationException",
        ),
        (
            "shorter than the pattern admits",
            "a".to_string(),
            400,
            "ValidationException",
        ),
        (
            "leading hyphen",
            "-job".to_string(),
            400,
            "ValidationException",
        ),
        (
            "trailing hyphen",
            "job-".to_string(),
            400,
            "ValidationException",
        ),
        (
            "trailing underscore",
            "job_".to_string(),
            400,
            "ValidationException",
        ),
        (
            "character outside the pattern",
            "job$1".to_string(),
            400,
            "ValidationException",
        ),
    ];

    for (case, id, status, error_type) in rows {
        let refused = emulator.get_metadata_transfer_job(&id).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (status, Some(error_type)),
            "case {case:?}: expected {status} {error_type}, got {refused:?}"
        );
        if status == 404 {
            // The refusal is built from the request: it names the job that was
            // looked for.
            assert_eq!(
                refused.member("message").as_str(),
                Some(metadata_transfer_job_not_found_message(&id).as_str()),
                "case {case:?}: the refusal must name the job looked for, got {refused:?}"
            );
        }
    }

    // Every refusal left the one job this account holds readable, and added
    // nothing beside it.
    assert_eq!(
        emulator.get_metadata_transfer_job(held).await.status,
        200,
        "the held job must still be readable after every refused read"
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .metadata_transfer_jobs
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![held.to_string()],
        "a refused read must neither mint a job nor remove one"
    );
}

/// RULE: a read is a report — whether it is served or refused, it mints no
/// metadata transfer job, removes none, and leaves the set of jobs this account
/// holds exactly as it was.
#[tokio::test]
async fn a_read_mints_no_job_and_removes_none() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the jobs transfer into must exist"
    );

    let ids = ["firstJob", "secondJob"];
    for id in ids {
        assert_eq!(
            emulator
                .create_metadata_transfer_job(create_request(id))
                .await
                .status,
            200,
            "the job {id:?} the cases read must be created"
        );
    }
    let before: Vec<String> = emulator
        .state()
        .lock()
        .metadata_transfer_jobs
        .keys()
        .cloned()
        .collect();

    // The rows differ in whether the id read is one this account holds, one the
    // model admits but nothing stored, and one the model rejects outright: none
    // of the three may change the stored set.
    let rows = [
        ("a held job", ids[0].to_string()),
        ("an unheld but well-formed id", "neverCreated".to_string()),
        ("an id outside the modelled Id", "-neverValid".to_string()),
        ("the same held job again", ids[0].to_string()),
    ];
    for (case, id) in rows {
        let read = emulator.get_metadata_transfer_job(&id).await;
        assert!(
            read.status == 200 || read.status == 404 || read.status == 400,
            "case {case:?}: a read answers either the job or a refusal, got {read:?}"
        );
        let after: Vec<String> = emulator
            .state()
            .lock()
            .metadata_transfer_jobs
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            after, before,
            "case {case:?}: a read must leave the stored jobs as they were, got {after:?}"
        );
    }

    // And both jobs are still readable, each under its own id.
    for id in ids {
        let read = emulator.get_metadata_transfer_job(id).await;
        assert_eq!(
            (read.status, read.member("metadataTransferJobId").as_str()),
            (200, Some(id)),
            "case {id:?}: every job this account holds stays readable, got {read:?}"
        );
    }
}

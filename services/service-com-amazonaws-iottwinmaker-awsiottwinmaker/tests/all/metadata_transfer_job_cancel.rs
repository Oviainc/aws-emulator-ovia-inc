// SPDX-License-Identifier: CC0-1.0

//! Cancelling a metadata transfer job, end to end over HTTP.
//!
//! `metadata_transfer_job` pins the create -> read round trip and the outcome
//! the clock owes an accepted job. This suite pins what CANCELLING one does:
//!
//! * whether a cancellation is accepted turns on ONE thing — whether the job
//!   has already finished. A job in any non-terminal `MetadataTransferJobState`
//!   is accepted and answers the non-terminal `CANCELLING`; a job in any of the
//!   three terminal ones (`COMPLETED`, `CANCELLED`, `ERROR`) has nothing left to
//!   cancel and is refused with the modelled `ConflictException` (HTTP 409).
//!   Nothing else about the job — which ends it transfers between, what it
//!   carries, whether a caller polled it — changes that answer, so the rule is
//!   asserted over a table of creates AND over every value of the modelled
//!   state enum;
//! * the CLOCK, not a caller, finishes a cancellation: the job is `CANCELLING`
//!   until [`METADATA_TRANSFER_JOB_CANCEL_SPAN`] has elapsed and `CANCELLED`
//!   from then on, reporting the instant the transition fell due;
//! * a cancellation already in flight is idempotent, and a job this account does
//!   not hold is the modelled `ResourceNotFoundException` (HTTP 404), which
//!   reserves nothing.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`), so
//! the route, the label, the status code and the serialized body are all
//! exercised.

use std::thread::sleep;
use std::time::{Duration, SystemTime};

use regex::Regex;
use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::METADATA_TRANSFER_JOB_STATE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    metadata_transfer_job_state_is_terminal, METADATA_TRANSFER_JOB_CANCEL_SPAN,
    METADATA_TRANSFER_JOB_ERROR_CODE, METADATA_TRANSFER_JOB_SETTLE_SPAN,
    METADATA_TRANSFER_JOB_STATE_CANCELLED, METADATA_TRANSFER_JOB_STATE_CANCELLING,
    METADATA_TRANSFER_JOB_STATE_ERROR, METADATA_TRANSFER_JOB_STATE_VALIDATING,
};

use crate::harness::{metadata_transfer_job_arn, workspace_arn, Emulator, Resp, REGION};

/// The workspaces the rows name: an IoT TwinMaker end of a transfer names a
/// workspace by ARN, and two of them let a row name a different workspace at
/// each end.
const WORKSPACE: &str = "cancelWorkspace";
/// See [`WORKSPACE`].
const OTHER_WORKSPACE: &str = "otherCancelWorkspace";

/// The modelled constraints this suite asserts against, copied from the model:
/// `smithy_query(query="shape", shape="Id,TwinMakerArn")`.
const ID_MAX: usize = 128;
const TWINMAKER_ARN_PATTERN: &str =
    r"^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$";
const TWINMAKER_ARN_MIN: usize = 20;

/// The `status.state` a response reports for a job.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("status")["state"].as_str()
}

/// A string of `length` ASCII characters built from `fill`.
fn filled(fill: char, length: usize) -> String {
    std::iter::repeat_n(fill, length).collect()
}

/// Whether `value` matches a modelled pattern, compiled per call: these tables
/// are small and a pattern is cheap next to a dispatched request.
fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern)
        .unwrap_or_else(|error| panic!("the modelled pattern {pattern:?} must compile: {error}"))
        .is_match(value)
}

/// An emulator holding the two workspaces the rows name.
async fn emulator_with_workspaces() -> Emulator {
    let emulator = Emulator::new();
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// The create table: `(case, request)`.
///
/// The rows differ along what the rule generalises over — which `SourceType`
/// and `DestinationType` the two ends carry (`ops::types::SOURCE_TYPE_VALUES`
/// and `DESTINATION_TYPE_VALUES` are both `s3`, `iotsitewise`,
/// `iottwinmaker`), whether an end carries the optional filters its
/// configuration admits, whether the create names a `description` (and whether
/// that description is the empty string `Description`'s `@length(0..2048)`
/// admits), and the shape and LENGTH of the `metadataTransferJobId` — the two
/// characters the `Id` pattern needs at minimum, the published 128-character
/// maximum, digits and underscores, and a create that names NO id at all, for
/// which the service mints one.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "s3IntoWorkspace",
            json!({
                "metadataTransferJobId": "cancelImport1",
                "description": "import a metadata document",
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/entities.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }),
        ),
        (
            "mintedIdWorkspaceOutToS3",
            json!({
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/exports"},
                },
            }),
        ),
        (
            "shortestIdSiteWiseIntoWorkspace",
            json!({
                "metadataTransferJobId": "ab",
                "description": "",
                "sources": [{
                    "type": "iotsitewise",
                    "iotSiteWiseConfiguration": {
                        "filters": [
                            {"filterByAssetModel": {
                                "assetModelId": "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
                                "includeOffspring": true,
                            }},
                            {"filterByAsset": {
                                "assetId": "4a5b6c7d-8e9f-4a1b-2c3d-4e5f6a7b8c9d",
                                "includeAssetModel": false,
                            }},
                        ],
                    },
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }),
        ),
        (
            "idAtMaxLengthWorkspaceToWorkspace",
            json!({
                "metadataTransferJobId": format!("{}z", filled('a', ID_MAX - 1)),
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {
                        "workspace": workspace_arn(WORKSPACE),
                        "filters": [{"filterByEntity": {"entityId": "pump1"}}],
                    },
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(OTHER_WORKSPACE)},
                },
            }),
        ),
        (
            "underscoredIdS3ToS3",
            json!({
                "metadataTransferJobId": "0_cancel-9",
                "description": "Kühlpumpen — 冷却ポンプ",
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/nested/in.json"},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/out"},
                },
            }),
        ),
    ]
}

/// RULE: a job that has not finished is cancellable — the cancel answers the
/// non-terminal `CANCELLING` with the job's own identity, the clock settles it
/// in `CANCELLED` and nothing cancels it twice — whatever the create carried.
#[tokio::test]
async fn an_accepted_job_is_cancellable_and_the_clock_settles_it_cancelled() {
    let emulator = emulator_with_workspaces().await;

    // Each row is created and cancelled before the next one starts, so the
    // cancel lands inside the window the create opened.
    let mut cancelled_rows: Vec<(&str, String, f64)> = Vec::new();
    for (case, request) in create_rows() {
        let created = emulator.create_metadata_transfer_job(request).await;
        assert_eq!(
            (created.status, reported_state(&created)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_VALIDATING)),
            "case {case:?}: the create must be accepted, got {created:?}"
        );
        let job_id = created
            .member("metadataTransferJobId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reports the job id"))
            .to_string();

        let cancelled = emulator.cancel_metadata_transfer_job(&job_id).await;
        assert_eq!(
            (cancelled.status, reported_state(&cancelled)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLING)),
            "case {case:?}: a job that has not finished answers CANCELLING, got {cancelled:?}"
        );
        assert_eq!(
            (
                cancelled.member("metadataTransferJobId").as_str(),
                cancelled.member("arn").as_str(),
            ),
            (
                Some(job_id.as_str()),
                Some(metadata_transfer_job_arn(&job_id).as_str()),
            ),
            "case {case:?}: expected the job's own identity, got {cancelled:?}"
        );
        let arn = cancelled.member("arn").as_str().unwrap_or_default();
        assert!(
            arn.chars().count() >= TWINMAKER_ARN_MIN && matches(TWINMAKER_ARN_PATTERN, arn),
            "case {case:?}: arn must be a TwinMakerArn, got {cancelled:?}"
        );
        assert!(
            cancelled.member("progress").is_null(),
            "case {case:?}: a cancellation moves no metadata, so it counts nothing, got \
             {cancelled:?}"
        );
        assert!(
            cancelled.member("status")["error"]["code"].is_null(),
            "case {case:?}: a cancelled job failed at nothing, got {cancelled:?}"
        );
        let cancelled_at = cancelled
            .member("updateDateTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: the cancel reports updateDateTime"));

        // The teardown window is NON-ZERO: a caller reading the job in the same
        // breath as the cancel still sees the transient state, and that read
        // moves nothing.
        let during = emulator.get_metadata_transfer_job(&job_id).await;
        assert_eq!(
            (
                during.status,
                reported_state(&during),
                during.member("updateDateTime").as_f64(),
            ),
            (
                200,
                Some(METADATA_TRANSFER_JOB_STATE_CANCELLING),
                Some(cancelled_at),
            ),
            "case {case:?}: a read inside the teardown window must report it and move nothing, \
             got {during:?}"
        );
        cancelled_rows.push((case, job_id, cancelled_at));
    }

    // The emulator stores the instant and never sleeps for it, so the test is
    // what waits; the margin keeps the wait off the span's own boundary.
    sleep(METADATA_TRANSFER_JOB_CANCEL_SPAN + Duration::from_millis(100));

    for (case, job_id, cancelled_at) in &cancelled_rows {
        let settled = emulator.get_metadata_transfer_job(job_id).await;
        assert_eq!(
            (settled.status, reported_state(&settled)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLED)),
            "case {case:?}: the clock must settle a cancelling job in CANCELLED, got {settled:?}"
        );
        // A cancelled job did not FAIL: the caller asked the transfer to stop,
        // so it carries no error code — unlike the outcome of a job that was
        // left to run (`METADATA_TRANSFER_JOB_ERROR_CODE`).
        assert!(
            settled.member("status")["error"]["code"].is_null(),
            "case {case:?}: a cancelled job reports no error code, got {settled:?}"
        );
        let settled_at = settled
            .member("updateDateTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: the read reports updateDateTime"));
        let span = METADATA_TRANSFER_JOB_CANCEL_SPAN.as_secs_f64();
        assert!(
            (settled_at - cancelled_at - span).abs() < 0.05,
            "case {case:?}: the settled job must report the instant the transition fell due \
             (cancelled at {cancelled_at}, span {span}), got {settled_at}"
        );
        let again = emulator.get_metadata_transfer_job(job_id).await;
        assert_eq!(
            (again.status, &again.body),
            (200, &settled.body),
            "case {case:?}: a read must report exactly what the previous read did, got {again:?}"
        );

        // A job that has settled has finished: cancelling it again is the
        // conflict, and the refusal leaves it exactly as it was.
        let refused = emulator.cancel_metadata_transfer_job(job_id).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: a cancelled job can no longer be cancelled, got {refused:?}"
        );
        let after = emulator.get_metadata_transfer_job(job_id).await;
        assert_eq!(
            (after.status, &after.body),
            (200, &settled.body),
            "case {case:?}: a refused cancel must change nothing a later caller sees, got \
             {after:?}"
        );
    }
}

/// RULE (the same rule, over the whole modelled enum): a cancellation is
/// accepted for a job in any NON-TERMINAL `MetadataTransferJobState` and
/// refused with the modelled 409 for a job in any TERMINAL one.
///
/// The emulator's own operations produce four of the seven values
/// (`VALIDATING` from a create, `ERROR` from its outcome, `CANCELLING` and
/// `CANCELLED` from a cancel), so the rest of the enum is reached by parking a
/// created job in each value through the store the handlers read — the seam
/// `Emulator::state` exists for.
#[tokio::test]
async fn cancelling_a_job_turns_only_on_whether_it_has_already_finished() {
    let emulator = emulator_with_workspaces().await;
    // The table spans every value the model's `MetadataTransferJobState` enum
    // defines, so a value added to the model fails here rather than going
    // untested.
    assert_eq!(
        METADATA_TRANSFER_JOB_STATE_VALUES.len(),
        7,
        "the table must span every modelled state, got {METADATA_TRANSFER_JOB_STATE_VALUES:?}"
    );

    for (index, state) in METADATA_TRANSFER_JOB_STATE_VALUES.iter().enumerate() {
        let job_id = format!("parked{index}");
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": job_id,
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {state:?}: the job the case parks must exist, got {created:?}"
        );

        // Park the job in the row's state. A non-terminal row keeps the outcome
        // the create owes it, but far enough out that nothing falls due while
        // the case runs, so the case exercises the cancel's rule and not the
        // clock's.
        let update_date_time = {
            let state_handle = emulator.state();
            let mut store = state_handle.lock();
            let job = store
                .region_mut(REGION)
                .metadata_transfer_jobs
                .get_mut(&job_id)
                .expect("the created job is in the store");
            job.state = (*state).to_string();
            job.settles_at = SystemTime::now() + Duration::from_secs(600);
            job.update_date_time
        };

        let cancelled = emulator.cancel_metadata_transfer_job(&job_id).await;
        if metadata_transfer_job_state_is_terminal(state) {
            assert_eq!(
                (cancelled.status, cancelled.error_type.as_deref()),
                (409, Some("ConflictException")),
                "case {state:?}: a job that has finished has nothing left to cancel, got \
                 {cancelled:?}"
            );
            let read = emulator.get_metadata_transfer_job(&job_id).await;
            assert_eq!(
                (read.status, reported_state(&read)),
                (200, Some(*state)),
                "case {state:?}: a refused cancel must leave the job in its own state, got \
                 {read:?}"
            );
            assert!(
                reports_instant(read.member("updateDateTime"), update_date_time),
                "case {state:?}: a refused cancel must not move updateDateTime, got {read:?}"
            );
            continue;
        }

        assert_eq!(
            (cancelled.status, reported_state(&cancelled)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLING)),
            "case {state:?}: a job that has not finished must be cancellable, got {cancelled:?}"
        );
        let read = emulator.get_metadata_transfer_job(&job_id).await;
        assert_eq!(
            (read.status, reported_state(&read)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLING)),
            "case {state:?}: the read-back must agree with the cancel, got {read:?}"
        );
        let reported_update = read.member("updateDateTime").as_f64();
        if *state == METADATA_TRANSFER_JOB_STATE_CANCELLING {
            // A job whose cancellation the service had already accepted is the
            // idempotent case: this call changes nothing, not even when the job
            // was last updated (see the idempotence suite).
            assert!(
                reports_instant(read.member("updateDateTime"), update_date_time),
                "case {state:?}: a cancel of a job already cancelling must change nothing, got \
                 {read:?}"
            );
        } else {
            assert!(
                reported_update > Some(epoch_seconds(update_date_time)),
                "case {state:?}: the cancel is an update, so updateDateTime moved, got {read:?}"
            );
        }
    }
}

/// A stored instant as the epoch seconds the wire carries it as, so a test can
/// compare a reported timestamp with the one the store held.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("a stored instant is after the epoch")
        .as_secs_f64()
}

/// Whether a reported timestamp is `at`, within the precision an epoch-seconds
/// JSON number keeps (a stored instant has nanosecond resolution, which a
/// double does not hold exactly).
fn reports_instant(value: &Value, at: SystemTime) -> bool {
    value
        .as_f64()
        .is_some_and(|seconds| (seconds - epoch_seconds(at)).abs() < 1e-3)
}

/// RULE: a cancellation already in flight is IDEMPOTENT — repeating the call
/// reports the same accepted status and neither moves the job's
/// `updateDateTime` nor pushes out the instant the clock owes `CANCELLED`.
#[tokio::test]
async fn cancelling_twice_while_the_teardown_is_in_flight_changes_nothing() {
    let emulator = emulator_with_workspaces().await;
    let job_ids = ["repeatOne", "repeatTwo"];

    for job_id in job_ids {
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": job_id,
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/repeat"},
                },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {job_id:?}: the job to cancel must exist, got {created:?}"
        );

        let first = emulator.cancel_metadata_transfer_job(job_id).await;
        assert_eq!(
            (first.status, reported_state(&first)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLING)),
            "case {job_id:?}: the first cancel is accepted, got {first:?}"
        );
        for repeat in 2..=3u32 {
            let again = emulator.cancel_metadata_transfer_job(job_id).await;
            assert_eq!(
                (again.status, &again.body),
                (200, &first.body),
                "case {job_id:?} (call {repeat}): repeating an accepted cancel must answer \
                 exactly what it answered, got {again:?}"
            );
        }
    }

    // The repeats did not push the teardown out: one span after the FIRST
    // cancel of each job, every one of them has settled.
    sleep(METADATA_TRANSFER_JOB_CANCEL_SPAN + Duration::from_millis(100));
    for job_id in job_ids {
        let settled = emulator.get_metadata_transfer_job(job_id).await;
        assert_eq!(
            (settled.status, reported_state(&settled)),
            (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLED)),
            "case {job_id:?}: a repeated cancel must not extend the teardown window, got \
             {settled:?}"
        );
    }
}

/// RULE: the cancel judges the job by the state the CLOCK had already made
/// true — a job whose outcome fell due before the request arrived has finished,
/// so cancelling it is the modelled 409 and leaves its outcome standing.
#[tokio::test]
async fn cancelling_a_job_whose_outcome_already_fell_due_is_the_conflict() {
    let emulator = emulator_with_workspaces().await;
    let job_id = "lateCancel";
    let created = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": job_id,
            "sources": [{
                "type": "s3",
                "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/late.json"},
            }],
            "destination": {
                "type": "iottwinmaker",
                "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
            },
        }))
        .await;
    assert_eq!(
        (created.status, reported_state(&created)),
        (200, Some(METADATA_TRANSFER_JOB_STATE_VALIDATING)),
        "the create must be accepted, got {created:?}"
    );

    // Nobody reads the job in between: the outcome falls due on the clock, so
    // the cancel is the call that has to materialise it.
    sleep(METADATA_TRANSFER_JOB_SETTLE_SPAN + Duration::from_millis(100));

    let refused = emulator.cancel_metadata_transfer_job(job_id).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (409, Some("ConflictException")),
        "a job whose outcome had already fallen due has nothing left to cancel, got {refused:?}"
    );
    let read = emulator.get_metadata_transfer_job(job_id).await;
    assert_eq!(
        (
            read.status,
            reported_state(&read),
            read.member("status")["error"]["code"].as_str(),
        ),
        (
            200,
            Some(METADATA_TRANSFER_JOB_STATE_ERROR),
            Some(METADATA_TRANSFER_JOB_ERROR_CODE),
        ),
        "the refused cancel must leave the job's own outcome standing, got {read:?}"
    );
}

/// RULE: a cancel of a job this account does not hold is refused — the modelled
/// 404 for a well-formed id it does not hold, the modelled 400 for an id
/// outside `Id` itself — and the refusal materialises and reserves nothing.
#[tokio::test]
async fn cancelling_a_job_the_account_does_not_hold_reserves_nothing() {
    let emulator = emulator_with_workspaces().await;

    // The rows differ along the id: well-formed ids at each end of `Id`'s
    // published length, and ids one past its maximum or outside its pattern.
    let refusals: Vec<(&str, String, (u16, &str))> = vec![
        (
            "absentId",
            "absentJob".to_string(),
            (404, "ResourceNotFoundException"),
        ),
        ("shortestId", "ab".to_string(), (404, "ResourceNotFoundException")),
        (
            "idAtMaxLength",
            format!("{}z", filled('a', ID_MAX - 1)),
            (404, "ResourceNotFoundException"),
        ),
        (
            "idPastMaxLength",
            filled('a', ID_MAX + 1),
            (400, "ValidationException"),
        ),
        (
            "idOutsidePattern",
            "-leadingDash".to_string(),
            (400, "ValidationException"),
        ),
    ];

    for (case, job_id, (status, error_type)) in &refusals {
        let refused = emulator.cancel_metadata_transfer_job(job_id).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (*status, Some(*error_type)),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        let read = emulator.get_metadata_transfer_job(job_id).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (*status, Some(*error_type)),
            "case {case:?}: a refused cancel must leave nothing to read, got {read:?}"
        );
    }

    assert!(
        emulator
            .state()
            .lock()
            .region(REGION)
            .metadata_transfer_jobs
            .is_empty(),
        "a refused cancel must materialise no job"
    );

    // The id a cancel was refused for is still a free job id, so a job created
    // under it afterwards is cancellable: the 404 reserved nothing.
    let created = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": "absentJob",
            "sources": [{
                "type": "s3",
                "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/free.json"},
            }],
            "destination": {
                "type": "iottwinmaker",
                "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
            },
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "an id a cancel was refused for must still be creatable, got {created:?}"
    );
    let cancelled = emulator.cancel_metadata_transfer_job("absentJob").await;
    assert_eq!(
        (cancelled.status, reported_state(&cancelled)),
        (200, Some(METADATA_TRANSFER_JOB_STATE_CANCELLING)),
        "the late job must be cancellable, got {cancelled:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `StopJob`: stopping a job that is in progress on a branch of an Amplify app.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `StopJob` stops a job that is IN PROGRESS: it answers the job's summary in the
//!    non-terminal `CANCELLING` status with no end time, the clock then settles that job
//!    to `CANCELLED`, and every later read agrees — whatever kind of job it is (a build
//!    `StartJob` started, a deployment `StartDeployment` started).
//! 2. A job that has already reached a terminal status has no progress left to stop, so
//!    it is refused with the modelled `BadRequestException` (HTTP 400), carrying that
//!    error class's fixed message, and is left exactly as it ended — including a job that
//!    was already stopped. The wording itself is spanned in
//!    `job_stop_terminal_wording.rs`.
//! 3. A job is stopped THROUGH its branch and its app, so an unknown app, an unknown
//!    branch, or a job id belonging to another branch is the modelled 404, and nothing
//!    is stopped on the way.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    JOB_ALREADY_TERMINAL_MESSAGE, JOB_CANCELLED_REASON, JOB_SETTLE_MILLIS,
};

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// The status `StopJob` answers with: `JobStatus` models both halves of stopping a job
/// (`smithy_query(query="shape",shape="JobStatus")`), and a stop that has been accepted
/// and not finished yet is `CANCELLING`.
const CANCELLING: &str = "CANCELLING";

/// The terminal status the clock settles a stopped job to.
const CANCELLED: &str = "CANCELLED";

/// How long a case waits before expecting the clock-owed status change.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 250)
}

/// `StopJob` over the wire.
async fn stop_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}/stop"),
            None,
        )
        .await
}

/// `GetJob` over the wire.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
}

/// A job started on a new branch of a new app, and the ids that address it.
///
/// `start` decides WHICH kind of job: a build `StartJob` runs on a repository-connected
/// app, or a deployment `StartDeployment` runs on a manually deployed one. The rule is
/// about a job in progress, not about how it was started, so both are rows of it.
async fn started_job(emulator: &Emulator, name: &str, kind: JobKind) -> (String, String) {
    let branch = "main";
    let app_id = match kind {
        JobKind::Build => emulator.app_id_of_new_repository_app(name).await,
        JobKind::Deployment => emulator.app_id_of_new_app(name).await,
    };
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
    let job_id = match kind {
        JobKind::Build => {
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/jobs"),
                    Some(&json!({ "jobType": "RELEASE" })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {name:?}: expected HTTP 200 from StartJob, got {started:?}"
            );
            string_member(&started.member("jobSummary"), "jobId")
        }
        JobKind::Deployment => {
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
                    Some(&json!({
                        "sourceUrl": "https://example.invalid/site.zip",
                        "sourceUrlType": "ZIP",
                    })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {name:?}: expected HTTP 200 from StartDeployment, got {started:?}"
            );
            string_member(&started.member("jobSummary"), "jobId")
        }
    };
    (app_id, job_id)
}

/// Which call started the job a case stops.
#[derive(Debug, Clone, Copy)]
enum JobKind {
    /// A build `StartJob` started.
    Build,
    /// A deployment `StartDeployment` started.
    Deployment,
}

/// Rule 1: a job in progress is stopped, answered as `CANCELLING`, and settled to
/// `CANCELLED` by the clock.
#[tokio::test]
async fn a_job_in_progress_is_stopped_and_settles_to_cancelled() {
    let emulator = Emulator::new();

    for (case, kind) in [
        ("a build started by StartJob", JobKind::Build),
        ("a deployment started by StartDeployment", JobKind::Deployment),
    ] {
        let (app_id, job_id) = started_job(&emulator, &format!("stop-{kind:?}"), kind).await;
        let stopped = stop_job(&emulator, &app_id, "main", &job_id).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: expected HTTP 200 from StopJob, got {stopped:?}"
        );
        let summary = stopped.member("jobSummary");
        assert_eq!(
            string_member(&summary, "jobId"),
            job_id,
            "case {case:?}: StopJob answers the job it stopped, got {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "status"),
            CANCELLING,
            "case {case:?}: a stop that has not finished is CANCELLING, got {summary:?}"
        );
        assert!(
            summary.get("endTime").is_none_or(Value::is_null),
            "case {case:?}: a job being cancelled has not ended, got {summary:?}"
        );

        // A read before the span elapses still reports the stop in flight: the span is
        // non-zero.
        let during = get_job(&emulator, &app_id, "main", &job_id).await.member("job");
        assert_eq!(
            string_member(
                &during.get("summary").cloned().unwrap_or(Value::Null),
                "status"
            ),
            CANCELLING,
            "case {case:?}: a read does not settle the cancellation early, got {during:?}"
        );

        sleep(past_the_settle_span());
        let settled = get_job(&emulator, &app_id, "main", &job_id).await.member("job");
        let summary = settled.get("summary").cloned().unwrap_or(Value::Null);
        assert_eq!(
            string_member(&summary, "status"),
            CANCELLED,
            "case {case:?}: the clock owes the stopped job its terminal status, got {settled:?}"
        );
        let steps = list_member(&settled, "steps");
        assert_eq!(
            steps.len(),
            1,
            "case {case:?}: a settled job carries one execution step, got {settled:?}"
        );
        assert_eq!(
            steps[0].get("statusReason").and_then(Value::as_str),
            Some(JOB_CANCELLED_REASON),
            "case {case:?}: the step says why the job ended, got {:?}",
            steps[0]
        );

        // Every later read agrees with the first.
        let again = get_job(&emulator, &app_id, "main", &job_id).await.member("job");
        assert_eq!(
            again, settled,
            "case {case:?}: a settled job reads the same every time, got {again:?}"
        );
    }
}

/// Rule 2: a job that has reached a terminal status cannot be stopped.
#[tokio::test]
async fn a_job_that_is_not_in_progress_is_refused() {
    let emulator = Emulator::new();

    // One row per way a job reaches a terminal status: the clock settling the build it
    // could not run, and a stop that already happened.
    let rows: Vec<(&str, bool)> = vec![
        ("a job the clock has settled", false),
        ("a job that was already stopped", true),
    ];

    for (case, stop_first) in rows {
        let (app_id, job_id) =
            started_job(&emulator, &format!("terminal-{}", case.len()), JobKind::Build).await;
        if stop_first {
            assert_eq!(
                stop_job(&emulator, &app_id, "main", &job_id).await.status,
                200,
                "case {case:?}: the first stop must be accepted"
            );
        }
        sleep(past_the_settle_span());
        let before = get_job(&emulator, &app_id, "main", &job_id).await.member("job");

        let refused = stop_job(&emulator, &app_id, "main", &job_id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {refused:?}"
        );
        let message = refused.message().unwrap_or_default();
        assert_eq!(
            message, JOB_ALREADY_TERMINAL_MESSAGE,
            "case {case:?}: the refusal carries this error class's fixed text, \
             got {message:?}"
        );

        // The refused call left the job exactly as it had ended.
        let after = get_job(&emulator, &app_id, "main", &job_id).await.member("job");
        assert_eq!(
            after, before,
            "case {case:?}: a refused stop changes nothing, got {after:?}"
        );
    }
}

/// Rule 3: a job is stopped only through its own app and branch.
#[tokio::test]
async fn a_job_is_stoppable_only_through_its_own_app_and_branch() {
    let emulator = Emulator::new();
    let (app_id, job_id) = started_job(&emulator, "stop-reachability", JobKind::Build).await;

    // A second branch of the same app, with no job of its own: the first branch's job id
    // names nothing there.
    let other = "feature-x";
    assert_eq!(
        emulator
            .create_branch(&app_id, &json!({ "branchName": other }))
            .await
            .status,
        200,
        "case \"the second branch\": expected HTTP 200 from CreateBranch"
    );

    // Each row: the case, the app, the branch and the job id the request addresses, and
    // the entity its 404 must name.
    let rows: Vec<(&str, String, String, String, &str)> = vec![
        (
            "an app that does not exist",
            "dnosuchapp1".to_string(),
            "main".to_string(),
            job_id.clone(),
            "App",
        ),
        (
            "a branch the app does not carry",
            app_id.clone(),
            "no-such-branch".to_string(),
            job_id.clone(),
            "Branch",
        ),
        (
            "a job id of another branch",
            app_id.clone(),
            other.to_string(),
            job_id.clone(),
            "Job",
        ),
        (
            "a job id nothing was started under",
            app_id.clone(),
            "main".to_string(),
            "4242".to_string(),
            "Job",
        ),
    ];

    for (case, app, branch, job, entity) in rows {
        let refused = stop_job(&emulator, &app, &branch, &job).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("NotFoundException"),
            "case {case:?}: expected the modelled NotFoundException, got {refused:?}"
        );
        let message = refused.message().unwrap_or_default();
        assert!(
            message.starts_with(entity),
            "case {case:?}: the 404 must name the absent {entity}, got {message:?}"
        );
    }

    // Nothing was stopped on the way: the job is still the one that was started.
    let job = get_job(&emulator, &app_id, "main", &job_id).await.member("job");
    assert_ne!(
        string_member(&job.get("summary").cloned().unwrap_or(Value::Null), "status"),
        CANCELLING,
        "case \"the started job\": a refused stop must not have stopped it, got {job:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! `StartJob`'s REPOSITORY precondition: the operation runs a build of the app's
//! connected repository, so it is supported only for an app that has one.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. `StartJob` on an app with NO repository provider is refused with the modelled
//!    `BadRequestException` (HTTP 400) and the service's fixed sentence, for EVERY
//!    `jobType` and whether or not the request carries a `jobId` — the precondition is
//!    the app's, so it is judged before the request's own parameter rules (the `jobId`
//!    a RETRY requires), and the refusal records nothing: the branch's job history is
//!    still empty afterwards.
//! 2. The same calls on an app that IS connected to a repository are judged by those
//!    parameter rules instead: a RETRY with no `jobId` takes the member refusal, and
//!    every other `jobType` starts a job.
//! 3. The precondition is the APP's, so an app id that names nothing is still the
//!    modelled 404 — there is no app whose repository could be judged.

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::ops::start_job::REPOSITORY_NOT_CONNECTED;

use crate::app_branch_lifecycle::{list_member, string_member, Emulator, Reply};

/// Every value `JobType` accepts (`smithy_query(query="shape",shape="JobType")`:
/// `RELEASE | RETRY | MANUAL | WEB_HOOK`). The precondition is about the APP, not about
/// which job was asked for, so every row of the table is one of these.
const JOB_TYPES: &[&str] = &["RELEASE", "RETRY", "MANUAL", "WEB_HOOK"];

/// The `jobType` that re-runs an existing job, and so the one the model requires a
/// `jobId` with.
const RETRY: &str = "RETRY";

/// `StartJob` over the wire.
async fn start_job(emulator: &Emulator, app_id: &str, branch: &str, request: &Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch}/jobs"),
            Some(request),
        )
        .await
}

/// `ListJobs` over the wire.
async fn list_jobs(emulator: &Emulator, app_id: &str, branch: &str) -> Reply {
    emulator
        .call("GET", &format!("/apps/{app_id}/branches/{branch}/jobs"), None)
        .await
}

/// A branch on `app_id`.
async fn branch_on(emulator: &Emulator, app_id: &str, branch: &str) {
    let created = emulator
        .create_branch(app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "app {app_id}: expected HTTP 200 from CreateBranch, got {created:?}"
    );
}

/// The requests the rule spans: every `jobType`, each with no `jobId`, with a `jobId`
/// no job was ever started under, and with the `jobId` of the branch's first job.
fn requests() -> Vec<(String, Value)> {
    let mut rows = Vec::new();
    for job_type in JOB_TYPES {
        rows.push((
            format!("{job_type} with no jobId"),
            json!({ "jobType": job_type }),
        ));
        rows.push((
            format!("{job_type} with an absent jobId"),
            json!({ "jobType": job_type, "jobId": "4242" }),
        ));
        rows.push((
            format!("{job_type} with the first jobId"),
            json!({ "jobType": job_type, "jobId": "1" }),
        ));
    }
    rows
}

/// Rule 1: every `StartJob` on a repository-less app takes the fixed 400, and records
/// nothing.
#[tokio::test]
async fn start_job_on_an_app_without_a_repository_is_not_supported() {
    let emulator = Emulator::new();
    let branch = "main";

    // One row per (jobType, jobId) shape, each on its OWN repository-less app, so no
    // row can be answered out of another row's leftovers.
    for (case, request) in requests() {
        let app_id = emulator
            .app_id_of_new_app(&format!("no-repository-{}", case.replace(' ', "-")))
            .await;
        branch_on(&emulator, &app_id, branch).await;

        let refused = start_job(&emulator, &app_id, branch, &request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {refused:?}"
        );
        assert_eq!(
            refused.message().as_deref(),
            Some(REPOSITORY_NOT_CONNECTED),
            "case {case:?}: expected the repository-provider refusal, got {:?}",
            refused.message()
        );

        // The refusal records nothing: the branch has started no job, and its history
        // is still empty.
        let listed = list_jobs(&emulator, &app_id, branch).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListJobs, got {listed:?}"
        );
        assert!(
            list_member(&listed.body, "jobSummaries").is_empty(),
            "case {case:?}: a refused StartJob starts no job, got {listed:?}"
        );
        let record = emulator.get_branch(&app_id, branch).await.member("branch");
        assert_eq!(
            string_member(&record, "totalNumberOfJobs"),
            "0",
            "case {case:?}: a refused StartJob is not a started job, got {record:?}"
        );
    }
}

/// Rule 2: on an app that IS connected to a repository the same requests are judged by
/// the `jobId` rules instead.
#[tokio::test]
async fn start_job_on_a_repository_app_is_judged_by_the_job_id_rules() {
    let emulator = Emulator::new();
    let branch = "main";

    for job_type in JOB_TYPES {
        let app_id = emulator
            .app_id_of_new_repository_app(&format!("repository-{job_type}"))
            .await;
        branch_on(&emulator, &app_id, branch).await;

        let reply = start_job(&emulator, &app_id, branch, &json!({ "jobType": job_type })).await;
        if *job_type == RETRY {
            // The member refusal, not the app one: this app HAS a repository, so what
            // is wrong with the request is the missing `jobId`.
            assert_eq!(
                reply.status, 400,
                "case {job_type:?}: a RETRY without a jobId must be refused, got {reply:?}"
            );
            let message = reply.message().unwrap_or_default();
            assert!(
                message.contains("jobId") && message.contains(RETRY),
                "case {job_type:?}: the refusal must name the member and the condition, \
                 got {message:?}"
            );
            assert_ne!(
                message, REPOSITORY_NOT_CONNECTED,
                "case {job_type:?}: a connected app is not refused for its repository"
            );
        } else {
            assert_eq!(
                reply.status, 200,
                "case {job_type:?}: a job of a connected app starts, got {reply:?}"
            );
            assert_eq!(
                string_member(&reply.member("jobSummary"), "jobId"),
                "1",
                "case {job_type:?}: the branch mints the job's id, got {reply:?}"
            );
        }
    }
}

/// Rule 3: an app id that names nothing is still the modelled 404, whatever the request
/// asks for — there is no app whose repository could be judged.
#[tokio::test]
async fn an_absent_app_is_still_the_modelled_not_found() {
    let emulator = Emulator::new();

    // Every identifier here is inside `AppId`'s modelled `pattern ^d[a-z0-9]+$`, so
    // what is being tested is the handler's lookup and not the boundary validator.
    for absent_app in ["d0", "dnosuchapp1", "d9999999999999999999"] {
        for (case, request) in requests() {
            let refused = start_job(&emulator, absent_app, "main", &request).await;
            assert_eq!(
                refused.status, 404,
                "case {absent_app}/{case:?}: expected HTTP 404, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("NotFoundException"),
                "case {absent_app}/{case:?}: expected the modelled NotFoundException, \
                 got {refused:?}"
            );
        }
    }
}

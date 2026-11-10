// SPDX-License-Identifier: CC0-1.0

//! `StopJob` on a job that has already reached a terminal status: the wording of the
//! refusal.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see.
//!
//! The rule under test: `BadRequestException` is the one modelled 400 this surface
//! reuses for every bad request ("A request contains unexpected data",
//! `smithy_query(query="operation",operation="StopJob")`), so the MESSAGE is what
//! discriminates the error classes from each other, and the message of a refusal that is
//! about the addressed resource's STATE rather than about an identifier the request
//! supplied is FIXED TEXT: it carries no job id, branch name, app id or status, and it
//! is the same string for every job in every terminal status.
//!
//! The table below spans that class: one row per terminal `JobStatus` a job of this
//! service reaches — `FAILED`, `SUCCEED` and `CANCELLED`
//! (`smithy_query(query="shape",shape="JobStatus")` models `CREATED | PENDING |
//! PROVISIONING | RUNNING | FAILED | SUCCEED | CANCELLING | CANCELLED`, of which those
//! three are the terminal ones) — reached the way each is actually reached: a build the
//! clock settled, a manual deployment that published its content, and a job a previous
//! `StopJob` stopped.
//!
//! The harness (`Emulator`, `Reply`) is the one `app_branch_lifecycle` already drives.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::{
    JOB_ALREADY_TERMINAL_MESSAGE, JOB_SETTLE_MILLIS, JOB_STATUS_CANCELLED, JOB_STATUS_FAILED,
    JOB_STATUS_SUCCEED,
};

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};

/// How long a case waits before expecting the status change the clock already owed.
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

/// `GetJob` over the wire, reduced to the `job` member.
async fn get_job(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> Value {
    emulator
        .call(
            "GET",
            &format!("/apps/{app_id}/branches/{branch}/jobs/{job_id}"),
            None,
        )
        .await
        .member("job")
}

/// The `status` of a job read back through `GetJob`.
async fn job_status(emulator: &Emulator, app_id: &str, branch: &str, job_id: &str) -> String {
    let job = get_job(emulator, app_id, branch, job_id).await;
    string_member(&job.get("summary").cloned().unwrap_or(Value::Null), "status")
}

/// Which terminal status a row drives the job to, and how it gets there.
#[derive(Debug, Clone, Copy)]
enum HowItEnded {
    /// A build `StartJob` started, which the clock settles (this emulator runs no
    /// build), ending `FAILED`.
    BuildSettled,
    /// A manual deployment whose uploaded archive carries the site's root document, so
    /// the content is published and the job ends `SUCCEED`.
    DeploymentPublished,
    /// A job a previous `StopJob` stopped, which the clock settles to `CANCELLED`.
    StoppedAlready,
}

/// A `.zip` carrying each named path with the content given, as a real archive a manual
/// deployment can be.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for (path, content) in files {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        std::io::Write::write_all(&mut writer, content).expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// Drive one row to its terminal status: a new app with one branch, a job on it, and the
/// clock past whatever that job was owed. Answers the app id, the branch name and the
/// job id addressing it.
async fn job_in_terminal_status(
    emulator: &Emulator,
    case: &str,
    branch: &str,
    how: HowItEnded,
) -> (String, String, String) {
    let app_id = match how {
        // `StartJob` builds the app's connected repository, so it is served only for an
        // app that has one.
        HowItEnded::BuildSettled | HowItEnded::StoppedAlready => {
            emulator.app_id_of_new_repository_app(case).await
        }
        HowItEnded::DeploymentPublished => emulator.app_id_of_new_app(case).await,
    };
    let created = emulator
        .create_branch(&app_id, &json!({ "branchName": branch }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateBranch, got {created:?}"
    );

    let job_id = match how {
        HowItEnded::BuildSettled | HowItEnded::StoppedAlready => {
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/jobs"),
                    Some(&json!({ "jobType": "RELEASE" })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartJob, got {started:?}"
            );
            let job_id = string_member(&started.member("jobSummary"), "jobId");
            if matches!(how, HowItEnded::StoppedAlready) {
                let stopped = stop_job(emulator, &app_id, branch, &job_id).await;
                assert_eq!(
                    stopped.status, 200,
                    "case {case:?}: the first stop must be accepted, got {stopped:?}"
                );
            }
            job_id
        }
        HowItEnded::DeploymentPublished => {
            let reserved = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/deployments"),
                    Some(&json!({})),
                )
                .await;
            assert_eq!(
                reserved.status, 200,
                "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
            );
            let job_id = string_member(&reserved.body, "jobId");
            let upload = http::Request::builder()
                .method("PUT")
                .uri(string_member(&reserved.body, "zipUploadUrl"))
                .body(bytes::Bytes::from(zip_of(&[(
                    "index.html",
                    b"<html>deployed</html>" as &[u8],
                )])))
                .expect("an upload destination is a well-formed URL");
            assert_eq!(
                emulator.dispatch(upload).await.status().as_u16(),
                200,
                "case {case:?}: the destination must accept the upload"
            );
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/{branch}/deployments/start"),
                    Some(&json!({ "jobId": job_id })),
                )
                .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected HTTP 200 from StartDeployment, got {started:?}"
            );
            job_id
        }
    };

    sleep(past_the_settle_span());
    (app_id, branch.to_string(), job_id)
}

/// The rule: the refusal of a `StopJob` on a terminal job is the modelled 400 with the
/// FIXED message of that error class, for every terminal status and every job.
#[tokio::test]
async fn refusing_a_terminal_job_is_worded_the_same_for_every_job() {
    let emulator = Emulator::new();

    // One row per terminal `JobStatus` a job of this service reaches, and the branch the
    // row drives it on. The branch names differ so that a message quoting one of them
    // could not pass by coincidence.
    let rows: Vec<(&str, &str, HowItEnded, &str)> = vec![
        (
            "a build the clock settled",
            "trunk",
            HowItEnded::BuildSettled,
            JOB_STATUS_FAILED,
        ),
        (
            "a manual deployment that published its content",
            "staging-two",
            HowItEnded::DeploymentPublished,
            JOB_STATUS_SUCCEED,
        ),
        (
            "a job a previous StopJob stopped",
            "feature-three",
            HowItEnded::StoppedAlready,
            JOB_STATUS_CANCELLED,
        ),
    ];

    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (case, branch, how, expected_status) in rows {
        let (app_id, branch, job_id) = job_in_terminal_status(&emulator, case, branch, how).await;
        let status = job_status(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: the row must reach {expected_status}, got {status:?}"
        );
        let before = get_job(&emulator, &app_id, &branch, &job_id).await;

        // Twice: the refusal is a function of the job's state, so a second identical
        // call is answered identically.
        for attempt in 1..=2 {
            let refused = stop_job(&emulator, &app_id, &branch, &job_id).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?} attempt {attempt}: expected HTTP 400, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("BadRequestException"),
                "case {case:?} attempt {attempt}: expected the modelled \
                 BadRequestException, got {refused:?}"
            );
            let message = refused.message().unwrap_or_default();
            assert_eq!(
                message, JOB_ALREADY_TERMINAL_MESSAGE,
                "case {case:?} attempt {attempt}: expected the fixed text of this error \
                 class, got {message:?}"
            );
            // Nothing of the request or of state is interpolated into it.
            for borrowed in [job_id.as_str(), branch.as_str(), app_id.as_str(), &status] {
                assert!(
                    !message.contains(borrowed),
                    "case {case:?} attempt {attempt}: the message must carry no \
                     identifier or status, got {borrowed:?} in {message:?}"
                );
            }
            messages.insert(message);
        }

        // A refused stop changes nothing: the job still reads exactly as it ended.
        let after = get_job(&emulator, &app_id, &branch, &job_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused stop changes nothing, got {after:?}"
        );
    }

    assert_eq!(
        messages.len(),
        1,
        "case \"every terminal status\": the class has ONE message, got {messages:?}"
    );
}

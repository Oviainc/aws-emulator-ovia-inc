// SPDX-License-Identifier: CC0-1.0

//! `ListServiceJobs`, end to end over the wire.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a listing reports the service jobs of the ONE job queue the request names — by name
//!   or by ARN, the two forms `jobQueue` documents — sorted by `createdAt` with the most
//!   recent first, every summary member read from the persisted record; a reference this
//!   service holds no queue for selects nothing, and naming no queue at all is the
//!   modelled `ClientException` (HTTP 400);
//! * `jobStatus` selects the status, and "if you don't specify a status, only RUNNING jobs
//!   are returned";
//! * a `filters` entry selects instead: only one filter may be used at a time, `jobStatus`
//!   is IGNORED while one is used except alongside `SHARE_IDENTIFIER` or
//!   `QUOTA_SHARE_NAME`, and the documented names are `JOB_NAME` (case-insensitive, a
//!   trailing `*` matching a prefix), `BEFORE_CREATED_AT`, `AFTER_CREATED_AT`,
//!   `SHARE_IDENTIFIER` and `QUOTA_SHARE_NAME`; anything else is the modelled
//!   `ClientException`;
//! * paging with `maxResults`/`nextToken` sees every job exactly once, `maxResults` is
//!   accepted on both published bounds and refused one past either, and an unusable
//!   `nextToken` is the modelled `ClientException`;
//! * a listing is a READ: two listings at the same instant answer identically, and
//!   `isTerminated` appears only on a service job a terminate ended.
//!
//! A record this suite lists is submitted over the wire and then PINNED in the store: the
//! clock-owed chain a fresh service job walks (`SUBMITTED` -> `RUNNABLE` -> the modelled
//! failure state) is what `tests/all/service_job_submission.rs` covers, while what is
//! under test here is the listing, so each record is held in the status it was accepted in
//! and given a known `createdAt` — which is also what gives the newest-first order and the
//! created-at filters distinct instants to select on, two submits inside one millisecond
//! testing neither.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{error_code, error_message, Batch};
use crate::service_job_submission::{sagemaker_queue, service_environment, submitted};
use crate::service_job_termination::terminate;

/// The only `ServiceJobType` the model declares
/// (`smithy_query(query="shape", shape="ServiceJobType")`: SAGEMAKER_TRAINING).
const SERVICE_JOB_TYPE: &str = "SAGEMAKER_TRAINING";

/// `ListServiceJobs`'s route (`smithy_query(query="operation",
/// operation="ListServiceJobs")`: POST /v1/listservicejobs).
const LIST: &str = "/v1/listservicejobs";

/// `ListServiceJobs` over the wire: the status and the decoded body.
async fn list(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call(LIST, body).await
}

/// `ListServiceJobs` over the wire, requiring the 200.
async fn listed(batch: &Batch, case: &str, body: Value) -> Value {
    let (status, decoded) = list(batch, body).await;
    assert_eq!(
        status, 200,
        "case {case}: expected ListServiceJobs to answer 200, got {status} {decoded}"
    );
    decoded
}

/// The `jobName`s of a listing's `jobSummaryList`, in the order it answered them.
fn names(body: &Value) -> Vec<String> {
    body["jobSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["jobName"]
                .as_str()
                .unwrap_or("<no jobName>")
                .to_string()
        })
        .collect()
}

/// The `jobId`s of a listing's `jobSummaryList`, in the order it answered them.
fn ids(body: &Value) -> Vec<String> {
    body["jobSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or("<no jobId>").to_string())
        .collect()
}

/// Hold the service job `job_id` in the status it was accepted in and give it the known
/// `created_at`. See this module's docs: the clock-owed chain is covered elsewhere, so the
/// listing is asserted against records that cannot move underneath it.
fn pin(batch: &Batch, job_id: &str, created_at: i64) {
    let store = batch.store();
    let mut store = store.lock();
    let job = store
        .service_jobs
        .get_mut(job_id)
        .expect("the submitted service job is in the store");
    job.created_at = created_at;
    job.next_phase_due_at = None;
}

/// One service job this suite lists: what the submit carries and when it was created.
struct Submission {
    job_name: &'static str,
    share_identifier: Option<&'static str>,
    quota_share_name: Option<&'static str>,
    /// How far BEFORE the suite's reference instant this record was created.
    created_before: i64,
}

/// Submit `submission` to `queue` and pin it; answers its `jobId`.
async fn submit_pinned(batch: &Batch, queue: &str, base: i64, submission: &Submission) -> String {
    let mut body = json!({
        "jobName": submission.job_name,
        "jobQueue": queue,
        "serviceJobType": SERVICE_JOB_TYPE,
        "serviceRequestPayload": format!(r#"{{"TrainingJobName":"{}"}}"#, submission.job_name),
    });
    if let Some(share) = submission.share_identifier {
        body["shareIdentifier"] = json!(share);
    }
    if let Some(quota) = submission.quota_share_name {
        body["quotaShareName"] = json!(quota);
    }
    let accepted = submitted(batch, submission.job_name, body).await;
    let job_id = accepted["jobId"].as_str().unwrap_or_default().to_string();
    pin(batch, &job_id, base - submission.created_before);
    job_id
}

/// The table this suite lists: rows differ in `jobName` length, alphabet and case, in
/// whether they carry a `shareIdentifier` or a `quotaShareName`, and in when they were
/// created.
fn submissions() -> Vec<Submission> {
    vec![
        Submission {
            job_name: "sj-alpha",
            share_identifier: Some("share-a"),
            quota_share_name: Some("quota-a"),
            created_before: 3_000,
        },
        Submission {
            job_name: "SJ-Alpha-2",
            share_identifier: Some("share-b"),
            quota_share_name: None,
            created_before: 2_000,
        },
        Submission {
            job_name: "sj-beta",
            share_identifier: Some("share-a"),
            quota_share_name: Some("quota-b"),
            created_before: 1_000,
        },
        Submission {
            job_name: "o",
            share_identifier: None,
            quota_share_name: None,
            created_before: 0,
        },
    ]
}

/// One `SAGEMAKER_TRAINING` queue holding [`submissions`], and a second, empty one beside
/// it; answers both queues' names and ARNs and the reference instant the records were
/// created around.
async fn fixture(batch: &Batch) -> (String, String, i64) {
    let environment = service_environment(batch, "se-listing").await;
    let filled = sagemaker_queue(batch, "q-listing", &environment).await;
    let empty = sagemaker_queue(batch, "q-listing-empty", &environment).await;
    let base = state::now_millis();
    for submission in submissions() {
        submit_pinned(batch, "q-listing", base, &submission).await;
    }
    (filled, empty, base)
}

/// RULE: a listing reports the service jobs of the one queue the request names, resolving
/// it by name or by ARN, newest first by `createdAt`, and reports NOTHING for a queue that
/// holds none or that this service does not hold at all.
#[tokio::test]
async fn a_listing_reports_one_queue_s_service_jobs_newest_first() {
    let batch = Batch::new();
    let (filled_arn, empty_arn, _) = fixture(&batch).await;
    let newest_first = vec![
        "o".to_string(),
        "sj-beta".to_string(),
        "SJ-Alpha-2".to_string(),
        "sj-alpha".to_string(),
    ];

    // The rows differ in which queue is named and in which of the two documented forms
    // ("You can specify either the name or the ARN of the queue"), including references
    // this service holds no queue for.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        ("the filled queue by name", "q-listing".to_string(), newest_first.clone()),
        ("the filled queue by ARN", filled_arn.clone(), newest_first.clone()),
        ("a queue holding no service job, by name", "q-listing-empty".to_string(), Vec::new()),
        ("a queue holding no service job, by ARN", empty_arn, Vec::new()),
        ("a queue name nothing answers to", "q-absent".to_string(), Vec::new()),
        (
            "a queue ARN nothing answers to",
            state::job_queue_arn("us-east-1", "q-absent"),
            Vec::new(),
        ),
    ];
    for (case, reference, expected) in cases {
        let body = listed(
            &batch,
            case,
            json!({"jobQueue": reference, "jobStatus": state::SERVICE_JOB_SUBMITTED}),
        )
        .await;
        assert_eq!(
            names(&body),
            expected,
            "case {case}: expected {expected:?} newest first, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {case}: a page that ends the results carries no nextToken, got {body}"
        );
        // A read: the same request at the same instant answers identically.
        let again = listed(&batch, case, json!({"jobQueue": reference, "jobStatus": state::SERVICE_JOB_SUBMITTED})).await;
        assert_eq!(
            again, body,
            "case {case}: listing twice must answer identically, got {again} then {body}"
        );
    }
}

/// RULE: every summary member is read from the persisted record, and a member the record
/// does not carry is ABSENT — including `isTerminated`, which "is only present when the
/// value is true", and the attempt and capacity members of work nothing dispatched.
#[tokio::test]
async fn a_summary_reports_the_persisted_service_job() {
    let batch = Batch::new();
    let (_, _, base) = fixture(&batch).await;

    let body = listed(
        &batch,
        "every member",
        json!({"jobQueue": "q-listing", "jobStatus": state::SERVICE_JOB_SUBMITTED}),
    )
    .await;
    let summaries = body["jobSummaryList"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        submissions().len(),
        "case every member: expected every submitted service job, got {body}"
    );

    for summary in &summaries {
        let case = summary["jobName"].as_str().unwrap_or("<no jobName>").to_string();
        let job_id = summary["jobId"].as_str().unwrap_or_default().to_string();
        let submission = submissions()
            .into_iter()
            .find(|row| row.job_name == case)
            .unwrap_or_else(|| panic!("case {case}: the listing answered a job nothing submitted, got {summary}"));
        let stored = {
            let store = batch.store();
            let store = store.lock();
            store
                .service_jobs
                .get(&job_id)
                .cloned()
                .unwrap_or_else(|| panic!("case {case}: no stored service job for {job_id}"))
        };
        assert_eq!(
            summary["jobArn"].as_str(),
            Some(stored.job_arn.as_str()),
            "case {case}: expected the stored jobArn {:?}, got {summary}",
            stored.job_arn
        );
        assert_eq!(
            summary["serviceJobType"].as_str(),
            Some(SERVICE_JOB_TYPE),
            "case {case}: expected the submitted serviceJobType, got {summary}"
        );
        assert_eq!(
            summary["status"].as_str(),
            Some(state::SERVICE_JOB_SUBMITTED),
            "case {case}: expected the persisted status, got {summary}"
        );
        assert_eq!(
            summary["createdAt"].as_i64(),
            Some(base - submission.created_before),
            "case {case}: expected the persisted createdAt, got {summary}"
        );
        assert_eq!(
            summary.get("shareIdentifier").and_then(Value::as_str),
            submission.share_identifier,
            "case {case}: expected the submitted shareIdentifier, got {summary}"
        );
        assert_eq!(
            summary.get("quotaShareName").and_then(Value::as_str),
            submission.quota_share_name,
            "case {case}: expected the submitted quotaShareName, got {summary}"
        );
        for absent in [
            "statusReason",
            "scheduledAt",
            "startedAt",
            "stoppedAt",
            "isTerminated",
            "latestAttempt",
            "capacityUsage",
        ] {
            assert!(
                summary.get(absent).is_none(),
                "case {case}: {absent} is nothing this service job carries, got {summary}"
            );
        }
    }
}

/// RULE: `jobStatus` selects the status, and a request that names none selects only
/// `RUNNING` service jobs.
#[tokio::test]
async fn a_listing_selects_the_status_asked_for_and_running_by_default() {
    let batch = Batch::new();
    fixture(&batch).await;
    let submitted_names = vec![
        "o".to_string(),
        "sj-beta".to_string(),
        "SJ-Alpha-2".to_string(),
        "sj-alpha".to_string(),
    ];

    // Every `ServiceJobStatus` the model declares (`smithy_query(query="shape",
    // shape="ServiceJobStatus")`), beside the request that names no status at all: only
    // the status the records are in selects them.
    let statuses = [
        "SUBMITTED", "PENDING", "RUNNABLE", "SCHEDULED", "STARTING", "RUNNING", "SUCCEEDED",
        "FAILED",
    ];
    for status in statuses {
        let body = listed(&batch, status, json!({"jobQueue": "q-listing", "jobStatus": status})).await;
        let expected = if status == state::SERVICE_JOB_SUBMITTED {
            submitted_names.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            names(&body),
            expected,
            "case {status}: expected {expected:?}, got {body}"
        );
    }

    // "If you don't specify a status, only RUNNING jobs are returned" — and nothing here
    // dispatches a service job's work, so nothing is RUNNING.
    let body = listed(&batch, "no jobStatus", json!({"jobQueue": "q-listing"})).await;
    assert_eq!(
        names(&body),
        Vec::<String>::new(),
        "case no jobStatus: the default selects only RUNNING service jobs, got {body}"
    );
}

/// RULE: a `filters` entry selects instead of `jobStatus` — which is ignored while one is
/// used, except alongside `SHARE_IDENTIFIER` or `QUOTA_SHARE_NAME` — and each documented
/// filter name selects what its own doc says it does.
#[tokio::test]
async fn a_listing_honours_every_documented_filter() {
    let batch = Batch::new();
    let (_, _, base) = fixture(&batch).await;

    // Rows differ in the filter name, in how many values it carries, in the case and the
    // `*` suffix of a `JOB_NAME` value, and in whether `jobStatus` is named beside it.
    let cases: Vec<(&str, Value, Option<&str>, Vec<&str>)> = vec![
        (
            "JOB_NAME is an exact, case-insensitive match",
            json!([{"name": "JOB_NAME", "values": ["SJ-ALPHA"]}]),
            None,
            vec!["sj-alpha"],
        ),
        (
            "JOB_NAME with a trailing * matches a prefix",
            json!([{"name": "JOB_NAME", "values": ["sj-alpha*"]}]),
            None,
            vec!["SJ-Alpha-2", "sj-alpha"],
        ),
        (
            "JOB_NAME with several values matches any of them",
            json!([{"name": "JOB_NAME", "values": ["sj-beta", "o"]}]),
            None,
            vec!["o", "sj-beta"],
        ),
        (
            "a filter makes jobStatus ignored",
            json!([{"name": "JOB_NAME", "values": ["sj-*"]}]),
            Some("FAILED"),
            vec!["sj-beta", "SJ-Alpha-2", "sj-alpha"],
        ),
        (
            "BEFORE_CREATED_AT keeps what was created no later",
            json!([{"name": "BEFORE_CREATED_AT", "values": [(base - 1_500).to_string()]}]),
            None,
            vec!["SJ-Alpha-2", "sj-alpha"],
        ),
        (
            "AFTER_CREATED_AT keeps what was created no earlier",
            json!([{"name": "AFTER_CREATED_AT", "values": [(base - 1_500).to_string()]}]),
            None,
            vec!["o", "sj-beta"],
        ),
        (
            "SHARE_IDENTIFIER is used together with jobStatus",
            json!([{"name": "SHARE_IDENTIFIER", "values": ["share-a"]}]),
            Some(state::SERVICE_JOB_SUBMITTED),
            vec!["sj-beta", "sj-alpha"],
        ),
        (
            "SHARE_IDENTIFIER alone still defaults jobStatus to RUNNING",
            json!([{"name": "SHARE_IDENTIFIER", "values": ["share-a"]}]),
            None,
            Vec::new(),
        ),
        (
            "QUOTA_SHARE_NAME is used together with jobStatus",
            json!([{"name": "QUOTA_SHARE_NAME", "values": ["quota-a", "quota-b"]}]),
            Some(state::SERVICE_JOB_SUBMITTED),
            vec!["sj-beta", "sj-alpha"],
        ),
        (
            "QUOTA_SHARE_NAME alone still defaults jobStatus to RUNNING",
            json!([{"name": "QUOTA_SHARE_NAME", "values": ["quota-a"]}]),
            None,
            Vec::new(),
        ),
        (
            "a filter value nothing carries selects nothing",
            json!([{"name": "SHARE_IDENTIFIER", "values": ["share-absent"]}]),
            Some(state::SERVICE_JOB_SUBMITTED),
            Vec::new(),
        ),
    ];
    for (case, filters, job_status, expected) in cases {
        let mut request = json!({"jobQueue": "q-listing", "filters": filters});
        if let Some(status) = job_status {
            request["jobStatus"] = json!(status);
        }
        let body = listed(&batch, case, request).await;
        assert_eq!(
            names(&body),
            expected,
            "case {case}: expected {expected:?} newest first, got {body}"
        );
    }
}

/// RULE: a request this operation cannot answer is the modelled `ClientException` (HTTP
/// 400): no job queue named, more than one filter, a filter name the operation does not
/// document, a created-at value that is not a number of milliseconds, a `maxResults`
/// outside the published 1..100, and a `nextToken` that resumes nothing.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let batch = Batch::new();
    fixture(&batch).await;

    // Rows differ in WHICH part of the request is unusable; each must be the modelled
    // 400, never a 5xx or a 501.
    let cases: Vec<(&str, Value)> = vec![
        ("no jobQueue at all", json!({})),
        ("an empty jobQueue", json!({"jobQueue": ""})),
        (
            "two filters at once",
            json!({"jobQueue": "q-listing", "filters": [
                {"name": "JOB_NAME", "values": ["sj-alpha"]},
                {"name": "SHARE_IDENTIFIER", "values": ["share-a"]},
            ]}),
        ),
        (
            "a filter name the operation does not document",
            json!({"jobQueue": "q-listing", "filters": [{"name": "NOT_A_FILTER", "values": ["x"]}]}),
        ),
        (
            "a filter entry carrying no name",
            json!({"jobQueue": "q-listing", "filters": [{"values": ["x"]}]}),
        ),
        (
            "a BEFORE_CREATED_AT value that is not milliseconds",
            json!({"jobQueue": "q-listing", "filters": [
                {"name": "BEFORE_CREATED_AT", "values": ["yesterday"]},
            ]}),
        ),
        (
            "a AFTER_CREATED_AT value that is not milliseconds",
            json!({"jobQueue": "q-listing", "filters": [
                {"name": "AFTER_CREATED_AT", "values": ["1.5"]},
            ]}),
        ),
        (
            "maxResults one below the published minimum",
            json!({
                "jobQueue": "q-listing",
                "maxResults": state::MIN_LIST_SERVICE_JOBS_RESULTS - 1,
            }),
        ),
        (
            "maxResults one past the published maximum",
            json!({
                "jobQueue": "q-listing",
                "maxResults": state::MAX_LIST_SERVICE_JOBS_RESULTS + 1,
            }),
        ),
        (
            "a nextToken that resumes nothing",
            json!({"jobQueue": "q-listing", "maxResults": 1, "nextToken": "not-a-token"}),
        ),
    ];
    for (case, request) in cases {
        let (status, body) = list(&batch, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException with HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected ClientException, got {body} ({})",
            error_message(&body)
        );
    }
}

/// RULE: paging with `maxResults`/`nextToken` sees every service job exactly once, in the
/// newest-first order, and `maxResults` is accepted on both published bounds.
#[tokio::test]
async fn paging_sees_every_service_job_exactly_once() {
    let batch = Batch::new();
    fixture(&batch).await;

    let all = listed(
        &batch,
        "unpaged",
        json!({"jobQueue": "q-listing", "jobStatus": state::SERVICE_JOB_SUBMITTED}),
    )
    .await;
    let expected = ids(&all);
    assert_eq!(
        expected.len(),
        submissions().len(),
        "case unpaged: expected every submitted service job, got {all}"
    );

    // Page sizes differ: one at a time, a size that does not divide the total, the exact
    // total, and both published bounds of `maxResults` (1 and 100).
    for page_size in [
        state::MIN_LIST_SERVICE_JOBS_RESULTS,
        3,
        i32::try_from(expected.len()).expect("the fixture fits in an i32"),
        state::MAX_LIST_SERVICE_JOBS_RESULTS,
    ] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "jobQueue": "q-listing",
                "jobStatus": state::SERVICE_JOB_SUBMITTED,
                "maxResults": page_size,
            });
            if let Some(resume) = token.as_deref() {
                request["nextToken"] = json!(resume);
            }
            let body = listed(&batch, &case, request).await;
            let page = ids(&body);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case {case}: a page must hold at most maxResults jobs, got {body}"
            );
            seen.extend(page);
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: paging did not terminate after {pages} pages, last token {token:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: paging must see every job exactly once in the unpaged order, got {seen:?}"
        );
    }
}

/// RULE: a terminate is what puts a service job in a terminal status, and the listing then
/// reports that status, the reason it carries, and `isTerminated` — "only present when the
/// value is true".
#[tokio::test]
async fn a_terminated_service_job_lists_as_terminated() {
    let batch = Batch::new();
    fixture(&batch).await;

    let before = listed(
        &batch,
        "before the terminate",
        json!({"jobQueue": "q-listing", "jobStatus": state::SERVICE_JOB_SUBMITTED}),
    )
    .await;
    let targets = ids(&before);
    let target = targets.first().cloned().expect("the fixture submitted jobs");
    let reason = "ended by the test";
    terminate(&batch, "terminated", &target, reason).await;

    let failed = listed(
        &batch,
        "after the terminate",
        json!({"jobQueue": "q-listing", "jobStatus": state::SERVICE_JOB_FAILED}),
    )
    .await;
    assert_eq!(
        ids(&failed),
        vec![target.clone()],
        "case after the terminate: expected only the terminated job in FAILED, got {failed}"
    );
    let summary = failed["jobSummaryList"][0].clone();
    assert_eq!(
        summary["statusReason"].as_str(),
        Some(reason),
        "case after the terminate: expected the terminate's reason, got {summary}"
    );
    assert_eq!(
        summary["isTerminated"].as_bool(),
        Some(true),
        "case after the terminate: isTerminated is present and true, got {summary}"
    );
    assert!(
        summary["stoppedAt"].as_i64().is_some(),
        "case after the terminate: a job the call ended carries stoppedAt, got {summary}"
    );

    let still_submitted = listed(
        &batch,
        "the rest",
        json!({"jobQueue": "q-listing", "jobStatus": state::SERVICE_JOB_SUBMITTED}),
    )
    .await;
    assert_eq!(
        ids(&still_submitted),
        targets[1..].to_vec(),
        "case the rest: a terminate moves only the job it named, got {still_submitted}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDICOMImportJobs`.
//!
//! RULE: a paginated listing reports exactly the records the emulator PERSISTED for the
//! collection the request names — here the import jobs of one data store — in a stable
//! total order, narrowed to the status the request filters on when it names one, one
//! `maxResults`-sized page at a time, with an opaque token that resumes the walk exactly
//! where the previous page stopped; and it reports each record's CURRENT state, the one
//! the clock has made true, the same state the record's own reader reports. A request
//! naming a collection the emulator does not hold is refused with the modelled
//! not-found error rather than answered with an empty page, and a token this service did
//! not issue for that collection is refused rather than honoured.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: page
//! size (both published `maxResults` bounds and sizes either side of the collection),
//! how many jobs the collection holds, whether a job carries a name, which status the
//! filter names (every value the model declares), which data store the listing names,
//! and what the supplied token is. Every case drives real HTTP through the production
//! front door (`crate::datastore_lifecycle`'s harness), so what is asserted is the wire
//! behaviour a `medical-imaging` SDK client sees; nothing reaches into the state struct.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde_json::{json, Value};

use service_com_amazonaws_medicalimaging_ahigatewayservice::ops::types::JOB_STATUS_VALUES;
use service_com_amazonaws_medicalimaging_ahigatewayservice::state::IMPORT_JOB_COMPLETION_DELAY;

use crate::datastore_lifecycle::{Answer, Emulator, SETTLE_MULTIPLE};

/// A `RoleArn` every submit below uses (len 20..2048,
/// `^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ImportRole";

/// The service's constant text for a data store it does not hold.
const DATASTORE_NOT_FOUND: &str = "Datastore resource is not found.";

/// The service's fixed sentence for a `nextToken` it did not issue.
const INVALID_NEXT_TOKEN: &str =
    "Failed to perform requested operation due to invalid NextToken. Please try again.";

/// A well-formed `DatastoreId` (`^[0-9a-z]{32}$`) no create in this suite mints.
fn unheld_datastore_id() -> String {
    "0".repeat(32)
}

/// `GET /listDICOMImportJobs/datastore/{datastoreId}` with the query string `query`
/// (`smithy_query(query="operation",operation="ListDICOMImportJobs")`).
async fn list_jobs(emulator: &Emulator, datastore_id: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/listDICOMImportJobs/datastore/{datastore_id}")
    } else {
        format!("/listDICOMImportJobs/datastore/{datastore_id}?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `jobSummaries` of one answer, as JSON objects.
fn summaries(answer: &Answer) -> Vec<Value> {
    answer
        .field(&["jobSummaries"])
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `jobId` of every summary in one answer, in the order served.
fn listed_ids(answer: &Answer) -> Vec<String> {
    summaries(answer)
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Submit one import job and return the `jobId` it minted.
async fn submit_job(
    emulator: &Emulator,
    datastore_id: &str,
    client_token: &str,
    job_name: Option<&str>,
) -> String {
    let mut request = json!({
        "clientToken": client_token,
        "dataAccessRoleArn": ROLE_ARN,
        "inputS3Uri": "s3://imaging-in/",
        "outputS3Uri": "s3://imaging-out/",
    });
    if let Some(name) = job_name {
        request["jobName"] = json!(name);
    }
    let started = emulator.start_import_job(datastore_id, request).await;
    assert_eq!(
        started.status, 200,
        "case {client_token:?}: expected HTTP 200 from StartDICOMImportJob, got {started:?}"
    );
    started.string(&["jobId"])
}

/// Walk the whole listing with `page_size`, returning every `jobId` in the order served.
///
/// `extra` is appended to each request's query string, so the same walk serves the
/// filtered cases too.
async fn walk(emulator: &Emulator, datastore_id: &str, page_size: usize, extra: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        pages += 1;
        assert!(
            pages <= 64,
            "case maxResults={page_size}{extra}: paging did not terminate after {pages} pages"
        );
        let mut query = format!("maxResults={page_size}{extra}");
        if let Some(value) = &token {
            query.push_str(&format!("&nextToken={value}"));
        }
        let page = list_jobs(emulator, datastore_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case maxResults={page_size}{extra}: expected HTTP 200, got {page:?}"
        );
        let listed = summaries(&page);
        assert!(
            listed.len() <= page_size,
            "case maxResults={page_size}{extra}: expected at most {page_size} summaries, got {}",
            listed.len()
        );
        seen.extend(listed_ids(&page));
        match page.field(&["nextToken"]).as_str() {
            Some(next) => {
                assert_eq!(
                    listed.len(),
                    page_size,
                    "case maxResults={page_size}{extra}: a non-final page must be full, got {}",
                    listed.len()
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    seen
}

/// Wait out the completion the clock owes every job of `datastore_id`, polling the
/// listing until each one reports `COMPLETED` so the suites that need a TERMINAL status
/// do not race the clock.
async fn settle_every_job(emulator: &Emulator, datastore_id: &str) {
    let deadline = Instant::now() + IMPORT_JOB_COMPLETION_DELAY * 40;
    loop {
        let page = list_jobs(emulator, datastore_id, "maxResults=50").await;
        assert_eq!(
            page.status, 200,
            "settling {datastore_id}: expected HTTP 200, got {page:?}"
        );
        let pending: Vec<Value> = summaries(&page)
            .into_iter()
            .filter(|summary| summary["jobStatus"].as_str() != Some("COMPLETED"))
            .collect();
        if pending.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "settling {datastore_id}: these jobs never reached COMPLETED: {pending:?}"
        );
        tokio::time::sleep(IMPORT_JOB_COMPLETION_DELAY / 4).await;
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// RULE: paging walks the whole collection exactly once, in submission order — each page
/// holds at most `maxResults` jobs, a non-final page is full, the token continues where
/// the previous page stopped, and the last page carries no token.
///
/// Rows span page sizes from the published lower bound to the published upper bound of
/// `maxResults` (range 1..50, from
/// `smithy_query(query="operation",operation="ListDICOMImportJobs")`), plus sizes either
/// side of the collection size, and jobs with and without a `jobName`.
#[tokio::test]
async fn paging_the_import_jobs_of_a_datastore_walks_every_job_exactly_once() {
    const JOBS: usize = 7;
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("page.jobs", "page-jobs").await;

    let mut submitted: Vec<String> = Vec::new();
    for index in 0..JOBS {
        // Every other job is submitted without a `jobName`, which the summary's
        // @required `jobName` must still answer (with the job's own id).
        let name = format!("page.import-{index}");
        let job_name = if index % 2 == 0 { Some(name.as_str()) } else { None };
        submitted.push(
            submit_job(
                &emulator,
                &datastore_id,
                &format!("page-job-{index}"),
                job_name,
            )
            .await,
        );
    }

    // 1 and 50 are `maxResults`' published bounds; 6, 7 and 8 straddle the collection.
    for page_size in [1_usize, 2, 3, 6, 7, 8, 50] {
        let seen = walk(&emulator, &datastore_id, page_size, "").await;
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a job was listed twice: {seen:?}"
        );
        assert_eq!(
            seen, submitted,
            "case maxResults={page_size}: expected every job once in submission order, got {seen:?}"
        );
    }

    // A listing with no `maxResults` at all serves the whole collection in the same
    // order, with no token.
    let whole = list_jobs(&emulator, &datastore_id, "").await;
    assert_eq!(
        whole.status, 200,
        "case \"no maxResults\": expected HTTP 200, got {whole:?}"
    );
    assert_eq!(
        listed_ids(&whole),
        submitted,
        "case \"no maxResults\": expected the whole collection in order, got {whole:?}"
    );
    assert!(
        whole.field(&["nextToken"]).is_null(),
        "case \"no maxResults\": expected no nextToken on a complete page, got {whole:?}"
    );

    // Every summary carries the members the model declares for it, computed from what
    // the submit persisted: a named job reports its name, a nameless one its own id.
    for (index, summary) in summaries(&whole).iter().enumerate() {
        let job_id = summary["jobId"].as_str().unwrap_or_default().to_string();
        let expected_name = if index % 2 == 0 {
            format!("page.import-{index}")
        } else {
            job_id.clone()
        };
        assert_eq!(
            summary["jobName"].as_str(),
            Some(expected_name.as_str()),
            "case summary {index}: expected jobName {expected_name:?}, got {summary:?}"
        );
        assert_eq!(
            summary["datastoreId"].as_str(),
            Some(datastore_id.as_str()),
            "case summary {index}: expected the data store id on every summary, got {summary:?}"
        );
        assert_eq!(
            summary["dataAccessRoleArn"].as_str(),
            Some(ROLE_ARN),
            "case summary {index}: expected the submitted role ARN, got {summary:?}"
        );
        assert!(
            summary["submittedAt"].is_number(),
            "case summary {index}: expected a submittedAt timestamp, got {summary:?}"
        );
    }
}

/// RULE: a continuation token names a POSITION in the collection's order, so a walk
/// resumed after the collection grew serves every job not already served exactly once
/// and never repeats one it served.
///
/// Rows span how many jobs are submitted mid-walk (one, and more than a page).
#[tokio::test]
async fn a_token_resumes_the_walk_when_jobs_were_submitted_between_pages() {
    for added in [1_usize, 3] {
        let emulator = Emulator::new();
        let datastore_id = emulator
            .active_datastore("resume.jobs", "resume-jobs")
            .await;
        let mut submitted: Vec<String> = Vec::new();
        for index in 0..5 {
            submitted.push(
                submit_job(
                    &emulator,
                    &datastore_id,
                    &format!("resume-job-{index}"),
                    None,
                )
                .await,
            );
        }

        let first_page = list_jobs(&emulator, &datastore_id, "maxResults=2").await;
        let served = listed_ids(&first_page);
        assert_eq!(
            served,
            submitted[..2].to_vec(),
            "case added={added}: expected the first page to be the first two in order, got {served:?}"
        );
        let token = first_page
            .field(&["nextToken"])
            .as_str()
            .unwrap_or_else(|| panic!("case added={added}: expected a second page, got {first_page:?}"))
            .to_string();

        for index in 0..added {
            submitted.push(
                submit_job(
                    &emulator,
                    &datastore_id,
                    &format!("resume-late-{added}-{index}"),
                    None,
                )
                .await,
            );
        }

        let mut resumed: Vec<String> = Vec::new();
        let mut cursor = Some(token);
        while let Some(next_token) = cursor {
            let page = list_jobs(
                &emulator,
                &datastore_id,
                &format!("maxResults=2&nextToken={next_token}"),
            )
            .await;
            assert_eq!(
                page.status, 200,
                "case added={added}: expected HTTP 200 on resume, got {page:?}"
            );
            resumed.extend(listed_ids(&page));
            cursor = page.field(&["nextToken"]).as_str().map(ToString::to_string);
        }
        assert_eq!(
            resumed,
            submitted[2..].to_vec(),
            "case added={added}: expected the resume to serve {:?} in order, got {resumed:?}",
            &submitted[2..]
        );
        for already_served in &served {
            assert!(
                !resumed.contains(already_served),
                "case added={added}: expected the resume not to repeat {already_served:?}, got {resumed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The status filter
// ---------------------------------------------------------------------------

/// RULE: `jobStatus` narrows the listing to the jobs in exactly that status, and every
/// status the enum declares is accepted.
///
/// The catalog is every value of `JobStatus`, taken from the model itself
/// (`JOB_STATUS_VALUES`, from `smithy_query(query="shape",shape="JobStatus")`):
/// COMPLETED, FAILED, IN_PROGRESS, SUBMITTED. The issue cited none of them. Each row
/// asserts the filtered set against the status the UNFILTERED listing reports for each
/// job, so the filter is checked against this service's own answer rather than against a
/// status the test assumed — and it is asserted once every job has reached a terminal
/// status, so no row races the clock.
#[tokio::test]
async fn listing_filters_by_every_modelled_job_status() {
    let emulator = Emulator::new();
    let datastore_id = emulator
        .active_datastore("filter.jobs", "filter-jobs")
        .await;
    for index in 0..4 {
        submit_job(
            &emulator,
            &datastore_id,
            &format!("filter-job-{index}"),
            Some(&format!("filter.import-{index}")),
        )
        .await;
    }
    settle_every_job(&emulator, &datastore_id).await;

    // What the service itself reports, grouped by status: the expectation every filter
    // row below is checked against.
    let unfiltered = list_jobs(&emulator, &datastore_id, "maxResults=50").await;
    let mut grouped: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for status in JOB_STATUS_VALUES {
        grouped.insert((*status).to_string(), BTreeSet::new());
    }
    for summary in summaries(&unfiltered) {
        let status = summary["jobStatus"].as_str().unwrap_or_default().to_string();
        let job_id = summary["jobId"].as_str().unwrap_or_default().to_string();
        let group = grouped.get_mut(&status).unwrap_or_else(|| {
            panic!("case \"grouping\": expected a modelled jobStatus, got {status:?}")
        });
        group.insert(job_id);
    }
    assert_eq!(
        grouped.get("COMPLETED").map(BTreeSet::len),
        Some(4),
        "case \"grouping\": expected all four jobs COMPLETED once the clock owed it, got {grouped:?}"
    );

    for status in JOB_STATUS_VALUES {
        let page = list_jobs(
            &emulator,
            &datastore_id,
            &format!("jobStatus={status}&maxResults=50"),
        )
        .await;
        assert_eq!(
            page.status, 200,
            "case {status:?}: expected HTTP 200 for a modelled status, got {page:?}"
        );
        let got: BTreeSet<String> = listed_ids(&page).into_iter().collect();
        let expected = grouped.get(*status).cloned().unwrap_or_default();
        assert_eq!(
            got, expected,
            "case {status:?}: expected {expected:?}, got {got:?}"
        );
        // The filter is honoured on every page, not only on a page big enough to hold
        // the whole answer.
        let walked: BTreeSet<String> = walk(&emulator, &datastore_id, 1, &format!("&jobStatus={status}"))
            .await
            .into_iter()
            .collect();
        assert_eq!(
            walked, expected,
            "case {status:?} paged one per page: expected {expected:?}, got {walked:?}"
        );
    }

    // A value no modelled `JobStatus` names is a client error, not an empty page.
    let refused = list_jobs(&emulator, &datastore_id, "jobStatus=NOT_A_STATUS").await;
    assert_eq!(
        refused.status, 400,
        "case \"unmodelled jobStatus\": expected HTTP 400, got {refused:?}"
    );
    assert_eq!(
        refused.error_code, "ValidationException",
        "case \"unmodelled jobStatus\": expected ValidationException, got {refused:?}"
    );
}

/// RULE: the listing reports each job's CURRENT state — the one the clock has made true,
/// the same one the job's own reader reports — rather than the state it was submitted in.
///
/// Rows span the two sides of the transition the clock owes: straight after the submit,
/// and once the completion span has elapsed.
#[tokio::test]
async fn the_listing_reports_the_state_the_clock_has_made_true() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("clock.jobs", "clock-jobs").await;
    let job_id = submit_job(&emulator, &datastore_id, "clock-job", Some("clock.import")).await;

    let fresh = list_jobs(&emulator, &datastore_id, "maxResults=50").await;
    let fresh_summary = summaries(&fresh)
        .into_iter()
        .find(|summary| summary["jobId"].as_str() == Some(job_id.as_str()))
        .unwrap_or_else(|| panic!("case \"fresh\": expected the submitted job listed, got {fresh:?}"));
    let fresh_status = fresh_summary["jobStatus"].as_str().unwrap_or_default();
    assert!(
        ["SUBMITTED", "IN_PROGRESS"].contains(&fresh_status),
        "case \"fresh\": expected a non-terminal status straight after the submit, got {fresh_summary:?}"
    );
    assert!(
        fresh_summary["endedAt"].is_null(),
        "case \"fresh\": expected no endedAt before the job ended, got {fresh_summary:?}"
    );

    tokio::time::sleep(SETTLE_MULTIPLE * IMPORT_JOB_COMPLETION_DELAY).await;
    let settled = list_jobs(&emulator, &datastore_id, "maxResults=50").await;
    let settled_summary = summaries(&settled)
        .into_iter()
        .find(|summary| summary["jobId"].as_str() == Some(job_id.as_str()))
        .unwrap_or_else(|| panic!("case \"settled\": expected the job listed, got {settled:?}"));
    assert_eq!(
        settled_summary["jobStatus"].as_str(),
        Some("COMPLETED"),
        "case \"settled\": expected COMPLETED once the clock owed it, got {settled_summary:?}"
    );
    let ended_at = settled_summary["endedAt"].as_f64();
    let submitted_at = settled_summary["submittedAt"].as_f64();
    assert!(
        ended_at.is_some() && ended_at >= submitted_at,
        "case \"settled\": expected endedAt at or after submittedAt, got {settled_summary:?}"
    );

    // The listing and the job's own reader agree, member for member, on a terminal job.
    let read = emulator.get_import_job(&datastore_id, &job_id).await;
    assert_eq!(
        read.status, 200,
        "case \"agreement\": expected HTTP 200 from GetDICOMImportJob, got {read:?}"
    );
    for member in [
        "jobId",
        "jobName",
        "jobStatus",
        "datastoreId",
        "dataAccessRoleArn",
    ] {
        assert_eq!(
            settled_summary[member].as_str(),
            read.field(&["jobProperties", member]).as_str(),
            "case \"agreement\": expected the listing and GetDICOMImportJob to agree on {member}, got {settled_summary:?} and {read:?}"
        );
    }
    for member in ["submittedAt", "endedAt"] {
        assert_eq!(
            settled_summary[member].as_f64(),
            read.field(&["jobProperties", member]).as_f64(),
            "case \"agreement\": expected the listing and GetDICOMImportJob to agree on {member}, got {settled_summary:?} and {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Which collection a listing reports
// ---------------------------------------------------------------------------

/// RULE: the listing reports the jobs of the data store the request names and of no
/// other, and a data store holding none answers an empty page rather than another
/// store's jobs.
///
/// Rows span how many jobs each data store holds, including none.
#[tokio::test]
async fn the_listing_holds_only_the_jobs_of_the_datastore_it_names() {
    let emulator = Emulator::new();
    let counts = [0_usize, 1, 3];
    let mut expected: Vec<(String, Vec<String>)> = Vec::new();
    for (store_index, count) in counts.iter().enumerate() {
        let datastore_id = emulator
            .active_datastore(
                &format!("isolated.store-{store_index}"),
                &format!("isolated-store-{store_index}"),
            )
            .await;
        let mut jobs = Vec::new();
        for job_index in 0..*count {
            jobs.push(
                submit_job(
                    &emulator,
                    &datastore_id,
                    &format!("isolated-{store_index}-{job_index}"),
                    None,
                )
                .await,
            );
        }
        expected.push((datastore_id, jobs));
    }

    for (datastore_id, jobs) in &expected {
        let page = list_jobs(&emulator, datastore_id, "maxResults=50").await;
        assert_eq!(
            page.status, 200,
            "case store {datastore_id}: expected HTTP 200, got {page:?}"
        );
        assert_eq!(
            &listed_ids(&page),
            jobs,
            "case store {datastore_id}: expected exactly its own {} job(s), got {page:?}",
            jobs.len()
        );
        assert!(
            page.field(&["nextToken"]).is_null(),
            "case store {datastore_id}: expected no nextToken on a complete page, got {page:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    query: String,
    datastore_id: String,
    status: u16,
    error_code: &'static str,
    message: Option<&'static str>,
}

/// RULE: a listing whose data store the emulator does not hold is refused with
/// `ResourceNotFoundException` (404) stated by the resource KIND; one whose identifier
/// cannot name a data store at all, whose `maxResults` falls outside the published range,
/// or whose `nextToken` this service did not issue for that collection, is refused with
/// `ValidationException` (400) — never an empty success.
///
/// Rows span the identifier axis (too short, too long, wrong character class), the
/// `maxResults` axis (each published bound and one past it), and the token axis (not hex,
/// hex without this listing's marker, and a token this service issued for ANOTHER data
/// store).
#[tokio::test]
async fn unknown_datastores_and_malformed_listing_inputs_are_refused() {
    let emulator = Emulator::new();
    let known = emulator
        .active_datastore("refusal.jobs", "refusal-jobs")
        .await;
    for index in 0..3 {
        submit_job(
            &emulator,
            &known,
            &format!("refusal-job-{index}"),
            None,
        )
        .await;
    }
    // A token this service issued, but for a DIFFERENT data store's jobs: a position in
    // another collection, so not one in this one.
    let other = emulator
        .active_datastore("refusal.other", "refusal-other")
        .await;
    for index in 0..3 {
        submit_job(&emulator, &other, &format!("refusal-other-{index}"), None).await;
    }
    let other_token = list_jobs(&emulator, &other, "maxResults=1")
        .await
        .field(&["nextToken"])
        .as_str()
        .expect("a second page of the other data store's jobs")
        .to_string();

    let rows: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "an unknown but well-formed data store id",
            query: String::new(),
            datastore_id: unheld_datastore_id(),
            status: 404,
            error_code: "ResourceNotFoundException",
            message: Some(DATASTORE_NOT_FOUND),
        },
        RefusalCase {
            case: "a 31-character data store id",
            query: String::new(),
            datastore_id: "0123456789abcdef0123456789abcde".to_string(),
            status: 400,
            error_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a 33-character data store id",
            query: String::new(),
            datastore_id: "0123456789abcdef0123456789abcdef0".to_string(),
            status: 400,
            error_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "an uppercase data store id",
            query: String::new(),
            datastore_id: "0123456789ABCDEF0123456789abcdef".to_string(),
            status: 400,
            error_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "maxResults below the published minimum",
            query: "maxResults=0".to_string(),
            datastore_id: known.clone(),
            status: 400,
            error_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "maxResults above the published maximum",
            query: "maxResults=51".to_string(),
            datastore_id: known.clone(),
            status: 400,
            error_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a nextToken this service never issued",
            query: "nextToken=not-a-token".to_string(),
            datastore_id: known.clone(),
            status: 400,
            error_code: "ValidationException",
            message: Some(INVALID_NEXT_TOKEN),
        },
        RefusalCase {
            // Well-formed hex that decodes to UTF-8 but carries no marker of this
            // listing: still not a token it issued.
            case: "a forged but hex-decodable nextToken",
            query: "nextToken=6161".to_string(),
            datastore_id: known.clone(),
            status: 400,
            error_code: "ValidationException",
            message: Some(INVALID_NEXT_TOKEN),
        },
        RefusalCase {
            case: "a nextToken issued for another data store's jobs",
            query: format!("nextToken={other_token}"),
            datastore_id: known.clone(),
            status: 400,
            error_code: "ValidationException",
            message: Some(INVALID_NEXT_TOKEN),
        },
    ];

    for row in &rows {
        let answer = list_jobs(&emulator, &row.datastore_id, &row.query).await;
        assert_eq!(
            answer.status, row.status,
            "case {:?}: expected HTTP {}, got {answer:?}",
            row.case, row.status
        );
        assert_eq!(
            answer.error_code, row.error_code,
            "case {:?}: expected error code {:?}, got {answer:?}",
            row.case, row.error_code
        );
        if let Some(expected) = row.message {
            assert_eq!(
                answer.string(&["message"]),
                expected,
                "case {:?}: expected message {expected:?}, got {answer:?}",
                row.case
            );
        }
    }

    // The published `maxResults` bounds themselves are accepted, and the token this
    // listing issued for THIS data store is honoured.
    for accepted in [1_usize, 50] {
        let page = list_jobs(&emulator, &known, &format!("maxResults={accepted}")).await;
        assert_eq!(
            page.status, 200,
            "case maxResults={accepted}: expected HTTP 200 at a published bound, got {page:?}"
        );
    }
    let own_token = list_jobs(&emulator, &known, "maxResults=1")
        .await
        .field(&["nextToken"])
        .as_str()
        .expect("a second page of this data store's jobs")
        .to_string();
    let resumed = list_jobs(&emulator, &known, &format!("maxResults=1&nextToken={own_token}")).await;
    assert_eq!(
        resumed.status, 200,
        "case \"own token\": expected HTTP 200 for a token this listing issued, got {resumed:?}"
    );
}

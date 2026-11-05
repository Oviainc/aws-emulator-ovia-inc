// SPDX-License-Identifier: CC0-1.0

//! `ListJobs` over the stored S3 Batch Operations jobs, driven end to end over
//! HTTP.
//!
//! The rule under test: a job listing reports, from this service's state, the
//! jobs of the account the request names — every job that has not ended, plus
//! every job that ended inside the listing window — each as the summary of the
//! record `CreateJob` stored (its id, the one action it runs, its priority, its
//! description, its status and its creation instant, and nothing about tasks
//! nobody ran), narrowed to the statuses the request lists when it lists any, in
//! a stable order (newest first), one page of at most `MaxResults` entries with
//! a continuation token exactly while jobs remain. A listing never reports
//! another account's jobs, and a request that asks for a page size outside the
//! modelled range, a status that is not a `JobStatus` or a continuation token
//! this service never issued is refused rather than answered.
//!
//! Every row below differs along something that rule generalises over: which of
//! the model's actions the job runs, whether its object list comes from a
//! manifest (in either modelled format) or a generator, its priority (including
//! each published bound of `JobPriority`), whether it carries a description,
//! which status it is in, how many jobs the account holds, which account holds
//! them, the page size asked for and the statuses the listing is narrowed to.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model (`MaxResults`, `NextToken` and the repeated
//! `jobStatuses` parameter all ride in the query string), runs the handler and
//! serializes the restXml response. The router, the response reader, the create
//! and describe helpers and the job request shapes are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_s3control_awss3controlservicev20180820::ops::types::{
    JOB_STATUS_VALUES, OPERATION_NAME_VALUES,
};
use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    JOB_PREPARATION, JOB_STATUS_READY, JOB_STATUS_SUSPENDED,
};

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT,
};
use crate::batch_job_lifecycle::{
    create_job, describe_job, JobCase, ACTION_COMPUTE_CHECKSUM, ACTION_INITIATE_RESTORE,
    ACTION_LAMBDA_INVOKE, ACTION_PUT_OBJECT_COPY, ACTION_PUT_OBJECT_TAGGING, GENERATED_MANIFEST,
    MANIFEST_CSV, MANIFEST_INVENTORY,
};

/// `MaxResults`'s published maximum, which is also the page size a request
/// naming none gets (`smithy_query(query="operation",operation="ListJobs")`:
/// `MaxResults` range 0..1000).
const MAX_PAGE_SIZE: usize = 1000;

/// Ask for one page of jobs, exactly as the model binds the request: a GET of
/// the jobs collection with the paging and filtering members in the query
/// string and the account in the `x-amz-account-id` header.
///
/// `query` is rendered verbatim, so a value outside a modelled range can be a
/// row of its own.
///
/// `pub(crate)` because the sibling suites read a stored job back through the
/// listing as well rather than standing up their own request.
pub(crate) async fn list_jobs(emulator: &Emulator, account_id: &str, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/v20180820/jobs".to_string()
    } else {
        format!("/v20180820/jobs?{query}")
    };
    emulator.call("GET", &uri, account_id, "").await
}

/// The `<member>` documents of the answer's `Jobs` list, in the order the
/// listing returned them.
///
/// `JobListDescriptor` carries no list member, so a summary holds no nested
/// `<member>`: the list's entries are exactly the spans between its member
/// tags.
fn job_summaries(reply: &Reply) -> Vec<String> {
    reply
        .text("Jobs")
        .map(|jobs| {
            jobs.split("<member>")
                .skip(1)
                .map(|entry| entry.trim_end_matches("</member>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The text of `element` inside one summary, read with the same reader the
/// whole-body assertions use.
fn summary_text(summary: &str, element: &str) -> Option<String> {
    Reply {
        status: 200,
        headers: Vec::new(),
        body: summary.to_string(),
    }
    .text(element)
}

/// The job ids a listing reported, in the order it reported them.
fn listed_job_ids(reply: &Reply) -> Vec<String> {
    job_summaries(reply)
        .iter()
        .map(|summary| {
            summary_text(summary, "JobId")
                .unwrap_or_else(|| panic!("every listed job carries a JobId: {summary}"))
        })
        .collect()
}

/// One row: a job the account holds, and what the listing must report for it.
struct ListRow {
    case: &'static str,
    /// The `<Operation>` body: exactly one action.
    action: &'static str,
    /// The whole `<Manifest>` or `<ManifestGenerator>` element.
    source: &'static str,
    /// The `OperationName` the summary must report for `action`: the member
    /// name `JobOperation` declares for it, which is a value of the model's
    /// `OperationName` enum (asserted against `OPERATION_NAME_VALUES` below).
    operation_name: &'static str,
    /// Rendered into the create verbatim; inside the modelled `JobPriority`
    /// range (0..2147483647).
    priority: &'static str,
    description: Option<&'static str>,
    /// A job created requiring confirmation awaits it in `Suspended`; any other
    /// created job is `New` and the clock moves it on to `Ready`.
    confirmation_required: bool,
}

impl ListRow {
    fn job_case(&self) -> JobCase {
        let mut case =
            JobCase::new(self.case, self.action, self.source).with_priority(self.priority);
        if let Some(description) = self.description {
            case = case.with_description(description);
        }
        if self.confirmation_required {
            case = case.awaiting_confirmation();
        }
        case
    }

    /// The status this job is in once the clock has had the chance to move a
    /// created job out of `New`.
    fn expected_status(&self) -> &'static str {
        if self.confirmation_required {
            JOB_STATUS_SUSPENDED
        } else {
            JOB_STATUS_READY
        }
    }
}

/// The jobs every assertion below is run over. No row is merely a rename of
/// another: each differs in something the listing generalises over.
fn list_rows() -> Vec<ListRow> {
    vec![
        ListRow {
            case: "copy from a CSV manifest",
            action: ACTION_PUT_OBJECT_COPY,
            source: MANIFEST_CSV,
            operation_name: "S3PutObjectCopy",
            priority: "10",
            description: None,
            confirmation_required: false,
        },
        ListRow {
            case: "tagging from an inventory manifest, lowest priority",
            action: ACTION_PUT_OBJECT_TAGGING,
            source: MANIFEST_INVENTORY,
            operation_name: "S3PutObjectTagging",
            // The minimum the modelled `JobPriority` range admits.
            priority: "0",
            description: Some("tag the archived objects"),
            confirmation_required: false,
        },
        ListRow {
            case: "lambda invoke from a generated manifest, highest priority",
            action: ACTION_LAMBDA_INVOKE,
            source: GENERATED_MANIFEST,
            operation_name: "LambdaInvoke",
            // The maximum the modelled `JobPriority` range admits.
            priority: "2147483647",
            description: Some("invoke the transform function"),
            confirmation_required: false,
        },
        ListRow {
            case: "restore from an inventory manifest, awaiting confirmation",
            action: ACTION_INITIATE_RESTORE,
            source: MANIFEST_INVENTORY,
            operation_name: "S3InitiateRestoreObject",
            priority: "5",
            description: None,
            confirmation_required: true,
        },
        ListRow {
            case: "checksum from a CSV manifest, awaiting confirmation",
            action: ACTION_COMPUTE_CHECKSUM,
            source: MANIFEST_CSV,
            operation_name: "S3ComputeObjectChecksum",
            priority: "1",
            description: Some("checksum every object"),
            confirmation_required: true,
        },
        ListRow {
            case: "copy from a generated manifest",
            action: ACTION_PUT_OBJECT_COPY,
            source: GENERATED_MANIFEST,
            operation_name: "S3PutObjectCopy",
            priority: "2147483646",
            description: None,
            confirmation_required: false,
        },
    ]
}

/// Create every row under `account_id` and answer the job id each was given, in
/// row order, with the clock's own `New` -> `Ready` transition already made due
/// so a status read afterwards is not one the clock was about to change under
/// the assertion.
async fn create_rows(emulator: &Emulator, account_id: &str, rows: &[ListRow]) -> Vec<String> {
    let mut job_ids = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let token = format!("{account_id}-{index}");
        let created = create_job(emulator, account_id, &row.job_case(), &token).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateJob to answer 200, got {} with body {}",
            row.case, created.status, created.body
        );
        job_ids.push(
            created
                .text("JobId")
                .unwrap_or_else(|| panic!("case {:?}: CreateJob answers a JobId", row.case)),
        );
        // Two jobs created in the same instant would leave the newest-first
        // order to the job id tie-break; a row apart in time pins the order the
        // creation instants give.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    tokio::time::sleep(JOB_PREPARATION + Duration::from_millis(50)).await;
    job_ids
}

#[tokio::test]
async fn a_listing_reports_each_of_the_account_s_jobs_as_the_summary_of_its_record() {
    let emulator = Emulator::start();
    let rows = list_rows();
    let job_ids = create_rows(&emulator, ACCOUNT, &rows).await;

    let listed = list_jobs(&emulator, ACCOUNT, "").await;
    assert_eq!(
        listed.status, 200,
        "expected the listing to answer 200, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed_job_ids(&listed).into_iter().collect::<BTreeSet<_>>(),
        job_ids.iter().cloned().collect::<BTreeSet<_>>(),
        "expected the listing to report every job the account holds, got body {}",
        listed.body
    );
    assert!(
        listed.text("NextToken").is_none(),
        "expected no continuation token while the whole set fits one page, got body {}",
        listed.body
    );

    let summaries = job_summaries(&listed);
    for (row, job_id) in rows.iter().zip(&job_ids) {
        let summary = summaries
            .iter()
            .find(|summary| summary_text(summary, "JobId").as_deref() == Some(job_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a summary for job {job_id:?}, got body {}",
                    row.case, listed.body
                )
            });

        assert_eq!(
            summary_text(summary, "Status").as_deref(),
            Some(row.expected_status()),
            "case {:?}: expected the stored status, got summary {summary}",
            row.case
        );
        assert_eq!(
            summary_text(summary, "Priority").as_deref(),
            Some(row.priority),
            "case {:?}: expected the submitted priority, got summary {summary}",
            row.case
        );
        assert!(
            OPERATION_NAME_VALUES.contains(&row.operation_name),
            "case {:?}: expected {:?} to be a modelled OperationName, which the model lists \
             as {OPERATION_NAME_VALUES:?}",
            row.case,
            row.operation_name
        );
        assert_eq!(
            summary_text(summary, "Operation").as_deref(),
            Some(row.operation_name),
            "case {:?}: expected the one action the job runs, got summary {summary}",
            row.case
        );
        assert_eq!(
            summary_text(summary, "Description").as_deref(),
            row.description,
            "case {:?}: expected the submitted description, got summary {summary}",
            row.case
        );
        assert!(
            summary_text(summary, "CreationTime").is_some_and(|time| !time.is_empty()),
            "case {:?}: expected the job's creation instant, got summary {summary}",
            row.case
        );
        // Nothing ran this job's tasks and nothing ended it, so the summary
        // claims neither.
        for absent in ["<TerminationDate>", "<ProgressSummary>", "<TotalNumberOfTasks>"] {
            assert!(
                !summary.contains(absent),
                "case {:?}: expected no {absent} on a job whose tasks nothing ran, got \
                 summary {summary}",
                row.case
            );
        }

        // One record, two readers: every member the summary and the full
        // descriptor share reports the same value.
        let described = describe_job(&emulator, ACCOUNT, job_id).await;
        for element in ["JobId", "Status", "Priority", "Description", "CreationTime"] {
            assert_eq!(
                summary_text(summary, element),
                described.text(element),
                "case {:?}: expected the listing and DescribeJob to agree about {element}, \
                 got summary {summary} and body {}",
                row.case,
                described.body
            );
        }
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_jobs_of_the_account_the_request_names() {
    let emulator = Emulator::start();
    let rows = list_rows();
    let mine = create_rows(&emulator, ACCOUNT, &rows).await;

    let listed = list_jobs(&emulator, ACCOUNT, "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed_job_ids(&listed).into_iter().collect::<BTreeSet<_>>(),
        mine.iter().cloned().collect::<BTreeSet<_>>(),
        "expected only the caller's own jobs, got body {}",
        listed.body
    );

    // A listing for another account is refused rather than answered — with
    // neither that account's jobs nor this one's — so no listing enumerates
    // across the account boundary.
    for (case, account) in [
        ("another account", OTHER_ACCOUNT),
        ("a third account", "210987654322"),
    ] {
        let refused = list_jobs(&emulator, account, "").await;
        assert_foreign_account_refused(case, account, &refused);
        assert!(
            job_summaries(&refused).is_empty() && refused.text("NextToken").is_none(),
            "case {case:?}: expected the refusal to carry no listing, got body {}",
            refused.body
        );
    }
}

#[tokio::test]
async fn a_listing_is_narrowed_to_the_statuses_the_request_lists() {
    let emulator = Emulator::start();
    let rows = list_rows();
    let job_ids = create_rows(&emulator, ACCOUNT, &rows).await;

    let of_status = |wanted: &str| -> BTreeSet<String> {
        rows.iter()
            .zip(&job_ids)
            .filter(|(row, _)| row.expected_status() == wanted)
            .map(|(_, job_id)| job_id.clone())
            .collect()
    };
    let ready = of_status(JOB_STATUS_READY);
    let suspended = of_status(JOB_STATUS_SUSPENDED);
    let all: BTreeSet<String> = job_ids.iter().cloned().collect();
    assert!(
        !ready.is_empty() && !suspended.is_empty(),
        "expected the rows to hold jobs in both statuses, got {ready:?} and {suspended:?}"
    );

    // Each row is a set of statuses the listing is narrowed to: one status some
    // jobs have, one no job has, both held statuses together, a held and an
    // unheld status together, and no filter at all.
    let cases: Vec<(&str, String, BTreeSet<String>)> = vec![
        (
            "one status the jobs have",
            format!("jobStatuses={JOB_STATUS_READY}"),
            ready.clone(),
        ),
        (
            "the other status the jobs have",
            format!("jobStatuses={JOB_STATUS_SUSPENDED}"),
            suspended.clone(),
        ),
        (
            "both statuses the jobs have",
            format!("jobStatuses={JOB_STATUS_READY}&jobStatuses={JOB_STATUS_SUSPENDED}"),
            all.clone(),
        ),
        // `Complete` and `Failed` are modelled `JobStatus` values no job here
        // is in, so they match nothing rather than being ignored.
        (
            "a status no job is in",
            "jobStatuses=Complete".to_string(),
            BTreeSet::new(),
        ),
        (
            "an unheld status beside a held one",
            format!("jobStatuses=Failed&jobStatuses={JOB_STATUS_SUSPENDED}"),
            suspended.clone(),
        ),
        ("no filter at all", String::new(), all.clone()),
    ];

    for (case, query, expected) in cases {
        let listed = list_jobs(&emulator, ACCOUNT, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed_job_ids(&listed).into_iter().collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: expected exactly the jobs in those statuses, got body {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_listing_pages_the_account_s_jobs_and_a_token_names_a_position_in_the_scan() {
    let emulator = Emulator::start();
    let rows = list_rows();
    let job_ids = create_rows(&emulator, ACCOUNT, &rows).await;
    let all: BTreeSet<String> = job_ids.iter().cloned().collect();

    // Every page size from one to one past the number of jobs, plus the
    // model's maximum, must walk the whole set exactly once.
    for page_size in [1usize, 2, 3, rows.len(), rows.len() + 1, MAX_PAGE_SIZE] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let page = list_jobs(&emulator, ACCOUNT, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            let page_ids = listed_job_ids(&page);
            assert!(
                page_ids.len() <= page_size,
                "case page size {page_size}: expected at most that many entries, got {page_ids:?}"
            );
            assert!(
                !page_ids.is_empty(),
                "case page size {page_size}: expected a non-empty page while a token was \
                 issued, got body {}",
                page.body
            );

            // The same token must answer the same page: a token is a position
            // in a scan, not a cursor the service consumes.
            if let Some(token) = &token {
                let replayed = list_jobs(
                    &emulator,
                    ACCOUNT,
                    &format!("maxResults={page_size}&nextToken={token}"),
                )
                .await;
                assert_eq!(
                    listed_job_ids(&replayed),
                    page_ids,
                    "case page size {page_size}: expected a token to answer the same page \
                     twice, got body {}",
                    replayed.body
                );
            }

            seen.extend(page_ids);
            pages += 1;
            assert!(
                pages <= rows.len() + 1,
                "case page size {page_size}: expected paging to finish, saw {pages} pages"
            );
            match page.text("NextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            seen.len(),
            all.len(),
            "case page size {page_size}: expected every job exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            all,
            "case page size {page_size}: expected the pages to cover the whole set, got {seen:?}"
        );
    }

    // A page size and a status filter compose: paging a narrowed listing walks
    // only the jobs in those statuses.
    let suspended: BTreeSet<String> = rows
        .iter()
        .zip(&job_ids)
        .filter(|(row, _)| row.expected_status() == JOB_STATUS_SUSPENDED)
        .map(|(_, job_id)| job_id.clone())
        .collect();
    let mut walked: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => format!("maxResults=1&jobStatuses={JOB_STATUS_SUSPENDED}"),
            Some(token) => {
                format!("maxResults=1&jobStatuses={JOB_STATUS_SUSPENDED}&nextToken={token}")
            }
        };
        let page = list_jobs(&emulator, ACCOUNT, &query).await;
        let page_ids = listed_job_ids(&page);
        assert_eq!(
            page_ids.len(),
            1,
            "expected one entry on a one-entry page of a narrowed listing, got body {}",
            page.body
        );
        walked.extend(page_ids);
        match page.text("NextToken") {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, suspended,
        "expected paging a narrowed listing to cover exactly the suspended jobs, got {walked:?}"
    );
}

#[tokio::test]
async fn a_listing_reports_the_newest_job_first() {
    let emulator = Emulator::start();
    let rows = list_rows();
    // Created in row order, each a moment after the one before, so the order
    // the listing must report is exactly the reverse.
    let job_ids = create_rows(&emulator, ACCOUNT, &rows).await;
    let newest_first: Vec<String> = job_ids.iter().rev().cloned().collect();

    let listed = list_jobs(&emulator, ACCOUNT, "").await;
    assert_eq!(
        listed_job_ids(&listed),
        newest_first,
        "expected the jobs newest first, got body {}",
        listed.body
    );

    // The order a page token resumes is the same order, so walking the set one
    // entry at a time sees it in exactly that sequence.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => "maxResults=1".to_string(),
            Some(token) => format!("maxResults=1&nextToken={token}"),
        };
        let page = list_jobs(&emulator, ACCOUNT, &query).await;
        walked.extend(listed_job_ids(&page));
        match page.text("NextToken") {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, newest_first,
        "expected paging to walk the same newest-first order, got {walked:?}"
    );
}

#[tokio::test]
async fn a_listing_request_the_service_cannot_serve_is_refused_rather_than_answered() {
    let emulator = Emulator::start();
    let rows = list_rows();
    let job_ids = create_rows(&emulator, ACCOUNT, &rows[..2]).await;

    // A continuation token this service never issued names no position in the
    // scan: it is the modelled `InvalidNextTokenException`, not an empty page
    // and not the first page over again.
    for (case, token) in [
        ("not the token encoding", "not-a-token"),
        ("the encoding of bytes that are not a position", "ffff"),
        // Hex of "0:0" — well-formed hex and UTF-8, but no job id, so it names
        // no entry.
        ("a position naming no job", "303a30"),
    ] {
        let listed = list_jobs(&emulator, ACCOUNT, &format!("nextToken={token}")).await;
        assert_eq!(
            (listed.status, listed.code().as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {} with body {}",
            listed.status,
            listed.body
        );
        assert!(
            job_summaries(&listed).is_empty(),
            "case {case:?}: expected a refusal to report no job, got body {}",
            listed.body
        );
    }

    // A page size outside the modelled `@range(min: 0, max: 1000)` is refused
    // rather than widened or clamped. A negative one is refused at the
    // boundary, before the handler; the handler refuses it too if it ever
    // arrives.
    for (case, max_results) in [
        ("negative", "-1"),
        ("one past the maximum", "1001"),
        ("far past the maximum", "100000"),
    ] {
        let listed = list_jobs(&emulator, ACCOUNT, &format!("maxResults={max_results}")).await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: expected a page size outside the modelled range to be refused, \
             got {} with body {}",
            listed.status,
            listed.body
        );
        assert!(
            job_summaries(&listed).is_empty(),
            "case {case:?}: expected a refusal to report no job, got body {}",
            listed.body
        );
    }

    // A status that is not a `JobStatus` matches no job, so it is refused
    // rather than narrowing the listing to nothing.
    for (case, status) in [
        ("not a status at all", "NotAStatus"),
        // The right word in the wrong case: `JobStatus` values are
        // case-sensitive.
        ("a modelled status miscased", "ready"),
        ("empty", ""),
    ] {
        assert!(
            !JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: expected {status:?} not to be one of the modelled job statuses \
             {JOB_STATUS_VALUES:?}"
        );
        let listed = list_jobs(&emulator, ACCOUNT, &format!("jobStatuses={status}")).await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: expected a status the model does not declare to be refused, got \
             {} with body {}",
            listed.status,
            listed.body
        );
        assert!(
            job_summaries(&listed).is_empty(),
            "case {case:?}: expected a refusal to report no job, got body {}",
            listed.body
        );
    }

    // And a refused listing changed nothing: the jobs are still there to be
    // listed.
    let listed = list_jobs(&emulator, ACCOUNT, "").await;
    assert_eq!(
        listed_job_ids(&listed).into_iter().collect::<BTreeSet<_>>(),
        job_ids.iter().cloned().collect::<BTreeSet<_>>(),
        "expected the refusals to leave the stored jobs alone, got body {}",
        listed.body
    );
}

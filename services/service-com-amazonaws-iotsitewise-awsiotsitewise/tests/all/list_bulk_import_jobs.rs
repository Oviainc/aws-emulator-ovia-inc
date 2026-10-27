// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListBulkImportJobs`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the bulk import jobs `CreateBulkImportJob`
//!   persisted in the scope the request names — every job of the account when it
//!   names no workspace, and only the jobs of that workspace when it names one;
//! * a workspace this service does not hold is the modelled not-found refusal,
//!   not an empty page, while a held workspace with no job of its own is an empty
//!   page;
//! * every summary reports the job's id, the name the create stated and the
//!   status the CLOCK has made due — the same values a read of that one job
//!   reports, and a listing changes neither;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are
//!   the whole listing once, in one order, whatever page size the caller asks
//!   for, and a token this listing never issued is refused;
//! * `filter` reports only the jobs in the status it names, `ALL` (and no filter
//!   at all) reports every job in scope, and a value outside the modelled filter
//!   catalogue is refused.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`,
//! `is_modelled_id`) is the asset suite's, the workspace fixture is the search
//! suite's, the dataset fixture is the data-segment suite's and the
//! storage-configuration put is that suite's; this suite reuses all four rather
//! than growing copies.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    BULK_IMPORT_JOB_STEP, STORAGE_STATE_ENABLED, STORAGE_TYPE_SITEWISE_DEFAULT,
};

use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};
use crate::dataset_data_segments::create_dataset;
use crate::search_lifecycle::create_workspace;
use crate::storage_configuration::put as put_storage;

/// The workspace the jobs that ingest into a session dataset are created in.
const WORKSPACE: &str = "plant-floor";

/// A second workspace with jobs of its own, so a scoped listing can be shown to
/// report only the jobs of the workspace it names.
const OTHER_WORKSPACE: &str = "cold-store";

/// A third workspace nobody creates a job in, so "no job" can be told apart from
/// "no workspace".
const EMPTY_WORKSPACE: &str = "quiet-site";

/// `GET /jobs`, as `OPERATIONS.md` reports this operation's route (and `POST`
/// there is `CreateBulkImportJob`).
const JOBS_URI: &str = "/jobs";

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// Every value `filter` admits. Source: `smithy_query(query="shape",
/// shape="ListBulkImportJobsFilter")` — written out here as an oracle
/// independent of the crate under test. `ALL` is the only one that is not a
/// `JobStatus`.
const FILTER_VALUES: [&str; 7] = [
    "ALL",
    "PENDING",
    "RUNNING",
    "CANCELLED",
    "FAILED",
    "COMPLETED_WITH_FAILURES",
    "COMPLETED",
];

/// Every `JobStatus` a summary may report. Source: `smithy_query(query="shape",
/// shape="JobStatus")` — the same oracle, without the filter-only `ALL`.
const JOB_STATUSES: [&str; 6] = [
    "PENDING",
    "CANCELLED",
    "RUNNING",
    "COMPLETED",
    "FAILED",
    "COMPLETED_WITH_FAILURES",
];

/// The published bounds of `maxResults` (`@range 1..250`, from
/// `smithy_query(query="operation", operation="ListBulkImportJobs")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 250;

/// Text the modelled `nextToken` pattern (`^[A-Za-z0-9+/=]+$`) admits that this
/// listing nonetheless never issued: it is not the hex of any resume key, so the
/// refusal it draws is the handler's own rather than the pattern's.
const NOT_A_TOKEN: &str = "ZZZZ";

/// Long enough that both steps the clock owes a job have elapsed, so every job
/// the suite created is in its terminal status whatever the scheduler does.
fn settled() -> Duration {
    2 * BULK_IMPORT_JOB_STEP + Duration::from_millis(250)
}

/// `GET /jobs` with the raw query string `query` (empty for none).
async fn list_raw(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        JOBS_URI.to_string()
    } else {
        format!("{JOBS_URI}?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// `ListBulkImportJobs`, asserting it answered and that the required
/// `jobSummaries` member is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, query: &str) -> Value {
    let (status, body) = list_raw(router, query).await;
    assert_eq!(
        status, 200,
        "case ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["jobSummaries"].is_array(),
        "case ?{query}: jobSummaries is a required list, got {body}"
    );
    body
}

/// The summaries a listing body reports, in the order it reported them.
fn summaries(body: &Value) -> Vec<Value> {
    body["jobSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobSummaries list, got {body}"))
        .clone()
}

/// The job ids a listing body reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries an id, got {summary}"))
                .to_string()
        })
        .collect()
}

/// One created job: the `files` the create listed and where it ingests, so the
/// rows differ in count, in the optional members a `File` carries and in the
/// destination that scopes them.
struct Case {
    label: &'static str,
    job_name: &'static str,
    files: fn() -> Value,
    /// The workspace whose session dataset the job ingests into, or `None` for a
    /// job ingesting into time series outside a workspace.
    workspace: Option<&'static str>,
}

/// The rows: jobs scoped to each of the two workspaces and to no workspace at
/// all, with one file and with several, with and without the optional members of
/// a `File`, and names spanning the forms the modelled `BulkImportJobName`
/// pattern admits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one-file-outside-a-workspace",
            job_name: "Historical import",
            files: || json!([{ "bucket": "sitewise-import", "key": "turbines/2026/01.csv" }]),
            workspace: None,
        },
        Case {
            label: "several-files-outside-a-workspace",
            job_name: "csv_import:2026/01",
            files: || {
                json!([
                    { "bucket": "sitewise-import", "key": "turbines/2026/02.csv" },
                    {
                        "bucket": "sitewise-import",
                        "key": "turbines/parquet/part-0",
                        "fileFormat": { "parquet": {} },
                    },
                ])
            },
            workspace: None,
        },
        Case {
            label: "a-file-with-every-optional-member-outside-a-workspace",
            job_name: "turbine.import-7",
            files: || {
                json!([{
                    "bucket": "sitewise-import",
                    "key": "turbines/2026/03.csv",
                    "versionId": "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd",
                    "alias": "/company/windfarm/3/turbine/7/temperature",
                    "startTime": { "timeInSeconds": 1_760_000_000_i64, "offsetInNanos": 250 },
                    "fileFormat": { "csv": { "columnNames": ["ASSET_ID", "VALUE"] } },
                }])
            },
            workspace: None,
        },
        Case {
            label: "into-a-session-dataset-of-the-first-workspace",
            job_name: "Session import",
            files: || json!([{ "bucket": "sw-import", "key": "sessions/2026/02.csv" }]),
            workspace: Some(WORKSPACE),
        },
        Case {
            label: "a-second-job-of-the-first-workspace",
            job_name: "Session import 2",
            files: || {
                json!([
                    { "bucket": "sw-import", "key": "sessions/2026/03.csv" },
                    { "bucket": "sw-import", "key": "sessions/2026/04.csv" },
                ])
            },
            workspace: Some(WORKSPACE),
        },
        Case {
            label: "into-a-session-dataset-of-the-second-workspace",
            job_name: "Cold store import",
            files: || json!([{ "bucket": "sw-import", "key": "cold/2026/01.csv" }]),
            workspace: Some(OTHER_WORKSPACE),
        },
        Case {
            label: "a-second-job-of-the-second-workspace",
            job_name: "Cold store import 2",
            files: || json!([{ "bucket": "sw-import", "key": "cold/2026/02.csv" }]),
            workspace: Some(OTHER_WORKSPACE),
        },
    ]
}

/// What the fixture created: for each job id, the row that created it.
struct Fixture {
    /// The job ids in the order the creates minted them, each with the name the
    /// create stated and the workspace it is scoped to.
    jobs: Vec<(String, &'static str, Option<&'static str>)>,
}

impl Fixture {
    /// Every job id the fixture created.
    fn all(&self) -> BTreeSet<String> {
        self.jobs.iter().map(|(id, _, _)| id.clone()).collect()
    }

    /// The ids of the jobs in `workspace` (`None` for the whole account).
    fn scoped(&self, workspace: Option<&str>) -> BTreeSet<String> {
        self.jobs
            .iter()
            .filter(|(_, _, job_workspace)| match workspace {
                None => true,
                Some(wanted) => *job_workspace == Some(wanted),
            })
            .map(|(id, _, _)| id.clone())
            .collect()
    }

    /// The `jobName` each created job was started with, by id.
    fn names(&self) -> BTreeMap<String, &'static str> {
        self.jobs
            .iter()
            .map(|(id, name, _)| (id.clone(), *name))
            .collect()
    }
}

/// The three workspaces, a session dataset in each of the two that hold jobs, the
/// warm tier the jobs outside a workspace need, and one created job per row.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    for name in [WORKSPACE, OTHER_WORKSPACE, EMPTY_WORKSPACE] {
        create_workspace(router, name).await;
    }
    // "Before you create a bulk import job that ingests data into time series
    // outside of a workspace, you must enable IoT SiteWise warm tier or IoT
    // SiteWise cold tier" — the precondition the rows with no workspace have.
    let (status, body) = put_storage(
        router,
        json!({
            "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
            "warmTier": STORAGE_STATE_ENABLED,
        }),
    )
    .await;
    assert_eq!(status, 200, "PutStorageConfiguration got {status} {body}");

    let mut datasets: BTreeMap<&'static str, String> = BTreeMap::new();
    for workspace in [WORKSPACE, OTHER_WORKSPACE] {
        let dataset_id =
            create_dataset(router, &format!("{workspace}-sessions"), None, Some(workspace)).await;
        datasets.insert(workspace, dataset_id);
    }

    let mut jobs = Vec::new();
    for case in cases() {
        let mut body = json!({
            "jobName": case.job_name,
            "jobRoleArn": JOB_ROLE_ARN,
            "files": (case.files)(),
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        });
        let object = body.as_object_mut().expect("an object body");
        if let Some(workspace) = case.workspace {
            object.insert("workspaceName".to_string(), json!(workspace));
            object.insert(
                "datasetId".to_string(),
                json!(datasets.get(workspace).expect("a created dataset")),
            );
        }
        let (status, created) = call(router, "POST", JOBS_URI, Some(body)).await;
        assert_eq!(
            status, 202,
            "case {:?}: create got {status} {created}",
            case.label
        );
        let job_id = created["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {:?}: expected a modelled jobId, got {created}",
            case.label
        );
        jobs.push((job_id, case.job_name, case.workspace));
    }
    Fixture { jobs }
}

#[tokio::test]
async fn a_listing_reports_the_jobs_in_the_scope_it_names() {
    let router = router();

    // Before any create, the account holds no job: an empty page, with no token.
    let empty = listed(&router, "").await;
    assert!(
        summaries(&empty).is_empty() && empty["nextToken"].is_null(),
        "an account with no job answers an empty page, got {empty}"
    );

    let fixture = fixture(&router).await;
    let page_size = format!("maxResults={MAX_RESULTS_MAX}");

    // Rows: the account-wide listing, which every created job answers, and each
    // workspace-scoped one, which only that workspace's jobs answer — a job of
    // another workspace and a job created for no workspace at all are both out of
    // scope.
    for (case, query, expected) in [
        ("no workspace named", page_size.clone(), fixture.scoped(None)),
        (
            "scoped to the first workspace",
            format!("workspaceName={WORKSPACE}&{page_size}"),
            fixture.scoped(Some(WORKSPACE)),
        ),
        (
            "scoped to the second workspace",
            format!("workspaceName={OTHER_WORKSPACE}&{page_size}"),
            fixture.scoped(Some(OTHER_WORKSPACE)),
        ),
        (
            "scoped to a held workspace with no job",
            format!("workspaceName={EMPTY_WORKSPACE}&{page_size}"),
            BTreeSet::new(),
        ),
    ] {
        let body = listed(&router, &query).await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        assert_eq!(
            reported,
            expected,
            "case {case:?}: expected {} jobs, got {body}",
            expected.len()
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // Every summary carries the id of a job this suite created, the name that
    // create stated, and a status of the modelled catalogue.
    let names = fixture.names();
    let body = listed(&router, &page_size).await;
    for summary in summaries(&body) {
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        let expected_name = names
            .get(&id)
            .unwrap_or_else(|| panic!("summary {summary} names a job this suite did not create"));
        assert_eq!(
            summary["name"].as_str(),
            Some(*expected_name),
            "case {id}: a summary reports the name the create stated, got {summary}"
        );
        let status = summary["status"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUSES.contains(&status),
            "case {id}: {status:?} is not a modelled JobStatus, got {summary}"
        );
    }

    // A workspace this service does not hold is the modelled absence rather than
    // an empty page — in every form of a name no `CreateWorkspace` minted,
    // including ones differing from a held name only in case, since a workspace is
    // named exactly. The workspace is resolved BEFORE the filters are read, so a
    // request that is wrong in both ways still reports the resource that is
    // absent.
    for name in ["no-such-site", "PLANT-FLOOR", "Plant-Floor", "plant-floor-2"] {
        for query in [
            String::new(),
            "&filter=COMPLETED".to_string(),
            "&filter=ALL".to_string(),
            format!("&nextToken={NOT_A_TOKEN}"),
        ] {
            let (status, body) = list_raw(&router, &format!("workspaceName={name}{query}")).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} {query:?}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "workspace {name:?} {query:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_the_status_the_clock_has_made_due() {
    let router = router();
    let fixture = fixture(&router).await;
    let page_size = format!("maxResults={MAX_RESULTS_MAX}");

    // Read straight after the creates, every job reports a status of the modelled
    // catalogue — asserted as membership rather than as one particular value,
    // since how far the clock has carried a job by the time this call lands is not
    // something the suite controls. What the clock OWES is pinned after the sleep
    // below.
    let fresh = listed(&router, &page_size).await;
    for summary in summaries(&fresh) {
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        let status = summary["status"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUSES.contains(&status),
            "case {id}: {status:?} is not a modelled JobStatus, got {summary}"
        );
    }

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a job on.
    let again = listed(&router, &page_size).await;
    assert_eq!(
        ids(&again),
        ids(&fresh),
        "two listings of one store must report the same jobs in the same order"
    );

    tokio::time::sleep(settled()).await;

    // The terminal status never moves again, so what a summary says now is stable
    // — and it is what a read of that ONE job says, since both derive it from the
    // instant the create started the job.
    let body = listed(&router, &page_size).await;
    assert_eq!(
        ids(&body).len(),
        fixture.jobs.len(),
        "every created job is still listed once, got {body}"
    );
    for summary in summaries(&body) {
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            summary["status"].as_str(),
            Some("COMPLETED"),
            "case {id}: a job the clock has settled is COMPLETED, got {summary}"
        );
        let (status, read) = call(&router, "GET", &format!("{JOBS_URI}/{id}"), None).await;
        assert_eq!(status, 200, "case {id}: describe got {status} {read}");
        assert_eq!(
            (
                read["jobId"].as_str(),
                read["jobName"].as_str(),
                read["jobStatus"].as_str()
            ),
            (
                summary["id"].as_str(),
                summary["name"].as_str(),
                summary["status"].as_str()
            ),
            "case {id}: the summary {summary} disagrees with the read {read}"
        );
    }
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    let fixture = fixture(&router).await;
    let total = fixture.jobs.len();

    // The whole listing in one page is the order every paged walk has to
    // reproduce.
    let baseline = ids(&listed(&router, &format!("maxResults={MAX_RESULTS_MAX}")).await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every created job"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that
    // do not, exactly the listing, more than it, the published maximum and no
    // `maxResults` at all.
    let sizes: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in sizes {
        let case = format!("maxResults={page_size:?}");
        let mut collected: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let body = listed(&router, &query.join("&")).await;
            let page = ids(&body);
            pages += 1;
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                    page.len()
                );
            }
            collected.extend(page);
            match body["nextToken"].as_str() {
                Some(next) => {
                    // A page followed by a token has to leave something to resume
                    // from.
                    assert!(
                        collected.len() < total,
                        "case {case:?}: page {pages} ends the listing yet carries {next}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= total + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            collected, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {collected:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the job the NEXT page starts at, in the form
    // this listing issues: the hex of that job's id, built by an oracle that does
    // not share the handler's encoder.
    let first = listed(&router, "maxResults=1").await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second job, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: unhexable text, hex of an id no job carries, and hex of the id
    // of a job the SCOPE does not cover.
    let elsewhere = fixture
        .scoped(Some(OTHER_WORKSPACE))
        .into_iter()
        .next()
        .expect("a job of the second workspace");
    for (case, query) in [
        (
            "not a token this service issues",
            format!("nextToken={NOT_A_TOKEN}"),
        ),
        (
            "hex of an unknown id",
            format!("nextToken={}", hex_token("00000000-0000-0000-0000-000000000000")),
        ),
        (
            "hex of a job outside the scope",
            format!("workspaceName={WORKSPACE}&nextToken={}", hex_token(&elsewhere)),
        ),
    ] {
        let (status, body) = list_raw(&router, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for ?{query}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250: each published bound is served and one
    // past it on either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", MAX_RESULTS_MIN, true),
        ("the range maximum", MAX_RESULTS_MAX, true),
        ("one under the minimum", MAX_RESULTS_MIN - 1, false),
        ("one past the maximum", MAX_RESULTS_MAX + 1, false),
    ] {
        let (status, body) = list_raw(&router, &format!("maxResults={max_results}")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_reports_only_the_jobs_in_the_status_it_names() {
    let router = router();
    let fixture = fixture(&router).await;
    let all = fixture.all();
    let page_size = format!("maxResults={MAX_RESULTS_MAX}");

    // While the clock still owes these jobs their transitions, the only invariant
    // that cannot depend on when the test runs: a filtered page reports jobs in
    // THAT status and nothing else, and `ALL` reports every job of the account.
    for filter in FILTER_VALUES {
        let body = listed(&router, &format!("filter={filter}&{page_size}")).await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        if filter == "ALL" {
            assert_eq!(
                reported, all,
                "case filter=ALL: every job of the account is in scope, got {body}"
            );
            continue;
        }
        for summary in summaries(&body) {
            assert_eq!(
                summary["status"].as_str(),
                Some(filter),
                "case filter={filter}: a filtered page reports only that status, got {summary}"
            );
        }
        assert!(
            reported.is_subset(&all),
            "case filter={filter}: a filtered page reports only created jobs, got {body}"
        );
    }

    tokio::time::sleep(settled()).await;

    // The terminal status never moves again, so the partition is now stable:
    // every job this suite created is COMPLETED, no other status holds one, and
    // `ALL` still holds them all.
    for filter in FILTER_VALUES {
        let body = listed(&router, &format!("filter={filter}&{page_size}")).await;
        let reported: BTreeSet<String> = ids(&body).into_iter().collect();
        let expected: BTreeSet<String> = if filter == "ALL" || filter == "COMPLETED" {
            all.clone()
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            reported, expected,
            "case filter={filter}: expected {} jobs, got {body}",
            expected.len()
        );
    }

    // A request that names no filter reads as `ALL`: the same page, scope for
    // scope.
    for (case, scope) in [
        ("the whole account", String::new()),
        (
            "one workspace",
            format!("workspaceName={WORKSPACE}&"),
        ),
    ] {
        let unfiltered = listed(&router, &format!("{scope}{page_size}")).await;
        let filtered = listed(&router, &format!("{scope}filter=ALL&{page_size}")).await;
        assert_eq!(
            ids(&unfiltered),
            ids(&filtered),
            "case {case:?}: no filter reads as ALL, got {unfiltered} and {filtered}"
        );
    }

    // A filter is narrowed by the scope, not the other way round: the jobs of one
    // workspace in the status the filter names, and no job of another workspace.
    let here = fixture.scoped(Some(WORKSPACE));
    let body = listed(
        &router,
        &format!("workspaceName={WORKSPACE}&filter=COMPLETED&{page_size}"),
    )
    .await;
    let reported: BTreeSet<String> = ids(&body).into_iter().collect();
    assert_eq!(
        reported, here,
        "case a filtered scoped listing: expected {} jobs of {WORKSPACE}, got {body}",
        here.len()
    );

    // A value outside the modelled filter catalogue names no status at all, so it
    // is the bad request it is rather than an empty page: a value in the wrong
    // case, a near-miss, a status of another API, a word that is no status and one
    // longer than every value of the catalogue.
    for (case, filter) in [
        ("the wrong case", "completed".to_string()),
        ("a near-miss", "RUNNIN".to_string()),
        ("a status of another API", "SUBMITTED".to_string()),
        ("a word that is no status", "EVERYTHING".to_string()),
        ("a long value", "C".repeat(64)),
    ] {
        let (status, body) = list_raw(&router, &format!("filter={filter}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for filter={filter}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

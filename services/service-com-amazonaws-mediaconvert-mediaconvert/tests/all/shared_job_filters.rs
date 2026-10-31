// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ONE narrowing path every jobs reader takes.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness —
//! the router picks the service from the request's SigV4 scope, the framework deserializes
//! and validates, the handler runs, and the response is serialized — so what is asserted is
//! the bytes a client would see, never an internal call.
//!
//! The rule under test, which every row below is one case of: a job detail a jobs reader
//! narrows on is read off the stored job, so the same value narrows the job set to the same
//! jobs through every reader that offers it — a `queue` keeps the jobs sent to exactly that
//! queue, a `status` the jobs in exactly that status and an input file the jobs one of whose
//! inputs contains it — and the filters ONE request carries narrow together, so a reader
//! reports only the jobs satisfying all of them.
//!
//! The three readers below are the three that narrow on those details: `ListJobs`'s and
//! `SearchJobs`'s `queue`, `status` and (for `SearchJobs`) `inputFile` members, and the
//! asynchronous jobs query's `queue`, `status` and `fileInput` filter keys. The rows span
//! each filter the readers share, over queues holding one job, two jobs, the account's
//! built-in queue and a queue no job was sent to, over every `JobStatus` the model declares,
//! over a whole input URL and partial ones at several positions of it, and over the
//! combinations where the filters narrow together.
//!
//! WHICH FORMS of a queue reference each reader resolves is a different rule, pinned in
//! `tests/all/list_jobs_queue_filter.rs`; every row here names a queue by the one form all
//! three readers offer, its name.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::job_lifecycle::{settings, Emulator, Reply, ROLE};
use super::job_search::job_ids;
use super::jobs_query::{pin_query_span, run_query, SHORT_QUERY};
use super::resource_tagging::encode_label;

/// The paths the two paged readers are bound to, from the model
/// (`smithy_query(query="digest")`): `GET /2017-08-29/jobs` and `GET /2017-08-29/search`.
const LIST_JOBS: &str = "/2017-08-29/jobs";
const SEARCH_JOBS: &str = "/2017-08-29/search";

/// Every value of `JobStatus`, from the model
/// (`smithy_query(query="shape",shape="JobStatus")`): "A job's status can be SUBMITTED,
/// PROGRESSING, COMPLETE, CANCELED, or ERROR."
const EVERY_JOB_STATUS: [&str; 5] = ["CANCELED", "COMPLETE", "ERROR", "PROGRESSING", "SUBMITTED"];

/// The two queues this suite creates, beside the account's built-in `Default` one.
const QUEUE_ONE: &str = "narrow-one";
const QUEUE_TWO: &str = "narrow-two";

/// Which job detail one row narrows on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    Queue,
    Status,
    InputFile,
}

impl Detail {
    /// The query-string member the two paged readers take this detail in.
    fn member(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Status => "status",
            Self::InputFile => "inputFile",
        }
    }

    /// The `JobsQueryFilterKey` the jobs query takes this detail in.
    fn filter_key(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::Status => "status",
            Self::InputFile => "fileInput",
        }
    }

    /// Whether `ListJobs` offers this detail at all: its request carries `queue` and
    /// `status` and no input-file member, so a row narrowing on an input file is driven
    /// through the other two readers only.
    fn on_list_jobs(self) -> bool {
        self != Self::InputFile
    }
}

/// The four jobs every row narrows over, in creation order, and the labels this suite names
/// them by. They differ in every detail the readers share: the queue they were sent to,
/// their status, and the bucket, path and extension of their input file.
struct Fixture {
    /// Queue [`QUEUE_ONE`], input `s3://example-bucket/in/one-clip.mp4`, left `SUBMITTED`.
    one_submitted: String,
    /// Queue [`QUEUE_TWO`], input `s3://example-bucket/in/two-clip.mov`, left `SUBMITTED`.
    two_submitted: String,
    /// Queue [`QUEUE_ONE`], input `s3://example-bucket/archive/three-clip.mp4`, `CANCELED`.
    one_canceled: String,
    /// The built-in `Default` queue, input `s3://other-bucket/media/four-clip.mp4`,
    /// `CANCELED`.
    default_canceled: String,
}

impl Fixture {
    /// Every stored job — the set a read carrying no filter reports.
    fn every_job(&self) -> BTreeSet<String> {
        BTreeSet::from([
            self.one_submitted.clone(),
            self.two_submitted.clone(),
            self.one_canceled.clone(),
            self.default_canceled.clone(),
        ])
    }
}

/// Create the queues and the jobs, cancel the two that are to be `CANCELED`, and answer
/// the ids by label.
async fn seed(emulator: &Emulator) -> Fixture {
    for name in [QUEUE_ONE, QUEUE_TWO] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }

    let mut ids = Vec::new();
    for (queue, file_input, modifier) in [
        (QUEUE_ONE, "s3://example-bucket/in/one-clip.mp4", "_one"),
        (QUEUE_TWO, "s3://example-bucket/in/two-clip.mov", "_two"),
        (
            QUEUE_ONE,
            "s3://example-bucket/archive/three-clip.mp4",
            "_three",
        ),
        (
            "Default",
            "s3://other-bucket/media/four-clip.mp4",
            "_four",
        ),
    ] {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings(file_input, modifier)
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "the fixture job {modifier:?} on {queue:?} must be created: got {} with body {}",
            created.status, created.body
        );
        ids.push(created.string("/job/id"));
    }

    let default_canceled = ids.pop().unwrap_or_default();
    let one_canceled = ids.pop().unwrap_or_default();
    let two_submitted = ids.pop().unwrap_or_default();
    let one_submitted = ids.pop().unwrap_or_default();

    for id in [&one_canceled, &default_canceled] {
        let canceled = emulator.cancel_job(id).await;
        assert_eq!(
            canceled.status, 202,
            "the fixture job {id:?} must be cancelled: got {} with body {}",
            canceled.status, canceled.body
        );
    }

    Fixture {
        one_submitted,
        two_submitted,
        one_canceled,
        default_canceled,
    }
}

/// One read of a paged reader, narrowed by the `member=value` pairs a row names.
///
/// Each value goes on the wire percent-encoded through the test binary's one encoding rule
/// ([`encode_label`]), so a row may carry an input file URL's `:` and `/` and the handler
/// still reads the value the row states.
async fn narrowed(emulator: &Emulator, path: &str, filters: &[(Detail, &str)]) -> Reply {
    let query = filters
        .iter()
        .map(|(detail, value)| format!("{}={}", detail.member(), encode_label(value)))
        .collect::<Vec<String>>()
        .join("&");
    let uri = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// The jobs one reply reported.
fn reported(reply: &Reply) -> BTreeSet<String> {
    job_ids(reply).into_iter().collect()
}

/// One jobs query carrying one filter: one key with one value, which is the narrowing a row
/// states (a query's filter list combines its terms with OR, so a single-term list reports
/// exactly the jobs that term keeps).
async fn queried(emulator: &Emulator, detail: Detail, value: &str) -> Reply {
    run_query(
        emulator,
        json!({ "filterList": [{ "key": detail.filter_key(), "values": [value] }] }),
    )
    .await
}

/// One row of the single-filter table.
struct Row {
    /// How a failing assertion names this row.
    case: &'static str,
    /// The detail the row narrows on.
    detail: Detail,
    /// The value it narrows by.
    value: String,
    /// The jobs the rule keeps.
    expected: BTreeSet<String>,
}

/// Every single-filter row, over the fixture jobs.
fn single_filter_rows(jobs: &Fixture) -> Vec<Row> {
    let on_one = BTreeSet::from([jobs.one_submitted.clone(), jobs.one_canceled.clone()]);
    let on_two = BTreeSet::from([jobs.two_submitted.clone()]);
    let on_default = BTreeSet::from([jobs.default_canceled.clone()]);
    let submitted = BTreeSet::from([jobs.one_submitted.clone(), jobs.two_submitted.clone()]);
    let canceled = BTreeSet::from([jobs.one_canceled.clone(), jobs.default_canceled.clone()]);
    let none = BTreeSet::new();

    let mut rows = vec![
        // `queue`: the jobs sent to exactly that queue — one holding two jobs, one holding
        // one, the account's built-in one, and a name no queue holds.
        Row {
            case: "queue holding two jobs",
            detail: Detail::Queue,
            value: QUEUE_ONE.to_string(),
            expected: on_one.clone(),
        },
        Row {
            case: "queue holding one job",
            detail: Detail::Queue,
            value: QUEUE_TWO.to_string(),
            expected: on_two,
        },
        Row {
            case: "the built-in queue",
            detail: Detail::Queue,
            value: "Default".to_string(),
            expected: on_default.clone(),
        },
        Row {
            case: "a queue no job was sent to",
            detail: Detail::Queue,
            value: "no-such-queue".to_string(),
            expected: none.clone(),
        },
        // `inputFile`: the jobs one of whose inputs CONTAINS the value — a whole URL, a
        // partial name, an extension three inputs share, a path segment, a bucket, and text
        // no input carries.
        Row {
            case: "inputFile, a whole URL",
            detail: Detail::InputFile,
            value: "s3://other-bucket/media/four-clip.mp4".to_string(),
            expected: on_default,
        },
        Row {
            case: "inputFile, a partial base name",
            detail: Detail::InputFile,
            value: "two-clip".to_string(),
            expected: BTreeSet::from([jobs.two_submitted.clone()]),
        },
        Row {
            case: "inputFile, an extension three inputs share",
            detail: Detail::InputFile,
            value: ".mp4".to_string(),
            expected: BTreeSet::from([
                jobs.one_submitted.clone(),
                jobs.one_canceled.clone(),
                jobs.default_canceled.clone(),
            ]),
        },
        Row {
            case: "inputFile, a path segment",
            detail: Detail::InputFile,
            value: "/in/".to_string(),
            expected: BTreeSet::from([jobs.one_submitted.clone(), jobs.two_submitted.clone()]),
        },
        Row {
            case: "inputFile, a bucket",
            detail: Detail::InputFile,
            value: "example-bucket".to_string(),
            expected: BTreeSet::from([
                jobs.one_submitted.clone(),
                jobs.two_submitted.clone(),
                jobs.one_canceled.clone(),
            ]),
        },
        Row {
            case: "inputFile no input carries",
            detail: Detail::InputFile,
            value: "no-such-input".to_string(),
            expected: none.clone(),
        },
    ];

    // `status`: every value the model declares, so the two a fixture job is in and the
    // three none is are all driven.
    for status in EVERY_JOB_STATUS {
        let expected = match status {
            "SUBMITTED" => submitted.clone(),
            "CANCELED" => canceled.clone(),
            _ => none.clone(),
        };
        rows.push(Row {
            case: "status",
            detail: Detail::Status,
            value: status.to_string(),
            expected,
        });
    }

    rows
}

/// One value narrows the job set to the same jobs through every reader that offers the
/// detail it names.
#[tokio::test]
async fn every_jobs_reader_narrows_one_detail_to_the_same_jobs() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed(&emulator).await;

    for row in single_filter_rows(&jobs) {
        let label = format!("{} {:?}={:?}", row.case, row.detail.member(), row.value);
        let mut reports: Vec<(&str, BTreeSet<String>)> = Vec::new();

        if row.detail.on_list_jobs() {
            let reply = narrowed(&emulator, LIST_JOBS, &[(row.detail, &row.value)]).await;
            assert_eq!(
                reply.status, 200,
                "case {label}: ListJobs must answer HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            reports.push(("ListJobs", reported(&reply)));
        }

        let reply = narrowed(&emulator, SEARCH_JOBS, &[(row.detail, &row.value)]).await;
        assert_eq!(
            reply.status, 200,
            "case {label}: SearchJobs must answer HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        reports.push(("SearchJobs", reported(&reply)));

        let results = queried(&emulator, row.detail, &row.value).await;
        reports.push(("a jobs query", reported(&results)));

        for (reader, actual) in &reports {
            assert_eq!(
                actual, &row.expected,
                "case {label}: {reader} must report the jobs {:?}, got {actual:?}",
                row.expected
            );
        }
    }

    // Narrowing is a report: every job is still where it was after all of it.
    let unfiltered = narrowed(&emulator, LIST_JOBS, &[]).await;
    let actual = reported(&unfiltered);
    assert_eq!(
        actual,
        jobs.every_job(),
        "case after every narrowed read: expected the stored jobs {:?}, got {actual:?} from \
         body {}",
        jobs.every_job(),
        unfiltered.body
    );
}

/// One row of the conjunction table: the filters one request carries and the jobs that
/// leaves.
struct Conjunction {
    case: &'static str,
    /// The `queue` the request names, if any.
    queue: Option<&'static str>,
    /// The `status` it names, if any.
    status: Option<&'static str>,
    /// The input file it names, if any — a `SearchJobs` row only.
    input_file: Option<&'static str>,
    /// The jobs the conjunction keeps.
    expected: BTreeSet<String>,
}

/// Every filter a request carries narrows the set together: a reader reports only the jobs
/// satisfying all of them, and a filter it leaves out narrows nothing.
#[tokio::test]
async fn the_filters_one_request_carries_narrow_together() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;

    let rows = vec![
        Conjunction {
            case: "no filter at all",
            queue: None,
            status: None,
            input_file: None,
            expected: jobs.every_job(),
        },
        Conjunction {
            case: "a queue and the status one of its jobs is in",
            queue: Some(QUEUE_ONE),
            status: Some("SUBMITTED"),
            input_file: None,
            expected: BTreeSet::from([jobs.one_submitted.clone()]),
        },
        Conjunction {
            case: "a queue and the status its other job is in",
            queue: Some(QUEUE_ONE),
            status: Some("CANCELED"),
            input_file: None,
            expected: BTreeSet::from([jobs.one_canceled.clone()]),
        },
        Conjunction {
            case: "a queue and a status no job of it is in",
            queue: Some(QUEUE_TWO),
            status: Some("CANCELED"),
            input_file: None,
            expected: BTreeSet::new(),
        },
        Conjunction {
            case: "the built-in queue and a status no job of it is in",
            queue: Some("Default"),
            status: Some("SUBMITTED"),
            input_file: None,
            expected: BTreeSet::new(),
        },
        Conjunction {
            case: "a status and an input file",
            queue: None,
            status: Some("SUBMITTED"),
            input_file: Some(".mp4"),
            expected: BTreeSet::from([jobs.one_submitted.clone()]),
        },
        Conjunction {
            case: "a queue and an input file",
            queue: Some(QUEUE_ONE),
            status: None,
            input_file: Some(".mp4"),
            expected: BTreeSet::from([jobs.one_submitted.clone(), jobs.one_canceled.clone()]),
        },
        Conjunction {
            case: "all three filters, which one job satisfies",
            queue: Some(QUEUE_ONE),
            status: Some("SUBMITTED"),
            input_file: Some("/in/"),
            expected: BTreeSet::from([jobs.one_submitted.clone()]),
        },
        Conjunction {
            case: "all three filters, which no job satisfies",
            queue: Some(QUEUE_ONE),
            status: Some("CANCELED"),
            input_file: Some("/in/"),
            expected: BTreeSet::new(),
        },
    ];

    for row in rows {
        let mut filters: Vec<(Detail, &str)> = Vec::new();
        if let Some(queue) = row.queue {
            filters.push((Detail::Queue, queue));
        }
        if let Some(status) = row.status {
            filters.push((Detail::Status, status));
        }
        // `ListJobs` offers no input-file member, so a row naming one is a `SearchJobs` row
        // and the readers the row is driven through are those whose request carries every
        // filter it names.
        let paths: &[&str] = if row.input_file.is_some() {
            &[SEARCH_JOBS]
        } else {
            &[LIST_JOBS, SEARCH_JOBS]
        };
        if let Some(input_file) = row.input_file {
            filters.push((Detail::InputFile, input_file));
        }

        for path in paths {
            let label = format!("{} through {path}", row.case);
            let reply = narrowed(&emulator, path, &filters).await;
            assert_eq!(
                reply.status, 200,
                "case {label}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let actual = reported(&reply);
            assert_eq!(
                actual, row.expected,
                "case {label}: expected the jobs {:?}, got {actual:?} from body {}",
                row.expected, reply.body
            );
            assert!(
                reply.at("/nextToken").is_null(),
                "case {label}: one page holds every match, so no nextToken, got {}",
                reply.body
            );
        }
    }
}

/// A narrowed read reports the stored job itself: every job a reader keeps is the record
/// `GetJob` reports, so the filters select a job rather than building a second view of it.
#[tokio::test]
async fn a_narrowed_read_reports_the_stored_job() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;

    for (case, id, filters) in [
        (
            "a submitted job, by queue and status",
            &jobs.one_submitted,
            vec![(Detail::Queue, QUEUE_ONE), (Detail::Status, "SUBMITTED")],
        ),
        (
            "a cancelled job, by queue and status",
            &jobs.one_canceled,
            vec![(Detail::Queue, QUEUE_ONE), (Detail::Status, "CANCELED")],
        ),
        (
            "a job in the built-in queue, by queue",
            &jobs.default_canceled,
            vec![(Detail::Queue, "Default")],
        ),
    ] {
        let fetched = emulator.get_job(id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetJob must answer HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );

        for path in [LIST_JOBS, SEARCH_JOBS] {
            let reply = narrowed(&emulator, path, &filters).await;
            let job = reply
                .at("/jobs")
                .as_array()
                .and_then(|listed| {
                    listed
                        .iter()
                        .find(|job| job.pointer("/id").and_then(Value::as_str) == Some(id.as_str()))
                })
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                &job,
                fetched.at("/job"),
                "case {case:?} through {path}: expected the job GetJob reports {}, got {job}",
                fetched.at("/job")
            );
        }
    }
}

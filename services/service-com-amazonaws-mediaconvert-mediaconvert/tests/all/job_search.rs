// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `SearchJobs`.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from
//! the request's SigV4 scope, the framework deserializes and validates, the handler
//! runs, and the response is serialized — so what is asserted is the bytes a client
//! would see, never an internal call.
//!
//! The rule under test, which every case below is one row of: `SearchJobs` reports the
//! jobs this service persisted — "Retrieve a JSON array that includes job details for up
//! to twenty of your most recent jobs. Optionally filter results further according to
//! input file, queue, or status." — so every filter the request carries NARROWS the
//! stored set (`inputFile` by containment over an input's `FileInput`, since it is "your
//! input file URL or your partial input file name"; `queue` by the queue a job was sent
//! to, named or given as an ARN; `status` by the job's current status), the survivors come
//! back ordered by creation time in the requested direction and newest first by default,
//! and a page carries at most `maxResults` jobs plus a `nextToken` only while a further
//! page remains.

use std::collections::BTreeSet;

use serde_json::Value;
use serde_json::json;

use super::job_lifecycle::{arn, settings, Emulator, Reply, PAST_THE_SETTLE, ROLE, SHORT_SETTLE};

/// One search, with the query string a case drives it through.
async fn search_jobs(emulator: &Emulator, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/2017-08-29/search".to_string()
    } else {
        format!("/2017-08-29/search?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// The `id` of every job in a reply, in the order returned.
fn job_ids(reply: &Reply) -> Vec<String> {
    reply
        .at("/jobs")
        .as_array()
        .map(|jobs| {
            jobs.iter()
                .map(|job| {
                    job.pointer("/id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The four jobs every filtering case searches over, in creation order (so oldest
/// first), and the labels this suite names them by.
struct Fixture {
    /// queue `queue-a`, input `s3://example-bucket/in/alpha-clip.mp4`, left `SUBMITTED`.
    alpha: String,
    /// queue `queue-b`, input `s3://example-bucket/in/beta-clip.mov`, left `SUBMITTED`.
    beta: String,
    /// queue `queue-a`, input `s3://example-bucket/archive/gamma-clip.mp4`, `CANCELED`.
    gamma: String,
    /// queue `queue-b`, input `s3://other-bucket/media/delta-clip.mp4`, left `SUBMITTED`.
    delta: String,
}

impl Fixture {
    /// Every job, oldest first — the order `ASCENDING` reports them in.
    fn oldest_first(&self) -> Vec<String> {
        vec![
            self.alpha.clone(),
            self.beta.clone(),
            self.gamma.clone(),
            self.delta.clone(),
        ]
    }

    /// Every job, newest first — the order a search with no `order` reports them in.
    fn newest_first(&self) -> Vec<String> {
        let mut ids = self.oldest_first();
        ids.reverse();
        ids
    }
}

/// Four jobs differing in every job detail a search can narrow on: the queue they were
/// sent to, their status, and the bucket, path and extension of their input file.
async fn seed_jobs(emulator: &Emulator) -> Fixture {
    for name in ["queue-a", "queue-b"] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }

    let mut ids = Vec::new();
    for (queue, file_input, modifier) in [
        ("queue-a", "s3://example-bucket/in/alpha-clip.mp4", "_alpha"),
        ("queue-b", "s3://example-bucket/in/beta-clip.mov", "_beta"),
        (
            "queue-a",
            "s3://example-bucket/archive/gamma-clip.mp4",
            "_gamma",
        ),
        ("queue-b", "s3://other-bucket/media/delta-clip.mp4", "_delta"),
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
            "the fixture job {modifier:?} must be created: got {} with body {}",
            created.status, created.body
        );
        ids.push(created.string("/job/id"));
    }

    let delta = ids.pop().unwrap_or_default();
    let gamma = ids.pop().unwrap_or_default();
    let beta = ids.pop().unwrap_or_default();
    let alpha = ids.pop().unwrap_or_default();

    let canceled = emulator.cancel_job(&gamma).await;
    assert_eq!(
        canceled.status, 202,
        "the fixture job must be cancelled: got {} with body {}",
        canceled.status, canceled.body
    );

    Fixture {
        alpha,
        beta,
        gamma,
        delta,
    }
}

/// A search reports exactly the stored jobs every filter it carries keeps.
///
/// The table spans the three documented filters and the forms each comes in — a whole
/// input file URL, a partial one at every position of the URL (bucket, path segment,
/// base name, extension) and one no input carries; a queue by name and by ARN, and one
/// no job was sent to; each `JobStatus` a fixture job is in and ones none is — plus the
/// combinations, where the filters narrow together.
#[tokio::test]
async fn a_search_reports_the_stored_jobs_every_filter_keeps() {
    let emulator = Emulator::new();
    let jobs = seed_jobs(&emulator).await;

    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        // No filter narrows nothing: every stored job comes back.
        ("no filter", String::new(), vec![
            &jobs.alpha,
            &jobs.beta,
            &jobs.gamma,
            &jobs.delta,
        ]),
        // `inputFile` — "your input file URL or your partial input file name".
        (
            "inputFile, whole URL",
            "inputFile=s3://example-bucket/in/beta-clip.mov".to_string(),
            vec![&jobs.beta],
        ),
        (
            "inputFile, partial base name",
            "inputFile=alpha-clip".to_string(),
            vec![&jobs.alpha],
        ),
        (
            "inputFile, extension shared by three inputs",
            "inputFile=.mp4".to_string(),
            vec![&jobs.alpha, &jobs.gamma, &jobs.delta],
        ),
        (
            "inputFile, a path segment",
            "inputFile=/in/".to_string(),
            vec![&jobs.alpha, &jobs.beta],
        ),
        (
            "inputFile, another path segment",
            "inputFile=/archive/".to_string(),
            vec![&jobs.gamma],
        ),
        (
            "inputFile, a bucket",
            "inputFile=other-bucket".to_string(),
            vec![&jobs.delta],
        ),
        (
            "inputFile, the shared bucket",
            "inputFile=s3://example-bucket".to_string(),
            vec![&jobs.alpha, &jobs.beta, &jobs.gamma],
        ),
        (
            "inputFile no input carries",
            "inputFile=no-such-input".to_string(),
            vec![],
        ),
        // `queue` — "Provide a queue name, or a queue ARN, to return only jobs from
        // that queue", so both forms resolve to the same queue.
        (
            "queue by name",
            "queue=queue-a".to_string(),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        (
            "queue by ARN",
            format!("queue={}", arn("queues/queue-b")),
            vec![&jobs.beta, &jobs.delta],
        ),
        (
            "the built-in queue, which no fixture job was sent to",
            "queue=Default".to_string(),
            vec![],
        ),
        ("queue no job was sent to", "queue=no-such-queue".to_string(), vec![]),
        // `status` — "A job's status can be SUBMITTED, PROGRESSING, COMPLETE, CANCELED,
        // or ERROR."
        (
            "status SUBMITTED",
            "status=SUBMITTED".to_string(),
            vec![&jobs.alpha, &jobs.beta, &jobs.delta],
        ),
        ("status CANCELED", "status=CANCELED".to_string(), vec![&jobs.gamma]),
        ("status PROGRESSING, which no job is in", "status=PROGRESSING".to_string(), vec![]),
        ("status COMPLETE, which no job is in", "status=COMPLETE".to_string(), vec![]),
        ("status ERROR, which no job is in", "status=ERROR".to_string(), vec![]),
        // Filters narrow TOGETHER: a job must satisfy every one of them.
        (
            "inputFile and status",
            "inputFile=.mp4&status=SUBMITTED".to_string(),
            vec![&jobs.alpha, &jobs.delta],
        ),
        (
            "inputFile and queue",
            "inputFile=.mp4&queue=queue-a".to_string(),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        (
            "queue and status, nothing in both",
            "queue=queue-b&status=CANCELED".to_string(),
            vec![],
        ),
        (
            "all three filters at once",
            "inputFile=clip&queue=queue-a&status=SUBMITTED".to_string(),
            vec![&jobs.alpha],
        ),
    ];

    for (case, query, expected) in cases {
        let reply = search_jobs(&emulator, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let actual: BTreeSet<String> = job_ids(&reply).into_iter().collect();
        let expected: BTreeSet<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the jobs {expected:?}, got {actual:?} from body {}",
            reply.body
        );
        assert!(
            reply.at("/nextToken").is_null(),
            "case {case:?}: one page holds every match, so no nextToken, got {}",
            reply.body
        );
    }
}

/// The matches are ordered by creation time — newest first unless `order` asks otherwise
/// — and paged at `maxResults`, with a `nextToken` the caller hands back to retrieve
/// "the twenty next most recent jobs" and nothing repeated or skipped.
#[tokio::test]
async fn the_matches_are_ordered_and_paged() {
    let emulator = Emulator::new();
    let jobs = seed_jobs(&emulator).await;
    let newest_first = jobs.newest_first();
    let oldest_first = jobs.oldest_first();

    for (case, order, expected) in [
        ("no order", None, &newest_first),
        ("DESCENDING", Some("DESCENDING"), &newest_first),
        ("ASCENDING", Some("ASCENDING"), &oldest_first),
    ] {
        let query = order.map(|order| format!("order={order}")).unwrap_or_default();
        let reply = search_jobs(&emulator, &query).await;
        let actual = job_ids(&reply);
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?} from body {}",
            reply.body
        );
    }

    // Every page size from one job per page up to one past the match count, in both
    // directions.
    for (case, order, expected) in [
        ("ASCENDING", "ASCENDING", &oldest_first),
        ("DESCENDING", "DESCENDING", &newest_first),
    ] {
        for page_size in 1..=expected.len() + 1 {
            let mut seen: Vec<String> = Vec::new();
            let mut cursor: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query = format!("order={order}&maxResults={page_size}");
                if let Some(token) = cursor.as_deref() {
                    query.push_str(&format!("&nextToken={token}"));
                }
                let reply = search_jobs(&emulator, &query).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} maxResults={page_size}: expected HTTP 200, got {} with \
                     body {}",
                    reply.status, reply.body
                );
                let page = job_ids(&reply);
                assert!(
                    page.len() <= page_size,
                    "case {case:?} maxResults={page_size}: a page holds at most \
                     {page_size} jobs, got {page:?}"
                );
                seen.extend(page);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?} maxResults={page_size}: paging must terminate, saw \
                     {pages} pages for {seen:?}"
                );
                match reply.at("/nextToken").as_str().map(str::to_string) {
                    Some(token) => cursor = Some(token),
                    None => break,
                }
            }
            assert_eq!(
                &seen, expected,
                "case {case:?} maxResults={page_size}: paging must walk every match once, \
                 in order, got {seen:?}"
            );
        }
    }
}

/// A searched job is the same record `GetJob` reports: the search projects the stored
/// job, it does not build a second view of it.
#[tokio::test]
async fn a_searched_job_is_the_job_get_job_reports() {
    let emulator = Emulator::new();
    let jobs = seed_jobs(&emulator).await;

    for (case, id) in [
        ("a submitted job", &jobs.alpha),
        ("a cancelled job", &jobs.gamma),
        ("a job in the other queue", &jobs.delta),
    ] {
        let fetched = emulator.get_job(id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetJob must answer HTTP 200, got {} with body {}",
            fetched.status, fetched.body
        );
        let searched = search_jobs(&emulator, &format!("inputFile={id}")).await;
        // The id is not an input file, so searching by it finds nothing — the job is
        // found through the filters, and compared member by member.
        assert!(
            job_ids(&searched).is_empty(),
            "case {case:?}: an id is not an input file, got {}",
            searched.body
        );

        let found = search_jobs(&emulator, "maxResults=20").await;
        let job = found
            .at("/jobs")
            .as_array()
            .and_then(|jobs| {
                jobs.iter()
                    .find(|job| job.pointer("/id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            &job,
            fetched.at("/job"),
            "case {case:?}: the searched job must be the job GetJob reports, got {job} \
             against {}",
            fetched.at("/job")
        );
    }
}

/// A search is a report: it changes nothing a later caller sees. A job due to leave
/// `SUBMITTED` still leaves it on the clock's schedule and not before, and whatever a
/// search found stays found.
#[tokio::test]
async fn searching_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::with_settle_span(SHORT_SETTLE);
    let created = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": settings("s3://example-bucket/in/polled.mp4", "_polled")
        }))
        .await;
    assert_eq!(
        created.status, 201,
        "the job must be created: got {} with body {}",
        created.status, created.body
    );
    let id = created.string("/job/id");

    // Immediately: the job is still in its non-terminal status, however often it is
    // searched for.
    for attempt in 0..3 {
        let reply = search_jobs(&emulator, "status=SUBMITTED").await;
        assert_eq!(
            job_ids(&reply),
            vec![id.clone()],
            "attempt {attempt}: the job must still be SUBMITTED, got {}",
            reply.body
        );
    }
    let fetched = emulator.get_job(&id).await;
    assert_eq!(
        fetched.string("/job/status"),
        "SUBMITTED",
        "searching must not move the job's status, got {}",
        fetched.body
    );

    // Past the due instant, the transition the clock owed has happened — and it is the
    // clock's, so the search reports it and two searches report the same thing.
    tokio::time::sleep(PAST_THE_SETTLE).await;
    let first = search_jobs(&emulator, "").await;
    let second = search_jobs(&emulator, "").await;
    assert_eq!(
        first.at("/jobs"),
        second.at("/jobs"),
        "two searches in a row must report the same jobs, got {} then {}",
        first.body,
        second.body
    );
    let settled = emulator.get_job(&id).await;
    assert_ne!(
        settled.string("/job/status"),
        "SUBMITTED",
        "past its due instant the job must have left SUBMITTED, got {}",
        settled.body
    );
    assert_eq!(
        first.at("/jobs").as_array().and_then(|jobs| jobs.first()),
        Some(settled.at("/job")),
        "the search must report the settled job, got {} against {}",
        first.body,
        settled.body
    );
}

/// A `nextToken` this service never issued is a problem in the request, not an empty
/// page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let emulator = Emulator::new();
    seed_jobs(&emulator).await;

    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex of nothing this service issued", "6e6f70653a6e6f7065"),
    ] {
        let refused = search_jobs(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_type
        );
    }

    // `maxResults` carries the model's own 1..20 range, so a value outside it is refused
    // at the boundary rather than clamped.
    for (case, value) in [("zero", "0"), ("past the maximum", "21")] {
        let refused = search_jobs(&emulator, &format!("maxResults={value}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults {case}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
    }

    // Each published bound of that range is accepted.
    for value in [1, 20] {
        let reply = search_jobs(&emulator, &format!("maxResults={value}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={value}: a published bound must be accepted, got {} with \
             body {}",
            reply.status, reply.body
        );
        assert!(
            job_ids(&reply).len() <= value,
            "case maxResults={value}: a page holds at most {value} jobs, got {:?}",
            job_ids(&reply)
        );
    }
}

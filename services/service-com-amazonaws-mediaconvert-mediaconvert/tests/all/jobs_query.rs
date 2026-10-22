// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the asynchronous jobs query: `StartJobsQuery` and
//! `GetJobsQueryResults`.
//!
//! Every case drives the real restJson1 wire path — the router picks the service from
//! the request's SigV4 scope, the framework deserializes and validates, the handler
//! runs, and the response is serialized — so what is asserted is the bytes a client
//! would see, never an internal call.
//!
//! The rule under test, which every case below is one row of: `StartJobsQuery` persists
//! the query the caller described and answers its id; the query leaves its non-terminal
//! `SUBMITTED` status when the clock makes its completion due, at which point it matches
//! the stored jobs — a job matching when ANY value of ANY filter matches the job detail
//! its `JobsQueryFilterKey` names ("Note that MediaConvert queries jobs using OR logic")
//! — and `GetJobsQueryResults` reports that status and, once complete, that page of jobs
//! and the cursor for the batch after it.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    MAX_JOBS_QUERIES, MAX_JOBS_QUERY_TERMS,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{arn, Emulator, Reply, ROLE};

/// A completion span short enough that a case can wait past it.
const SHORT_QUERY: Duration = Duration::from_millis(20);

/// Comfortably past [`SHORT_QUERY`], so a case that waits always observes the
/// completion, while keeping the suite's total real waiting small.
const PAST_THE_QUERY: Duration = Duration::from_millis(150);

/// Longer than any case takes to run, so a query stays `SUBMITTED` for the whole of the
/// case that is about the non-terminal status.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(3_600);

/// Pin how long after `StartJobsQuery` a query is due to complete.
///
/// The span is state, as the job settle span is, so no case depends on how fast the
/// machine is. Nothing on a request path writes it.
fn pin_query_span(emulator: &Emulator, span: Duration) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .jobs_query_completes_after = span;
}

/// A structurally real `JobSettings` with one input and one output, carrying the two
/// codecs a jobs query can filter on.
fn settings_with_codecs(file_input: &str, audio_codec: &str, video_codec: &str) -> Value {
    json!({
        "inputs": [{ "fileInput": file_input }],
        "outputGroups": [{
            "name": "File Group",
            "outputs": [{
                "nameModifier": "_out",
                "audioDescriptions": [{ "codecSettings": { "codec": audio_codec } }],
                "videoDescription": { "codecSettings": { "codec": video_codec } }
            }],
            "outputGroupSettings": {
                "type": "FILE_GROUP_SETTINGS",
                "fileGroupSettings": { "destination": "s3://example-bucket/out/" }
            }
        }]
    })
}

/// Start a jobs query and return its reply.
async fn start_query(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/2017-08-29/jobsQueries", Some(body))
        .await
}

/// Read a jobs query's results back.
async fn query_results(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send("GET", &format!("/2017-08-29/jobsQueries/{id}"), None)
        .await
}

/// The `id` of every job in a `GetJobsQueryResults` reply, in the order returned.
fn result_ids(reply: &Reply) -> Vec<String> {
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

/// Start a query, wait past its completion and return the results reply.
async fn run_query(emulator: &Emulator, body: Value) -> Reply {
    let started = start_query(emulator, body.clone()).await;
    assert_eq!(
        started.status, 201,
        "case {body}: StartJobsQuery must answer HTTP 201, got {} with body {}",
        started.status, started.body
    );
    let id = started.string("/id");
    assert!(
        !id.is_empty(),
        "case {body}: StartJobsQuery must answer an id, got {}",
        started.body
    );
    tokio::time::sleep(PAST_THE_QUERY).await;
    let results = query_results(emulator, &id).await;
    assert_eq!(
        results.status, 200,
        "case {body}: GetJobsQueryResults must answer HTTP 200, got {} with body {}",
        results.status, results.body
    );
    assert_eq!(
        results.string("/status"),
        "COMPLETE",
        "case {body}: a query past its due instant must be COMPLETE, got {}",
        results.body
    );
    results
}

/// The three jobs every filtering case queries over, and the labels this suite names
/// them by.
struct Fixture {
    /// `alpha`: queue `alpha`, an `.mp4` input, AAC audio, H_264 video, an engine
    /// version requested, left `SUBMITTED`.
    alpha: String,
    /// `beta`: queue `beta`, a `.mov` input, MP3 audio, H_265 video, no engine version,
    /// left `SUBMITTED`.
    beta: String,
    /// `gamma`: queue `alpha`, an `.mp4` input, AAC audio, MPEG2 video, CANCELED.
    gamma: String,
}

/// Three jobs differing in every job detail a jobs query can filter on, so one table can
/// span the whole `JobsQueryFilterKey` catalogue.
async fn seed_jobs(emulator: &Emulator) -> Fixture {
    for name in ["alpha", "beta"] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }

    let mut ids = Vec::new();
    for (queue, file_input, audio, video, engine_version) in [
        (
            "alpha",
            "s3://example-bucket/in/alpha-clip.mp4",
            "AAC",
            "H_264",
            Some("2024-09-25"),
        ),
        (
            "beta",
            "s3://example-bucket/in/beta-clip.mov",
            "MP3",
            "H_265",
            None,
        ),
        (
            "alpha",
            "s3://example-bucket/in/gamma-clip.mp4",
            "AAC",
            "MPEG2",
            None,
        ),
    ] {
        let mut body = json!({
            "role": ROLE,
            "queue": queue,
            "settings": settings_with_codecs(file_input, audio, video)
        });
        if let Some(version) = engine_version {
            body["jobEngineVersion"] = json!(version);
        }
        let created = emulator.create_job(body).await;
        assert_eq!(
            created.status, 201,
            "the fixture job in {queue:?} must be created: got {} with body {}",
            created.status, created.body
        );
        ids.push(created.string("/job/id"));
    }

    let gamma = ids.pop().unwrap_or_default();
    let beta = ids.pop().unwrap_or_default();
    let alpha = ids.pop().unwrap_or_default();

    let canceled = emulator.cancel_job(&gamma).await;
    assert_eq!(
        canceled.status, 202,
        "the fixture job must be cancelled: got {} with body {}",
        canceled.status, canceled.body
    );

    Fixture { alpha, beta, gamma }
}

/// A completed jobs query reports exactly the jobs its filter list matches, with a job
/// matching when ANY value of ANY filter matches the job detail that filter's key names.
///
/// The table spans the whole `JobsQueryFilterKey` catalogue — the eight values
/// `smithy_query(query="shape",shape="JobsQueryFilterKey")` lists — plus the forms a
/// filter list comes in: absent, one key with one value, one key with several values and
/// several keys at once.
#[tokio::test]
async fn a_jobs_query_matches_on_any_value_of_any_filter() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed_jobs(&emulator).await;
    let queue_arn = arn("queues/alpha");

    let cases: Vec<(&str, Value, Vec<&String>)> = vec![
        // No filter list narrows nothing, as a listing with no filter does not.
        ("no filter", json!({}), vec![&jobs.alpha, &jobs.beta, &jobs.gamma]),
        // `queue` — "Your Queue's name or ARN", so both forms resolve to the same queue.
        (
            "queue by name",
            json!({ "filterList": [{ "key": "queue", "values": ["alpha"] }] }),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        (
            "queue by ARN",
            json!({ "filterList": [{ "key": "queue", "values": [queue_arn] }] }),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        // Several values of one key are an OR over those values.
        (
            "queue, two values",
            json!({ "filterList": [{ "key": "queue", "values": ["alpha", "beta"] }] }),
            vec![&jobs.alpha, &jobs.beta, &jobs.gamma],
        ),
        // `status` — "Your job's status."
        (
            "status SUBMITTED",
            json!({ "filterList": [{ "key": "status", "values": ["SUBMITTED"] }] }),
            vec![&jobs.alpha, &jobs.beta],
        ),
        (
            "status CANCELED",
            json!({ "filterList": [{ "key": "status", "values": ["CANCELED"] }] }),
            vec![&jobs.gamma],
        ),
        (
            "status no job is in",
            json!({ "filterList": [{ "key": "status", "values": ["PROGRESSING"] }] }),
            vec![],
        ),
        // `fileInput` — "Your input file URL, or partial input file name", so a partial
        // name matches and a whole URL matches too.
        (
            "fileInput, partial name",
            json!({ "filterList": [{ "key": "fileInput", "values": ["beta-clip"] }] }),
            vec![&jobs.beta],
        ),
        (
            "fileInput, whole URL",
            json!({ "filterList": [{
                "key": "fileInput",
                "values": ["s3://example-bucket/in/alpha-clip.mp4"]
            }] }),
            vec![&jobs.alpha],
        ),
        (
            "fileInput, shared suffix",
            json!({ "filterList": [{ "key": "fileInput", "values": [".mp4"] }] }),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        // `jobEngineVersionRequested` — "The Job engine version that you requested."
        (
            "jobEngineVersionRequested",
            json!({ "filterList": [{
                "key": "jobEngineVersionRequested",
                "values": ["2024-09-25"]
            }] }),
            vec![&jobs.alpha],
        ),
        // `jobEngineVersionUsed` and `errorCode` name the two `Job` members no job in
        // this emulator carries: nothing transcoded a job, so no engine version was
        // used and no transcoder error code was produced.
        (
            "jobEngineVersionUsed",
            json!({ "filterList": [{
                "key": "jobEngineVersionUsed",
                "values": ["2024-09-25"]
            }] }),
            vec![],
        ),
        (
            "errorCode",
            json!({ "filterList": [{ "key": "errorCode", "values": ["1010"] }] }),
            vec![],
        ),
        // `audioCodec` / `videoCodec` — "Your output's audio codec" / "video codec".
        (
            "audioCodec",
            json!({ "filterList": [{ "key": "audioCodec", "values": ["AAC"] }] }),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        (
            "audioCodec no output uses",
            json!({ "filterList": [{ "key": "audioCodec", "values": ["FLAC"] }] }),
            vec![],
        ),
        (
            "videoCodec",
            json!({ "filterList": [{ "key": "videoCodec", "values": ["H_265"] }] }),
            vec![&jobs.beta],
        ),
        (
            "videoCodec, two values",
            json!({ "filterList": [{ "key": "videoCodec", "values": ["H_264", "MPEG2"] }] }),
            vec![&jobs.alpha, &jobs.gamma],
        ),
        // "Note that MediaConvert queries jobs using OR logic": two filters of different
        // keys match the UNION of what each matches, not the intersection.
        (
            "two keys at once",
            json!({ "filterList": [
                { "key": "videoCodec", "values": ["H_265"] },
                { "key": "queue", "values": ["alpha"] }
            ] }),
            vec![&jobs.alpha, &jobs.beta, &jobs.gamma],
        ),
        (
            "two keys, one matching nothing",
            json!({ "filterList": [
                { "key": "status", "values": ["PROGRESSING"] },
                { "key": "audioCodec", "values": ["MP3"] }
            ] }),
            vec![&jobs.beta],
        ),
    ];

    for (case, body, expected) in cases {
        let results = run_query(&emulator, body).await;
        let actual: BTreeSet<String> = result_ids(&results).into_iter().collect();
        let expected: BTreeSet<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the jobs {expected:?}, got {actual:?} from body {}",
            results.body
        );
    }
}

/// A query is asynchronous: a caller reading its results straight after starting it sees
/// the non-terminal `SUBMITTED` status and no jobs; once the clock makes the completion
/// due, the status is `COMPLETE` and the page is there.
#[tokio::test]
async fn a_query_reports_submitted_until_its_completion_comes_due() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, HELD_IN_SUBMITTED);
    let jobs = seed_jobs(&emulator).await;

    let started = start_query(&emulator, json!({})).await;
    assert_eq!(
        started.status, 201,
        "StartJobsQuery must answer HTTP 201, got {} with body {}",
        started.status, started.body
    );
    let held = started.string("/id");
    let pending = query_results(&emulator, &held).await;
    assert_eq!(
        pending.string("/status"),
        "SUBMITTED",
        "a caller reading results immediately must still see SUBMITTED, got {}",
        pending.body
    );
    assert!(
        pending.at("/jobs").is_null(),
        "a query that has not completed reports no jobs yet, got {}",
        pending.body
    );

    // Polling must not make the query complete: the transition is the clock's.
    let polled_again = query_results(&emulator, &held).await;
    assert_eq!(
        polled_again.string("/status"),
        "SUBMITTED",
        "polling must not complete a query, got {}",
        polled_again.body
    );

    // The same query, with a span the case can wait past.
    pin_query_span(&emulator, SHORT_QUERY);
    let results = run_query(&emulator, json!({})).await;
    let actual: BTreeSet<String> = result_ids(&results).into_iter().collect();
    let expected: BTreeSet<String> =
        [jobs.alpha, jobs.beta, jobs.gamma].into_iter().collect();
    assert_eq!(
        actual, expected,
        "a completed query reports every stored job: expected {expected:?}, got {actual:?}"
    );
}

/// The page a query matched is fixed when the query completes: polling it again reports
/// the same jobs, and a job created after the query completed is not in it.
#[tokio::test]
async fn a_completed_querys_results_do_not_change_under_the_caller() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed_jobs(&emulator).await;

    let started = start_query(&emulator, json!({})).await;
    let id = started.string("/id");
    tokio::time::sleep(PAST_THE_QUERY).await;

    let first = query_results(&emulator, &id).await;
    let before: Vec<String> = result_ids(&first);
    assert_eq!(
        before.len(),
        3,
        "the completed query must have matched the three stored jobs, got {}",
        first.body
    );

    let later = emulator
        .create_job(json!({
            "role": ROLE,
            "settings": settings_with_codecs("s3://example-bucket/in/late.mp4", "AAC", "H_264")
        }))
        .await;
    assert_eq!(
        later.status, 201,
        "the later job must be created: got {} with body {}",
        later.status, later.body
    );
    let late_id = later.string("/job/id");

    let second = query_results(&emulator, &id).await;
    let after: Vec<String> = result_ids(&second);
    assert_eq!(
        after, before,
        "case poll twice: a completed query's page must not change, got {after:?} after \
         {before:?}"
    );
    assert!(
        !after.contains(&late_id),
        "case job created after completion: {late_id:?} must not be in {after:?}"
    );

    // A query started now does match it, so the page is matched per query, not cached.
    let fresh = run_query(&emulator, json!({})).await;
    let fresh_ids: BTreeSet<String> = result_ids(&fresh).into_iter().collect();
    let expected: BTreeSet<String> = [jobs.alpha, jobs.beta, jobs.gamma, late_id]
        .into_iter()
        .collect();
    assert_eq!(
        fresh_ids, expected,
        "a query started after the job was created must match it: expected {expected:?}, \
         got {fresh_ids:?}"
    );
}

/// The matches are ordered newest-first unless `order` asks otherwise, and paged at
/// `maxResults` with a `nextToken` the caller hands to the next `StartJobsQuery` ("Use
/// this string to request the next batch of jobs via the StartJobsQuery API") to get the
/// batch after it, with nothing repeated or skipped.
#[tokio::test]
async fn the_matches_are_ordered_and_paged() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed_jobs(&emulator).await;
    let newest_first = vec![jobs.gamma.clone(), jobs.beta.clone(), jobs.alpha.clone()];
    let oldest_first = vec![jobs.alpha.clone(), jobs.beta.clone(), jobs.gamma.clone()];

    // "Default varies by resource": the jobs readers report the most recent first.
    for (case, order, expected) in [
        ("no order", None, &newest_first),
        ("DESCENDING", Some("DESCENDING"), &newest_first),
        ("ASCENDING", Some("ASCENDING"), &oldest_first),
    ] {
        let mut body = json!({});
        if let Some(order) = order {
            body["order"] = json!(order);
        }
        let results = run_query(&emulator, body).await;
        assert_eq!(
            &result_ids(&results),
            expected,
            "case {case:?}: expected {expected:?}, got {:?} from body {}",
            result_ids(&results),
            results.body
        );
        assert!(
            results.at("/nextToken").is_null(),
            "case {case:?}: one page holds every match, so no nextToken, got {}",
            results.body
        );
    }

    // Every page size from one match per page up to one page for all of them.
    for page_size in 1..=3usize {
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut body = json!({ "maxResults": page_size });
            if let Some(token) = cursor.as_deref() {
                body["nextToken"] = json!(token);
            }
            let results = run_query(&emulator, body).await;
            let page = result_ids(&results);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: a page holds at most {page_size} jobs, got \
                 {page:?}"
            );
            seen.extend(page);
            let token = results.at("/nextToken").as_str().map(str::to_string);
            match token {
                Some(token) => cursor = Some(token),
                None => break,
            }
        }
        assert_eq!(
            seen, newest_first,
            "case maxResults={page_size}: paging must walk every match once, in order, \
             got {seen:?}"
        );
    }
}

/// Nothing a caller sends grows the store, or the work one query does, without a ceiling:
/// a query matches on at most [`MAX_JOBS_QUERY_TERMS`] filter values and the store holds
/// at most [`MAX_JOBS_QUERIES`] queries, and past either cap the modelled refusal comes
/// instead of the growth.
///
/// The capacity row also pins the per-call bound on settling: the whole batch of queries
/// comes due inside ONE completion window, and the next request still answers with the
/// query's own page, because completing a due query is a status flip and nothing more.
#[tokio::test]
async fn a_jobs_query_cannot_grow_the_store_or_its_own_work_without_bound() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    let jobs = seed_jobs(&emulator).await;

    // The cap itself is accepted, and still matches: the last value names a real input.
    let mut at_the_cap: Vec<String> = (1..MAX_JOBS_QUERY_TERMS)
        .map(|index| format!("no-such-input-{index}"))
        .collect();
    at_the_cap.push("beta-clip".to_string());
    assert_eq!(
        at_the_cap.len(),
        MAX_JOBS_QUERY_TERMS,
        "case at the cap: the case must send exactly {MAX_JOBS_QUERY_TERMS} values"
    );
    let results = run_query(
        &emulator,
        json!({ "filterList": [{ "key": "fileInput", "values": at_the_cap.clone() }] }),
    )
    .await;
    assert_eq!(
        result_ids(&results),
        vec![jobs.beta.clone()],
        "case at the cap: expected the one job a value names, got {}",
        results.body
    );

    // One value past it is a problem in the request, not a bigger query.
    let mut past_the_cap = at_the_cap;
    past_the_cap.push("one-too-many".to_string());
    let refused = start_query(
        &emulator,
        json!({ "filterList": [{ "key": "fileInput", "values": past_the_cap }] }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case past the cap: expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("BadRequestException"),
        "case past the cap: expected BadRequestException, got {:?}",
        refused.error_type
    );

    // Fill the query store. Every one of these comes due inside one completion window.
    // One query is already stored: the at-the-cap row above (the refused one never was).
    let mut first = String::new();
    for index in 1..MAX_JOBS_QUERIES {
        let started = start_query(&emulator, json!({})).await;
        assert_eq!(
            started.status, 201,
            "case query #{index} of {MAX_JOBS_QUERIES}: expected HTTP 201, got {} with \
             body {}",
            started.status, started.body
        );
        if first.is_empty() {
            first = started.string("/id");
        }
    }
    for case in ["one-too-many", "and-another"] {
        let refused = start_query(&emulator, json!({})).await;
        assert_eq!(
            refused.status, 402,
            "case {case:?}: expected HTTP 402 at capacity, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case:?}: expected ServiceQuotaExceededException, got {:?}",
            refused.error_type
        );
    }

    // A query already stored still reads back at capacity, with the page it matched.
    tokio::time::sleep(PAST_THE_QUERY).await;
    let stored = query_results(&emulator, &first).await;
    assert_eq!(
        stored.status, 200,
        "case read at capacity: expected HTTP 200, got {} with body {}",
        stored.status, stored.body
    );
    assert_eq!(
        stored.string("/status"),
        "COMPLETE",
        "case read at capacity: the query must have completed, got {}",
        stored.body
    );
    let matched: BTreeSet<String> = result_ids(&stored).into_iter().collect();
    let expected: BTreeSet<String> = [jobs.alpha, jobs.beta, jobs.gamma].into_iter().collect();
    assert_eq!(
        matched, expected,
        "case read at capacity: expected {expected:?}, got {matched:?}"
    );
}

/// The refusals: an id no query is stored under, a cursor this service never issued, and
/// a filter that names no key.
#[tokio::test]
async fn the_modelled_refusals() {
    let emulator = Emulator::new();
    pin_query_span(&emulator, SHORT_QUERY);
    seed_jobs(&emulator).await;

    let missing = query_results(&emulator, "no-such-query").await;
    assert_eq!(
        missing.status, 404,
        "case unknown id: expected HTTP 404, got {} with body {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_type.as_deref(),
        Some("NotFoundException"),
        "case unknown id: expected NotFoundException, got {:?}",
        missing.error_type
    );

    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex of nothing this service issued", "6e6f70653a6e6f7065"),
    ] {
        let refused = start_query(&emulator, json!({ "nextToken": token })).await;
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

    let keyless = start_query(
        &emulator,
        json!({ "filterList": [{ "values": ["alpha"] }] }),
    )
    .await;
    assert_eq!(
        keyless.status, 400,
        "case filter with no key: expected HTTP 400, got {} with body {}",
        keyless.status, keyless.body
    );
    assert_eq!(
        keyless.error_type.as_deref(),
        Some("BadRequestException"),
        "case filter with no key: expected BadRequestException, got {:?}",
        keyless.error_type
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of WHICH FORMS of a queue reference a jobs reader's `queue`
//! filter resolves.
//!
//! The rule every case below is one row of: a reader resolves a caller-sent queue
//! reference in exactly the forms its own modelled member offers, and in no others. The
//! model states a different set for each of the two paged readers —
//! `smithy_query(query="doc",shape="ListJobsRequest",member="Queue")` is "Optional.
//! Provide a queue name to get back only jobs from that queue" while
//! `smithy_query(query="doc",shape="SearchJobsRequest",member="Queue")` is "Optional.
//! Provide a queue name, or a queue ARN, to return only jobs from that queue" — so
//! `ListJobs` selects on the stored queue name byte for byte, and `SearchJobs` selects on
//! the queue a name OR an ARN names. (The third reader, a jobs query's `queue` filter key,
//! is documented "Your Queue's name or ARN" and is pinned in `tests/all/jobs_query.rs`.)
//!
//! A queue's name is stored exactly as its creator wrote it
//! (`tests/all/queue_name_identity.rs`), so the distinction is observable in both
//! directions: a value in a form the member does not offer narrows to the queue literally
//! named by that whole value, and a queue whose own name carries an ARN's resource-id
//! separator — or is an ARN — is still reachable through the reader that does not reduce.
//!
//! Every case drives the real restJson1 wire path through the crate's one test harness, so
//! what is asserted is the bytes a client would see, never an internal call.

use std::collections::BTreeSet;

use serde_json::json;

use super::job_lifecycle::{queue_arn, settings, Emulator, Reply, ROLE};
use super::job_search::job_ids;
use super::resource_tagging::encode_label;

/// The two readers whose `queue` filter this suite drives, by the path each is bound to.
const LIST_JOBS: &str = "/2017-08-29/jobs";
const SEARCH_JOBS: &str = "/2017-08-29/search";

/// One read of `path`, narrowed by the `queue` value `reference`.
///
/// The value goes on the wire percent-encoded through the test binary's one encoding rule
/// ([`encode_label`]), so a row may carry any byte — a space, an ARN's `:` and `/`, a
/// character the query string reserves — and the handler still reads the value the row
/// states.
async fn filtered_by_queue(emulator: &Emulator, path: &str, reference: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("{path}?queue={}", encode_label(reference)),
            None,
        )
        .await
}

/// A queue whose name carries the `:queues/` separator a MediaConvert queue ARN puts
/// before its resource name, WITHOUT being an ARN — so no reader may reduce it.
const ARN_LIKE_QUEUE: &str = "legacy:queues/alpha";

/// The jobs every case reads over, each on its own queue.
struct Fixture {
    /// On the queue `alpha`.
    alpha: String,
    /// A second job on the queue `alpha`.
    alpha_again: String,
    /// On the queue `beta`.
    beta: String,
    /// On the account's built-in queue.
    default_queue: String,
    /// On the queue [`ARN_LIKE_QUEUE`].
    arn_like: String,
}

impl Fixture {
    /// Every job, as the set a read with no narrowing filter reports.
    fn every_job(&self) -> BTreeSet<String> {
        BTreeSet::from([
            self.alpha.clone(),
            self.alpha_again.clone(),
            self.beta.clone(),
            self.default_queue.clone(),
            self.arn_like.clone(),
        ])
    }
}

/// Create the queues and the jobs, and answer the job ids by the label this suite names
/// them with.
async fn seed(emulator: &Emulator) -> Fixture {
    for name in ["alpha", "beta", ARN_LIKE_QUEUE] {
        let created = emulator.create_queue(json!({ "name": name })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {name:?} must be created: got {} with body {}",
            created.status, created.body
        );
    }

    let mut ids = Vec::new();
    for (queue, modifier) in [
        ("alpha", "_alpha"),
        ("alpha", "_alpha_again"),
        ("beta", "_beta"),
        ("Default", "_default"),
        (ARN_LIKE_QUEUE, "_arn_like"),
    ] {
        let created = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings("s3://example-bucket/in/a.mp4", modifier)
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "the fixture job {modifier:?} on {queue:?} must be created: got {} with body {}",
            created.status, created.body
        );
        ids.push(created.string("/job/id"));
    }

    let arn_like = ids.pop().unwrap_or_default();
    let default_queue = ids.pop().unwrap_or_default();
    let beta = ids.pop().unwrap_or_default();
    let alpha_again = ids.pop().unwrap_or_default();
    let alpha = ids.pop().unwrap_or_default();

    Fixture {
        alpha,
        alpha_again,
        beta,
        default_queue,
        arn_like,
    }
}

/// Each reader narrows by the forms of queue reference its own member offers: `ListJobs`
/// by a queue NAME alone, `SearchJobs` by a name or an ARN.
///
/// One table of reference forms, driven through both readers with the set each is to
/// report: a plain name, a name padded with whitespace (a different name), the built-in
/// queue's name, a name no queue holds, the ARN of a queue that holds jobs, the ARN of the
/// built-in queue, the ARN of a queue nobody created, the bare `queues/<name>` resource id
/// of an ARN, and the name of a queue that carries an ARN's resource-id separator without
/// being an ARN.
#[tokio::test]
async fn each_jobs_reader_narrows_by_the_reference_forms_its_member_offers() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;

    let on_alpha = BTreeSet::from([jobs.alpha.clone(), jobs.alpha_again.clone()]);
    let on_beta = BTreeSet::from([jobs.beta.clone()]);
    let on_default = BTreeSet::from([jobs.default_queue.clone()]);
    let on_arn_like = BTreeSet::from([jobs.arn_like.clone()]);
    let none: BTreeSet<String> = BTreeSet::new();

    // (case, the `queue` value, what `ListJobs` reports, what `SearchJobs` reports)
    let cases: Vec<(&str, String, &BTreeSet<String>, &BTreeSet<String>)> = vec![
        ("a queue name", "alpha".to_string(), &on_alpha, &on_alpha),
        ("another queue name", "beta".to_string(), &on_beta, &on_beta),
        (
            "the built-in queue's name",
            "Default".to_string(),
            &on_default,
            &on_default,
        ),
        (
            "a name no queue holds",
            "no-such-queue".to_string(),
            &none,
            &none,
        ),
        (
            "a name padded with whitespace, which no queue holds",
            "alpha ".to_string(),
            &none,
            &none,
        ),
        // The crux: only the member documented to offer the ARN form resolves one.
        (
            "the ARN of a queue that holds jobs",
            queue_arn("alpha"),
            &none,
            &on_alpha,
        ),
        (
            "the ARN of the built-in queue",
            queue_arn("Default"),
            &none,
            &on_default,
        ),
        (
            "the ARN of a queue nobody created",
            queue_arn("no-such-queue"),
            &none,
            &none,
        ),
        // A resource id on its own is not an ARN, so neither reader reduces it.
        (
            "the resource id of a queue's ARN, without the ARN",
            "queues/alpha".to_string(),
            &none,
            &none,
        ),
        // Nor is a value that merely carries the separator an ARN puts before the
        // resource name: it is the whole name of a queue, for both readers.
        (
            "a queue name carrying an ARN's resource-id separator",
            ARN_LIKE_QUEUE.to_string(),
            &on_arn_like,
            &on_arn_like,
        ),
    ];

    for (case, reference, listed, searched) in cases {
        for (path, expected) in [(LIST_JOBS, listed), (SEARCH_JOBS, searched)] {
            let case = format!("{path} narrowed by {case} ({reference:?})");
            let reply = filtered_by_queue(&emulator, path, &reference).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let reported: BTreeSet<String> = job_ids(&reply).into_iter().collect();
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected the jobs {expected:?}, got {reported:?} from body {}",
                reply.body
            );
        }
    }

    // A narrowed read is still a report: every job is where it was afterwards.
    let unfiltered = emulator.send("GET", LIST_JOBS, None).await;
    let reported: BTreeSet<String> = job_ids(&unfiltered).into_iter().collect();
    assert_eq!(
        reported,
        jobs.every_job(),
        "expected the stored jobs {:?} after the narrowed reads, got {reported:?} from body {}",
        jobs.every_job(),
        unfiltered.body
    );
}

/// The consequence of the same rule at its sharpest: a queue whose own NAME is an ARN.
///
/// The name is stored byte for byte, so `ListJobs` — which takes a queue name — must read
/// that value as that queue's name and report that queue's jobs (none here), never the
/// jobs of the queue the ARN's resource id spells, which is what reducing the value would
/// report.
#[tokio::test]
async fn a_queue_whose_name_is_an_arn_is_its_own_name_to_the_name_only_reader() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;
    // A queue literally NAMED the ARN of the queue `alpha`, beside `alpha` itself.
    let impostor = queue_arn("alpha");
    let created = emulator.create_queue(json!({ "name": impostor })).await;
    assert_eq!(
        created.status, 201,
        "the ARN-named fixture queue must be created: got {} with body {}",
        created.status, created.body
    );
    assert_eq!(
        created.string("/queue/name"),
        impostor,
        "the queue's name must be stored byte for byte, got {}",
        created.body
    );

    let listed = emulator.send("GET", "/2017-08-29/queues?maxResults=20", None).await;
    let names: BTreeSet<String> = listed
        .at("/queues")
        .as_array()
        .map(|queues| {
            queues
                .iter()
                .map(|queue| queue["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&impostor),
        "case {impostor:?}: the stored queues must hold the ARN-named one, got {names:?} from \
         body {}",
        listed.body
    );

    // `ListJobs` reads the value as the name it is: that queue holds no job.
    let by_name = filtered_by_queue(&emulator, LIST_JOBS, &impostor).await;
    let reported: BTreeSet<String> = job_ids(&by_name).into_iter().collect();
    assert_eq!(
        reported,
        BTreeSet::new(),
        "case {impostor:?}: ListJobs must report the jobs of the queue NAMED by the value, of \
         which there are none, got {reported:?} from body {}",
        by_name.body
    );
    assert!(
        !reported.contains(&jobs.alpha),
        "case {impostor:?}: ListJobs must not report the jobs of the queue the ARN's resource \
         id spells, got {reported:?} from body {}",
        by_name.body
    );
}

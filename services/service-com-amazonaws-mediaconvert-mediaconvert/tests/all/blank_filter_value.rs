// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the reporting half of the rule that a member carrying a
//! resource identity or a text to match on must carry at least one character.
//!
//! `tests/all/blank_resource_name.rs` pins it for the doors that MINT or RESOLVE such a
//! member; this suite pins it for the readers that FILTER on one. Every filter member a
//! jobs reader takes is the model's bare `__string`
//! (`smithy_query(query="operation",operation="SearchJobs,ListJobs",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`),
//! so the framework's boundary validation passes the zero-length value and one made only
//! of whitespace alike.
//!
//! The rule under test: a filter member the request CARRIES has to name something to
//! narrow by, and a value made entirely of whitespace names nothing — no queue the account
//! could hold, no status, and no text an input file could be matched against — so each
//! reader answers it with `BadRequestException` (400) naming the member, rather than
//! resolving the value into a comparison and reporting the page that comes out (an empty
//! one for an equality test, every record for a containment one). Leaving the member out
//! is a different request: it asks for no narrowing at all.
//!
//! Its boundary is the other half of the same rule: a value that carries at least one
//! character is used verbatim, surrounding whitespace included, so it narrows — possibly
//! to nothing — rather than being refused.
//!
//! The blank forms and the refusal sentence are the ones `blank_resource_name` drives its
//! own rows through, and the page a reply reports is read with `job_search`'s projection, so
//! the halves of the rule cannot disagree about which values are blank or how a refusal
//! reads. Every case drives the real restJson1 wire path through the crate's one test
//! harness, so what is asserted is the bytes a client would see.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::blank_resource_name::{blanks, refusal_message, Blank};
use super::job_lifecycle::{settings, Emulator, ROLE};
use super::job_search::job_ids;

/// The request one door carries a blank value in.
struct Request {
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// One reader that filters on a caller-sent member: how it is asked, and the member its
/// refusal names.
struct Door {
    label: &'static str,
    /// The member the refusal names, spelled as the model spells it.
    member: &'static str,
    /// The request that sends this blank form to that member — through the query string
    /// for the two paged readers (so the percent-encoded form), and through the body for
    /// the jobs query's filter list.
    request: fn(&Blank) -> Request,
}

/// A `GET` whose query string carries the blank form.
fn query_door(uri: String) -> Request {
    Request {
        method: "GET",
        uri,
        body: None,
    }
}

/// A `StartJobsQuery` whose filter list carries `values` under `key`.
fn jobs_query_door(body: Value) -> Request {
    Request {
        method: "POST",
        uri: "/2017-08-29/jobsQueries".to_string(),
        body: Some(body),
    }
}

/// Every reader that resolves a caller-sent filter member, and every such member it takes:
/// the two documented free-text filters of `SearchJobs`, the one `ListJobs` takes, and the
/// values of a jobs query's filter list — at each key whose job detail is compared by
/// equality and the one compared by containment, alone and beside a value that carries
/// characters.
fn doors() -> Vec<Door> {
    vec![
        Door {
            label: "SearchJobs, queue",
            member: "Queue",
            request: |blank| query_door(format!("/2017-08-29/search?queue={}", blank.path)),
        },
        Door {
            label: "SearchJobs, queue beside a filter that carries characters",
            member: "Queue",
            request: |blank| {
                query_door(format!(
                    "/2017-08-29/search?status=SUBMITTED&queue={}",
                    blank.path
                ))
            },
        },
        Door {
            label: "SearchJobs, inputFile",
            member: "InputFile",
            request: |blank| query_door(format!("/2017-08-29/search?inputFile={}", blank.path)),
        },
        Door {
            label: "SearchJobs, inputFile beside a filter that carries characters",
            member: "InputFile",
            request: |blank| {
                query_door(format!(
                    "/2017-08-29/search?queue=queue-a&inputFile={}",
                    blank.path
                ))
            },
        },
        Door {
            label: "ListJobs, queue",
            member: "Queue",
            request: |blank| query_door(format!("/2017-08-29/jobs?queue={}", blank.path)),
        },
        Door {
            label: "ListJobs, queue beside a filter that carries characters",
            member: "Queue",
            request: |blank| {
                query_door(format!(
                    "/2017-08-29/jobs?status=SUBMITTED&queue={}",
                    blank.path
                ))
            },
        },
        Door {
            label: "StartJobsQuery, a filter value under the queue key",
            member: "FilterList.Values",
            request: |blank| {
                jobs_query_door(json!({
                    "filterList": [{ "key": "queue", "values": [blank.value] }]
                }))
            },
        },
        Door {
            label: "StartJobsQuery, a filter value under the fileInput key",
            member: "FilterList.Values",
            request: |blank| {
                jobs_query_door(json!({
                    "filterList": [{ "key": "fileInput", "values": [blank.value] }]
                }))
            },
        },
        Door {
            label: "StartJobsQuery, a filter value under the status key",
            member: "FilterList.Values",
            request: |blank| {
                jobs_query_door(json!({
                    "filterList": [{ "key": "status", "values": [blank.value] }]
                }))
            },
        },
        Door {
            label: "StartJobsQuery, a filter value beside one that carries characters",
            member: "FilterList.Values",
            request: |blank| {
                jobs_query_door(json!({
                    "filterList": [{ "key": "queue", "values": ["queue-a", blank.value] }]
                }))
            },
        },
        Door {
            label: "StartJobsQuery, a filter value beside another filter",
            member: "FilterList.Values",
            request: |blank| {
                jobs_query_door(json!({
                    "filterList": [
                        { "key": "status", "values": ["SUBMITTED"] },
                        { "key": "fileInput", "values": [blank.value] }
                    ]
                }))
            },
        },
    ]
}

/// The two jobs every case below reads over: one on a queue named plainly, one on a queue
/// whose name is padded with the very whitespace a blank value is made of — so a case can
/// tell a value that carries characters from one that does not.
struct Fixture {
    /// queue `queue-a`, input `s3://example-bucket/in/alpha-clip.mp4`.
    alpha: String,
    /// queue `" padded "`, input `s3://example-bucket/in/padded clip.mp4`.
    padded: String,
}

impl Fixture {
    /// Both jobs, as the set a search with no narrowing filter reports.
    fn every_job(&self) -> BTreeSet<String> {
        BTreeSet::from([self.alpha.clone(), self.padded.clone()])
    }
}

/// Seed the two jobs, each on its own queue.
async fn seed(emulator: &Emulator) -> Fixture {
    let mut ids = Vec::new();
    for (queue, file_input, modifier) in [
        ("queue-a", "s3://example-bucket/in/alpha-clip.mp4", "_alpha"),
        (" padded ", "s3://example-bucket/in/padded clip.mp4", "_padded"),
    ] {
        let created = emulator.create_queue(json!({ "name": queue })).await;
        assert_eq!(
            created.status, 201,
            "the fixture queue {queue:?} must be created: got {} with body {}",
            created.status, created.body
        );
        let job = emulator
            .create_job(json!({
                "role": ROLE,
                "queue": queue,
                "settings": settings(file_input, modifier)
            }))
            .await;
        assert_eq!(
            job.status, 201,
            "the fixture job {modifier:?} must be created: got {} with body {}",
            job.status, job.body
        );
        ids.push(job.string("/job/id"));
    }

    let padded = ids.pop().unwrap_or_default();
    let alpha = ids.pop().unwrap_or_default();
    Fixture { alpha, padded }
}

/// Every reader that filters on a caller-sent member refuses a value carrying no
/// characters, naming the member — and reports nothing and starts nothing for it.
#[tokio::test]
async fn a_filter_value_carrying_no_characters_is_refused_by_every_jobs_reader() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;

    for door in doors() {
        for blank in blanks() {
            let case = format!("{} of {}", door.label, blank.label);
            let request = (door.request)(&blank);
            let refused = emulator
                .send(request.method, &request.uri, request.body)
                .await;

            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {:?} with body {}",
                refused.error_type,
                refused.body
            );
            assert_eq!(
                refused.string("/message"),
                refusal_message(door.member),
                "case {case:?}: expected the refusal to name {}, got body {}",
                door.member,
                refused.body
            );
            assert!(
                refused.at("/jobs").is_null() && refused.at("/id").is_null(),
                "case {case:?}: a refused read reports no page and no query, got body {}",
                refused.body
            );
        }
    }

    // A refusal leaves the store exactly as it was: every job is still reported, and no
    // jobs query was started by the refused ones.
    for uri in ["/2017-08-29/search", "/2017-08-29/jobs"] {
        let reply = emulator.send("GET", uri, None).await;
        let reported: BTreeSet<String> = job_ids(&reply).into_iter().collect();
        assert_eq!(
            reported,
            jobs.every_job(),
            "case {uri:?}: expected the stored jobs {:?} after the refusals, got \
             {reported:?} from body {}",
            jobs.every_job(),
            reply.body
        );
    }
    let started = emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .jobs_queries
        .len();
    assert_eq!(
        started, 0,
        "a refused StartJobsQuery must persist no query, got {started} of them"
    );
}

/// The boundary of the same rule: a filter value that carries at least one character is
/// taken verbatim — whitespace around it is part of the value, so it narrows, and narrows
/// to nothing when nothing stored holds it.
#[tokio::test]
async fn a_filter_value_that_carries_characters_narrows_verbatim() {
    let emulator = Emulator::new();
    let jobs = seed(&emulator).await;
    let alpha = BTreeSet::from([jobs.alpha.clone()]);
    let padded = BTreeSet::from([jobs.padded.clone()]);
    let none = BTreeSet::new();

    // The `queue` filter, which both paged readers take: a name, that name padded (a
    // different name, which only the padded queue holds), and a padded form no queue holds.
    for uri in ["/2017-08-29/search", "/2017-08-29/jobs"] {
        for (label, query, expected) in [
            ("a plain name", "queue=queue-a", &alpha),
            ("a padded name a queue holds", "queue=%20padded%20", &padded),
            ("a padded name no queue holds", "queue=%20queue-a", &none),
            ("one character no queue is named", "queue=.", &none),
        ] {
            let case = format!("{uri} with {label}");
            let reply = emulator.send("GET", &format!("{uri}?{query}"), None).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let reported: BTreeSet<String> = job_ids(&reply).into_iter().collect();
            assert_eq!(
                &reported, expected,
                "case {case:?}: expected {expected:?}, got {reported:?} from body {}",
                reply.body
            );
        }
    }

    // `SearchJobs`'s `InputFile` is matched by containment, so one character of an input
    // narrows to the inputs that carry it — including a character a blank value is made of,
    // which one input's name carries in the middle.
    for (label, query, expected) in [
        ("a whole URL", "inputFile=s3://example-bucket/in/alpha-clip.mp4", &alpha),
        ("a partial base name", "inputFile=alpha-clip", &alpha),
        ("a space an input name carries", "inputFile=%20clip", &padded),
        ("an extension both inputs carry", "inputFile=.mp4", &jobs.every_job()),
        ("text no input carries", "inputFile=no-such-input", &none),
    ] {
        let case = format!("SearchJobs with {label}");
        let reply = emulator
            .send("GET", &format!("/2017-08-29/search?{query}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        let reported: BTreeSet<String> = job_ids(&reply).into_iter().collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from body {}",
            reply.body
        );
    }

    // The jobs query's filter values take the same boundary: a value carrying characters is
    // accepted, padding included.
    for (label, value) in [
        ("a plain name", "queue-a"),
        ("a padded name a queue holds", " padded "),
        ("a padded name no queue holds", " queue-a"),
    ] {
        let case = format!("StartJobsQuery with {label}");
        let started = emulator
            .send(
                "POST",
                "/2017-08-29/jobsQueries",
                Some(json!({
                    "filterList": [{ "key": "queue", "values": [value] }]
                })),
            )
            .await;
        assert_eq!(
            started.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            started.status, started.body
        );
        assert!(
            !started.string("/id").is_empty(),
            "case {case:?}: an accepted query must answer its id, got body {}",
            started.body
        );
    }
}

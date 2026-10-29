// SPDX-License-Identifier: CC0-1.0

//! What the rest of this service can do WHILE one request reads a profile document, end
//! to end over HTTP through the router this service mounts in production.
//!
//! The rule every case here asserts: reading a profile document is work whose size and
//! shape the CALLER chose, so no request does it while holding the service's store lock
//! or the async worker serving it — the record it needs is resolved under the lock, the
//! lock and the worker are handed back, and the document is read elsewhere. Every other
//! operation of the service is therefore served while that read is in flight, whatever
//! the submitted or stored payload holds, and the answers are the same as ever.
//!
//! Each case probes that directly, with no reliance on how long a read takes: the
//! request's future is polled ONCE and must not have finished (a request that reads the
//! document on this worker finishes in that single poll, since nothing else in this
//! service's request path awaits), and then — while that future is parked, so the read is
//! genuinely still in flight — a describe and a list are driven to completion on the same
//! single-worker runtime, which they can only reach by taking the store lock. The parked
//! request is finally driven to its answer, which must be the one it has always given.
//!
//! Rows vary what the rule generalises over: the operation that reads a document
//! (`PostAgentProfile`, which reads the bytes a caller SUBMITS, and the two readers of a
//! STORED document, `GetProfile` and `BatchGetFrameMetricData`), the representation
//! (JSON and Amazon Ion's binary encoding), the payload's shape and size (a wide struct,
//! a long list, bytes that are no document at all) and the answer the read ends in
//! (stored, refused, served).
//!
//! The harness is the one [`crate::profiling_group_lifecycle`] already builds.

use std::future::{Future, poll_fn};
use std::task::Poll;

use crate::profiling_group_lifecycle::{Answer, Harness, PROFILE_START_MILLIS};

/// The two media types the model's `PostAgentProfile$contentType` doc names:
/// "application/json — standard JSON format" and "application/x-amzn-ion — the Amazon Ion
/// data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// A media type that names NEITHER representation, which the data a caller submits is
/// judged on all the same.
const TEXT: &str = "text/plain";

/// How many frames (or elements) a caller-sized payload in these cases carries. Large
/// enough that reading it is real work, small enough to stay well inside the store's
/// per-group retention byte cap.
const ELEMENTS: usize = 20_000;

/// Half an hour in seconds — each side of the window the frame-metric case reports over,
/// so that window is one hour, well inside the one day that operation allows.
const HALF_HOUR_SECONDS: i64 = 30 * 60;

/// A profile document carrying `frames` frames beside its `start`: a WIDE document, so
/// reading it is work that grows with what the caller sent.
fn wide_profile(frames: usize) -> Vec<u8> {
    let mut callgraph = serde_json::Map::new();
    for index in 0..frames {
        callgraph.insert(
            format!("Frame{index}"),
            serde_json::json!({ "RUNNABLE": 1, "BLOCKED": 0 }),
        );
    }
    serde_json::to_vec(&serde_json::json!({
        "start": PROFILE_START_MILLIS,
        "callgraph": callgraph,
    }))
    .expect("a serializable document")
}

/// The same size of payload in Amazon Ion's binary encoding: a profile stating its start
/// and carrying a long list beside it.
fn wide_ion_profile(elements: usize) -> Vec<u8> {
    let samples: Vec<ion_rs::Element> = (0..elements)
        .map(|index| ion_rs::Element::from(index as i64))
        .collect();
    ion_rs::Element::from(ion_rs::Struct::from_iter([
        (
            ion_rs::Symbol::from("start"),
            ion_rs::Element::from(PROFILE_START_MILLIS),
        ),
        (
            ion_rs::Symbol::from("samples"),
            ion_rs::Element::from(ion_rs::Value::List(ion_rs::Sequence::new(samples))),
        ),
    ]))
    .encode_as(ion_rs::v1_0::Binary)
    .expect("an encodable Ion document")
}

/// A long JSON list of profile-like structs: a document whose ROOT is a list, so it is no
/// agent profile however much it holds — and the whole of it is read before that is said.
fn list_of_profiles(elements: usize) -> Vec<u8> {
    let items: Vec<serde_json::Value> = (0..elements)
        .map(|index| serde_json::json!({ "start": PROFILE_START_MILLIS, "index": index }))
        .collect();
    serde_json::to_vec(&serde_json::Value::Array(items)).expect("a serializable document")
}

/// A long JSON list with its closing bracket missing: bytes that are no document at all,
/// which is said only after reading through them.
fn truncated_list(elements: usize) -> Vec<u8> {
    let mut bytes = b"[".to_vec();
    for index in 0..elements {
        bytes.extend_from_slice(format!("{index},").as_bytes());
    }
    bytes
}

/// Which request reads the document in one case.
enum Read {
    /// `PostAgentProfile` of these bytes, announced under this media type: the operation
    /// that reads the bytes a caller SUBMITS.
    Submit {
        content_type: &'static str,
        bytes: Vec<u8>,
    },
    /// `GetProfile` of the document the group was seeded with.
    Serve,
    /// `BatchGetFrameMetricData` over the document the group was seeded with.
    Tally,
}

/// One case: the profile the group holds before the request (for the readers of a stored
/// document), the request that reads a document, and the status it must answer with.
struct Case {
    label: &'static str,
    seeded: Option<Vec<u8>>,
    read: Read,
    status: u16,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a wide JSON profile is submitted",
            seeded: None,
            read: Read::Submit {
                content_type: JSON,
                bytes: wide_profile(ELEMENTS),
            },
            status: 204,
        },
        Case {
            label: "a wide profile in Ion's binary encoding is submitted",
            seeded: None,
            read: Read::Submit {
                content_type: ION,
                bytes: wide_ion_profile(ELEMENTS),
            },
            status: 204,
        },
        Case {
            label: "a long list of profiles rather than one is submitted",
            seeded: None,
            read: Read::Submit {
                content_type: JSON,
                bytes: list_of_profiles(ELEMENTS),
            },
            status: 400,
        },
        Case {
            label: "bytes that are no document at all are submitted",
            seeded: None,
            read: Read::Submit {
                content_type: JSON,
                bytes: truncated_list(ELEMENTS),
            },
            status: 400,
        },
        Case {
            // A profile announced under a media type that names NEITHER representation:
            // what the bytes hold decides, so this one is read too.
            label: "a profile is submitted under a media type that names no representation",
            seeded: None,
            read: Read::Submit {
                content_type: TEXT,
                bytes: wide_profile(ELEMENTS / 2),
            },
            status: 204,
        },
        Case {
            label: "a wide stored profile is served",
            seeded: Some(wide_profile(ELEMENTS)),
            read: Read::Serve,
            status: 200,
        },
        Case {
            label: "a frame metric is tallied over a wide stored profile",
            seeded: Some(wide_profile(ELEMENTS)),
            read: Read::Tally,
            status: 200,
        },
    ]
}

async fn create_group(harness: &Harness, name: &str) -> Answer {
    harness
        .send(
            "POST",
            &format!("/profilingGroups?clientToken=tok-{name}"),
            JSON,
            serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                .expect("a serializable body"),
        )
        .await
}

/// `at` (epoch seconds) in the ISO 8601 `date-time` form the modelled query members of
/// this service carry.
fn iso(at: i64) -> String {
    aws_smithy_types::DateTime::from_secs(at)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a formattable instant")
}

/// A `BatchGetFrameMetricData` window around the clock's current instant — one hour, well
/// inside the one day that operation reports over — so the receipt of a profile posted
/// through this harness falls in it.
fn metrics_window() -> String {
    let now = aws_smithy_types::DateTime::from(std::time::SystemTime::now()).secs();
    format!(
        "?startTime={}&endTime={}",
        iso(now - HALF_HOUR_SECONDS),
        iso(now + HALF_HOUR_SECONDS)
    )
}

/// The request one case makes, as a future that has not been polled yet.
async fn issue(harness: &Harness, name: &str, read: &Read) -> Answer {
    match read {
        Read::Submit {
            content_type,
            bytes,
        } => {
            harness
                .post_agent_profile(name, content_type, None, bytes.clone())
                .await
        }
        Read::Serve => harness.get_profile(name, "").await,
        Read::Tally => {
            let metrics = vec![crate::frame_metric_data::frame_metric(
                "Frame0",
                &["RUNNABLE"],
            )];
            crate::frame_metric_data::frame_metric_data(
                harness,
                name,
                &metrics_window(),
                &metrics,
            )
            .await
        }
    }
}

#[tokio::test]
async fn no_document_read_holds_the_store_lock_or_the_async_worker() {
    // The default `#[tokio::test]` runtime has ONE async worker, which is what makes the
    // probe below an observation rather than a race: a request that reads the document on
    // that worker cannot hand it back, so nothing else can be served until it is done.
    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let harness = Harness::new();
        let name = format!("concurrent-{index}");
        assert_eq!(
            create_group(&harness, &name).await.status,
            201,
            "case {label:?}: create must succeed"
        );
        if let Some(seeded) = &case.seeded {
            let posted = harness
                .post_agent_profile(&name, JSON, None, seeded.clone())
                .await;
            assert_eq!(
                posted.status,
                204,
                "case {label:?}: seeding the group must succeed, got {} {}",
                posted.status,
                String::from_utf8_lossy(&posted.body)
            );
            harness.complete_aggregation(&name);
        }

        // ONE poll of the request that reads a document. It must not be finished: the
        // read is elsewhere by now, and the worker is back.
        let mut reading = std::pin::pin!(issue(&harness, &name, &case.read));
        let finished = poll_fn(|cx| Poll::Ready(reading.as_mut().poll(cx).is_ready())).await;
        assert!(
            !finished,
            "case {label:?}: expected the request to hand the async worker back before it \
             finished reading the document, got an answer within its first poll"
        );

        // The read is in flight — the future above is parked, so nothing is polling it —
        // and the rest of the service is served, which takes the store lock.
        let described = harness.describe(&name).await;
        assert_eq!(
            described.status,
            200,
            "case {label:?}: expected a describe to be served while the document is read, \
             got {} {}",
            described.status,
            String::from_utf8_lossy(&described.body)
        );
        assert_eq!(
            described
                .json()
                .get("name")
                .and_then(|value| value.as_str()),
            Some(name.as_str()),
            "case {label:?}: expected the described group to be {name:?}, got {}",
            String::from_utf8_lossy(&described.body)
        );
        let listed = harness.send("GET", "/profilingGroups", JSON, Vec::new()).await;
        assert_eq!(
            listed.status,
            200,
            "case {label:?}: expected a list to be served while the document is read, got \
             {} {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );

        // And the request that was reading answers exactly what it has always answered.
        let answer = reading.await;
        assert_eq!(
            answer.status,
            case.status,
            "case {label:?}: expected HTTP {}, got {} {}",
            case.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        match (&case.read, case.status) {
            // A submission that was accepted is STORED: the aggregated profile reads back
            // the submitted bytes, so the read off the lock wrote what it read.
            (Read::Submit { bytes, .. }, 204) => {
                harness.complete_aggregation(&name);
                let served = harness.get_profile(&name, "").await;
                assert_eq!(
                    served.status,
                    200,
                    "case {label:?}: the stored profile must read back, got {} {}",
                    served.status,
                    String::from_utf8_lossy(&served.body)
                );
                assert_eq!(
                    served.body,
                    *bytes,
                    "case {label:?}: expected the submitted bytes back, got {} bytes",
                    served.body.len()
                );
            }
            // A submission that was refused stored NOTHING, before or after the
            // aggregation a stored profile would have been owed.
            (Read::Submit { .. }, _) => {
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {label:?}: expected ValidationException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    described
                        .json()
                        .pointer("/profilingStatus/latestAgentProfileReportedAt"),
                    None,
                    "case {label:?}: a refused submission must leave the profiling status \
                     alone, got {}",
                    String::from_utf8_lossy(&described.body)
                );
            }
            // A stored document that was READ is unchanged by having been read: it is
            // served byte for byte, and a second read answers the same.
            (Read::Serve, _) => {
                let again = harness.get_profile(&name, "").await;
                assert_eq!(
                    again.body,
                    answer.body,
                    "case {label:?}: expected the same bytes on a second read, got {} bytes",
                    again.body.len()
                );
            }
            (Read::Tally, _) => {
                let data = answer.json();
                assert!(
                    data.get("frameMetricData").is_some(),
                    "case {label:?}: expected a frame metric series, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }
    }
}

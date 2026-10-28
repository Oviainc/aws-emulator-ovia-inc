// SPDX-License-Identifier: CC0-1.0

//! Which representation a STORED profile is read in, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: a stored profile is read as a document in whichever
//! representation its bytes ARE a document in — the media type it was announced under
//! says how to read bytes, it cannot stop bytes that carry a document from being read as
//! one — so `GetProfile` honours `accept` and `maxDepth` for every stored profile this
//! service can read at all, reports in `contentType` the representation the bytes it
//! serves are in, and a frame metric tallies the frames such a profile records.
//!
//! Rows vary what the rule generalises over: the media type the profile was ANNOUNCED
//! under (each of the two the model names, one of them carrying a parameter, a media type
//! that names no representation, and a third media type) crossed with the representation
//! its bytes are really a document in (each of the two), and then — for each of those —
//! the representation the request accepts and the depth it bounds the profile to.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ion_rs::{Element, v1_0};

use crate::frame_metric_data::{datum, frame_metric, frame_metric_data};
use crate::interval_validation::create_group;
use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::Harness;

/// The media types the model's `GetProfile$accept` doc names: "application/json —
/// standard JSON format" and "application/x-amzn-ion — the Amazon Ion data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// Two media types that name NEITHER representation this service serves: one the model's
/// own `PostAgentProfile$contentType` is unconstrained enough to carry (and which this
/// crate's submission table pins as accepted), and one naming arbitrary bytes.
const TEXT: &str = "text/plain";
const OCTETS: &str = "application/octet-stream";

/// The instant every profile below is placed at, in epoch seconds: long past, so the
/// aggregation it is owed has fallen due and the only thing deciding what is served is
/// the request.
const RECEIPT_SECONDS: i64 = 1_600_000_000;

/// One hour in seconds — the resolution the frame-metric case reports at.
const HOUR: i64 = 60 * 60;

/// An instant as the ISO 8601 `date-time` form the modelled timestamp members carry.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// The two representations a stored profile's bytes can be a document in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Representation {
    Json,
    Ion,
}

impl Representation {
    /// The media type `contentType` announces this representation under, which is also
    /// what an `Accept` header asks for it by.
    fn media_type(self) -> &'static str {
        match self {
            Self::Json => JSON,
            Self::Ion => ION,
        }
    }

    /// `document` written out in this representation: a JSON text, or Amazon Ion's binary
    /// encoding (what an agent posts as `application/x-amzn-ion`).
    fn bytes(self, document: &serde_json::Value) -> Vec<u8> {
        match self {
            Self::Json => serde_json::to_vec(document).expect("a serializable document"),
            Self::Ion => element_of(&serde_json::to_vec(document).expect("a serializable document"))
                .encode_as(v1_0::Binary)
                .expect("an encodable Ion document"),
        }
    }
}

/// `bytes` read as the one document they hold, in WHICHEVER encoding they are written in.
///
/// Every expectation below is stated once, as a JSON document, and compared in this one
/// form: Ion's text encoding is a superset of JSON, so a JSON text and the binary Ion
/// encoding of the same document read back as the same Ion value. The documents here hold
/// integers only, because Ion text reads a number with a fraction as a decimal where
/// JSON's data model has a float.
fn element_of(bytes: &[u8]) -> Element {
    Element::read_one(bytes).unwrap_or_else(|error| {
        panic!(
            "expected one document, got {error}: {:?}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// The expectation `document` states, in the one form [`element_of`] compares in.
fn expected_element(document: &serde_json::Value) -> Element {
    element_of(&serde_json::to_vec(document).expect("a serializable document"))
}

/// The profile document every case stores: a call stack three frames deep under one
/// member, beside the `start` member a submitted profile has to carry.
fn profile() -> serde_json::Value {
    serde_json::json!({
        "callgraph": { "A": { "B": { "RUNNABLE": 2 } } },
        "start": 1_600_000_000_000i64,
    })
}

/// [`profile`] with every frame nested deeper than `max_depth` left out — the member's
/// doc: "if the maxDepth is set to 2, then the aggregated profile contains
/// representations of methods A and B".
fn bounded_profile(max_depth: i64) -> serde_json::Value {
    let callgraph = match max_depth {
        1 => serde_json::json!({}),
        2 => serde_json::json!({ "A": {} }),
        3 => serde_json::json!({ "A": { "B": {} } }),
        _ => serde_json::json!({ "A": { "B": { "RUNNABLE": 2 } } }),
    };
    serde_json::json!({ "callgraph": callgraph, "start": 1_600_000_000_000i64 })
}

/// One stored profile: the media type it was announced under, and the representation its
/// bytes are really a document in.
struct Case {
    label: &'static str,
    announced: &'static str,
    representation: Representation,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "JSON bytes announced as JSON",
            announced: JSON,
            representation: Representation::Json,
        },
        Case {
            label: "Ion bytes announced as Ion",
            announced: ION,
            representation: Representation::Ion,
        },
        Case {
            label: "JSON bytes announced as the service's other representation",
            announced: ION,
            representation: Representation::Json,
        },
        Case {
            label: "Ion bytes announced as the service's other representation",
            announced: JSON,
            representation: Representation::Ion,
        },
        Case {
            label: "JSON bytes announced under a media type that names no representation",
            announced: TEXT,
            representation: Representation::Json,
        },
        Case {
            label: "Ion bytes announced under a media type that names no representation",
            announced: TEXT,
            representation: Representation::Ion,
        },
        Case {
            label: "JSON bytes announced as a third media type",
            announced: OCTETS,
            representation: Representation::Json,
        },
        Case {
            label: "Ion bytes announced as a third media type",
            announced: OCTETS,
            representation: Representation::Ion,
        },
        Case {
            label: "JSON bytes announced as JSON with a media-type parameter",
            announced: "application/json; charset=utf-8",
            representation: Representation::Json,
        },
        Case {
            label: "Ion bytes announced as Ion with a media-type parameter",
            announced: "application/x-amzn-ion; charset=utf-8",
            representation: Representation::Ion,
        },
    ]
}

/// A group holding exactly one SUBMITTED profile: the bytes of [`profile`] in `case`'s
/// representation, posted under the media type `case` announces them with, and placed at
/// [`RECEIPT_SECONDS`] so its aggregation is due.
async fn group_holding(harness: &Harness, name: &str, case: &Case, at: i64) -> Vec<u8> {
    let bytes = case.representation.bytes(&profile());
    create_group(harness, name).await;
    let posted = harness
        .post_agent_profile(name, case.announced, None, bytes.clone())
        .await;
    assert_eq!(
        posted.status,
        204,
        "case {:?}: posting the profile must succeed, got {} {}",
        case.label,
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    place_receipts(harness, name, &[at]);
    bytes
}

/// `GetProfile` serves a stored profile in the representation its BYTES are a document
/// in: `contentType` names that representation whatever media type the profile was
/// announced under, the stored bytes answer a request that accepts it unbounded, and the
/// request's `accept` chooses between the two representations either way.
#[tokio::test]
async fn the_representation_served_is_the_one_the_stored_bytes_are_a_document_in() {
    let harness = Harness::new();

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("stored-representation-{index}");
        let stored = group_holding(&harness, &name, &case, RECEIPT_SECONDS).await;
        let served_in = case.representation;

        // No `Accept` and no `maxDepth`: the stored bytes, announced under the
        // representation they are a document in.
        let answer = harness.get_profile(&name, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(served_in.media_type()),
            "case {label:?}: expected the representation the bytes are a document in, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.body,
            stored,
            "case {label:?}: expected the stored bytes, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );

        // `accept` chooses the representation; a header naming neither leaves the profile
        // in the one its bytes are a document in.
        let accepted: [(&str, Option<&str>, Representation); 5] = [
            ("JSON", Some(JSON), Representation::Json),
            ("Ion", Some(ION), Representation::Ion),
            ("anything", Some("*/*"), served_in),
            ("neither representation", Some("text/csv"), served_in),
            ("no Accept at all", None, served_in),
        ];
        for (asked, accept, wanted) in accepted {
            let answer = harness.get_profile_accepting(&name, "", accept).await;
            assert_eq!(
                answer.status,
                200,
                "case {label:?} accepting {asked}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.content_type.as_deref(),
                Some(wanted.media_type()),
                "case {label:?} accepting {asked}: expected {}, got {:?}",
                wanted.media_type(),
                answer.content_type
            );
            let served = element_of(&answer.body);
            let expected = expected_element(&profile());
            assert_eq!(
                served, expected,
                "case {label:?} accepting {asked}: expected the stored document, got {served}"
            );
        }

        // `maxDepth` leaves out the frames nested deeper than it, in either
        // representation. The rows span the depths this document's frames sit at, the
        // depth no frame reaches, and the maximum the model declares for the member
        // (@range(min: 1, max: 10000) — a value outside it is refused by the boundary
        // validator, which `refusal_message_formats` pins).
        for max_depth in [1i64, 2, 3, 4, 10_000] {
            for (asked, accept, wanted) in [
                ("its own representation", None, served_in),
                ("JSON", Some(JSON), Representation::Json),
                ("Ion", Some(ION), Representation::Ion),
            ] {
                let answer = harness
                    .get_profile_accepting(&name, &format!("?maxDepth={max_depth}"), accept)
                    .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {label:?} bounded to {max_depth} accepting {asked}: expected HTTP 200, \
                     got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.content_type.as_deref(),
                    Some(wanted.media_type()),
                    "case {label:?} bounded to {max_depth} accepting {asked}: expected {}, got \
                     {:?}",
                    wanted.media_type(),
                    answer.content_type
                );
                let served = element_of(&answer.body);
                let expected = expected_element(&bounded_profile(max_depth));
                assert_eq!(
                    served, expected,
                    "case {label:?} bounded to {max_depth} accepting {asked}: expected \
                     {expected}, got {served}"
                );
            }
        }

        // Serving the profile changed nothing: the stored bytes still answer the next
        // plain read.
        let again = harness.get_profile(&name, "").await;
        assert_eq!(
            again.body, stored,
            "case {label:?}: a read must not change what the next caller is served"
        );
        assert_eq!(
            again.content_type.as_deref(),
            Some(served_in.media_type()),
            "case {label:?}: a read must not change what the next caller is told, got {:?}",
            again.content_type
        );
    }
}

/// The frame-metric reader finds the frames of a stored profile by the same rule: a
/// profile whose bytes are a document records its counts whatever media type it was
/// announced under.
///
/// The document records one sample under frame `A` and one under frame `C`, so
/// `AggregatedRelativeTotalTime` for `A` — "Metric value aggregated for all instances of
/// a frame name in a profile relative to the root frame" — is half the profile's counts,
/// where a profile the reader cannot read at all records nothing and reports zero.
#[tokio::test]
async fn a_frame_metric_reads_a_stored_profile_by_the_same_rule() {
    let harness = Harness::new();
    let document = serde_json::json!({
        "callgraph": { "A": { "RUNNABLE": 1 }, "C": { "RUNNABLE": 1 } },
        "start": 1_600_000_000_000i64,
    });
    // The series is reported over one hourly window holding the receipt, so the window is
    // aligned to UTC by starting it a whole number of hours past the epoch.
    let window_start = RECEIPT_SECONDS - RECEIPT_SECONDS.rem_euclid(HOUR);

    for (index, case) in cases().into_iter().enumerate() {
        let label = case.label;
        let name = format!("stored-representation-metric-{index}");
        create_group(&harness, &name).await;
        let posted = harness
            .post_agent_profile(
                &name,
                case.announced,
                None,
                case.representation.bytes(&document),
            )
            .await;
        assert_eq!(
            posted.status,
            204,
            "case {label:?}: posting the profile must succeed, got {} {}",
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );
        place_receipts(&harness, &name, &[window_start + 60]);

        let requested = frame_metric("A", &["RUNNABLE"]);
        let answer = frame_metric_data(
            &harness,
            &name,
            &format!(
                "?startTime={}&endTime={}&targetResolution=PT1H",
                iso(window_start),
                iso(window_start + HOUR)
            ),
            std::slice::from_ref(&requested),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (metric, values) = datum(&answer, 0);
        assert_eq!(
            metric, requested,
            "case {label:?}: the datum must name the metric it was asked for, got {metric:?}"
        );
        assert_eq!(
            values,
            vec![0.5],
            "case {label:?}: expected the frame's counts over the profile's counts, got \
             {values:?}"
        );
    }
}

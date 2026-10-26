// SPDX-License-Identifier: CC0-1.0

//! How `GetProfile` serves the aggregated profile a request selects: the WINDOW its
//! `period` names, the REPRESENTATION its `Accept` asks for, and the call-stack DEPTH its
//! `maxDepth` bounds it to — end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rule every case here asserts: those three members say what the caller wants DONE
//! with the aggregated profile, not what a request must look like, so a well-formed value
//! of any of them is answered with a profile (or the operation's 404 when no aggregated
//! profile covers the window) rather than refused. `period` is any ISO 8601 duration and
//! is the LENGTH of the requested window; `Accept` chooses between the two
//! representations the service serves (`application/json` and `application/x-amzn-ion`),
//! and the response reports in `contentType` which one it served; `maxDepth` leaves out
//! the frames nested deeper than it. The one refusal left is a `period` that names no
//! fixed span of time at all, and a window whose bounds do not increase.
//!
//! Rows vary what the rule generalises over: the duration's fields, length and which
//! bound it completes; the media ranges and qualities a request accepts; the
//! representation the profile was posted in; the document's shape; and the depth asked
//! for.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ion_rs::{Element, v1_0};

use crate::interval_validation::{create_group, expected_refusal};
use crate::profile_times::place_receipts;
use crate::profiling_group_lifecycle::Harness;

/// The instant every case below places its stored profile at, in epoch seconds: long
/// past, so the aggregation the profile is owed has fallen due and the only thing that
/// decides whether it is served is the window the request names.
const RECEIPT_SECONDS: i64 = 1_600_000_000;

/// [`RECEIPT_SECONDS`] in milliseconds, which is the resolution the window arithmetic
/// below works at (a `period` may be shorter than a second).
const RECEIPT_MILLIS: i64 = RECEIPT_SECONDS * 1_000;

/// The media types the model's `GetProfile$accept` doc names: "application/json —
/// standard JSON format" and "application/x-amzn-ion — the Amazon Ion data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// `millis` since the epoch in the ISO 8601 `date-time` form the modelled timestamp
/// members carry.
fn iso(millis: i64) -> String {
    DateTime::from_secs_and_nanos(
        millis.div_euclid(1_000),
        u32::try_from(millis.rem_euclid(1_000)).expect("a millisecond inside a second")
            * 1_000_000,
    )
    .fmt(Format::DateTime)
    .expect("a representable instant")
}

/// A group holding exactly one posted profile, placed at [`RECEIPT_SECONDS`].
async fn group_holding(harness: &Harness, name: &str, content_type: &str, bytes: Vec<u8>) {
    create_group(harness, name).await;
    let posted = harness
        .post_agent_profile(name, content_type, None, bytes)
        .await;
    assert_eq!(
        posted.status,
        204,
        "posting the profile of {name} must succeed, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    place_receipts(harness, name, &[RECEIPT_SECONDS]);
}

/// The durations the window table spans, as (the ISO 8601 text, the span it names in
/// milliseconds).
///
/// SOURCE: the duration grammar `GetProfile$period` names — "Used with startTime or
/// endTime to specify the time range for the returned aggregated profile. Specify using
/// the ISO 8601 format. For example, P1DT1H1M1S"
/// (`smithy_query(query="doc",shape="GetProfileRequest",member="period")`), the member
/// targeting `Period` (a string of length 1..64) and NOT the `AggregationPeriod` enum.
/// The rows span the three periods this service aggregates at (PT5M, PT1H, P1D), the
/// member doc's own example, durations between and below them, a sub-second one, a
/// multi-field one, and the 7-day maximum range the operation's doc names.
const PERIODS: &[(&str, i64)] = &[
    ("PT5M", 300_000),
    ("PT1H", 3_600_000),
    ("P1D", 86_400_000),
    ("P1DT1H1M1S", 90_061_000),
    ("PT7M", 420_000),
    ("PT30M", 1_800_000),
    ("PT2S", 2_000),
    ("PT1.5S", 1_500),
    ("PT12H30M", 45_000_000),
    ("P7D", 604_800_000),
];

/// A `period` of ANY ISO 8601 duration completes the window, and the window it completes
/// is exactly the span that duration names: the profile is served when the window
/// contains its receipt and the operation's 404 answers when it does not — from either
/// bound, since `period` completes whichever one was given.
#[tokio::test]
async fn a_period_names_the_length_of_the_requested_window() {
    let harness = Harness::new();
    let body = br#"{"callgraph":{"A":{}},"samples":1}"#.to_vec();
    group_holding(&harness, "period-window", JSON, body.clone()).await;

    for (period, millis) in PERIODS {
        // Half the span, so both windows below are the stated length and only one of
        // them reaches the receipt.
        let grace = millis / 2;
        let rows: [(&str, String, u16); 5] = [
            (
                "a start the period reaches past the receipt from",
                format!(
                    "?startTime={}&period={period}",
                    iso(RECEIPT_MILLIS - millis + grace)
                ),
                200,
            ),
            (
                "a start the period ends before the receipt from",
                format!(
                    "?startTime={}&period={period}",
                    iso(RECEIPT_MILLIS - millis - grace)
                ),
                404,
            ),
            (
                "an end the period reaches back past the receipt from",
                format!(
                    "?endTime={}&period={period}",
                    iso(RECEIPT_MILLIS + millis - grace)
                ),
                200,
            ),
            (
                "an end the period starts after the receipt from",
                format!("?endTime={}&period={period}", iso(RECEIPT_MILLIS - grace)),
                404,
            ),
            // "If you specify only period, the latest aggregated profile is returned."
            ("the period alone", format!("?period={period}"), 200),
        ];

        for (label, query, expected) in rows {
            let answer = harness.get_profile("period-window", &query).await;
            assert_eq!(
                answer.status, expected,
                "case {period} / {label:?} ({query}): expected HTTP {expected}, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            if answer.status == 200 {
                assert_eq!(
                    answer.body,
                    body,
                    "case {period} / {label:?}: expected the stored profile, got {:?}",
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }
    }
}

/// A `period` that names no fixed span of time is the one refusal this member has left:
/// a value that is not an ISO 8601 duration at all, and one whose length depends on the
/// calendar (a year or a month designator), so no window can be computed from it.
#[tokio::test]
async fn a_period_that_names_no_fixed_span_is_refused() {
    let harness = Harness::new();
    group_holding(&harness, "period-bad", JSON, b"{}".to_vec()).await;

    let rows: [(&str, &str); 8] = [
        ("a bare designator", "P"),
        ("a time designator with no fields", "PT"),
        ("minutes written the way a human would", "5m"),
        ("a field with no unit", "PT5"),
        ("a word", "notaduration"),
        ("a calendar year", "P1Y"),
        ("a calendar month", "P1M"),
        ("a duration with trailing text", "PT5Mx"),
    ];

    for (label, period) in rows {
        let answer = harness
            .get_profile("period-bad", &format!("?period={period}"))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {label:?} ({period}): expected HTTP 400, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {label:?} ({period}): expected ValidationException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            format!(
                "period must name a fixed span of time in the ISO 8601 duration format, \
                 for example P1DT1H1M1S, got {period}"
            ),
            "case {label:?} ({period}): expected the rule the member states, got {:?}",
            answer.error_message()
        );
    }

    // A duration of zero length IS a duration, and the window it completes is zero wide:
    // that is the service's interval rule refusing it, with the sentence that rule
    // states, rather than a word about the duration.
    let start = "2020-09-13T12:26:40Z";
    let answer = harness
        .get_profile("period-bad", &format!("?startTime={start}&period=PT0S"))
        .await;
    assert_eq!(
        answer.status, 400,
        "case a zero-length period: expected HTTP 400, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_message(),
        expected_refusal(start, start),
        "case a zero-length period: expected the interval rule's sentence, got {:?}",
        answer.error_message()
    );
}

/// The profile document the depth table bounds: three levels of nested frames under one
/// member, a shallower nested member beside it, and scalars at the top.
fn nested_document() -> serde_json::Value {
    serde_json::json!({
        "agentMetadata": { "agentInfo": { "agentVersion": "1.0" } },
        "callgraph": { "A": { "B": { "C": {} } } },
        "samplesPerSecond": 1.5,
        "start": 1_600_000_000_000i64,
    })
}

/// `maxDepth` serves the profile with the frames nested deeper than it left out — the
/// member's doc: "if the maxDepth is set to 2, then the aggregated profile contains
/// representations of methods A and B". A frame AT the bound is served and emptied, a
/// scalar a frame carries is served wherever it sits, and a bound no frame reaches serves
/// the whole document.
#[tokio::test]
async fn max_depth_leaves_out_the_frames_nested_deeper_than_it() {
    let harness = Harness::new();
    let document = nested_document();
    let body = serde_json::to_vec(&document).expect("a serializable document");
    group_holding(&harness, "depth-json", JSON, body.clone()).await;

    let rows: Vec<(i64, serde_json::Value)> = vec![
        (
            1,
            serde_json::json!({
                "agentMetadata": {},
                "callgraph": {},
                "samplesPerSecond": 1.5,
                "start": 1_600_000_000_000i64,
            }),
        ),
        (
            2,
            serde_json::json!({
                "agentMetadata": { "agentInfo": {} },
                "callgraph": { "A": {} },
                "samplesPerSecond": 1.5,
                "start": 1_600_000_000_000i64,
            }),
        ),
        (
            3,
            serde_json::json!({
                "agentMetadata": { "agentInfo": { "agentVersion": "1.0" } },
                "callgraph": { "A": { "B": {} } },
                "samplesPerSecond": 1.5,
                "start": 1_600_000_000_000i64,
            }),
        ),
        (4, document.clone()),
        // The bounds the model declares for `maxDepth` (@range(min: 1, max: 10000)): the
        // deepest frame of this document is far above neither, so both serve all of it.
        (10_000, document.clone()),
    ];

    for (max_depth, expected) in rows {
        let answer = harness
            .get_profile("depth-json", &format!("?maxDepth={max_depth}"))
            .await;
        assert_eq!(
            answer.status, 200,
            "case maxDepth={max_depth}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON),
            "case maxDepth={max_depth}: expected the profile's own representation, got {:?}",
            answer.content_type
        );
        let served: serde_json::Value = serde_json::from_slice(&answer.body)
            .unwrap_or_else(|error| panic!("case maxDepth={max_depth}: unreadable body: {error}"));
        assert_eq!(
            served, expected,
            "case maxDepth={max_depth}: expected {expected}, got {served}"
        );
    }

    // A frame tree written as nested LISTS is bounded the same way: the depth of a frame
    // is how many containers of the document it sits inside, whatever kind they are.
    let listed = serde_json::json!({ "frames": [["A", ["B"]]] });
    let body = serde_json::to_vec(&listed).expect("a serializable document");
    group_holding(&harness, "depth-lists", JSON, body).await;
    let list_rows: Vec<(i64, serde_json::Value)> = vec![
        (1, serde_json::json!({ "frames": [] })),
        (2, serde_json::json!({ "frames": [[]] })),
        (3, serde_json::json!({ "frames": [["A", []]] })),
        (4, listed.clone()),
    ];
    for (max_depth, expected) in list_rows {
        let answer = harness
            .get_profile("depth-lists", &format!("?maxDepth={max_depth}"))
            .await;
        let served: serde_json::Value = serde_json::from_slice(&answer.body)
            .unwrap_or_else(|error| panic!("case lists maxDepth={max_depth}: {error}"));
        assert_eq!(
            served, expected,
            "case lists maxDepth={max_depth}: expected {expected}, got {served}"
        );
    }

    // No `maxDepth` bounds nothing, and the stored bytes are then the answer byte for
    // byte — including the member order the caller posted, which no re-encoding touched.
    let whole = harness.get_profile("depth-json", "").await;
    assert_eq!(
        whole.body,
        serde_json::to_vec(&document).expect("a serializable document"),
        "a read with no maxDepth must serve the stored bytes, got {:?}",
        String::from_utf8_lossy(&whole.body)
    );
}

/// An Ion document, read from its text form so the case states what it holds, and served
/// as the binary encoding an agent posts.
fn ion_document(text: &str) -> Element {
    Element::read_one(text).unwrap_or_else(|error| panic!("unreadable Ion {text:?}: {error}"))
}

/// The binary encoding of `element` — what `application/x-amzn-ion` carries.
fn ion_bytes(element: &Element) -> Vec<u8> {
    element
        .encode_as(v1_0::Binary)
        .unwrap_or_else(|error| panic!("unencodable Ion document: {error}"))
}

/// `bytes` read back as one Ion document.
fn ion_read(bytes: &[u8]) -> Element {
    Element::read_one(bytes).unwrap_or_else(|error| {
        panic!(
            "expected an Ion document, got {error}: {:?}",
            String::from_utf8_lossy(bytes)
        )
    })
}

/// `Accept` chooses which of the two representations the service serves a profile in, and
/// the response says which one it served: the stored representation when the request
/// accepts it (served byte for byte, since nothing has to be re-encoded), the other one
/// when the request ranks it higher, and the stored one again when the request accepts
/// neither — there is no third representation to answer with, and no refusal either.
#[tokio::test]
async fn accept_chooses_the_representation_a_profile_is_served_in() {
    let harness = Harness::new();
    // Integers only: the document is compared against the Ion reading of its own JSON
    // text, and Ion text reads a number with a fraction as a decimal where JSON's data
    // model has a float.
    let document = serde_json::json!({ "callgraph": { "A": { "B": {} } }, "samples": 3 });
    let body = serde_json::to_vec(&document).expect("a serializable document");
    group_holding(&harness, "accept-json", JSON, body.clone()).await;

    // (case, the Accept header, the representation the response must be in)
    let rows: [(&str, Option<&str>, &str); 10] = [
        ("no Accept at all", None, JSON),
        ("the representation it is stored in", Some(JSON), JSON),
        ("any representation", Some("*/*"), JSON),
        ("any representation of the application type", Some("application/*"), JSON),
        ("the service's other representation", Some(ION), ION),
        ("that representation with a parameter", Some("application/x-amzn-ion; charset=utf-8"), ION),
        ("Ion ranked above JSON", Some("application/json;q=0.2, application/x-amzn-ion;q=0.8"), ION),
        ("JSON ranked above Ion", Some("application/x-amzn-ion;q=0.2, application/json;q=0.8"), JSON),
        ("a media type this service does not serve", Some("text/csv"), JSON),
        ("the stored representation refused outright", Some("application/json;q=0"), JSON),
    ];

    for (label, accept, expected) in rows {
        let answer = harness
            .get_profile_accepting("accept-json", "", accept)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?} (accept: {accept:?}): expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(expected),
            "case {label:?} (accept: {accept:?}): expected {expected}, got {:?}",
            answer.content_type
        );
        if expected == JSON {
            assert_eq!(
                answer.body,
                body,
                "case {label:?}: the stored representation is served byte for byte, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        } else {
            // The same document, in Ion: a JSON text is itself valid Ion text, so the
            // served binary Ion is compared against the Ion reading of what was posted.
            let served = ion_read(&answer.body);
            let posted = ion_read(&body);
            assert_eq!(
                served, posted,
                "case {label:?}: expected the posted document in Ion, got {served}"
            );
        }
    }

    // Serving a profile changed nothing: the stored bytes still answer a plain read.
    let again = harness.get_profile("accept-json", "").await;
    assert_eq!(
        again.body, body,
        "a read must not change what the next caller is served"
    );
}

/// A profile posted as Ion is served as JSON when the request accepts JSON: every Ion
/// type JSON does not have is carried over as the JSON value that holds the same
/// information, and the depth bound applies to an Ion document exactly as it does to a
/// JSON one.
#[tokio::test]
async fn an_ion_profile_is_served_in_either_representation() {
    let harness = Harness::new();
    let document = ion_document(
        r#"{ callgraph: { A: { B: { } } }, samples: 3, name: "demo", tag: symbolic,
             when: 2020-09-13T12:26:40Z, payload: {{aGk=}}, ratio: 1.5 }"#,
    );
    let body = ion_bytes(&document);
    group_holding(&harness, "accept-ion", ION, body.clone()).await;

    // Stored representation, no depth bound: the bytes as posted.
    for (label, accept) in [("no Accept at all", None), ("Ion itself", Some(ION))] {
        let answer = harness.get_profile_accepting("accept-ion", "", accept).await;
        assert_eq!(
            answer.content_type.as_deref(),
            Some(ION),
            "case {label:?}: expected {ION}, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.body, body,
            "case {label:?}: expected the posted Ion bytes"
        );
    }

    // Asked for JSON: the same document, down-converted.
    let answer = harness
        .get_profile_accepting("accept-ion", "", Some(JSON))
        .await;
    assert_eq!(
        answer.status, 200,
        "case Ion served as JSON: expected HTTP 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.content_type.as_deref(),
        Some(JSON),
        "case Ion served as JSON: expected {JSON}, got {:?}",
        answer.content_type
    );
    let mut served: serde_json::Value = serde_json::from_slice(&answer.body)
        .unwrap_or_else(|error| panic!("case Ion served as JSON: unreadable body: {error}"));
    // A timestamp carries over as its text; the text is Ion's rendering of that instant,
    // so the case asserts the instant it names rather than one spelling of it.
    let when = served
        .as_object_mut()
        .and_then(|fields| fields.remove("when"))
        .unwrap_or_else(|| panic!("case Ion served as JSON: no `when` member in {served}"));
    assert!(
        when.as_str()
            .is_some_and(|text| text.starts_with("2020-09-13T12:26:40")),
        "case Ion served as JSON: expected the timestamp as text, got {when}"
    );
    assert_eq!(
        served,
        serde_json::json!({
            "callgraph": { "A": { "B": {} } },
            "samples": 3,
            "name": "demo",
            // A symbol carries over as its text, a blob as base64 text ("aGk=" is "hi"),
            // a decimal as a number.
            "tag": "symbolic",
            "payload": "aGk=",
            "ratio": 1.5,
        }),
        "case Ion served as JSON: expected the down-converted document, got {served}"
    );

    // The depth bound, in each representation.
    let bounded = harness.get_profile("accept-ion", "?maxDepth=2").await;
    assert_eq!(
        bounded.content_type.as_deref(),
        Some(ION),
        "case Ion bounded to depth 2: expected {ION}, got {:?}",
        bounded.content_type
    );
    let served = ion_read(&bounded.body);
    let expected = ion_document(
        r#"{ callgraph: { A: { } }, samples: 3, name: "demo", tag: symbolic,
             when: 2020-09-13T12:26:40Z, payload: {{aGk=}}, ratio: 1.5 }"#,
    );
    assert_eq!(
        served, expected,
        "case Ion bounded to depth 2: expected {expected}, got {served}"
    );

    let bounded = harness
        .get_profile_accepting("accept-ion", "?maxDepth=1", Some(JSON))
        .await;
    let served: serde_json::Value = serde_json::from_slice(&bounded.body)
        .unwrap_or_else(|error| panic!("case Ion bounded and served as JSON: {error}"));
    assert_eq!(
        served.get("callgraph"),
        Some(&serde_json::json!({})),
        "case Ion bounded to depth 1 and served as JSON: expected an empty frame, got \
         {served}"
    );
}

/// Containers nested `levels` deep as a JSON text — which is also an Ion TEXT document
/// (Ion text is a superset of JSON).
fn nested_containers(levels: usize) -> Vec<u8> {
    format!("{}{}", "[".repeat(levels), "]".repeat(levels)).into_bytes()
}

/// The same nesting as one BINARY Ion document, built a level at a time so the test
/// itself descends into nothing.
fn nested_ion_containers(levels: usize) -> Vec<u8> {
    let mut element = Element::from(ion_rs::Value::List(ion_rs::Sequence::new(
        Vec::<Element>::new(),
    )));
    for _ in 1..levels {
        element = Element::from(ion_rs::Value::List(ion_rs::Sequence::new(vec![element])));
    }
    ion_bytes(&element)
}

/// The nesting a profile document is read at is bounded by the OPERATION, not by the
/// request: a document within the bound is re-encoded and depth-bounded as any other is.
#[tokio::test]
async fn a_document_inside_the_read_bound_is_served_bounded() {
    let harness = Harness::new();

    for (label, content_type, name, bytes) in [
        (
            "inside the bound, as JSON",
            JSON,
            "deep-ok-json",
            nested_containers(120),
        ),
        (
            "inside the bound, as Ion",
            ION,
            "deep-ok-ion",
            nested_ion_containers(120),
        ),
    ] {
        group_holding(&harness, name, content_type, bytes).await;
        let answer = harness
            .get_profile_accepting(name, "?maxDepth=2", Some(JSON))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            String::from_utf8_lossy(&answer.body),
            "[[[]]]",
            "case {label:?}: expected the document bounded to depth 2"
        );
    }
}

/// A document nested deeper than this operation reads is served exactly as stored,
/// however deeply the posted payload nests — nothing descends into it. So is a payload in
/// Ion's TEXT encoding, where finding where one value ends means reading everything nested
/// inside it first: the encoding this operation reads is Ion's binary one.
#[tokio::test]
async fn a_document_past_the_read_bound_is_served_as_it_is_stored() {
    let harness = Harness::new();

    for (label, content_type, name, bytes) in [
        (
            "JSON past the bound",
            JSON,
            "deep-past-json",
            nested_containers(200),
        ),
        (
            "binary Ion past the bound",
            ION,
            "deep-past-ion",
            nested_ion_containers(200),
        ),
        (
            "Ion text, at any depth",
            ION,
            "ion-text",
            br#"{ callgraph: { A: { } } }"#.to_vec(),
        ),
    ] {
        group_holding(&harness, name, content_type, bytes.clone()).await;
        for (ask, query, accept) in [
            ("a depth bound", "?maxDepth=2", None),
            ("the other representation", "", Some(JSON)),
        ] {
            let answer = harness.get_profile_accepting(name, query, accept).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}, asked for {ask}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.content_type.as_deref(),
                Some(content_type),
                "case {label:?}, asked for {ask}: expected the media type it really has, \
                 got {:?}",
                answer.content_type
            );
            assert_eq!(
                answer.body, bytes,
                "case {label:?}, asked for {ask}: expected the stored bytes"
            );
        }
    }
}

/// A payload that is not a document in the representation it was posted under holds no
/// frames this service can see and has no other representation to be written out in, so
/// it is served exactly as it is stored, under the media type it really has — which is
/// what `contentType` then reports.
#[tokio::test]
async fn a_payload_that_is_no_document_is_served_as_it_is_stored() {
    let harness = Harness::new();

    // (case, the media type it was posted under, the bytes)
    let rows: [(&str, &str, Vec<u8>); 3] = [
        ("every byte value, posted as Ion", ION, (0..=255u8).collect()),
        ("a truncated JSON text", JSON, b"{\"callgraph\":".to_vec()),
        ("one brace, posted as JSON", JSON, vec![b'{']),
    ];

    for (index, (label, content_type, bytes)) in rows.into_iter().enumerate() {
        let name = format!("opaque-{index}");
        group_holding(&harness, &name, content_type, bytes.clone()).await;

        for (ask, query, accept) in [
            ("a depth bound", "?maxDepth=2", None),
            ("the other representation", "", Some(JSON)),
            ("both", "?maxDepth=3", Some(ION)),
        ] {
            let answer = harness.get_profile_accepting(&name, query, accept).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?} asked for {ask}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.content_type.as_deref(),
                Some(content_type),
                "case {label:?} asked for {ask}: expected the media type it really has, \
                 got {:?}",
                answer.content_type
            );
            assert_eq!(
                answer.body,
                bytes,
                "case {label:?} asked for {ask}: expected the stored bytes, got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

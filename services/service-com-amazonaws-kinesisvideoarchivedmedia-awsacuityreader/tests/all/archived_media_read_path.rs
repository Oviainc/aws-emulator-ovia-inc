// SPDX-License-Identifier: CC0-1.0

//! The archived-media read loop, end to end over HTTP.
//!
//! Every test drives the real `MultiServiceRouter` with this one service
//! registered, so a request goes through routing, restJson1 deserialization,
//! constraint validation, the handler, and serialization — the same path a
//! socket-served request takes.
//!
//! The archive is SEEDED through the service's own state API (this service has
//! no create operation: streams and fragments are ingested through the Kinesis
//! Video Streams control/data plane), and every assertion below is about what
//! the handlers compute FROM that archive.

use bytes::Bytes;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use archived_media::state::{
    epoch_millis, from_epoch_millis, service_state, ArchivedStream, StoredFragment,
};

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";
/// The endpoint every request in this suite addresses: this emulator, on
/// loopback. A session URL must come back here, never to a real AWS host.
pub(crate) const EMULATOR_ENDPOINT: &str = "127.0.0.1:9297";
/// The seeded stream with an archive and 24 hours of retention.
pub(crate) const STREAM: &str = "demo-stream";
/// A seeded stream with `DataRetentionInHours` of 0.
pub(crate) const NO_RETENTION_STREAM: &str = "no-retention-stream";
/// A seeded stream whose fragments were ingested just now.
pub(crate) const LIVE_STREAM: &str = "live-stream";
/// The base of the seeded range: an instant well in the past, so nothing in
/// this stream's archive is within the LIVE 30-second window.
pub(crate) const T0_SECS: i64 = 1_761_000_000;
/// How many fragments `STREAM` holds.
const SEEDED_FRAGMENTS: u64 = 5;
/// How many fragments `LIVE_STREAM` holds.
pub(crate) const LIVE_FRAGMENTS: u64 = 7;
/// A realistic Kinesis Video fragment number: a long, increasing decimal.
const FRAGMENT_BASE: u64 = 9_134_385_233_318_143_200;

/// The fragment number of the `index`-th seeded fragment.
pub(crate) fn fragment_number(index: u64) -> String {
    (FRAGMENT_BASE + index).to_string()
}

/// The media bytes of the `index`-th seeded fragment: distinct content AND a
/// distinct length per fragment, so a concatenation can only come out right if
/// the right fragments were picked in the right order.
fn media(index: u64) -> Vec<u8> {
    let filler = b'a' + u8::try_from(index).expect("small index");
    vec![filler; 16 + usize::try_from(index).expect("small index")]
}

/// The producer timestamp of the `index`-th seeded fragment.
fn producer_secs(index: u64) -> i64 {
    T0_SECS + 10 * i64::try_from(index).expect("small index")
}

/// The server timestamp of the `index`-th seeded fragment: an hour's worth of
/// offset from the producer timestamp, so selecting on one field cannot pass by
/// accident when the other was meant.
fn server_secs(index: u64) -> i64 {
    producer_secs(index) + 60
}

/// A seeded emulator: the router, the shared engine, and the seeded stream's ARN.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
    pub(crate) arn: String,
}

impl Harness {
    /// POST `body` to `path` through the router, signed for `REGION` so the
    /// request routes by its SigV4 credential scope, and addressed at a
    /// loopback endpoint — the emulator's own, never a real AWS host.
    pub(crate) async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{EMULATOR_ENDPOINT}{path}"))
            .header("host", EMULATOR_ENDPOINT)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("a valid request");
        self.router.dispatch(request).await
    }
}

/// The JSON body of a response.
pub(crate) fn body_json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// The `content-type` header of a response.
fn content_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A router serving this service alone over an engine seeded with three
/// streams: an archive in the past, one with no retention, and one ingested
/// just now.
pub(crate) fn seeded() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let arn = {
        let state = service_state(&*engine).expect("the service store");
        let mut state = state.lock();

        let created_at = from_epoch_millis(T0_SECS * 1000);
        state.put_stream(ArchivedStream::new(STREAM, REGION, created_at, 24));
        state.put_stream(ArchivedStream::new(
            NO_RETENTION_STREAM,
            REGION,
            created_at,
            0,
        ));
        state.put_stream(ArchivedStream::new(LIVE_STREAM, REGION, created_at, 24));

        // Appended out of ingestion order on purpose: the archive is kept in
        // ascending fragment-number order by the state API, not by the caller.
        for index in [2, 0, 4, 1, 3] {
            assert!(
                state.append_fragment(
                    STREAM,
                    StoredFragment::new(
                        fragment_number(index),
                        from_epoch_millis(producer_secs(index) * 1000),
                        from_epoch_millis(server_secs(index) * 1000),
                        2000,
                        media(index),
                    ),
                ),
                "case seed fragment {index}: the seeded stream must exist"
            );
        }

        let now = SystemTime::now();
        for index in 0..LIVE_FRAGMENTS {
            let ingested = now - Duration::from_secs(LIVE_FRAGMENTS - index);
            assert!(
                state.append_fragment(
                    LIVE_STREAM,
                    StoredFragment::new(
                        fragment_number(index),
                        ingested,
                        ingested,
                        1000,
                        media(index),
                    ),
                ),
                "case seed live fragment {index}: the live stream must exist"
            );
        }

        state
            .streams
            .get(STREAM)
            .expect("the seeded stream")
            .arn
            .clone()
    };

    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
    Harness {
        router,
        engine,
        arn,
    }
}

/// `base` with every member of `extra` added.
pub(crate) fn with_members(
    base: &serde_json::Value,
    extra: &serde_json::Value,
) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().expect("an object of members") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(object)
}

/// Every operation of the read loop, with a request body that succeeds for the
/// seeded stream once the stream-naming members are added.
pub(crate) fn read_loop_operations() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "/listFragments",
            serde_json::json!({
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getMediaForFragmentList",
            serde_json::json!({ "Fragments": [fragment_number(0)] }),
        ),
        (
            "/getClip",
            serde_json::json!({
                "ClipFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getHLSStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getDASHStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
    ]
}

/// RULE: every archived-media operation resolves the stream the same way —
/// exactly one of `StreamName` and `StreamARN` must be present (neither and
/// both are `InvalidArgumentException`, 400), and a name or ARN that answers to
/// no stream is `ResourceNotFoundException` (404).
#[tokio::test]
async fn every_operation_resolves_the_stream_by_the_same_rule() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "StreamName only",
            serde_json::json!({ "StreamName": STREAM }),
            200,
            "",
        ),
        (
            "StreamARN only",
            serde_json::json!({ "StreamARN": harness.arn.clone() }),
            200,
            "",
        ),
        (
            "both StreamName and StreamARN",
            serde_json::json!({ "StreamName": STREAM, "StreamARN": harness.arn.clone() }),
            400,
            "InvalidArgumentException",
        ),
        (
            "neither StreamName nor StreamARN",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "unknown StreamName",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown StreamARN",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000"
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (path, extra) in read_loop_operations() {
        for (case, stream, status, error) in &cases {
            let body = with_members(stream, &extra);
            let response = harness.post(path, &body).await;
            assert_eq!(
                response.status().as_u16(),
                *status,
                "case {path} / {case}: expected HTTP {status}, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            if !error.is_empty() {
                let reported = body_json(&response);
                assert_eq!(
                    reported["__type"], *error,
                    "case {path} / {case}: expected error {error}, got {reported:?}"
                );
            }
        }
    }
}

/// RULE: `ListFragments` returns the archived fragments whose SELECTED
/// timestamp field (`PRODUCER_TIMESTAMP` or `SERVER_TIMESTAMP`) falls inside
/// the inclusive `TimestampRange` — "This value is inclusive. The EndTimestamp
/// is compared to the (starting) timestamp of the fragment."
#[tokio::test]
async fn list_fragments_filters_on_the_selected_timestamp_field() {
    let harness = seeded();
    // (case, selector type, start offset, end offset, the fragment indexes the
    // archive holds in that window)
    let cases: Vec<(&str, &str, i64, i64, Vec<u64>)> = vec![
        ("producer, whole archive", "PRODUCER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
        ("producer, first three", "PRODUCER_TIMESTAMP", 0, 25, vec![0, 1, 2]),
        ("producer, one instant", "PRODUCER_TIMESTAMP", 10, 10, vec![1]),
        ("producer, before the archive", "PRODUCER_TIMESTAMP", -100, -1, vec![]),
        ("producer, inclusive end", "PRODUCER_TIMESTAMP", 20, 30, vec![2, 3]),
        // The same windows read on the server timestamps, which are offset by
        // a minute: the SAME range selects a different set.
        ("server, producer window", "SERVER_TIMESTAMP", 0, 25, vec![]),
        ("server, first three", "SERVER_TIMESTAMP", 60, 85, vec![0, 1, 2]),
        ("server, last one", "SERVER_TIMESTAMP", 100, 1000, vec![4]),
        ("server, whole archive", "SERVER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
    ];

    for (case, selector_type, start, end, expected) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "FragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/listFragments", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        let numbers: Vec<String> = reported["Fragments"]
            .as_array()
            .expect("Fragments is a list")
            .iter()
            .map(|fragment| fragment["FragmentNumber"].as_str().unwrap_or("").to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().copied().map(fragment_number).collect();
        assert_eq!(
            numbers, wanted,
            "case {case}: expected fragments {wanted:?}, got {numbers:?}"
        );
        assert!(
            reported["NextToken"].is_null(),
            "case {case}: a page that returned everything must carry no NextToken, got {:?}",
            reported["NextToken"]
        );
    }
}

/// RULE: each returned `Fragment` reports that fragment's own persisted
/// identity, size, timestamps and duration.
#[tokio::test]
async fn list_fragments_reports_each_fragment_from_the_archive() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "FragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });
    let response = harness.post("/listFragments", &body).await;
    assert_eq!(response.status().as_u16(), 200, "expected HTTP 200");
    let reported = body_json(&response);
    let fragments = reported["Fragments"].as_array().expect("Fragments is a list");

    for index in 0..SEEDED_FRAGMENTS {
        let reported = &fragments[usize::try_from(index).expect("small index")];
        let case = format!("fragment {index}");
        assert_eq!(
            reported["FragmentNumber"].as_str().unwrap_or_default(),
            fragment_number(index),
            "case {case}: wrong FragmentNumber, got {reported:?}"
        );
        assert_eq!(
            reported["FragmentSizeInBytes"].as_i64().unwrap_or_default(),
            i64::try_from(media(index).len()).expect("a small fragment"),
            "case {case}: FragmentSizeInBytes must be the stored media's length, got {reported:?}"
        );
        assert_eq!(
            reported["ProducerTimestamp"].as_f64().unwrap_or_default(),
            producer_secs(index) as f64,
            "case {case}: wrong ProducerTimestamp, got {reported:?}"
        );
        assert_eq!(
            reported["ServerTimestamp"].as_f64().unwrap_or_default(),
            server_secs(index) as f64,
            "case {case}: wrong ServerTimestamp, got {reported:?}"
        );
        assert_eq!(
            reported["FragmentLengthInMilliseconds"]
                .as_i64()
                .unwrap_or_default(),
            2000,
            "case {case}: wrong FragmentLengthInMilliseconds, got {reported:?}"
        );
    }
}

/// RULE: pagination is a partition — whatever the page size, replaying the
/// returned `NextToken` continues where the previous page stopped, no fragment
/// repeats or goes missing, and the final page carries no token. The token also
/// carries the selector, which is why the model makes `FragmentSelector`
/// optional on a continuation.
#[tokio::test]
async fn list_fragments_pages_partition_the_matching_fragments() {
    let harness = seeded();
    let all: Vec<String> = (0..SEEDED_FRAGMENTS).map(fragment_number).collect();

    for page_size in 1..=(SEEDED_FRAGMENTS + 1) {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages: u64 = 0;

        loop {
            let body = match &next_token {
                // A continuation carries ONLY the token: no selector.
                Some(token) => serde_json::json!({
                    "StreamName": STREAM,
                    "MaxResults": page_size,
                    "NextToken": token,
                }),
                None => serde_json::json!({
                    "StreamName": STREAM,
                    "MaxResults": page_size,
                    "FragmentSelector": {
                        "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                        "TimestampRange": {
                            "StartTimestamp": T0_SECS,
                            "EndTimestamp": T0_SECS + 1000,
                        },
                    },
                }),
            };
            let response = harness.post("/listFragments", &body).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case}: expected HTTP 200, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let reported = body_json(&response);
            let page: Vec<String> = reported["Fragments"]
                .as_array()
                .expect("Fragments is a list")
                .iter()
                .map(|fragment| fragment["FragmentNumber"].as_str().unwrap_or("").to_string())
                .collect();
            assert!(
                page.len() <= usize::try_from(page_size).expect("small page size"),
                "case {case}: a page returned {} fragments, more than MaxResults",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(pages <= SEEDED_FRAGMENTS + 2, "case {case}: pagination did not terminate");

            match reported["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='),
                        "case {case}: NextToken {token} is outside the modelled token pattern"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, all,
            "case {case}: paging must yield every matching fragment exactly once, in order"
        );
    }
}

/// RULE: a request that carries neither `NextToken` nor `FragmentSelector`, or
/// a range whose end precedes its start, or a token issued for another stream,
/// is `InvalidArgumentException` (400).
#[tokio::test]
async fn list_fragments_refuses_a_selector_it_cannot_page_on() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "no selector and no token",
            serde_json::json!({ "StreamName": STREAM }),
        ),
        (
            "end before start",
            serde_json::json!({
                "StreamName": STREAM,
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 10,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
        ),
        (
            "a token that is not one",
            serde_json::json!({ "StreamName": STREAM, "NextToken": "bm90YXRva2Vu" }),
        ),
    ];

    for (case, body) in cases {
        let response = harness.post("/listFragments", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {reported:?}"
        );
    }
}

/// RULE: `GetMediaForFragmentList`'s payload is the stored media of exactly the
/// requested fragments, concatenated in the order the request lists them.
#[tokio::test]
async fn get_media_for_fragment_list_concatenates_exactly_what_was_asked_for() {
    let harness = seeded();
    // (case, the fragment indexes requested, in request order)
    let cases: Vec<(&str, Vec<u64>)> = vec![
        ("one fragment", vec![0]),
        ("two in archive order", vec![0, 1]),
        ("two in reverse order", vec![1, 0]),
        ("the same fragment twice", vec![2, 2]),
        ("a non-contiguous selection", vec![4, 1, 3]),
        ("the whole archive", vec![0, 1, 2, 3, 4]),
    ];

    for (case, indexes) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "Fragments": indexes.iter().copied().map(fragment_number).collect::<Vec<_>>(),
        });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            content_type(&response),
            "video/webm",
            "case {case}: wrong ContentType header"
        );
        let expected: Vec<u8> = indexes.iter().copied().flat_map(media).collect();
        assert_eq!(
            response.body().as_ref(),
            expected.as_slice(),
            "case {case}: expected the concatenation of {indexes:?}, got {} bytes",
            response.body().len()
        );
    }
}

/// RULE: a fragment number the archive does not hold is
/// `InvalidArgumentException` (400) — never a short payload.
#[tokio::test]
async fn get_media_for_fragment_list_refuses_an_unarchived_fragment() {
    let harness = seeded();
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("only an absent fragment", vec![fragment_number(99)]),
        (
            "an absent fragment beside a present one",
            vec![fragment_number(0), fragment_number(99)],
        ),
        (
            "a fragment of another stream's numbering",
            vec!["1".to_string()],
        ),
    ];

    for (case, fragments) in cases {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {reported:?}"
        );
    }
}

/// RULE: the bytes one response assembles are bounded, not just the number of
/// fragments a request may name — a list that names one large fragment
/// repeatedly is refused with the modelled limit error rather than
/// materialising N copies of it.
#[tokio::test]
async fn get_media_for_fragment_list_bounds_the_bytes_it_assembles() {
    use archived_media::state::MAX_PAYLOAD_BYTES;

    let harness = seeded();
    // One fragment just over half the ceiling, so TWO references to it exceed
    // it while one does not: the count is identical in both cases, only the
    // product differs.
    let large = fragment_number(SEEDED_FRAGMENTS + 1);
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        assert!(
            state.append_fragment(
                STREAM,
                StoredFragment::new(
                    large.clone(),
                    from_epoch_millis((T0_SECS + 500) * 1000),
                    from_epoch_millis((T0_SECS + 560) * 1000),
                    2000,
                    vec![b'z'; MAX_PAYLOAD_BYTES / 2 + 1],
                ),
            ),
            "case seed the large fragment: the seeded stream must exist"
        );
    }

    let cases: Vec<(&str, Vec<String>, u16)> = vec![
        ("one copy, under the ceiling", vec![large.clone()], 200),
        (
            "two copies, over the ceiling",
            vec![large.clone(), large.clone()],
            400,
        ),
    ];

    for (case, fragments, status) in cases {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {}",
            response.status()
        );
        if status == 400 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "ClientLimitExceededException",
                "case {case}: expected ClientLimitExceededException, got {reported:?}"
            );
        }
    }
}

/// RULE: `GetClip` answers `video/mp4` and a payload assembled from exactly the
/// fragments the archive holds in the selected inclusive range.
#[tokio::test]
async fn get_clip_assembles_the_fragments_the_archive_holds_in_range() {
    let harness = seeded();
    // (case, selector type, start offset, end offset, fragment indexes)
    let cases: Vec<(&str, &str, i64, i64, Vec<u64>)> = vec![
        ("producer, whole archive", "PRODUCER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
        ("producer, first two", "PRODUCER_TIMESTAMP", 0, 15, vec![0, 1]),
        ("producer, one fragment", "PRODUCER_TIMESTAMP", 30, 35, vec![3]),
        ("server, first three", "SERVER_TIMESTAMP", 60, 85, vec![0, 1, 2]),
        ("server, whole archive", "SERVER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
    ];

    for (case, selector_type, start, end, indexes) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "ClipFragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/getClip", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            content_type(&response),
            "video/mp4",
            "case {case}: GetClip downloads an MP4 clip"
        );
        let expected: Vec<u8> = indexes.iter().copied().flat_map(media).collect();
        assert_eq!(
            response.body().as_ref(),
            expected.as_slice(),
            "case {case}: expected the clip of {indexes:?}, got {} bytes",
            response.body().len()
        );
    }
}

/// RULE: a clip carries at most the documented number of fragments — "The
/// output will contain the first 100 MB or the first 200 fragments from the
/// specified start timestamp" — counted from the start of the range, so a range
/// holding more than that is TRUNCATED rather than refused or served whole.
#[tokio::test]
async fn get_clip_truncates_at_the_documented_fragment_ceiling() {
    /// The published ceiling this test pins, from the `GetClip$Payload` doc
    /// sentence quoted above.
    const MAX_CLIP_FRAGMENTS: usize = 200;
    /// A stream holding more fragments in range than one clip may carry.
    const DEEP_STREAM: &str = "deep-archive-stream";
    /// The size of each of its fragments: small, so the whole archive is
    /// cheap and only the FRAGMENT ceiling can be the one that bites.
    const FRAGMENT_SIZE: usize = 8;

    // The filler varies with the index, so the expected clip pins WHICH
    // fragments were taken and in what order, not only how many bytes came
    // back: a handler that returned the LAST 200 fragments of the range, or
    // any other 200, produces different bytes of the same length.
    let filler = |index: usize| b'a' + u8::try_from(index % 26).expect("a letter");

    let harness = seeded();
    let held = MAX_CLIP_FRAGMENTS + 50;
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        state.put_stream(ArchivedStream::new(
            DEEP_STREAM,
            REGION,
            from_epoch_millis(T0_SECS * 1000),
            24,
        ));
        for index in 0..held {
            let at = from_epoch_millis((T0_SECS + index as i64) * 1000);
            assert!(
                state.append_fragment(
                    DEEP_STREAM,
                    StoredFragment::new(
                        fragment_number(index as u64),
                        at,
                        at,
                        1000,
                        vec![filler(index); FRAGMENT_SIZE],
                    ),
                ),
                "case seed fragment {index}: the deep stream must exist"
            );
        }
    }

    let body = serde_json::json!({
        "StreamName": DEEP_STREAM,
        "ClipFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": {
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + held as i64,
            },
        },
    });
    let response = harness.post("/getClip", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case truncated clip: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(
        content_type(&response),
        "video/mp4",
        "case truncated clip: a truncated clip is still an MP4 clip"
    );
    let expected: Vec<u8> = (0..MAX_CLIP_FRAGMENTS)
        .flat_map(|index| vec![filler(index); FRAGMENT_SIZE])
        .collect();
    assert_eq!(
        response.body().len(),
        expected.len(),
        "case truncated clip: a clip must stop at the first {MAX_CLIP_FRAGMENTS} fragments of the \
         range, got {} bytes for an archive of {held} fragments in range",
        response.body().len()
    );
    assert_eq!(
        response.body().as_ref(),
        expected.as_slice(),
        "case truncated clip: a clip must be the archive's own bytes for the FIRST \
         {MAX_CLIP_FRAGMENTS} fragments of the range, in order"
    );

    // The same range read through ListFragments still reports every fragment:
    // the ceiling is GetClip's own output limit, not a limit on the archive.
    let listed = harness
        .post(
            "/listFragments",
            &serde_json::json!({
                "StreamName": DEEP_STREAM,
                "MaxResults": 1000,
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + held as i64,
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        listed.status().as_u16(),
        200,
        "case the archive behind the clip: expected HTTP 200, got {}",
        listed.status()
    );
    assert_eq!(
        body_json(&listed)["Fragments"]
            .as_array()
            .map_or(0, Vec::len),
        held,
        "case the archive behind the clip: ListFragments must still report every fragment in range"
    );
}

/// RULE: a stream that retains no data is `NoDataRetentionException` (400) —
/// "Data retention must be greater than 0" — and a range the archive holds no
/// fragment in is `ResourceNotFoundException` (404).
#[tokio::test]
async fn get_clip_refuses_a_stream_or_range_it_cannot_clip() {
    let harness = seeded();
    let cases: Vec<(&str, &str, i64, i64, u16, &str)> = vec![
        (
            "a stream with no retention",
            NO_RETENTION_STREAM,
            0,
            1000,
            400,
            "NoDataRetentionException",
        ),
        (
            "a range before the archive",
            STREAM,
            -1000,
            -1,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a range after the archive",
            STREAM,
            5000,
            6000,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a range between two fragments",
            STREAM,
            1,
            9,
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, stream, start, end, status, error) in cases {
        let body = serde_json::json!({
            "StreamName": stream,
            "ClipFragmentSelector": {
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/getClip", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], error,
            "case {case}: expected {error}, got {reported:?}"
        );
    }
}

/// RULE: `GetHLSStreamingSessionURL` mints a session per call and RECORDS it —
/// its parameters, its expiry and the playlist snapshot it serves — so a second
/// call answers a different URL and both sessions exist in state.
#[tokio::test]
async fn get_hls_streaming_session_url_records_a_distinct_session_per_call() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "PlaybackMode": "ON_DEMAND",
        "Expires": 600,
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });

    let mut urls: Vec<String> = Vec::new();
    for call in 1..=2 {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case call {call}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        let url = reported["HLSStreamingSessionURL"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            url.starts_with(&format!("https://{EMULATOR_ENDPOINT}/hls/v1/")),
            "case call {call}: the URL must address the endpoint the caller reached this \
             emulator on, got {url:?}"
        );
        urls.push(url);
    }

    assert_ne!(
        urls[0], urls[1],
        "case two calls: each call must mint its own session, got {urls:?}"
    );

    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    assert_eq!(
        state.sessions.len(),
        2,
        "case two calls: both sessions must be recorded, got {:?}",
        state.sessions.keys().collect::<Vec<_>>()
    );
    for (token, session) in &state.sessions {
        assert!(
            urls.iter().any(|url| url.ends_with(token)),
            "case session {token}: no returned URL carries this session's token"
        );
        assert_eq!(
            session.stream_name, STREAM,
            "case session {token}: wrong stream, got {session:?}"
        );
        assert_eq!(
            session.expires_in_secs, 600,
            "case session {token}: Expires must be honoured, got {session:?}"
        );
        assert_eq!(
            epoch_millis(session.expires_at()) - epoch_millis(session.created_at),
            600 * 1000,
            "case session {token}: the expiry must be created_at + Expires, got {session:?}"
        );
        assert_eq!(
            session.fragment_numbers,
            (0..SEEDED_FRAGMENTS).map(fragment_number).collect::<Vec<_>>(),
            "case session {token}: the playlist snapshot must be the fragments in range"
        );
    }
}

/// One row of the `GetHLSStreamingSessionURL` defaults table: the case name,
/// the selector type, the requested `MaxMediaPlaylistFragmentResults`, the
/// `DiscontinuityMode` the defaults imply, and the playlist snapshot expected.
type HlsDefaultsRow = (&'static str, &'static str, Option<u64>, &'static str, Vec<u64>);

/// RULE: the session store is bounded at the site that grows it — a session
/// whose `Expires` window the clock has already closed is dropped when the next
/// session is recorded, and live sessions never exceed `MAX_STREAMING_SESSIONS`.
#[tokio::test]
async fn minting_a_session_bounds_the_session_store() {
    use archived_media::state::{
        MAX_STREAMING_SESSIONS, SessionProtocol, StreamingSession,
    };

    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "PlaybackMode": "ON_DEMAND",
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });

    // A session recorded long enough ago that its own Expires window has
    // closed, beside one that is still live.
    let now = SystemTime::now();
    let session_at = |token: &str, created_at: SystemTime, expires_in_secs: u32| StreamingSession {
        token: token.to_string(),
        stream_name: STREAM.to_string(),
        protocol: SessionProtocol::Hls,
        playback_mode: "ON_DEMAND".to_string(),
        container_format: "FRAGMENTED_MP4".to_string(),
        discontinuity_mode: Some("NEVER".to_string()),
        display_fragment_timestamp: "NEVER".to_string(),
        display_fragment_number: None,
        fragment_numbers: Vec::new(),
        created_at,
        expires_in_secs,
    };
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        state.record_session(session_at("expired", now - Duration::from_secs(3600), 300));
        state.record_session(session_at("live", now, 43200));
    }

    let response = harness.post("/getHLSStreamingSessionURL", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "expected HTTP 200, got {}",
        response.status()
    );

    {
        let state = service_state(&*harness.engine).expect("the service store");
        let state = state.lock();
        assert!(
            !state.sessions.contains_key("expired"),
            "case expired session: a session past its Expires window must be dropped, held {:?}",
            state.sessions.keys().collect::<Vec<_>>()
        );
        assert!(
            state.sessions.contains_key("live"),
            "case live session: a session inside its Expires window must be kept, held {:?}",
            state.sessions.keys().collect::<Vec<_>>()
        );
    }

    // And live sessions alone cannot grow the store without end: recording
    // more than the ceiling keeps the ceiling and drops the oldest first.
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        for ordinal in 0..(MAX_STREAMING_SESSIONS + 16) {
            let created_at = now + Duration::from_millis(ordinal as u64);
            state.record_session(session_at(&format!("live-{ordinal}"), created_at, 43200));
        }
        assert!(
            state.sessions.len() <= MAX_STREAMING_SESSIONS,
            "case over the ceiling: the store holds {} sessions, over the {MAX_STREAMING_SESSIONS} \
             ceiling",
            state.sessions.len()
        );
        assert!(
            !state.sessions.contains_key("live-0"),
            "case over the ceiling: the oldest session must be the one evicted"
        );
        assert!(
            state.sessions.contains_key(&format!("live-{}", MAX_STREAMING_SESSIONS + 15)),
            "case over the ceiling: the newest session must be kept"
        );
    }
}

/// RULE: the documented parameter defaults are what an unspecified member
/// means — `Expires` 300, `ContainerFormat` FRAGMENTED_MP4,
/// `DisplayFragmentTimestamp` NEVER, and `DiscontinuityMode` ALWAYS for
/// SERVER_TIMESTAMP / NEVER for PRODUCER_TIMESTAMP — and
/// `MaxMediaPlaylistFragmentResults` caps the playlist snapshot.
#[tokio::test]
async fn get_hls_streaming_session_url_applies_the_documented_defaults() {
    // (case, selector type, max results, expected discontinuity mode, snapshot)
    let cases: Vec<HlsDefaultsRow> = vec![
        (
            "producer timestamps, no cap",
            "PRODUCER_TIMESTAMP",
            None,
            "NEVER",
            vec![0, 1, 2, 3, 4],
        ),
        (
            "server timestamps, no cap",
            "SERVER_TIMESTAMP",
            None,
            "ALWAYS",
            vec![0, 1, 2, 3, 4],
        ),
        (
            "server timestamps, capped at two",
            "SERVER_TIMESTAMP",
            Some(2),
            "ALWAYS",
            vec![0, 1],
        ),
        (
            "producer timestamps, capped at one",
            "PRODUCER_TIMESTAMP",
            Some(1),
            "NEVER",
            vec![0],
        ),
    ];

    for (case, selector_type, max_results, discontinuity, snapshot) in cases {
        let harness = seeded();
        let mut body = serde_json::json!({
            "StreamName": STREAM,
            "PlaybackMode": "ON_DEMAND",
            "HLSFragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
            },
        });
        if let Some(max) = max_results {
            body["MaxMediaPlaylistFragmentResults"] = serde_json::json!(max);
        }
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        let state = service_state(&*harness.engine).expect("the service store");
        let state = state.lock();
        let session = state
            .sessions
            .values()
            .next()
            .expect("the minted session is recorded");
        assert_eq!(
            session.expires_in_secs, 300,
            "case {case}: the default Expires is 300 seconds, got {session:?}"
        );
        assert_eq!(
            session.container_format, "FRAGMENTED_MP4",
            "case {case}: the default ContainerFormat is FRAGMENTED_MP4, got {session:?}"
        );
        assert_eq!(
            session.display_fragment_timestamp, "NEVER",
            "case {case}: the default DisplayFragmentTimestamp is NEVER, got {session:?}"
        );
        assert_eq!(
            session.discontinuity_mode.as_deref(),
            Some(discontinuity),
            "case {case}: wrong default DiscontinuityMode, got {session:?}"
        );
        assert_eq!(
            session.fragment_numbers,
            snapshot.iter().copied().map(fragment_number).collect::<Vec<_>>(),
            "case {case}: wrong playlist snapshot, got {session:?}"
        );
    }
}

/// RULE: a session that would serve nothing does not exist —
/// `ResourceNotFoundException` (404) for ON_DEMAND / LIVE_REPLAY with no
/// fragment in the requested range, and for LIVE with no fragment in the last
/// 30 seconds. A LIVE session over a freshly ingested archive is minted, and
/// serves the most recent fragments up to the maximum.
#[tokio::test]
async fn get_hls_streaming_session_url_needs_fragments_to_serve() {
    let harness = seeded();
    let empty_range = serde_json::json!({
        "StartTimestamp": T0_SECS - 1000,
        "EndTimestamp": T0_SECS - 1,
    });
    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "ON_DEMAND outside the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": empty_range,
                },
            }),
            404,
        ),
        (
            "LIVE_REPLAY outside the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": empty_range,
                },
            }),
            404,
        ),
        (
            "LIVE over an archive older than the live window",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE" }),
            404,
        ),
        (
            "LIVE over a freshly ingested archive",
            serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            200,
        ),
    ];

    for (case, body, status) in cases {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 404 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {reported:?}"
            );
        }
    }

    // The LIVE session that was minted serves the newest fragments, capped by
    // the documented LIVE default of 5.
    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    let session = state
        .sessions
        .values()
        .find(|session| session.stream_name == LIVE_STREAM)
        .expect("the LIVE session is recorded");
    let newest: Vec<String> = ((LIVE_FRAGMENTS - 5)..LIVE_FRAGMENTS)
        .map(fragment_number)
        .collect();
    assert_eq!(
        session.fragment_numbers, newest,
        "case LIVE snapshot: a LIVE playlist carries the 5 most recent fragments, got {session:?}"
    );
}

/// RULE: the playback mode decides which selector a session may carry —
/// `HLSFragmentSelector` with a `TimestampRange` is required for ON_DEMAND and
/// LIVE_REPLAY, `EndTimestamp` is required for ON_DEMAND and optional for
/// LIVE_REPLAY, and a LIVE session must carry no `TimestampRange`.
#[tokio::test]
async fn get_hls_streaming_session_url_enforces_the_selector_rules_of_each_mode() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "ON_DEMAND with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "ON_DEMAND" }),
            400,
        ),
        (
            "LIVE_REPLAY with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE_REPLAY" }),
            400,
        ),
        (
            "ON_DEMAND with no EndTimestamp",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            400,
        ),
        (
            "LIVE_REPLAY with no EndTimestamp",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            200,
        ),
        (
            "LIVE with a TimestampRange",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            400,
        ),
        (
            "ON_DEMAND with an end that precedes the start",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 10,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
            400,
        ),
    ];

    for (case, body, status) in cases {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 400 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {reported:?}"
            );
        }
    }
}

/// RULE: a stream that retains no data cannot have a streaming session —
/// `NoDataRetentionException` (400).
#[tokio::test]
async fn get_hls_streaming_session_url_refuses_a_stream_with_no_retention() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": NO_RETENTION_STREAM,
        "PlaybackMode": "ON_DEMAND",
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });
    let response = harness.post("/getHLSStreamingSessionURL", &body).await;
    assert_eq!(
        response.status().as_u16(),
        400,
        "expected HTTP 400, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let reported = body_json(&response);
    assert_eq!(
        reported["__type"], "NoDataRetentionException",
        "expected NoDataRetentionException, got {reported:?}"
    );
}

/// Every operation of this service is implemented, so none of them answers
/// 501: `GetDASHStreamingSessionURL`, the last scaffold, is exercised in depth
/// by the `get_dash_streaming_session_url` suite.
#[tokio::test]
async fn no_operation_of_this_service_answers_not_implemented() {
    let harness = seeded();
    let named = serde_json::json!({ "StreamName": STREAM });

    for (path, body) in read_loop_operations() {
        let response = harness.post(path, &with_members(&body, &named)).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {path}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

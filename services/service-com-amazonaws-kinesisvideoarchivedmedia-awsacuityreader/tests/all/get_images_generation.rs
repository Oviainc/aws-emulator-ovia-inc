// SPDX-License-Identifier: CC0-1.0

//! `GetImages` over the fragment archive, end to end over HTTP.
//!
//! Every assertion here is about a RULE, asserted over a set of inputs rather than
//! over one request:
//!
//! 1. a requested window the addressed stream's archive holds no fragment in is
//!    refused WHOLESALE — HTTP 404, `ResourceNotFoundException`, one fixed sentence
//!    — and because that refusal is a function of the archive and the window alone,
//!    identical calls in flight together are answered identically;
//! 2. the images are the window's sampled instants: its start, then one per
//!    `SamplingInterval`, at most 25 to a page, with a `NextToken` while more remain;
//! 3. each image is generated FROM the covering fragment's archived bytes, in the
//!    requested `Format` at the requested size, and reports the modelled
//!    per-image `ImageError` when it cannot be — `NO_MEDIA` for an instant no
//!    fragment covers, `MEDIA_ERROR` for a fragment whose bytes carry no frame;
//! 4. a request outside the documented window, interval and format-config rules is
//!    refused with the modelled client error, never with an image list.
//!
//! The harness is `archived_media_read_path`'s — the real `MultiServiceRouter` with
//! this one service registered — with this suite's own streams seeded into the same
//! engine through the service's state API.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;

use archived_media::state::{from_epoch_millis, service_state, ArchivedStream, StoredFragment};

use crate::archived_media_read_path::{body_json, seeded, Harness};

/// The region this suite's streams are created in.
const REGION: &str = "us-east-1";

/// The stream whose fragments carry a real frame each.
pub(crate) const IMAGES_STREAM: &str = "get-images-stream";
/// A stream whose single fragment's archived bytes carry no frame at all.
const OPAQUE_STREAM: &str = "get-images-opaque-stream";
/// A retention-enabled stream nothing has ever been ingested into.
pub(crate) const EMPTY_STREAM: &str = "get-images-empty-stream";
/// A stream with `DataRetentionInHours` of 0.
const NO_RETENTION_STREAM: &str = "get-images-no-retention-stream";

/// The base of this suite's archive: an instant well in the past, so no window here
/// depends on the wall clock.
const T0_SECS: i64 = 1_761_000_000;
/// How far apart the seeded fragments start. Wider than their duration, so the
/// archive has GAPS no fragment covers.
const FRAGMENT_SPACING_SECS: i64 = 10;
/// How long each seeded fragment plays, in milliseconds.
const FRAGMENT_DURATION_MILLIS: u64 = 2_000;
/// How many fragments `IMAGES_STREAM` holds.
const SEEDED_FRAGMENTS: i64 = 3;
/// How far a seeded fragment's server timestamp sits from its producer timestamp, so
/// selecting on one field cannot pass by accident when the other was meant.
const SERVER_OFFSET_SECS: i64 = 600;
/// A realistic Kinesis Video fragment number: a long, increasing decimal.
const FRAGMENT_BASE: u64 = 9_134_385_233_318_143_200;

/// The size of the frame the seeded fragments of `IMAGES_STREAM` carry.
pub(crate) const FRAME_WIDTH: u32 = 40;
pub(crate) const FRAME_HEIGHT: u32 = 20;

/// A stream whose one fragment carries a frame of an extreme aspect ratio, so a single
/// requested side derives an enormous other side.
const TALL_FRAME_STREAM: &str = "get-images-tall-frame-stream";
/// The size of that frame: one pixel wide.
const TALL_FRAME_WIDTH: u32 = 1;
const TALL_FRAME_HEIGHT: u32 = 2_048;

/// The largest `WidthPixels` the model admits (`WidthPixels` range 1..3840).
pub(crate) const MAX_WIDTH_PIXELS: u32 = 3_840;
/// The largest `HeightPixels` the model admits (`HeightPixels` range 1..2160).
pub(crate) const MAX_HEIGHT_PIXELS: u32 = 2_160;

/// The refusal a window the archive holds no fragment in is answered with.
pub(crate) const NO_FRAGMENTS: &str = "No fragments found in the stream for the GetImages request.";
/// The refusal a request naming only ONE of the two output sides is answered with.
pub(crate) const INCOMPLETE_RESIZE: &str =
    "Height and width for the images need to be provided or none.";
/// The code every refused request bound carries (`InvalidArgumentException`, HTTP 400).
pub(crate) const INVALID_ARGUMENT: &str = "InvalidArgumentException";
/// The refusal a window that does not run forwards is answered with.
const RANGE_ORDER: &str = "StartTimestamp must be before EndTimestamp.";
/// The refusal a window wider than the documented 300 seconds is answered with.
pub(crate) const WINDOW_TOO_WIDE: &str =
    "EndTimestamp must be within 300 seconds of StartTimestamp.";
/// The refusal a sampling interval under the documented minimum is answered with.
const INTERVAL_TOO_SMALL: &str = "SamplingInterval must be at least 200 ms.";
/// The refusal a `JPEGQuality` outside the documented 1..100 is answered with.
const QUALITY_OUT_OF_RANGE: &str = "The JPEGQuality value must be an integer from 1 to 100.";

/// How many times a racing caller re-schedules while waiting for its peers.
const MAX_RENDEZVOUS_SPINS: usize = 10_000;

/// The producer timestamp of the `index`-th seeded fragment, in epoch seconds.
pub(crate) fn producer_secs(index: i64) -> i64 {
    T0_SECS + FRAGMENT_SPACING_SECS * index
}

/// The server timestamp of the `index`-th seeded fragment, in epoch seconds.
fn server_secs(index: i64) -> i64 {
    producer_secs(index) + SERVER_OFFSET_SECS
}

/// The frame every seeded fragment carries, as PNG bytes.
///
/// A gradient rather than a flat fill, so a lossy re-encoding of it differs in size
/// with the quality it was encoded at — a flat image compresses to about the same
/// bytes whatever the quality, and would assert nothing about `JPEGQuality`.
fn frame_png(width: u32, height: u32) -> Vec<u8> {
    let frame = ::image::RgbaImage::from_fn(width, height, |x, y| {
        let red = u8::try_from((x * 251) % 256).expect("a byte");
        let green = u8::try_from((y * 197) % 256).expect("a byte");
        let blue = u8::try_from((x * y * 131) % 256).expect("a byte");
        ::image::Rgba([red, green, blue, 255])
    });
    let mut encoded: Vec<u8> = Vec::new();
    ::image::DynamicImage::ImageRgba8(frame)
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            ::image::ImageFormat::Png,
        )
        .expect("the seed frame encodes as PNG");
    encoded
}

/// The harness of `archived_media_read_path`, with this suite's four streams seeded
/// into the same engine.
pub(crate) fn harness() -> Harness {
    let harness = seeded();
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        let created_at = from_epoch_millis(T0_SECS * 1000);
        state.put_stream(ArchivedStream::new(IMAGES_STREAM, REGION, created_at, 24));
        state.put_stream(ArchivedStream::new(OPAQUE_STREAM, REGION, created_at, 24));
        state.put_stream(ArchivedStream::new(EMPTY_STREAM, REGION, created_at, 1));
        state.put_stream(ArchivedStream::new(
            TALL_FRAME_STREAM,
            REGION,
            created_at,
            24,
        ));
        state.put_stream(ArchivedStream::new(
            NO_RETENTION_STREAM,
            REGION,
            created_at,
            0,
        ));

        for index in 0..SEEDED_FRAGMENTS {
            let number = FRAGMENT_BASE + u64::try_from(index).expect("a small index");
            assert!(
                state.append_fragment(
                    IMAGES_STREAM,
                    StoredFragment::new(
                        number.to_string(),
                        from_epoch_millis(producer_secs(index) * 1000),
                        from_epoch_millis(server_secs(index) * 1000),
                        FRAGMENT_DURATION_MILLIS,
                        frame_png(FRAME_WIDTH, FRAME_HEIGHT),
                    ),
                ),
                "case seed fragment {index}: the seeded stream must exist"
            );
        }

        assert!(
            state.append_fragment(
                OPAQUE_STREAM,
                StoredFragment::new(
                    FRAGMENT_BASE.to_string(),
                    from_epoch_millis(producer_secs(0) * 1000),
                    from_epoch_millis(server_secs(0) * 1000),
                    FRAGMENT_DURATION_MILLIS,
                    b"these archived bytes carry no frame".to_vec(),
                ),
            ),
            "the opaque stream must exist"
        );

        assert!(
            state.append_fragment(
                TALL_FRAME_STREAM,
                StoredFragment::new(
                    FRAGMENT_BASE.to_string(),
                    from_epoch_millis(producer_secs(0) * 1000),
                    from_epoch_millis(server_secs(0) * 1000),
                    FRAGMENT_DURATION_MILLIS,
                    frame_png(TALL_FRAME_WIDTH, TALL_FRAME_HEIGHT),
                ),
            ),
            "the tall-frame stream must exist"
        );
    }
    harness
}

/// The `__type` and message of a refusal.
pub(crate) fn reported(response: &http::Response<bytes::Bytes>) -> (String, String) {
    let body = body_json(response);
    let code = body["__type"].as_str().unwrap_or_default().to_string();
    let message = body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// A `GetImages` request body naming `stream`, over `[start_secs, end_secs]`.
pub(crate) fn request(
    stream: &str,
    selector: &str,
    start_secs: i64,
    end_secs: i64,
    format: &str,
) -> serde_json::Value {
    serde_json::json!({
        "StreamName": stream,
        "ImageSelectorType": selector,
        "StartTimestamp": start_secs,
        "EndTimestamp": end_secs,
        "Format": format,
    })
}

/// `base` with every member of `extra` added.
pub(crate) fn with(base: &serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().expect("an object of members") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(object)
}

/// The images of a 200 answer.
pub(crate) fn images(response: &http::Response<bytes::Bytes>) -> Vec<serde_json::Value> {
    body_json(response)["Images"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The epoch milliseconds an `Image$TimeStamp` reports.
fn image_millis(image: &serde_json::Value) -> i64 {
    let seconds = image["TimeStamp"].as_f64().unwrap_or_default();
    (seconds * 1000.0).round() as i64
}

/// The bytes an `Image$ImageContent` carries.
pub(crate) fn image_bytes(image: &serde_json::Value) -> Vec<u8> {
    let content = image["ImageContent"].as_str().unwrap_or_default();
    ::aws_smithy_types::base64::decode(content).unwrap_or_default()
}

/// RULE: a window the addressed stream's archive holds no fragment in is refused
/// wholesale — HTTP 404, `ResourceNotFoundException`, the one fixed sentence — never
/// answered 200 with a list of absence markers.
///
/// The rows span what "holds no fragment in the window" generalises over: an archive
/// that is empty, a window before the archive, a window after it, a window inside a
/// gap between fragments, and a window read on the OTHER timestamp field, plus the
/// members a request may carry alongside (format, size, sampling interval, page size)
/// — none of which changes the refusal.
#[tokio::test]
async fn a_window_with_no_fragments_is_refused_wholesale() {
    let harness = harness();
    let last = SEEDED_FRAGMENTS - 1;
    // (case, stream, selector, start secs, end secs, extra members)
    let rows: Vec<(&str, &str, &str, i64, i64, serde_json::Value)> = vec![
        (
            "a retention-enabled stream nothing was ingested into",
            EMPTY_STREAM,
            "PRODUCER_TIMESTAMP",
            T0_SECS - 10,
            T0_SECS,
            serde_json::json!({ "SamplingInterval": 1000 }),
        ),
        (
            "the same stream read on the server timestamp, as PNG",
            EMPTY_STREAM,
            "SERVER_TIMESTAMP",
            T0_SECS,
            T0_SECS + 300,
            serde_json::json!({ "Format": "PNG" }),
        ),
        (
            "a window entirely before the archive, with both pixel sizes",
            IMAGES_STREAM,
            "PRODUCER_TIMESTAMP",
            T0_SECS - 100,
            T0_SECS - 1,
            serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
        ),
        (
            "a window entirely after the archive, at the largest page size",
            IMAGES_STREAM,
            "PRODUCER_TIMESTAMP",
            producer_secs(last) + 100,
            producer_secs(last) + 200,
            serde_json::json!({ "MaxResults": 100 }),
        ),
        (
            "a window inside a gap between two fragments",
            IMAGES_STREAM,
            "PRODUCER_TIMESTAMP",
            producer_secs(0) + 3,
            producer_secs(1) - 1,
            serde_json::json!({ "SamplingInterval": 200 }),
        ),
        (
            "the producer window read on the server timestamp field",
            IMAGES_STREAM,
            "SERVER_TIMESTAMP",
            producer_secs(0),
            producer_secs(last),
            serde_json::json!({}),
        ),
    ];

    for (case, stream, selector, start, end, extra) in rows {
        let body = with(&request(stream, selector, start, end, "JPEG"), extra);
        let response = harness.post("/getImages", &body).await;
        let (code, message) = reported(&response);
        assert_eq!(
            response.status().as_u16(),
            404,
            "case {case:?}: expected HTTP 404, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?}"
        );
        assert_eq!(
            message, NO_FRAGMENTS,
            "case {case:?}: expected {NO_FRAGMENTS:?}, got {message:?}"
        );
    }
}

/// RULE: the answer is a function of the archive and the requested window, so N
/// identical calls in flight together are answered identically — both when the window
/// holds no fragment (every caller sees the same refusal) and when it does (every
/// caller sees the same images).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_identical_calls_are_answered_identically() {
    let harness = Arc::new(harness());
    // (case, the request every caller makes, the status they all expect)
    let rows: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "a window the archive holds nothing in",
            with(
                &request(
                    EMPTY_STREAM,
                    "PRODUCER_TIMESTAMP",
                    T0_SECS - 10,
                    T0_SECS,
                    "JPEG",
                ),
                serde_json::json!({ "SamplingInterval": 1000 }),
            ),
            404,
        ),
        (
            "a window the archive holds fragments in",
            with(
                &request(
                    IMAGES_STREAM,
                    "PRODUCER_TIMESTAMP",
                    producer_secs(0),
                    producer_secs(0) + 1,
                    "JPEG",
                ),
                serde_json::json!({ "SamplingInterval": 500 }),
            ),
            200,
        ),
    ];

    let callers = 8usize;
    for (case, body, expected_status) in rows {
        let ready = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::with_capacity(callers);
        for _ in 0..callers {
            let harness = Arc::clone(&harness);
            let ready = Arc::clone(&ready);
            let body = body.clone();
            tasks.push(tokio::spawn(async move {
                // Announce, then re-schedule until every peer has announced too, so
                // the calls reach the router together rather than one after another.
                ready.fetch_add(1, Ordering::SeqCst);
                let mut spins = 0;
                while ready.load(Ordering::SeqCst) < callers && spins < MAX_RENDEZVOUS_SPINS {
                    spins += 1;
                    tokio::task::yield_now().await;
                }
                let response = harness.post("/getImages", &body).await;
                (response.status().as_u16(), body_json(&response))
            }));
        }

        let mut answers = Vec::with_capacity(callers);
        for task in tasks {
            answers.push(task.await.expect("the racing call ran to completion"));
        }

        let first = answers.first().cloned().expect("a first answer");
        for (index, (status, payload)) in answers.iter().enumerate() {
            assert_eq!(
                *status, expected_status,
                "case {case:?}: caller {index} expected HTTP {expected_status}, got {status} \
                 with body {payload:?}"
            );
            assert_eq!(
                (*status, payload.clone()),
                first,
                "case {case:?}: caller {index} was answered {payload:?}, the first caller \
                 {:?}",
                first.1
            );
        }
    }
}

/// One row of the sampling table: a window, how it is sampled and paged, and how many
/// images that leaves on the first page.
struct SamplingCase {
    /// What this row is about.
    case: &'static str,
    /// The window's end, in epoch seconds.
    end_secs: i64,
    /// The `SamplingInterval` the request names, in milliseconds.
    interval_millis: Option<i64>,
    /// The `MaxResults` the request names.
    max_results: Option<i64>,
    /// How many images the first page carries.
    expected_count: usize,
    /// Whether a `NextToken` comes back with it.
    more_pages: bool,
}

/// RULE: the images are the window's sampled instants — its start, then one every
/// `SamplingInterval` milliseconds while the window lasts, at most `MaxResults` and
/// never more than the documented 25 to a page, with a `NextToken` exactly while
/// sampled instants remain.
///
/// The rows span the interval (absent, the documented minimum, a wide one, one wider
/// than the window), the window length, and the page size (absent, small, over the
/// ceiling).
#[tokio::test]
async fn the_images_are_the_windows_sampled_instants() {
    let harness = harness();
    let from = producer_secs(0);
    let rows = vec![
        SamplingCase {
            case: "no interval named: the start instant alone",
            end_secs: from + 1,
            interval_millis: None,
            max_results: None,
            expected_count: 1,
            more_pages: false,
        },
        SamplingCase {
            case: "a 500 ms interval over a 1 s window",
            end_secs: from + 1,
            interval_millis: Some(500),
            max_results: None,
            expected_count: 3,
            more_pages: false,
        },
        SamplingCase {
            case: "the documented 200 ms minimum over 1 s",
            end_secs: from + 1,
            interval_millis: Some(200),
            max_results: None,
            expected_count: 6,
            more_pages: false,
        },
        SamplingCase {
            case: "an interval wider than the window: the start alone",
            end_secs: from + 1,
            interval_millis: Some(5_000),
            max_results: None,
            expected_count: 1,
            more_pages: false,
        },
        SamplingCase {
            case: "a page of 2 of the 3 sampled instants",
            end_secs: from + 1,
            interval_millis: Some(500),
            max_results: Some(2),
            expected_count: 2,
            more_pages: true,
        },
        SamplingCase {
            case: "a page size over the 25-image ceiling",
            end_secs: from + 20,
            interval_millis: Some(200),
            max_results: Some(100),
            expected_count: 25,
            more_pages: true,
        },
        SamplingCase {
            case: "no page size: the same 25-image ceiling",
            end_secs: from + 20,
            interval_millis: Some(200),
            max_results: None,
            expected_count: 25,
            more_pages: true,
        },
    ];

    for row in rows {
        let SamplingCase {
            case,
            end_secs: end,
            interval_millis: interval,
            max_results,
            expected_count,
            more_pages: more,
        } = row;
        let mut body = request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", from, end, "JPEG");
        if let Some(interval) = interval {
            body = with(&body, serde_json::json!({ "SamplingInterval": interval }));
        }
        if let Some(max_results) = max_results {
            body = with(&body, serde_json::json!({ "MaxResults": max_results }));
        }
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let page = images(&response);
        assert_eq!(
            page.len(),
            expected_count,
            "case {case:?}: expected {expected_count} images, got {}",
            page.len()
        );
        // Each image is stamped at the instant it was generated for: the window's
        // start, then one per interval.
        let step = interval.unwrap_or_default();
        for (index, image) in page.iter().enumerate() {
            let expected = from * 1000 + step * i64::try_from(index).expect("a small index");
            let actual = image_millis(image);
            assert_eq!(
                actual, expected,
                "case {case:?}: image {index} expected to be stamped {expected} ms from the \
                 epoch, got {actual}"
            );
        }
        let next_token = body_json(&response)["NextToken"].as_str().map(str::to_string);
        assert_eq!(
            next_token.is_some(),
            more,
            "case {case:?}: expected a NextToken? {more}, got {next_token:?}"
        );
    }
}

/// RULE: a `NextToken` continues the same sampling where the page before it stopped,
/// and the pages together are exactly the window's sampled instants, each stamped once.
#[tokio::test]
async fn the_pages_of_one_window_are_its_instants_once_each() {
    let harness = harness();
    let from = producer_secs(0);
    // Two page sizes over one window, so the rule is asserted over more than one
    // partition of the same instants.
    for page_size in [1i64, 2, 3] {
        let base = with(
            &request(
                IMAGES_STREAM,
                "PRODUCER_TIMESTAMP",
                from,
                from + 2,
                "JPEG",
            ),
            serde_json::json!({ "SamplingInterval": 500, "MaxResults": page_size }),
        );
        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                None => base.clone(),
                Some(token) => with(&base, serde_json::json!({ "NextToken": token })),
            };
            let response = harness.post("/getImages", &body).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case page size {page_size}: expected HTTP 200, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let page = images(&response);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a small page size"),
                "case page size {page_size}: expected at most {page_size} images, got {}",
                page.len()
            );
            seen.extend(page.iter().map(image_millis));
            token = body_json(&response)["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= 16,
                "case page size {page_size}: expected the pages to end, got {pages} of them"
            );
            if token.is_none() {
                break;
            }
        }
        let expected: Vec<i64> = (0..5).map(|index| from * 1000 + 500 * index).collect();
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected the instants {expected:?}, got {seen:?}"
        );
    }
}

/// One row of the image-form table: the format and size a request asks for, and the
/// image that should come back.
struct FormCase {
    /// What this row is about.
    case: &'static str,
    /// The `Format` the request names.
    format: &'static str,
    /// The `WidthPixels` the request names.
    width: Option<u32>,
    /// The `HeightPixels` the request names.
    height: Option<u32>,
    /// The size the generated image decodes at.
    expected: (u32, u32),
}

/// RULE: an image is generated FROM the covering fragment's archived bytes, encoded in
/// the requested `Format` at the requested size — both sides stretch the frame, one
/// side alone keeps its ratio, neither keeps the archived frame's own size.
///
/// The rows span the format and every form the two size members can take.
#[tokio::test]
async fn an_image_is_the_archived_frame_in_the_requested_form() {
    let harness = harness();
    let at = producer_secs(1);
    let rows = vec![
        FormCase {
            case: "JPEG at the archived frame's own size",
            format: "JPEG",
            width: None,
            height: None,
            expected: (FRAME_WIDTH, FRAME_HEIGHT),
        },
        FormCase {
            case: "PNG at the archived frame's own size",
            format: "PNG",
            width: None,
            height: None,
            expected: (FRAME_WIDTH, FRAME_HEIGHT),
        },
        FormCase {
            case: "both sides given: stretched to that ratio",
            format: "JPEG",
            width: Some(FRAME_WIDTH * 2),
            height: Some(FRAME_HEIGHT / 2),
            expected: (FRAME_WIDTH * 2, FRAME_HEIGHT / 2),
        },
        FormCase {
            case: "both sides given, as PNG",
            format: "PNG",
            width: Some(FRAME_WIDTH * 3),
            height: Some(FRAME_HEIGHT * 3),
            expected: (FRAME_WIDTH * 3, FRAME_HEIGHT * 3),
        },
    ];
    // One side ALONE is not a row here: `WidthPixels` documents that such a request is
    // refused, and the refusal is pinned in `get_images_request_bounds`.

    for row in rows {
        let FormCase {
            case,
            format,
            width,
            height,
            expected: (expected_width, expected_height),
        } = row;
        let mut body = request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", at, at + 1, format);
        if let Some(width) = width {
            body = with(&body, serde_json::json!({ "WidthPixels": width }));
        }
        if let Some(height) = height {
            body = with(&body, serde_json::json!({ "HeightPixels": height }));
        }
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let page = images(&response);
        let first = page.first().cloned().unwrap_or_default();
        assert_eq!(
            first["Error"],
            serde_json::Value::Null,
            "case {case:?}: expected no per-image error, got {first:?}"
        );
        let bytes = image_bytes(&first);
        let decoded = ::image::load_from_memory(&bytes);
        let decoded = decoded.unwrap_or_else(|error| {
            panic!("case {case:?}: expected a decodable image, got {error}");
        });
        let expected_format = if format == "JPEG" {
            ::image::ImageFormat::Jpeg
        } else {
            ::image::ImageFormat::Png
        };
        let actual_format = ::image::guess_format(&bytes).ok();
        assert_eq!(
            actual_format,
            Some(expected_format),
            "case {case:?}: expected a {expected_format:?} image, got {actual_format:?}"
        );
        assert_eq!(
            (decoded.width(), decoded.height()),
            (expected_width, expected_height),
            "case {case:?}: expected a {expected_width}x{expected_height} image, got {}x{}",
            decoded.width(),
            decoded.height()
        );
    }
}

/// How many bytes the generated JPEG of the fragment at `at` takes when the request
/// names `quality` as its `JPEGQuality` (or names none).
async fn encoded_jpeg_len(harness: &Harness, at: i64, quality: Option<&str>) -> usize {
    let mut body = request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", at, at + 1, "JPEG");
    if let Some(quality) = quality {
        body = with(
            &body,
            serde_json::json!({ "FormatConfig": { "JPEGQuality": quality } }),
        );
    }
    let response = harness.post("/getImages", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case quality {quality:?}: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let page = images(&response);
    image_bytes(&page.first().cloned().unwrap_or_default()).len()
}

/// RULE: `FormatConfig`'s `JPEGQuality` is honoured — "If the value is 1, the image
/// will be generated with less quality and the best compression. If the value is 100,
/// the image will be generated with the best quality and less compression." — so the
/// same archived frame encoded at the published bounds comes back at different sizes.
#[tokio::test]
async fn the_jpeg_quality_of_the_generated_image_is_the_requested_one() {
    let harness = harness();
    let at = producer_secs(1);

    // The published bounds of the member, and the default the documentation names.
    let worst = encoded_jpeg_len(&harness, at, Some("1")).await;
    let best = encoded_jpeg_len(&harness, at, Some("100")).await;
    let defaulted = encoded_jpeg_len(&harness, at, None).await;
    assert!(
        worst > 0 && best > worst,
        "case the published quality bounds: expected quality 100 to encode to more bytes than \
         quality 1, got {best} and {worst}"
    );
    assert!(
        defaulted >= worst && defaulted <= best,
        "case the default quality: expected the default (80) to encode between the bounds \
         {worst} and {best}, got {defaulted}"
    );
}

/// RULE: an image that could not be generated reports the modelled `ImageError` and no
/// content — `NO_MEDIA` for a sampled instant no fragment covers, `MEDIA_ERROR` for an
/// instant whose covering fragment carries no frame — while the window itself, holding
/// fragments, is still a success.
#[tokio::test]
async fn an_image_that_cannot_be_generated_reports_the_modelled_error() {
    let harness = harness();
    let from = producer_secs(0);
    // One window spanning a fragment and the gap after it, sampled every second: the
    // instants inside the fragment's 2 s carry an image, the rest report NO_MEDIA.
    let body = with(
        &request(
            IMAGES_STREAM,
            "PRODUCER_TIMESTAMP",
            from,
            from + 5,
            "JPEG",
        ),
        serde_json::json!({ "SamplingInterval": 1000 }),
    );
    let response = harness.post("/getImages", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case a window spanning a fragment and a gap: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let page = images(&response);
    assert_eq!(
        page.len(),
        6,
        "case a window spanning a fragment and a gap: expected 6 images, got {}",
        page.len()
    );
    for (index, image) in page.iter().enumerate() {
        let covered = (index as u64) * 1000 < FRAGMENT_DURATION_MILLIS;
        if covered {
            assert_eq!(
                image["Error"],
                serde_json::Value::Null,
                "case instant {index}: expected an image, got {image:?}"
            );
            assert!(
                !image_bytes(image).is_empty(),
                "case instant {index}: expected image content, got {image:?}"
            );
        } else {
            assert_eq!(
                image["Error"].as_str(),
                Some("NO_MEDIA"),
                "case instant {index}: expected NO_MEDIA, got {image:?}"
            );
            assert_eq!(
                image["ImageContent"],
                serde_json::Value::Null,
                "case instant {index}: expected no image content, got {image:?}"
            );
        }
    }

    // A fragment whose archived bytes carry no frame: the instants it covers report
    // MEDIA_ERROR, which is a different claim from "no media was available".
    let body = with(
        &request(
            OPAQUE_STREAM,
            "PRODUCER_TIMESTAMP",
            from,
            from + 1,
            "PNG",
        ),
        serde_json::json!({ "SamplingInterval": 500 }),
    );
    let response = harness.post("/getImages", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case a fragment carrying no frame: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let page = images(&response);
    assert_eq!(
        page.len(),
        3,
        "case a fragment carrying no frame: expected 3 images, got {}",
        page.len()
    );
    for (index, image) in page.iter().enumerate() {
        assert_eq!(
            image["Error"].as_str(),
            Some("MEDIA_ERROR"),
            "case opaque instant {index}: expected MEDIA_ERROR, got {image:?}"
        );
        assert_eq!(
            image["ImageContent"],
            serde_json::Value::Null,
            "case opaque instant {index}: expected no image content, got {image:?}"
        );
    }
}

/// RULE: the image a request asks for is generated at the size it names, inside this
/// emulator's own frame budget.
///
/// The rows differ along the one thing the budget generalises over: how large the
/// requested size is — the archived frame's own ratio scaled up, and the two modelled
/// side maxima together. A size PAST the budget is not reachable from a request at all,
/// because the modelled side ranges (1..3840 by 1..2160) multiply to fewer pixels than
/// the budget allows; the refusal arm is exercised against the generation helper itself
/// in that handler's own unit tests.
#[tokio::test]
async fn a_frame_is_generated_at_the_requested_size_inside_the_budget() {
    let harness = harness();
    let at = producer_secs(0);
    // (case, the requested width and height, the size the image decodes at)
    let rows: Vec<(&str, u32, u32)> = vec![
        (
            "the archived frame stretched off its own ratio",
            TALL_FRAME_WIDTH * 2,
            TALL_FRAME_HEIGHT,
        ),
        (
            "both modelled side maxima, still inside the frame budget",
            MAX_WIDTH_PIXELS,
            2_160,
        ),
    ];

    for (case, width, height) in rows {
        let body = with(
            &request(TALL_FRAME_STREAM, "PRODUCER_TIMESTAMP", at, at + 1, "PNG"),
            serde_json::json!({ "WidthPixels": width, "HeightPixels": height }),
        );
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let first = images(&response).first().cloned().unwrap_or_default();
        let bytes = image_bytes(&first);
        let decoded = ::image::load_from_memory(&bytes).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a decodable image, got {error}");
        });
        assert_eq!(
            (decoded.width(), decoded.height()),
            (width, height),
            "case {case:?}: expected a {width}x{height} image, got {}x{}",
            decoded.width(),
            decoded.height()
        );
    }
}

/// RULE: a request outside the documented window, interval, format-config, stream and
/// retention rules is refused with the modelled client error — never with an image
/// list — and a request ON the published bound is not.
///
/// The rows span each rule, with the bound and one value past it where the rule has
/// one.
#[tokio::test]
async fn a_request_outside_the_documented_rules_is_refused() {
    let harness = harness();
    let from = producer_secs(0);
    let window = |start: i64, end: i64| {
        request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", start, end, "JPEG")
    };
    // (case, request, expected status, expected code, expected message)
    let rows: Vec<(&str, serde_json::Value, u16, &str, &str)> = vec![
        (
            "a reversed window",
            window(from, from - 60),
            400,
            "InvalidArgumentException",
            RANGE_ORDER,
        ),
        (
            "a window that ends where it starts",
            window(from, from),
            400,
            "InvalidArgumentException",
            RANGE_ORDER,
        ),
        (
            "a window one second past the 300 second maximum",
            window(from, from + 301),
            400,
            "InvalidArgumentException",
            WINDOW_TOO_WIDE,
        ),
        (
            "a window exactly 300 seconds wide: accepted, and the archive answers",
            window(producer_secs(2) + 100, producer_secs(2) + 400),
            404,
            "ResourceNotFoundException",
            NO_FRAGMENTS,
        ),
        (
            "a sampling interval one millisecond under the minimum",
            with(
                &window(from, from + 1),
                serde_json::json!({ "SamplingInterval": 199 }),
            ),
            400,
            "InvalidArgumentException",
            INTERVAL_TOO_SMALL,
        ),
        (
            "a JPEGQuality of 0",
            with(
                &window(from, from + 1),
                serde_json::json!({ "FormatConfig": { "JPEGQuality": "0" } }),
            ),
            400,
            "InvalidArgumentException",
            QUALITY_OUT_OF_RANGE,
        ),
        (
            "a JPEGQuality of 101",
            with(
                &window(from, from + 1),
                serde_json::json!({ "FormatConfig": { "JPEGQuality": "101" } }),
            ),
            400,
            "InvalidArgumentException",
            QUALITY_OUT_OF_RANGE,
        ),
        (
            "a JPEGQuality that is not a number",
            with(
                &window(from, from + 1),
                serde_json::json!({ "FormatConfig": { "JPEGQuality": "high" } }),
            ),
            400,
            "InvalidArgumentException",
            QUALITY_OUT_OF_RANGE,
        ),
        (
            "both stream identifiers",
            with(
                &window(from, from + 1),
                serde_json::json!({ "StreamARN": harness.arn.clone() }),
            ),
            400,
            "InvalidArgumentException",
            "One of StreamName or StreamARN must be provided, but not both.",
        ),
        (
            "a stream nothing answers to",
            request(
                "no-such-stream",
                "PRODUCER_TIMESTAMP",
                from,
                from + 1,
                "JPEG",
            ),
            404,
            "ResourceNotFoundException",
            "The requested stream is not found or not active.",
        ),
        (
            "a stream that retains no data",
            request(
                NO_RETENTION_STREAM,
                "PRODUCER_TIMESTAMP",
                from,
                from + 1,
                "JPEG",
            ),
            400,
            "NoDataRetentionException",
            "The stream data retention is set to 0 hours.",
        ),
        (
            "a NextToken this service never issued",
            with(
                &window(from, from + 1),
                serde_json::json!({ "NextToken": "bm90LWEtdG9rZW4=" }),
            ),
            400,
            "InvalidArgumentException",
            "NextToken is not a valid pagination token",
        ),
    ];

    for (case, body, status, code, message) in rows {
        let response = harness.post("/getImages", &body).await;
        let (actual_code, actual_message) = reported(&response);
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            actual_code, code,
            "case {case:?}: expected {code}, got {actual_code:?}"
        );
        assert_eq!(
            actual_message, message,
            "case {case:?}: expected {message:?}, got {actual_message:?}"
        );
    }

    // The bounds themselves are ACCEPTED: the documented minimum interval, both
    // published quality bounds and both modelled pixel ceilings answer with images
    // rather than with a refusal.
    for (case, extra) in [
        (
            "the 200 ms minimum interval",
            serde_json::json!({ "SamplingInterval": 200 }),
        ),
        (
            "the lowest JPEGQuality",
            serde_json::json!({ "FormatConfig": { "JPEGQuality": "1" } }),
        ),
        (
            "the highest JPEGQuality",
            serde_json::json!({ "FormatConfig": { "JPEGQuality": "100" } }),
        ),
        (
            "the modelled pixel ceilings",
            serde_json::json!({ "WidthPixels": MAX_WIDTH_PIXELS, "HeightPixels": 2160 }),
        ),
        (
            "the modelled pixel floors",
            serde_json::json!({ "WidthPixels": 1, "HeightPixels": 1 }),
        ),
    ] {
        let body = with(&window(from, from + 1), extra);
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }

    // And a pixel size PAST a modelled ceiling (or under its floor) is refused before
    // any frame is generated — the model's own `@range`, answered by the request
    // validator with its own code rather than by the handler.
    for (case, extra) in [
        (
            "a width one past the modelled ceiling",
            serde_json::json!({ "WidthPixels": MAX_WIDTH_PIXELS + 1 }),
        ),
        (
            "a height one past the modelled ceiling",
            serde_json::json!({ "HeightPixels": 2161 }),
        ),
        (
            "a zero width",
            serde_json::json!({ "WidthPixels": 0 }),
        ),
    ] {
        let body = with(&window(from, from + 1), extra);
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (code, _) = reported(&response);
        assert!(
            !code.is_empty() && code != "NotImplemented",
            "case {case:?}: expected a modelled refusal code, got {code:?}"
        );
    }
}

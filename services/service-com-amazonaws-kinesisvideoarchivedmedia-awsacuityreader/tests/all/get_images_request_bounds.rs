// SPDX-License-Identifier: CC0-1.0

//! The output size `GetImages` generates its images at, end to end over HTTP:
//! `WidthPixels` and `HeightPixels` arrive as a PAIR, and one of them alone is a bad
//! argument, decided before the archive is consulted.
//!
//! The harness, the request builder and the seeded streams are `get_images_generation`'s,
//! so this suite reads the same archive the rest of the `GetImages` tests do.

use crate::get_images_generation::{
    harness, image_bytes, images, producer_secs, reported, request, with, EMPTY_STREAM,
    FRAME_HEIGHT, FRAME_WIDTH, IMAGES_STREAM, INCOMPLETE_RESIZE, INVALID_ARGUMENT,
    MAX_HEIGHT_PIXELS, MAX_WIDTH_PIXELS, NO_FRAGMENTS, WINDOW_TOO_WIDE,
};

/// How many seconds past the documented 300-second window a row has to reach to break
/// that bound.
const PAST_THE_WINDOW_SECS: i64 = 301;

/// RULE: the output size is a PAIR — `WidthPixels` and `HeightPixels` are both given or
/// both omitted, and ONE of them alone is refused HTTP 400 `InvalidArgumentException`
/// with one fixed sentence ("If only the WidthPixels parameter is provided or if only
/// the HeightPixels is provided, a ValidationException will be thrown"); a complete
/// pair is the size the generated image really has.
///
/// The rows differ along what the rule generalises over: which side is named, its value
/// (the published floor of that side, a middling size, its published ceiling) and
/// whether the other side is there.
#[tokio::test]
async fn one_output_side_alone_is_refused_and_the_pair_is_served() {
    let harness = harness();
    let at = producer_secs(1);
    // (case, the size members the request carries, the status expected)
    let rows: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "a width at the published floor, alone",
            serde_json::json!({ "WidthPixels": 1 }),
            400,
        ),
        (
            "a middling width, alone",
            serde_json::json!({ "WidthPixels": 320 }),
            400,
        ),
        (
            "a width at the published ceiling, alone",
            serde_json::json!({ "WidthPixels": MAX_WIDTH_PIXELS }),
            400,
        ),
        (
            "a height at the published floor, alone",
            serde_json::json!({ "HeightPixels": 1 }),
            400,
        ),
        (
            "a middling height, alone",
            serde_json::json!({ "HeightPixels": 240 }),
            400,
        ),
        (
            "a height at the published ceiling, alone",
            serde_json::json!({ "HeightPixels": MAX_HEIGHT_PIXELS }),
            400,
        ),
        (
            "a width alone, beside an unrelated format config",
            serde_json::json!({ "WidthPixels": 320, "FormatConfig": { "JPEGQuality": "90" } }),
            400,
        ),
        (
            "both sides",
            serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
            200,
        ),
        (
            "both sides at the published floors",
            serde_json::json!({ "WidthPixels": 1, "HeightPixels": 1 }),
            200,
        ),
        ("neither side", serde_json::json!({}), 200),
    ];

    for (case, size, status) in rows {
        let body = with(
            &request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", at, at + 1, "JPEG"),
            size.clone(),
        );
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 400 {
            let (code, message) = reported(&response);
            assert_eq!(
                code, INVALID_ARGUMENT,
                "case {case:?}: expected {INVALID_ARGUMENT}, got {code:?}"
            );
            assert_eq!(
                message, INCOMPLETE_RESIZE,
                "case {case:?}: expected {INCOMPLETE_RESIZE:?}, got {message:?}"
            );
            continue;
        }
        // An accepted size is the size the generated image really has, so a complete
        // pair is not merely tolerated.
        let first = images(&response).first().cloned().unwrap_or_default();
        let bytes = image_bytes(&first);
        let decoded = ::image::load_from_memory(&bytes).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a decodable image, got {error}")
        });
        let expected = (
            size["WidthPixels"]
                .as_u64()
                .and_then(|side| u32::try_from(side).ok())
                .unwrap_or(FRAME_WIDTH),
            size["HeightPixels"]
                .as_u64()
                .and_then(|side| u32::try_from(side).ok())
                .unwrap_or(FRAME_HEIGHT),
        );
        assert_eq!(
            (decoded.width(), decoded.height()),
            expected,
            "case {case:?}: expected a {}x{} image, got {}x{}",
            expected.0,
            expected.1,
            decoded.width(),
            decoded.height()
        );
    }
}

/// RULE: both request bounds are decided BEFORE the archive is consulted, so a stream
/// holding no fragment at all answers the request-bound refusal for a request that
/// breaks one, and the fragment-absence refusal only for a request that breaks neither.
///
/// The rows differ along which bound the request breaks — each side alone, the window
/// width — and include the two requests that break none, which is what makes the
/// ordering visible rather than assumed.
#[tokio::test]
async fn a_request_bound_answers_before_the_archive_is_consulted() {
    let harness = harness();
    let at = producer_secs(0);
    // (case, how wide the window is in seconds, the extra members, the status, code and
    // message expected)
    let rows: Vec<(&str, i64, serde_json::Value, u16, &str, &str)> = vec![
        (
            "a width without a height",
            1,
            serde_json::json!({ "WidthPixels": 320 }),
            400,
            INVALID_ARGUMENT,
            INCOMPLETE_RESIZE,
        ),
        (
            "a height without a width",
            1,
            serde_json::json!({ "HeightPixels": 240 }),
            400,
            INVALID_ARGUMENT,
            INCOMPLETE_RESIZE,
        ),
        (
            "a window past the published bound",
            PAST_THE_WINDOW_SECS,
            serde_json::json!({}),
            400,
            INVALID_ARGUMENT,
            WINDOW_TOO_WIDE,
        ),
        (
            "a complete pair inside every bound",
            1,
            serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
            404,
            "ResourceNotFoundException",
            NO_FRAGMENTS,
        ),
        (
            "no size at all, inside every bound",
            1,
            serde_json::json!({}),
            404,
            "ResourceNotFoundException",
            NO_FRAGMENTS,
        ),
    ];

    for (case, width, extra, status, code, expected) in rows {
        let body = with(
            &request(EMPTY_STREAM, "PRODUCER_TIMESTAMP", at, at + width, "JPEG"),
            extra,
        );
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let (actual_code, message) = reported(&response);
        assert_eq!(
            actual_code, code,
            "case {case:?}: expected {code}, got {actual_code:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

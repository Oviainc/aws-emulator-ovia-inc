// SPDX-License-Identifier: CC0-1.0

//! The both-or-neither rule `GetImages`' output size follows, pinned as an INVARIANT
//! rather than as one request: naming exactly ONE of `WidthPixels` and `HeightPixels`
//! is a bad argument whatever ELSE the call says, and naming both or neither never is.
//!
//! The sweep below crosses a catalog of size members with a set of request contexts
//! that differ in the archive they address (a stream nothing was ingested into, a
//! stream holding fragments, a window before that stream's archive), the timestamp
//! field selected on, the output format and the extra members the request carries.
//! The refusal is the same in every one of them — and, on the contexts whose archive
//! answers nothing, it REPLACES the fragment-absence refusal a complete pair gets
//! there, which is what makes the argument check's position before the archive lookup
//! observable.
//!
//! The harness, the request builder and the seeded streams are `get_images_generation`'s.

use crate::get_images_generation::{
    harness, images, producer_secs, reported, request, with, EMPTY_STREAM, IMAGES_STREAM,
    INCOMPLETE_RESIZE, INVALID_ARGUMENT, MAX_HEIGHT_PIXELS, MAX_WIDTH_PIXELS, NO_FRAGMENTS,
};

/// The code a window no fragment falls in is refused with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// How far before the seeded archive a window that holds nothing starts, in seconds.
const BEFORE_THE_ARCHIVE_SECS: i64 = 3_600;

/// Everything about a `GetImages` call EXCEPT its output size.
struct Context {
    /// What this context is.
    case: &'static str,
    /// The stream the request addresses; both retain data.
    stream: &'static str,
    /// The timestamp field the window is read on (`ImageSelectorType`).
    selector: &'static str,
    /// The format the images are asked for in.
    format: &'static str,
    /// The window, as an offset in seconds from the first seeded fragment, and its width.
    offset_secs: i64,
    width_secs: i64,
    /// The members the request carries beside the window and the size.
    extra: serde_json::Value,
    /// What a request carrying BOTH sides, or NEITHER, is answered with here: the
    /// status, and the refusal when it is one.
    whole_pair: (u16, Option<(&'static str, &'static str)>),
}

/// One row of the size catalog: the size members a request carries, and whether that
/// names exactly one of the two sides.
struct Size {
    /// What this size is.
    case: &'static str,
    /// The members themselves.
    members: serde_json::Value,
    /// Whether the pair is incomplete — exactly one side named.
    incomplete: bool,
}

/// The size members swept over every context.
///
/// The values sit on the bounds the model publishes for the two members
/// (`GetImagesInput$WidthPixels` range 1..3840, `GetImagesInput$HeightPixels` range
/// 1..2160) and on a middling size between them, so a row is never a number picked at
/// random; the rows differ along which side is named, its value, and whether the other
/// side is there.
fn sizes() -> Vec<Size> {
    vec![
        Size {
            case: "the width floor alone",
            members: serde_json::json!({ "WidthPixels": 1 }),
            incomplete: true,
        },
        Size {
            case: "a middling width alone",
            members: serde_json::json!({ "WidthPixels": 320 }),
            incomplete: true,
        },
        Size {
            case: "the width ceiling alone",
            members: serde_json::json!({ "WidthPixels": MAX_WIDTH_PIXELS }),
            incomplete: true,
        },
        Size {
            case: "the height floor alone",
            members: serde_json::json!({ "HeightPixels": 1 }),
            incomplete: true,
        },
        Size {
            case: "a middling height alone",
            members: serde_json::json!({ "HeightPixels": 240 }),
            incomplete: true,
        },
        Size {
            case: "the height ceiling alone",
            members: serde_json::json!({ "HeightPixels": MAX_HEIGHT_PIXELS }),
            incomplete: true,
        },
        Size {
            case: "no side at all",
            members: serde_json::json!({}),
            incomplete: false,
        },
        Size {
            case: "both sides at their floors",
            members: serde_json::json!({ "WidthPixels": 1, "HeightPixels": 1 }),
            incomplete: false,
        },
        Size {
            case: "both sides at a middling size",
            members: serde_json::json!({ "WidthPixels": 320, "HeightPixels": 240 }),
            incomplete: false,
        },
        Size {
            case: "both sides at their ceilings",
            members: serde_json::json!({
                "WidthPixels": MAX_WIDTH_PIXELS,
                "HeightPixels": MAX_HEIGHT_PIXELS,
            }),
            incomplete: false,
        },
    ]
}

/// The contexts the size catalog is swept over.
fn contexts() -> Vec<Context> {
    vec![
        Context {
            case: "a retention-enabled stream nothing was ingested into",
            stream: EMPTY_STREAM,
            selector: "PRODUCER_TIMESTAMP",
            format: "JPEG",
            offset_secs: 0,
            width_secs: 5,
            extra: serde_json::json!({}),
            whole_pair: (404, Some((NOT_FOUND, NO_FRAGMENTS))),
        },
        Context {
            case: "that same empty stream, read on server timestamps, asked for PNG",
            stream: EMPTY_STREAM,
            selector: "SERVER_TIMESTAMP",
            format: "PNG",
            offset_secs: 0,
            width_secs: 1,
            extra: serde_json::json!({}),
            whole_pair: (404, Some((NOT_FOUND, NO_FRAGMENTS))),
        },
        Context {
            case: "a window before the archive of a stream that does hold fragments",
            stream: IMAGES_STREAM,
            selector: "PRODUCER_TIMESTAMP",
            format: "JPEG",
            offset_secs: -BEFORE_THE_ARCHIVE_SECS,
            width_secs: 2,
            extra: serde_json::json!({}),
            whole_pair: (404, Some((NOT_FOUND, NO_FRAGMENTS))),
        },
        Context {
            case: "a window the archive answers, served as JPEG at a named quality",
            stream: IMAGES_STREAM,
            selector: "PRODUCER_TIMESTAMP",
            format: "JPEG",
            offset_secs: 0,
            width_secs: 1,
            extra: serde_json::json!({ "FormatConfig": { "JPEGQuality": "50" } }),
            whole_pair: (200, None),
        },
        Context {
            case: "a window the archive answers, sampled into a bounded page of PNGs",
            stream: IMAGES_STREAM,
            selector: "PRODUCER_TIMESTAMP",
            format: "PNG",
            offset_secs: 0,
            width_secs: 1,
            extra: serde_json::json!({ "SamplingInterval": 200, "MaxResults": 2 }),
            whole_pair: (200, None),
        },
    ]
}

/// RULE: `WidthPixels` and `HeightPixels` are one PAIR — a request naming exactly one
/// of them is refused HTTP 400 `InvalidArgumentException` with a single fixed sentence,
/// decided from the request alone and therefore BEFORE the addressed archive is
/// consulted, while a request naming both or neither is never refused for its size.
#[tokio::test]
async fn exactly_one_output_side_is_refused_in_every_request_context() {
    let harness = harness();
    for context in contexts() {
        let start = producer_secs(0) + context.offset_secs;
        let base = with(
            &request(
                context.stream,
                context.selector,
                start,
                start + context.width_secs,
                context.format,
            ),
            context.extra.clone(),
        );
        for size in sizes() {
            let case = format!("{} / {}", context.case, size.case);
            let body = with(&base, size.members.clone());
            let response = harness.post("/getImages", &body).await;
            let status = response.status().as_u16();

            if size.incomplete {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {:?}",
                    String::from_utf8_lossy(response.body())
                );
                let (code, message) = reported(&response);
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    (INVALID_ARGUMENT, INCOMPLETE_RESIZE),
                    "case {case:?}: expected ({INVALID_ARGUMENT:?}, {INCOMPLETE_RESIZE:?}), got \
                     ({code:?}, {message:?})"
                );
                continue;
            }

            let (expected_status, expected_refusal) = context.whole_pair;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with body {:?}",
                String::from_utf8_lossy(response.body())
            );
            match expected_refusal {
                Some((expected_code, expected_message)) => {
                    let (code, message) = reported(&response);
                    assert_eq!(
                        (code.as_str(), message.as_str()),
                        (expected_code, expected_message),
                        "case {case:?}: expected ({expected_code:?}, {expected_message:?}), got \
                         ({code:?}, {message:?})"
                    );
                }
                None => {
                    let generated = images(&response);
                    assert!(
                        !generated.is_empty(),
                        "case {case:?}: expected a generated image, got {:?}",
                        String::from_utf8_lossy(response.body())
                    );
                }
            }
        }
    }
}

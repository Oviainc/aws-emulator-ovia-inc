// SPDX-License-Identifier: CC0-1.0

//! The width of the window `GetImages` samples, end to end over HTTP: at most the
//! documented 300 seconds, refused above that with one fixed sentence.
//!
//! The harness, the request builder and the seeded streams are `get_images_generation`'s
//! — the real `MultiServiceRouter` with this one service registered — so this suite
//! reads the same archive the rest of the `GetImages` tests do, and the image behaviour
//! INSIDE an accepted window is pinned there.

use crate::get_images_generation::{
    harness, images, producer_secs, reported, request, with, IMAGES_STREAM, WINDOW_TOO_WIDE,
};

/// The widest window the model admits for one call, in seconds: `EndTimestamp` — "If
/// the time range between StartTimestamp and EndTimestamp is more than 300 seconds
/// above StartTimestamp, you will receive an IllegalArgumentException."
const MAX_WINDOW_SECS: i64 = 300;

/// The code a refused request bound carries (`InvalidArgumentException`, HTTP 400).
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// RULE: a request whose window spans at most the documented 300 seconds is served;
/// one wider is refused HTTP 400 `InvalidArgumentException` with one fixed sentence
/// that carries nothing of the request.
///
/// The rows differ along what the rule generalises over: how wide the window is — a
/// second, long enough to span every archived fragment, one second under the published
/// bound, the bound itself, half a second past it, a second past it, and an hour.
#[tokio::test]
async fn a_window_wider_than_the_published_bound_is_refused_with_one_sentence() {
    let harness = harness();
    let from = producer_secs(0);
    // (case, the EndTimestamp the request carries, in epoch seconds, the status
    // expected)
    let rows: [(&str, f64, u16); 7] = [
        ("one second wide", (from + 1) as f64, 200),
        (
            "wide enough to span every archived fragment",
            (from + 30) as f64,
            200,
        ),
        (
            "one second under the bound",
            (from + MAX_WINDOW_SECS - 1) as f64,
            200,
        ),
        (
            "exactly at the bound",
            (from + MAX_WINDOW_SECS) as f64,
            200,
        ),
        (
            "half a second past the bound",
            (from + MAX_WINDOW_SECS) as f64 + 0.5,
            400,
        ),
        (
            "one second past the bound",
            (from + MAX_WINDOW_SECS + 1) as f64,
            400,
        ),
        ("an hour wide", (from + 3_600) as f64, 400),
    ];

    for (case, end, status) in rows {
        let body = with(
            &request(IMAGES_STREAM, "PRODUCER_TIMESTAMP", from, from + 1, "JPEG"),
            serde_json::json!({ "EndTimestamp": end }),
        );
        let response = harness.post("/getImages", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 200 {
            let page = images(&response);
            assert!(
                !page.is_empty() && page.iter().all(|image| image["Error"].is_null()),
                "case {case:?}: expected images generated from the archive, got {page:?}"
            );
            continue;
        }
        let (code, message) = reported(&response);
        assert_eq!(
            code, INVALID_ARGUMENT,
            "case {case:?}: expected {INVALID_ARGUMENT}, got {code:?}"
        );
        assert_eq!(
            message, WINDOW_TOO_WIDE,
            "case {case:?}: expected {WINDOW_TOO_WIDE:?}, got {message:?}"
        );
        assert!(
            !message.contains(IMAGES_STREAM),
            "case {case:?}: the sentence carries nothing of the request, but {message:?} names \
             the stream"
        );
    }
}

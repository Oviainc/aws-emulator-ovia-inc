// SPDX-License-Identifier: CC0-1.0

//! The CREATION-TIME WINDOW a listing filters on, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! THE RULE UNDER TEST: `AfterCreationTime` "[s]pecifies the lower bound for the …
//! operation" and `BeforeCreationTime` the upper bound of the window a listing filters
//! on, so a pair whose lower bound lies AFTER its upper bound describes no window at all
//! and is refused — `ValidationException` (HTTP 400) with the sentence Amazon Textract
//! states for it — before anything is listed, rather than answered with the empty listing
//! that filter would select. A window with one bound, with neither, or with its bounds in
//! order is a range, and is served.
//!
//! The table spans what the rule generalises over: which bounds the request carries, how
//! the two compare (ordered, equal, crossed by a second, crossed by years, crossed below
//! the second), and which listing is asked — both operations that take the pair hold to
//! the one rule.

use serde_json::{json, Value};

use crate::document_extraction::{call, error_code, error_message, harness};

/// The sentence Amazon Textract states `ValidationException` with for a creation-time
/// window that is not a range: fixed text, with nothing of the request in it.
const INVALID_TIME_RANGE_MESSAGE: &str = "Request has invalid time range.";

/// An instant well inside the representable range, so the bounds below are exact on the
/// wire.
const BASE_SECS: f64 = 1_700_000_000.0;

/// One row of the window table: the bounds the request carries, and whether they are a
/// range.
struct WindowCase {
    name: &'static str,
    bounds: Value,
    is_range: bool,
}

fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            name: "no bounds at all",
            bounds: json!({}),
            is_range: true,
        },
        WindowCase {
            name: "a lower bound only",
            bounds: json!({ "AfterCreationTime": BASE_SECS }),
            is_range: true,
        },
        WindowCase {
            name: "an upper bound only",
            bounds: json!({ "BeforeCreationTime": BASE_SECS }),
            is_range: true,
        },
        WindowCase {
            name: "bounds in order",
            bounds: json!({
                "AfterCreationTime": BASE_SECS,
                "BeforeCreationTime": BASE_SECS + 3600.0,
            }),
            is_range: true,
        },
        WindowCase {
            // Both bounds are inclusive, so one instant is a window of one instant.
            name: "both bounds on the same instant",
            bounds: json!({
                "AfterCreationTime": BASE_SECS,
                "BeforeCreationTime": BASE_SECS,
            }),
            is_range: true,
        },
        WindowCase {
            name: "bounds a quarter of a second apart, in order",
            bounds: json!({
                "AfterCreationTime": BASE_SECS,
                "BeforeCreationTime": BASE_SECS + 0.25,
            }),
            is_range: true,
        },
        WindowCase {
            name: "bounds crossed by one second",
            bounds: json!({
                "AfterCreationTime": BASE_SECS + 1.0,
                "BeforeCreationTime": BASE_SECS,
            }),
            is_range: false,
        },
        WindowCase {
            name: "bounds crossed by a quarter of a second",
            bounds: json!({
                "AfterCreationTime": BASE_SECS + 0.25,
                "BeforeCreationTime": BASE_SECS,
            }),
            is_range: false,
        },
        WindowCase {
            name: "bounds crossed by years",
            bounds: json!({
                "AfterCreationTime": BASE_SECS,
                "BeforeCreationTime": BASE_SECS - 86_400.0 * 365.0 * 5.0,
            }),
            is_range: false,
        },
        WindowCase {
            name: "a lower bound after the epoch and an upper bound before it",
            bounds: json!({ "AfterCreationTime": 1.0, "BeforeCreationTime": -1.0 }),
            is_range: false,
        },
    ]
}

#[tokio::test]
async fn a_listings_creation_time_window_must_be_a_range() {
    let router = harness();

    // An adapter to scope the version listing to, so both listings are asked about
    // something that exists and a served row reaches the listing's own answer.
    let (status, created) = call(
        &router,
        "CreateAdapter",
        &json!({ "AdapterName": "window-adapter", "FeatureTypes": ["QUERIES"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the adapter the version listing is scoped to must be created, got {status} with \
         {created}"
    );
    let adapter_id = created["AdapterId"].as_str().unwrap_or_default().to_string();
    assert!(
        !adapter_id.is_empty(),
        "CreateAdapter must mint an AdapterId, got {created}"
    );

    // Both operations that take the bound pair, and the member each answers with.
    let listings: Vec<(&str, Value, &str)> = vec![
        ("ListAdapters", json!({}), "Adapters"),
        (
            "ListAdapterVersions",
            json!({ "AdapterId": adapter_id }),
            "AdapterVersions",
        ),
    ];

    for case in window_cases() {
        for (operation, base, listed) in &listings {
            let mut request = base.clone();
            for (member, value) in case.bounds.as_object().expect("an object of bounds") {
                request[member] = value.clone();
            }
            let name = format!("{} / {operation}", case.name);
            let (status, body) = call(&router, operation, &request).await;
            if case.is_range {
                assert_eq!(
                    status, 200,
                    "case {name:?}: a window whose bounds are a range must be served, got \
                     {status} with {body}"
                );
                assert!(
                    body.get(*listed).and_then(Value::as_array).is_some(),
                    "case {name:?}: a served listing must report its {listed}, got {body}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {name:?}: a window whose bounds are crossed must be refused, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {name:?}: expected ValidationException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                INVALID_TIME_RANGE_MESSAGE,
                "case {name:?}: expected the message {INVALID_TIME_RANGE_MESSAGE:?}, got {:?}",
                error_message(&body)
            );
            assert!(
                body.get(*listed).is_none(),
                "case {name:?}: a refused listing must report nothing, got {body}"
            );
        }
    }
}

/// RULE: the refusal is about the WINDOW and nothing else, so it is answered whatever
/// else the request carries — a page size, a cursor member — and it is answered before
/// the listing is computed, so an account holding adapters gets it too.
#[tokio::test]
async fn a_crossed_window_is_refused_whatever_else_the_request_carries() {
    let router = harness();

    for index in 0..3 {
        let (status, body) = call(
            &router,
            "CreateAdapter",
            &json!({
                "AdapterName": format!("adapter-{index}"),
                "FeatureTypes": ["QUERIES"],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "the seeded adapters must be created, got {status} with {body}"
        );
    }

    let crossed = json!({
        "AfterCreationTime": BASE_SECS + 10.0,
        "BeforeCreationTime": BASE_SECS,
    });
    // The rows differ along what the request carries BESIDE the window.
    let extras: Vec<(&str, Value)> = vec![
        ("the window alone", json!({})),
        ("a page size", json!({ "MaxResults": 1 })),
        ("the largest page size this listing takes", json!({ "MaxResults": 1000 })),
    ];

    for (name, extra) in extras {
        let mut request = crossed.clone();
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, "ListAdapters", &request).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a crossed window is refused whatever else the request carries, \
             got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {body}"
        );
    }

    // The positive control on the same state: the window the other way round is served,
    // and it reports the adapters that were created.
    let (status, body) = call(
        &router,
        "ListAdapters",
        &json!({ "AfterCreationTime": 0.0, "BeforeCreationTime": BASE_SECS * 2.0 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a window in order must be served, got {status} with {body}"
    );
    assert_eq!(
        body["Adapters"].as_array().map(Vec::len),
        Some(3),
        "the served listing must report the adapters inside its window, got {body}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The per-entry `BatchItemError` text a position outside the WGS 84 coordinate
//! domain is reported with.
//!
//! RULE: a coordinate the service refuses because it falls outside its axis's WGS 84
//! domain is reported with the fixed sentence `"<axis> must be between <min> and <max>
//! degrees"` — the axis name and that axis's two domain bounds, and nothing else: no
//! model member name and no value read from the request. `Position` is a `@sensitive`
//! list of `Double` with `@length 2..2` and NO `@range` on its members
//! (`smithy_query(query="shape",shape="Position")`), so the domain check is the
//! service's own and a submitted coordinate may not be echoed into an error string.
//! Per-entry refusals carry `BatchItemError$Code` `"ValidationError"` — the
//! `BatchItemErrorCode` member for "Input fails to satisfy the constraints specified by
//! the service" — inside a 200 response.
//!
//! The table spans what the rule generalises over: the axis (longitude, latitude), the
//! sign of the violated bound, the size of the overshoot (one ulp past the bound, a
//! whole degree, an extreme), and where the bad entry sits in the batch — so the text a
//! client matches on is the same sentence for every rejected coordinate of an axis.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, wire_time};

/// The WGS 84 coordinate domain, one entry per axis: the axis name a refusal states, the
/// bound pair that defines its domain, and the sentence the service states for a coordinate
/// outside it, written out as a LITERAL so the wording is pinned by text rather than
/// re-derived from the implementation's template. Source: `Position` is documented as
/// "WGS 84 format: `[X or longitude, Y or latitude]`"
/// (`smithy_query(query="doc",shape="DevicePositionUpdate",member="Position")`), whose
/// axes are longitude in [-180, 180] degrees and latitude in [-90, 90] degrees.
const AXES: [(&str, f64, &str); 2] = [
    (
        "longitude",
        180.0,
        "longitude must be between -180 and 180 degrees",
    ),
    (
        "latitude",
        90.0,
        "latitude must be between -90 and 90 degrees",
    ),
];

/// The sentence the rule requires for an out-of-domain coordinate on `axis`: the literal
/// the table above carries.
fn expected_message(axis: &str) -> &'static str {
    AXES.iter()
        .find(|(name, _, _)| *name == axis)
        .map(|(_, _, sentence)| *sentence)
        .unwrap_or_else(|| panic!("the table must carry a sentence for the {axis} axis"))
}

/// One out-of-domain coordinate: which axis it violates, and the `[longitude,
/// latitude]` pair to submit.
struct OutOfDomain {
    case: &'static str,
    axis: &'static str,
    position: [f64; 2],
}

/// The out-of-domain table. Rows differ along the axis, the sign of the bound they
/// pass, and how far past it they sit — every row must yield the SAME sentence for its
/// axis.
fn out_of_domain_cases() -> Vec<OutOfDomain> {
    let mut cases = Vec::new();
    for (axis, bound, _) in AXES {
        // The in-domain half of each pair is a valid coordinate for the OTHER axis, so
        // only the named axis is ever the reason.
        let at = |value: f64| -> [f64; 2] {
            if axis == "longitude" {
                [value, 47.60]
            } else {
                [-122.33, value]
            }
        };
        for (label, value) in [
            ("just past the max bound", bound + 0.000001),
            ("just past the min bound", -bound - 0.000001),
            ("a whole degree past the max bound", bound + 1.0),
            ("a whole degree past the min bound", -bound - 1.0),
            ("far past the max bound", bound * 10.0 + 20.0),
            ("far past the min bound", -(bound * 10.0 + 20.0)),
        ] {
            cases.push(OutOfDomain {
                case: label,
                axis,
                position: at(value),
            });
        }
    }
    cases
}

fn update(device: &str, sample_time: &str, position: [f64; 2]) -> Value {
    json!({
        "DeviceId": device,
        "SampleTime": wire_time(sample_time),
        "Position": [position[0], position[1]],
    })
}

async fn create_tracker(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        "POST",
        "/tracking/v0/trackers",
        Some(json!({ "TrackerName": name })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "CreateTracker {name:?} must succeed, got {} {:?}",
        answer.status, answer.body
    );
}

async fn batch_update(router: &MultiServiceRouter, name: &str, updates: Value) -> Value {
    let answer = call(
        router,
        "POST",
        &format!("/tracking/v0/trackers/{name}/positions"),
        Some(json!({ "Updates": updates })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "BatchUpdateDevicePosition must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.json()
}

/// The `Errors` list of a batch answer.
fn errors(case: &str, body: &Value) -> Vec<Value> {
    body.get("Errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Errors list, got {body}"))
        .clone()
}

#[tokio::test]
async fn an_out_of_domain_coordinate_is_reported_with_the_fixed_domain_sentence() {
    for OutOfDomain {
        case,
        axis,
        position,
    } in out_of_domain_cases()
    {
        let case = format!("{axis} {case}");
        let router = emulator();
        create_tracker(&router, "fleet").await;

        let body = batch_update(
            &router,
            "fleet",
            json!([update("device-1", "2024-01-01T00:00:00Z", position)]),
        )
        .await;
        let reported = errors(&case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one reported entry, got {reported:?}"
        );
        let error = reported[0]
            .get("Error")
            .unwrap_or_else(|| panic!("case {case:?}: expected an Error member, got {:?}", reported[0]));

        let message = error
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected an Error.Message, got {error}"));
        let expected = expected_message(axis);
        assert_eq!(
            message, expected,
            "case {case:?}: expected Error.Message {expected:?}, got {message:?}"
        );

        // The sentence names no model member and echoes no submitted coordinate: the
        // list is `@sensitive`, and a message that interpolates a request value is not
        // a stable string for a client to branch on.
        assert!(
            !message.contains("Position"),
            "case {case:?}: the message must not name the model member, got {message:?}"
        );
        let offending = if axis == "longitude" {
            position[0]
        } else {
            position[1]
        };
        for rendering in [format!("{offending}"), format!("{offending:.1}")] {
            assert!(
                !message.contains(&rendering),
                "case {case:?}: the message must not echo the submitted coordinate \
                 {rendering:?}, got {message:?}"
            );
        }

        assert_eq!(
            error.get("Code").and_then(Value::as_str),
            Some("ValidationError"),
            "case {case:?}: expected Error.Code ValidationError, got {error}"
        );
    }
}

#[tokio::test]
async fn every_rejected_coordinate_of_one_axis_reports_the_same_sentence() {
    // The whole point of a fixed sentence: a client keying retry/skip logic on the
    // message sees ONE string per axis, whatever coordinate was submitted and wherever
    // the entry sat in the batch.
    let router = emulator();
    create_tracker(&router, "fleet").await;

    let cases = out_of_domain_cases();
    // `DevicePositionUpdateList` is `len 1..10`, so the table is sent in chunks the
    // model admits, each chunk putting the bad entries at different offsets.
    for (chunk_index, chunk) in cases.chunks(4).enumerate() {
        let updates: Vec<Value> = chunk
            .iter()
            .enumerate()
            .map(|(i, bad)| {
                update(
                    &format!("bad-{chunk_index}-{i}"),
                    "2024-01-01T00:00:00Z",
                    bad.position,
                )
            })
            .collect();
        let body = batch_update(&router, "fleet", json!(updates)).await;
        let case = format!("chunk {chunk_index}");
        let reported = errors(&case, &body);
        assert_eq!(
            reported.len(),
            chunk.len(),
            "case {case:?}: every entry of this chunk is out of domain, got {reported:?}"
        );
        for (i, bad) in chunk.iter().enumerate() {
            let entry = reported
                .iter()
                .find(|e| {
                    e.get("DeviceId").and_then(Value::as_str)
                        == Some(&format!("bad-{chunk_index}-{i}"))
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: entry bad-{chunk_index}-{i} must be reported, got {reported:?}")
                });
            let message = entry
                .get("Error")
                .and_then(|e| e.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let expected = expected_message(bad.axis);
            assert_eq!(
                message, expected,
                "case {case:?} entry {i}: expected Error.Message {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_partial_batch_reports_the_bad_entry_and_commits_the_valid_one() {
    // The documented partial-failure contract: the valid entry is committed and
    // readable, the rejected one was never stored, and the 200 body carries the fixed
    // domain sentence for the rejected entry only.
    for (case, bad_position, axis) in [
        ("longitude past the max bound", [200.0, 47.60], "longitude"),
        ("longitude past the min bound", [-200.0, 47.60], "longitude"),
        ("latitude past the max bound", [-122.33, 120.0], "latitude"),
        ("latitude past the min bound", [-122.33, -120.0], "latitude"),
    ] {
        let router = emulator();
        create_tracker(&router, "fleet").await;

        let good: [f64; 2] = [-122.33, 47.60];
        let body = batch_update(
            &router,
            "fleet",
            json!([
                update("good", "2024-01-01T00:00:00Z", good),
                update("bad", "2024-01-01T00:00:00Z", bad_position),
            ]),
        )
        .await;
        let reported = errors(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: only the out-of-domain entry may be reported, got {reported:?}"
        );
        assert_eq!(
            reported[0].get("DeviceId").and_then(Value::as_str),
            Some("bad"),
            "case {case:?}: the reported entry must name the refused device, got {:?}",
            reported[0]
        );
        let expected = expected_message(axis);
        assert_eq!(
            reported[0]
                .get("Error")
                .and_then(|e| e.get("Message"))
                .and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected Error.Message {expected:?}, got {:?}",
            reported[0]
        );

        let committed = call(
            &router,
            "GET",
            "/tracking/v0/trackers/fleet/devices/good/positions/latest",
            None,
        )
        .await;
        assert_eq!(
            committed.status, 200,
            "case {case:?}: the valid entry must be readable, got {} {:?}",
            committed.status, committed.body
        );
        assert_eq!(
            committed.json().get("Position").and_then(Value::as_array),
            Some(&vec![json!(good[0]), json!(good[1])]),
            "case {case:?}: expected the stored Position {good:?}, got {:?}",
            committed.body
        );

        let refused = call(
            &router,
            "GET",
            "/tracking/v0/trackers/fleet/devices/bad/positions/latest",
            None,
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: the refused entry must store nothing, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
    }
}

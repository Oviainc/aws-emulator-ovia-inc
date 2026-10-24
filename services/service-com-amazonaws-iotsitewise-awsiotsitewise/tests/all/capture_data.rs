// SPDX-License-Identifier: CC0-1.0

//! `GetCaptureData` end to end: the video a workspace's capture source holds is read
//! back over HTTP, through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a read of a capture source answers from the segments the
//! producer entry point persisted — every stored segment whose span OVERLAPS the
//! half-open requested window, oldest first, one segment per answer, each reporting
//! that segment's own span and the token that resumes at the next overlapping segment
//! (absent on the last) — a window the source holds no video in is an empty answer, and
//! an identifier that names no source of that workspace, or a window that covers no
//! time, is a modelled refusal rather than a fabricated answer.
//!
//! The tables below span the windows (inside one segment, across two, on a boundary, in
//! a gap, before and after everything, with sub-second offsets), the two identifying
//! forms, the workspace scoping and the refusals.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `hex_token`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::TimeInNanos;
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, CaptureSegment, CaptureWriteRefusal, MAX_CAPTURE_SEGMENTS_PER_SOURCE,
};

use crate::asset_lifecycle::{call, error_code, hex_token, router_with_engine};

/// The workspace every suite below reads from unless it is about workspace scoping.
const WORKSPACE: &str = "capture-floor";

/// The workspace used to prove a capture source is addressable only inside its own.
const OTHER_WORKSPACE: &str = "other-floor";

/// The seeded source's `TimeSeriesId`. A 36-character `ID`-shaped value, inside the
/// modelled `TimeSeriesId` bound of 36..73 (`smithy_query(query="shape",
/// shape="TimeSeriesId")`).
const SOURCE_ID: &str = "7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e3";

/// The `propertyAlias` bound to that source — the second identifying form.
const SOURCE_ALIAS: &str = "/company/windfarm/3/turbine/7/camera";

/// The `VideoDataType` the seeded source's media is in. Source:
/// `smithy_query(query="shape", shape="VideoDataType")` — the enum holds exactly
/// this one value.
const DATA_TYPE: &str = "VIDEO-MP4";

/// One seeded segment: its span and its bytes.
#[derive(Debug, Clone, Copy)]
struct Seed {
    start_seconds: i64,
    start_nanos: i32,
    end_seconds: i64,
    end_nanos: i32,
    data: &'static [u8],
}

/// The seeded video: four segments at different spans, of deliberately different
/// lengths and contents (one single byte, one with non-UTF-8 bytes), with a gap
/// between the third and the fourth and sub-second offsets on two of the bounds, so an
/// answer that selected, ordered, sliced or padded them wrongly cannot pass.
const SEEDED: &[Seed] = &[
    Seed {
        start_seconds: 1_700_000_000,
        start_nanos: 0,
        end_seconds: 1_700_000_010,
        end_nanos: 0,
        data: b"segment-one-mp4-bytes",
    },
    Seed {
        start_seconds: 1_700_000_010,
        start_nanos: 0,
        end_seconds: 1_700_000_020,
        end_nanos: 500_000_000,
        data: b"2",
    },
    Seed {
        start_seconds: 1_700_000_020,
        start_nanos: 500_000_000,
        end_seconds: 1_700_000_030,
        end_nanos: 0,
        data: b"\x00\x01\x02\xff\xfe-third-segment",
    },
    Seed {
        start_seconds: 1_700_000_100,
        start_nanos: 0,
        end_seconds: 1_700_000_160,
        end_nanos: 250_000_000,
        data: b"fourth-segment-bytes-which-are-the-longest-of-the-four",
    },
];

/// `POST /workspaces/{workspaceName}/get-capture-data`, as `OPERATIONS.md` reports the
/// route.
fn capture_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/get-capture-data")
}

/// A modelled `TimeInNanos` on the wire.
fn instant(seconds: i64, nanos: i32) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": nanos })
}

fn time_in_nanos(seconds: i64, nanos: i32) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: Some(nanos),
    }
}

/// Standard base64 with padding — the encoding restJson1 binds a blob member to.
///
/// The runtime's own encoder, the one the sibling suites of this workspace send blob
/// members through, rather than a second copy of the primitive.
fn base64(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

/// The `nextToken` this operation issues to resume at the segment starting at
/// `(seconds, nanos)`: the hex of that start time, built here from the test's own hex
/// helper rather than from the handler's encoder.
fn resume_token(seconds: i64, nanos: i32) -> String {
    hex_token(&format!("{seconds}.{nanos}"))
}

/// Create a workspace with the service's default encryption, over the API.
async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the workspace create to be accepted, got {status} {body}"
    );
}

/// Seed `segments` on the `(workspace, time_series_id)` capture source, through the
/// producer entry point on this service's state — the one writer of a workspace's
/// video, which is what `GetCaptureData` reads back.
fn seed(
    engine: &Arc<engine::InMemoryEngine>,
    workspace: &str,
    time_series_id: &str,
    alias: Option<&str>,
    segments: &[Seed],
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for seed in segments {
        let stored = store.put_capture_segment(
            workspace,
            time_series_id,
            alias,
            DATA_TYPE,
            CaptureSegment {
                start_time: time_in_nanos(seed.start_seconds, seed.start_nanos),
                end_time: time_in_nanos(seed.end_seconds, seed.end_nanos),
                data: seed.data.to_vec(),
            },
        );
        assert_eq!(
            stored,
            Ok(()),
            "seeding {workspace}/{time_series_id}: the segment at \
             {}.{} was refused by the producer entry point",
            seed.start_seconds,
            seed.start_nanos
        );
    }
}

/// A router over a fresh engine carrying the workspaces and the seeded capture source
/// every suite below reads.
async fn seeded() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    seed(&engine, WORKSPACE, SOURCE_ID, Some(SOURCE_ALIAS), SEEDED);
    router
}

/// One `GetCaptureData` call.
async fn get_capture_data(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &capture_uri(workspace), Some(body)).await
}

/// A request body naming the source by `timeSeriesId` over the window
/// `[start, end)`.
fn by_id(window: (i64, i32, i64, i32)) -> Value {
    let (start_seconds, start_nanos, end_seconds, end_nanos) = window;
    json!({
        "timeSeriesId": SOURCE_ID,
        "startTime": instant(start_seconds, start_nanos),
        "endTime": instant(end_seconds, end_nanos),
    })
}

/// The same, naming the source by its `propertyAlias`.
fn by_alias(window: (i64, i32, i64, i32)) -> Value {
    let (start_seconds, start_nanos, end_seconds, end_nanos) = window;
    json!({
        "propertyAlias": SOURCE_ALIAS,
        "startTime": instant(start_seconds, start_nanos),
        "endTime": instant(end_seconds, end_nanos),
    })
}

/// Follow the paging to the end and answer every page's body, with at most one page per
/// seeded segment plus one, so a token that failed to advance cannot loop forever.
async fn walk(router: &MultiServiceRouter, workspace: &str, body: Value) -> Vec<Value> {
    let mut pages = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = body.clone();
        if let Some(token) = &next {
            request
                .as_object_mut()
                .expect("an object body")
                .insert("nextToken".to_string(), json!(token));
        }
        let (status, page) = get_capture_data(router, workspace, request).await;
        assert_eq!(
            status, 200,
            "paging {workspace}: expected 200 on page {}, got {status} {page}",
            pages.len()
        );
        next = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        pages.push(page);
        if next.is_none() {
            return pages;
        }
        assert!(
            pages.len() <= SEEDED.len(),
            "paging {workspace}: the paging did not terminate within {} pages",
            SEEDED.len()
        );
    }
}

/// One row of the window table: the requested range and the seeded segments it must
/// deliver, in order.
#[derive(Debug)]
struct WindowCase {
    label: &'static str,
    window: (i64, i32, i64, i32),
    expected: &'static [usize],
}

/// The rows: windows spanning every relation a range can have to the stored segments.
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            label: "covers-every-segment",
            window: (1_699_999_000, 0, 1_700_001_000, 0),
            expected: &[0, 1, 2, 3],
        },
        WindowCase {
            label: "strictly-inside-one-segment",
            window: (1_700_000_002, 0, 1_700_000_004, 0),
            expected: &[0],
        },
        WindowCase {
            label: "exactly-one-segment",
            window: (1_700_000_000, 0, 1_700_000_010, 0),
            expected: &[0],
        },
        WindowCase {
            label: "starts-where-a-segment-ends",
            window: (1_700_000_010, 0, 1_700_000_015, 0),
            expected: &[1],
        },
        WindowCase {
            label: "ends-where-a-segment-starts",
            window: (1_699_999_900, 0, 1_700_000_000, 0),
            expected: &[],
        },
        WindowCase {
            label: "crosses-two-segments",
            window: (1_700_000_005, 0, 1_700_000_012, 0),
            expected: &[0, 1],
        },
        WindowCase {
            label: "sub-second-offset-selects-the-third-only",
            window: (1_700_000_020, 500_000_001, 1_700_000_029, 0),
            expected: &[2],
        },
        WindowCase {
            label: "sub-second-offset-reaches-back-into-the-second",
            window: (1_700_000_020, 499_999_999, 1_700_000_029, 0),
            expected: &[1, 2],
        },
        WindowCase {
            label: "inside-the-gap-between-segments",
            window: (1_700_000_040, 0, 1_700_000_090, 0),
            expected: &[],
        },
        WindowCase {
            label: "before-every-segment",
            window: (1_000_000_000, 0, 1_600_000_000, 0),
            expected: &[],
        },
        WindowCase {
            label: "after-every-segment",
            window: (1_700_000_200, 0, 1_700_000_300, 0),
            expected: &[],
        },
        WindowCase {
            label: "tail-of-the-last-segment-only",
            window: (1_700_000_150, 0, 1_700_001_000, 0),
            expected: &[3],
        },
    ]
}

/// A window delivers exactly the stored segments it overlaps, oldest first, each
/// carrying its own bytes and its own span, with the resume token of the one after it
/// and none on the last.
#[tokio::test]
async fn a_window_delivers_every_segment_it_overlaps() {
    let router = seeded().await;

    for case in window_cases() {
        let pages = walk(&router, WORKSPACE, by_id(case.window)).await;
        let label = case.label;

        if case.expected.is_empty() {
            let (start_seconds, start_nanos, end_seconds, end_nanos) = case.window;
            assert_eq!(
                pages.len(),
                1,
                "case {label:?}: expected one empty answer, got {} pages: {pages:?}",
                pages.len()
            );
            let page = &pages[0];
            assert_eq!(
                page["data"].as_str().unwrap_or_default(),
                "",
                "case {label:?}: expected no video, got {page}"
            );
            assert_eq!(
                page["dataType"].as_str().unwrap_or_default(),
                DATA_TYPE,
                "case {label:?}: expected the source's data type, got {page}"
            );
            assert_eq!(
                page["startTime"],
                instant(start_seconds, start_nanos),
                "case {label:?}: expected the requested start echoed, got {page}"
            );
            assert_eq!(
                page["endTime"],
                instant(end_seconds, end_nanos),
                "case {label:?}: expected the requested end echoed, got {page}"
            );
            assert!(
                page.get("nextToken").and_then(Value::as_str).is_none(),
                "case {label:?}: expected no nextToken on an empty answer, got {page}"
            );
            continue;
        }

        assert_eq!(
            pages.len(),
            case.expected.len(),
            "case {label:?}: expected {} chunks, got {}: {pages:?}",
            case.expected.len(),
            pages.len()
        );
        for (position, segment) in case.expected.iter().enumerate() {
            let seed = SEEDED[*segment];
            let page = &pages[position];
            assert_eq!(
                page["data"].as_str().unwrap_or_default(),
                base64(seed.data),
                "case {label:?}: chunk {position} expected the bytes of segment \
                 {segment}, got {page}"
            );
            assert_eq!(
                page["startTime"],
                instant(seed.start_seconds, seed.start_nanos),
                "case {label:?}: chunk {position} expected segment {segment}'s own \
                 start, got {page}"
            );
            assert_eq!(
                page["endTime"],
                instant(seed.end_seconds, seed.end_nanos),
                "case {label:?}: chunk {position} expected segment {segment}'s own \
                 end, got {page}"
            );
            assert_eq!(
                page["dataType"].as_str().unwrap_or_default(),
                DATA_TYPE,
                "case {label:?}: chunk {position} expected the source's data type, \
                 got {page}"
            );
            let expected_token = case
                .expected
                .get(position + 1)
                .map(|next| SEEDED[*next])
                .map(|next| resume_token(next.start_seconds, next.start_nanos));
            assert_eq!(
                page.get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                expected_token,
                "case {label:?}: chunk {position} carried the wrong nextToken: {page}"
            );
        }
    }
}

/// The two identifying forms name ONE source: an alias-addressed read and an
/// id-addressed read over the same window answer identically, and each form resolves
/// only inside the source's own workspace.
#[tokio::test]
async fn the_two_identifying_forms_name_one_source() {
    let router = seeded().await;

    for case in window_cases() {
        let label = case.label;
        let (_, from_id) = get_capture_data(&router, WORKSPACE, by_id(case.window)).await;
        let (status, from_alias) =
            get_capture_data(&router, WORKSPACE, by_alias(case.window)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 for the alias form, got {status} {from_alias}"
        );
        assert_eq!(
            from_alias, from_id,
            "case {label:?}: the alias form must answer exactly as the id form, got \
             {from_alias} against {from_id}"
        );
    }

    // The same identifiers, against a workspace that holds no capture source: a source
    // is addressable only inside the workspace that contains it.
    for (label, body) in [
        ("id-in-another-workspace", by_id((1_699_999_000, 0, 1_700_001_000, 0))),
        (
            "alias-in-another-workspace",
            by_alias((1_699_999_000, 0, 1_700_001_000, 0)),
        ),
    ] {
        let (status, body) = get_capture_data(&router, OTHER_WORKSPACE, body).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// One row of the refusal table: the request body, the workspace it is sent to, and the
/// status and modelled error code it must be refused with.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    workspace: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
}

/// The rows: every way a request can fail to name a window or a source.
fn refusal_cases() -> Vec<RefusalCase> {
    let window = instant(1_700_000_000, 0);
    vec![
        RefusalCase {
            label: "end-equals-start",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "startTime": window,
                "endTime": window,
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "end-before-start",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "startTime": instant(1_700_000_010, 0),
                "endTime": instant(1_700_000_000, 0),
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "end-before-start-by-one-nanosecond",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "startTime": instant(1_700_000_000, 1),
                "endTime": instant(1_700_000_000, 0),
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "both-identifiers",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "propertyAlias": SOURCE_ALIAS,
                "startTime": instant(1_700_000_000, 0),
                "endTime": instant(1_700_000_010, 0),
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "no-identifier-at-all",
            workspace: WORKSPACE,
            body: json!({
                "startTime": instant(1_700_000_000, 0),
                "endTime": instant(1_700_000_010, 0),
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "unknown-workspace",
            workspace: "no-such-workspace",
            body: by_id((1_700_000_000, 0, 1_700_000_010, 0)),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "unknown-time-series-id",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": "11111111-2222-4333-8444-555555555555",
                "startTime": instant(1_700_000_000, 0),
                "endTime": instant(1_700_000_010, 0),
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "unknown-alias",
            workspace: WORKSPACE,
            body: json!({
                "propertyAlias": "/company/windfarm/3/turbine/7/no-such-camera",
                "startTime": instant(1_700_000_000, 0),
                "endTime": instant(1_700_000_010, 0),
            }),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "token-this-operation-never-issued",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "startTime": instant(1_699_999_000, 0),
                "endTime": instant(1_700_001_000, 0),
                "nextToken": "not-a-token-this-service-minted",
            }),
            status: 400,
            code: "InvalidRequestException",
        },
        RefusalCase {
            label: "token-naming-a-segment-outside-the-window",
            workspace: WORKSPACE,
            body: json!({
                "timeSeriesId": SOURCE_ID,
                "startTime": instant(1_700_000_000, 0),
                "endTime": instant(1_700_000_010, 0),
                // Well-formed, and the start of a stored segment — but not one this
                // window selects, so it resumes nothing.
                "nextToken": resume_token(1_700_000_100, 0),
            }),
            status: 400,
            code: "InvalidRequestException",
        },
    ]
}

/// Every malformed or unresolvable request is refused with the modelled error, and none
/// of them is answered with video.
#[tokio::test]
async fn malformed_and_unresolvable_requests_are_refused() {
    let router = seeded().await;

    for case in refusal_cases() {
        let label = case.label;
        let (status, body) = get_capture_data(&router, case.workspace, case.body.clone()).await;
        assert_eq!(
            status, case.status,
            "case {label:?}: expected {}, got {status} {body}",
            case.status
        );
        assert!(
            error_code(&body).contains(case.code),
            "case {label:?}: expected {}, got {body}",
            case.code
        );
        assert!(
            body.get("data").is_none(),
            "case {label:?}: a refusal must carry no video, got {body}"
        );
    }
}

/// Reading capture data changes nothing a later caller sees: the same window answers
/// identically on a second call, and the stored segments are exactly those seeded.
#[tokio::test]
async fn reading_capture_data_writes_nothing() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    seed(&engine, WORKSPACE, SOURCE_ID, Some(SOURCE_ALIAS), SEEDED);

    let window = (1_699_999_000, 0, 1_700_001_000, 0);
    let first = walk(&router, WORKSPACE, by_id(window)).await;
    let second = walk(&router, WORKSPACE, by_alias(window)).await;
    assert_eq!(
        second, first,
        "case \"re-read\": a second read must answer exactly as the first, got \
         {second:?} against {first:?}"
    );

    let state = service_state(&*engine).expect("the IoT SiteWise store");
    let store = state.lock();
    let source = store
        .capture_source_by_id(WORKSPACE, SOURCE_ID)
        .expect("the seeded capture source");
    assert_eq!(
        source.segments.len(),
        SEEDED.len(),
        "case \"re-read\": expected the {} seeded segments to be untouched, got {}",
        SEEDED.len(),
        source.segments.len()
    );
    assert_eq!(
        source.alias.as_deref(),
        Some(SOURCE_ALIAS),
        "case \"re-read\": expected the seeded alias to still be bound, got {:?}",
        source.alias
    );
}

/// The producer entry point refuses what it cannot store, and bounds one source's
/// video: a segment for a workspace this store does not hold, an identifier or data
/// type the model cannot express, and a segment covering no time are all written
/// nowhere, and past [`MAX_CAPTURE_SEGMENTS_PER_SOURCE`] segments the oldest goes.
#[tokio::test]
async fn the_producer_entry_point_refuses_what_it_cannot_store() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;

    let segment = |start: i64, end: i64| CaptureSegment {
        start_time: time_in_nanos(start, 0),
        end_time: time_in_nanos(end, 0),
        data: b"bytes".to_vec(),
    };

    let state = service_state(&*engine).expect("the IoT SiteWise store");
    {
        let mut store = state.lock();
        for (label, workspace, id, alias, data_type, span, expected) in [
            (
                "unknown-workspace",
                "no-such-workspace",
                SOURCE_ID,
                None,
                DATA_TYPE,
                (1_700_000_000, 1_700_000_010),
                CaptureWriteRefusal::NoSuchWorkspace,
            ),
            (
                "short-time-series-id",
                WORKSPACE,
                "too-short",
                None,
                DATA_TYPE,
                (1_700_000_000, 1_700_000_010),
                CaptureWriteRefusal::TimeSeriesIdLength,
            ),
            (
                "long-time-series-id",
                WORKSPACE,
                // 74 characters: one past the modelled maximum of 73.
                "7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e3.7f3a1c24-9b85-4d17-8e62-0a5c41d9b7e34",
                None,
                DATA_TYPE,
                (1_700_000_000, 1_700_000_010),
                CaptureWriteRefusal::TimeSeriesIdLength,
            ),
            (
                "empty-alias",
                WORKSPACE,
                SOURCE_ID,
                Some(""),
                DATA_TYPE,
                (1_700_000_000, 1_700_000_010),
                CaptureWriteRefusal::AliasLength,
            ),
            (
                "unmodelled-data-type",
                WORKSPACE,
                SOURCE_ID,
                None,
                "VIDEO-AVI",
                (1_700_000_000, 1_700_000_010),
                CaptureWriteRefusal::UnmodelledDataType,
            ),
            (
                "empty-span",
                WORKSPACE,
                SOURCE_ID,
                None,
                DATA_TYPE,
                (1_700_000_000, 1_700_000_000),
                CaptureWriteRefusal::EmptySpan,
            ),
            (
                "reversed-span",
                WORKSPACE,
                SOURCE_ID,
                None,
                DATA_TYPE,
                (1_700_000_010, 1_700_000_000),
                CaptureWriteRefusal::EmptySpan,
            ),
            (
                "unmodelled-instant",
                WORKSPACE,
                SOURCE_ID,
                None,
                DATA_TYPE,
                // `timeInSeconds` is `range 1..`, so zero is outside it.
                (0, 1_700_000_010),
                CaptureWriteRefusal::UnmodelledInstant,
            ),
        ] {
            let refusal =
                store.put_capture_segment(workspace, id, alias, data_type, segment(span.0, span.1));
            assert_eq!(
                refusal,
                Err(expected),
                "case {label:?}: expected the producer entry point to refuse with \
                 {expected:?}, got {refusal:?}"
            );
            assert!(
                store.capture_sources.is_empty(),
                "case {label:?}: a refused segment must create no capture source, got {:?}",
                store.capture_sources
            );
        }

        // One more segment than the bound: the oldest is the one that goes, so the
        // source keeps exactly the cap and its earliest segment is the second written.
        let first_second = 1_600_000_000;
        for offset in 0..=(MAX_CAPTURE_SEGMENTS_PER_SOURCE as i64) {
            let start = first_second + offset * 10;
            let stored = store.put_capture_segment(
                WORKSPACE,
                SOURCE_ID,
                None,
                DATA_TYPE,
                segment(start, start + 10),
            );
            assert_eq!(
                stored,
                Ok(()),
                "case \"bound\": expected the segment starting at {start} to be stored"
            );
        }
        let source = store
            .capture_source_by_id(WORKSPACE, SOURCE_ID)
            .expect("the capture source the bounded writes created");
        assert_eq!(
            source.segments.len(),
            MAX_CAPTURE_SEGMENTS_PER_SOURCE,
            "case \"bound\": expected the log to stop at {MAX_CAPTURE_SEGMENTS_PER_SOURCE}, got {}",
            source.segments.len()
        );
        assert_eq!(
            source.segments[0].start_time.time_in_seconds,
            first_second + 10,
            "case \"bound\": expected the oldest segment to have been dropped, got {}",
            source.segments[0].start_time.time_in_seconds
        );

        // A write naming ANOTHER media type for a source that already holds video is
        // refused rather than accepted and silently reported as the stored type. The
        // modelled `VideoDataType` declares exactly one value today, so such a type is
        // refused as unmodelled first; the mismatch check behind it is what keeps the
        // rule true if the enum ever grows.
        let mismatched = store.put_capture_segment(
            WORKSPACE,
            SOURCE_ID,
            None,
            "VIDEO-AVI",
            segment(1_700_000_000, 1_700_000_010),
        );
        assert_eq!(
            mismatched,
            Err(CaptureWriteRefusal::UnmodelledDataType),
            "case \"other-data-type\": an unmodelled type is refused as such, got {mismatched:?}"
        );
    }

    // A re-ingest of a span the source already holds replaces the bytes stored for it
    // rather than storing the span twice.
    {
        let mut store = state.lock();
        let stored = store.put_capture_segment(
            WORKSPACE,
            SOURCE_ID,
            None,
            DATA_TYPE,
            CaptureSegment {
                start_time: time_in_nanos(1_600_000_010, 0),
                end_time: time_in_nanos(1_600_000_020, 0),
                data: b"re-ingested".to_vec(),
            },
        );
        assert_eq!(
            stored,
            Ok(()),
            "case \"re-ingest\": expected the segment to be stored, got {stored:?}"
        );
        let source = store
            .capture_source_by_id(WORKSPACE, SOURCE_ID)
            .expect("the capture source");
        assert_eq!(
            source.segments.len(),
            MAX_CAPTURE_SEGMENTS_PER_SOURCE,
            "case \"re-ingest\": expected the segment count to stay at the cap, got {}",
            source.segments.len()
        );
        assert_eq!(
            source.segments[0].data,
            b"re-ingested".to_vec(),
            "case \"re-ingest\": expected the stored bytes to be replaced, got {:?}",
            source.segments[0].data
        );
    }

    // And the read path answers from exactly that: the re-ingested bytes.
    let (status, body) = get_capture_data(
        &router,
        WORKSPACE,
        by_id((1_600_000_010, 0, 1_600_000_020, 0)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"re-ingest\": expected 200, got {status} {body}"
    );
    assert_eq!(
        body["data"].as_str().unwrap_or_default(),
        base64(b"re-ingested"),
        "case \"re-ingest\": expected the re-ingested bytes, got {body}"
    );
}

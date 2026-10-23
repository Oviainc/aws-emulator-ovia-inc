// SPDX-License-Identifier: CC0-1.0

//! AddFlowMediaStreams, end to end over HTTP.
//!
//! The rule under test: adding media streams to a flow materialises EVERY requested
//! `AddMediaStreamRequest` into the flow's stored record — keeping each member the
//! request names, assigning the `Fmt` (RTP payload type) MediaConnect owns so that no
//! two media streams of one flow share one, and defaulting `ClockRate` to the 90000 Hz
//! the model documents for video and ancillary data — and reports those stored records,
//! which is what DescribeFlow then reports too. A request the flow cannot accept (an
//! unknown flow, a repeated media stream name or id, a flow already at its bound) is
//! refused with the modelled error and stores nothing.
//!
//! The cases drive the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;

/// The ceiling on how many media streams one flow holds.
const LIMIT: usize = mc::MAX_FLOW_MEDIA_STREAMS;

/// Create a flow named `name`, optionally already carrying `media_streams`, and return
/// its ARN.
///
/// `pub(crate)` because `tests/all/update_flow_media_stream.rs` sets up its flows the same
/// way.
pub(crate) async fn create_flow(emulator: &Emulator, name: &str, media_streams: Value) -> String {
    let mut body = json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
    });
    if !media_streams.is_null() {
        body["mediaStreams"] = media_streams;
    }
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "setup: CreateFlow {name:?} expected 201 with no error, got {status} {code:?}: \
         {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `POST /v1/flows/{FlowArn}/mediaStreams` — the AddFlowMediaStreams binding
/// (`smithy_query(query="operation",operation="AddFlowMediaStreams")`).
async fn add_media_streams(
    emulator: &Emulator,
    flow_arn: &str,
    media_streams: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/mediaStreams"),
            Some(json!({ "mediaStreams": media_streams })),
        )
        .await
}

/// The media streams DescribeFlow reports for `flow_arn`.
///
/// `pub(crate)` because `tests/all/update_flow_media_stream.rs` reads a flow's media
/// streams back through the same reader.
pub(crate) async fn described_media_streams(emulator: &Emulator, flow_arn: &str) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DescribeFlow expected 200 with no error, got {status} {code:?}: {described}"
    );
    media_streams(&described["flow"])
}

/// The `MediaStreams` member of a response body (or of a reported flow), as a list.
pub(crate) fn media_streams(body: &Value) -> Vec<Value> {
    body["mediaStreams"].as_array().cloned().unwrap_or_default()
}

/// The `MediaStreamAttributes` the service must report for a requested
/// `MediaStreamAttributesRequest`: `Fmtp` is a required member of the reported shape,
/// so a request that names no `fmtp` is reported with an empty one.
fn expected_attributes(request: &Value) -> Value {
    let mut expected = json!({ "fmtp": request.get("fmtp").cloned().unwrap_or_else(|| json!({})) });
    if let Some(lang) = request.get("lang") {
        expected["lang"] = lang.clone();
    }
    expected
}

/// The media stream the service must have stored for one `AddMediaStreamRequest`, given
/// the payload type `fmt` it assigned: every member the request named, and the
/// documented clock-rate fallback. `MediaStreamTags` is deliberately absent — the
/// reported `MediaStream` shape models no tag member.
fn expected_stream(request: &Value, fmt: i32) -> Value {
    let mut expected = json!({
        "fmt": fmt,
        "mediaStreamId": request["mediaStreamId"].clone(),
        "mediaStreamName": request["mediaStreamName"].clone(),
        "mediaStreamType": request["mediaStreamType"].clone(),
    });
    // `AddMediaStreamRequest$ClockRate`: "If the media stream type is video or ancillary
    // data, set this value to 90000. If the media stream type is audio, set this value to
    // either 48000 or 96000" — so video and ancillary data have one documented rate to
    // fall back on and audio has none.
    if let Some(clock_rate) = request.get("clockRate") {
        expected["clockRate"] = clock_rate.clone();
    } else if matches!(
        request["mediaStreamType"].as_str(),
        Some(mc::MEDIA_STREAM_TYPE_VIDEO) | Some(mc::MEDIA_STREAM_TYPE_ANCILLARY_DATA)
    ) {
        expected["clockRate"] = json!(mc::VIDEO_CLOCK_RATE_HZ);
    }
    for member in ["description", "videoFormat"] {
        if let Some(value) = request.get(member) {
            expected[member] = value.clone();
        }
    }
    if let Some(attributes) = request.get("attributes") {
        expected["attributes"] = expected_attributes(attributes);
    }
    expected
}

/// One row of the AddFlowMediaStreams table: a flow name and the `MediaStreams` the
/// request carries. The expectation is COMPUTED from the request by [`expected_stream`],
/// so a row states only its input.
struct AddCase {
    case: &'static str,
    flow: &'static str,
    requested: Value,
}

/// The rows span the class the rule generalises over: each of the three media stream
/// types the model declares (`smithy_query(query="shape",shape="MediaStreamType")`:
/// video | audio | ancillary-data), a clock rate named and omitted, the optional members
/// present and absent, attributes with and without `fmtp`, and one, two and three
/// streams in a single request.
fn add_cases() -> Vec<AddCase> {
    vec![
        AddCase {
            case: "one video stream, every optional member omitted",
            flow: "ms-video-defaults",
            requested: json!([
                { "mediaStreamId": 1, "mediaStreamName": "vid-1", "mediaStreamType": "video" }
            ]),
        },
        AddCase {
            case: "video stream naming a clock rate, a description and a video format",
            flow: "ms-video-full",
            requested: json!([
                {
                    "mediaStreamId": 7,
                    "mediaStreamName": "vid_full",
                    "mediaStreamType": "video",
                    "clockRate": 90_000,
                    "description": "the programme feed",
                    "videoFormat": "1080i50",
                    "attributes": {
                        "lang": "en",
                        "fmtp": { "colorimetry": "BT709", "exactFramerate": "25", "scanMode": "interlace" }
                    }
                }
            ]),
        },
        AddCase {
            case: "audio stream at 48 kHz with language attributes and tags",
            flow: "ms-audio-48k",
            requested: json!([
                {
                    "mediaStreamId": 2,
                    "mediaStreamName": "aud-48",
                    "mediaStreamType": "audio",
                    "clockRate": 48_000,
                    "attributes": { "lang": "de", "fmtp": { "channelOrder": "SMPTE2110.(ST)" } },
                    "mediaStreamTags": { "team": "audio" }
                }
            ]),
        },
        AddCase {
            case: "audio stream naming no clock rate keeps none: audio has no single \
                   documented rate",
            flow: "ms-audio-no-rate",
            requested: json!([
                { "mediaStreamId": 3, "mediaStreamName": "aud-default", "mediaStreamType": "audio" }
            ]),
        },
        AddCase {
            case: "ancillary-data stream, attributes without fmtp",
            flow: "ms-ancillary",
            requested: json!([
                {
                    "mediaStreamId": 4,
                    "mediaStreamName": "anc-1",
                    "mediaStreamType": "ancillary-data",
                    "description": "closed captions",
                    "attributes": { "lang": "en" }
                }
            ]),
        },
        AddCase {
            case: "three streams of three types in one request",
            flow: "ms-three",
            requested: json!([
                { "mediaStreamId": 10, "mediaStreamName": "v", "mediaStreamType": "video" },
                { "mediaStreamId": 20, "mediaStreamName": "a", "mediaStreamType": "audio", "clockRate": 96_000 },
                { "mediaStreamId": 30, "mediaStreamName": "anc", "mediaStreamType": "ancillary-data" }
            ]),
        },
        AddCase {
            case: "two audio streams whose ids and names differ only in their last character",
            flow: "ms-pair",
            requested: json!([
                { "mediaStreamId": 100, "mediaStreamName": "pair-a", "mediaStreamType": "audio", "clockRate": 48_000 },
                { "mediaStreamId": 101, "mediaStreamName": "pair-b", "mediaStreamType": "audio", "clockRate": 48_000 }
            ]),
        },
    ]
}

/// AddFlowMediaStreams materialises every requested media stream into the flow's stored
/// record, answers with those stored records, and DescribeFlow then reports the same
/// ones: the response is read out of the store, not echoed from the request.
#[tokio::test]
async fn add_flow_media_streams_persists_every_requested_stream() {
    let emulator = Emulator::new();

    for AddCase {
        case,
        flow,
        requested,
    } in add_cases()
    {
        let flow_arn = create_flow(&emulator, flow, Value::Null).await;
        let (status, code, added) = add_media_streams(&emulator, &flow_arn, requested.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {added}"
        );
        assert_eq!(
            added["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the streams were added to, got {added}"
        );

        let requested = requested.as_array().cloned().unwrap_or_default();
        let reported = media_streams(&added);
        assert_eq!(
            reported.len(),
            requested.len(),
            "case {case:?}: one reported media stream per requested one, got {added}"
        );
        // The flow held no media stream before this call, so the payload types the
        // service assigns run from the first dynamic RTP payload type upwards.
        for (index, request) in requested.iter().enumerate() {
            let fmt = mc::DYNAMIC_RTP_PAYLOAD_TYPE_BASE + i32::try_from(index).unwrap_or(0);
            let expected = expected_stream(request, fmt);
            assert_eq!(
                reported[index], expected,
                "case {case:?}: media stream {index} expected {expected}, got {}",
                reported[index]
            );
        }

        let described = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            described, reported,
            "case {case:?}: DescribeFlow reports the stored media streams, expected \
             {reported:?}, got {described:?}"
        );
    }
}

/// `MediaStreamTags` is kept on the stored record even though the reported `MediaStream`
/// shape has no member for it, so nothing the request carried is dropped.
#[tokio::test]
async fn add_flow_media_streams_stores_the_tags_the_request_carried() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(&emulator, "ms-tagged", Value::Null).await;
    let (status, code, added) = add_media_streams(
        &emulator,
        &flow_arn,
        json!([
            {
                "mediaStreamId": 1,
                "mediaStreamName": "tagged",
                "mediaStreamType": "video",
                "mediaStreamTags": { "owner": "playout", "tier": "primary" }
            },
            { "mediaStreamId": 2, "mediaStreamName": "untagged", "mediaStreamType": "video" }
        ]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "expected 201 with no error, got {status} {code:?}: {added}"
    );
    assert!(
        media_streams(&added)
            .iter()
            .all(|stream| stream.get("mediaStreamTags").is_none()),
        "the reported MediaStream shape models no tags, got {added}"
    );

    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    let stored = store
        .flows
        .get(&flow_arn)
        .expect("the flow the streams were added to is stored");
    let tags: Vec<(&str, Vec<(&str, &str)>)> = stored
        .media_streams
        .iter()
        .map(|stream| {
            (
                stream.stream.media_stream_name.as_str(),
                stream
                    .tags
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        tags,
        vec![
            ("tagged", vec![("owner", "playout"), ("tier", "primary")]),
            ("untagged", Vec::new()),
        ],
        "the stored media streams keep the tags their request carried, got {tags:?}"
    );
}

/// `MediaStream$Fmt` is assigned by the service, and a flow's payload types are unique:
/// whatever order and in however many calls its media streams arrive, every stream of
/// one flow has its own `Fmt`, drawn from the dynamic RTP payload types upwards.
#[tokio::test]
async fn add_flow_media_streams_assigns_a_unique_payload_type_per_flow() {
    let emulator = Emulator::new();
    // A flow that already carries a media stream from CreateFlow, so the numbering this
    // operation continues is the stored flow's, not the request's.
    let flow_arn = create_flow(
        &emulator,
        "ms-fmt",
        json!([{ "mediaStreamId": 1, "mediaStreamName": "from-create", "mediaStreamType": "video" }]),
    )
    .await;

    // Three further calls, of three different sizes.
    let calls = [
        json!([{ "mediaStreamId": 2, "mediaStreamName": "added-1", "mediaStreamType": "audio", "clockRate": 48_000 }]),
        json!([
            { "mediaStreamId": 3, "mediaStreamName": "added-2", "mediaStreamType": "video" },
            { "mediaStreamId": 4, "mediaStreamName": "added-3", "mediaStreamType": "ancillary-data" }
        ]),
        json!([{ "mediaStreamId": 5, "mediaStreamName": "added-4", "mediaStreamType": "video" }]),
    ];
    let mut held = 1usize;
    for (call, requested) in calls.iter().enumerate() {
        let (status, code, added) =
            add_media_streams(&emulator, &flow_arn, requested.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "call {call}: expected 201 with no error, got {status} {code:?}: {added}"
        );
        // A call reports only the streams IT added, not the whole flow.
        let reported = media_streams(&added);
        assert_eq!(
            reported.len(),
            requested.as_array().map(Vec::len).unwrap_or_default(),
            "call {call}: the response carries only the streams this call added, got {added}"
        );
        held += reported.len();

        let described = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            described.len(),
            held,
            "call {call}: the flow accumulates its media streams, expected {held}, got \
             {described:?}"
        );
        let fmts: Vec<i64> = described
            .iter()
            .map(|stream| stream["fmt"].as_i64().unwrap_or_default())
            .collect();
        let mut unique = fmts.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            fmts.len(),
            "call {call}: a flow's payload types are unique, got {fmts:?}"
        );
        assert!(
            fmts.iter()
                .all(|fmt| *fmt >= i64::from(mc::DYNAMIC_RTP_PAYLOAD_TYPE_BASE)),
            "call {call}: payload types are assigned from the dynamic range upwards, got \
             {fmts:?}"
        );
        // Nothing has been removed, so the flow holds exactly the run of payload types
        // starting at the base.
        let expected: Vec<i64> = (0..held as i64)
            .map(|offset| i64::from(mc::DYNAMIC_RTP_PAYLOAD_TYPE_BASE) + offset)
            .collect();
        assert_eq!(
            fmts, expected,
            "call {call}: expected the assigned payload types {expected:?}, got {fmts:?}"
        );
    }
}

/// One row of the refusal table: a request this flow cannot accept, and the modelled
/// answer for it.
struct RefusalCase {
    case: &'static str,
    requested: Value,
    status: u16,
    code: &'static str,
}

/// Each refusal is the modelled error for its cause, and it stores NOTHING: the flow
/// below already holds two media streams and must still hold exactly those two after
/// every row.
#[tokio::test]
async fn add_flow_media_streams_refuses_what_a_flow_cannot_accept() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(
        &emulator,
        "ms-refusals",
        json!([
            { "mediaStreamId": 1, "mediaStreamName": "held-1", "mediaStreamType": "video" },
            { "mediaStreamId": 2, "mediaStreamName": "held-2", "mediaStreamType": "audio", "clockRate": 48_000 }
        ]),
    )
    .await;
    let before = described_media_streams(&emulator, &flow_arn).await;

    for RefusalCase {
        case,
        requested,
        status: expected_status,
        code: expected_code,
    } in [
        RefusalCase {
            case: "no media stream at all",
            requested: json!([]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a name the flow already holds",
            requested: json!([
                { "mediaStreamId": 9, "mediaStreamName": "held-1", "mediaStreamType": "video" }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an id the flow already holds",
            requested: json!([
                { "mediaStreamId": 2, "mediaStreamName": "new-name", "mediaStreamType": "video" }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "one request repeating a name within itself",
            requested: json!([
                { "mediaStreamId": 11, "mediaStreamName": "twice", "mediaStreamType": "video" },
                { "mediaStreamId": 12, "mediaStreamName": "twice", "mediaStreamType": "audio", "clockRate": 48_000 }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "one request repeating an id within itself",
            requested: json!([
                { "mediaStreamId": 13, "mediaStreamName": "first", "mediaStreamType": "video" },
                { "mediaStreamId": 13, "mediaStreamName": "second", "mediaStreamType": "video" }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an empty media stream name, which nothing could address",
            requested: json!([
                { "mediaStreamId": 14, "mediaStreamName": "", "mediaStreamType": "video" }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a second entry the flow cannot accept refuses the whole request",
            requested: json!([
                { "mediaStreamId": 15, "mediaStreamName": "fine", "mediaStreamType": "video" },
                { "mediaStreamId": 1, "mediaStreamName": "clashing", "mediaStreamType": "video" }
            ]),
            status: 400,
            code: "BadRequestException",
        },
    ] {
        let (status, code, refused) = add_media_streams(&emulator, &flow_arn, requested).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {refused}"
        );
        let after = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request stores nothing, expected {before:?}, got \
             {after:?}"
        );
    }

    // A flow this account never created is not found, whatever the request body says.
    let unknown = engine::emulator_arn(
        mc::SERVICE_NAMESPACE,
        "us-east-1",
        &mc::flow_resource_fragment(9_999, "no-such-flow"),
    );
    let (status, code, refused) = add_media_streams(
        &emulator,
        &unknown,
        json!([
            { "mediaStreamId": 1, "mediaStreamName": "orphan", "mediaStreamType": "video" }
        ]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "case \"a flow that does not exist\": expected 404 NotFoundException, got {status} \
         {code:?}: {refused}"
    );
}

/// The uniqueness rule belongs to the FLOW, not to one operation: CreateFlow's
/// `MediaStreams` adds media streams to a flow too, so it refuses exactly the lists
/// AddFlowMediaStreams refuses and no flow can be created in a state the add would never
/// permit.
#[tokio::test]
async fn create_flow_refuses_media_streams_that_would_repeat_within_a_flow() {
    let emulator = Emulator::new();

    for (row, (case, media_streams, expected_status, expected_code)) in [
        (
            "two entries with one name",
            json!([
                { "mediaStreamId": 1, "mediaStreamName": "same", "mediaStreamType": "video" },
                { "mediaStreamId": 2, "mediaStreamName": "same", "mediaStreamType": "audio", "clockRate": 48_000 }
            ]),
            400,
            Some("BadRequestException"),
        ),
        (
            "two entries with one id",
            json!([
                { "mediaStreamId": 3, "mediaStreamName": "first", "mediaStreamType": "video" },
                { "mediaStreamId": 3, "mediaStreamName": "second", "mediaStreamType": "video" }
            ]),
            400,
            Some("BadRequestException"),
        ),
        (
            "an empty media stream name",
            json!([
                { "mediaStreamId": 4, "mediaStreamName": "", "mediaStreamType": "video" }
            ]),
            400,
            Some("BadRequestException"),
        ),
        (
            "distinct names and ids, which the flow can hold",
            json!([
                { "mediaStreamId": 5, "mediaStreamName": "one", "mediaStreamType": "video" },
                { "mediaStreamId": 6, "mediaStreamName": "two", "mediaStreamType": "ancillary-data" }
            ]),
            201,
            None,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("ms-create-{row}");
        let (status, code, body) = emulator
            .send(
                "POST",
                "/v1/flows",
                Some(json!({
                    "name": name,
                    "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                    "mediaStreams": media_streams
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code:?}, got {status} \
             {code:?}: {body}"
        );
    }
}

/// How many media streams a flow may HOLD is bounded, so what it already holds counts
/// towards the bound: the call that fills the last slot is accepted and the one past it
/// is refused with the modelled `BadRequestException`, storing nothing.
#[tokio::test]
async fn add_flow_media_streams_bounds_what_a_flow_holds() {
    /// `count` distinct media stream requests, numbered from `first`.
    fn streams(first: usize, count: usize) -> Value {
        (first..first + count)
            .map(|index| {
                json!({
                    "mediaStreamId": index,
                    "mediaStreamName": format!("stream-{index}"),
                    "mediaStreamType": "video"
                })
            })
            .collect()
    }

    let emulator = Emulator::new();

    // One short of the bound, then the call that fills the last slot.
    let flow_arn = create_flow(&emulator, "ms-at-bound", streams(0, LIMIT - 1)).await;
    let (status, code, added) =
        add_media_streams(&emulator, &flow_arn, streams(LIMIT - 1, 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the call that fills the last slot ({LIMIT}) expected 201, got {status} {code:?}: \
         {added}"
    );
    let at_bound = described_media_streams(&emulator, &flow_arn).await;
    assert_eq!(
        at_bound.len(),
        LIMIT,
        "at the bound the flow holds {LIMIT} media streams, got {}",
        at_bound.len()
    );

    // One past it, and nothing is stored.
    let (status, code, refused) = add_media_streams(&emulator, &flow_arn, streams(LIMIT, 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "one media stream past the bound ({LIMIT}) expected 400 BadRequestException, got \
         {status} {code:?}: {refused}"
    );
    let after = described_media_streams(&emulator, &flow_arn).await;
    assert_eq!(
        after, at_bound,
        "a refused request stores nothing, expected {} streams, got {}",
        at_bound.len(),
        after.len()
    );

    // The bound is on the flow, not on one request: an empty flow accepts a whole
    // bound's worth in one call, and then no more.
    let empty = create_flow(&emulator, "ms-bound-in-one-call", Value::Null).await;
    let (status, code, added) = add_media_streams(&emulator, &empty, streams(0, LIMIT)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "a bound's worth in one call expected 201, got {status} {code:?}: {added}"
    );
    let (status, code, refused) = add_media_streams(&emulator, &empty, streams(LIMIT, 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "one past the bound expected 400 BadRequestException, got {status} {code:?}: {refused}"
    );
}

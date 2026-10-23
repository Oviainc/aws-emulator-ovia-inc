// SPDX-License-Identifier: CC0-1.0

//! UpdateFlowMediaStream, end to end over HTTP: a partial update of ONE stored media
//! stream of a flow.
//!
//! The rule these cases assert, over a table rather than over one request: a member the
//! request carries replaces that setting on the stored media stream, a member it omits
//! leaves the stored setting untouched, the stream's identity (its name, its
//! service-assigned `Fmt` and its `MediaStreamId`) never moves, and the 202 answer is the
//! stored record — the same bytes DescribeFlow then reports. A request that resolves no
//! flow or no media stream of that flow is refused with the modelled error and writes
//! nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_media_streams::{create_flow, described_media_streams};
use crate::flow_outputs::unknown_flow_arn;

/// `PUT /v1/flows/{FlowArn}/mediaStreams/{MediaStreamName}` — the UpdateFlowMediaStream
/// binding (`smithy_query(query="operation",operation="UpdateFlowMediaStream")`).
async fn update_media_stream(
    emulator: &Emulator,
    flow_arn: &str,
    media_stream_name: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}/mediaStreams/{media_stream_name}"),
            Some(body),
        )
        .await
}

/// The value at a dotted path of a JSON object: `attributes.fmtp.colorimetry`.
fn at_path<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = &current[segment];
    }
    current
}

/// `value` with the given top-level keys removed, so two media streams can be compared on
/// everything the request did NOT touch.
fn without_keys(value: &Value, keys: &[&str]) -> Value {
    let mut stripped = value.clone();
    if let Some(object) = stripped.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
    stripped
}

/// The media stream of `flow_arn` named `name`, as DescribeFlow reports it.
async fn described_stream(emulator: &Emulator, case: &str, flow_arn: &str, name: &str) -> Value {
    described_media_streams(emulator, flow_arn)
        .await
        .into_iter()
        .find(|stream| stream["mediaStreamName"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: the flow still holds the media stream {name:?}"))
}

/// One row of the UpdateFlowMediaStream table: the request body, and the paths of the
/// stored media stream it must change, with the values they must take.
struct UpdateCase {
    case: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// The rows differ along what the rule generalises over: which member the request carries
/// (a scalar string, an integer, an enum, the nested request-only
/// `MediaStreamAttributesRequest` wrapper and its nested `FmtpRequest`), how many at once,
/// whether the setting existed before, and a request that carries none at all. They are
/// applied in order to the SAME media stream, so each row also proves the settings the
/// earlier rows wrote survive an update that does not mention them.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a description on its own",
            body: json!({ "description": "the programme feed" }),
            expect: vec![("description", json!("the programme feed"))],
        },
        UpdateCase {
            case: "the description replaced by another",
            body: json!({ "description": "the backup feed" }),
            expect: vec![("description", json!("the backup feed"))],
        },
        UpdateCase {
            case: "a video format on a stream that had none",
            body: json!({ "videoFormat": "1080i50" }),
            expect: vec![("videoFormat", json!("1080i50"))],
        },
        UpdateCase {
            case: "the clock rate and the video format together",
            body: json!({ "clockRate": 48_000, "videoFormat": "2160p60" }),
            expect: vec![
                ("clockRate", json!(48_000)),
                ("videoFormat", json!("2160p60")),
            ],
        },
        UpdateCase {
            // `MediaStreamAttributes$Fmtp` is required on the reported shape, so attributes
            // given by a request that names no `fmtp` report an empty one.
            case: "attributes carrying only a language",
            body: json!({ "attributes": { "lang": "en" } }),
            expect: vec![("attributes", json!({ "lang": "en", "fmtp": {} }))],
        },
        UpdateCase {
            // `FmtpRequest`'s members are each optional, so one member of the fmtp changes
            // and its siblings keep what the earlier rows wrote.
            case: "two fmtp members, leaving the language alone",
            body: json!({ "attributes": { "fmtp": { "colorimetry": "BT709", "exactFramerate": "25" } } }),
            expect: vec![(
                "attributes",
                json!({
                    "lang": "en",
                    "fmtp": { "colorimetry": "BT709", "exactFramerate": "25" }
                }),
            )],
        },
        UpdateCase {
            case: "one fmtp member changed and one added",
            body: json!({ "attributes": { "fmtp": { "colorimetry": "BT2020", "scanMode": "interlace" } } }),
            expect: vec![(
                "attributes",
                json!({
                    "lang": "en",
                    "fmtp": {
                        "colorimetry": "BT2020",
                        "exactFramerate": "25",
                        "scanMode": "interlace"
                    }
                }),
            )],
        },
        UpdateCase {
            case: "the language changed, leaving every fmtp member alone",
            body: json!({ "attributes": { "lang": "de" } }),
            expect: vec![(
                "attributes",
                json!({
                    "lang": "de",
                    "fmtp": {
                        "colorimetry": "BT2020",
                        "exactFramerate": "25",
                        "scanMode": "interlace"
                    }
                }),
            )],
        },
        UpdateCase {
            // `MediaStreamType` is one of video | audio | ancillary-data
            // (`smithy_query(query="shape",shape="MediaStreamType")`).
            case: "the media stream type changed to ancillary-data",
            body: json!({ "mediaStreamType": "ancillary-data" }),
            expect: vec![("mediaStreamType", json!("ancillary-data"))],
        },
        UpdateCase {
            case: "the media stream type changed back to video",
            body: json!({ "mediaStreamType": "video" }),
            expect: vec![("mediaStreamType", json!("video"))],
        },
        UpdateCase {
            case: "every member of the operation at once",
            body: json!({
                "clockRate": 96_000,
                "description": "everything at once",
                "mediaStreamType": "audio",
                "videoFormat": "720p50",
                "attributes": { "lang": "fr", "fmtp": { "channelOrder": "SMPTE2110.(ST)" } }
            }),
            expect: vec![
                ("clockRate", json!(96_000)),
                ("description", json!("everything at once")),
                ("mediaStreamType", json!("audio")),
                ("videoFormat", json!("720p50")),
                (
                    "attributes",
                    json!({
                        "lang": "fr",
                        "fmtp": {
                            "channelOrder": "SMPTE2110.(ST)",
                            "colorimetry": "BT2020",
                            "exactFramerate": "25",
                            "scanMode": "interlace"
                        }
                    }),
                ),
            ],
        },
        UpdateCase {
            case: "a request that carries no setting at all",
            body: json!({}),
            expect: Vec::new(),
        },
    ]
}

/// UpdateFlowMediaStream writes the settings the request carries onto the stored media
/// stream, leaves the ones it omits alone, and answers with the stored record.
#[tokio::test]
async fn update_flow_media_stream_applies_only_the_settings_the_request_carries() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(
        &emulator,
        "ms-update",
        json!([
            { "mediaStreamId": 1, "mediaStreamName": "vid-1", "mediaStreamType": "video" },
            { "mediaStreamId": 2, "mediaStreamName": "aud-1", "mediaStreamType": "audio", "clockRate": 48_000 }
        ]),
    )
    .await;
    let held = described_media_streams(&emulator, &flow_arn).await;
    let untouched_sibling = held[1].clone();
    let identity = (
        held[0]["mediaStreamName"].clone(),
        held[0]["fmt"].clone(),
        held[0]["mediaStreamId"].clone(),
    );

    for UpdateCase { case, body, expect } in update_cases() {
        let before = described_stream(&emulator, case, &flow_arn, "vid-1").await;
        let (status, code, response) =
            update_media_stream(&emulator, &flow_arn, "vid-1", body).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
        );
        assert_eq!(
            response["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the media stream belongs to, got \
             {response}"
        );
        let updated = response["mediaStream"].clone();

        for (path, expected) in &expect {
            assert_eq!(
                at_path(&updated, path),
                expected,
                "case {case:?}: {path} must be {expected}, got {updated}"
            );
        }

        // Everything the request did not touch is exactly as it was — including the
        // stream's identity, which no input member names.
        let touched: Vec<&str> = expect
            .iter()
            .map(|(path, _)| path.split('.').next().unwrap_or(path))
            .collect();
        assert_eq!(
            without_keys(&updated, &touched),
            without_keys(&before, &touched),
            "case {case:?}: the settings the request omitted are unchanged, got {updated}"
        );
        assert_eq!(
            (
                updated["mediaStreamName"].clone(),
                updated["fmt"].clone(),
                updated["mediaStreamId"].clone()
            ),
            identity,
            "case {case:?}: an update never moves the media stream's identity, got {updated}"
        );

        // The round trip: the 202 body is the stored record, and no other media stream of
        // the flow moved.
        let after = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            after[0], updated,
            "case {case:?}: DescribeFlow reports what the update answered, got {:?}",
            after[0]
        );
        assert_eq!(
            after[1], untouched_sibling,
            "case {case:?}: the flow's other media stream is untouched, got {:?}",
            after[1]
        );
        assert_eq!(
            after.len(),
            2,
            "case {case:?}: an update adds and removes no media stream, got {} streams",
            after.len()
        );
    }
}

/// The clock rate a media stream reports when no request has ever named one follows its
/// TYPE, which is the rule the add path applies too: `AddMediaStreamRequest$ClockRate`
/// documents 90000 for video and ancillary data, and audio has no single documented rate.
/// So a stream whose type this operation changes reports the rate documented for the type
/// it now has, while a stream that already carries a rate keeps it until a request names
/// another.
#[tokio::test]
async fn update_flow_media_stream_resolves_the_clock_rate_from_the_type_it_now_has() {
    let emulator = Emulator::new();

    // Each row: the type the stream is created with, the rate its creation named, the type
    // the update moves it to, and the rate the stream must then report.
    let rows: Vec<(&str, &str, Option<i32>, &str, Value)> = vec![
        (
            "an audio stream with no rate becomes video",
            mc::MEDIA_STREAM_TYPE_AUDIO,
            None,
            mc::MEDIA_STREAM_TYPE_VIDEO,
            json!(mc::VIDEO_CLOCK_RATE_HZ),
        ),
        (
            "an audio stream with no rate becomes ancillary-data",
            mc::MEDIA_STREAM_TYPE_AUDIO,
            None,
            mc::MEDIA_STREAM_TYPE_ANCILLARY_DATA,
            json!(mc::VIDEO_CLOCK_RATE_HZ),
        ),
        (
            "an audio stream with no rate stays audio and reports none",
            mc::MEDIA_STREAM_TYPE_AUDIO,
            None,
            mc::MEDIA_STREAM_TYPE_AUDIO,
            Value::Null,
        ),
        (
            "an audio stream at 48 kHz becomes video and keeps its own rate",
            mc::MEDIA_STREAM_TYPE_AUDIO,
            Some(48_000),
            mc::MEDIA_STREAM_TYPE_VIDEO,
            json!(48_000),
        ),
        (
            "a video stream becomes audio and keeps the rate it reported",
            mc::MEDIA_STREAM_TYPE_VIDEO,
            None,
            mc::MEDIA_STREAM_TYPE_AUDIO,
            json!(mc::VIDEO_CLOCK_RATE_HZ),
        ),
        (
            "an ancillary-data stream becomes video and reports the same rate",
            mc::MEDIA_STREAM_TYPE_ANCILLARY_DATA,
            None,
            mc::MEDIA_STREAM_TYPE_VIDEO,
            json!(mc::VIDEO_CLOCK_RATE_HZ),
        ),
    ];

    for (row, (case, created_type, created_rate, updated_type, expected_rate)) in
        rows.into_iter().enumerate()
    {
        let mut requested = json!({
            "mediaStreamId": 1,
            "mediaStreamName": "stream",
            "mediaStreamType": created_type
        });
        if let Some(rate) = created_rate {
            requested["clockRate"] = json!(rate);
        }
        let flow_arn = create_flow(
            &emulator,
            &format!("ms-clock-rate-{row}"),
            json!([requested]),
        )
        .await;

        let (status, code, response) = update_media_stream(
            &emulator,
            &flow_arn,
            "stream",
            json!({ "mediaStreamType": updated_type }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
        );
        assert_eq!(
            response["mediaStream"]["clockRate"],
            expected_rate,
            "case {case:?}: expected clockRate {expected_rate}, got {}",
            response["mediaStream"]
        );
        // And the store agrees with the answer.
        let stored = described_stream(&emulator, case, &flow_arn, "stream").await;
        assert_eq!(
            stored, response["mediaStream"],
            "case {case:?}: DescribeFlow reports what the update answered, got {stored}"
        );
    }
}

/// One row of the refusal table: an UpdateFlowMediaStream request the service must refuse,
/// what it addresses, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    media_stream_name: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A request UpdateFlowMediaStream will not satisfy is refused with the operation's own
/// modelled error and writes nothing: the flow's media streams still carry exactly the
/// settings they carried.
#[tokio::test]
async fn update_flow_media_stream_refuses_what_it_cannot_apply() {
    let emulator = Emulator::new();
    let flow_arn = create_flow(
        &emulator,
        "ms-update-refusals",
        json!([
            { "mediaStreamId": 1, "mediaStreamName": "vid-1", "mediaStreamType": "video" },
            { "mediaStreamId": 2, "mediaStreamName": "aud-1", "mediaStreamType": "audio", "clockRate": 48_000 }
        ]),
    )
    .await;
    // A media stream of ANOTHER flow, addressed through this one: a media stream name is
    // addressed within its flow, so it resolves nothing here.
    let other_flow = create_flow(
        &emulator,
        "ms-update-other",
        json!([
            { "mediaStreamId": 1, "mediaStreamName": "elsewhere", "mediaStreamType": "video" }
        ]),
    )
    .await;
    let before = described_media_streams(&emulator, &flow_arn).await;

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            media_stream_name: "vid-1".to_string(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow_arn()),
        },
        RefusalCase {
            case: "a media stream name this flow does not hold",
            flow_arn: flow_arn.clone(),
            media_stream_name: "no-such-stream".to_string(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("no-such-stream".to_string()),
        },
        RefusalCase {
            case: "a media stream of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            media_stream_name: "elsewhere".to_string(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("elsewhere".to_string()),
        },
        RefusalCase {
            case: "a media stream name that differs only in case",
            flow_arn: flow_arn.clone(),
            media_stream_name: "VID-1".to_string(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("VID-1".to_string()),
        },
        RefusalCase {
            // `MediaStreamType` is one of video | audio | ancillary-data, so a value
            // outside the modelled set is refused before the handler runs.
            case: "a media stream type outside the modelled value set",
            flow_arn: flow_arn.clone(),
            media_stream_name: "vid-1".to_string(),
            body: json!({ "mediaStreamType": "subtitles" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `Fmtp$Colorimetry` targets the `Colorimetry` enum
            // (`smithy_query(query="shape",shape="Colorimetry")`), so a nested member of
            // the attributes is validated too.
            case: "a colorimetry outside the modelled value set",
            flow_arn: flow_arn.clone(),
            media_stream_name: "vid-1".to_string(),
            body: json!({ "attributes": { "fmtp": { "colorimetry": "BT9999" } } }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        media_stream_name,
        body,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, response) =
            update_media_stream(&emulator, &target_flow, &media_stream_name, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {response}"
        );
        match &not_found_identifier {
            Some(identifier) => assert_eq!(
                response["message"].as_str(),
                Some(mc::not_found_message(identifier).as_str()),
                "case {case:?}: the envelope names the identifier that resolved nothing, got \
                 {response}"
            ),
            None => assert!(
                response["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: a refused request keeps this service's validation envelope, got \
                 {response}"
            ),
        }

        let after = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, got {after:?}"
        );
    }

    // The other flow's media stream is untouched by every refusal addressed at this one.
    let elsewhere = described_stream(&emulator, "the other flow", &other_flow, "elsewhere").await;
    assert_eq!(
        elsewhere["description"],
        Value::Null,
        "a refusal addressed at one flow writes nothing to another, got {elsewhere}"
    );
}

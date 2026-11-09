// SPDX-License-Identifier: CC0-1.0

//! RemoveFlowMediaStream, end to end over HTTP: one call drops exactly the media stream
//! whose name it addresses from the flow whose ARN it addresses.
//!
//! The rule these cases assert, over a table rather than over one request: a remove
//! answers 200 with the stored flow's ARN and the removed media stream's name, the flow
//! then holds the media streams it held minus that one IN THE SAME ORDER and with nothing
//! else about it moved, the stream's name, id and payload type are free again, a request
//! naming a flow the store does not hold or a media stream that flow does not hold is
//! refused `NotFoundException` (404), and — the operation's documented precondition — a
//! media stream any of the flow's sources or outputs is configured to carry is refused
//! `BadRequestException` (400). Every refusal removes nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_media_streams::{add_media_streams, create_flow, described_media_streams};
use crate::flow_outputs::{
    attached_vpc_interfaces, media_stream_output_configuration, unknown_flow_arn,
};

/// `DELETE /v1/flows/{FlowArn}/mediaStreams/{MediaStreamName}` — the
/// RemoveFlowMediaStream binding
/// (`smithy_query(query="operation",operation="RemoveFlowMediaStream")`).
async fn remove_media_stream(
    emulator: &Emulator,
    flow_arn: &str,
    media_stream_name: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/mediaStreams/{media_stream_name}"),
            None,
        )
        .await
}

/// `count` media stream requests named `ms-<n>` from `first`, each with its own id, so a
/// row can build a flow with a known number of distinguishable media streams.
fn numbered_streams(first: i32, count: i32) -> Value {
    (first..first + count)
        .map(|index| {
            json!({
                "mediaStreamId": index,
                "mediaStreamName": format!("ms-{index}"),
                "mediaStreamType": "video"
            })
        })
        .collect()
}

/// The flow's status, as DescribeFlow reports it.
async fn described_status(emulator: &Emulator, flow_arn: &str) -> String {
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    described["flow"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The flow as DescribeFlow reports it, with its media streams removed — what a removal
/// must leave untouched.
async fn described_flow_without_streams(emulator: &Emulator, flow_arn: &str) -> Value {
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    let mut flow = described["flow"].clone();
    if let Some(object) = flow.as_object_mut() {
        object.remove("mediaStreams");
    }
    flow
}

/// Remove the media stream at `position` of the flow's CURRENT list and assert the whole
/// rule for that one call: the 200 answer names the flow and that media stream, the flow
/// then holds the other media streams unchanged and in order, and nothing else about the
/// flow moved.
async fn remove_at(emulator: &Emulator, case: &str, flow_arn: &str, position: usize) {
    let before = described_media_streams(emulator, flow_arn).await;
    let rest_before = described_flow_without_streams(emulator, flow_arn).await;
    let target = before.get(position).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the flow must hold a media stream at {position}")
    });
    let name = target["mediaStreamName"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, body) = remove_media_stream(emulator, flow_arn, &name).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: removing media stream {position} ({name:?}) expected 200 with no \
         error, got {status} {code:?}: {body}"
    );
    assert_eq!(
        (body["flowArn"].as_str(), body["mediaStreamName"].as_str()),
        (Some(flow_arn), Some(name.as_str())),
        "case {case:?}: the response names the flow and the removed media stream, got {body}"
    );

    let after = described_media_streams(emulator, flow_arn).await;
    let expected: Vec<Value> = before
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, stream)| stream.clone())
        .collect();
    assert_eq!(
        after, expected,
        "case {case:?}: the flow holds the other media streams, in order, expected \
         {expected:?}, got {after:?}"
    );
    let rest_after = described_flow_without_streams(emulator, flow_arn).await;
    assert_eq!(
        rest_after, rest_before,
        "case {case:?}: removing a media stream moves nothing else about the flow, got \
         {rest_after}"
    );
}

/// One row of the RemoveFlowMediaStream table: the media streams the flow is created with,
/// the ones AddFlowMediaStreams then appends, and the positions removed by successive calls
/// (each one into the list as it stands when that call is made).
struct RemoveCase {
    case: &'static str,
    created: Value,
    added: Value,
    remove: &'static [usize],
}

/// The rows differ along what the rule generalises over: how many media streams the flow
/// holds, where in its list the removed one sits (only, first, middle, last), how many
/// calls remove one, whether the removed stream was stored by CreateFlow or by
/// AddFlowMediaStreams, which of the three modelled `MediaStreamType`s it has, how its
/// name is spelled, and how many optional members it carries.
fn remove_cases() -> Vec<RemoveCase> {
    vec![
        RemoveCase {
            case: "the only media stream of a flow",
            created: numbered_streams(1, 1),
            added: json!([]),
            remove: &[0],
        },
        RemoveCase {
            case: "the first of three",
            created: numbered_streams(1, 3),
            added: json!([]),
            remove: &[0],
        },
        RemoveCase {
            case: "the middle of three",
            created: numbered_streams(1, 3),
            added: json!([]),
            remove: &[1],
        },
        RemoveCase {
            case: "the last of three",
            created: numbered_streams(1, 3),
            added: json!([]),
            remove: &[2],
        },
        RemoveCase {
            case: "a media stream AddFlowMediaStreams appended, not CreateFlow",
            created: numbered_streams(1, 1),
            added: numbered_streams(2, 2),
            remove: &[2],
        },
        RemoveCase {
            case: "two media streams of five, over two calls",
            created: numbered_streams(1, 2),
            added: numbered_streams(3, 3),
            remove: &[4, 0],
        },
        RemoveCase {
            case: "every media stream of a flow, one call at a time",
            created: numbered_streams(1, 2),
            added: numbered_streams(3, 1),
            remove: &[1, 1, 0],
        },
        RemoveCase {
            // The three values of `MediaStreamType`
            // (`smithy_query(query="shape",shape="MediaStreamType")`: video | audio |
            // ancillary-data), with names spelled differently and the optional members
            // present, so the removal is not keyed to one shape of stream. The audio
            // stream in the middle is the one removed.
            case: "an audio stream carrying attributes, between a video and an ancillary one",
            created: json!([
                {
                    "mediaStreamId": 11,
                    "mediaStreamName": "VIDEO-1",
                    "mediaStreamType": "video",
                    "videoFormat": "1080i50",
                    "description": "programme"
                },
                {
                    "mediaStreamId": 12,
                    "mediaStreamName": "aud_2",
                    "mediaStreamType": "audio",
                    "clockRate": 48_000,
                    "attributes": { "lang": "de", "fmtp": { "channelOrder": "SMPTE2110.(ST)" } },
                    "mediaStreamTags": { "team": "audio" }
                },
                {
                    "mediaStreamId": 13,
                    "mediaStreamName": "anc3",
                    "mediaStreamType": "ancillary-data"
                }
            ]),
            added: json!([]),
            remove: &[1],
        },
    ]
}

/// RemoveFlowMediaStream drops exactly the media stream its `MediaStreamName` names from
/// the flow its `FlowArn` names, and leaves every other media stream of that flow — and
/// everything else about the flow — exactly as it was.
#[tokio::test]
async fn remove_flow_media_stream_drops_only_the_stream_it_names() {
    let emulator = Emulator::new();

    for (index, row) in remove_cases().into_iter().enumerate() {
        let RemoveCase {
            case,
            created,
            added,
            remove,
        } = row;
        let flow_arn = create_flow(&emulator, &format!("flow-ms-remove-{index}"), created).await;
        if added.as_array().is_some_and(|streams| !streams.is_empty()) {
            let (status, code, body) = add_media_streams(&emulator, &flow_arn, added).await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?}: the fixture media streams must be added, got {status} \
                 {code:?}: {body}"
            );
        }

        for position in remove {
            remove_at(&emulator, case, &flow_arn, *position).await;
        }
    }
}

/// One row of the refusal table: a RemoveFlowMediaStream request the service must refuse,
/// what it addresses, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    media_stream_name: String,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A RemoveFlowMediaStream request that resolves no media stream removes nothing: it is
/// refused with the operation's own modelled error, naming what resolved nothing, and
/// every flow still holds exactly the media streams it held.
#[tokio::test]
async fn remove_flow_media_stream_refuses_what_it_cannot_remove() {
    let emulator = Emulator::new();

    let flow_arn = create_flow(&emulator, "flow-ms-refusals", numbered_streams(1, 3)).await;
    let other_flow = create_flow(&emulator, "flow-ms-other", numbered_streams(7, 1)).await;
    let other_before = described_media_streams(&emulator, &other_flow).await;

    // One media stream removed for real, so a second call on its name addresses a stream
    // that no longer exists.
    let gone = "ms-3".to_string();
    let (status, _, body) = remove_media_stream(&emulator, &flow_arn, &gone).await;
    assert_eq!(
        status, 200,
        "the fixture removal must succeed, got {status}: {body}"
    );
    let before = described_media_streams(&emulator, &flow_arn).await;

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted, with a media stream name it holds",
            flow_arn: unknown_flow_arn(),
            media_stream_name: "ms-1".to_string(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow_arn()),
        },
        RefusalCase {
            case: "a media stream name no flow holds",
            flow_arn: flow_arn.clone(),
            media_stream_name: "never-added".to_string(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("never-added".to_string()),
        },
        RefusalCase {
            case: "a media stream of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            media_stream_name: "ms-7".to_string(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("ms-7".to_string()),
        },
        RefusalCase {
            case: "a media stream already removed",
            flow_arn: flow_arn.clone(),
            media_stream_name: gone.clone(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(gone),
        },
        RefusalCase {
            case: "a name that differs from a held one only in case",
            flow_arn: flow_arn.clone(),
            media_stream_name: "MS-1".to_string(),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some("MS-1".to_string()),
        },
        RefusalCase {
            // A label that is not an ARN at all names no stored flow either, so it
            // resolves to no flow and is answered as a not-found — the same answer
            // DescribeFlow and DeleteFlow give such a label (see
            // `tests/all/flow_lifecycle.rs`).
            case: "a flow label that is no flow ARN at all",
            flow_arn: "not-a-flow-arn".to_string(),
            media_stream_name: "ms-1".to_string(),
            status: 404,
            code: "NotFoundException",
            // The label is no flow ARN, so it reached no flow lookup: the envelope carries
            // the generic reason rather than the caller's own text.
            not_found_identifier: Some(mc::UNRESOLVED_PATH_NOT_FOUND_REASON.to_string()),
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        media_stream_name,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, body) =
            remove_media_stream(&emulator, &target_flow, &media_stream_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        match &not_found_identifier {
            Some(identifier) => assert_eq!(
                body["message"].as_str(),
                Some(mc::not_found_message(identifier).as_str()),
                "case {case:?}: the envelope names the identifier that resolved nothing, got \
                 {body}"
            ),
            None => assert!(
                body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: a refused request keeps this service's validation envelope, \
                 got {body}"
            ),
        }

        let after = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused remove stores nothing, got {after:?}"
        );
        let other_after = described_media_streams(&emulator, &other_flow).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: a refused remove leaves every other flow alone, got {other_after:?}"
        );
    }
}

/// One `MediaStreamSourceConfigurations` entry for the media stream `media_stream_name`,
/// listening on `input_port`, arriving on a VPC interface the flow holds
/// ([`attached_vpc_interfaces`]).
fn media_stream_source_configuration(media_stream_name: &str, input_port: i32) -> Value {
    json!({
        "encodingName": "jxsv",
        "mediaStreamName": media_stream_name,
        "inputConfigurations": [
            { "inputPort": input_port, "interface": { "name": "vpc-1" } }
        ]
    })
}

/// One row of the association table: which of the flow's resources is configured to carry
/// the media stream `associated`, and the body CreateFlow is called with.
struct AssociationCase {
    case: &'static str,
    flow: &'static str,
    /// The media stream the flow's source and/or output is configured to carry.
    associated: &'static str,
    /// A media stream of the same flow that nothing carries, so each row also proves the
    /// refusal is about the ASSOCIATION and not about the flow.
    free: &'static str,
    body: Value,
}

/// The rows span the two kinds of association the documentation names — "a source or
/// output" — alone and together, with the carried stream sitting at different positions of
/// the flow's media stream list.
fn association_cases() -> Vec<AssociationCase> {
    let streams = json!([
        { "mediaStreamId": 1, "mediaStreamName": "carried-1", "mediaStreamType": "video" },
        { "mediaStreamId": 2, "mediaStreamName": "carried-2", "mediaStreamType": "audio" },
        { "mediaStreamId": 3, "mediaStreamName": "loose", "mediaStreamType": "ancillary-data" }
    ]);
    vec![
        AssociationCase {
            case: "a media stream the flow's source is configured to carry",
            flow: "flow-ms-assoc-source",
            associated: "carried-1",
            free: "loose",
            body: json!({
                "name": "flow-ms-assoc-source",
                "mediaStreams": streams,
                "vpcInterfaces": attached_vpc_interfaces(),
                "source": {
                    "name": "src",
                    "protocol": "st2110-jpegxs",
                    "whitelistCidr": "10.0.0.0/16",
                    "mediaStreamSourceConfigurations": [
                        media_stream_source_configuration("carried-1", 6000)
                    ]
                }
            }),
        },
        AssociationCase {
            case: "a media stream one of the flow's outputs is configured to carry",
            flow: "flow-ms-assoc-output",
            associated: "carried-2",
            free: "loose",
            body: json!({
                "name": "flow-ms-assoc-output",
                "mediaStreams": streams,
                "vpcInterfaces": attached_vpc_interfaces(),
                "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
                "outputs": [
                    { "name": "plain", "protocol": "udp", "destination": "203.0.113.9", "port": 7000 },
                    {
                        "name": "carrier",
                        "protocol": "st2110-jpegxs",
                        "port": 7100,
                        "mediaStreamOutputConfigurations": [
                            media_stream_output_configuration("carried-2", "198.51.100.60")
                        ]
                    }
                ]
            }),
        },
        AssociationCase {
            case: "a media stream both a source and an output are configured to carry",
            flow: "flow-ms-assoc-both",
            associated: "carried-1",
            free: "loose",
            body: json!({
                "name": "flow-ms-assoc-both",
                "mediaStreams": streams,
                "vpcInterfaces": attached_vpc_interfaces(),
                "source": {
                    "name": "src",
                    "protocol": "st2110-jpegxs",
                    "whitelistCidr": "10.0.0.0/16",
                    "mediaStreamSourceConfigurations": [
                        media_stream_source_configuration("carried-1", 6002)
                    ]
                },
                "outputs": [{
                    "name": "carrier",
                    "protocol": "st2110-jpegxs",
                    "port": 7102,
                    "mediaStreamOutputConfigurations": [
                        media_stream_output_configuration("carried-1", "198.51.100.61")
                    ]
                }]
            }),
        },
    ]
}

/// "This action is only available if the media stream is not associated with a source or
/// output": a media stream any of the flow's sources or outputs is configured to carry is
/// refused `BadRequestException` and stays on the flow, while a media stream of the same
/// flow that nothing carries is removed.
#[tokio::test]
async fn remove_flow_media_stream_refuses_a_stream_a_source_or_output_carries() {
    let emulator = Emulator::new();

    for AssociationCase {
        case,
        flow,
        associated,
        free,
        body,
    } in association_cases()
    {
        let (status, code, created) = emulator.create_flow(body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: CreateFlow {flow:?} expected 201, got {status} {code:?}: {created}"
        );
        let flow_arn = created["flow"]["flowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let before = described_media_streams(&emulator, &flow_arn).await;

        let (status, code, body) = remove_media_stream(&emulator, &flow_arn, associated).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: removing the carried media stream {associated:?} expected 400 \
             BadRequestException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal keeps this service's validation envelope, got {body}"
        );
        let after = described_media_streams(&emulator, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused remove leaves every media stream in place, got \
             {after:?}"
        );

        // The same flow, the same call, a media stream nothing carries: removed.
        let (status, code, body) = remove_media_stream(&emulator, &flow_arn, free).await;
        assert_eq!(
            (status, code.as_deref(), body["mediaStreamName"].as_str()),
            (200, None, Some(free)),
            "case {case:?}: the unassociated media stream {free:?} is removed, got {status} \
             {code:?}: {body}"
        );
        let names: Vec<String> = described_media_streams(&emulator, &flow_arn)
            .await
            .iter()
            .map(|stream| {
                stream["mediaStreamName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert!(
            names.iter().any(|name| name == associated)
                && !names.iter().any(|name| name == free),
            "case {case:?}: the carried stream stays and the free one is gone, got {names:?}"
        );
    }
}

/// An association is a fact about the STORED flow: a source created with
/// `MediaStreamSourceConfigurations` reports them back through DescribeFlow — with the one
/// member no request carries, `InputConfiguration$InputIp`, assigned by the service — and
/// the precondition is lifted exactly when the resource carrying the stream is gone.
#[tokio::test]
async fn an_association_is_stored_and_lifting_it_frees_the_media_stream() {
    let emulator = Emulator::new();
    let case = "a stored source and output association";

    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "flow-ms-assoc-roundtrip",
            // The case adds a spare source further down (a flow keeps at least one
            // source), so this flow carries source failover ACTIVE: "if the state is
            // active, the flow can have one or two sources" (`FailoverConfig$State`).
            "sourceFailoverConfig": { "state": "ENABLED" },
            "mediaStreams": [
                { "mediaStreamId": 1, "mediaStreamName": "carried", "mediaStreamType": "video" }
            ],
            "vpcInterfaces": attached_vpc_interfaces(),
            "source": {
                "name": "src",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "10.0.0.0/16",
                "mediaStreamSourceConfigurations": [
                    media_stream_source_configuration("carried", 6004)
                ]
            },
            "outputs": [{
                "name": "carrier",
                "protocol": "st2110-jpegxs",
                "port": 7104,
                "mediaStreamOutputConfigurations": [
                    media_stream_output_configuration("carried", "198.51.100.62")
                ]
            }]
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateFlow expected 201, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let output_arn = created["flow"]["outputs"][0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The association round-trips through the store: DescribeFlow reports it, including
    // the `InputIp` the SERVICE assigns ("the IP address that the flow listens on for
    // incoming content for a media stream"), drawn from the same documentation-range pool
    // the source's ingest address is.
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow expected 200, got {status} {code:?}: {described}"
    );
    let configuration = &described["flow"]["source"]["mediaStreamSourceConfigurations"][0];
    assert_eq!(
        (
            configuration["mediaStreamName"].as_str(),
            configuration["encodingName"].as_str(),
            configuration["inputConfigurations"][0]["inputPort"].as_i64(),
            configuration["inputConfigurations"][0]["interface"]["name"].as_str(),
        ),
        (Some("carried"), Some("jxsv"), Some(6004), Some("vpc-1")),
        "case {case:?}: the source reports the stored association, got {configuration}"
    );
    let input_ip = configuration["inputConfigurations"][0]["inputIp"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        (1..=254).any(|host| mc::assigned_ingest_ip(host) == input_ip),
        "case {case:?}: inputIp is an address the service assigns from its ingest pool, got \
         {configuration}"
    );
    assert_eq!(
        described["flow"]["source"]["ingestIp"].as_str(),
        Some(input_ip.as_str()),
        "case {case:?}: the stream listens on the address the source ingests on, got {}",
        described["flow"]["source"]
    );

    // While the output carries it, the stream cannot be removed — even though the source
    // association is the one listed first.
    let (status, code, body) = remove_media_stream(&emulator, &flow_arn, "carried").await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: the carried stream is refused, got {status} {code:?}: {body}"
    );

    // Drop the output: the stream is still carried by the SOURCE, so the precondition
    // still refuses it.
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: RemoveFlowOutput expected 202, got {status} {code:?}: {body}"
    );
    let (status, code, body) = remove_media_stream(&emulator, &flow_arn, "carried").await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: the source still carries the stream, got {status} {code:?}: {body}"
    );

    // Lift the source association the only way this emulator can today: by removing the
    // source that carries it. A flow keeps at least one source, so a spare is added first
    // (RemoveFlowSource refuses a flow's last source).
    let (status, code, body) = emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/source"),
            Some(json!({
                "sources": [
                    { "name": "spare", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
                ]
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: AddFlowSources expected 201, got {status} {code:?}: {body}"
    );
    let source_arn = described["flow"]["source"]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}/source/{source_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: RemoveFlowSource expected 202, got {status} {code:?}: {body}"
    );

    // Nothing carries the media stream now, so the removal is available.
    let (status, code, body) = remove_media_stream(&emulator, &flow_arn, "carried").await;
    assert_eq!(
        (status, code.as_deref(), body["mediaStreamName"].as_str()),
        (200, None, Some("carried")),
        "case {case:?}: once no source and no output carries it, the stream is removed, got \
         {status} {code:?}: {body}"
    );
    assert!(
        described_media_streams(&emulator, &flow_arn)
            .await
            .is_empty(),
        "case {case:?}: the flow holds no media stream any more"
    );
}

/// A media stream name, its id and its service-assigned `Fmt` are unique within a flow
/// only while the stream is there: once it is removed all three are free again, and the
/// stream added in its place is a NEW record.
#[tokio::test]
async fn a_removed_media_stream_frees_its_name_id_and_payload_type() {
    let emulator = Emulator::new();
    let case = "a removed media stream's identity is free again";
    let flow_arn = create_flow(&emulator, "flow-ms-recycle", numbered_streams(1, 3)).await;

    let held = described_media_streams(&emulator, &flow_arn).await;
    let first = held[0].clone();
    let fmt = first["fmt"].as_i64().unwrap_or_default();
    assert_eq!(
        fmt,
        i64::from(mc::DYNAMIC_RTP_PAYLOAD_TYPE_BASE),
        "case {case:?}: the first media stream carries the first dynamic payload type, got \
         {first}"
    );

    // The precondition: while the stream is there, its name and its id are taken.
    let clash = json!([
        { "mediaStreamId": 1, "mediaStreamName": "ms-1", "mediaStreamType": "audio" }
    ]);
    let (status, code, body) = add_media_streams(&emulator, &flow_arn, clash.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a name the flow still uses is refused, got {status} {code:?}: {body}"
    );

    let (status, code, body) = remove_media_stream(&emulator, &flow_arn, "ms-1").await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the remove expected 200, got {status} {code:?}: {body}"
    );

    // The freed name and id are accepted again, and the stream that takes them is assigned
    // the lowest payload type no stream of the flow holds — the one the removal freed.
    let (status, code, body) = add_media_streams(&emulator, &flow_arn, clash).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the freed name and id are accepted again, got {status} {code:?}: {body}"
    );
    let re_added = body["mediaStreams"][0].clone();
    assert_eq!(
        (
            re_added["fmt"].as_i64(),
            re_added["mediaStreamType"].as_str(),
            re_added["clockRate"].as_i64()
        ),
        (Some(fmt), Some(mc::MEDIA_STREAM_TYPE_AUDIO), None),
        "case {case:?}: the stream added in its place is a new record on the freed payload \
         type, got {re_added}"
    );
}

/// A flow in a transient status still serves a remove: the removal materialises only the
/// status change the CLOCK already owed, never one of its own, so the caller sees the
/// transient status before the span is up and the settled one after it — with the media
/// stream gone either way.
#[tokio::test]
async fn removing_a_media_stream_leaves_the_clock_to_move_the_status() {
    let emulator = Emulator::new();
    let case = "a remove while the flow is starting";
    let flow_arn = create_flow(&emulator, "flow-ms-transient", numbered_streams(1, 2)).await;

    let (status, code, body) = emulator
        .send("POST", &format!("/v1/flows/start/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref(), body["status"].as_str()),
        (202, None, Some(mc::STATUS_STARTING)),
        "case {case:?}: StartFlow expected 202 STARTING, got {status} {code:?}: {body}"
    );

    // Inside the transition span: the remove is served and the flow is still STARTING.
    remove_at(&emulator, case, &flow_arn, 0).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_STARTING,
        "case {case:?}: the flow is still in its transient status"
    );

    // Past the span: the clock settles the status, and the media stream is still gone.
    tokio::time::sleep(mc::TRANSITION_SPAN + Duration::from_millis(400)).await;
    assert_eq!(
        described_status(&emulator, &flow_arn).await,
        mc::STATUS_ACTIVE,
        "case {case:?}: the clock settles the status the start owed"
    );
    let names: Vec<String> = described_media_streams(&emulator, &flow_arn)
        .await
        .iter()
        .map(|stream| {
            stream["mediaStreamName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        vec!["ms-2".to_string()],
        "case {case:?}: the removed media stream stays removed, got {names:?}"
    );
}

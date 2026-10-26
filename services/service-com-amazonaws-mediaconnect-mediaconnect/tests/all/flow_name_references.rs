// SPDX-License-Identifier: CC0-1.0

//! A flow's by-name references to its own media streams and VPC interfaces, end to end
//! over HTTP.
//!
//! The rule under test: every request member that references a sub-resource of a flow BY
//! NAME must name one that flow holds once the request is applied — a media stream
//! association's `MediaStreamName`, the `Interface.Name` of a destination or input
//! configuration, `SetSourceRequest$VpcInterfaceName` and a
//! `VpcInterfaceAttachment$VpcInterfaceName` — and a request that names one the flow has
//! not got is refused `BadRequestException` (HTTP 400) naming the member and the name, with
//! nothing stored. The rule is asserted on every operation that stores one of those
//! members: CreateFlow (against the media streams and VPC interfaces the SAME request
//! creates), AddFlowOutputs, AddFlowSources, UpdateFlowOutput and UpdateFlowSource.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_media_streams::add_media_streams;
use crate::flow_outputs::{add_flow_outputs, described_outputs};
use crate::flow_sources::{add_flow_sources, described_sources, failover_active};
use crate::flow_vpc_interfaces::{add_vpc_interfaces, vpc_interface_request, SUBNET_ID};
use crate::update_flow_output::update_flow_output;
use crate::update_flow_source::update_flow_source;

/// The media streams the flow under test holds: `(name, id, type)` as
/// `AddMediaStreamRequest` takes them. A media stream is addressed by NAME within its flow,
/// so these names are what an association of this flow can reference.
const HELD_MEDIA_STREAMS: [(&str, i32, &str); 2] = [("vid", 1, "video"), ("aud", 2, "audio")];

/// The VPC interfaces the flow under test holds. A VPC interface is addressed by NAME
/// within its flow too, so these are what an attachment or an interface of this flow can
/// reference.
const HELD_VPC_INTERFACES: [&str; 2] = ["vpc-a", "vpc-b"];

/// Names no media stream and no VPC interface of the flow under test carries, so each is a
/// reference to nothing whichever member it travels in: a plain absent name, the EMPTY name
/// (which addresses nothing at all), a held name in another case (names are matched
/// exactly, not case-insensitively) and a held name with a suffix.
const UNKNOWN_NAMES: [&str; 4] = ["does-not-exist", "", "VID", "vpc-a-2"];

/// A security group the fixture's VPC interface requests name.
const SECURITY_GROUP_IDS: [&str; 1] = ["sg-0123456789abcdef0"];

/// A bridge ARN, well formed against the modelled `SetGatewayBridgeSourceRequest$BridgeArn`
/// pattern (`^arn:.+:mediaconnect.+:bridge:.+$`), for the row whose source is fed by a
/// bridge: the member under test there is the VPC interface the bridge source attaches.
const BRIDGE_ARN: &str = "arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000beef:bridge-1";

/// The flow's `MediaStreams`, as [`HELD_MEDIA_STREAMS`] names them.
fn held_media_streams() -> Value {
    HELD_MEDIA_STREAMS
        .iter()
        .map(|(name, id, kind)| {
            json!({ "mediaStreamId": id, "mediaStreamName": name, "mediaStreamType": kind })
        })
        .collect()
}

/// The flow's `VpcInterfaces`, as [`HELD_VPC_INTERFACES`] names them.
fn held_vpc_interfaces() -> Value {
    HELD_VPC_INTERFACES
        .iter()
        .map(|name| vpc_interface_request(name, None, &SECURITY_GROUP_IDS, SUBNET_ID, None))
        .collect()
}

/// One `MediaStreamOutputConfigurations` entry: the media stream `media_stream` leaving on
/// the VPC interface `vpc_interface` towards `destination_ip`.
fn output_association(media_stream: &str, vpc_interface: &str, destination_ip: &str) -> Value {
    json!({
        "mediaStreamName": media_stream,
        "encodingName": "jxsv",
        "destinationConfigurations": [{
            "destinationIp": destination_ip,
            "destinationPort": 6000,
            "interface": { "name": vpc_interface }
        }]
    })
}

/// One `MediaStreamSourceConfigurations` entry: the media stream `media_stream` arriving on
/// the VPC interface `vpc_interface` at `input_port`.
fn source_association(media_stream: &str, vpc_interface: &str, input_port: i32) -> Value {
    json!({
        "mediaStreamName": media_stream,
        "encodingName": "jxsv",
        "inputConfigurations": [{ "inputPort": input_port, "interface": { "name": vpc_interface } }]
    })
}

/// Create the flow the cases work on — holding [`HELD_MEDIA_STREAMS`],
/// [`HELD_VPC_INTERFACES`], one plain source and one plain output, with source failover
/// ACTIVE so one more source can be added ("if the state is active, the flow can have one
/// or two sources", `FailoverConfig$State`) — and return its ARN.
async fn fixture_flow(emulator: &Emulator, name: &str) -> String {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "sourceFailoverConfig": failover_active(),
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "outputs": [
                { "name": "plain", "protocol": "udp", "destination": "203.0.113.5", "port": 5000 }
            ],
            "mediaStreams": held_media_streams(),
            "vpcInterfaces": held_vpc_interfaces(),
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "setup: CreateFlow {name:?} expected 201 with no error, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert `body` is the service's refusal for the reference `member` carried the name
/// `name`: `BadRequestException` at HTTP 400, in this service's validation envelope, naming
/// both the member and the name a client sent.
fn assert_unresolved_reference(
    case: &str,
    member: &str,
    name: &str,
    status: u16,
    code: Option<&str>,
    body: &Value,
) {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: an unresolved {member} expected 400 BadRequestException, got {status} \
         {code:?}: {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.starts_with(mc::VALIDATION_FAILED_PREFIX),
        "case {case:?}: the refusal keeps this service's validation envelope, got {body}"
    );
    assert!(
        message.contains(member),
        "case {case:?}: the refusal names the member that carried the reference, got {body}"
    );
    assert!(
        message.contains(&format!("{name:?}")),
        "case {case:?}: the refusal names the unresolved name {name:?}, got {body}"
    );
}

/// One row of the output table: an `AddOutputRequest` carrying exactly ONE reference built
/// from a variable name — the member named by `member` — with every other reference naming
/// something the flow holds.
struct OutputCase {
    case: &'static str,
    member: &'static str,
    output: Value,
}

/// The output rows, with `media_stream` / `vpc_interface` filling the member each row puts
/// under test. Driven once with names the flow holds (every row must be accepted) and once
/// per entry of [`UNKNOWN_NAMES`] (every row must be refused).
fn output_cases(media_stream: &str, vpc_interface: &str) -> Vec<OutputCase> {
    let held_stream = HELD_MEDIA_STREAMS[1].0;
    let held_interface = HELD_VPC_INTERFACES[1];
    vec![
        OutputCase {
            case: "the media stream an association names",
            member: "MediaStreamOutputConfigurations[0].MediaStreamName",
            output: json!({
                "name": "o-association",
                "protocol": "st2110-jpegxs",
                "port": 7100,
                "mediaStreamOutputConfigurations": [
                    output_association(media_stream, held_interface, "198.51.100.10")
                ]
            }),
        },
        OutputCase {
            case: "the media stream the second association names",
            member: "MediaStreamOutputConfigurations[1].MediaStreamName",
            output: json!({
                "name": "o-second-association",
                "protocol": "st2110-jpegxs",
                "port": 7102,
                "mediaStreamOutputConfigurations": [
                    output_association(held_stream, held_interface, "198.51.100.11"),
                    output_association(media_stream, held_interface, "198.51.100.12")
                ]
            }),
        },
        OutputCase {
            case: "the VPC interface a destination configuration sends out on",
            member: "MediaStreamOutputConfigurations[0].DestinationConfigurations[0].Interface.Name",
            output: json!({
                "name": "o-destination-interface",
                "protocol": "st2110-jpegxs",
                "port": 7104,
                "mediaStreamOutputConfigurations": [
                    output_association(held_stream, vpc_interface, "198.51.100.13")
                ]
            }),
        },
        OutputCase {
            case: "the VPC interface the second destination configuration sends out on",
            member: "MediaStreamOutputConfigurations[0].DestinationConfigurations[1].Interface.Name",
            output: json!({
                "name": "o-second-destination-interface",
                "protocol": "st2110-jpegxs",
                "port": 7106,
                "mediaStreamOutputConfigurations": [{
                    "mediaStreamName": held_stream,
                    "encodingName": "jxsv",
                    "destinationConfigurations": [
                        {
                            "destinationIp": "198.51.100.14",
                            "destinationPort": 6000,
                            "interface": { "name": held_interface }
                        },
                        {
                            "destinationIp": "198.51.100.15",
                            "destinationPort": 6002,
                            "interface": { "name": vpc_interface }
                        }
                    ]
                }]
            }),
        },
        OutputCase {
            case: "the VPC interface the output attaches to",
            member: "VpcInterfaceAttachment.VpcInterfaceName",
            output: json!({
                "name": "o-attachment",
                "protocol": "srt-listener",
                "port": 7108,
                "minLatency": 120,
                "vpcInterfaceAttachment": { "vpcInterfaceName": vpc_interface }
            }),
        },
    ]
}

/// One row of the source table: a `SetSourceRequest` carrying exactly ONE reference built
/// from a variable name, read the same way [`OutputCase`] is.
struct SourceCase {
    case: &'static str,
    member: &'static str,
    source: Value,
}

/// The source rows, driven exactly as [`output_cases`] is.
fn source_cases(media_stream: &str, vpc_interface: &str) -> Vec<SourceCase> {
    let held_stream = HELD_MEDIA_STREAMS[1].0;
    let held_interface = HELD_VPC_INTERFACES[1];
    vec![
        SourceCase {
            case: "the media stream an association names",
            member: "MediaStreamSourceConfigurations[0].MediaStreamName",
            source: json!({
                "name": "s-association",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "10.1.0.0/16",
                "mediaStreamSourceConfigurations": [
                    source_association(media_stream, held_interface, 9100)
                ]
            }),
        },
        SourceCase {
            case: "the media stream the second association names",
            member: "MediaStreamSourceConfigurations[1].MediaStreamName",
            source: json!({
                "name": "s-second-association",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "10.2.0.0/16",
                "mediaStreamSourceConfigurations": [
                    source_association(held_stream, held_interface, 9102),
                    source_association(media_stream, held_interface, 9104)
                ]
            }),
        },
        SourceCase {
            case: "the VPC interface an input configuration listens on",
            member: "MediaStreamSourceConfigurations[0].InputConfigurations[0].Interface.Name",
            source: json!({
                "name": "s-input-interface",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "10.3.0.0/16",
                "mediaStreamSourceConfigurations": [
                    source_association(held_stream, vpc_interface, 9106)
                ]
            }),
        },
        SourceCase {
            case: "the VPC interface the source itself uses",
            member: "VpcInterfaceName",
            source: json!({
                "name": "s-interface",
                "protocol": "rtp",
                "whitelistCidr": "10.4.0.0/16",
                "vpcInterfaceName": vpc_interface
            }),
        },
        SourceCase {
            case: "the VPC interface a gateway bridge source attaches",
            member: "GatewayBridgeSource.VpcInterfaceAttachment.VpcInterfaceName",
            source: json!({
                "name": "s-bridge",
                "gatewayBridgeSource": {
                    "bridgeArn": BRIDGE_ARN,
                    "vpcInterfaceAttachment": { "vpcInterfaceName": vpc_interface }
                }
            }),
        },
    ]
}

/// AddFlowOutputs resolves every by-name reference an output carries against the flow it
/// adds to: a reference the flow does not hold is refused and appends nothing, and the same
/// row naming what the flow DOES hold is stored and reported back.
#[tokio::test]
async fn add_flow_outputs_resolves_every_output_reference_against_the_flow() {
    let emulator = Emulator::new();
    let flow_arn = fixture_flow(&emulator, "flow-add-output-references").await;

    for unknown in UNKNOWN_NAMES {
        for OutputCase {
            case,
            member,
            output,
        } in output_cases(unknown, unknown)
        {
            let case = format!("{case} as {unknown:?}, added");
            let before = described_outputs(&emulator, &case, &flow_arn).await;
            let (status, code, body) =
                add_flow_outputs(&emulator, &flow_arn, json!([output])).await;
            assert_unresolved_reference(
                &case,
                &format!("Outputs[0].{member}"),
                unknown,
                status,
                code.as_deref(),
                &body,
            );
            let after = described_outputs(&emulator, &case, &flow_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused add appends no output, got {after:?}"
            );
        }
    }

    // The same rows, each reference naming what the flow holds: stored, and reported by
    // DescribeFlow exactly as the add answered.
    for OutputCase {
        case,
        member,
        output,
    } in output_cases(HELD_MEDIA_STREAMS[0].0, HELD_VPC_INTERFACES[0])
    {
        let case = format!("{case}, resolved");
        let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, json!([output])).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: a resolved {member} is accepted, got {status} {code:?}: {body}"
        );
        let added = body["outputs"][0].clone();
        let stored = described_outputs(&emulator, &case, &flow_arn).await;
        assert_eq!(
            stored.last().cloned().unwrap_or(Value::Null),
            added,
            "case {case:?}: DescribeFlow reports the output the add answered, got {stored:?}"
        );
    }
}

/// AddFlowSources resolves every by-name reference a source carries, the same way
/// AddFlowOutputs resolves an output's.
#[tokio::test]
async fn add_flow_sources_resolves_every_source_reference_against_the_flow() {
    let emulator = Emulator::new();
    let flow_arn = fixture_flow(&emulator, "flow-add-source-references").await;

    for unknown in UNKNOWN_NAMES {
        for SourceCase {
            case,
            member,
            source,
        } in source_cases(unknown, unknown)
        {
            let case = format!("{case} as {unknown:?}, added");
            let before = described_sources(&emulator, &case, &flow_arn).await;
            let (status, code, body) =
                add_flow_sources(&emulator, &flow_arn, json!([source])).await;
            assert_unresolved_reference(
                &case,
                &format!("Sources[0].{member}"),
                unknown,
                status,
                code.as_deref(),
                &body,
            );
            let after = described_sources(&emulator, &case, &flow_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused add appends no source, got {after:?}"
            );
        }
    }

    // The same rows, resolved. A flow holds at most two sources with failover ACTIVE, so
    // each row gets its own fixture flow and the add brings it to two.
    for (index, SourceCase { case, member, source }) in
        source_cases(HELD_MEDIA_STREAMS[0].0, HELD_VPC_INTERFACES[0])
            .into_iter()
            .enumerate()
    {
        let case = format!("{case}, resolved");
        let row_flow = fixture_flow(&emulator, &format!("flow-add-source-ok-{index}")).await;
        let (status, code, body) = add_flow_sources(&emulator, &row_flow, json!([source])).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: a resolved {member} is accepted, got {status} {code:?}: {body}"
        );
        let added = body["sources"][0].clone();
        let stored = described_sources(&emulator, &case, &row_flow).await;
        assert_eq!(
            stored.last().cloned().unwrap_or(Value::Null),
            added,
            "case {case:?}: DescribeFlow reports the source the add answered, got {stored:?}"
        );
    }
}

/// CreateFlow resolves a source's and an output's references against the media streams and
/// VPC interfaces the SAME request creates — a flow being created holds nothing else — so a
/// request that names one it does not create stores no flow at all.
#[tokio::test]
async fn create_flow_resolves_a_reference_against_what_the_same_request_creates() {
    let emulator = Emulator::new();

    // The whole request, minus the collections a row drops: the source and the output each
    // reference a media stream and a VPC interface this very request creates.
    let body = |media_streams: Value, vpc_interfaces: Value| {
        let mut request = json!({
            "name": "flow-create-references",
            "source": {
                "name": "src",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "10.0.0.0/16",
                "vpcInterfaceName": HELD_VPC_INTERFACES[0],
                "mediaStreamSourceConfigurations": [
                    source_association(HELD_MEDIA_STREAMS[0].0, HELD_VPC_INTERFACES[0], 9100)
                ]
            },
            "outputs": [{
                "name": "out",
                "protocol": "st2110-jpegxs",
                "port": 7100,
                "vpcInterfaceAttachment": { "vpcInterfaceName": HELD_VPC_INTERFACES[1] },
                "mediaStreamOutputConfigurations": [
                    output_association(HELD_MEDIA_STREAMS[1].0, HELD_VPC_INTERFACES[1], "198.51.100.20")
                ]
            }]
        });
        if !media_streams.is_null() {
            request["mediaStreams"] = media_streams;
        }
        if !vpc_interfaces.is_null() {
            request["vpcInterfaces"] = vpc_interfaces;
        }
        request
    };

    // Refused: the request references what it never creates. Each row drops exactly one of
    // the two collections, so each kind of reference is the one left unresolved.
    let refusals = [
        (
            "a media stream the request does not create",
            body(Value::Null, held_vpc_interfaces()),
            "Sources[0].MediaStreamSourceConfigurations[0].MediaStreamName",
            HELD_MEDIA_STREAMS[0].0,
        ),
        (
            "a VPC interface the request does not create",
            body(held_media_streams(), Value::Null),
            "Sources[0].MediaStreamSourceConfigurations[0].InputConfigurations[0].Interface.Name",
            HELD_VPC_INTERFACES[0],
        ),
    ];
    for (case, request, member, name) in refusals {
        let (status, code, answered) = emulator.create_flow(request).await;
        assert_unresolved_reference(case, member, name, status, code.as_deref(), &answered);
        let (status, code, listed) = emulator.list_flows("").await;
        assert_eq!(
            (status, code.as_deref(), listed["flows"].as_array().map(Vec::len)),
            (200, None, Some(0)),
            "case {case:?}: a refused create stores no flow, got {listed}"
        );
    }

    // Accepted: the request creates everything it references, in either order relative to
    // the members that reference it, and DescribeFlow reports the stored associations.
    let case = "every reference created by the same request";
    let (status, code, created) = emulator
        .create_flow(body(held_media_streams(), held_vpc_interfaces()))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: expected 201 with no error, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, described) = emulator.describe_flow(&flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    let flow = described["flow"].clone();
    assert_eq!(
        (
            flow["sources"][0]["mediaStreamSourceConfigurations"][0]["mediaStreamName"].as_str(),
            flow["sources"][0]["vpcInterfaceName"].as_str(),
            flow["outputs"][0]["mediaStreamOutputConfigurations"][0]["mediaStreamName"].as_str(),
            flow["outputs"][0]["vpcInterfaceAttachment"]["vpcInterfaceName"].as_str(),
        ),
        (
            Some(HELD_MEDIA_STREAMS[0].0),
            Some(HELD_VPC_INTERFACES[0]),
            Some(HELD_MEDIA_STREAMS[1].0),
            Some(HELD_VPC_INTERFACES[1]),
        ),
        "case {case:?}: the stored flow carries the references the request created, got {flow}"
    );
}

/// UpdateFlowOutput and UpdateFlowSource resolve the references they MOVE against the flow
/// too: an update naming a media stream or VPC interface the flow does not hold is refused
/// and leaves the stored record exactly as it was.
#[tokio::test]
async fn an_update_resolves_every_reference_it_moves_against_the_flow() {
    let emulator = Emulator::new();
    let flow_arn = fixture_flow(&emulator, "flow-update-references").await;
    let output_arn = described_outputs(&emulator, "updates", &flow_arn).await[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let source_arn = described_sources(&emulator, "updates", &flow_arn).await[0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for unknown in UNKNOWN_NAMES {
        // The output's two reference-carrying members, and the source's three: the update
        // shapes carry them unnested, so the refusal names the member alone.
        let output_bodies = [
            (
                "MediaStreamOutputConfigurations[0].MediaStreamName",
                json!({
                    "mediaStreamOutputConfigurations": [
                        output_association(unknown, HELD_VPC_INTERFACES[0], "198.51.100.30")
                    ]
                }),
            ),
            (
                "MediaStreamOutputConfigurations[0].DestinationConfigurations[0].Interface.Name",
                json!({
                    "mediaStreamOutputConfigurations": [
                        output_association(HELD_MEDIA_STREAMS[0].0, unknown, "198.51.100.31")
                    ]
                }),
            ),
            (
                "VpcInterfaceAttachment.VpcInterfaceName",
                json!({ "vpcInterfaceAttachment": { "vpcInterfaceName": unknown } }),
            ),
        ];
        for (member, update) in output_bodies {
            let case = format!("output {member} as {unknown:?}");
            let before = described_outputs(&emulator, &case, &flow_arn).await;
            let (status, code, body) =
                update_flow_output(&emulator, &flow_arn, &output_arn, update).await;
            assert_unresolved_reference(&case, member, unknown, status, code.as_deref(), &body);
            let after = described_outputs(&emulator, &case, &flow_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update leaves the output as it was, got {after:?}"
            );
        }

        let source_bodies = [
            (
                "MediaStreamSourceConfigurations[0].MediaStreamName",
                json!({
                    "mediaStreamSourceConfigurations": [
                        source_association(unknown, HELD_VPC_INTERFACES[0], 9200)
                    ]
                }),
            ),
            (
                "MediaStreamSourceConfigurations[0].InputConfigurations[0].Interface.Name",
                json!({
                    "mediaStreamSourceConfigurations": [
                        source_association(HELD_MEDIA_STREAMS[0].0, unknown, 9202)
                    ]
                }),
            ),
            ("VpcInterfaceName", json!({ "vpcInterfaceName": unknown })),
            (
                "GatewayBridgeSource.VpcInterfaceAttachment.VpcInterfaceName",
                json!({
                    "gatewayBridgeSource": {
                        "bridgeArn": BRIDGE_ARN,
                        "vpcInterfaceAttachment": { "vpcInterfaceName": unknown }
                    }
                }),
            ),
        ];
        for (member, update) in source_bodies {
            let case = format!("source {member} as {unknown:?}");
            let before = described_sources(&emulator, &case, &flow_arn).await;
            let (status, code, body) =
                update_flow_source(&emulator, &flow_arn, &source_arn, update).await;
            assert_unresolved_reference(&case, member, unknown, status, code.as_deref(), &body);
            let after = described_sources(&emulator, &case, &flow_arn).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update leaves the source as it was, got {after:?}"
            );
        }
    }

    // The same two updates, naming what the flow holds: applied, and read back.
    let case = "an update that moves a reference the flow holds";
    let (status, code, updated) = update_flow_output(
        &emulator,
        &flow_arn,
        &output_arn,
        json!({
            "vpcInterfaceAttachment": { "vpcInterfaceName": HELD_VPC_INTERFACES[1] },
            "mediaStreamOutputConfigurations": [
                output_association(HELD_MEDIA_STREAMS[1].0, HELD_VPC_INTERFACES[1], "198.51.100.32")
            ]
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202 with no error, got {status} {code:?}: {updated}"
    );
    let stored_output = described_outputs(&emulator, case, &flow_arn).await[0].clone();
    assert_eq!(
        stored_output, updated["output"],
        "case {case:?}: DescribeFlow reports the output the update answered, got {stored_output}"
    );

    let (status, code, updated) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({
            "vpcInterfaceName": HELD_VPC_INTERFACES[0],
            "mediaStreamSourceConfigurations": [
                source_association(HELD_MEDIA_STREAMS[0].0, HELD_VPC_INTERFACES[0], 9204)
            ]
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202 with no error, got {status} {code:?}: {updated}"
    );
    let stored_source = described_sources(&emulator, case, &flow_arn).await[0].clone();
    assert_eq!(
        stored_source, updated["source"],
        "case {case:?}: DescribeFlow reports the source the update answered, got {stored_source}"
    );
}

/// The reference resolves against the flow's CURRENT contents, not against a fixed set: the
/// very request a flow refused is accepted once AddFlowMediaStreams and
/// AddFlowVpcInterfaces have given the flow the media stream and VPC interface it names.
#[tokio::test]
async fn a_reference_resolves_as_soon_as_the_flow_holds_what_it_names() {
    let emulator = Emulator::new();
    let flow_arn = fixture_flow(&emulator, "flow-reference-grows").await;
    let case = "a reference to a media stream and VPC interface added later";
    let late_stream = "late-stream";
    let late_interface = "late-vpc";
    let output = json!([{
        "name": "o-late",
        "protocol": "st2110-jpegxs",
        "port": 7200,
        "vpcInterfaceAttachment": { "vpcInterfaceName": late_interface },
        "mediaStreamOutputConfigurations": [
            output_association(late_stream, late_interface, "198.51.100.40")
        ]
    }]);

    let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, output.clone()).await;
    assert_unresolved_reference(
        case,
        "Outputs[0].MediaStreamOutputConfigurations[0].MediaStreamName",
        late_stream,
        status,
        code.as_deref(),
        &body,
    );

    let (status, code, added) = add_media_streams(
        &emulator,
        &flow_arn,
        json!([
            { "mediaStreamId": 9, "mediaStreamName": late_stream, "mediaStreamType": "video" }
        ]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: AddFlowMediaStreams expected 201, got {status} {code:?}: {added}"
    );
    let (status, code, added) = add_vpc_interfaces(
        &emulator,
        &flow_arn,
        json!([vpc_interface_request(
            late_interface,
            None,
            &SECURITY_GROUP_IDS,
            SUBNET_ID,
            None
        )]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: AddFlowVpcInterfaces expected 201, got {status} {code:?}: {added}"
    );

    let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, output).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the same add is accepted once the flow holds both, got {status} \
         {code:?}: {body}"
    );
    let stored = described_outputs(&emulator, case, &flow_arn).await;
    assert_eq!(
        stored.last().cloned().unwrap_or(Value::Null),
        body["outputs"][0],
        "case {case:?}: DescribeFlow reports the output the add answered, got {stored:?}"
    );
}

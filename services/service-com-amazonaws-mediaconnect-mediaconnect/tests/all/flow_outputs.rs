// SPDX-License-Identifier: CC0-1.0

//! AddFlowOutputs, end to end over HTTP: every requested output is materialised into a
//! record of the flow it was added to, and DescribeFlow reports what the add appended.
//!
//! The cases drive the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect
//! client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::{Emulator, REGION};

/// The ARN of a flow in this Region that this service never minted — well formed enough
/// to pass the modelled `FlowArn` label pattern, so the request reaches the handler.
///
/// `pub(crate)` because `tests/all/remove_flow_output.rs` addresses the same unknown flow.
pub(crate) fn unknown_flow_arn() -> String {
    format!("arn:aws:mediaconnect:{REGION}:111122223333:flow:1-0000dead:never-minted")
}

/// `POST /v1/flows/{FlowArn}/outputs` — the AddFlowOutputs binding
/// (`smithy_query(query="operation",operation="AddFlowOutputs")`).
pub(crate) async fn add_flow_outputs(
    emulator: &Emulator,
    flow_arn: &str,
    outputs: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/outputs"),
            Some(json!({ "outputs": outputs })),
        )
        .await
}

/// Create the flow a case adds outputs to, with `initial` outputs already on it, and
/// return its ARN.
pub(crate) async fn create_flow_with_outputs(
    emulator: &Emulator,
    case: &str,
    name: &str,
    initial: Value,
) -> String {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "outputs": initial,
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The flow's stored outputs, as DescribeFlow reports them.
pub(crate) async fn described_outputs(
    emulator: &Emulator,
    case: &str,
    flow_arn: &str,
) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["outputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every `OutputArn` the service mints must satisfy the pattern the model puts on the
/// `OutputArn` label of the operations that address one output
/// (`smithy_query(query="operation",operation="RemoveFlowOutput")`:
/// `^arn:.+:mediaconnect.+:output:.+$`), and carry the output's own name as its last
/// segment, the way a flow ARN carries the flow's.
pub(crate) fn assert_minted_output_arn(case: &str, arn: &str, name: &str) {
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        8,
        "case {case:?}: an output ARN has 8 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", REGION, "output"),
        "case {case:?}: ARN prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        segments[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        segments[4].len() == 12 && segments[4].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the account id is 12 digits, got {arn:?}"
    );
    assert!(
        !segments[6].is_empty(),
        "case {case:?}: the output carries an id of its own, got {arn:?}"
    );
    assert_eq!(
        segments[7], name,
        "case {case:?}: the last segment is the output's name, got {arn:?}"
    );
}

/// One row of the AddFlowOutputs table: the outputs the flow is created with, the
/// outputs one AddFlowOutputs call then adds, and the names the class rule says those
/// added outputs take.
struct AddCase {
    case: &'static str,
    initial: Value,
    add: Value,
    expected_names: &'static [&'static str],
}

/// The rows differ along what the rule generalises over: how many outputs one call adds,
/// whether each names itself or leaves the service to, which transport protocol and
/// transport members it carries (or none at all), and whether it asks for an
/// `OutputStatus`.
fn add_cases() -> Vec<AddCase> {
    vec![
        AddCase {
            case: "one named udp output onto a flow with no outputs",
            initial: json!([]),
            add: json!([
                { "name": "out-1", "protocol": "udp", "destination": "203.0.113.5", "port": 5000 }
            ]),
            expected_names: &["out-1"],
        },
        AddCase {
            case: "a named and an unnamed output onto a flow that already has one",
            initial: json!([
                { "name": "existing", "protocol": "rtp-fec", "destination": "203.0.113.6", "port": 5002 }
            ]),
            add: json!([
                {
                    "name": "srt-out",
                    "protocol": "srt-caller",
                    "destination": "203.0.113.7",
                    "port": 5004,
                    "streamId": "s-9",
                    "minLatency": 150,
                    "description": "the caller output"
                },
                { "protocol": "udp", "destination": "203.0.113.8", "port": 5006 }
            ]),
            expected_names: &["srt-out", "output-1"],
        },
        AddCase {
            case: "three unnamed outputs take successive free generated names",
            initial: json!([
                { "name": "output-1", "protocol": "udp", "destination": "203.0.113.9", "port": 5008 }
            ]),
            add: json!([
                { "protocol": "udp", "destination": "203.0.113.10", "port": 5010 },
                {
                    "protocol": "rist",
                    "destination": "203.0.113.11",
                    "port": 5012,
                    "smoothingLatency": 100,
                    "cidrAllowList": ["10.0.0.0/16", "192.0.2.0/24"]
                },
                {
                    "protocol": "zixi-push",
                    "destination": "203.0.113.12",
                    "port": 5014,
                    "remoteId": "r-1",
                    "maxLatency": 2000
                }
            ]),
            expected_names: &["output-2", "output-3", "output-4"],
        },
        AddCase {
            case: "an explicit OutputStatus is kept and an omitted one defaults",
            initial: json!([]),
            add: json!([
                { "name": "dis", "protocol": "udp", "destination": "203.0.113.13", "port": 5016, "outputStatus": "DISABLED" },
                { "name": "en", "protocol": "udp", "destination": "203.0.113.14", "port": 5018, "outputStatus": "ENABLED" },
                { "name": "def", "protocol": "udp", "destination": "203.0.113.15", "port": 5020 }
            ]),
            expected_names: &["dis", "en", "def"],
        },
        AddCase {
            case: "an output that names no protocol carries no transport",
            initial: json!([]),
            add: json!([
                { "name": "no-proto", "destination": "203.0.113.16", "port": 7000, "description": "no transport" }
            ]),
            expected_names: &["no-proto"],
        },
        AddCase {
            case: "encryption and a VPC interface attachment are stored as requested",
            initial: json!([]),
            add: json!([
                {
                    "name": "enc",
                    "protocol": "srt-listener",
                    "port": 7002,
                    "minLatency": 120,
                    "encryption": {
                        "algorithm": "aes256",
                        "keyType": "static-key",
                        "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                        "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-abc"
                    },
                    "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" }
                }
            ]),
            expected_names: &["enc"],
        },
    ]
}

/// AddFlowOutputs materialises each requested output into a stored record with its own
/// ARN and a resolved `OutputStatus`, appends it to the flow it names, and answers with
/// those records — the same bytes DescribeFlow then reports.
#[tokio::test]
async fn add_flow_outputs_persists_the_outputs_it_returns() {
    let emulator = Emulator::new();

    for (index, case_row) in add_cases().into_iter().enumerate() {
        let AddCase {
            case,
            initial,
            add,
            expected_names,
        } = case_row;
        let flow_arn = create_flow_with_outputs(
            &emulator,
            case,
            &format!("flow-add-{index}"),
            initial.clone(),
        )
        .await;
        let before = described_outputs(&emulator, case, &flow_arn).await;
        let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, add.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
        );
        assert_eq!(
            body["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the outputs were added to, got {:?}",
            body["flowArn"]
        );

        let requested = add.as_array().cloned().unwrap_or_default();
        let added = body["outputs"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            added.len(),
            requested.len(),
            "case {case:?}: one Output per requested output, got {added:?}"
        );

        for (position, (output, request)) in added.iter().zip(requested.iter()).enumerate() {
            let name = output["name"].as_str().unwrap_or_default();
            assert_eq!(
                name,
                expected_names[position],
                "case {case:?}: output {position} name, got {output:?}"
            );
            assert_minted_output_arn(case, output["outputArn"].as_str().unwrap_or_default(), name);

            // "If you don't specify the outputStatus field in your request, MediaConnect
            // sets it to ENABLED" (`AddOutputRequest$OutputStatus`).
            assert_eq!(
                output["outputStatus"].as_str(),
                Some(request["outputStatus"].as_str().unwrap_or("ENABLED")),
                "case {case:?}: output {position} status, got {output:?}"
            );

            // The members `AddOutputRequest` and `Output` share are kept verbatim.
            for member in [
                "description",
                "destination",
                "port",
                "encryption",
                "vpcInterfaceAttachment",
            ] {
                assert_eq!(
                    output[member], request[member],
                    "case {case:?}: output {position} {member}, got {output:?}"
                );
            }

            // The transport members of the request fold into `Output.Transport`, and an
            // output that names no protocol has no transport at all (`Transport.Protocol`
            // is required).
            match request["protocol"].as_str() {
                Some(protocol) => {
                    assert_eq!(
                        output["transport"]["protocol"].as_str(),
                        Some(protocol),
                        "case {case:?}: output {position} transport protocol, got {output:?}"
                    );
                    for member in [
                        "cidrAllowList",
                        "maxLatency",
                        "minLatency",
                        "remoteId",
                        "senderControlPort",
                        "smoothingLatency",
                        "streamId",
                    ] {
                        assert_eq!(
                            output["transport"][member], request[member],
                            "case {case:?}: output {position} transport {member}, got {output:?}"
                        );
                    }
                }
                None => assert!(
                    output["transport"].is_null(),
                    "case {case:?}: output {position} has no transport, got {output:?}"
                ),
            }
        }

        // The round trip: the flow now holds the outputs it held before, in order,
        // followed by the ones this call added — byte for byte what the add answered.
        let after = described_outputs(&emulator, case, &flow_arn).await;
        let expected: Vec<Value> = before.iter().chain(added.iter()).cloned().collect();
        assert_eq!(
            after, expected,
            "case {case:?}: DescribeFlow reports the previous outputs then the added ones, \
             got {after:?}"
        );

        // Adding an output does not move the flow's status.
        let (_, _, described) = emulator.describe_flow(&flow_arn).await;
        assert_eq!(
            described["flow"]["status"].as_str(),
            Some("STANDBY"),
            "case {case:?}: the flow keeps the status it had, got {:?}",
            described["flow"]["status"]
        );
    }
}

/// One row of the refusal table: a request AddFlowOutputs must refuse, and the modelled
/// error it must refuse it with.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    outputs: Value,
    status: u16,
    code: &'static str,
}

/// A request the service will not satisfy is refused with the operation's own modelled
/// error, and stores nothing: the flow still holds exactly the outputs it held.
#[tokio::test]
async fn add_flow_outputs_refuses_what_it_cannot_add() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_outputs(
        &emulator,
        "refusals",
        "flow-refusals",
        json!([
            { "name": "taken", "protocol": "udp", "destination": "203.0.113.30", "port": 5100 }
        ]),
    )
    .await;
    let before = described_outputs(&emulator, "refusals", &flow_arn).await;

    let one_output =
        json!([{ "name": "fresh", "protocol": "udp", "destination": "203.0.113.31", "port": 5102 }]);
    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            outputs: one_output.clone(),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "a request that adds no output at all",
            flow_arn: flow_arn.clone(),
            outputs: json!([]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a name the flow already uses",
            flow_arn: flow_arn.clone(),
            outputs: json!([
                { "name": "taken", "protocol": "udp", "destination": "203.0.113.32", "port": 5104 }
            ]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "one name used twice inside a single request",
            flow_arn: flow_arn.clone(),
            outputs: json!([
                { "name": "twice", "protocol": "udp", "destination": "203.0.113.33", "port": 5106 },
                { "name": "twice", "protocol": "udp", "destination": "203.0.113.34", "port": 5108 }
            ]),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target,
        outputs,
        status: expected_status,
        code: expected_code,
    } in rows
    {
        let (status, code, body) = add_flow_outputs(&emulator, &target, outputs).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        let after = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused request stores nothing, got {after:?}"
        );
    }
}

/// One `MediaStreamOutputConfigurations` entry for the media stream `media_stream_name`,
/// sending to `destination_ip`.
///
/// `pub(crate)` because `tests/all/remove_flow_media_stream.rs` associates a media stream
/// with an output the same way.
pub(crate) fn media_stream_output_configuration(
    media_stream_name: &str,
    destination_ip: &str,
) -> Value {
    json!({
        "encodingName": "jxsv",
        "mediaStreamName": media_stream_name,
        "destinationConfigurations": [
            {
                "destinationIp": destination_ip,
                "destinationPort": 6000,
                "interface": { "name": "vpc-1" }
            }
        ],
        "encodingParameters": { "compressionFactor": 8.0, "encoderProfile": "high" }
    })
}

/// Assert the stored `configurations` are the `requested` ones, with the one member no
/// request can carry — `DestinationConfiguration$OutboundIp`, "the IP address that the
/// receiver requires in order to establish a connection with the flow" — assigned by the
/// SERVICE from the documentation-range pool its egress addresses are drawn from
/// (`service_..::state::assigned_egress_ip`).
fn assert_media_stream_output_configurations(
    case: &str,
    configurations: &Value,
    requested: &Value,
) {
    let stored = configurations.as_array().cloned().unwrap_or_default();
    let wanted = requested.as_array().cloned().unwrap_or_default();
    assert_eq!(
        stored.len(),
        wanted.len(),
        "case {case:?}: one configuration per requested configuration, got {configurations:?}"
    );
    for (position, (configuration, request)) in stored.iter().zip(wanted.iter()).enumerate() {
        for member in ["encodingName", "mediaStreamName", "encodingParameters"] {
            assert_eq!(
                configuration[member], request[member],
                "case {case:?}: configuration {position} {member}, got {configuration:?}"
            );
        }
        let destinations = configuration["destinationConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let wanted_destinations = request["destinationConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            destinations.len(),
            wanted_destinations.len(),
            "case {case:?}: configuration {position} destination count, got {configuration:?}"
        );
        for (destination, want) in destinations.iter().zip(wanted_destinations.iter()) {
            for member in ["destinationIp", "destinationPort", "interface"] {
                assert_eq!(
                    destination[member], want[member],
                    "case {case:?}: configuration {position} destination {member}, got \
                     {destination:?}"
                );
            }
            let outbound = destination["outboundIp"].as_str().unwrap_or_default();
            assert!(
                (1..=254).any(|host| {
                    service_com_amazonaws_mediaconnect_mediaconnect::state::assigned_egress_ip(host)
                        == outbound
                }),
                "case {case:?}: configuration {position} outboundIp is an address the service \
                 assigns from its egress pool, got {destination:?}"
            );
        }
    }
}

/// The members of an output request that the stored record carries beyond the plain ones:
/// its media stream associations and its router integration settings. Both the operations
/// that ADD an output to a flow store them, so both are read back through DescribeFlow.
#[tokio::test]
async fn an_added_output_persists_its_media_stream_and_router_integration_settings() {
    let emulator = Emulator::new();

    let case = "an output created with media stream associations";
    let created_configurations = json!([media_stream_output_configuration("ms-1", "198.51.100.60")]);
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "flow-output-media-streams",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
            "mediaStreams": [
                { "mediaStreamId": 1, "mediaStreamName": "ms-1", "mediaStreamType": "video" },
                { "mediaStreamId": 2, "mediaStreamName": "ms-2", "mediaStreamType": "audio" }
            ],
            "outputs": [{
                "name": "created",
                "protocol": "st2110-jpegxs",
                "port": 7100,
                "mediaStreamOutputConfigurations": created_configurations,
                "routerIntegrationState": "ENABLED"
            }],
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let stored = described_outputs(&emulator, case, &flow_arn).await;
    let output = stored.first().cloned().unwrap_or(Value::Null);
    assert_media_stream_output_configurations(
        case,
        &output["mediaStreamOutputConfigurations"],
        &created_configurations,
    );
    assert_eq!(
        output["routerIntegrationState"].as_str(),
        Some("ENABLED"),
        "case {case:?}: the output reports the router integration state it was created \
         with, got {output:?}"
    );

    let case = "an output added with media stream associations";
    let added_configurations = json!([
        media_stream_output_configuration("ms-1", "198.51.100.61"),
        media_stream_output_configuration("ms-2", "198.51.100.62")
    ]);
    let (status, code, body) = add_flow_outputs(
        &emulator,
        &flow_arn,
        json!([{
            "name": "added",
            "protocol": "st2110-jpegxs",
            "port": 7102,
            "mediaStreamOutputConfigurations": added_configurations
        }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
    );
    assert_media_stream_output_configurations(
        case,
        &body["outputs"][0]["mediaStreamOutputConfigurations"],
        &added_configurations,
    );
    // "Indicates if router integration is enabled or disabled on the flow output" is
    // reported on every output, so one added without it reports the disabled state.
    assert_eq!(
        body["outputs"][0]["routerIntegrationState"].as_str(),
        Some("DISABLED"),
        "case {case:?}: an output added without a router integration state reports the \
         disabled one, got {body}"
    );

    // The round trip: DescribeFlow reports what the add answered.
    let after = described_outputs(&emulator, case, &flow_arn).await;
    assert_eq!(
        after.last().cloned().unwrap_or(Value::Null),
        body["outputs"][0],
        "case {case:?}: DescribeFlow reports the record the add answered, got {after:?}"
    );
}

/// `count` output requests, named `out-<n>` from `first`, so a case can fill a flow to a
/// known number of outputs.
fn numbered_outputs(first: usize, count: usize) -> Value {
    (first..first + count)
        .map(|index| {
            json!({
                "name": format!("out-{index}"),
                "protocol": "udp",
                "destination": "203.0.113.40",
                "port": 6000 + i32::try_from(index).unwrap_or(0) * 2
            })
        })
        .collect()
}

/// "You can create up to 50 outputs per flow" bounds the STORED flow, however many calls
/// fill it: the call that fills the last slot is accepted, and anything past the bound is
/// refused with the modelled `AddFlowOutputs420Exception` (420).
#[tokio::test]
async fn add_flow_outputs_bounds_the_outputs_one_flow_holds() {
    const LIMIT: usize = service_com_amazonaws_mediaconnect_mediaconnect::state::MAX_FLOW_OUTPUTS;

    let emulator = Emulator::new();

    // A flow created with ten outputs, filled to the bound by one add.
    let case = "filled to the bound over two calls";
    let flow_arn =
        create_flow_with_outputs(&emulator, case, "flow-ceiling", numbered_outputs(0, 10)).await;
    let (status, code, body) =
        add_flow_outputs(&emulator, &flow_arn, numbered_outputs(10, LIMIT - 10)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the add that fills the last slot is accepted, got {status} {code:?}: \
         {body}"
    );
    let at_bound = described_outputs(&emulator, case, &flow_arn).await;
    assert_eq!(
        at_bound.len(),
        LIMIT,
        "case {case:?}: the flow holds {LIMIT} outputs, got {}",
        at_bound.len()
    );

    // One output past the bound.
    let case = "one output past the bound";
    let (status, code, body) =
        add_flow_outputs(&emulator, &flow_arn, numbered_outputs(LIMIT, 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("AddFlowOutputs420Exception")),
        "case {case:?}: expected 420 AddFlowOutputs420Exception, got {status} {code:?}: {body}"
    );
    let after = described_outputs(&emulator, case, &flow_arn).await;
    assert_eq!(
        after, at_bound,
        "case {case:?}: the refused add stores nothing, got {} outputs",
        after.len()
    );

    // And a single request that would overshoot the bound on its own.
    let case = "one request past the bound on an empty flow";
    let fresh =
        create_flow_with_outputs(&emulator, case, "flow-ceiling-single", json!([])).await;
    let (status, code, body) =
        add_flow_outputs(&emulator, &fresh, numbered_outputs(0, LIMIT + 1)).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("AddFlowOutputs420Exception")),
        "case {case:?}: expected 420 AddFlowOutputs420Exception, got {status} {code:?}: {body}"
    );
    assert!(
        described_outputs(&emulator, case, &fresh).await.is_empty(),
        "case {case:?}: the refused add stores nothing"
    );

    // At the bound in one request: the same count the two-call path accepted.
    let case = "one request exactly at the bound";
    let exact = create_flow_with_outputs(&emulator, case, "flow-ceiling-exact", json!([])).await;
    let (status, code, body) =
        add_flow_outputs(&emulator, &exact, numbered_outputs(0, LIMIT)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
    );
    assert_eq!(
        described_outputs(&emulator, case, &exact).await.len(),
        LIMIT,
        "case {case:?}: the flow holds {LIMIT} outputs"
    );
}

/// An output name is unique within its flow, so a generated name is the first free
/// `output-<n>` — across calls, and around names a caller chose for itself.
#[tokio::test]
async fn generated_output_names_stay_unique_across_calls() {
    let emulator = Emulator::new();
    let case = "generated names skip the ones already taken";
    let flow_arn = create_flow_with_outputs(
        &emulator,
        case,
        "flow-generated-names",
        json!([
            { "name": "output-2", "protocol": "udp", "destination": "203.0.113.50", "port": 6500 }
        ]),
    )
    .await;

    for (call, expected) in [
        (json!([{ "protocol": "udp" }, { "protocol": "udp" }]), vec!["output-1", "output-3"]),
        (json!([{ "protocol": "udp" }]), vec!["output-4"]),
    ] {
        let (status, code, body) = add_flow_outputs(&emulator, &flow_arn, call).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: expected 201 with no error, got {status} {code:?}: {body}"
        );
        let names: Vec<&str> = body["outputs"]
            .as_array()
            .map(|outputs| {
                outputs
                    .iter()
                    .map(|output| output["name"].as_str().unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names, expected,
            "case {case:?}: the generated names are the first free ones, got {names:?}"
        );
    }

    let mut stored: Vec<&str> = Vec::new();
    let outputs = described_outputs(&emulator, case, &flow_arn).await;
    for output in &outputs {
        stored.push(output["name"].as_str().unwrap_or_default());
    }
    let mut unique = stored.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        stored.len(),
        "case {case:?}: every output name on the flow is unique, got {stored:?}"
    );
}

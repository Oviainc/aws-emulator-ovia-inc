// SPDX-License-Identifier: CC0-1.0

//! UpdateFlowOutput, end to end over HTTP: a partial update of ONE stored output of a
//! flow.
//!
//! The rule these cases assert, over a table rather than over one request: a member the
//! request carries replaces that setting on the stored output, a member it omits leaves
//! the stored setting untouched, the output's identity (its name and its ARN) never moves,
//! and the 202 answer is the stored record — the same bytes DescribeFlow then reports. A
//! request that resolves no output, or that would leave a setting the model declares
//! required without a value, is refused and writes nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_outputs::{create_flow_with_outputs, described_outputs, unknown_flow_arn};

/// `PUT /v1/flows/{FlowArn}/outputs/{OutputArn}` — the UpdateFlowOutput binding
/// (`smithy_query(query="operation",operation="UpdateFlowOutput")`).
async fn update_flow_output(
    emulator: &Emulator,
    flow_arn: &str,
    output_arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
            Some(body),
        )
        .await
}

/// The value at a dotted path of a JSON object, where a numeric segment indexes a list:
/// `transport.minLatency`, `mediaStreamOutputConfigurations.0.encodingName`.
fn at_path<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = match segment.parse::<usize>() {
            Ok(index) => &current[index],
            Err(_) => &current[segment],
        };
    }
    current
}

/// `value` with the given top-level keys removed, so two outputs can be compared on
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

/// One row of the UpdateFlowOutput table: the request body, and the paths of the stored
/// output it must change, with the values they must take.
struct UpdateCase {
    case: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// The rows differ along what the rule generalises over: which member the request carries
/// (a scalar, an enum, a list, a nested structure, the request-only `UpdateEncryption`
/// wrapper), how many at once, whether the member belongs to the output itself or to its
/// `Transport`, whether the setting existed before, and a request that carries none at
/// all. They are applied in order to the SAME output, so each row also proves the
/// settings the earlier rows wrote survive an update that does not mention them.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a description on its own",
            body: json!({ "description": "the primary feed" }),
            expect: vec![("description", json!("the primary feed"))],
        },
        UpdateCase {
            case: "the destination and the port together",
            body: json!({ "destination": "203.0.113.80", "port": 9000 }),
            expect: vec![
                ("destination", json!("203.0.113.80")),
                ("port", json!(9000)),
            ],
        },
        UpdateCase {
            // "An indication of whether the output should transmit data or not. If you
            // don't specify the outputStatus field in your request, MediaConnect leaves
            // the value unchanged." (`UpdateFlowOutputRequest$OutputStatus`.)
            case: "the output status toggled to DISABLED",
            body: json!({ "outputStatus": "DISABLED" }),
            expect: vec![("outputStatus", json!("DISABLED"))],
        },
        UpdateCase {
            case: "two transport latency members, leaving the protocol alone",
            body: json!({ "minLatency": 200, "smoothingLatency": 90 }),
            expect: vec![
                ("transport.minLatency", json!(200)),
                ("transport.smoothingLatency", json!(90)),
                ("transport.protocol", json!("udp")),
            ],
        },
        UpdateCase {
            case: "the protocol changed, leaving the latency members alone",
            body: json!({ "protocol": "srt-listener" }),
            expect: vec![
                ("transport.protocol", json!("srt-listener")),
                ("transport.minLatency", json!(200)),
                ("transport.smoothingLatency", json!(90)),
            ],
        },
        UpdateCase {
            case: "a stream id and the NDI transport members",
            body: json!({
                "streamId": "s-42",
                "ndiProgramName": "programme",
                "ndiSpeedHqQuality": 80,
                "ndiOutputTimecodeSource": "UTC_SYSTEM_TIME",
                "ndiAudioReferenceLevel": -18
            }),
            expect: vec![
                ("transport.streamId", json!("s-42")),
                ("transport.ndiProgramName", json!("programme")),
                ("transport.ndiSpeedHqQuality", json!(80)),
                ("transport.ndiOutputTimecodeSource", json!("UTC_SYSTEM_TIME")),
                ("transport.ndiAudioReferenceLevel", json!(-18)),
                ("transport.protocol", json!("srt-listener")),
            ],
        },
        UpdateCase {
            case: "encryption given to an output that had none, with the required role",
            body: json!({
                "encryption": {
                    "algorithm": "aes256",
                    "keyType": "static-key",
                    "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                    "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-1"
                }
            }),
            expect: vec![
                ("encryption.algorithm", json!("aes256")),
                ("encryption.keyType", json!("static-key")),
                (
                    "encryption.roleArn",
                    json!("arn:aws:iam::111122223333:role/MediaConnect"),
                ),
                (
                    "encryption.secretArn",
                    json!("arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-1"),
                ),
            ],
        },
        UpdateCase {
            // `UpdateEncryption`'s members are each optional, so one member of the
            // encryption changes and its siblings keep the values the row above wrote.
            case: "one member of the encryption changed",
            body: json!({ "encryption": { "algorithm": "aes128" } }),
            expect: vec![
                ("encryption.algorithm", json!("aes128")),
                ("encryption.keyType", json!("static-key")),
                (
                    "encryption.roleArn",
                    json!("arn:aws:iam::111122223333:role/MediaConnect"),
                ),
            ],
        },
        UpdateCase {
            case: "a VPC interface attachment",
            body: json!({ "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-7" } }),
            expect: vec![(
                "vpcInterfaceAttachment",
                json!({ "vpcInterfaceName": "vpc-7" }),
            )],
        },
        UpdateCase {
            // `EncodingName` is one of jxsv | raw | smpte291 | pcm and `EncoderProfile`
            // one of main | high (`smithy_query(query="shape",shape="EncodingName")` /
            // `…shape="EncoderProfile"`).
            case: "the media streams associated with the output",
            body: json!({
                "mediaStreamOutputConfigurations": [
                    {
                        "mediaStreamName": "vid",
                        "encodingName": "jxsv",
                        "encodingParameters": { "compressionFactor": 8.0, "encoderProfile": "high" },
                        "destinationConfigurations": [
                            {
                                "destinationIp": "203.0.113.90",
                                "destinationPort": 9100,
                                "interface": { "name": "vpc-7" }
                            }
                        ]
                    }
                ]
            }),
            expect: vec![
                (
                    "mediaStreamOutputConfigurations.0.mediaStreamName",
                    json!("vid"),
                ),
                ("mediaStreamOutputConfigurations.0.encodingName", json!("jxsv")),
                (
                    "mediaStreamOutputConfigurations.0.encodingParameters.compressionFactor",
                    json!(8.0),
                ),
                (
                    "mediaStreamOutputConfigurations.0.encodingParameters.encoderProfile",
                    json!("high"),
                ),
                (
                    "mediaStreamOutputConfigurations.0.destinationConfigurations.0.destinationIp",
                    json!("203.0.113.90"),
                ),
                (
                    "mediaStreamOutputConfigurations.0.destinationConfigurations.0.destinationPort",
                    json!(9100),
                ),
                (
                    "mediaStreamOutputConfigurations.0.destinationConfigurations.0.interface.name",
                    json!("vpc-7"),
                ),
            ],
        },
        UpdateCase {
            case: "an empty association list disassociates every media stream",
            body: json!({ "mediaStreamOutputConfigurations": [] }),
            expect: vec![("mediaStreamOutputConfigurations", Value::Null)],
        },
        UpdateCase {
            // "Indicates whether to enable or disable router integration for this flow
            // output" — stored as configured and reported for every output.
            case: "the router integration state enabled",
            body: json!({ "routerIntegrationState": "ENABLED" }),
            expect: vec![("routerIntegrationState", json!("ENABLED"))],
        },
        UpdateCase {
            case: "a request that carries no setting at all",
            body: json!({}),
            expect: Vec::new(),
        },
    ]
}

/// UpdateFlowOutput writes the settings the request carries onto the stored output,
/// leaves the ones it omits alone, and answers with the stored record.
#[tokio::test]
async fn update_flow_output_applies_only_the_settings_the_request_carries() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_outputs(
        &emulator,
        "updates",
        "flow-update-output",
        json!([
            { "name": "feed", "protocol": "udp", "destination": "203.0.113.75", "port": 8900 },
            { "name": "other", "protocol": "rtp", "destination": "203.0.113.76", "port": 8902 }
        ]),
    )
    .await;
    let held = described_outputs(&emulator, "updates", &flow_arn).await;
    let output_arn = held[0]["outputArn"].as_str().unwrap_or_default().to_string();
    let untouched_sibling = held[1].clone();

    for UpdateCase { case, body, expect } in update_cases() {
        let before = described_outputs(&emulator, case, &flow_arn).await[0].clone();
        let (status, code, response) =
            update_flow_output(&emulator, &flow_arn, &output_arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
        );
        assert_eq!(
            response["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the output belongs to, got {response}"
        );
        let updated = response["output"].clone();

        for (path, expected) in &expect {
            assert_eq!(
                at_path(&updated, path),
                expected,
                "case {case:?}: {path} must be {expected}, got {updated}"
            );
        }

        // Everything the request did not touch is exactly as it was — including the
        // output's identity, which no input member names.
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
            (updated["name"].as_str(), updated["outputArn"].as_str()),
            (Some("feed"), Some(output_arn.as_str())),
            "case {case:?}: an update never moves the output's identity, got {updated}"
        );

        // The round trip: the 202 body is the stored record, and no other output of the
        // flow moved.
        let after = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            after[0], updated,
            "case {case:?}: DescribeFlow reports what the update answered, got {:?}",
            after[0]
        );
        assert_eq!(
            after[1], untouched_sibling,
            "case {case:?}: the flow's other output is untouched, got {:?}",
            after[1]
        );
        assert_eq!(
            after.len(),
            2,
            "case {case:?}: an update adds and removes no output, got {} outputs",
            after.len()
        );
    }
}

/// The `OutboundIp` of a destination configuration has no counterpart in the request
/// shape and is REQUIRED on the stored one, so the SERVICE assigns it: it is an address of
/// the documentation range the emulator's egress pool draws from, and it is STORED — every
/// later read reports the same address.
#[tokio::test]
async fn an_association_carries_the_service_assigned_outbound_address() {
    let emulator = Emulator::new();
    let case = "the outbound address of an association";
    let flow_arn = create_flow_with_outputs(
        &emulator,
        case,
        "flow-update-outbound",
        json!([
            { "name": "feed", "protocol": "st2110-jpegxs", "destination": "203.0.113.77", "port": 8904 }
        ]),
    )
    .await;
    let output_arn = described_outputs(&emulator, case, &flow_arn).await[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, response) = update_flow_output(
        &emulator,
        &flow_arn,
        &output_arn,
        json!({
            "mediaStreamOutputConfigurations": [
                {
                    "mediaStreamName": "vid",
                    "encodingName": "jxsv",
                    "destinationConfigurations": [
                        { "destinationIp": "203.0.113.91", "destinationPort": 9102, "interface": { "name": "vpc-1" } },
                        { "destinationIp": "203.0.113.92", "destinationPort": 9104, "interface": { "name": "vpc-1" } }
                    ]
                },
                { "mediaStreamName": "aud", "encodingName": "pcm" }
            ]
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
    );

    let configurations = response["output"]["mediaStreamOutputConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        configurations.len(),
        2,
        "case {case:?}: one configuration per requested association, got {configurations:?}"
    );
    for destination in configurations[0]["destinationConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let outbound = destination["outboundIp"].as_str().unwrap_or_default();
        assert!(
            outbound.starts_with("203.0.113."),
            "case {case:?}: the service assigns an outbound address from its egress pool, got \
             {destination}"
        );
    }
    // An association with no destination configurations keeps none, rather than an empty
    // list the request never sent.
    assert!(
        configurations[1]["destinationConfigurations"].is_null(),
        "case {case:?}: an association with no destinations carries none, got {:?}",
        configurations[1]
    );

    // Stable across reads: DescribeFlow reports the same addresses, twice.
    for read in 0..2 {
        let stored = described_outputs(&emulator, case, &flow_arn).await[0]
            ["mediaStreamOutputConfigurations"]
            .clone();
        assert_eq!(
            stored,
            response["output"]["mediaStreamOutputConfigurations"],
            "case {case:?}: read {read} reports the stored associations unchanged, got {stored}"
        );
    }
}

/// One row of the refusal table: an UpdateFlowOutput request the service must refuse, what
/// it carries, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    output_arn: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A request UpdateFlowOutput will not satisfy is refused with the operation's own
/// modelled error and writes nothing: the output still carries exactly the settings it
/// carried.
#[tokio::test]
async fn update_flow_output_refuses_what_it_cannot_apply() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_outputs(
        &emulator,
        "refusals",
        "flow-update-refusals",
        json!([
            { "name": "feed", "protocol": "udp", "destination": "203.0.113.78", "port": 8906 },
            { "name": "doomed", "protocol": "udp", "destination": "203.0.113.79", "port": 8908 },
            { "name": "bare", "destination": "203.0.113.81", "port": 8910 }
        ]),
    )
    .await;
    let other_flow = create_flow_with_outputs(
        &emulator,
        "refusals",
        "flow-update-other",
        json!([
            { "name": "elsewhere", "protocol": "udp", "destination": "203.0.113.82", "port": 8912 }
        ]),
    )
    .await;
    let held = described_outputs(&emulator, "refusals", &flow_arn).await;
    let output_arn = held[0]["outputArn"].as_str().unwrap_or_default().to_string();
    let bare_arn = held[2]["outputArn"].as_str().unwrap_or_default().to_string();
    let other_output = described_outputs(&emulator, "refusals", &other_flow).await[0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // An output removed for real, so a mutation addressed at it resolves nothing: the
    // interaction `RemoveFlowOutput` and `UpdateFlowOutput` share.
    let removed = held[1]["outputArn"].as_str().unwrap_or_default().to_string();
    let (status, _, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/outputs/{removed}"),
            None,
        )
        .await;
    assert_eq!(
        status, 202,
        "the fixture removal must succeed, got {status}: {body}"
    );

    let before = described_outputs(&emulator, "refusals", &flow_arn).await;
    let never_minted =
        format!("arn:aws:mediaconnect:{REGION}:111122223333:output:1-0000dead:never-minted");

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            output_arn: output_arn.clone(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow_arn()),
        },
        RefusalCase {
            case: "an output ARN this service never minted",
            flow_arn: flow_arn.clone(),
            output_arn: never_minted.clone(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(never_minted),
        },
        RefusalCase {
            case: "an output of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            output_arn: other_output.clone(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(other_output),
        },
        RefusalCase {
            case: "an output that RemoveFlowOutput has removed",
            flow_arn: flow_arn.clone(),
            output_arn: removed.clone(),
            body: json!({ "outputStatus": "DISABLED" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(removed),
        },
        RefusalCase {
            // `Transport$Protocol` is required, so transport settings cannot be given to
            // an output that has no transport without naming the protocol.
            case: "transport settings on an output with no transport, and no protocol",
            flow_arn: flow_arn.clone(),
            output_arn: bare_arn.clone(),
            body: json!({ "minLatency": 150 }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `Encryption$RoleArn` is required, and `UpdateEncryption$RoleArn` is not.
            case: "encryption without the required role on an unencrypted output",
            flow_arn: flow_arn.clone(),
            output_arn: output_arn.clone(),
            body: json!({ "encryption": { "algorithm": "aes256" } }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "an output status outside the modelled value set",
            flow_arn: flow_arn.clone(),
            output_arn: output_arn.clone(),
            body: json!({ "outputStatus": "PAUSED" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `NdiAudioReferenceLevel` is bounded -40..0 by the model.
            case: "an NDI audio reference level above the modelled range",
            flow_arn: flow_arn.clone(),
            output_arn: output_arn.clone(),
            body: json!({ "ndiAudioReferenceLevel": 5 }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "an output label that is no output ARN at all",
            flow_arn: flow_arn.clone(),
            output_arn: "not-an-output-arn".to_string(),
            body: json!({ "description": "nope" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        output_arn: target_output,
        body,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, response) =
            update_flow_output(&emulator, &target_flow, &target_output, body).await;
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

        let after = described_outputs(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, got {after:?}"
        );
    }
}

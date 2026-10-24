// SPDX-License-Identifier: CC0-1.0

//! UpdateFlowSource, end to end over HTTP: a partial update of ONE stored source of a
//! flow.
//!
//! The rule these cases assert, over a table rather than over one request: a member the
//! request carries replaces that setting on the stored source, a member it omits leaves
//! the stored setting untouched, the source's identity (its name and its ARN) never moves,
//! and the 202 answer is the stored record — the same bytes DescribeFlow then reports. A
//! request that resolves no source — including one whose `SourceArn` RemoveFlowSource has
//! already detached — or that would leave a setting the model declares required without a
//! value, is refused with the operation's own modelled error and writes nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_sources::{
    create_flow_with_sources, described_sources, remove_flow_source, unknown_flow_arn,
    unknown_source_arn,
};
use crate::update_flow_output::{at_path, without_keys};

/// `PUT /v1/flows/{FlowArn}/source/{SourceArn}` — the UpdateFlowSource binding
/// (`smithy_query(query="operation",operation="UpdateFlowSource")`).
async fn update_flow_source(
    emulator: &Emulator,
    flow_arn: &str,
    source_arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "PUT",
            &format!("/v1/flows/{flow_arn}/source/{source_arn}"),
            Some(body),
        )
        .await
}

/// One row of the UpdateFlowSource table: the request body, and the paths of the stored
/// source it must change, with the values they must take.
struct UpdateCase {
    case: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// The rows differ along what the rule generalises over: which member the request carries
/// (a scalar, an enum, a list, a nested structure, the request-only `UpdateEncryption` and
/// `UpdateGatewayBridgeSourceRequest` wrappers), how many at once, whether the member
/// belongs to the source itself, to its `Transport`, or to both, whether the setting
/// existed before, and a request that carries none at all. They are applied in order to
/// the SAME source, so each row also proves the settings the earlier rows wrote survive an
/// update that does not mention them.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a description on its own",
            body: json!({ "description": "the primary feed" }),
            expect: vec![("description", json!("the primary feed"))],
        },
        UpdateCase {
            case: "the whitelist CIDR and the ingest port together",
            body: json!({ "whitelistCidr": "10.20.0.0/16", "ingestPort": 5101 }),
            expect: vec![
                ("whitelistCidr", json!("10.20.0.0/16")),
                ("ingestPort", json!(5101)),
            ],
        },
        UpdateCase {
            // `Transport$MaxBitrate` is the service's assignment for an `rtp` source that
            // named none (`crate::state::ASSIGNED_SOURCE_MAX_BITRATE_BPS`), and a request
            // that touches other transport members leaves it alone.
            case: "two transport buffer members, leaving the protocol alone",
            body: json!({ "maxLatency": 1500, "maxSyncBuffer": 1000 }),
            expect: vec![
                ("transport.maxLatency", json!(1500)),
                ("transport.maxSyncBuffer", json!(1000)),
                ("transport.protocol", json!("rtp")),
                ("transport.maxBitrate", json!(160_000_000)),
            ],
        },
        UpdateCase {
            case: "the protocol changed, leaving the buffer members alone",
            body: json!({ "protocol": "srt-listener", "minLatency": 120 }),
            expect: vec![
                ("transport.protocol", json!("srt-listener")),
                ("transport.minLatency", json!(120)),
                ("transport.maxLatency", json!(1500)),
                ("transport.maxSyncBuffer", json!(1000)),
            ],
        },
        UpdateCase {
            case: "an explicit max bitrate replaces the assigned one",
            body: json!({ "maxBitrate": 7_000_000 }),
            expect: vec![
                ("transport.maxBitrate", json!(7_000_000)),
                ("transport.protocol", json!("srt-listener")),
            ],
        },
        UpdateCase {
            case: "the srt-caller listener members and a stream id",
            body: json!({
                "protocol": "srt-caller",
                "sourceListenerAddress": "198.51.100.20",
                "sourceListenerPort": 9200,
                "streamId": "s-42"
            }),
            expect: vec![
                ("transport.protocol", json!("srt-caller")),
                ("transport.sourceListenerAddress", json!("198.51.100.20")),
                ("transport.sourceListenerPort", json!(9200)),
                ("transport.streamId", json!("s-42")),
            ],
        },
        UpdateCase {
            // `SenderIpAddress` and `SenderControlPort` are members of `Source` AND of its
            // `Transport`, so one request member moves both.
            case: "the sender address and control port, which live on both",
            body: json!({ "senderIpAddress": "198.51.100.9", "senderControlPort": 9101 }),
            expect: vec![
                ("senderIpAddress", json!("198.51.100.9")),
                ("senderControlPort", json!(9101)),
                ("transport.senderIpAddress", json!("198.51.100.9")),
                ("transport.senderControlPort", json!(9101)),
            ],
        },
        UpdateCase {
            // `NdiSourceSettings`: "the exact name of an existing NDI sender", and an
            // audio reference level bounded -40..0
            // (`smithy_query(query="shape",shape="NdiSourceSettings")`).
            case: "the NDI source settings",
            body: json!({
                "protocol": "ndi-speed-hq",
                "ndiSourceSettings": { "sourceName": "STUDIO (Programme)", "audioReferenceLevel": -20 }
            }),
            expect: vec![
                ("transport.protocol", json!("ndi-speed-hq")),
                (
                    "transport.ndiSourceSettings.sourceName",
                    json!("STUDIO (Programme)"),
                ),
                ("transport.ndiSourceSettings.audioReferenceLevel", json!(-20)),
            ],
        },
        UpdateCase {
            // "When you update the NDI source, include only the members you want to
            // change. MediaConnect leaves the members that you don't specify unchanged."
            case: "one member of the NDI source settings changed",
            body: json!({ "ndiSourceSettings": { "audioReferenceLevel": -18 } }),
            expect: vec![
                ("transport.ndiSourceSettings.audioReferenceLevel", json!(-18)),
                (
                    "transport.ndiSourceSettings.sourceName",
                    json!("STUDIO (Programme)"),
                ),
            ],
        },
        UpdateCase {
            case: "decryption given to a source that had none, with the required role",
            body: json!({
                "decryption": {
                    "algorithm": "aes256",
                    "keyType": "static-key",
                    "roleArn": "arn:aws:iam::111122223333:role/MediaConnect",
                    "secretArn": "arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-1"
                }
            }),
            expect: vec![
                ("decryption.algorithm", json!("aes256")),
                ("decryption.keyType", json!("static-key")),
                (
                    "decryption.roleArn",
                    json!("arn:aws:iam::111122223333:role/MediaConnect"),
                ),
                (
                    "decryption.secretArn",
                    json!("arn:aws:secretsmanager:us-east-1:111122223333:secret:mc-1"),
                ),
            ],
        },
        UpdateCase {
            // `UpdateEncryption`'s members are each optional, so one member of the
            // decryption changes and its siblings keep the values the row above wrote.
            case: "one member of the decryption changed",
            body: json!({ "decryption": { "algorithm": "aes128" } }),
            expect: vec![
                ("decryption.algorithm", json!("aes128")),
                ("decryption.keyType", json!("static-key")),
                (
                    "decryption.roleArn",
                    json!("arn:aws:iam::111122223333:role/MediaConnect"),
                ),
            ],
        },
        UpdateCase {
            case: "a VPC interface name",
            body: json!({ "vpcInterfaceName": "vpc-7" }),
            expect: vec![("vpcInterfaceName", json!("vpc-7"))],
        },
        UpdateCase {
            // "Indicates whether to enable or disable router integration for this flow
            // source" — stored as configured and reported for every source.
            case: "the router integration state enabled",
            body: json!({ "routerIntegrationState": "ENABLED" }),
            expect: vec![("routerIntegrationState", json!("ENABLED"))],
        },
        UpdateCase {
            // `EntitlementArn` carries the modelled pattern
            // `^arn:.+:mediaconnect.+:entitlement:.+$`.
            case: "an entitlement ARN",
            body: json!({
                "entitlementArn":
                    "arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared"
            }),
            expect: vec![(
                "entitlementArn",
                json!("arn:aws:mediaconnect:us-east-1:111122223333:entitlement:1-0000beef:shared"),
            )],
        },
        UpdateCase {
            // `EncodingName` is one of jxsv | raw | smpte291 | pcm
            // (`smithy_query(query="shape",shape="EncodingName")`).
            case: "the media streams associated with the source",
            body: json!({
                "mediaStreamSourceConfigurations": [
                    {
                        "mediaStreamName": "vid",
                        "encodingName": "jxsv",
                        "inputConfigurations": [
                            { "inputPort": 9100, "interface": { "name": "vpc-7" } }
                        ]
                    }
                ]
            }),
            expect: vec![
                (
                    "mediaStreamSourceConfigurations.0.mediaStreamName",
                    json!("vid"),
                ),
                (
                    "mediaStreamSourceConfigurations.0.encodingName",
                    json!("jxsv"),
                ),
                (
                    "mediaStreamSourceConfigurations.0.inputConfigurations.0.inputPort",
                    json!(9100),
                ),
                (
                    "mediaStreamSourceConfigurations.0.inputConfigurations.0.interface.name",
                    json!("vpc-7"),
                ),
            ],
        },
        UpdateCase {
            case: "an empty association list disassociates every media stream",
            body: json!({ "mediaStreamSourceConfigurations": [] }),
            expect: vec![("mediaStreamSourceConfigurations", Value::Null)],
        },
        UpdateCase {
            // `GatewayBridgeSource$BridgeArn` is required on the stored shape, so this row
            // names it; the next changes only its sibling.
            case: "a gateway bridge source",
            body: json!({
                "gatewayBridgeSource": {
                    "bridgeArn": "arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1"
                }
            }),
            expect: vec![(
                "gatewayBridgeSource.bridgeArn",
                json!("arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1"),
            )],
        },
        UpdateCase {
            case: "the VPC attachment of the gateway bridge source, keeping the bridge",
            body: json!({
                "gatewayBridgeSource": { "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-7" } }
            }),
            expect: vec![
                (
                    "gatewayBridgeSource.vpcInterfaceAttachment.vpcInterfaceName",
                    json!("vpc-7"),
                ),
                (
                    "gatewayBridgeSource.bridgeArn",
                    json!("arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1"),
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

/// UpdateFlowSource writes the settings the request carries onto the stored source,
/// leaves the ones it omits alone, and answers with the stored record.
#[tokio::test]
async fn update_flow_source_applies_only_the_settings_the_request_carries() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_sources(
        &emulator,
        "updates",
        "flow-update-source",
        json!([
            { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16", "ingestPort": 5001 },
            { "name": "secondary", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16", "ingestPort": 5002 }
        ]),
    )
    .await;
    let held = described_sources(&emulator, "updates", &flow_arn).await;
    let source_arn = held[0]["sourceArn"].as_str().unwrap_or_default().to_string();
    let untouched_sibling = held[1].clone();

    for UpdateCase { case, body, expect } in update_cases() {
        let before = described_sources(&emulator, case, &flow_arn).await[0].clone();
        let (status, code, response) =
            update_flow_source(&emulator, &flow_arn, &source_arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
        );
        assert_eq!(
            response["flowArn"].as_str(),
            Some(flow_arn.as_str()),
            "case {case:?}: the response names the flow the source belongs to, got {response}"
        );
        let updated = response["source"].clone();

        for (path, expected) in &expect {
            assert_eq!(
                at_path(&updated, path),
                expected,
                "case {case:?}: {path} must be {expected}, got {updated}"
            );
        }

        // Everything the request did not touch is exactly as it was — including the
        // source's identity, which no input member names.
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
            (updated["name"].as_str(), updated["sourceArn"].as_str()),
            (Some("primary"), Some(source_arn.as_str())),
            "case {case:?}: an update never moves the source's identity, got {updated}"
        );

        // The round trip: the 202 body is the stored record, and no other source of the
        // flow moved.
        let after = described_sources(&emulator, case, &flow_arn).await;
        assert_eq!(
            after[0], updated,
            "case {case:?}: DescribeFlow reports what the update answered, got {:?}",
            after[0]
        );
        assert_eq!(
            after[1], untouched_sibling,
            "case {case:?}: the flow's other source is untouched, got {:?}",
            after[1]
        );
        assert_eq!(
            after.len(),
            2,
            "case {case:?}: an update adds and removes no source, got {} sources",
            after.len()
        );
    }
}

/// The `InputIp` of an input configuration has no counterpart in the request shape and is
/// REQUIRED on the stored one, so the SERVICE assigns it: it is an address of the
/// documentation range the emulator's ingest pool draws from, and it is STORED — every
/// later read reports the same address.
#[tokio::test]
async fn an_association_carries_the_service_assigned_input_address() {
    let emulator = Emulator::new();
    let case = "the input address of an association";
    let flow_arn = create_flow_with_sources(
        &emulator,
        case,
        "flow-update-source-input",
        json!([
            { "name": "primary", "protocol": "st2110-jpegxs", "whitelistCidr": "10.3.0.0/16" }
        ]),
    )
    .await;
    let source_arn = described_sources(&emulator, case, &flow_arn).await[0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, code, response) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({
            "mediaStreamSourceConfigurations": [
                {
                    "mediaStreamName": "vid",
                    "encodingName": "jxsv",
                    "inputConfigurations": [
                        { "inputPort": 9102, "interface": { "name": "vpc-1" } },
                        { "inputPort": 9104, "interface": { "name": "vpc-1" } }
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

    let configurations = response["source"]["mediaStreamSourceConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        configurations.len(),
        2,
        "case {case:?}: one configuration per requested association, got {configurations:?}"
    );
    for input in configurations[0]["inputConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let input_ip = input["inputIp"].as_str().unwrap_or_default();
        assert!(
            input_ip.starts_with("198.51.100."),
            "case {case:?}: the service assigns an input address from its ingest pool, got {input}"
        );
    }
    // An association with no input configurations keeps none, rather than an empty list
    // the request never sent.
    assert!(
        configurations[1]["inputConfigurations"].is_null(),
        "case {case:?}: an association with no inputs carries none, got {:?}",
        configurations[1]
    );

    // Stable across reads: DescribeFlow reports the same addresses, twice.
    for read in 0..2 {
        let stored = described_sources(&emulator, case, &flow_arn).await[0]
            ["mediaStreamSourceConfigurations"]
            .clone();
        assert_eq!(
            stored,
            response["source"]["mediaStreamSourceConfigurations"],
            "case {case:?}: read {read} reports the stored associations unchanged, got {stored}"
        );
    }
}

/// One row of the refusal table: an UpdateFlowSource request the service must refuse, what
/// it carries, and the modelled error that refusal has.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    source_arn: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// The identifier the `NotFoundException` envelope must name, when the refusal is one.
    not_found_identifier: Option<String>,
}

/// A request UpdateFlowSource will not satisfy is refused with the operation's own
/// modelled error and writes nothing: the source still carries exactly the settings it
/// carried. A `SourceArn` RemoveFlowSource has detached is one such request — the
/// interaction RemoveFlowSource and UpdateFlowSource share.
#[tokio::test]
async fn update_flow_source_refuses_what_it_cannot_apply() {
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_sources(
        &emulator,
        "refusals",
        "flow-update-source-refusals",
        json!([
            { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.4.0.0/16" },
            { "name": "doomed", "protocol": "rtp", "whitelistCidr": "10.5.0.0/16" }
        ]),
    )
    .await;
    let other_flow = create_flow_with_sources(
        &emulator,
        "refusals",
        "flow-update-source-other",
        json!([{ "name": "elsewhere", "protocol": "rtp", "whitelistCidr": "10.7.0.0/16" }]),
    )
    .await;
    let held = described_sources(&emulator, "refusals", &flow_arn).await;
    let source_arn = held[0]["sourceArn"].as_str().unwrap_or_default().to_string();
    let other_source = described_sources(&emulator, "refusals", &other_flow).await[0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A source detached for real, so a mutation addressed at it resolves nothing.
    let removed = held[1]["sourceArn"].as_str().unwrap_or_default().to_string();
    let (status, _, body) = remove_flow_source(&emulator, &flow_arn, &removed).await;
    assert_eq!(
        status, 202,
        "the fixture removal must succeed, got {status}: {body}"
    );

    // The freed slot takes a source with no protocol at all, so one row can address a
    // source that carries no transport. The flow holds the failover pair again, which is
    // every source a flow can hold ("if the state is active, the flow can have one or two
    // sources", `FailoverConfig$State`).
    let (status, code, added) = crate::flow_sources::add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "bare", "whitelistCidr": "10.6.0.0/16" }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture source with no transport must be added, got {status} {code:?}: {added}"
    );
    let bare_arn = added["sources"][0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let before = described_sources(&emulator, "refusals", &flow_arn).await;
    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            source_arn: source_arn.clone(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_flow_arn()),
        },
        RefusalCase {
            case: "a source ARN this service never minted",
            flow_arn: flow_arn.clone(),
            source_arn: unknown_source_arn(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(unknown_source_arn()),
        },
        RefusalCase {
            case: "a source of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            source_arn: other_source.clone(),
            body: json!({ "description": "nope" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(other_source),
        },
        RefusalCase {
            case: "a source RemoveFlowSource has detached",
            flow_arn: flow_arn.clone(),
            source_arn: removed.clone(),
            body: json!({ "description": "dead" }),
            status: 404,
            code: "NotFoundException",
            not_found_identifier: Some(removed),
        },
        RefusalCase {
            // `Transport$Protocol` is required, so transport settings cannot be given to a
            // source that has no transport without naming the protocol.
            case: "transport settings on a source with no transport, and no protocol",
            flow_arn: flow_arn.clone(),
            source_arn: bare_arn.clone(),
            body: json!({ "minLatency": 150 }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `Encryption$RoleArn` is required, and `UpdateEncryption$RoleArn` is not.
            case: "decryption without the required role on an undecrypted source",
            flow_arn: flow_arn.clone(),
            source_arn: source_arn.clone(),
            body: json!({ "decryption": { "algorithm": "aes256" } }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `GatewayBridgeSource$BridgeArn` is required, and the request member is not.
            case: "a gateway bridge source without the required bridge ARN",
            flow_arn: flow_arn.clone(),
            source_arn: source_arn.clone(),
            body: json!({
                "gatewayBridgeSource": { "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-1" } }
            }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "a protocol outside the modelled value set",
            flow_arn: flow_arn.clone(),
            source_arn: source_arn.clone(),
            body: json!({ "protocol": "carrier-pigeon" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `NdiSourceSettings$AudioReferenceLevel` is bounded -40..0 by the model.
            case: "an NDI audio reference level above the modelled range",
            flow_arn: flow_arn.clone(),
            source_arn: source_arn.clone(),
            body: json!({ "ndiSourceSettings": { "audioReferenceLevel": 5 } }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            // `UpdateFlowSourceRequest$EntitlementArn` carries the modelled pattern
            // `^arn:.+:mediaconnect.+:entitlement:.+$`.
            case: "an entitlement ARN that is no entitlement ARN at all",
            flow_arn: flow_arn.clone(),
            source_arn: source_arn.clone(),
            body: json!({ "entitlementArn": "not-an-entitlement-arn" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
        RefusalCase {
            case: "a source label that is no source ARN at all",
            flow_arn: flow_arn.clone(),
            source_arn: "not-a-source-arn".to_string(),
            body: json!({ "description": "nope" }),
            status: 400,
            code: "BadRequestException",
            not_found_identifier: None,
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        source_arn: target_source,
        body,
        status: expected_status,
        code: expected_code,
        not_found_identifier,
    } in rows
    {
        let (status, code, response) =
            update_flow_source(&emulator, &target_flow, &target_source, body).await;
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

        let after = described_sources(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update writes nothing, got {after:?}"
        );
    }
}

/// A source added by AddFlowSources is updated exactly like one CreateFlow stored, and
/// the ARN the add minted is the one the update addresses: the full round trip the
/// behavioral suite drives — create, add, update, remove, update again.
#[tokio::test]
async fn an_added_source_is_updated_then_gone_once_removed() {
    let emulator = Emulator::new();
    let case = "an added source round trip";
    let flow_arn = crate::flow_sources::create_flow_with_sources_and_failover(
        &emulator,
        case,
        "flow-add-then-update",
        json!([{ "name": "source1", "protocol": "rtp", "whitelistCidr": "0.0.0.0/0", "ingestPort": 5001 }]),
        Some(crate::flow_sources::failover_active()),
    )
    .await;
    let (status, code, added) = crate::flow_sources::add_flow_sources(
        &emulator,
        &flow_arn,
        json!([{ "name": "source2", "protocol": "rtp", "whitelistCidr": "0.0.0.0/0", "ingestPort": 5002 }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the source under test must be added, got {status} {code:?}: {added}"
    );
    let source_arn = added["sources"][0]["sourceArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        source_arn.starts_with(&format!("arn:aws:mediaconnect:{REGION}:")),
        "case {case:?}: the add mints the ARN the update addresses, got {source_arn:?}"
    );

    // The update applies to the added source and nothing else.
    let (status, code, response) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({ "description": "the backup feed", "whitelistCidr": "10.8.0.0/16" }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: expected 202 with no error, got {status} {code:?}: {response}"
    );
    assert_eq!(
        (
            response["source"]["description"].as_str(),
            response["source"]["whitelistCidr"].as_str(),
            response["source"]["sourceArn"].as_str()
        ),
        (
            Some("the backup feed"),
            Some("10.8.0.0/16"),
            Some(source_arn.as_str())
        ),
        "case {case:?}: the update writes the settings it carries onto the added source, got \
         {response}"
    );
    let sources = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        sources[1], response["source"],
        "case {case:?}: DescribeFlow reports what the update answered, got {:?}",
        sources[1]
    );

    // Once the source is detached, its ARN resolves nothing: the mutation is refused with
    // the modelled 404 rather than succeeding or reporting a server fault.
    let (status, _, removed) = remove_flow_source(&emulator, &flow_arn, &source_arn).await;
    assert_eq!(
        status, 202,
        "case {case:?}: the removal must succeed, got {status}: {removed}"
    );
    let (status, code, response) = update_flow_source(
        &emulator,
        &flow_arn,
        &source_arn,
        json!({ "description": "dead" }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref(), response["message"].as_str()),
        (
            404,
            Some("NotFoundException"),
            Some(mc::not_found_message(&source_arn).as_str())
        ),
        "case {case:?}: updating a detached source is refused as not found, got {status} \
         {code:?}: {response}"
    );
    let after = described_sources(&emulator, case, &flow_arn).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: the refused update neither restores nor adds a source, got {after:?}"
    );
}

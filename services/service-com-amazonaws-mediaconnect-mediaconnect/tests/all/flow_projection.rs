// SPDX-License-Identifier: CC0-1.0

//! The projection of a stored flow into the modelled `Flow` / `Source` / `Output`
//! shapes, end to end over HTTP.
//!
//! Three rules of the class are asserted here, each over a TABLE of inputs rather than
//! one recorded request:
//!
//! * a transport member a source request carries (`maxBitrate` and its siblings) is
//!   persisted and reported on `flow.source.transport` and on every
//!   `flow.sources[*].transport`, and a source on a protocol the smoothing bitrate
//!   applies to that names none reports the bitrate the service assigns it;
//! * `routerIntegrationState`, which the request shapes declare optional and the service
//!   resolves, is reported on every source and every output, with the `DISABLED` default
//!   when the request omits it;
//! * `flow.mediaStreams` is reported as a present-but-empty list when the flow holds no
//!   media stream, never omitted;
//! * a flow's `sourceFailoverConfig` reports the recovery window the request named, and
//!   the one the service assigns when it named none.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what these tests assert is what a
//! MediaConnect client sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;

/// The smoothing max bitrate (in bps) the service assigns a source that names none, and
/// the recovery window (in ms) it assigns a failover config that names none — the values
/// AWS answers with, written out here rather than read from the crate, so a change to the
/// emulator's constants cannot quietly move what these tests assert.
const ASSIGNED_SOURCE_MAX_BITRATE_BPS: i64 = 160_000_000;
/// See [`ASSIGNED_SOURCE_MAX_BITRATE_BPS`].
const ASSIGNED_RECOVERY_WINDOW_MS: i64 = 200;

/// The protocols `Transport$MaxBitrate` applies to: "the smoothing max bitrate (in bps)
/// for RIST, RTP, and RTP-FEC streams"
/// (`smithy_query(query="doc",shape="Transport",member="MaxBitrate")`), as the `Protocol`
/// enum spells them. Named from that sentence, not from the crate, so the catalog case
/// below checks the rule rather than the code's own idea of it.
const SMOOTHING_BITRATE_PROTOCOLS: [&str; 3] = ["rist", "rtp", "rtp-fec"];

/// Every value of the modelled `Protocol` enum
/// (`smithy_query(query="shape",shape="Protocol")`), taken from the GENERATED catalog so
/// a model update cannot leave the assertion below stale.
use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::PROTOCOL_VALUES;

/// The two `State` enum values (`smithy_query(query="shape",shape="State")`: ENABLED |
/// DISABLED), which is what `Source$RouterIntegrationState` and
/// `Output$RouterIntegrationState` target.
const STATE_ENABLED: &str = "ENABLED";
/// See [`STATE_ENABLED`].
const STATE_DISABLED: &str = "DISABLED";

/// The `Protocol` value naming the NDI® transport
/// (`smithy_query(query="shape",shape="Protocol")`: … | ndi-speed-hq), and a `FlowSize`
/// that carries it (`smithy_query(query="shape",shape="FlowSize")`: MEDIUM | LARGE |
/// LARGE_4X). A flow whose source or output is NDI exists only at the larger size — see
/// `crate::flow_ndi_sizing`, which asserts that rule; here it only keeps the catalog row
/// for the NDI protocol describing a flow AWS would create.
const NDI_PROTOCOL: &str = "ndi-speed-hq";
/// See [`NDI_PROTOCOL`].
const NDI_CAPABLE_FLOW_SIZE: &str = "LARGE";

/// Create a flow and return its 201 body, asserting the create itself succeeded.
async fn create(emulator: &Emulator, case: &str, body: Value) -> Value {
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: CreateFlow must answer 201 with no error, got {status} {code:?}: \
         {created}"
    );
    created
}

/// Read a flow back and return its 200 body, asserting the read itself succeeded.
async fn describe(emulator: &Emulator, case: &str, flow_arn: &str) -> Value {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200 with no error, got {status} {code:?}: \
         {described}"
    );
    described
}

/// The ARN of the flow in a CreateFlow 201 body.
fn flow_arn(created: &Value) -> String {
    created["flow"]["flowArn"]
        .as_str()
        .expect("a created flow carries its ARN")
        .to_string()
}

/// One row of the source-transport table: the `SetSourceRequest` members a source
/// carries, and the `transport` object the stored source must report for them.
struct TransportCase {
    case: &'static str,
    /// The source as the request states it.
    source: Value,
    /// The whole `transport` object the projection must report for it.
    transport: Value,
}

/// The transport members of `SetSourceRequest` span the integer range the model allows
/// (`Transport$MaxBitrate` and its siblings target `Integer`), so the rows differ along
/// value, count and which members are present — including a row that carries none.
fn transport_cases() -> Vec<TransportCase> {
    vec![
        TransportCase {
            case: "rtp source with only a max bitrate",
            source: json!({
                "name": "src-rtp",
                "protocol": "rtp",
                "whitelistCidr": "10.24.43.0/24",
                "maxBitrate": 160_000_000,
                "ingestPort": 5090
            }),
            transport: json!({ "protocol": "rtp", "maxBitrate": 160_000_000 }),
        },
        TransportCase {
            case: "rist source with a max bitrate of one bit per second",
            source: json!({
                "name": "src-rist",
                "protocol": "rist",
                "whitelistCidr": "0.0.0.0/0",
                "maxBitrate": 1
            }),
            transport: json!({ "protocol": "rist", "maxBitrate": 1 }),
        },
        TransportCase {
            case: "rtp-fec source with the largest Integer max bitrate",
            source: json!({
                "name": "src-rtp-fec",
                "protocol": "rtp-fec",
                "whitelistCidr": "192.0.2.0/24",
                "maxBitrate": i32::MAX
            }),
            transport: json!({ "protocol": "rtp-fec", "maxBitrate": i32::MAX }),
        },
        TransportCase {
            case: "udp source that carries no transport member at all",
            source: json!({
                "name": "src-udp",
                "protocol": "udp",
                "whitelistCidr": "198.51.100.0/24"
            }),
            // `Transport$MaxBitrate` is the smoothing bitrate "for RIST, RTP, and
            // RTP-FEC streams", and udp is none of those, so nothing but the required
            // protocol is reported.
            transport: json!({ "protocol": "udp" }),
        },
        TransportCase {
            case: "st2110-jpegxs source outside the smoothing-bitrate protocols",
            source: json!({
                "name": "src-st2110",
                "protocol": "st2110-jpegxs",
                "whitelistCidr": "198.51.100.0/24",
                "maxSyncBuffer": 1_000
            }),
            transport: json!({ "protocol": "st2110-jpegxs", "maxSyncBuffer": 1_000 }),
        },
        TransportCase {
            case: "zixi-push source with a max bitrate beside three other transport members",
            source: json!({
                "name": "src-zixi",
                "protocol": "zixi-push",
                "whitelistCidr": "10.1.0.0/16",
                "maxBitrate": 80_000_000,
                "maxLatency": 2_000,
                "maxSyncBuffer": 1_000,
                "streamId": "stream-7"
            }),
            transport: json!({
                "protocol": "zixi-push",
                "maxBitrate": 80_000_000,
                "maxLatency": 2_000,
                "maxSyncBuffer": 1_000,
                "streamId": "stream-7"
            }),
        },
        TransportCase {
            case: "srt-caller source with a max bitrate and a listener endpoint",
            source: json!({
                "name": "src-srt",
                "protocol": "srt-caller",
                "whitelistCidr": "203.0.113.0/24",
                "maxBitrate": 20_000_000,
                "minLatency": 120,
                "sourceListenerAddress": "203.0.113.7",
                "sourceListenerPort": 9001
            }),
            transport: json!({
                "protocol": "srt-caller",
                "maxBitrate": 20_000_000,
                "minLatency": 120,
                "sourceListenerAddress": "203.0.113.7",
                "sourceListenerPort": 9001
            }),
        },
    ]
}

/// Assert `transport` reports exactly the members `expected` names, and no other member
/// of the modelled `Transport` shape.
fn assert_transport(case: &str, where_: &str, transport: &Value, expected: &Value) {
    let expected_members = expected
        .as_object()
        .expect("a transport expectation is an object");
    for (member, value) in expected_members {
        assert_eq!(
            &transport[member], value,
            "case {case:?}: {where_} transport member {member:?} must be {value}, got {:?}",
            transport[member]
        );
    }
    let reported = transport
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: {where_} must report a transport object"));
    for member in reported.keys() {
        assert!(
            expected_members.contains_key(member),
            "case {case:?}: {where_} reports transport member {member:?} nothing configured, \
             got {:?}",
            reported[member]
        );
    }
}

/// Every transport member a source request carries is persisted and reported on
/// `flow.source.transport` and on each `flow.sources[*].transport`, by CreateFlow and by
/// the DescribeFlow that reads the flow back.
#[tokio::test]
async fn source_transport_members_round_trip_on_every_source_projection() {
    let emulator = Emulator::new();

    for (index, TransportCase {
        case,
        source,
        transport,
    }) in transport_cases().into_iter().enumerate()
    {
        let created = create(
            &emulator,
            case,
            json!({ "name": format!("transport-flow-{index}"), "source": source }),
        )
        .await;
        let arn = flow_arn(&created);
        let described = describe(&emulator, case, &arn).await;

        for (body_name, flow) in [("CreateFlow", &created["flow"]), ("DescribeFlow", &described["flow"])] {
            assert_transport(
                case,
                &format!("{body_name} flow.source"),
                &flow["source"]["transport"],
                &transport,
            );
            let sources = flow["sources"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {body_name} must report flow.sources"));
            assert_eq!(
                sources.len(),
                1,
                "case {case:?}: {body_name} reports one source, got {:?}",
                flow["sources"]
            );
            assert_transport(
                case,
                &format!("{body_name} flow.sources[0]"),
                &sources[0]["transport"],
                &transport,
            );
        }
    }
}

/// A source whose request names no `maxBitrate` reports the bitrate the service assigns
/// it on exactly the protocols the smoothing bitrate applies to, and no bitrate at all on
/// the others — on `flow.source` and on `flow.sources[*]`, at create and at read.
///
/// The rows are the whole `Protocol` catalog, so the rule is asserted over every value
/// the model declares and not only the `rtp` the recordings used.
#[tokio::test]
async fn a_source_that_names_no_max_bitrate_reports_the_assigned_one() {
    let emulator = Emulator::new();

    for (index, protocol) in PROTOCOL_VALUES.iter().copied().enumerate() {
        let case = protocol;
        let expected = if SMOOTHING_BITRATE_PROTOCOLS.contains(&protocol) {
            json!(ASSIGNED_SOURCE_MAX_BITRATE_BPS)
        } else {
            Value::Null
        };
        let mut body = json!({
            "name": format!("assigned-bitrate-flow-{index}"),
            "source": {
                "name": "src",
                "protocol": protocol,
                "whitelistCidr": "10.0.0.0/16"
            }
        });
        // A flow whose source uses the NDI® transport exists only at an NDI-capable size:
        // "set this optional parameter to LARGE if you want to enable NDI sources or
        // outputs on the flow"
        // (`smithy_query(query="doc",shape="CreateFlowRequest",member="FlowSize")`), so the
        // NDI row of this catalog asks for that size.
        if protocol == NDI_PROTOCOL {
            body["flowSize"] = json!(NDI_CAPABLE_FLOW_SIZE);
        }
        let created = create(&emulator, case, body).await;
        let arn = flow_arn(&created);
        let described = describe(&emulator, case, &arn).await;

        for (body_name, flow) in [
            ("CreateFlow", &created["flow"]),
            ("DescribeFlow", &described["flow"]),
        ] {
            assert_eq!(
                flow["source"]["transport"]["maxBitrate"], expected,
                "case {case:?}: {body_name} flow.source.transport.maxBitrate must be \
                 {expected}, got {:?}",
                flow["source"]["transport"]["maxBitrate"]
            );
            assert_eq!(
                flow["sources"][0]["transport"]["maxBitrate"], expected,
                "case {case:?}: {body_name} flow.sources[0].transport.maxBitrate must be \
                 {expected}, got {:?}",
                flow["sources"][0]["transport"]["maxBitrate"]
            );
        }
    }
}

/// A failover pair: both entries of `flow.sources` carry their OWN transport, so the
/// list is not the first source repeated.
#[tokio::test]
async fn each_source_of_a_failover_pair_keeps_its_own_transport() {
    let emulator = Emulator::new();
    let case = "failover pair with two different max bitrates";

    let created = create(
        &emulator,
        case,
        json!({
            "name": "transport-flow-pair",
            // A pair of sources needs source failover ACTIVE: "if the state is active,
            // the flow can have one or two sources" (`FailoverConfig$State`).
            "sourceFailoverConfig": { "state": STATE_ENABLED },
            "sources": [
                { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16", "maxBitrate": 40_000_000 },
                { "name": "backup", "protocol": "rist", "whitelistCidr": "10.2.0.0/16", "maxBitrate": 70_000_000 }
            ]
        }),
    )
    .await;
    let arn = flow_arn(&created);
    let described = describe(&emulator, case, &arn).await;

    for (body_name, flow) in [
        ("CreateFlow", &created["flow"]),
        ("DescribeFlow", &described["flow"]),
    ] {
        assert_transport(
            case,
            &format!("{body_name} flow.source"),
            &flow["source"]["transport"],
            &json!({ "protocol": "rtp", "maxBitrate": 40_000_000 }),
        );
        let sources = flow["sources"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: {body_name} must report flow.sources"));
        assert_eq!(
            sources.len(),
            2,
            "case {case:?}: {body_name} reports both sources, got {:?}",
            flow["sources"]
        );
        assert_transport(
            case,
            &format!("{body_name} flow.sources[0]"),
            &sources[0]["transport"],
            &json!({ "protocol": "rtp", "maxBitrate": 40_000_000 }),
        );
        assert_transport(
            case,
            &format!("{body_name} flow.sources[1]"),
            &sources[1]["transport"],
            &json!({ "protocol": "rist", "maxBitrate": 70_000_000 }),
        );
    }
}

/// One row of the router-integration table: what the request states for
/// `routerIntegrationState`, and the state the projection must report.
struct RouterIntegrationCase {
    case: &'static str,
    /// The member as the request states it, or `None` when the request omits it.
    requested: Option<&'static str>,
    expected: &'static str,
}

/// `SetSourceRequest$RouterIntegrationState` and
/// `AddOutputRequest$RouterIntegrationState` both target the `State` enum, so the rows
/// are its two values plus the absent case the service resolves.
fn router_integration_cases() -> Vec<RouterIntegrationCase> {
    vec![
        RouterIntegrationCase {
            case: "request omits router integration",
            requested: None,
            expected: STATE_DISABLED,
        },
        RouterIntegrationCase {
            case: "request disables router integration",
            requested: Some(STATE_DISABLED),
            expected: STATE_DISABLED,
        },
        RouterIntegrationCase {
            case: "request enables router integration",
            requested: Some(STATE_ENABLED),
            expected: STATE_ENABLED,
        },
    ]
}

/// Put `routerIntegrationState` on `object` when the row states one.
fn with_router_integration(mut object: Value, requested: Option<&str>) -> Value {
    if let Some(state) = requested {
        object["routerIntegrationState"] = json!(state);
    }
    object
}

/// Every source and every output a flow reports carries `routerIntegrationState`: the
/// state the request asked for, or the `DISABLED` default when it asked for none. The
/// member is reported by CreateFlow, by AddFlowOutputs and by DescribeFlow alike.
#[tokio::test]
async fn router_integration_state_is_reported_on_every_source_and_output() {
    let emulator = Emulator::new();

    for (index, RouterIntegrationCase {
        case,
        requested,
        expected,
    }) in router_integration_cases().into_iter().enumerate()
    {
        let created = create(
            &emulator,
            case,
            json!({
                "name": format!("router-flow-{index}"),
                "sourceFailoverConfig": { "state": STATE_ENABLED },
                "sources": [
                    with_router_integration(
                        json!({ "name": "primary", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }),
                        requested,
                    ),
                    with_router_integration(
                        json!({ "name": "backup", "protocol": "rtp", "whitelistCidr": "10.4.0.0/16" }),
                        requested,
                    )
                ],
                "outputs": [
                    with_router_integration(
                        json!({ "name": "out-created", "protocol": "rtp", "destination": "198.51.100.9", "port": 5100 }),
                        requested,
                    )
                ]
            }),
        )
        .await;
        let arn = flow_arn(&created);

        // AddFlowOutputs materialises an output through the same path, so its 201 body
        // reports the member too.
        let (status, code, added) = emulator
            .send(
                "POST",
                &format!("/v1/flows/{arn}/outputs"),
                Some(json!({
                    "outputs": [with_router_integration(
                        json!({ "name": "out-added", "protocol": "rist", "destination": "198.51.100.10", "port": 5102 }),
                        requested,
                    )]
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: AddFlowOutputs must answer 201 with no error, got {status} \
             {code:?}: {added}"
        );
        for (position, output) in added["outputs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: AddFlowOutputs must report its outputs"))
            .iter()
            .enumerate()
        {
            assert_eq!(
                output["routerIntegrationState"],
                json!(expected),
                "case {case:?}: AddFlowOutputs outputs[{position}].routerIntegrationState \
                 must be {expected:?}, got {:?}",
                output["routerIntegrationState"]
            );
        }

        let described = describe(&emulator, case, &arn).await;
        for (body_name, flow) in [
            ("CreateFlow", &created["flow"]),
            ("DescribeFlow", &described["flow"]),
        ] {
            assert_eq!(
                flow["source"]["routerIntegrationState"],
                json!(expected),
                "case {case:?}: {body_name} flow.source.routerIntegrationState must be \
                 {expected:?}, got {:?}",
                flow["source"]["routerIntegrationState"]
            );
            for (position, source) in flow["sources"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {body_name} must report flow.sources"))
                .iter()
                .enumerate()
            {
                assert_eq!(
                    source["routerIntegrationState"],
                    json!(expected),
                    "case {case:?}: {body_name} flow.sources[{position}]\
                     .routerIntegrationState must be {expected:?}, got {:?}",
                    source["routerIntegrationState"]
                );
            }
            for (position, output) in flow["outputs"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: {body_name} must report flow.outputs"))
                .iter()
                .enumerate()
            {
                assert_eq!(
                    output["routerIntegrationState"],
                    json!(expected),
                    "case {case:?}: {body_name} flow.outputs[{position}]\
                     .routerIntegrationState must be {expected:?}, got {:?}",
                    output["routerIntegrationState"]
                );
            }
        }
        // DescribeFlow reports the output AddFlowOutputs appended, with the same state.
        assert_eq!(
            described["flow"]["outputs"].as_array().map(Vec::len),
            Some(2),
            "case {case:?}: DescribeFlow reports both outputs, got {:?}",
            described["flow"]["outputs"]
        );
    }
}

/// One row of the media-streams table: the `MediaStreams` a flow is created with, and
/// how many entries `flow.mediaStreams` must then report.
struct MediaStreamsCase {
    case: &'static str,
    /// The `mediaStreams` member of the create request, or `None` when it is absent.
    requested: Option<Value>,
    count: usize,
}

/// `Flow$MediaStreams` is a list that is always reported, so the rows span the empty
/// list (absent member and explicitly empty) and lists of one and two entries.
fn media_streams_cases() -> Vec<MediaStreamsCase> {
    vec![
        MediaStreamsCase {
            case: "create request omits mediaStreams",
            requested: None,
            count: 0,
        },
        MediaStreamsCase {
            case: "create request carries an empty mediaStreams list",
            requested: Some(json!([])),
            count: 0,
        },
        MediaStreamsCase {
            case: "one video media stream",
            requested: Some(json!([
                {
                    "mediaStreamName": "video-1",
                    "mediaStreamId": 1,
                    "mediaStreamType": "video",
                    "videoFormat": "2160p"
                }
            ])),
            count: 1,
        },
        MediaStreamsCase {
            case: "two media streams of different types",
            requested: Some(json!([
                {
                    "mediaStreamName": "video-1",
                    "mediaStreamId": 1,
                    "mediaStreamType": "video",
                    "videoFormat": "1080p"
                },
                {
                    "mediaStreamName": "audio-1",
                    "mediaStreamId": 2,
                    "mediaStreamType": "audio",
                    "attributes": { "fmtp": {}, "lang": "en" },
                    "clockRate": 48_000
                }
            ])),
            count: 2,
        },
    ]
}

/// `flow.mediaStreams` is always reported as a list — present and EMPTY for a flow with
/// no media stream — on the CreateFlow 201 body, the DescribeFlow 200 body and the
/// UpdateFlow 202 body alike.
#[tokio::test]
async fn flow_media_streams_is_a_present_list_even_when_the_flow_has_none() {
    let emulator = Emulator::new();

    for (index, MediaStreamsCase {
        case,
        requested,
        count,
    }) in media_streams_cases().into_iter().enumerate()
    {
        let mut body = json!({
            "name": format!("media-streams-flow-{index}"),
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        });
        if let Some(streams) = requested {
            body["mediaStreams"] = streams;
        }
        let created = create(&emulator, case, body).await;
        let arn = flow_arn(&created);
        let described = describe(&emulator, case, &arn).await;

        // UpdateFlow projects the same flow, so its 202 body reports the member too.
        let (status, code, updated) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{arn}"),
                Some(json!({ "flowSize": "MEDIUM" })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateFlow must answer 202 with no error, got {status} {code:?}: \
             {updated}"
        );

        for (body_name, flow) in [
            ("CreateFlow", &created["flow"]),
            ("DescribeFlow", &described["flow"]),
            ("UpdateFlow", &updated["flow"]),
        ] {
            let streams = flow["mediaStreams"].as_array().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {body_name} must report flow.mediaStreams as a list, got \
                     {:?}",
                    flow["mediaStreams"]
                )
            });
            assert_eq!(
                streams.len(),
                count,
                "case {case:?}: {body_name} flow.mediaStreams holds {count} entries, got {:?}",
                flow["mediaStreams"]
            );
        }
    }
}

/// One row of the failover-merge table: the `SourceFailoverConfig` an UpdateFlow request
/// carries, and the whole `sourceFailoverConfig` the flow must then report.
struct FailoverCase {
    case: &'static str,
    update: Value,
    expected: Value,
}

/// `UpdateFailoverConfig` mirrors `FailoverConfig` member for member and every member is
/// optional, so the rows walk the members one at a time — each one on its own, then all
/// of them together — over a flow that starts with a failover config already set. The
/// window the service assigned at create is what the rows that do not name one keep.
fn failover_cases() -> Vec<FailoverCase> {
    vec![
        FailoverCase {
            case: "only the recovery window",
            update: json!({ "recoveryWindow": 3_000 }),
            expected: json!({
                "state": STATE_ENABLED,
                "failoverMode": "FAILOVER",
                "recoveryWindow": 3_000
            }),
        },
        FailoverCase {
            case: "only the failover mode, so the recovery window is kept",
            update: json!({ "failoverMode": "MERGE" }),
            expected: json!({
                "state": STATE_ENABLED,
                "failoverMode": "MERGE",
                "recoveryWindow": 3_000
            }),
        },
        FailoverCase {
            case: "only the source priority",
            update: json!({ "sourcePriority": { "primarySource": "backup" } }),
            expected: json!({
                "state": STATE_ENABLED,
                "failoverMode": "MERGE",
                "recoveryWindow": 3_000,
                "sourcePriority": { "primarySource": "backup" }
            }),
        },
        FailoverCase {
            case: "a one-millisecond recovery window replaces the stored one",
            update: json!({ "recoveryWindow": 1 }),
            expected: json!({
                "state": STATE_ENABLED,
                "failoverMode": "MERGE",
                "recoveryWindow": 1,
                "sourcePriority": { "primarySource": "backup" }
            }),
        },
        FailoverCase {
            case: "every member of the wrapper at once",
            update: json!({
                "state": STATE_ENABLED,
                "failoverMode": "FAILOVER",
                "recoveryWindow": 15_000,
                "sourcePriority": { "primarySource": "primary" }
            }),
            expected: json!({
                "state": STATE_ENABLED,
                "failoverMode": "FAILOVER",
                "recoveryWindow": 15_000,
                "sourcePriority": { "primarySource": "primary" }
            }),
        },
    ]
}

/// Each member of `UpdateFlow`'s failover wrapper — `recoveryWindow` included — is
/// merged into the flow's stored `sourceFailoverConfig`, reported on the 202 body and
/// still reported by the DescribeFlow that follows. A failover config that names no
/// recovery window reports the one the service assigns, whether CreateFlow or UpdateFlow
/// set it.
#[tokio::test]
async fn update_flow_merges_and_persists_every_failover_member() {
    let emulator = Emulator::new();
    let setup = "a create that sets failover but names no recovery window";
    let created = create(
        &emulator,
        setup,
        json!({
            "name": "failover-merge-flow",
            "sources": [
                { "name": "primary", "protocol": "rtp", "whitelistCidr": "10.1.0.0/16" },
                { "name": "backup", "protocol": "rtp", "whitelistCidr": "10.2.0.0/16" }
            ],
            "sourceFailoverConfig": { "state": STATE_ENABLED, "failoverMode": "FAILOVER" }
        }),
    )
    .await;
    let arn = flow_arn(&created);
    assert_eq!(
        created["flow"]["sourceFailoverConfig"],
        json!({
            "state": STATE_ENABLED,
            "failoverMode": "FAILOVER",
            "recoveryWindow": ASSIGNED_RECOVERY_WINDOW_MS
        }),
        "case {setup:?}: the created flow reports the assigned recovery window, got {:?}",
        created["flow"]["sourceFailoverConfig"]
    );

    for FailoverCase {
        case,
        update,
        expected,
    } in failover_cases()
    {
        let (status, code, updated) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{arn}"),
                Some(json!({ "sourceFailoverConfig": update })),
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: UpdateFlow must answer 202 with no error, got {status} {code:?}: \
             {updated}"
        );
        assert_eq!(
            updated["flow"]["sourceFailoverConfig"], expected,
            "case {case:?}: the UpdateFlow body reports the merged failover config, got {:?}",
            updated["flow"]["sourceFailoverConfig"]
        );

        let described = describe(&emulator, case, &arn).await;
        assert_eq!(
            described["flow"]["sourceFailoverConfig"], expected,
            "case {case:?}: DescribeFlow still reports the merged failover config, got {:?}",
            described["flow"]["sourceFailoverConfig"]
        );
    }
}

/// A flow created without any failover config reports NONE — the service invents no
/// config for a flow that has none — and the first UpdateFlow that sets one without
/// naming a recovery window gets the window the service assigns, on the 202 body and on
/// every read afterwards.
#[tokio::test]
async fn an_update_that_sets_failover_without_a_window_gets_the_assigned_one() {
    let emulator = Emulator::new();
    let case = "a flow whose failover config is set for the first time by UpdateFlow";

    let created = create(
        &emulator,
        case,
        json!({
            "name": "failover-from-update-flow",
            "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
        }),
    )
    .await;
    let arn = flow_arn(&created);
    assert_eq!(
        created["flow"]["sourceFailoverConfig"],
        Value::Null,
        "case {case:?}: a flow created with no failover config reports none, got {:?}",
        created["flow"]["sourceFailoverConfig"]
    );

    let expected = json!({
        "state": STATE_ENABLED,
        "failoverMode": "MERGE",
        "recoveryWindow": ASSIGNED_RECOVERY_WINDOW_MS
    });
    let (status, code, updated) = emulator
        .send(
            "PUT",
            &format!("/v1/flows/{arn}"),
            Some(json!({
                "sourceFailoverConfig": { "failoverMode": "MERGE", "state": STATE_ENABLED }
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: UpdateFlow must answer 202 with no error, got {status} {code:?}: \
         {updated}"
    );
    assert_eq!(
        updated["flow"]["sourceFailoverConfig"], expected,
        "case {case:?}: the UpdateFlow body reports the assigned recovery window, got {:?}",
        updated["flow"]["sourceFailoverConfig"]
    );

    let described = describe(&emulator, case, &arn).await;
    assert_eq!(
        described["flow"]["sourceFailoverConfig"], expected,
        "case {case:?}: DescribeFlow still reports the assigned recovery window, got {:?}",
        described["flow"]["sourceFailoverConfig"]
    );
}

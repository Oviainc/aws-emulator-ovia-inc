// SPDX-License-Identifier: CC0-1.0

//! Router outputs end to end over HTTP: CreateRouterOutput -> BatchGetRouterOutput.
//!
//! The subject is BatchGetRouterOutput: it must report, for the ARNs one request names,
//! the router outputs the STORE HOLDS — computed from the stored record, each requested
//! resource once — and one `Errors` entry for every named ARN the store holds no output
//! for, rather than a value fixed in the handler. Its write side (CreateRouterOutput) is
//! implemented in the same change, because a reader with nothing to read proves nothing.
//!
//! The cases below vary what those rules generalise over: which configuration union
//! variant (and so which output type) the output carries, which of the five output
//! protocols a STANDARD one is configured with, every tier in the published catalog at its
//! bitrate ceiling and one bit past it, whether the request names a Region / Availability
//! Zone / tags / maintenance / fabric configuration or lets the service derive them, and —
//! for the batch read — how many ARNs the request names, in what order, and whether each
//! resolves, repeats or resolves nothing.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_network_interfaces::{create_network_interface, public_configuration};

/// A `RouterNetworkInterfaceArn` matching the modelled pattern
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`)
/// that no create minted.
///
/// A create REFUSES a configuration naming it (see
/// `tests/all/router_output_configuration_targets.rs`), so the only thing here that uses it
/// is [`seed_router_outputs`], which writes straight into the store rather than over the
/// wire.
const NETWORK_INTERFACE_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:a1b2c3d4e5f6";
/// A `FlowArn` matching the modelled pattern (see `flow_lifecycle`) that no create minted —
/// an ARN of this service that is not a router output ARN, which the batch read's modelled
/// `Arns` pattern refuses.
///
/// `pub(crate)` because `tests/all/update_router_network_interface.rs` addresses the same
/// ARN for the same reason.
pub(crate) const FLOW_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000a:ro-target";
/// A `MediaLiveInputArn` matching the modelled pattern
/// `^arn:(aws[a-zA-Z-]*):medialive:[a-z0-9-]+:[0-9]{12}:input:[a-zA-Z0-9]+$`. See
/// [`FLOW_ARN`] for why it is `pub(crate)`.
pub(crate) const MEDIALIVE_INPUT_ARN: &str =
    "arn:aws:medialive:us-east-1:000000000000:input:mlinput1";
/// A `SecretArn` matching the modelled pattern. See [`FLOW_ARN`] for why it is
/// `pub(crate)`.
pub(crate) const SECRET_ARN: &str =
    "arn:aws:secretsmanager:us-east-1:000000000000:secret:router-key";
/// A `RoleArn` matching the modelled pattern. See [`FLOW_ARN`] for why it is `pub(crate)`.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/router-output-role";

/// An ARN satisfying the modelled `RouterOutputArn` pattern that no create minted, so no
/// store holds a router output for it.
pub(crate) const UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:ffffffffffff";
/// A second one, for the cases that name more than one absent output.
const OTHER_UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:eeeeeeeeeeee";

/// Send one CreateRouterOutput request.
///
/// `pub(crate)` for the same reason as [`FLOW_ARN`]: the suites that update, read and
/// delete a router output create what they act on through this one helper.
pub(crate) async fn create_router_output(
    emulator: &Emulator,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator.send("POST", "/v1/routerOutput", Some(body)).await
}

/// Mint one router network interface over the wire and answer the ARN the service gave it.
///
/// A STANDARD router output egresses FROM a router network interface of this account, which
/// the create resolves against the store, so every suite that creates such an output mints
/// its interface first. The request goes through the POST helper and the configuration
/// fixture `router_network_interfaces` owns — the suite that pins
/// CreateRouterNetworkInterface's own rules — so there is one spelling of that create.
pub(crate) async fn minted_network_interface(emulator: &Emulator, name: &str) -> String {
    let body = json!({
        "name": name,
        "configuration": public_configuration(0),
    });
    let (status, code, created) = create_network_interface(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateRouterNetworkInterface expected 201 for {name:?}, got {status} {code:?}: \
         {created}"
    );
    created["routerNetworkInterface"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the minted interface carries its ARN: {created}"))
        .to_string()
}

/// Mint one flow with one source over the wire and answer `(FlowArn, FlowSourceArn)`.
///
/// A MEDIACONNECT FLOW router output delivers INTO a flow source of this account, which the
/// create resolves against the store, so every suite that creates such an output mints the
/// flow first — through this one helper, for the same reason as
/// [`minted_network_interface`].
pub(crate) async fn minted_flow_source(emulator: &Emulator, name: &str) -> (String, String) {
    let body = json!({
        "name": name,
        "source": { "name": format!("{name}-src"), "protocol": "rtp",
            "whitelistCidr": "10.0.0.0/16" },
    });
    let (status, code, created) = emulator.create_flow(body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateFlow expected 201 for {name:?}, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the minted flow carries its ARN: {created}"))
        .to_string();
    let source_arn = created["flow"]["source"]["sourceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the minted flow's source carries its ARN: {created}"))
        .to_string();
    (flow_arn, source_arn)
}

/// Every resource of THIS service a router output configuration can name, minted over the
/// wire on one emulator: the router network interface a STANDARD output egresses from, and
/// the flow and flow source a MEDIACONNECT FLOW output delivers into.
///
/// `pub(crate)` because every suite that creates a router output needs them, and they are
/// per-emulator (an interface one emulator minted is not in another's store).
pub(crate) struct ConfigurationTargets {
    pub(crate) interface_arn: String,
    pub(crate) flow_arn: String,
    pub(crate) flow_source_arn: String,
}

/// Mint the whole set [`ConfigurationTargets`] describes, naming each resource after
/// `prefix` so a suite's records are its own.
pub(crate) async fn minted_configuration_targets(
    emulator: &Emulator,
    prefix: &str,
) -> ConfigurationTargets {
    let interface_arn = minted_network_interface(emulator, &format!("rni-{prefix}")).await;
    let (flow_arn, flow_source_arn) =
        minted_flow_source(emulator, &format!("flow-{prefix}")).await;
    ConfigurationTargets {
        interface_arn,
        flow_arn,
        flow_source_arn,
    }
}

/// BatchGetRouterOutput binds its `Arns` member to the `arns` query parameter, one
/// parameter per ARN (`smithy_query(query="operation",operation="BatchGetRouterOutput")`:
/// `GET /v1/routerOutputs`, `Arns` `@httpQuery("arns")`).
pub(crate) async fn batch_get_router_outputs(
    emulator: &Emulator,
    arns: &[&str],
) -> (u16, Option<String>, Value) {
    let query = arns
        .iter()
        .map(|arn| format!("arns={arn}"))
        .collect::<Vec<String>>()
        .join("&");
    let uri = if query.is_empty() {
        "/v1/routerOutputs".to_string()
    } else {
        format!("/v1/routerOutputs?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// Every ARN CreateRouterOutput mints must satisfy the modelled `RouterOutputArn`
/// constraint (`smithy_query(query="shape",shape="RouterOutputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerOutput:[a-z0-9]{12}$`),
/// because GetRouterOutput / DeleteRouterOutput take it as an `@httpLabel` and
/// BatchGetRouterOutput's `Arns` member carries that same pattern.
///
/// The check runs through the crate's own encoding of the pattern and then re-asserts,
/// field by field, the parts it fixes, so a bug in that function cannot hide here.
///
/// `region` is the Region the output was created in — a parameter rather than [`REGION`]
/// because `CreateRouterOutput$RegionName` can name another one. `pub(crate)` so
/// `tests/all/router_output_lifecycle.rs` asserts minted ARNs through this ONE function
/// too.
pub(crate) fn assert_minted_router_output_arn(case: &str, arn: &str, id: &str, region: &str) {
    assert!(
        mc::is_mintable_router_output_arn(arn),
        "case {case:?}: a minted RouterOutputArn must satisfy the modelled pattern, got {arn:?}"
    );
    let segments: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        segments.len(),
        7,
        "case {case:?}: a router output ARN has 7 colon-separated segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[2], segments[3], segments[5]),
        ("arn", "mediaconnect", region, "routerOutput"),
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
    assert_eq!(
        segments[6], id,
        "case {case:?}: the last ARN segment is the output's own Id, got {arn:?}"
    );
    assert!(
        id.len() == 12
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "case {case:?}: the output Id is 12 [a-z0-9] characters, got {id:?}"
    );
}

/// A timestamp the service reports, in seconds since the epoch.
///
/// `RouterOutput$CreatedAt` / `$UpdatedAt` carry `@timestampFormat("date-time")` in the
/// model, but this emulator's codec renders every timestamp in restJson1's DEFAULT
/// epoch-seconds form — the framework's generated member schemas carry no
/// `@timestampFormat`, for every service alike — so that is the form asserted here. The
/// value is read from the clock either way, which is what these cases are about.
pub(crate) fn reported_timestamp(case: &str, member: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be reported as a timestamp, got {value}")
    })
}

/// Now, in seconds since the epoch — what a reported timestamp is compared against.
pub(crate) fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The members of a `RouterOutput` that a read must report exactly as the create stored
/// them. `state` is excluded on purpose: it is the one member the CLOCK moves (see
/// [`the_clock_settles_a_new_router_output_into_standby`]).
pub(crate) const PERSISTED_MEMBERS: [&str; 20] = [
    "name",
    "arn",
    "id",
    "outputType",
    "configuration",
    "routedState",
    "regionName",
    "availabilityZone",
    "maximumBitrate",
    "routingScope",
    "tier",
    "createdAt",
    "updatedAt",
    "messages",
    "tags",
    "streamDetails",
    "ipAddress",
    "maintenanceType",
    "maintenanceConfiguration",
    "fabricConfiguration",
];

/// One create, and everything the stored router output must report for it.
struct CreateCase {
    case: &'static str,
    body: Value,
    /// `RouterOutput$OutputType`, which the service derives from the configuration's
    /// union variant rather than reading from the request.
    output_type: &'static str,
    /// The configuration the service must STORE: the requested one, with a STANDARD
    /// output's `Protocol` normalised to the protocol its `ProtocolConfiguration`
    /// configures.
    configuration: Value,
    /// `RouterOutput$StreamDetails`, the union member matching the configuration.
    stream_details: Value,
    availability_zone: &'static str,
    maintenance_type: &'static str,
    maintenance_configuration: Value,
    recovery_latency_mode: &'static str,
    tags: Value,
}

/// A STANDARD configuration body for one protocol configuration, with no `Protocol`
/// member: the service derives the protocol from the configuration.
///
/// A union's members are keyed on the wire by their MODELLED member name (`Standard`,
/// `Rtp`, …): the framework's union schemas carry no `@jsonName`, so that is the key this
/// emulator reads and writes for every service.
pub(crate) fn standard(interface_arn: &str, protocol_configuration: Value) -> Value {
    json!({
        "Standard": {
            "networkInterfaceArn": interface_arn,
            "protocolConfiguration": protocol_configuration,
        }
    })
}

/// The same configuration as [`standard`], with the `Protocol` the service must report
/// filled in — what the store must hold afterwards. `pub(crate)` for the same reason as
/// [`FLOW_ARN`].
pub(crate) fn standard_with_protocol(
    interface_arn: &str,
    protocol_configuration: Value,
    protocol: &str,
) -> Value {
    json!({
        "Standard": {
            "networkInterfaceArn": interface_arn,
            "protocolConfiguration": protocol_configuration,
            "protocol": protocol,
        }
    })
}

/// The rows span the class the rules generalise over: all three configuration variants
/// (so all three output types), all five router output protocols
/// (`smithy_query(query="shape",shape="RouterOutputProtocol")`: RTP | RIST | SRT_CALLER |
/// SRT_LISTENER | RTMP_PUSH), a request that names nothing optional beside a request that
/// names every optional member, and both maintenance configurations.
///
/// The targets are the ones the account HOLDS: `interface_arn` is a minted router network
/// interface ([`minted_network_interface`]) and `flow_arn` / `flow_source_arn` a minted flow
/// and its source ([`minted_flow_source`]), because a configuration naming a target this
/// account does not hold is refused (see
/// `tests/all/router_output_configuration_targets.rs`).
fn create_cases(interface_arn: &str, flow_arn: &str, flow_source_arn: &str) -> Vec<CreateCase> {
    let rtp = json!({"Rtp": {"destinationAddress": "198.51.100.10", "destinationPort": 5004,
        "forwardErrorCorrection": "ENABLED"}});
    let rist = json!({"Rist": {"destinationAddress": "198.51.100.11", "destinationPort": 2030}});
    let srt_caller = json!({"SrtCaller": {"destinationAddress": "198.51.100.12",
        "destinationPort": 9000, "minimumLatencyMilliseconds": 120, "streamId": "primary"}});
    let srt_listener = json!({"SrtListener": {"port": 3000, "minimumLatencyMilliseconds": 10}});
    let rtmp_push = json!({"RtmpPush": {"destinationAddress": "198.51.100.13",
        "destinationPort": 1935, "applicationName": "live", "streamName": "primary",
        "tlsEncryption": {"encryptionType": "PUBLIC",
            "encryptionConfiguration": {"Public": {}}}}});

    vec![
        CreateCase {
            case: "standard RTP, nothing optional named",
            body: json!({
                "name": "ro-rtp",
                "configuration": standard(interface_arn, rtp.clone()),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            configuration: standard_with_protocol(interface_arn, rtp, "RTP"),
            stream_details: json!({"Standard": {"destinationIpAddress": "198.51.100.10"}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
        CreateCase {
            case: "standard RIST, Region named explicitly",
            body: json!({
                "name": "ro-rist",
                "configuration": standard(interface_arn, rist.clone()),
                "maximumBitrate": 30_000_000i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_50",
                "regionName": REGION,
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            configuration: standard_with_protocol(interface_arn, rist, "RIST"),
            stream_details: json!({"Standard": {"destinationIpAddress": "198.51.100.11"}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
        CreateCase {
            case: "standard SRT caller, maintenance window and tags named",
            body: json!({
                "name": "ro-srt-caller",
                "configuration": standard(interface_arn, srt_caller.clone()),
                "maximumBitrate": 100_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_100",
                "availabilityZone": "us-east-1c",
                "tags": {"env": "test", "team": "media"},
                "maintenanceConfiguration": {"PreferredDayTime": {"day": "TUESDAY",
                    "time": "22:00"}},
                "fabricConfiguration": {"recoveryLatencyMode": "LOW_LATENCY"},
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            configuration: standard_with_protocol(interface_arn, srt_caller, "SRT_CALLER"),
            stream_details: json!({"Standard": {"destinationIpAddress": "198.51.100.12"}}),
            availability_zone: "us-east-1c",
            maintenance_type: mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME,
            maintenance_configuration: json!({"PreferredDayTime": {"day": "TUESDAY",
                "time": "22:00"}}),
            recovery_latency_mode: "LOW_LATENCY",
            tags: json!({"env": "test", "team": "media"}),
        },
        CreateCase {
            case: "standard SRT listener, which names no destination address",
            body: json!({
                "name": "ro-srt-listener",
                "configuration": standard(interface_arn, srt_listener.clone()),
                "maximumBitrate": 500_000_000i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_500",
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            configuration: standard_with_protocol(interface_arn, srt_listener, "SRT_LISTENER"),
            // A listener waits for a caller instead of sending to an address, so the
            // stream details carry no destination address at all.
            stream_details: json!({"Standard": {}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
        CreateCase {
            case: "standard RTMP push, naming the protocol its configuration implies",
            body: json!({
                "name": "ro-rtmp-push",
                "configuration": standard_with_protocol(
                    interface_arn,
                    rtmp_push.clone(),
                    "RTMP_PUSH",
                ),
                "maximumBitrate": 1_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_250",
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_STANDARD,
            configuration: standard_with_protocol(interface_arn, rtmp_push, "RTMP_PUSH"),
            stream_details: json!({"Standard": {"destinationIpAddress": "198.51.100.13"}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
        CreateCase {
            case: "MediaConnect flow destination",
            body: json!({
                "name": "ro-mediaconnect-flow",
                "configuration": {"MediaConnectFlow": {
                    "flowArn": flow_arn,
                    "flowSourceArn": flow_source_arn,
                    "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                        "encryptionKeyConfiguration": {"Automatic": {}}},
                }},
                "maximumBitrate": 250_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_250",
                "maintenanceConfiguration": {"Default": {}},
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIACONNECT_FLOW,
            configuration: json!({"MediaConnectFlow": {
                "flowArn": flow_arn,
                "flowSourceArn": flow_source_arn,
                "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                    "encryptionKeyConfiguration": {"Automatic": {}}},
            }}),
            stream_details: json!({"MediaConnectFlow": {}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
        CreateCase {
            case: "MediaLive input destination",
            body: json!({
                "name": "ro-medialive-input",
                "configuration": {"MediaLiveInput": {
                    "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                    "mediaLivePipelineId": "PIPELINE_1",
                    "destinationTransitEncryption": {"encryptionKeyType": "SECRETS_MANAGER",
                        "encryptionKeyConfiguration": {"SecretsManager": {
                            "secretArn": SECRET_ARN, "roleArn": ROLE_ARN}}},
                }},
                "maximumBitrate": 50_000_000i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_50",
            }),
            output_type: mc::ROUTER_OUTPUT_TYPE_MEDIALIVE_INPUT,
            configuration: json!({"MediaLiveInput": {
                "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
                "mediaLivePipelineId": "PIPELINE_1",
                "destinationTransitEncryption": {"encryptionKeyType": "SECRETS_MANAGER",
                    "encryptionKeyConfiguration": {"SecretsManager": {
                        "secretArn": SECRET_ARN, "roleArn": ROLE_ARN}}},
            }}),
            stream_details: json!({"MediaLiveInput": {}}),
            availability_zone: "us-east-1a",
            maintenance_type: mc::MAINTENANCE_TYPE_DEFAULT,
            maintenance_configuration: json!({"Default": {}}),
            recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED,
            tags: json!({}),
        },
    ]
}

#[tokio::test]
async fn create_router_output_persists_what_batch_get_reports() {
    let emulator = Emulator::new();
    // The targets the configurations below name, minted first: a router output's
    // configuration must name resources this account holds.
    let interface_arn = minted_network_interface(&emulator, "rni-ro-create").await;
    let (flow_arn, flow_source_arn) = minted_flow_source(&emulator, "flow-ro-create").await;

    for CreateCase {
        case,
        body,
        output_type,
        configuration,
        stream_details,
        availability_zone,
        maintenance_type,
        maintenance_configuration,
        recovery_latency_mode,
        tags,
    } in create_cases(&interface_arn, &flow_arn, &flow_source_arn)
    {
        let requested_name = body["name"].as_str().unwrap_or_default().to_string();
        let requested_bitrate = body["maximumBitrate"].as_i64().unwrap_or_default();
        let requested_scope = body["routingScope"].as_str().unwrap_or_default().to_string();
        let requested_tier = body["tier"].as_str().unwrap_or_default().to_string();

        let (status, code, created) = create_router_output(&emulator, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: create expected 201, got {status} {code:?}: {created}"
        );

        let output = created["routerOutput"].clone();
        let arn = output["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
            .to_string();
        let id = output["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an Id: {created}"))
            .to_string();
        assert_minted_router_output_arn(case, &arn, &id, REGION);

        for (member, expected) in [
            ("name", json!(requested_name)),
            ("outputType", json!(output_type)),
            ("configuration", configuration.clone()),
            ("routedState", json!(mc::ROUTED_STATE_UNROUTED)),
            ("regionName", json!(REGION)),
            ("availabilityZone", json!(availability_zone)),
            ("maximumBitrate", json!(requested_bitrate)),
            ("routingScope", json!(requested_scope)),
            ("tier", json!(requested_tier)),
            ("state", json!(mc::ROUTER_OUTPUT_STATE_CREATING)),
            ("messages", json!([])),
            ("tags", tags.clone()),
            ("streamDetails", stream_details.clone()),
            ("maintenanceType", json!(maintenance_type)),
            (
                "maintenanceConfiguration",
                maintenance_configuration.clone(),
            ),
            (
                "fabricConfiguration",
                json!({"recoveryLatencyMode": recovery_latency_mode}),
            ),
        ] {
            assert_eq!(
                output[member], expected,
                "case {case:?}: the created router output's {member} must be {expected}, got {:?}",
                output[member]
            );
        }

        // Nothing is routed to a new output and no maintenance has been scheduled for it,
        // so those optional members are ABSENT rather than reported empty.
        for member in [
            "routedInputArn",
            "maintenanceSchedule",
            "maintenanceScheduleType",
        ] {
            assert!(
                output.get(member).is_none(),
                "case {case:?}: a new router output reports no {member}, got {:?}",
                output.get(member)
            );
        }
        assert!(
            output["ipAddress"].as_str().is_some_and(|ip| !ip.is_empty()),
            "case {case:?}: the service assigns the output an IP address, got {:?}",
            output["ipAddress"]
        );
        let created_at = reported_timestamp(case, "createdAt", &output["createdAt"]);
        let updated_at = reported_timestamp(case, "updatedAt", &output["updatedAt"]);
        assert_eq!(
            created_at, updated_at,
            "case {case:?}: nothing has updated a new output, so UpdatedAt equals CreatedAt, \
             got {created_at} and {updated_at}"
        );
        assert!(
            (now_epoch_seconds() - created_at).abs() < 300.0,
            "case {case:?}: CreatedAt is read from the clock, got {created_at}"
        );

        // And the batch read reports the record the create stored, for that ARN alone.
        let (status, code, read) = batch_get_router_outputs(&emulator, &[&arn]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: batch get expected 200, got {status} {code:?}: {read}"
        );
        assert_eq!(
            read["errors"],
            json!([]),
            "case {case:?}: an ARN the store holds yields no error entry, got {read}"
        );
        let reported = read["routerOutputs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one requested ARN is reported once, got {read}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                reported[0][member], output[member],
                "case {case:?}: BatchGetRouterOutput must report the created {member}, got {:?}",
                reported[0][member]
            );
        }
        let reported_state = reported[0]["state"].as_str().unwrap_or_default().to_string();
        assert!(
            [
                mc::ROUTER_OUTPUT_STATE_CREATING,
                mc::ROUTER_OUTPUT_STATE_STANDBY
            ]
            .contains(&reported_state.as_str()),
            "case {case:?}: a read router output is CREATING or STANDBY, got {reported_state:?}"
        );
    }
}

/// How one batch read's request list maps onto its two response lists.
struct BatchCase {
    case: &'static str,
    /// Indexes into the created outputs, or `None` for an ARN nothing holds.
    requested: Vec<Option<usize>>,
    /// The created outputs the response must report, in order.
    expected_outputs: Vec<usize>,
    /// How many `Errors` entries the response must carry.
    expected_errors: usize,
}

#[tokio::test]
async fn batch_get_reports_each_requested_arn_once_and_errors_for_the_rest() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-batch").await;

    // Three stored outputs to address, created through the wire like any client's.
    let mut arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let (status, _, created) = create_router_output(
            &emulator,
            json!({
                "name": format!("ro-batch-{index}"),
                "configuration": standard(&interface_arn, json!({"Rtp": {
                    "destinationAddress": format!("198.51.100.{}", 20 + index),
                    "destinationPort": 5004,
                }})),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
        )
        .await;
        assert_eq!(status, 201, "seeding create expected 201, got {created}");
        arns.push(
            created["routerOutput"]["arn"]
                .as_str()
                .expect("the create answers an ARN")
                .to_string(),
        );
    }

    let cases = vec![
        BatchCase {
            case: "one stored ARN",
            requested: vec![Some(1)],
            expected_outputs: vec![1],
            expected_errors: 0,
        },
        BatchCase {
            case: "every stored ARN, in the order the request names them",
            requested: vec![Some(2), Some(0), Some(1)],
            expected_outputs: vec![2, 0, 1],
            expected_errors: 0,
        },
        BatchCase {
            case: "a stored ARN either side of one nothing holds",
            requested: vec![Some(0), None, Some(2)],
            expected_outputs: vec![0, 2],
            expected_errors: 1,
        },
        BatchCase {
            case: "only ARNs nothing holds",
            requested: vec![None, None],
            // Two different absent ARNs, so two error entries and no outputs.
            expected_outputs: vec![],
            expected_errors: 2,
        },
        BatchCase {
            case: "a repeated ARN is one resource, reported once",
            requested: vec![Some(1), Some(1), Some(0)],
            expected_outputs: vec![1, 0],
            expected_errors: 0,
        },
    ];

    for BatchCase {
        case,
        requested,
        expected_outputs,
        expected_errors,
    } in cases
    {
        // Two distinct absent ARNs, so a case naming both gets two error entries.
        let mut unknown_arns = [UNKNOWN_ARN, OTHER_UNKNOWN_ARN].into_iter();
        let requested_arns: Vec<&str> = requested
            .iter()
            .map(|slot| match slot {
                Some(index) => arns[*index].as_str(),
                None => unknown_arns.next().unwrap_or(UNKNOWN_ARN),
            })
            .collect();

        let (status, code, read) = batch_get_router_outputs(&emulator, &requested_arns).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: a batch read answers 200 whatever its ARNs resolve, got \
             {status} {code:?}: {read}"
        );

        let reported: Vec<String> = read["routerOutputs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|output| output["arn"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = expected_outputs
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: RouterOutputs must report the stored outputs in request order, \
             got {read}"
        );

        let errors = read["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            expected_errors,
            "case {case:?}: expected {expected_errors} Errors entries, got {read}"
        );
        for error in &errors {
            let arn = error["arn"].as_str().unwrap_or_default().to_string();
            assert!(
                requested_arns.contains(&arn.as_str()) && !reported.contains(&arn),
                "case {case:?}: an error entry names a requested ARN that was not reported, \
                 got {error}"
            );
            assert_eq!(
                error["code"].as_str(),
                Some("NotFoundException"),
                "case {case:?}: an ARN the store holds nothing for carries the not-found code, \
                 got {error}"
            );
            assert_eq!(
                error["message"].as_str(),
                Some(mc::not_found_message(&arn).as_str()),
                "case {case:?}: the error message is this service's not-found envelope, got \
                 {error}"
            );
        }
    }

    // A read changes nothing a later caller sees: the same request answers the same body,
    // and the store still holds exactly the three outputs the creates made.
    let every_arn: Vec<&str> = arns.iter().map(String::as_str).collect();
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (_, _, first) = batch_get_router_outputs(&emulator, &every_arn).await;
    let (_, _, second) = batch_get_router_outputs(&emulator, &every_arn).await;
    assert_eq!(
        first, second,
        "a batch read must not change what the next one sees, got {first} then {second}"
    );
    assert_eq!(
        stored_count(&emulator),
        3,
        "a read stores no router output, got {} stored",
        stored_count(&emulator)
    );
}

/// How many router outputs the store holds.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_outputs
        .len()
}

#[tokio::test]
async fn the_clock_settles_a_new_router_output_into_standby() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-clock").await;
    let (status, _, created) = create_router_output(
        &emulator,
        json!({
            "name": "ro-lifecycle",
            "configuration": standard(&interface_arn, json!({"Rist": {
                "destinationAddress": "198.51.100.30", "destinationPort": 2030}})),
            "maximumBitrate": 20_000_000i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
        }),
    )
    .await;
    assert_eq!(status, 201, "create expected 201, got {status}: {created}");
    let arn = created["routerOutput"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // A caller reading straight after the create still observes the transient state.
    let (_, _, read) = batch_get_router_outputs(&emulator, &[&arn]).await;
    assert_eq!(
        read["routerOutputs"][0]["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_CREATING),
        "an output read straight after the create is still CREATING, got {read}"
    );

    // Wait out the transition the clock owes, and the settled state is reported.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (_, _, read) = batch_get_router_outputs(&emulator, &[&arn]).await;
    assert_eq!(
        read["routerOutputs"][0]["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
        "the clock settles a new output into STANDBY, got {read}"
    );
    for member in PERSISTED_MEMBERS {
        assert_eq!(
            read["routerOutputs"][0][member], created["routerOutput"][member],
            "settling the state leaves {member} as the create stored it, got {:?}",
            read["routerOutputs"][0][member]
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_mints_no_second_router_output() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-token").await;
    let body = |name: &str, token: &str| {
        json!({
            "name": name,
            "configuration": standard(&interface_arn, json!({"Rtp": {
                "destinationAddress": "198.51.100.40", "destinationPort": 5004}})),
            "maximumBitrate": 20_000_000i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
            "clientToken": token,
        })
    };

    let (status, _, first) = create_router_output(&emulator, body("ro-token", "token-one")).await;
    assert_eq!(status, 201, "create expected 201, got {status}: {first}");
    let arn = first["routerOutput"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();

    // The same token again — even carrying a different name — answers with the output that
    // token already minted, and mints nothing.
    let (status, _, replay) =
        create_router_output(&emulator, body("ro-token-renamed", "token-one")).await;
    assert_eq!(status, 201, "replay expected 201, got {status}: {replay}");
    assert_eq!(
        replay["routerOutput"], first["routerOutput"],
        "a replayed ClientToken answers with the output it minted, got {replay}"
    );
    assert_eq!(
        stored_count(&emulator),
        1,
        "a replayed ClientToken mints no second output, got {} stored",
        stored_count(&emulator)
    );

    // A DIFFERENT token is a different request: it mints its own output.
    let (status, _, second) = create_router_output(&emulator, body("ro-token", "token-two")).await;
    assert_eq!(status, 201, "create expected 201, got {status}: {second}");
    let other_arn = second["routerOutput"]["arn"]
        .as_str()
        .expect("the create answers an ARN")
        .to_string();
    assert_ne!(
        other_arn, arn,
        "a different ClientToken mints a distinct output, got {other_arn}"
    );
    assert_eq!(
        stored_count(&emulator),
        2,
        "two tokens mint two outputs, got {} stored",
        stored_count(&emulator)
    );
    let (_, _, read) = batch_get_router_outputs(&emulator, &[&arn, &other_arn]).await;
    assert_eq!(
        read["routerOutputs"]
            .as_array()
            .map(|outputs| outputs.len()),
        Some(2),
        "both minted outputs are readable, got {read}"
    );
}

#[tokio::test]
async fn the_tier_bounds_the_maximum_bitrate() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-tier").await;

    // The catalog is `CreateRouterOutput$Tier`'s own documentation
    // (`smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`): the
    // tier "sets the maximum bitrate, in Mbps, for the router output" — OUTPUT_20 20 Mbps,
    // OUTPUT_50 50, OUTPUT_100 100, OUTPUT_250 250, OUTPUT_500 500 — which is every tier
    // the enum declares. Each is driven at its published bound and one bit per second past
    // it.
    for (tier, megabits_per_second) in [
        ("OUTPUT_20", 20i64),
        ("OUTPUT_50", 50),
        ("OUTPUT_100", 100),
        ("OUTPUT_250", 250),
        ("OUTPUT_500", 500),
    ] {
        let ceiling = megabits_per_second * mc::BITS_PER_MEGABIT;
        assert_eq!(
            mc::router_output_tier_bitrate_ceiling(tier),
            Some(ceiling),
            "case {tier:?}: the crate's ceiling must be the documented {megabits_per_second} Mbps"
        );

        let body = |bitrate: i64| {
            json!({
                "name": format!("ro-{}", tier.to_ascii_lowercase()),
                "configuration": standard(&interface_arn, json!({"Rtp": {
                    "destinationAddress": "198.51.100.50", "destinationPort": 5004}})),
                "maximumBitrate": bitrate,
                "routingScope": "REGIONAL",
                "tier": tier,
            })
        };

        let (status, code, at_bound) = create_router_output(&emulator, body(ceiling)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {tier:?}: a request at the tier's {ceiling} bps ceiling is accepted, got \
             {status} {code:?}: {at_bound}"
        );
        assert_eq!(
            at_bound["routerOutput"]["maximumBitrate"].as_i64(),
            Some(ceiling),
            "case {tier:?}: the stored MaximumBitrate is the one requested, got {at_bound}"
        );

        let (status, code, past_bound) = create_router_output(&emulator, body(ceiling + 1)).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {tier:?}: one bit per second past the tier's ceiling is refused, got \
             {status} {code:?}: {past_bound}"
        );
        assert!(
            past_bound["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {tier:?}: the refusal carries this service's validation envelope, got \
             {past_bound}"
        );
    }
}

/// One request this service must refuse, and the modelled failure it answers with.
struct RefusalCase {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-refusals").await;
    let configuration = standard(
        &interface_arn,
        json!({"Rtp": {"destinationAddress": "198.51.100.60", "destinationPort": 5004}}),
    );
    let valid = json!({
        "name": "ro-refusals",
        "configuration": configuration.clone(),
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    });
    let twenty_one_arns: Vec<String> = (0..21)
        .map(|index| format!("arns=arn:aws:mediaconnect:us-east-1:000000000000:routerOutput:{index:012x}"))
        .collect();

    let cases = vec![
        RefusalCase {
            case: "a MaximumBitrate of zero is not a bitrate",
            method: "POST",
            uri: "/v1/routerOutput".to_string(),
            body: Some(json!({
                "name": "ro-zero-bitrate",
                "configuration": configuration.clone(),
                "maximumBitrate": 0i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an Availability Zone outside the output's Region",
            method: "POST",
            uri: "/v1/routerOutput".to_string(),
            body: Some(json!({
                "name": "ro-foreign-zone",
                "configuration": configuration.clone(),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
                "availabilityZone": "us-west-2a",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a RegionName that is not a Region name",
            method: "POST",
            uri: "/v1/routerOutput".to_string(),
            body: Some(json!({
                "name": "ro-bad-region",
                "configuration": configuration.clone(),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
                "regionName": "US-EAST-1",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a Protocol that contradicts the protocol configuration",
            method: "POST",
            uri: "/v1/routerOutput".to_string(),
            body: Some(json!({
                "name": "ro-protocol-mismatch",
                "configuration": standard_with_protocol(
                    &interface_arn,
                    json!({"Rtp": {"destinationAddress": "198.51.100.61",
                        "destinationPort": 5004}}),
                    "RIST",
                ),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a RoutingScope outside the modelled value set",
            method: "POST",
            uri: "/v1/routerOutput".to_string(),
            body: Some(json!({
                "name": "ro-bad-scope",
                "configuration": configuration.clone(),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "PLANETARY",
                "tier": "OUTPUT_20",
            })),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read naming an ARN that is not an ARN at all",
            method: "GET",
            uri: "/v1/routerOutputs?arns=".to_string(),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read past the modelled 20-ARN maximum",
            method: "GET",
            uri: format!("/v1/routerOutputs?{}", twenty_one_arns.join("&")),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a batch read naming an ARN that is not a router output ARN",
            method: "GET",
            uri: format!("/v1/routerOutputs?arns={FLOW_ARN}"),
            body: None,
            status: 400,
            code: "BadRequestException",
        },
    ];

    // The valid shape of the same request is accepted, so each refusal below is about the
    // one thing the case changed.
    let (status, code, created) = create_router_output(&emulator, valid).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the control request must be accepted, got {status} {code:?}: {created}"
    );

    for RefusalCase {
        case,
        method,
        uri,
        body,
        status: expected_status,
        code: expected_code,
    } in cases
    {
        let before = stored_count(&emulator);
        let (status, code, answer) = emulator.send(method, &uri, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {answer}"
        );
        assert_eq!(
            stored_count(&emulator),
            before,
            "case {case:?}: a refused request stores nothing, got {} stored",
            stored_count(&emulator)
        );
    }
}

#[tokio::test]
async fn create_refuses_past_the_store_ceiling() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-ceiling").await;
    let body = |name: &str| {
        json!({
            "name": name,
            "configuration": standard(&interface_arn, json!({"Rtp": {
                "destinationAddress": "198.51.100.70", "destinationPort": 5004}})),
            "maximumBitrate": 20_000_000i64,
            "routingScope": "REGIONAL",
            "tier": "OUTPUT_20",
        })
    };

    // The ceiling is a bound on what is STORED, so the store is filled to one slot short
    // of it directly; the two calls that matter still go over the wire.
    seed_router_outputs(&emulator, mc::MAX_ROUTER_OUTPUTS - 1);
    let (status, code, created) = create_router_output(&emulator, body("ro-last-slot")).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create that fills the last slot is accepted, got {status} {code:?}: {created}"
    );

    let (status, code, refused) = create_router_output(&emulator, body("ro-past-ceiling")).await;
    assert_eq!(
        (status, code.as_deref()),
        (420, Some("RouterOutputServiceQuotaExceededException")),
        "a create past the store ceiling answers the modelled quota error, got {status} \
         {code:?}: {refused}"
    );
    assert_eq!(
        stored_count(&emulator),
        mc::MAX_ROUTER_OUTPUTS,
        "the refused create stored nothing, got {} stored",
        stored_count(&emulator)
    );
}

/// Put `count` router outputs straight into the store, the way CreateRouterOutput stores
/// one.
fn seed_router_outputs(emulator: &Emulator, count: usize) {
    use service_com_amazonaws_mediaconnect_mediaconnect::ops::types::{
        FabricConfiguration, MaintenanceConfiguration, RouterOutputConfiguration,
        RouterOutputProtocolConfiguration, RtpRouterOutputConfiguration,
        StandardRouterOutputConfiguration,
    };

    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    for _ in 0..count {
        let suffix = store.next_suffix();
        let id = mc::router_resource_id(suffix);
        let arn = engine::emulator_arn(
            mc::SERVICE_NAMESPACE,
            REGION,
            &mc::router_output_resource_fragment(&id),
        );
        let now = SystemTime::now();
        store.router_outputs.insert(
            arn.clone(),
            mc::StoredRouterOutput {
                arn,
                id,
                name: format!("seeded-router-output-{suffix}"),
                configuration: RouterOutputConfiguration::Standard(
                    StandardRouterOutputConfiguration {
                        network_interface_arn: NETWORK_INTERFACE_ARN.to_string(),
                        protocol_configuration: RouterOutputProtocolConfiguration::Rtp(
                            RtpRouterOutputConfiguration {
                                destination_address: "198.51.100.80".to_string(),
                                destination_port: 5004,
                                forward_error_correction: None,
                            },
                        ),
                        protocol: Some("RTP".to_string()),
                    },
                ),
                maximum_bitrate: 20_000_000,
                routing_scope: "REGIONAL".to_string(),
                tier: "OUTPUT_20".to_string(),
                region_name: REGION.to_string(),
                availability_zone: mc::default_availability_zone(REGION),
                ip_address: Some(mc::assigned_egress_ip(suffix)),
                maintenance_configuration: MaintenanceConfiguration::Default(Default::default()),
                fabric_configuration: FabricConfiguration {
                    recovery_latency_mode: mc::FABRIC_LATENCY_MODE_BALANCED.to_string(),
                },
                tags: Default::default(),
                // A seeded output is settled, like one whose creation the clock finished.
                state: mc::ROUTER_OUTPUT_STATE_STANDBY.to_string(),
                pending: None,
                routed_input_arn: None,
                messages: Vec::new(),
                // A seeded output has not been started, so nothing has scheduled a
                // maintenance window for it, exactly as a created one has none.
                maintenance_schedule: None,
                client_token: None,
                // A seeded output is live, like a created one: only DeleteRouterOutput sets
                // the instant the clock completes a delete at.
                removal_due_at: None,
                created_at: now,
                updated_at: now,
            },
        );
    }
}

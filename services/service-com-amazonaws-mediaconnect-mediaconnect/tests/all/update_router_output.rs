// SPDX-License-Identifier: CC0-1.0

//! UpdateRouterOutput end to end over HTTP: CreateRouterOutput -> UpdateRouterOutput ->
//! BatchGetRouterOutput.
//!
//! The subject is UpdateRouterOutput: it must APPLY the settings one request carries to the
//! router output its `Arn` addresses in the store — leaving every setting the request omits
//! as it was — and answer with the stored record read back out of the store, never a value
//! fixed in the handler. Its read side (BatchGetRouterOutput) is what each case reads the
//! store through, because an update nobody can read back proves nothing.
//!
//! The cases below vary what those rules generalise over: WHICH member the request carries
//! (each one alone, all of them at once, and none at all), which configuration union
//! variant the output holds and which of the five protocols a STANDARD one is reconfigured
//! with, every tier in the published catalog at its bitrate ceiling and one bit per second
//! past it, which direction a configuration-type change is attempted in, whether the output
//! has an active route, and whether the ARN addresses an output at all.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine) and the
//! request helpers are the ones `router_outputs` owns, so every case drives the same request
//! path a MediaConnect client reaches: routing, restJson1 deserialization, constraint
//! validation, handler, serialization.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_output_lifecycle::delete_router_output;
use crate::router_outputs::{
    batch_get_router_outputs, create_router_output, minted_configuration_targets,
    minted_flow_source, now_epoch_seconds, reported_timestamp, standard, standard_with_protocol,
    PERSISTED_MEMBERS, UNKNOWN_ARN,
};

/// A `RouterInputArn` matching the modelled pattern
/// (`smithy_query(query="shape",shape="RouterInputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerInput:[a-z0-9]{12}$`) — the
/// input an output is ROUTED from in the cases about an active route.
const ROUTED_INPUT_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:a1b2c3d4e5f6";

/// Long enough for a transition the clock owes to be due: twice [`mc::TRANSITION_SPAN`], so
/// a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// UpdateRouterOutput binds its `Arn` member to the URI
/// (`smithy_query(query="operation",operation="UpdateRouterOutput")`:
/// `PUT /v1/routerOutput/{Arn}`).
async fn update_router_output(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/routerOutput/{arn}"), Some(body))
        .await
}

/// The targets a router output's configuration may name: the resources of THIS service a
/// create and an update resolve against the store, minted over the wire so every
/// configuration below names something the account holds.
///
/// The second flow is there so a MEDIACONNECT_FLOW output can be repointed at another flow
/// without changing the TYPE of its configuration.
struct Targets {
    interface_arn: String,
    flow_arn: String,
    flow_source_arn: String,
    other_flow_arn: String,
    other_flow_source_arn: String,
}

impl Targets {
    async fn mint(emulator: &Emulator) -> Self {
        let primary = minted_configuration_targets(emulator, "ro-update").await;
        let (other_flow_arn, other_flow_source_arn) =
            minted_flow_source(emulator, "flow-ro-update-other").await;
        Self {
            interface_arn: primary.interface_arn,
            flow_arn: primary.flow_arn,
            flow_source_arn: primary.flow_source_arn,
            other_flow_arn,
            other_flow_source_arn,
        }
    }
}

/// A STANDARD RTP create at a tier with room to move the bitrate in both directions, which
/// is what most cases start from.
fn standard_create(targets: &Targets, name: &str) -> Value {
    json!({
        "name": name,
        "configuration": standard(&targets.interface_arn, json!({"Rtp": {
            "destinationAddress": "198.51.100.10",
            "destinationPort": 5004, "forwardErrorCorrection": "ENABLED"}})),
        "maximumBitrate": 100_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_250",
    })
}

/// A MEDIACONNECT_FLOW create — the other kind of output the update cases act on.
fn mediaconnect_flow_create(targets: &Targets, name: &str) -> Value {
    json!({
        "name": name,
        "configuration": {"MediaConnectFlow": {
            "flowArn": targets.flow_arn,
            "flowSourceArn": targets.flow_source_arn,
            "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                "encryptionKeyConfiguration": {"Automatic": {}}},
        }},
        "maximumBitrate": 50_000_000i64,
        "routingScope": "GLOBAL",
        "tier": "OUTPUT_50",
    })
}

/// Create one router output through the wire and answer `(arn, the created RouterOutput)`.
async fn create_output(emulator: &Emulator, body: Value) -> (String, Value) {
    let (status, code, created) = create_router_output(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "seeding create expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers an ARN: {created}"))
        .to_string();
    (arn, created["routerOutput"].clone())
}

/// The router output the store holds for `arn`, read back through BatchGetRouterOutput.
async fn read_output(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, read) = batch_get_router_outputs(emulator, &[arn]).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read back expected 200, got {status} {code:?}: {read}"
    );
    read["routerOutputs"][0].clone()
}

/// One update request, and what the stored output must report once it is applied.
struct UpdateCase {
    case: &'static str,
    /// The output this case acts on, created through the wire.
    create: Value,
    /// The body of the update.
    update: Value,
    /// The members the updated output must report DIFFERENTLY from the created one, as
    /// `(member, value)`. Every other member must be reported exactly as the create stored
    /// it, which is the half of the merge rule a request that omits a member exercises.
    changed: Vec<(&'static str, Value)>,
}

/// The rows span the class the merge rule generalises over: each updatable member on its
/// own (`Name`, `MaximumBitrate`, `Tier`, `RoutingScope`, `MaintenanceConfiguration`,
/// `FabricConfiguration`, `Configuration`), every one of them at once, none of them at all,
/// a STANDARD output reconfigured onto each of the other router output protocols — one that
/// sends to an address and one that waits for a caller, so the derived `StreamDetails`
/// moves both ways — and a MEDIACONNECT_FLOW output repointed at another flow.
fn update_cases(targets: &Targets) -> Vec<UpdateCase> {
    let rist = json!({"Rist": {"destinationAddress": "198.51.100.21", "destinationPort": 2030}});
    let srt_listener = json!({"SrtListener": {"port": 3000, "minimumLatencyMilliseconds": 10}});
    let srt_caller = json!({"SrtCaller": {"destinationAddress": "198.51.100.22",
        "destinationPort": 9000, "minimumLatencyMilliseconds": 120, "streamId": "primary"}});
    let rtmp_push = json!({"RtmpPush": {"destinationAddress": "198.51.100.23",
        "destinationPort": 1935, "applicationName": "live", "streamName": "primary"}});
    let other_flow = json!({"MediaConnectFlow": {
        "flowArn": targets.other_flow_arn,
        "flowSourceArn": targets.other_flow_source_arn,
        "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
            "encryptionKeyConfiguration": {"Automatic": {}}},
    }});
    let interface_arn = targets.interface_arn.as_str();

    vec![
        UpdateCase {
            case: "Name alone",
            create: standard_create(targets, "ro-update-name"),
            update: json!({"name": "ro-update-name-renamed"}),
            changed: vec![("name", json!("ro-update-name-renamed"))],
        },
        UpdateCase {
            case: "MaximumBitrate alone, inside the stored tier's ceiling",
            create: standard_create(targets, "ro-update-bitrate"),
            update: json!({"maximumBitrate": 250_000_000i64}),
            changed: vec![("maximumBitrate", json!(250_000_000i64))],
        },
        UpdateCase {
            case: "Tier alone, raised above the stored bitrate",
            create: standard_create(targets, "ro-update-tier"),
            update: json!({"tier": "OUTPUT_500"}),
            changed: vec![("tier", json!("OUTPUT_500"))],
        },
        UpdateCase {
            case: "Tier and MaximumBitrate lowered together",
            create: standard_create(targets, "ro-update-tier-down"),
            update: json!({"tier": "OUTPUT_20", "maximumBitrate": 20_000_000i64}),
            changed: vec![
                ("tier", json!("OUTPUT_20")),
                ("maximumBitrate", json!(20_000_000i64)),
            ],
        },
        UpdateCase {
            case: "RoutingScope alone",
            create: standard_create(targets, "ro-update-scope"),
            update: json!({"routingScope": "GLOBAL"}),
            changed: vec![("routingScope", json!("GLOBAL"))],
        },
        UpdateCase {
            case: "MaintenanceConfiguration alone, onto the other union member",
            create: standard_create(targets, "ro-update-maintenance"),
            update: json!({"maintenanceConfiguration": {"PreferredDayTime": {"day": "TUESDAY",
                "time": "22:00"}}}),
            changed: vec![
                (
                    "maintenanceConfiguration",
                    json!({"PreferredDayTime": {"day": "TUESDAY", "time": "22:00"}}),
                ),
                (
                    "maintenanceType",
                    json!(mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME),
                ),
            ],
        },
        UpdateCase {
            case: "FabricConfiguration alone, on an output with no active route",
            create: standard_create(targets, "ro-update-fabric"),
            update: json!({"fabricConfiguration": {"recoveryLatencyMode": "LOW_LATENCY"}}),
            changed: vec![(
                "fabricConfiguration",
                json!({"recoveryLatencyMode": "LOW_LATENCY"}),
            )],
        },
        UpdateCase {
            case: "Configuration onto RIST, which sends to an address",
            create: standard_create(targets, "ro-update-rist"),
            update: json!({"configuration": standard(interface_arn, rist.clone())}),
            changed: vec![
                (
                    "configuration",
                    standard_with_protocol(interface_arn, rist, "RIST"),
                ),
                (
                    "streamDetails",
                    json!({"Standard": {"destinationIpAddress": "198.51.100.21"}}),
                ),
            ],
        },
        UpdateCase {
            case: "Configuration onto an SRT listener, which names no destination address",
            create: standard_create(targets, "ro-update-srt-listener"),
            update: json!({"configuration": standard(interface_arn, srt_listener.clone())}),
            changed: vec![
                (
                    "configuration",
                    standard_with_protocol(interface_arn, srt_listener, "SRT_LISTENER"),
                ),
                ("streamDetails", json!({"Standard": {}})),
            ],
        },
        UpdateCase {
            case: "Configuration onto an SRT caller, naming the protocol it implies",
            create: standard_create(targets, "ro-update-srt-caller"),
            update: json!({"configuration": standard_with_protocol(interface_arn,
                srt_caller.clone(), "SRT_CALLER")}),
            changed: vec![
                (
                    "configuration",
                    standard_with_protocol(interface_arn, srt_caller, "SRT_CALLER"),
                ),
                (
                    "streamDetails",
                    json!({"Standard": {"destinationIpAddress": "198.51.100.22"}}),
                ),
            ],
        },
        UpdateCase {
            case: "Configuration onto an RTMP push",
            create: standard_create(targets, "ro-update-rtmp-push"),
            update: json!({"configuration": standard(interface_arn, rtmp_push.clone())}),
            changed: vec![
                (
                    "configuration",
                    standard_with_protocol(interface_arn, rtmp_push, "RTMP_PUSH"),
                ),
                (
                    "streamDetails",
                    json!({"Standard": {"destinationIpAddress": "198.51.100.23"}}),
                ),
            ],
        },
        UpdateCase {
            case: "a MEDIACONNECT_FLOW output repointed at another flow",
            create: mediaconnect_flow_create(targets, "ro-update-flow"),
            update: json!({"configuration": other_flow.clone()}),
            // The stream details of an output that delivers into another AWS resource carry
            // nothing to recompute, so they stay as the model declares them.
            changed: vec![("configuration", other_flow)],
        },
        UpdateCase {
            case: "every updatable member at once",
            create: standard_create(targets, "ro-update-everything"),
            update: json!({
                "name": "ro-update-everything-renamed",
                "configuration": standard(interface_arn, json!({"Rtp": {
                    "destinationAddress": "198.51.100.24",
                    "destinationPort": 5006}})),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "GLOBAL",
                "tier": "OUTPUT_20",
                "maintenanceConfiguration": {"PreferredDayTime": {"day": "SUNDAY",
                    "time": "03:00"}},
                "fabricConfiguration": {"recoveryLatencyMode": "LOW_LATENCY"},
            }),
            changed: vec![
                ("name", json!("ro-update-everything-renamed")),
                (
                    "configuration",
                    standard_with_protocol(
                        interface_arn,
                        json!({"Rtp": {"destinationAddress": "198.51.100.24",
                            "destinationPort": 5006}}),
                        "RTP",
                    ),
                ),
                (
                    "streamDetails",
                    json!({"Standard": {"destinationIpAddress": "198.51.100.24"}}),
                ),
                ("maximumBitrate", json!(20_000_000i64)),
                ("routingScope", json!("GLOBAL")),
                ("tier", json!("OUTPUT_20")),
                (
                    "maintenanceConfiguration",
                    json!({"PreferredDayTime": {"day": "SUNDAY", "time": "03:00"}}),
                ),
                (
                    "maintenanceType",
                    json!(mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME),
                ),
                (
                    "fabricConfiguration",
                    json!({"recoveryLatencyMode": "LOW_LATENCY"}),
                ),
            ],
        },
        UpdateCase {
            case: "no updatable member at all",
            create: standard_create(targets, "ro-update-nothing"),
            update: json!({}),
            changed: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn update_router_output_applies_what_it_carries_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    for UpdateCase {
        case,
        create,
        update,
        changed,
    } in update_cases(&targets)
    {
        let (arn, created) = create_output(&emulator, create).await;

        let (status, code, answered) = update_router_output(&emulator, &arn, update).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
        );
        let updated = answered["routerOutput"].clone();

        // Every member of the updated output: the value this request asked for where it
        // asked for one, and the value the create stored where it did not. `UpdatedAt` is
        // the one member the CALL moves rather than the request, asserted below.
        for member in PERSISTED_MEMBERS {
            if member == "updatedAt" {
                continue;
            }
            let expected = changed
                .iter()
                .find(|(changed_member, _)| *changed_member == member)
                .map_or_else(|| created[member].clone(), |(_, value)| value.clone());
            assert_eq!(
                updated[member], expected,
                "case {case:?}: the updated router output's {member} must be {expected}, got {:?}",
                updated[member]
            );
        }

        // The update changed the resource, so its `UpdatedAt` is read from the clock and is
        // no earlier than the `CreatedAt` the create stored, which the update leaves alone.
        let created_at = reported_timestamp(case, "createdAt", &updated["createdAt"]);
        let updated_at = reported_timestamp(case, "updatedAt", &updated["updatedAt"]);
        assert!(
            updated_at >= created_at,
            "case {case:?}: UpdatedAt must not precede CreatedAt, got {updated_at} and \
             {created_at}"
        );
        assert!(
            (now_epoch_seconds() - updated_at).abs() < 300.0,
            "case {case:?}: UpdatedAt is read from the clock, got {updated_at}"
        );

        // The response is the STORED record: a read straight afterwards reports exactly it.
        let stored = read_output(&emulator, case, &arn).await;
        assert_eq!(
            stored, updated,
            "case {case:?}: BatchGetRouterOutput must report the record the update stored, \
             got {stored}"
        );
    }
}

#[tokio::test]
async fn an_updated_output_reports_updating_until_the_clock_settles_it() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // An output whose creation has already settled comes back to the state it was settled
    // in; one that is still being created comes back to the state its CREATE owed it. Both
    // rows are `(case, whether to wait the create out)`.
    for (case, wait_for_create) in [
        ("an output settled in STANDBY", true),
        ("an output still being created", false),
    ] {
        let (arn, _) =
            create_output(&emulator, standard_create(&targets, "ro-update-lifecycle")).await;
        if wait_for_create {
            tokio::time::sleep(past_transition_span()).await;
            let settled = read_output(&emulator, case, &arn).await;
            assert_eq!(
                settled["state"].as_str(),
                Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
                "case {case:?}: the clock settles the create into STANDBY first, got {settled}"
            );
        }

        let (status, code, answered) =
            update_router_output(&emulator, &arn, json!({"name": "ro-update-lifecycle-2"})).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["routerOutput"]["state"].as_str(),
            Some(mc::ROUTER_OUTPUT_STATE_UPDATING),
            "case {case:?}: an accepted update answers the transient UPDATING state, got \
             {answered}"
        );

        // A caller reading straight after the update still observes that transient state.
        let in_flight = read_output(&emulator, case, &arn).await;
        assert_eq!(
            in_flight["state"].as_str(),
            Some(mc::ROUTER_OUTPUT_STATE_UPDATING),
            "case {case:?}: an output read straight after the update is still UPDATING, got \
             {in_flight}"
        );

        // And once the transition the clock owes is due, the output is back in the state it
        // was heading for, with everything the update stored still in place.
        tokio::time::sleep(past_transition_span()).await;
        let settled = read_output(&emulator, case, &arn).await;
        assert_eq!(
            settled["state"].as_str(),
            Some(mc::ROUTER_OUTPUT_STATE_STANDBY),
            "case {case:?}: the clock settles an updated output back into STANDBY, got \
             {settled}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                settled[member], answered["routerOutput"][member],
                "case {case:?}: settling the state leaves {member} as the update stored it, \
                 got {:?}",
                settled[member]
            );
        }
    }
}

#[tokio::test]
async fn the_tier_bounds_the_maximum_bitrate_an_update_may_set() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // The catalog is `CreateRouterOutput$Tier`'s own documentation
    // (`smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`), which
    // `UpdateRouterOutput$Tier` repeats: the tier "sets the maximum bitrate, in Mbps, for
    // the router output" — OUTPUT_20 20 Mbps, OUTPUT_50 50, OUTPUT_100 100, OUTPUT_250 250,
    // OUTPUT_500 500 — which is every tier the enum declares. Each is driven at its
    // published bound and one bit per second past it.
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

        let (arn, created) = create_output(
            &emulator,
            standard_create(&targets, &format!("ro-bound-{}", tier.to_ascii_lowercase())),
        )
        .await;

        let (status, code, past_bound) = update_router_output(
            &emulator,
            &arn,
            json!({"tier": tier, "maximumBitrate": ceiling + 1}),
        )
        .await;
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
        let refused = read_output(&emulator, tier, &arn).await;
        assert_eq!(
            (
                refused["tier"].clone(),
                refused["maximumBitrate"].clone(),
                refused["updatedAt"].clone()
            ),
            (
                created["tier"].clone(),
                created["maximumBitrate"].clone(),
                created["updatedAt"].clone()
            ),
            "case {tier:?}: a refused update stores nothing, got {refused}"
        );

        let (status, code, at_bound) = update_router_output(
            &emulator,
            &arn,
            json!({"tier": tier, "maximumBitrate": ceiling}),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {tier:?}: an update at the tier's {ceiling} bps ceiling is accepted, got \
             {status} {code:?}: {at_bound}"
        );
        assert_eq!(
            (
                at_bound["routerOutput"]["tier"].as_str(),
                at_bound["routerOutput"]["maximumBitrate"].as_i64()
            ),
            (Some(tier), Some(ceiling)),
            "case {tier:?}: the stored tier and bitrate are the ones requested, got {at_bound}"
        );
    }
}

/// One update this service must refuse, and the modelled failure it answers with.
struct RefusalCase {
    case: &'static str,
    /// Which created output the request addresses: `Some(index)` into the outputs the test
    /// creates, or `None` for an ARN nothing holds.
    output: Option<usize>,
    body: Value,
    status: u16,
    code: &'static str,
    /// The envelope the message must open with, or `None` where the modelled error carries
    /// neither of this service's `BadRequestException` envelopes.
    envelope: Option<&'static str>,
}

#[tokio::test]
async fn refusals_name_the_modelled_error_and_store_nothing() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // Two outputs of DIFFERENT types, so a configuration-type change can be attempted in
    // both directions.
    let (standard_arn, standard_created) =
        create_output(&emulator, standard_create(&targets, "ro-refusal-standard")).await;
    let (flow_output_arn, flow_created) = create_output(
        &emulator,
        mediaconnect_flow_create(&targets, "ro-refusal-flow"),
    )
    .await;
    let arns = [standard_arn.clone(), flow_output_arn.clone()];
    let created = [standard_created, flow_created];

    let cases = vec![
        RefusalCase {
            case: "an ARN no create minted",
            output: None,
            body: json!({"name": "ro-refusal-absent"}),
            status: 404,
            code: "NotFoundException",
            envelope: None,
        },
        RefusalCase {
            case: "replacing a STANDARD configuration with a MEDIACONNECT_FLOW one",
            output: Some(0),
            body: json!({"configuration": {"MediaConnectFlow": {
                "flowArn": targets.other_flow_arn,
                "flowSourceArn": targets.other_flow_source_arn,
                "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                    "encryptionKeyConfiguration": {"Automatic": {}}},
            }}}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::BAD_REQUEST_PREFIX),
        },
        RefusalCase {
            case: "replacing a MEDIACONNECT_FLOW configuration with a STANDARD one",
            output: Some(1),
            body: json!({"configuration": standard(&targets.interface_arn, json!({"Rtp": {
                "destinationAddress": "198.51.100.31", "destinationPort": 5004}}))}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::BAD_REQUEST_PREFIX),
        },
        RefusalCase {
            case: "a Protocol that contradicts the protocol configuration",
            output: Some(0),
            body: json!({"configuration": standard_with_protocol(
                &targets.interface_arn,
                json!({"Rtp": {"destinationAddress": "198.51.100.32", "destinationPort": 5004}}),
                "RIST",
            )}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a MaximumBitrate of zero is not a bitrate",
            output: Some(0),
            body: json!({"maximumBitrate": 0i64}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a negative MaximumBitrate is not a bitrate",
            output: Some(0),
            body: json!({"maximumBitrate": -20_000_000i64}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a Tier lowered under the bitrate the output already carries",
            output: Some(0),
            body: json!({"tier": "OUTPUT_20"}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a Tier outside the modelled value set",
            output: Some(0),
            body: json!({"tier": "OUTPUT_1000"}),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a RoutingScope outside the modelled value set",
            output: Some(0),
            body: json!({"routingScope": "PLANETARY"}),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a Name past the modelled 128-character maximum",
            output: Some(0),
            body: json!({"name": "n".repeat(129)}),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a Name under the modelled 1-character minimum",
            output: Some(0),
            body: json!({"name": ""}),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
    ];

    for RefusalCase {
        case,
        output,
        body,
        status: expected_status,
        code: expected_code,
        envelope,
    } in cases
    {
        let arn = output.map_or(UNKNOWN_ARN, |index| arns[index].as_str());
        let (status, code, refusal) = update_router_output(&emulator, arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} \
             {code:?}: {refusal}"
        );
        if let Some(envelope) = envelope {
            assert!(
                refusal["message"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(envelope),
                "case {case:?}: the refusal must open with {envelope:?}, got {refusal}"
            );
        }
        if let Some(index) = output {
            let stored = read_output(&emulator, case, arn).await;
            for member in PERSISTED_MEMBERS {
                assert_eq!(
                    stored[member], created[index][member],
                    "case {case:?}: a refused update stores nothing, so {member} is still \
                     what the create stored, got {:?}",
                    stored[member]
                );
            }
        }
    }

    // The ARN nothing holds is reported with this service's not-found envelope, and the
    // refusal created no record for it.
    let (_, _, refusal) =
        update_router_output(&emulator, UNKNOWN_ARN, json!({"name": "ro-absent"})).await;
    assert_eq!(
        refusal["message"].as_str(),
        Some(mc::not_found_message(UNKNOWN_ARN).as_str()),
        "the not-found refusal carries this service's not-found envelope, got {refusal}"
    );
    let (_, _, read) = batch_get_router_outputs(&emulator, &[UNKNOWN_ARN]).await;
    assert_eq!(
        read["routerOutputs"],
        json!([]),
        "a refused update on an absent ARN creates no router output, got {read}"
    );
}

#[tokio::test]
async fn an_update_is_refused_while_the_output_is_being_deleted() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;
    let (arn, created) = create_output(&emulator, standard_create(&targets, "ro-deleting")).await;

    let (status, code, deleted) = delete_router_output(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete expected 200, got {status} {code:?}: {deleted}"
    );
    assert_eq!(
        deleted["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_DELETING),
        "the delete leaves the output in DELETING, got {deleted}"
    );

    // An output on its way out of the store has nothing left to reconfigure.
    let rename = json!({"name": "ro-deleting-renamed"});
    let (status, code, refusal) = update_router_output(&emulator, &arn, rename.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "an output being deleted refuses an update, got {status} {code:?}: {refusal}"
    );
    let stored = read_output(&emulator, "being deleted", &arn).await;
    assert_eq!(
        stored["name"], created["name"],
        "the refused update left the stored Name alone, got {stored}"
    );
    assert_eq!(
        stored["state"].as_str(),
        Some(mc::ROUTER_OUTPUT_STATE_DELETING),
        "the refused update left the output in the state its delete put it in, got {stored}"
    );

    // And once the clock has completed the deletion there is no record at all, so the same
    // request is a not-found rather than a conflict.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, refusal) = update_router_output(&emulator, &arn, rename).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "an output the clock has removed is a not-found, got {status} {code:?}: {refusal}"
    );
    assert_eq!(
        refusal["message"].as_str(),
        Some(mc::not_found_message(&arn).as_str()),
        "the not-found refusal carries this service's not-found envelope, got {refusal}"
    );
}

/// Route `input_arn` to the router output `arn` in the store, or unroute it with `None`.
///
/// TakeRouterInput — the operation that routes a router input to an output — is still a
/// scaffold, so the ACTIVE ROUTE that `UpdateRouterOutput$FabricConfiguration`'s documented
/// precondition is stated against cannot be reached over the wire yet. The record is put in
/// the state the service holds a routed output in instead: `RouterOutput$RoutedInputArn` is
/// the one place that relation lives.
fn set_routed_input(emulator: &Emulator, arn: &str, input_arn: Option<&str>) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let mut store = state.lock();
    let output = store
        .router_outputs
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the created output {arn} is in the store"));
    output.routed_input_arn = input_arn.map(ToString::to_string);
}

#[tokio::test]
async fn the_fabric_configuration_cannot_be_updated_while_the_output_is_routed() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;
    let (arn, created) = create_output(&emulator, standard_create(&targets, "ro-routed")).await;
    set_routed_input(&emulator, &arn, Some(ROUTED_INPUT_ARN));

    // "You cannot update the fabric configuration while the output has an active route."
    let (status, code, refusal) = update_router_output(
        &emulator,
        &arn,
        json!({"fabricConfiguration": {"recoveryLatencyMode": "LOW_LATENCY"}}),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "a routed output refuses a FabricConfiguration update, got {status} {code:?}: \
         {refusal}"
    );
    let stored = read_output(&emulator, "routed", &arn).await;
    assert_eq!(
        stored["fabricConfiguration"], created["fabricConfiguration"],
        "the refused update left the stored FabricConfiguration alone, got {stored}"
    );
    assert_eq!(
        stored["routedState"].as_str(),
        Some(mc::ROUTED_STATE_ROUTED),
        "the output under test is the ROUTED one, got {stored}"
    );

    // The refusal is about that ONE member: the same output takes an update that does not
    // touch its fabric configuration.
    let (status, code, answered) =
        update_router_output(&emulator, &arn, json!({"name": "ro-routed-renamed"})).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "a routed output still takes an update that leaves its FabricConfiguration alone, \
         got {status} {code:?}: {answered}"
    );
    assert_eq!(
        answered["routerOutput"]["name"].as_str(),
        Some("ro-routed-renamed"),
        "the accepted update stored the new Name, got {answered}"
    );

    // "You must unroute the output before updating the fabric configuration" — and once it
    // is unrouted, the same request is accepted.
    set_routed_input(&emulator, &arn, None);
    let (status, code, answered) = update_router_output(
        &emulator,
        &arn,
        json!({"fabricConfiguration": {"recoveryLatencyMode": "LOW_LATENCY"}}),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "an unrouted output takes a FabricConfiguration update, got {status} {code:?}: \
         {answered}"
    );
    assert_eq!(
        answered["routerOutput"]["fabricConfiguration"],
        json!({"recoveryLatencyMode": "LOW_LATENCY"}),
        "the accepted update stored the new FabricConfiguration, got {answered}"
    );
}

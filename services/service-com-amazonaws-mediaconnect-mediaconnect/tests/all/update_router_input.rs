// SPDX-License-Identifier: CC0-1.0

//! UpdateRouterInput end to end over HTTP: CreateRouterInput -> UpdateRouterInput ->
//! GetRouterInput.
//!
//! The subject is UpdateRouterInput: it must APPLY the settings one request carries to the
//! router input its `Arn` addresses in the store — leaving every setting the request omits
//! as it was — and answer with the stored record read back out of the store, never a value
//! fixed in the handler. Its read side (GetRouterInput) is what each case reads the store
//! through, because an update nobody can read back proves nothing.
//!
//! The cases below vary what those rules generalise over: WHICH member the request carries
//! (each one alone, all of them at once, and none at all), which configuration union variant
//! the input holds and which protocol a STANDARD one is reconfigured with, every tier in the
//! published catalog at its bitrate ceiling and one bit per second past it, which direction a
//! configuration-type change is attempted in, whether the input is connected to the source it
//! takes its content from, whether a delete is already under way, and whether the ARN
//! addresses an input at all.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine) and the
//! request helpers are the ones `router_input_restart` and `router_outputs` own, so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_configuration_targets::minted_flow_output;
use crate::router_input_restart::{
    create_router_input, get_router_input, past_transition_span, settle_router_input_in,
};
use crate::router_outputs::{
    minted_network_interface, now_epoch_seconds, reported_timestamp, ROLE_ARN, SECRET_ARN,
};

/// An ARN satisfying the modelled `RouterInputArn` pattern that no create minted, so no store
/// holds a router input for it.
const UNKNOWN_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:ffffffffffff";

/// A `MediaLiveChannelArn` matching the modelled pattern
/// (`smithy_query(query="shape",shape="MediaLiveChannelRouterInputConfiguration")`). The
/// channel belongs to ANOTHER service, so nothing in this store resolves it.
const MEDIALIVE_CHANNEL_ARN: &str = "arn:aws:medialive:us-east-1:000000000000:channel:1234567";

/// The members of a `RouterInput` an accepted update must report either as the request asked
/// or exactly as they were stored. `state` is excluded on purpose: it is the one member the
/// CALL and then the CLOCK move (see
/// [`an_updated_input_reports_updating_until_the_clock_settles_it`]).
const PERSISTED_MEMBERS: [&str; 25] = [
    "name",
    "arn",
    "id",
    "inputType",
    "configuration",
    "routedOutputs",
    "maximumRoutedOutputs",
    "regionName",
    "availabilityZone",
    "maximumBitrate",
    "tier",
    "routingScope",
    "createdAt",
    "updatedAt",
    "messages",
    "transitEncryption",
    "tags",
    "streamDetails",
    "ipAddress",
    "maintenanceType",
    "maintenanceConfiguration",
    "maintenanceScheduleType",
    "maintenanceSchedule",
    "contentQualityAnalysisType",
    "contentQualityAnalysisConfiguration",
];

/// UpdateRouterInput binds its `Arn` member to the URI
/// (`smithy_query(query="operation",operation="UpdateRouterInput")`:
/// `PUT /v1/routerInput/{Arn}`).
///
/// `pub(crate)` because the suite that pins what a flow removal does to the router records
/// pointing into it re-states the configuration a read reports through this same request
/// (see `tests/all/flow_removal_clears_router_references.rs`).
pub(crate) async fn update_router_input(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/routerInput/{arn}"), Some(body))
        .await
}

/// Send one DeleteRouterInput request (`DELETE /v1/routerInput/{Arn}`).
async fn delete_router_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// The `sourceTransitDecryption` of a source whose key MediaConnect manages itself.
fn automatic_decryption() -> Value {
    json!({ "encryptionKeyType": "AUTOMATIC", "encryptionKeyConfiguration": { "Automatic": {} } })
}

/// The `sourceTransitDecryption` of a source whose key the caller holds in Secrets Manager —
/// the OTHER decryption, so a case can ask for a different one.
fn secrets_manager_decryption() -> Value {
    json!({
        "encryptionKeyType": "SECRETS_MANAGER",
        "encryptionKeyConfiguration": {
            "SecretsManager": { "secretArn": SECRET_ARN, "roleArn": ROLE_ARN },
        },
    })
}

/// A STANDARD configuration ingesting on `interface_arn` with `protocol_configuration`.
fn standard(interface_arn: &str, protocol_configuration: Value) -> Value {
    json!({ "Standard": {
        "networkInterfaceArn": interface_arn,
        "protocolConfiguration": protocol_configuration,
    }})
}

/// The STANDARD configuration a request carrying `protocol_configuration` and NO `Protocol`
/// is STORED as: `StandardRouterInputConfiguration$Protocol` is implied by the variant of
/// the required `ProtocolConfiguration`, so a read reports `protocol` whether or not the
/// request named it.
fn stored_standard(interface_arn: &str, protocol_configuration: Value, protocol: &str) -> Value {
    let mut configuration = standard(interface_arn, protocol_configuration);
    configuration["Standard"]["protocol"] = json!(protocol);
    configuration
}

/// A FAILOVER configuration ingesting on `interface_arn`: "a list of exactly two protocol
/// configurations for the failover input sources. Both must use the same protocol type"
/// (`smithy_query(query="shape",shape="FailoverRouterInputConfiguration")`).
fn failover(interface_arn: &str, first_port: i64, second_port: i64) -> Value {
    json!({ "Failover": {
        "networkInterfaceArn": interface_arn,
        "sourcePriorityMode": "PRIMARY_SECONDARY",
        "primarySourceIndex": 0,
        "protocolConfigurations": [
            { "Rtp": { "port": first_port } },
            { "Rtp": { "port": second_port } },
        ],
    }})
}

/// A MEDIACONNECT FLOW configuration carrying `decryption`, naming the flow and the flow
/// output only where the case carries one: "to connect the router input to the flow output,
/// provide both `flowArn` and `flowOutputArn`. To disconnect it, call the update operation and
/// omit both fields while keeping this configuration object"
/// (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`).
fn mediaconnect_flow(
    flow_arn: Option<&str>,
    flow_output_arn: Option<&str>,
    decryption: Value,
) -> Value {
    let mut payload = json!({ "sourceTransitDecryption": decryption });
    if let Some(flow_arn) = flow_arn {
        payload["flowArn"] = json!(flow_arn);
    }
    if let Some(flow_output_arn) = flow_output_arn {
        payload["flowOutputArn"] = json!(flow_output_arn);
    }
    json!({ "MediaConnectFlow": payload })
}

/// A MEDIALIVE CHANNEL configuration carrying `decryption`, naming the channel output only
/// where the case carries one: "to connect the router input to the MediaLive channel output,
/// provide all three of `mediaLiveChannelArn`, `mediaLivePipelineId`, and
/// `mediaLiveChannelOutputName`. To disconnect it, call the update operation and omit all
/// three fields while keeping this configuration object"
/// (`smithy_query(query="shape",shape="MediaLiveChannelRouterInputConfiguration")`).
fn medialive_channel(connected_output: Option<&str>, decryption: Value) -> Value {
    let mut payload = json!({ "sourceTransitDecryption": decryption });
    if let Some(output_name) = connected_output {
        payload["mediaLiveChannelArn"] = json!(MEDIALIVE_CHANNEL_ARN);
        payload["mediaLivePipelineId"] = json!("PIPELINE_0");
        payload["mediaLiveChannelOutputName"] = json!(output_name);
    }
    json!({ "MediaLiveChannel": payload })
}

/// The resources of THIS service a router input configuration may name, minted over the wire
/// so every configuration below points at something the account holds.
///
/// The second interface and the second flow output are there so an input can be repointed
/// without changing the TYPE of its configuration.
struct Targets {
    interface_arn: String,
    other_interface_arn: String,
    flow_arn: String,
    flow_output_arn: String,
    other_flow_arn: String,
    other_flow_output_arn: String,
}

impl Targets {
    async fn mint(emulator: &Emulator) -> Self {
        let interface_arn = minted_network_interface(emulator, "rni-ri-update").await;
        let other_interface_arn = minted_network_interface(emulator, "rni-ri-update-other").await;
        let (flow_arn, flow_output_arn) = minted_flow_output(emulator, "flow-ri-update").await;
        let (other_flow_arn, other_flow_output_arn) =
            minted_flow_output(emulator, "flow-ri-update-other").await;
        Self {
            interface_arn,
            other_interface_arn,
            flow_arn,
            flow_output_arn,
            other_flow_arn,
            other_flow_output_arn,
        }
    }
}

/// A CreateRouterInput body for `configuration` at a tier with room to move the bitrate in
/// both directions, which is what most cases start from.
fn create_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "configuration": configuration,
        "maximumBitrate": 100_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "INPUT_250",
    })
}

/// A STANDARD RTP create — the kind of input most cases act on.
fn standard_create(targets: &Targets, name: &str) -> Value {
    create_body(
        name,
        standard(
            &targets.interface_arn,
            json!({ "Rtp": { "port": 5000, "forwardErrorCorrection": "ENABLED" } }),
        ),
    )
}

/// Create one router input through the wire and answer `(arn, the created RouterInput)`.
async fn create_input(emulator: &Emulator, body: Value) -> (String, Value) {
    let (status, code, created) = create_router_input(emulator, body).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "seeding create expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create answers an ARN: {created}"))
        .to_string();
    (arn, created["routerInput"].clone())
}

/// The router input the store holds for `arn`, read back through GetRouterInput.
async fn read_input(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, read) = get_router_input(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read back expected 200, got {status} {code:?}: {read}"
    );
    read["routerInput"].clone()
}

/// One update request, and what the stored input must report once it is applied.
struct UpdateCase {
    case: &'static str,
    /// The input this case acts on, created through the wire.
    create: Value,
    /// The body of the update.
    update: Value,
    /// The members the updated input must report DIFFERENTLY from the created one, as
    /// `(member, value)`. Every other member must be reported exactly as the create stored
    /// it, which is the half of the merge rule a request that omits a member exercises.
    changed: Vec<(&'static str, Value)>,
}

/// The rows span the class the merge rule generalises over: each updatable member on its own
/// (`Name`, `MaximumBitrate`, `Tier`, `RoutingScope`, `TransitEncryption`,
/// `MaintenanceConfiguration`, `ContentQualityAnalysisConfiguration`, `Configuration`), every
/// one of them at once, none of them at all, a STANDARD input reconfigured onto another
/// interface and another protocol, a FAILOVER input reconfigured in place, a MEDIACONNECT
/// FLOW input repointed at another flow's output, and a MEDIALIVE CHANNEL input disconnected
/// from the channel output it was connected to.
fn update_cases(targets: &Targets) -> Vec<UpdateCase> {
    let srt_listener = json!({ "SrtListener": { "port": 9000, "minimumLatencyMilliseconds": 120 } });
    let other_standard = standard(&targets.other_interface_arn, srt_listener.clone());
    let other_standard_stored =
        stored_standard(&targets.other_interface_arn, srt_listener, "SRT_LISTENER");
    let other_failover = failover(&targets.interface_arn, 5100, 5102);
    let other_flow = mediaconnect_flow(
        Some(&targets.other_flow_arn),
        Some(&targets.other_flow_output_arn),
        automatic_decryption(),
    );
    let disconnected_channel = medialive_channel(None, automatic_decryption());
    let preferred_maintenance = json!({ "PreferredDayTime": { "day": "TUESDAY", "time": "22:00" } });
    let analysed = json!({ "ContentLevel": {
        "blackFrames": { "state": "ENABLED", "thresholdSeconds": 10 },
        "silentAudio": { "state": "ENABLED", "thresholdSeconds": 60 },
    }});

    vec![
        UpdateCase {
            case: "Name alone",
            create: standard_create(targets, "ri-update-name"),
            update: json!({ "name": "ri-update-name-renamed" }),
            changed: vec![("name", json!("ri-update-name-renamed"))],
        },
        UpdateCase {
            case: "MaximumBitrate alone, inside the stored tier's ceiling",
            create: standard_create(targets, "ri-update-bitrate"),
            update: json!({ "maximumBitrate": 250_000_000i64 }),
            changed: vec![("maximumBitrate", json!(250_000_000i64))],
        },
        UpdateCase {
            case: "Tier alone, raised above the stored bitrate",
            create: standard_create(targets, "ri-update-tier"),
            update: json!({ "tier": "INPUT_500" }),
            changed: vec![("tier", json!("INPUT_500"))],
        },
        UpdateCase {
            case: "Tier and MaximumBitrate lowered together",
            create: standard_create(targets, "ri-update-tier-down"),
            update: json!({ "tier": "INPUT_20", "maximumBitrate": 20_000_000i64 }),
            changed: vec![
                ("tier", json!("INPUT_20")),
                ("maximumBitrate", json!(20_000_000i64)),
            ],
        },
        UpdateCase {
            case: "RoutingScope alone",
            create: standard_create(targets, "ri-update-scope"),
            update: json!({ "routingScope": "GLOBAL" }),
            changed: vec![("routingScope", json!("GLOBAL"))],
        },
        UpdateCase {
            case: "TransitEncryption alone, onto a key the caller holds",
            create: standard_create(targets, "ri-update-transit"),
            update: json!({ "transitEncryption": secrets_manager_decryption() }),
            changed: vec![("transitEncryption", secrets_manager_decryption())],
        },
        UpdateCase {
            case: "MaintenanceConfiguration alone, onto the other union member",
            create: standard_create(targets, "ri-update-maintenance"),
            update: json!({ "maintenanceConfiguration": preferred_maintenance.clone() }),
            changed: vec![
                ("maintenanceConfiguration", preferred_maintenance),
                (
                    "maintenanceType",
                    json!(mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME),
                ),
            ],
        },
        UpdateCase {
            case: "ContentQualityAnalysisConfiguration alone",
            create: standard_create(targets, "ri-update-cqa"),
            update: json!({ "contentQualityAnalysisConfiguration": analysed.clone() }),
            changed: vec![("contentQualityAnalysisConfiguration", analysed)],
        },
        UpdateCase {
            case: "Configuration onto another interface and another protocol",
            create: standard_create(targets, "ri-update-standard"),
            update: json!({ "configuration": other_standard }),
            changed: vec![("configuration", other_standard_stored)],
        },
        UpdateCase {
            case: "a FAILOVER input reconfigured on new ports",
            create: create_body(
                "ri-update-failover",
                failover(&targets.interface_arn, 5002, 5004),
            ),
            update: json!({ "configuration": other_failover.clone() }),
            changed: vec![("configuration", other_failover)],
        },
        UpdateCase {
            case: "a MEDIACONNECT_FLOW input repointed at another flow's output",
            create: create_body(
                "ri-update-flow",
                mediaconnect_flow(
                    Some(&targets.flow_arn),
                    Some(&targets.flow_output_arn),
                    automatic_decryption(),
                ),
            ),
            update: json!({ "configuration": other_flow.clone() }),
            changed: vec![("configuration", other_flow)],
        },
        UpdateCase {
            case: "a MEDIALIVE_CHANNEL input disconnected from its channel output",
            create: create_body(
                "ri-update-channel",
                medialive_channel(Some("out-0"), automatic_decryption()),
            ),
            update: json!({ "configuration": disconnected_channel.clone() }),
            changed: vec![("configuration", disconnected_channel)],
        },
        UpdateCase {
            case: "every updatable member at once",
            create: standard_create(targets, "ri-update-everything"),
            update: json!({
                "name": "ri-update-everything-renamed",
                "configuration": standard(&targets.other_interface_arn,
                    json!({ "Rist": { "port": 4000, "recoveryLatencyMilliseconds": 200 } })),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "GLOBAL",
                "tier": "INPUT_20",
                "transitEncryption": secrets_manager_decryption(),
                "maintenanceConfiguration": { "PreferredDayTime": { "day": "SUNDAY",
                    "time": "03:00" } },
                "contentQualityAnalysisConfiguration": { "ContentLevel": {
                    "frozenFrames": { "state": "DISABLED", "thresholdSeconds": 30 } } },
            }),
            changed: vec![
                ("name", json!("ri-update-everything-renamed")),
                (
                    "configuration",
                    stored_standard(
                        &targets.other_interface_arn,
                        json!({ "Rist": { "port": 4000, "recoveryLatencyMilliseconds": 200 } }),
                        "RIST",
                    ),
                ),
                ("maximumBitrate", json!(20_000_000i64)),
                ("routingScope", json!("GLOBAL")),
                ("tier", json!("INPUT_20")),
                ("transitEncryption", secrets_manager_decryption()),
                (
                    "maintenanceConfiguration",
                    json!({ "PreferredDayTime": { "day": "SUNDAY", "time": "03:00" } }),
                ),
                (
                    "maintenanceType",
                    json!(mc::MAINTENANCE_TYPE_PREFERRED_DAY_TIME),
                ),
                (
                    "contentQualityAnalysisConfiguration",
                    json!({ "ContentLevel": {
                        "frozenFrames": { "state": "DISABLED", "thresholdSeconds": 30 } } }),
                ),
            ],
        },
        UpdateCase {
            case: "no updatable member at all",
            create: standard_create(targets, "ri-update-nothing"),
            update: json!({}),
            changed: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn update_router_input_applies_what_it_carries_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    for UpdateCase {
        case,
        create,
        update,
        changed,
    } in update_cases(&targets)
    {
        let (arn, created) = create_input(&emulator, create).await;

        let (status, code, answered) = update_router_input(&emulator, &arn, update).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
        );
        let updated = answered["routerInput"].clone();

        // Every member of the updated input: the value this request asked for where it asked
        // for one, and the value the create stored where it did not. `UpdatedAt` is the one
        // member the CALL moves rather than the request, asserted below.
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
                "case {case:?}: the updated router input's {member} must be {expected}, got {:?}",
                updated[member]
            );
        }

        // The update changed the resource, so its `UpdatedAt` is read from the clock and is no
        // earlier than the `CreatedAt` the create stored, which the update leaves alone.
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
        let stored = read_input(&emulator, case, &arn).await;
        assert_eq!(
            stored, updated,
            "case {case:?}: GetRouterInput must report the record the update stored, got \
             {stored}"
        );
    }
}

#[tokio::test]
async fn an_updated_input_reports_updating_until_the_clock_settles_it() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // The rows vary where the input stood when the update arrived: settled in the state its
    // create owed it, settled in a state a start left it in, and still being created — a
    // reconfiguration always comes back to the state the input was HEADING for. Each is
    // `(case, the state to settle the input in first, the state the clock must return it to)`.
    for (case, settle_in, settles_back_to) in [
        (
            "an input settled in STANDBY",
            Some(mc::ROUTER_INPUT_STATE_STANDBY),
            mc::ROUTER_INPUT_STATE_STANDBY,
        ),
        (
            "an input running in ACTIVE",
            Some(mc::ROUTER_INPUT_STATE_ACTIVE),
            mc::ROUTER_INPUT_STATE_ACTIVE,
        ),
        (
            "an input still being created",
            None,
            mc::ROUTER_INPUT_STATE_STANDBY,
        ),
    ] {
        let (arn, _) =
            create_input(&emulator, standard_create(&targets, "ri-update-lifecycle")).await;
        if let Some(state) = settle_in {
            settle_router_input_in(&emulator, &arn, state);
            let settled = read_input(&emulator, case, &arn).await;
            assert_eq!(
                settled["state"].as_str(),
                Some(state),
                "case {case:?}: the input under test starts settled in {state}, got {settled}"
            );
        }

        let (status, code, answered) =
            update_router_input(&emulator, &arn, json!({ "name": "ri-update-lifecycle-2" })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: the update expected 202, got {status} {code:?}: {answered}"
        );
        assert_eq!(
            answered["routerInput"]["state"].as_str(),
            Some(mc::ROUTER_INPUT_STATE_UPDATING),
            "case {case:?}: an accepted update answers the transient UPDATING state, got \
             {answered}"
        );

        // A caller reading straight after the update still observes that transient state.
        let in_flight = read_input(&emulator, case, &arn).await;
        assert_eq!(
            in_flight["state"].as_str(),
            Some(mc::ROUTER_INPUT_STATE_UPDATING),
            "case {case:?}: an input read straight after the update is still UPDATING, got \
             {in_flight}"
        );

        // And once the transition the clock owes is due, the input is back in the state it was
        // heading for, with everything the update stored still in place.
        tokio::time::sleep(past_transition_span()).await;
        let settled = read_input(&emulator, case, &arn).await;
        assert_eq!(
            settled["state"].as_str(),
            Some(settles_back_to),
            "case {case:?}: the clock settles the updated input into {settles_back_to}, got \
             {settled}"
        );
        for member in PERSISTED_MEMBERS {
            assert_eq!(
                settled[member], answered["routerInput"][member],
                "case {case:?}: settling the state leaves {member} as the update stored it, got \
                 {:?}",
                settled[member]
            );
        }
    }
}

#[tokio::test]
async fn the_tier_bounds_the_maximum_bitrate_an_update_may_set() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // The catalog is `CreateRouterInput$Tier`'s own documentation
    // (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")`), which
    // `UpdateRouterInput$Tier` repeats: the tier "sets the maximum bitrate, in Mbps, for the
    // router input" — INPUT_20 20 Mbps, INPUT_50 50, INPUT_100 100, INPUT_250 250, INPUT_500
    // 500 — which is every tier the enum declares. Each is driven at its published bound and
    // one bit per second past it.
    for (tier, megabits_per_second) in [
        ("INPUT_20", 20i64),
        ("INPUT_50", 50),
        ("INPUT_100", 100),
        ("INPUT_250", 250),
        ("INPUT_500", 500),
    ] {
        let ceiling = megabits_per_second * mc::BITS_PER_MEGABIT;
        assert_eq!(
            mc::router_input_tier_bitrate_ceiling(tier),
            Some(ceiling),
            "case {tier:?}: the crate's ceiling must be the documented {megabits_per_second} Mbps"
        );

        let (arn, created) = create_input(
            &emulator,
            standard_create(&targets, &format!("ri-bound-{}", tier.to_ascii_lowercase())),
        )
        .await;

        let (status, code, past_bound) = update_router_input(
            &emulator,
            &arn,
            json!({ "tier": tier, "maximumBitrate": ceiling + 1 }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {tier:?}: one bit per second past the tier's ceiling is refused, got {status} \
             {code:?}: {past_bound}"
        );
        assert!(
            past_bound["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {tier:?}: the refusal carries this service's validation envelope, got \
             {past_bound}"
        );
        let refused = read_input(&emulator, tier, &arn).await;
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

        let (status, code, at_bound) = update_router_input(
            &emulator,
            &arn,
            json!({ "tier": tier, "maximumBitrate": ceiling }),
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
                at_bound["routerInput"]["tier"].as_str(),
                at_bound["routerInput"]["maximumBitrate"].as_i64()
            ),
            (Some(tier), Some(ceiling)),
            "case {tier:?}: the stored tier and bitrate are the ones requested, got {at_bound}"
        );
    }
}

/// One update this service must refuse, and the modelled failure it answers with.
struct RefusalCase {
    case: &'static str,
    /// Which created input the request addresses: `Some(index)` into the inputs the test
    /// creates, or `None` for an ARN nothing holds.
    input: Option<usize>,
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

    // Two inputs of DIFFERENT types, so a configuration-type change can be attempted in both
    // directions.
    let (standard_arn, standard_created) =
        create_input(&emulator, standard_create(&targets, "ri-refusal-standard")).await;
    let (flow_input_arn, flow_created) = create_input(
        &emulator,
        create_body(
            "ri-refusal-flow",
            mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&targets.flow_output_arn),
                automatic_decryption(),
            ),
        ),
    )
    .await;
    let arns = [standard_arn, flow_input_arn];
    let created = [standard_created, flow_created];

    // A well-formed `routerNetworkInterface` ARN no create minted, so the account holds no
    // interface for it.
    let unheld_interface =
        "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:ffffffffffff";

    let cases = vec![
        RefusalCase {
            case: "an ARN no create minted",
            input: None,
            body: json!({ "name": "ri-refusal-absent" }),
            status: 404,
            code: "NotFoundException",
            envelope: None,
        },
        RefusalCase {
            case: "replacing a STANDARD configuration with a MEDIACONNECT_FLOW one",
            input: Some(0),
            body: json!({ "configuration": mediaconnect_flow(
                Some(&targets.other_flow_arn),
                Some(&targets.other_flow_output_arn),
                automatic_decryption(),
            )}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::BAD_REQUEST_PREFIX),
        },
        RefusalCase {
            case: "replacing a MEDIACONNECT_FLOW configuration with a STANDARD one",
            input: Some(1),
            body: json!({ "configuration": standard(&targets.interface_arn,
                json!({ "Rtp": { "port": 5000 } }))}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::BAD_REQUEST_PREFIX),
        },
        RefusalCase {
            case: "replacing a STANDARD configuration with a FAILOVER one",
            input: Some(0),
            body: json!({ "configuration": failover(&targets.interface_arn, 5200, 5202) }),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::BAD_REQUEST_PREFIX),
        },
        RefusalCase {
            case: "a Configuration ingesting on an interface the account does not hold",
            input: Some(0),
            body: json!({ "configuration": standard(unheld_interface,
                json!({ "Rtp": { "port": 5000 } }))}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a Configuration naming a flow output the account does not hold",
            input: Some(1),
            body: json!({ "configuration": mediaconnect_flow(
                Some(&targets.flow_arn),
                Some("arn:aws:mediaconnect:us-east-1:000000000000:output:1-0000000f:absent"),
                automatic_decryption(),
            )}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a MaximumBitrate of zero is not a bitrate",
            input: Some(0),
            body: json!({ "maximumBitrate": 0i64 }),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a negative MaximumBitrate is not a bitrate",
            input: Some(0),
            body: json!({ "maximumBitrate": -20_000_000i64 }),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a Tier lowered under the bitrate the input already carries",
            input: Some(0),
            body: json!({ "tier": "INPUT_20" }),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a maintenance preference whose Time names no time of day",
            input: Some(0),
            body: json!({ "maintenanceConfiguration": { "PreferredDayTime": {
                "day": "MONDAY", "time": "midnight" } }}),
            status: 400,
            code: "BadRequestException",
            envelope: Some(mc::VALIDATION_FAILED_PREFIX),
        },
        RefusalCase {
            case: "a Tier outside the modelled value set",
            input: Some(0),
            body: json!({ "tier": "INPUT_1000" }),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a RoutingScope outside the modelled value set",
            input: Some(0),
            body: json!({ "routingScope": "PLANETARY" }),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a Name past the modelled 128-character maximum",
            input: Some(0),
            body: json!({ "name": "n".repeat(129) }),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
        RefusalCase {
            case: "a Name under the modelled 1-character minimum",
            input: Some(0),
            body: json!({ "name": "" }),
            status: 400,
            code: "BadRequestException",
            envelope: None,
        },
    ];

    for RefusalCase {
        case,
        input,
        body,
        status: expected_status,
        code: expected_code,
        envelope,
    } in cases
    {
        let arn = input.map_or(UNKNOWN_ARN, |index| arns[index].as_str());
        let (status, code, refusal) = update_router_input(&emulator, arn, body).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {refusal}"
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
        if let Some(index) = input {
            let stored = read_input(&emulator, case, arn).await;
            for member in PERSISTED_MEMBERS {
                assert_eq!(
                    stored[member], created[index][member],
                    "case {case:?}: a refused update stores nothing, so {member} is still what \
                     the create stored, got {:?}",
                    stored[member]
                );
            }
        }
    }

    // The ARN nothing holds is reported with this service's not-found envelope, and the
    // refusal created no record for it.
    let (_, _, refusal) =
        update_router_input(&emulator, UNKNOWN_ARN, json!({ "name": "ri-absent" })).await;
    assert_eq!(
        refusal["message"].as_str(),
        Some(mc::not_found_message(UNKNOWN_ARN).as_str()),
        "the not-found refusal carries this service's not-found envelope, got {refusal}"
    );
    let (status, code, read) = get_router_input(&emulator, UNKNOWN_ARN).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "a refused update on an absent ARN creates no router input, got {status} {code:?}: \
         {read}"
    );
}

#[tokio::test]
async fn an_update_is_refused_while_the_input_is_being_deleted() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;
    let (arn, created) = create_input(&emulator, standard_create(&targets, "ri-deleting")).await;

    let (status, code, deleted) = delete_router_input(&emulator, &arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete expected 200, got {status} {code:?}: {deleted}"
    );
    assert_eq!(
        deleted["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_DELETING),
        "the delete leaves the input in DELETING, got {deleted}"
    );

    // An input on its way out of the store has nothing left to reconfigure.
    let rename = json!({ "name": "ri-deleting-renamed" });
    let (status, code, refusal) = update_router_input(&emulator, &arn, rename.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "an input being deleted refuses an update, got {status} {code:?}: {refusal}"
    );
    let stored = read_input(&emulator, "being deleted", &arn).await;
    assert_eq!(
        stored["name"], created["name"],
        "the refused update left the stored Name alone, got {stored}"
    );
    assert_eq!(
        stored["state"].as_str(),
        Some(mc::ROUTER_INPUT_STATE_DELETING),
        "the refused update left the input in the state its delete put it in, got {stored}"
    );

    // And once the clock has completed the deletion there is no record at all, so the same
    // request is a not-found rather than a conflict.
    tokio::time::sleep(past_transition_span()).await;
    let (status, code, refusal) = update_router_input(&emulator, &arn, rename).await;
    assert_eq!(
        (status, code.as_deref()),
        (404, Some("NotFoundException")),
        "an input the clock has removed is a not-found, got {status} {code:?}: {refusal}"
    );
    assert_eq!(
        refusal["message"].as_str(),
        Some(mc::not_found_message(&arn).as_str()),
        "the not-found refusal carries this service's not-found envelope, got {refusal}"
    );
}

/// One row of the connected-source rule: the configuration the input is created with, the
/// replacement that changes only its `sourceTransitDecryption`, and the replacement that
/// DISCONNECTS the input while keeping the decryption it already had.
struct DecryptionCase {
    case: &'static str,
    connected: Value,
    changed_decryption: Value,
    disconnected: Value,
}

#[tokio::test]
async fn the_source_transit_decryption_cannot_change_while_the_input_is_connected() {
    let emulator = Emulator::new();
    let targets = Targets::mint(&emulator).await;

    // Both configurations that take their content from a resource of another service carry a
    // `sourceTransitDecryption` and document the same precondition, so both are rows.
    let cases = vec![
        DecryptionCase {
            case: "a MEDIACONNECT_FLOW input connected to a flow output",
            connected: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&targets.flow_output_arn),
                automatic_decryption(),
            ),
            changed_decryption: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&targets.flow_output_arn),
                secrets_manager_decryption(),
            ),
            disconnected: mediaconnect_flow(None, None, automatic_decryption()),
        },
        DecryptionCase {
            case: "a MEDIALIVE_CHANNEL input connected to a channel output",
            connected: medialive_channel(Some("out-0"), automatic_decryption()),
            changed_decryption: medialive_channel(
                Some("out-0"),
                secrets_manager_decryption(),
            ),
            disconnected: medialive_channel(None, automatic_decryption()),
        },
    ];

    for DecryptionCase {
        case,
        connected,
        changed_decryption,
        disconnected,
    } in cases
    {
        let (arn, created) = create_input(
            &emulator,
            create_body("ri-connected", connected.clone()),
        )
        .await;

        // "You can't change sourceTransitDecryption while the router input is connected."
        let (status, code, refusal) =
            update_router_input(&emulator, &arn, json!({ "configuration": changed_decryption.clone() }))
                .await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: a connected input refuses a decryption change, got {status} \
             {code:?}: {refusal}"
        );
        let stored = read_input(&emulator, case, &arn).await;
        assert_eq!(
            stored["configuration"], created["configuration"],
            "case {case:?}: the refused update left the stored Configuration alone, got {stored}"
        );

        // The refusal is about that ONE member: the same input takes a replacement that keeps
        // the decryption it already had.
        let (status, code, answered) =
            update_router_input(&emulator, &arn, json!({ "configuration": connected })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a connected input takes a replacement that leaves its decryption \
             alone, got {status} {code:?}: {answered}"
        );

        // "To change it, first disconnect the router input, then update the decryption" — and
        // once it is disconnected, the same change is accepted.
        let (status, code, answered) =
            update_router_input(&emulator, &arn, json!({ "configuration": disconnected })).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: disconnecting the input is accepted, got {status} {code:?}: \
             {answered}"
        );
        let (status, code, answered) =
            update_router_input(&emulator, &arn, json!({ "configuration": changed_decryption.clone() }))
                .await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: a disconnected input takes the decryption change, got {status} \
             {code:?}: {answered}"
        );
        assert_eq!(
            answered["routerInput"]["configuration"], changed_decryption,
            "case {case:?}: the accepted update stored the new decryption, got {answered}"
        );
    }
}

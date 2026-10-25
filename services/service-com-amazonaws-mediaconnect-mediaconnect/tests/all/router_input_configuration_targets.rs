// SPDX-License-Identifier: CC0-1.0

//! What a router input's `Configuration` may POINT AT: CreateRouterInput resolves every
//! resource of THIS service a configuration names against the store, and refuses one the
//! account does not hold with `BadRequestException` (400), storing nothing.
//!
//! The configuration is what the input INGESTS ON — a STANDARD, FAILOVER or MERGE input
//! ingests on a router network interface (`NetworkInterfaceArn`, required on each, a
//! `routerNetworkInterface` ARN this service mints in CreateRouterNetworkInterface), a
//! MEDIACONNECT FLOW input takes its content from a flow and one of that flow's outputs
//! (`MediaConnectFlowRouterInputConfiguration$FlowArn` / `$FlowOutputArn`, both optional
//! because the configuration is kept while the input is disconnected) — so a reference that
//! does not exist is a configuration the service cannot store: the input would ingest
//! nothing, and the association could never be read back from the interface it claims
//! (`RouterNetworkInterface$AssociatedInputCount`).
//!
//! The rows span what the rule generalises over: each configuration variant, each member
//! that carries such a reference, whether the reference is held, absent or held by ANOTHER
//! resource, and the ways a well-formed ARN can name nothing (an id no create minted,
//! another Region, another account, another partition). A MEDIALIVE CHANNEL configuration
//! names a resource of another service, which this store does not hold, so it is accepted as
//! given — the row is there to pin that the rule reaches only this service's resources. The
//! last suite proves the verdict comes from the STORE rather than from the ARN's shape: a
//! reference the account deletes stops resolving, and the association count the accepted
//! input leaves on its interface is the other side of the same relation.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    create_body, create_router_input, get_router_input, union, union_member,
};
use crate::router_output_configuration_targets::unheld_interface_arns;
use crate::router_outputs::minted_network_interface;

/// A `MediaLiveChannelArn` matching the modelled pattern
/// (`smithy_query(query="shape",shape="MediaLiveChannelRouterInputConfiguration")`:
/// `^arn:(aws[a-zA-Z-]*):medialive:[a-z0-9-]+:[0-9]{12}:channel:[a-zA-Z0-9]+$`). The channel
/// belongs to ANOTHER service, which this store does not hold, so nothing here resolves it.
const MEDIALIVE_CHANNEL_ARN: &str = "arn:aws:medialive:us-east-1:000000000000:channel:1234567";

/// The `sourceTransitDecryption` every MEDIACONNECT FLOW / MEDIALIVE CHANNEL configuration
/// requires.
fn automatic_decryption() -> Value {
    json!({
        "encryptionKeyType": "AUTOMATIC",
        "encryptionKeyConfiguration": union("Automatic", "automatic", json!({})),
    })
}

/// A STANDARD configuration ingesting on `interface_arn`.
fn standard(interface_arn: &str) -> Value {
    union(
        "Standard",
        "standard",
        json!({
            "networkInterfaceArn": interface_arn,
            "protocol": "RTP",
            "protocolConfiguration": union("Rtp", "rtp", json!({ "port": 5000 })),
        }),
    )
}

/// A FAILOVER configuration ingesting on `interface_arn`: "a list of exactly two protocol
/// configurations for the failover input sources".
fn failover(interface_arn: &str) -> Value {
    union(
        "Failover",
        "failover",
        json!({
            "networkInterfaceArn": interface_arn,
            "sourcePriorityMode": "PRIMARY_SECONDARY",
            "primarySourceIndex": 0,
            "protocolConfigurations": [
                union("Rtp", "rtp", json!({ "port": 5002 })),
                union("Rtp", "rtp", json!({ "port": 5004 })),
            ],
        }),
    )
}

/// A MERGE configuration ingesting on `interface_arn`.
fn merge(interface_arn: &str) -> Value {
    union(
        "Merge",
        "merge",
        json!({
            "networkInterfaceArn": interface_arn,
            "mergeRecoveryWindowMilliseconds": 500,
            "protocolConfigurations": [
                union("Rtp", "rtp", json!({ "port": 5006 })),
                union("Rtp", "rtp", json!({ "port": 5008 })),
            ],
        }),
    )
}

/// A MEDIACONNECT FLOW configuration naming `flow_arn` and `flow_output_arn`, each only
/// where the case carries one: the model makes both optional, so a configuration may name
/// the flow, the output, both or neither.
fn mediaconnect_flow(flow_arn: Option<&str>, flow_output_arn: Option<&str>) -> Value {
    let mut payload = json!({ "sourceTransitDecryption": automatic_decryption() });
    if let Some(flow_arn) = flow_arn {
        payload["flowArn"] = json!(flow_arn);
    }
    if let Some(flow_output_arn) = flow_output_arn {
        payload["flowOutputArn"] = json!(flow_output_arn);
    }
    union("MediaConnectFlow", "mediaConnectFlow", payload)
}

/// A MEDIALIVE CHANNEL configuration — the variant whose content comes from another service.
fn medialive_channel() -> Value {
    union(
        "MediaLiveChannel",
        "mediaLiveChannel",
        json!({
            "mediaLiveChannelArn": MEDIALIVE_CHANNEL_ARN,
            "mediaLivePipelineId": "PIPELINE_0",
            "sourceTransitDecryption": automatic_decryption(),
        }),
    )
}

/// Every resource of THIS service a router input configuration can name, minted over the
/// wire on one emulator: the router network interface a STANDARD / FAILOVER / MERGE input
/// ingests on, and the flow and flow output a MEDIACONNECT FLOW input takes its content
/// from.
struct InputTargets {
    interface_arn: String,
    flow_arn: String,
    flow_output_arn: String,
}

/// Mint one flow carrying one output over the wire and answer `(FlowArn, OutputArn)`.
///
/// `crate::router_outputs::minted_flow_source` mints the flow a router OUTPUT delivers into
/// (a flow SOURCE); a router INPUT takes its content from a flow OUTPUT, so the flow this
/// mints carries one.
async fn minted_flow_output(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "source": { "name": format!("{name}-src"), "protocol": "rtp",
                "whitelistCidr": "10.0.0.0/16" },
            "outputs": [{ "name": format!("{name}-out"), "protocol": "rtp-fec",
                "destination": "203.0.113.30", "port": 5000 }],
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateFlow expected 201 for {name:?}, got {status} {code:?}: {created}"
    );
    let flow_arn = created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the minted flow carries its ARN: {created}"))
        .to_string();
    let output_arn = created["flow"]["outputs"][0]["outputArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the minted flow's output carries its ARN: {created}"))
        .to_string();
    (flow_arn, output_arn)
}

/// Mint the whole set [`InputTargets`] describes, naming each resource after `prefix` so one
/// emulator can hold several sets.
async fn minted_input_targets(emulator: &Emulator, prefix: &str) -> InputTargets {
    let interface_arn = minted_network_interface(emulator, &format!("rni-{prefix}")).await;
    let (flow_arn, flow_output_arn) =
        minted_flow_output(emulator, &format!("flow-{prefix}")).await;
    InputTargets {
        interface_arn,
        flow_arn,
        flow_output_arn,
    }
}

/// One configuration, and whether the service may store it.
struct TargetCase {
    case: String,
    configuration: Value,
    /// Whether every reference the configuration names resolves, so the request is accepted.
    accepted: bool,
    /// A fragment of the member name the refusal must state, for a refused row.
    member: &'static str,
}

/// Every row of the class, built against the targets `targets` holds and the second flow
/// `other` holds, so a case can name an output that belongs to ANOTHER flow.
fn target_cases(targets: &InputTargets, other: &InputTargets) -> Vec<TargetCase> {
    let unheld_flow = "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000f:absent";
    let unheld_output =
        "arn:aws:mediaconnect:us-east-1:000000000000:output:1-0000000f:absent-output";

    let mut cases = vec![
        TargetCase {
            case: "STANDARD on a router network interface the account holds".to_string(),
            configuration: standard(&targets.interface_arn),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "FAILOVER on a router network interface the account holds".to_string(),
            configuration: failover(&targets.interface_arn),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MERGE on a router network interface the account holds".to_string(),
            configuration: merge(&targets.interface_arn),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a flow and an output of that flow".to_string(),
            configuration: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&targets.flow_output_arn),
            ),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming the flow alone".to_string(),
            configuration: mediaconnect_flow(Some(&targets.flow_arn), None),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming the output alone".to_string(),
            configuration: mediaconnect_flow(None, Some(&targets.flow_output_arn)),
            accepted: true,
            member: "",
        },
        TargetCase {
            // "To disconnect it, call the update operation and omit both fields while
            // keeping this configuration object"
            // (`smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`).
            case: "MEDIACONNECT FLOW naming neither, the disconnected form".to_string(),
            configuration: mediaconnect_flow(None, None),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIALIVE CHANNEL, whose content comes from another service".to_string(),
            configuration: medialive_channel(),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a flow no create minted".to_string(),
            configuration: mediaconnect_flow(Some(unheld_flow), None),
            accepted: false,
            member: "FlowArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a flow no create minted and a held output"
                .to_string(),
            configuration: mediaconnect_flow(
                Some(unheld_flow),
                Some(&targets.flow_output_arn),
            ),
            accepted: false,
            member: "FlowArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a held flow and an output no create minted"
                .to_string(),
            configuration: mediaconnect_flow(Some(&targets.flow_arn), Some(unheld_output)),
            accepted: false,
            member: "FlowOutputArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a held flow and an output of ANOTHER flow"
                .to_string(),
            configuration: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&other.flow_output_arn),
            ),
            accepted: false,
            member: "FlowOutputArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming an output no create minted, with no flow"
                .to_string(),
            configuration: mediaconnect_flow(None, Some(unheld_output)),
            accepted: false,
            member: "FlowOutputArn",
        },
    ];
    // Every configuration that carries a `NetworkInterfaceArn`, against every way a
    // well-formed interface ARN can name nothing this account holds.
    for (case, arn) in unheld_interface_arns() {
        for (variant, configuration) in [
            ("STANDARD", standard(&arn)),
            ("FAILOVER", failover(&arn)),
            ("MERGE", merge(&arn)),
        ] {
            cases.push(TargetCase {
                case: format!("{variant} on {case}"),
                configuration,
                accepted: false,
                member: "NetworkInterfaceArn",
            });
        }
    }
    cases
}

/// How many router inputs the store holds — a refused request must leave it where it was.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_inputs
        .len()
}

/// A name no other case uses, derived from the row so a failure names the row.
fn case_name(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{slug}")
}

/// Every ARN the configuration the store holds for `arn` names, read back through
/// GetRouterInput: the interface a STANDARD / FAILOVER / MERGE input ingests on, and the
/// flow and flow output a MEDIACONNECT FLOW input takes its content from.
async fn stored_references(emulator: &Emulator, case: &str, arn: &str) -> Vec<String> {
    let (status, code, read) = get_router_input(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read back expected 200, got {status} {code:?}: {read}"
    );
    let configuration = read["routerInput"]["configuration"].clone();
    let mut references: Vec<String> = Vec::new();
    for (member_name, json_name, members) in [
        ("Standard", "standard", &["networkInterfaceArn"][..]),
        ("Failover", "failover", &["networkInterfaceArn"][..]),
        ("Merge", "merge", &["networkInterfaceArn"][..]),
        (
            "MediaConnectFlow",
            "mediaConnectFlow",
            &["flowArn", "flowOutputArn"][..],
        ),
    ] {
        let payload = union_member(&configuration, member_name, json_name);
        for member in members {
            if let Some(value) = payload.get(member).and_then(Value::as_str) {
                references.push(value.to_string());
            }
        }
    }
    references
}

/// A create resolves every reference of this service its configuration names: one the
/// account HOLDS is stored and read back, one it does not is refused with
/// `BadRequestException` (400) naming the member, and nothing is stored for it.
#[tokio::test]
async fn a_create_resolves_every_reference_its_configuration_names() {
    let emulator = Emulator::new();
    let targets = minted_input_targets(&emulator, "ri-targets").await;
    let other = minted_input_targets(&emulator, "ri-targets-other").await;

    for TargetCase {
        case,
        configuration,
        accepted,
        member,
    } in target_cases(&targets, &other)
    {
        let before = stored_count(&emulator);
        let name = case_name("ri", &case);
        let (status, code, answered) =
            create_router_input(&emulator, create_body(&name, configuration.clone())).await;

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a configuration naming a resource the account does not hold \
                 is refused, got {status} {code:?}: {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default().to_string();
            assert!(
                message.starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal carries this service's validation envelope, got \
                 {message:?}"
            );
            assert!(
                message.contains(member),
                "case {case:?}: the refusal names the member it could not resolve ({member}), \
                 got {message:?}"
            );
            assert_eq!(
                stored_count(&emulator),
                before,
                "case {case:?}: a refused create stores nothing, got {} stored",
                stored_count(&emulator)
            );
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {case:?}: a configuration whose references the account holds is accepted, \
             got {status} {code:?}: {answered}"
        );
        let arn = answered["routerInput"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
            .to_string();
        assert_eq!(
            stored_count(&emulator),
            before + 1,
            "case {case:?}: the accepted create stored one router input, got {} stored",
            stored_count(&emulator)
        );

        // The accepted configuration is the one the store now holds, every reference as the
        // request named it.
        let stored = stored_references(&emulator, &case, &arn).await;
        let expected: Vec<String> = [
            Some(targets.interface_arn.as_str())
                .filter(|_| configuration.get("Standard").is_some()
                    || configuration.get("Failover").is_some()
                    || configuration.get("Merge").is_some()),
            configuration["MediaConnectFlow"]
                .get("flowArn")
                .and_then(Value::as_str),
            configuration["MediaConnectFlow"]
                .get("flowOutputArn")
                .and_then(Value::as_str),
        ]
        .into_iter()
        .flatten()
        .map(str::to_string)
        .collect();
        assert_eq!(
            stored, expected,
            "case {case:?}: the read reports every reference the create stored, got {stored:?}"
        );
    }
}

/// The verdict is read from the STORE, not from the ARN's shape: the very ARN a create
/// accepted stops resolving once the account no longer holds the resource — the interface
/// after its deletion has come due, and the flow (with its outputs) as soon as it is
/// deleted. The accepted input is the other side of the same relation while it lasts, so the
/// interface it ingests on counts it.
#[tokio::test]
async fn a_reference_the_account_no_longer_holds_is_refused() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ri-gone").await;
    let (flow_arn, flow_output_arn) = minted_flow_output(&emulator, "flow-ri-gone").await;

    // While the account holds them, both configurations are accepted.
    for (name, configuration) in [
        ("ri-gone-standard", standard(&interface_arn)),
        (
            "ri-gone-flow",
            mediaconnect_flow(Some(&flow_arn), Some(&flow_output_arn)),
        ),
    ] {
        let (status, code, created) =
            create_router_input(&emulator, create_body(name, configuration)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "the create of {name:?} expected 201, got {status} {code:?}: {created}"
        );
    }

    // The association is readable from the interface the accepted input ingests on, which is
    // what a dangling reference could never be.
    let (status, _, read) = emulator
        .send(
            "GET",
            &format!("/v1/routerNetworkInterfaces?arns={interface_arn}"),
            None,
        )
        .await;
    assert_eq!(
        (status, read["routerNetworkInterfaces"][0]["associatedInputCount"].as_i64()),
        (200, Some(1)),
        "the interface the accepted input ingests on counts it, got {read}"
    );

    // The interface leaves the store one transition span after its delete is accepted.
    let (status, code, deleted) = emulator
        .send(
            "DELETE",
            &format!("/v1/routerNetworkInterface/{interface_arn}"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "DeleteRouterNetworkInterface expected 200, got {status} {code:?}: {deleted}"
    );
    tokio::time::sleep(mc::TRANSITION_SPAN * 2).await;

    let (status, code, flow_deleted) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "DeleteFlow expected 202, got {status} {code:?}: {flow_deleted}"
    );

    let before = stored_count(&emulator);
    for (case, configuration, member) in [
        (
            "the interface the account deleted",
            standard(&interface_arn),
            "NetworkInterfaceArn",
        ),
        (
            "the flow the account deleted",
            mediaconnect_flow(Some(&flow_arn), Some(&flow_output_arn)),
            "FlowArn",
        ),
        (
            "the output of the flow the account deleted",
            mediaconnect_flow(None, Some(&flow_output_arn)),
            "FlowOutputArn",
        ),
    ] {
        let (status, code, refused) = create_router_input(
            &emulator,
            create_body(&case_name("ri-gone", case), configuration),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a configuration naming it is refused once it is gone, got \
             {status} {code:?}: {refused}"
        );
        let message = refused["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX) && message.contains(member),
            "case {case:?}: the refusal carries this service's validation envelope and names \
             {member}, got {message:?}"
        );
    }
    assert_eq!(
        stored_count(&emulator),
        before,
        "the refused creates stored nothing, got {} stored",
        stored_count(&emulator)
    );
}

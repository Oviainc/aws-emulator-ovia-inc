// SPDX-License-Identifier: CC0-1.0

//! What a router output's `Configuration` may POINT AT: CreateRouterOutput and
//! UpdateRouterOutput resolve every resource of THIS service a configuration names against
//! the store, and refuse one the account does not hold with `BadRequestException` (400),
//! storing nothing.
//!
//! The configuration is what the output egresses to — a STANDARD output egresses from a
//! router network interface (`StandardRouterOutputConfiguration$NetworkInterfaceArn`,
//! required, a `routerNetworkInterface` ARN this service mints), a MEDIACONNECT FLOW output
//! delivers into a flow and one of that flow's sources
//! (`MediaConnectFlowRouterOutputConfiguration$FlowArn` / `$FlowSourceArn`, both optional
//! because the configuration is kept while the output is disconnected) — so a target that
//! does not exist is a configuration the service cannot store.
//!
//! The rows span what the rule generalises over: each configuration variant, each member
//! that carries such a reference, whether the reference is held, absent or held by ANOTHER
//! resource, and the ways a well-formed ARN can name nothing (an id no create minted,
//! another Region, another account, another partition). A MEDIALIVE INPUT configuration
//! names a resource of another service, which this store does not hold, so it is accepted
//! as given — the row is there to pin that the rule reaches only this service's resources.
//! The last two suites prove the verdict comes from the STORE rather than from the ARN's
//! shape: a target the account deletes stops being one, and both the create and the update
//! answer the same way for the same configuration.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_outputs::{
    create_router_output, minted_configuration_targets, minted_flow_source,
    minted_network_interface, standard, ConfigurationTargets, MEDIALIVE_INPUT_ARN,
};

/// GetRouterOutput, the single-ARN read side (`GET /v1/routerOutput/{Arn}`) — what proves
/// an accepted configuration is the one the store now holds.
async fn get_router_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// UpdateRouterOutput (`PUT /v1/routerOutput/{Arn}`).
async fn update_router_output(
    emulator: &Emulator,
    arn: &str,
    body: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send("PUT", &format!("/v1/routerOutput/{arn}"), Some(body))
        .await
}

/// How many router outputs the store holds — a refused request must leave it where it was.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>()
        .lock()
        .router_outputs
        .len()
}

/// A `MediaConnectFlow` configuration naming `flow_arn` and `flow_source_arn`, each only
/// where the case carries one: the model makes both optional, so a configuration may name
/// the flow, the source, both or neither.
fn mediaconnect_flow(flow_arn: Option<&str>, flow_source_arn: Option<&str>) -> Value {
    let mut payload = json!({
        "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
            "encryptionKeyConfiguration": {"Automatic": {}}},
    });
    if let Some(flow_arn) = flow_arn {
        payload["flowArn"] = json!(flow_arn);
    }
    if let Some(flow_source_arn) = flow_source_arn {
        payload["flowSourceArn"] = json!(flow_source_arn);
    }
    json!({"MediaConnectFlow": payload})
}

/// A `MediaLiveInput` configuration — the variant whose target belongs to another service.
fn medialive_input() -> Value {
    json!({"MediaLiveInput": {
        "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
        "mediaLivePipelineId": "PIPELINE_1",
        "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
            "encryptionKeyConfiguration": {"Automatic": {}}},
    }})
}

/// One configuration, and whether the service may store it.
struct TargetCase {
    case: String,
    configuration: Value,
    /// Whether the configuration's targets all resolve, so the request is accepted.
    accepted: bool,
    /// A fragment of the member name the refusal must state, for a refused row.
    member: &'static str,
}

/// A router network interface ARN of the modelled shape that names nothing this account
/// holds: the same id this emulator could have minted, and then each ARN segment moved in
/// turn, so a well-formed ARN is refused for what it NAMES rather than for its shape.
///
/// The segments are the ones the modelled `RouterNetworkInterfaceArn` pattern fixes
/// (`smithy_query(query="shape",shape="RouterNetworkInterfaceArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerNetworkInterface:[a-z0-9]{12}$`).
fn unheld_interface_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "an interface id no create minted",
            "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:ffffffffffff"
                .to_string(),
        ),
        (
            "an interface of another Region",
            "arn:aws:mediaconnect:eu-west-1:000000000000:routerNetworkInterface:000000000001"
                .to_string(),
        ),
        (
            "an interface of another account",
            "arn:aws:mediaconnect:us-east-1:000000000001:routerNetworkInterface:000000000001"
                .to_string(),
        ),
        (
            "an interface of another partition",
            "arn:aws-us-gov:mediaconnect:us-east-1:000000000000:routerNetworkInterface:000000000001"
                .to_string(),
        ),
    ]
}

/// Every row of the class, built against the targets `targets` holds and the second flow
/// `other` holds, so a case can name a source that belongs to ANOTHER flow.
fn target_cases(targets: &ConfigurationTargets, other: &ConfigurationTargets) -> Vec<TargetCase> {
    let protocol = json!({"Rtp": {"destinationAddress": "198.51.100.10",
        "destinationPort": 5004}});
    let unheld_flow = "arn:aws:mediaconnect:us-east-1:000000000000:flow:1-0000000f:absent";
    let unheld_source =
        "arn:aws:mediaconnect:us-east-1:000000000000:source:1-0000000f:absent-source";

    let mut cases = vec![
        TargetCase {
            case: "STANDARD on a router network interface the account holds".to_string(),
            configuration: standard(&targets.interface_arn, protocol.clone()),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a flow and a source of that flow".to_string(),
            configuration: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&targets.flow_source_arn),
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
            case: "MEDIACONNECT FLOW naming the source alone".to_string(),
            configuration: mediaconnect_flow(None, Some(&targets.flow_source_arn)),
            accepted: true,
            member: "",
        },
        TargetCase {
            // "To disconnect it, call the update operation and omit both fields while
            // keeping this configuration object"
            // (`smithy_query(query="shape",shape="MediaConnectFlowRouterOutputConfiguration")`).
            case: "MEDIACONNECT FLOW naming neither, the disconnected form".to_string(),
            configuration: mediaconnect_flow(None, None),
            accepted: true,
            member: "",
        },
        TargetCase {
            case: "MEDIALIVE INPUT, whose target belongs to another service".to_string(),
            configuration: medialive_input(),
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
            case: "MEDIACONNECT FLOW naming a flow no create minted and a held source"
                .to_string(),
            configuration: mediaconnect_flow(
                Some(unheld_flow),
                Some(&targets.flow_source_arn),
            ),
            accepted: false,
            member: "FlowArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a held flow and a source no create minted"
                .to_string(),
            configuration: mediaconnect_flow(Some(&targets.flow_arn), Some(unheld_source)),
            accepted: false,
            member: "FlowSourceArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a held flow and a source of ANOTHER flow"
                .to_string(),
            configuration: mediaconnect_flow(
                Some(&targets.flow_arn),
                Some(&other.flow_source_arn),
            ),
            accepted: false,
            member: "FlowSourceArn",
        },
        TargetCase {
            case: "MEDIACONNECT FLOW naming a source no create minted, with no flow"
                .to_string(),
            configuration: mediaconnect_flow(None, Some(unheld_source)),
            accepted: false,
            member: "FlowSourceArn",
        },
    ];
    for (case, arn) in unheld_interface_arns() {
        cases.push(TargetCase {
            case: format!("STANDARD on {case}"),
            configuration: standard(&arn, protocol.clone()),
            accepted: false,
            member: "NetworkInterfaceArn",
        });
    }
    cases
}

/// A create body carrying `configuration`, named after the row so each accepted case stores
/// a record of its own.
fn create_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "configuration": configuration,
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    })
}

/// A name no other case uses, derived from the row so a failure names the row.
fn case_name(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{slug}")
}

/// The configuration the store holds for `arn`, read back through GetRouterOutput.
async fn stored_configuration(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let (status, code, read) = get_router_output(emulator, arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read back expected 200, got {status} {code:?}: {read}"
    );
    read["routerOutput"]["configuration"].clone()
}

#[tokio::test]
async fn a_create_resolves_every_target_its_configuration_names() {
    let emulator = Emulator::new();
    let targets = minted_configuration_targets(&emulator, "ro-targets").await;
    let other = minted_configuration_targets(&emulator, "ro-targets-other").await;

    for TargetCase {
        case,
        configuration,
        accepted,
        member,
    } in target_cases(&targets, &other)
    {
        let before = stored_count(&emulator);
        let name = case_name("ro", &case);
        let (status, code, answered) =
            create_router_output(&emulator, create_body(&name, configuration.clone())).await;

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: a configuration naming a target the account does not hold is \
                 refused, got {status} {code:?}: {answered}"
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
            "case {case:?}: a configuration whose targets the account holds is accepted, got \
             {status} {code:?}: {answered}"
        );
        let arn = answered["routerOutput"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
            .to_string();
        assert_eq!(
            stored_count(&emulator),
            before + 1,
            "case {case:?}: the accepted create stored one router output, got {} stored",
            stored_count(&emulator)
        );

        // The accepted configuration is the one the store now holds: a STANDARD one carries
        // the protocol its protocol configuration implies, and the other variants are stored
        // as they arrived.
        let stored = stored_configuration(&emulator, &case, &arn).await;
        let expected = if configuration.get("Standard").is_some() {
            let mut expected = configuration.clone();
            expected["Standard"]["protocol"] = json!("RTP");
            expected
        } else {
            configuration.clone()
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: the read reports the configuration the create stored, got {stored}"
        );
    }
}

#[tokio::test]
async fn an_update_resolves_every_target_its_replacement_configuration_names() {
    let emulator = Emulator::new();
    let targets = minted_configuration_targets(&emulator, "ro-upd-targets").await;
    let other = minted_configuration_targets(&emulator, "ro-upd-targets-other").await;

    // One output per configuration KIND the rows replace, because "changing the type of the
    // configuration is not supported": a STANDARD row is applied to the STANDARD output and
    // a MEDIACONNECT FLOW row to the flow output.
    let (standard_arn, _) = created_output(
        &emulator,
        "ro-upd-standard",
        standard(
            &targets.interface_arn,
            json!({"Rtp": {"destinationAddress": "198.51.100.11", "destinationPort": 5006}}),
        ),
    )
    .await;
    let (flow_output_arn, _) = created_output(
        &emulator,
        "ro-upd-flow",
        mediaconnect_flow(Some(&targets.flow_arn), Some(&targets.flow_source_arn)),
    )
    .await;

    for TargetCase {
        case,
        configuration,
        accepted,
        member,
    } in target_cases(&targets, &other)
    {
        // The MEDIALIVE INPUT row is a third kind of configuration, which no output here is,
        // and an update may not change the kind — so it is driven by the create suite alone.
        let arn = if configuration.get("Standard").is_some() {
            standard_arn.as_str()
        } else if configuration.get("MediaConnectFlow").is_some() {
            flow_output_arn.as_str()
        } else {
            continue;
        };

        let before = stored_configuration(&emulator, &case, arn).await;
        let (status, code, answered) =
            update_router_output(&emulator, arn, json!({"configuration": configuration.clone()}))
                .await;

        if !accepted {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: an update naming a target the account does not hold is \
                 refused, got {status} {code:?}: {answered}"
            );
            let message = answered["message"].as_str().unwrap_or_default().to_string();
            assert!(
                message.starts_with(mc::VALIDATION_FAILED_PREFIX) && message.contains(member),
                "case {case:?}: the refusal carries this service's validation envelope and \
                 names {member}, got {message:?}"
            );
            assert_eq!(
                stored_configuration(&emulator, &case, arn).await,
                before,
                "case {case:?}: a refused update leaves the stored configuration alone"
            );
            continue;
        }

        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: an update whose targets the account holds is accepted, got \
             {status} {code:?}: {answered}"
        );
        let stored = stored_configuration(&emulator, &case, arn).await;
        let expected = if configuration.get("Standard").is_some() {
            let mut expected = configuration.clone();
            expected["Standard"]["protocol"] = json!("RTP");
            expected
        } else {
            configuration.clone()
        };
        assert_eq!(
            stored, expected,
            "case {case:?}: the accepted update stored the replacement configuration, got \
             {stored}"
        );
    }
}

/// Create one router output that must be accepted and answer `(arn, name)`.
async fn created_output(emulator: &Emulator, name: &str, configuration: Value) -> (String, String) {
    let (status, code, created) =
        create_router_output(emulator, create_body(name, configuration)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the create of {name:?} expected 201, got {status} {code:?}: {created}"
    );
    let arn = created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create of {name:?} answers an ARN: {created}"))
        .to_string();
    (arn, name.to_string())
}

/// The verdict is read from the STORE, not from the ARN's shape: the very ARN a create
/// accepted stops being a target once the account no longer holds the resource — the
/// interface after its deletion has come due, and the flow (with its source) as soon as it
/// is deleted.
#[tokio::test]
async fn a_target_the_account_no_longer_holds_is_refused() {
    let emulator = Emulator::new();
    let interface_arn = minted_network_interface(&emulator, "rni-ro-gone").await;
    let (flow_arn, flow_source_arn) = minted_flow_source(&emulator, "flow-ro-gone").await;
    let protocol = json!({"Rtp": {"destinationAddress": "198.51.100.12",
        "destinationPort": 5008}});

    // While the account holds them, both configurations are accepted.
    created_output(
        &emulator,
        "ro-gone-standard",
        standard(&interface_arn, protocol.clone()),
    )
    .await;
    created_output(
        &emulator,
        "ro-gone-flow",
        mediaconnect_flow(Some(&flow_arn), Some(&flow_source_arn)),
    )
    .await;

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
            standard(&interface_arn, protocol),
            "NetworkInterfaceArn",
        ),
        (
            "the flow the account deleted",
            mediaconnect_flow(Some(&flow_arn), Some(&flow_source_arn)),
            "FlowArn",
        ),
        (
            "the source of the flow the account deleted",
            mediaconnect_flow(None, Some(&flow_source_arn)),
            "FlowSourceArn",
        ),
    ] {
        let (status, code, refused) = create_router_output(
            &emulator,
            create_body(&case_name("ro-gone", case), configuration),
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

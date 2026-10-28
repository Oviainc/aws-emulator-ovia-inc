// SPDX-License-Identifier: CC0-1.0

//! What a REMOVAL does to the router records that pointed into what it removed, end to end
//! over HTTP.
//!
//! The rule these cases assert, over a table rather than over one request: a stored
//! reference a write path resolves against this store, which the model makes OPTIONAL, never
//! outlives the resource it names — when a flow, a flow output or a flow source leaves the
//! store, the member naming it is dropped from every router input and router output
//! configuration that held it, leaving the configuration in the DISCONNECTED form the model
//! defines ("to connect the router input to the flow output, provide both `flowArn` and
//! `flowOutputArn`. To disconnect it … omit both fields while keeping this configuration
//! object",
//! `smithy_query(query="shape",shape="MediaConnectFlowRouterInputConfiguration")`, and the
//! same on `MediaConnectFlowRouterOutputConfiguration`). So every read reports a
//! configuration the write paths would still accept, which is the other half of the rule the
//! create and the update enforce (see `tests/all/router_input_configuration_targets.rs` and
//! `tests/all/router_output_configuration_targets.rs`).
//!
//! The rows differ along what the rule generalises over: which record holds the reference (a
//! router input, which names a flow OUTPUT, or a router output, which names a flow SOURCE),
//! which call takes the target away (RemoveFlowOutput, RemoveFlowSource, DeleteFlow — the
//! last taking the flow's sources and outputs with it), which of the two optional members the
//! configuration named (both, one, neither), and whether what the call removed is something
//! that holder actually named. Every row also carries two BYSTANDERS the call must leave
//! alone: one pointing into a second flow nothing removes, and one whose configuration names
//! no flow at all.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;
use crate::router_input_batch_get::batch_get_router_inputs;
use crate::router_input_configuration_targets::{
    mediaconnect_flow as flow_input_configuration, standard as standard_input_configuration,
};
use crate::router_input_restart::{
    create_body as input_create_body, create_router_input, get_router_input, union_member,
};
use crate::router_output_configuration_targets::{
    create_body as output_create_body, get_router_output,
    mediaconnect_flow as flow_output_configuration, update_router_output,
};
use crate::router_outputs::{
    batch_get_router_outputs, create_router_output, minted_network_interface,
    standard as standard_output_configuration,
};
use crate::update_router_input::update_router_input;

/// Which kind of router record holds the reference under test.
///
/// The two are the only records whose configuration can name a flow resource: a router input
/// takes its content FROM a flow output, a router output delivers INTO a flow source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holder {
    Input,
    Output,
}

impl Holder {
    /// The member a holder of this kind names its sub-resource in — the one the flow-level
    /// `flowArn` accompanies.
    fn target_member(self) -> &'static str {
        match self {
            Holder::Input => "flowOutputArn",
            Holder::Output => "flowSourceArn",
        }
    }
}

/// How the row takes a resource out of the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Removal {
    /// RemoveFlowOutput on the flow output a router input can name.
    FlowOutput,
    /// RemoveFlowSource on the flow source a router output can name.
    FlowSource,
    /// DeleteFlow, which takes the flow's sources and outputs with it.
    Flow,
}

/// One flow minted over the wire, with every sub-resource ARN a router configuration can
/// name.
///
/// Two sources and two outputs: RemoveFlowSource "can be made only if there is more than one
/// source on the flow", so a flow a source can be removed from holds two.
struct MintedFlow {
    flow_arn: String,
    output_arn: String,
    source_arn: String,
}

/// Mint one such flow, naming every sub-resource after `name` so one emulator can hold
/// several.
///
/// A flow holds two sources only while its failover state is active ("if the state is
/// inactive, the flow can have only one source. If the state is active, the flow can have one
/// or two sources", `FailoverConfig$State`), so the create states that configuration.
async fn minted_flow(emulator: &Emulator, name: &str) -> MintedFlow {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": name,
            "sources": [
                { "name": format!("{name}-src-a"), "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16" },
                { "name": format!("{name}-src-b"), "protocol": "rtp",
                    "whitelistCidr": "10.1.0.0/16" },
            ],
            "sourceFailoverConfig": { "state": "ENABLED", "failoverMode": "FAILOVER",
                "recoveryWindow": 3000 },
            "outputs": [
                { "name": format!("{name}-out-a"), "protocol": "rtp-fec",
                    "destination": "203.0.113.30", "port": 5000 },
                { "name": format!("{name}-out-b"), "protocol": "rtp-fec",
                    "destination": "203.0.113.31", "port": 5002 },
            ],
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "CreateFlow expected 201 for {name:?}, got {status} {code:?}: {created}"
    );
    let arn_at = |pointer: &str| {
        created
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the minted flow carries {pointer}: {created}"))
            .to_string()
    };
    MintedFlow {
        flow_arn: arn_at("/flow/flowArn"),
        output_arn: arn_at("/flow/outputs/0/outputArn"),
        source_arn: arn_at("/flow/sources/0/sourceArn"),
    }
}

/// A MEDIACONNECT FLOW configuration for a holder of kind `holder`, naming the flow and its
/// own sub-resource of `flow` only where the row carries each.
fn flow_configuration(
    holder: Holder,
    flow: &MintedFlow,
    names_flow: bool,
    names_target: bool,
) -> Value {
    let flow_arn = names_flow.then_some(flow.flow_arn.as_str());
    match holder {
        Holder::Input => {
            flow_input_configuration(flow_arn, names_target.then_some(flow.output_arn.as_str()))
        }
        Holder::Output => {
            flow_output_configuration(flow_arn, names_target.then_some(flow.source_arn.as_str()))
        }
    }
}

/// A configuration for a holder of kind `holder` that names NO flow resource at all: it
/// ingests on, or egresses from, a router network interface instead.
fn interface_configuration(holder: Holder, interface_arn: &str) -> Value {
    match holder {
        Holder::Input => standard_input_configuration(interface_arn),
        Holder::Output => standard_output_configuration(
            interface_arn,
            json!({ "Rtp": { "destinationAddress": "198.51.100.10", "destinationPort": 5004 } }),
        ),
    }
}

/// Create one router record of kind `holder` carrying `configuration`, and answer its ARN.
async fn create_holder(
    emulator: &Emulator,
    case: &str,
    holder: Holder,
    name: &str,
    configuration: Value,
) -> String {
    let (status, code, created) = match holder {
        Holder::Input => {
            create_router_input(emulator, input_create_body(name, configuration)).await
        }
        Holder::Output => {
            create_router_output(emulator, output_create_body(name, configuration)).await
        }
    };
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the create of {name:?} expected 201, got {status} {code:?}: {created}"
    );
    let pointer = match holder {
        Holder::Input => "/routerInput/arn",
        Holder::Output => "/routerOutput/arn",
    };
    created
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create answers {pointer}: {created}"))
        .to_string()
}

/// The configuration the store holds for the router record `arn`, read back through the
/// single-resource read of its kind.
async fn reported_configuration(
    emulator: &Emulator,
    case: &str,
    holder: Holder,
    arn: &str,
) -> Value {
    let (status, code, read) = match holder {
        Holder::Input => get_router_input(emulator, arn).await,
        Holder::Output => get_router_output(emulator, arn).await,
    };
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the read of {arn} expected 200, got {status} {code:?}: {read}"
    );
    let pointer = match holder {
        Holder::Input => "/routerInput/configuration",
        Holder::Output => "/routerOutput/configuration",
    };
    read.pointer(pointer)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the read reports {pointer}: {read}"))
}

/// The same configuration, read back through the BATCH read of its kind — the other reader
/// the issue's reproduction walks, which must report exactly what the single read reports.
async fn batch_reported_configuration(
    emulator: &Emulator,
    case: &str,
    holder: Holder,
    arn: &str,
) -> Value {
    let (status, code, read) = match holder {
        Holder::Input => batch_get_router_inputs(emulator, &[arn]).await,
        Holder::Output => batch_get_router_outputs(emulator, &[arn]).await,
    };
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the batch read of {arn} expected 200, got {status} {code:?}: {read}"
    );
    assert_eq!(
        read["errors"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: the batch read resolves the ARN it names, got {read}"
    );
    let pointer = match holder {
        Holder::Input => "/routerInputs/0/configuration",
        Holder::Output => "/routerOutputs/0/configuration",
    };
    read.pointer(pointer)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the batch read reports {pointer}: {read}"))
}

/// The two flow references `configuration` names, as `(FlowArn, the holder's own member)`.
fn flow_references(holder: Holder, configuration: &Value) -> (Option<String>, Option<String>) {
    let payload = union_member(configuration, "MediaConnectFlow", "mediaConnectFlow");
    let member = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    (member("flowArn"), member(holder.target_member()))
}

/// Take the resource the row removes out of the store, over the wire.
async fn perform_removal(emulator: &Emulator, case: &str, removal: Removal, flow: &MintedFlow) {
    let (uri, expected) = match removal {
        Removal::FlowOutput => (
            format!("/v1/flows/{}/outputs/{}", flow.flow_arn, flow.output_arn),
            202,
        ),
        Removal::FlowSource => (
            format!("/v1/flows/{}/source/{}", flow.flow_arn, flow.source_arn),
            202,
        ),
        Removal::Flow => (format!("/v1/flows/{}", flow.flow_arn), 202),
    };
    let (status, code, answered) = emulator.send("DELETE", &uri, None).await;
    assert_eq!(
        (status, code.as_deref()),
        (expected, None),
        "case {case:?}: DELETE {uri} expected {expected}, got {status} {code:?}: {answered}"
    );
}

/// Re-state `configuration` through the update of the holder's kind — what a caller does with
/// the configuration a read just reported.
async fn restate_configuration(
    emulator: &Emulator,
    arn: &str,
    holder: Holder,
    configuration: Value,
) -> (u16, Option<String>, Value) {
    let body = json!({ "configuration": configuration });
    match holder {
        Holder::Input => update_router_input(emulator, arn, body).await,
        Holder::Output => update_router_output(emulator, arn, body).await,
    }
}

/// One row of the table.
struct Row {
    case: &'static str,
    holder: Holder,
    removal: Removal,
    /// Whether the holder's configuration names the flow before the removal.
    names_flow: bool,
    /// Whether it names the sub-resource its kind connects to: the flow OUTPUT for a router
    /// input, the flow SOURCE for a router output.
    names_target: bool,
    /// Whether the read must still report `flowArn` afterwards.
    keeps_flow: bool,
    /// Whether it must still report the holder's own member afterwards.
    keeps_target: bool,
}

/// The rows span the class: both holders, all three removals, every combination of the two
/// optional members a configuration can name, and both the case where what the call removed
/// is something the holder named and the case where it is not.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an input naming the flow and the removed output",
            holder: Holder::Input,
            removal: Removal::FlowOutput,
            names_flow: true,
            names_target: true,
            keeps_flow: true,
            keeps_target: false,
        },
        Row {
            case: "an input naming the removed output alone",
            holder: Holder::Input,
            removal: Removal::FlowOutput,
            names_flow: false,
            names_target: true,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an input naming only the flow, whose output is removed",
            holder: Holder::Input,
            removal: Removal::FlowOutput,
            names_flow: true,
            names_target: false,
            keeps_flow: true,
            keeps_target: false,
        },
        Row {
            case: "an input naming the flow and an output, when a SOURCE is removed",
            holder: Holder::Input,
            removal: Removal::FlowSource,
            names_flow: true,
            names_target: true,
            keeps_flow: true,
            keeps_target: true,
        },
        Row {
            case: "an input naming the flow and an output of the deleted flow",
            holder: Holder::Input,
            removal: Removal::Flow,
            names_flow: true,
            names_target: true,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an input naming only the deleted flow",
            holder: Holder::Input,
            removal: Removal::Flow,
            names_flow: true,
            names_target: false,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an input in the disconnected form, whose flow is deleted",
            holder: Holder::Input,
            removal: Removal::Flow,
            names_flow: false,
            names_target: false,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an output naming the flow and the removed source",
            holder: Holder::Output,
            removal: Removal::FlowSource,
            names_flow: true,
            names_target: true,
            keeps_flow: true,
            keeps_target: false,
        },
        Row {
            case: "an output naming the removed source alone",
            holder: Holder::Output,
            removal: Removal::FlowSource,
            names_flow: false,
            names_target: true,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an output naming only the flow, whose source is removed",
            holder: Holder::Output,
            removal: Removal::FlowSource,
            names_flow: true,
            names_target: false,
            keeps_flow: true,
            keeps_target: false,
        },
        Row {
            case: "an output naming the flow and a source, when an OUTPUT is removed",
            holder: Holder::Output,
            removal: Removal::FlowOutput,
            names_flow: true,
            names_target: true,
            keeps_flow: true,
            keeps_target: true,
        },
        Row {
            case: "an output naming the flow and a source of the deleted flow",
            holder: Holder::Output,
            removal: Removal::Flow,
            names_flow: true,
            names_target: true,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an output naming the source of the deleted flow alone",
            holder: Holder::Output,
            removal: Removal::Flow,
            names_flow: false,
            names_target: true,
            keeps_flow: false,
            keeps_target: false,
        },
        Row {
            case: "an output in the disconnected form, whose flow is deleted",
            holder: Holder::Output,
            removal: Removal::Flow,
            names_flow: false,
            names_target: false,
            keeps_flow: false,
            keeps_target: false,
        },
    ]
}

/// A removal drops every router reference into what it removed, leaves every other reference
/// where it was, and leaves behind a configuration the update still accepts.
#[tokio::test]
async fn a_removal_drops_every_router_reference_into_what_it_removed() {
    for Row {
        case,
        holder,
        removal,
        names_flow,
        names_target,
        keeps_flow,
        keeps_target,
    } in rows()
    {
        let emulator = Emulator::new();
        let interface_arn = minted_network_interface(&emulator, "rni-removal").await;
        let flow = minted_flow(&emulator, "flow-removed").await;
        // A second flow nothing removes, and a holder pointing into it: a repair must reach
        // the references that stopped resolving and no others.
        let other = minted_flow(&emulator, "flow-kept").await;

        let holder_arn = create_holder(
            &emulator,
            case,
            holder,
            "holder-under-test",
            flow_configuration(holder, &flow, names_flow, names_target),
        )
        .await;
        let bystander_arn = create_holder(
            &emulator,
            case,
            holder,
            "holder-on-another-flow",
            flow_configuration(holder, &other, true, true),
        )
        .await;
        let unconnected_arn = create_holder(
            &emulator,
            case,
            holder,
            "holder-naming-no-flow",
            interface_configuration(holder, &interface_arn),
        )
        .await;

        let bystander_before = reported_configuration(&emulator, case, holder, &bystander_arn).await;
        let unconnected_before =
            reported_configuration(&emulator, case, holder, &unconnected_arn).await;

        perform_removal(&emulator, case, removal, &flow).await;

        let configuration = reported_configuration(&emulator, case, holder, &holder_arn).await;
        let (reported_flow, reported_target) = flow_references(holder, &configuration);
        let expected_flow = keeps_flow.then(|| flow.flow_arn.clone());
        let expected_target = keeps_target.then(|| match holder {
            Holder::Input => flow.output_arn.clone(),
            Holder::Output => flow.source_arn.clone(),
        });
        assert_eq!(
            (reported_flow.clone(), reported_target.clone()),
            (expected_flow, expected_target),
            "case {case:?}: after the removal the read must report only the references the \
             store still holds, got flowArn {reported_flow:?} and {} {reported_target:?}",
            holder.target_member()
        );

        // The batch read answers from the same record, so it cannot report a reference the
        // single read no longer does.
        let batched = batch_reported_configuration(&emulator, case, holder, &holder_arn).await;
        assert_eq!(
            batched, configuration,
            "case {case:?}: the batch read reports the configuration the single read reports, \
             got {batched}"
        );

        // Neither bystander named anything this call removed, so neither changed.
        let bystander_after = reported_configuration(&emulator, case, holder, &bystander_arn).await;
        assert_eq!(
            bystander_after, bystander_before,
            "case {case:?}: a holder pointing into another flow keeps its references, got \
             {bystander_after}"
        );
        let unconnected_after =
            reported_configuration(&emulator, case, holder, &unconnected_arn).await;
        assert_eq!(
            unconnected_after, unconnected_before,
            "case {case:?}: a holder naming no flow resource is untouched, got \
             {unconnected_after}"
        );

        // What the read reports is a configuration the write path still resolves: re-stating
        // it is accepted, and the record keeps it.
        let (status, code, restated) =
            restate_configuration(&emulator, &holder_arn, holder, configuration.clone()).await;
        assert_eq!(
            (status, code.as_deref()),
            (202, None),
            "case {case:?}: re-stating the configuration the read reported expected 202, got \
             {status} {code:?}: {restated}"
        );
        let after_restate = reported_configuration(&emulator, case, holder, &holder_arn).await;
        assert_eq!(
            after_restate, configuration,
            "case {case:?}: the re-stated configuration is the one the store holds, got \
             {after_restate}"
        );
    }
}

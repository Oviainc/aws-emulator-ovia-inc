// SPDX-License-Identifier: CC0-1.0

//! What keeps a stored router input or router output from naming a router network interface
//! the store does not hold, end to end over HTTP.
//!
//! The rule the cases below pin: a resource of this service whose identifier another stored
//! record names in a REQUIRED member of its configuration is not taken away while such a
//! record is in the store — the removal is refused `ConflictException` (409, "the requested
//! operation would cause a conflict with the current state of a service resource associated
//! with the request. Resolve the conflict before retrying this request"), stating the holders
//! the store counts, and the caller clears the association (by deleting those records or
//! pointing them at another resource) and retries. The one such reference this service holds
//! is the router network interface a router input ingests on and a router output egresses
//! from (`StandardRouterInputConfiguration$NetworkInterfaceArn` and the same member on the
//! FAILOVER and MERGE input configurations,
//! `StandardRouterOutputConfiguration$NetworkInterfaceArn`, each required —
//! `smithy_query(query="shape",shape="StandardRouterInputConfiguration")`), which is what
//! `RouterNetworkInterface$AssociatedInputCount` / `$AssociatedOutputCount` count and what
//! CreateRouterInput / UpdateRouterInput / CreateRouterOutput / UpdateRouterOutput resolve
//! against the store rather than taking on trust. The same reading closes the other way in
//! too: an interface whose deletion has already been accepted is no longer a target a new
//! configuration can be stored against, because the clock owes its removal whether or not
//! anybody calls.
//!
//! The rows differ along what the rule generalises over: which configuration variant holds
//! the reference (STANDARD — on two protocols — FAILOVER and MERGE on the input side,
//! STANDARD on the output side), which side holds it, how many holders there are, whether a
//! record names THIS interface, another one or none at all, and in which order the
//! associations are cleared.
//!
//! Every call travels the whole request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], and each case ends by asserting the store holds NO
//! configuration naming an interface it does not hold — the state a delete must never
//! produce.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    configuration_rows, create_body, create_router_input, get_router_input, past_transition_span,
    union_member, ConfigurationRow,
};
use crate::router_network_interfaces::batch_get_network_interfaces;
use crate::router_outputs::{
    create_router_output, minted_flow_source, minted_network_interface, standard,
    MEDIALIVE_INPUT_ARN,
};

/// The sentence a refused delete must carry, built here rather than read out of the crate, so
/// the wording is pinned by the test and not by the code under test.
///
/// It names the interface the request addressed and states the two counts
/// `RouterNetworkInterface$AssociatedInputCount` / `$AssociatedOutputCount` report for it,
/// which is what the caller has to clear before retrying.
fn expected_conflict(interface_arn: &str, inputs: i64, outputs: i64) -> String {
    format!(
        "Router network interface {interface_arn} still has {inputs} associated router \
         input(s) and {outputs} associated router output(s). Delete them, or point their \
         configuration at another router network interface, before deleting the interface."
    )
}

/// `DELETE /v1/routerNetworkInterface/{Arn}`.
async fn delete_interface(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerNetworkInterface/{arn}"), None)
        .await
}

/// `DELETE /v1/routerInput/{Arn}`.
async fn delete_input(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerInput/{arn}"), None)
        .await
}

/// `DELETE /v1/routerOutput/{Arn}`.
async fn delete_output(emulator: &Emulator, arn: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("DELETE", &format!("/v1/routerOutput/{arn}"), None)
        .await
}

/// A minimal CreateRouterOutput body for `configuration`, the output-side counterpart of
/// [`create_body`].
fn output_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "configuration": configuration,
        "maximumBitrate": 20_000_000i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    })
}

/// A STANDARD router output configuration on one RTP protocol configuration — the one output
/// configuration variant that names a router network interface.
fn output_standard(interface_arn: &str, port: i64) -> Value {
    standard(
        interface_arn,
        json!({"Rtp": {"destinationAddress": "198.51.100.10", "destinationPort": port}}),
    )
}

/// A `RouterNetworkInterfaceArn` of the modelled shape that no create minted, used only to
/// count the rows of [`rows_naming`] before the interface each row will name exists.
const PLACEHOLDER_INTERFACE_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerNetworkInterface:ffffffffffff";

/// The rows of the crate's one router input configuration table whose configuration NAMES the
/// router network interface `interface_arn` — the STANDARD, FAILOVER and MERGE variants,
/// which carry a required `NetworkInterfaceArn`.
///
/// Picked by looking for the ARN in the row's own payload rather than by naming the variants
/// here, so a variant added to that table is covered by these suites too.
fn rows_naming(interface_arn: &str, flow_arn: &str) -> Vec<ConfigurationRow> {
    configuration_rows(interface_arn, flow_arn)
        .into_iter()
        .filter(|(_, _, _, configuration, _)| configuration.to_string().contains(interface_arn))
        .collect()
}

/// Create one router input over the wire and answer its ARN.
async fn created_input(
    emulator: &Emulator,
    case: &str,
    name: &str,
    configuration: Value,
) -> String {
    let (status, code, created) =
        create_router_input(emulator, create_body(name, configuration)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the create of {name:?} expected 201, got {status} {code:?}: {created}"
    );
    created["routerInput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// Create one router output over the wire and answer its ARN.
async fn created_output(
    emulator: &Emulator,
    case: &str,
    name: &str,
    configuration: Value,
) -> String {
    let (status, code, created) =
        create_router_output(emulator, output_body(name, configuration)).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the create of {name:?} expected 201, got {status} {code:?}: {created}"
    );
    created["routerOutput"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {created}"))
        .to_string()
}

/// The `(state, associatedInputCount, associatedOutputCount)` the implemented reader
/// (BatchGetRouterNetworkInterface) reports for `arn`, or `None` once the store holds no
/// interface for it.
async fn reported_interface(emulator: &Emulator, arn: &str) -> Option<(String, i64, i64)> {
    let (status, code, read) = batch_get_network_interfaces(emulator, &[arn]).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the read-back of {arn:?} expected 200, got {status} {code:?}: {read}"
    );
    let reported = &read["routerNetworkInterfaces"][0];
    if reported.is_null() {
        return None;
    }
    Some((
        reported["state"].as_str().unwrap_or_default().to_string(),
        reported["associatedInputCount"].as_i64().unwrap_or(-1),
        reported["associatedOutputCount"].as_i64().unwrap_or(-1),
    ))
}

/// Every stored router input and router output whose configuration names a router network
/// interface the store does NOT hold, as `(record ARN, interface ARN)`.
///
/// The reference is read through the crate's own two readings of it
/// ([`mc::router_input_network_interface_arn`], [`mc::router_output_network_interface_arn`]),
/// the same ones the association counts are taken from, so this sees exactly what the service
/// resolves.
fn dangling_interface_references(emulator: &Emulator) -> Vec<(String, String)> {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    let mut dangling = Vec::new();
    for (arn, router_input) in store.router_inputs.iter() {
        if let Some(interface_arn) =
            mc::router_input_network_interface_arn(&router_input.configuration)
        {
            if !store.router_network_interfaces.contains_key(interface_arn) {
                dangling.push((arn.clone(), interface_arn.to_string()));
            }
        }
    }
    for (arn, router_output) in store.router_outputs.iter() {
        if let Some(interface_arn) =
            mc::router_output_network_interface_arn(&router_output.configuration)
        {
            if !store.router_network_interfaces.contains_key(interface_arn) {
                dangling.push((arn.clone(), interface_arn.to_string()));
            }
        }
    }
    dangling
}

/// Assert the invariant this whole suite is about: no stored record names an interface the
/// store does not hold.
fn assert_no_dangling_references(emulator: &Emulator, case: &str) {
    let dangling = dangling_interface_references(emulator);
    assert!(
        dangling.is_empty(),
        "case {case:?}: no stored router input or output may name an interface the store does \
         not hold, got {dangling:?}"
    );
}

/// The delete of an interface a stored record still names is refused, whichever side holds
/// the reference and whichever configuration variant carries it — and the refused call
/// changes nothing: the interface stays exactly as it was, counting its holder.
#[tokio::test]
async fn an_interface_a_stored_record_names_is_not_deleted() {
    let emulator = Emulator::new();
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-assoc").await;

    // One interface per row, so each row is judged on its own holders. The interface has to
    // exist before the row's configuration can name it, so the table is rebuilt per row.
    let row_count = rows_naming(PLACEHOLDER_INTERFACE_ARN, &flow_arn).len();
    let mut rows: Vec<(String, String, i64, i64)> = Vec::new();
    for index in 0..row_count {
        let interface_arn =
            minted_network_interface(&emulator, &format!("rni-assoc-{index}")).await;
        let table = rows_naming(&interface_arn, &flow_arn);
        let Some((case, _, _, configuration, _)) = table.get(index).cloned() else {
            break;
        };
        created_input(
            &emulator,
            case,
            &format!("ri-assoc-{index}"),
            configuration,
        )
        .await;
        rows.push((format!("router input, {case}"), interface_arn, 1, 0));
    }

    let output_interface_arn = minted_network_interface(&emulator, "rni-assoc-output").await;
    created_output(
        &emulator,
        "router output, standard",
        "ro-assoc-standard",
        output_standard(&output_interface_arn, 5100),
    )
    .await;
    rows.push((
        "router output, standard".to_string(),
        output_interface_arn,
        0,
        1,
    ));

    assert!(
        rows.len() >= 5,
        "every configuration variant that names an interface is a row, got {rows:?}"
    );

    for (case, interface_arn, inputs, outputs) in &rows {
        let before = reported_interface(&emulator, interface_arn).await;
        assert_eq!(
            before
                .as_ref()
                .map(|(state, held_inputs, held_outputs)| (
                    state.as_str(),
                    *held_inputs,
                    *held_outputs
                )),
            Some((
                mc::ROUTER_NETWORK_INTERFACE_STATE_CREATING,
                *inputs,
                *outputs
            )),
            "case {case:?}: the interface is live and counts its holder, got {before:?}"
        );

        let (status, code, refused) = delete_interface(&emulator, interface_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: deleting an interface a record names is refused, got {status} \
             {code:?}: {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(expected_conflict(interface_arn, *inputs, *outputs).as_str()),
            "case {case:?}: expected {:?}, got {refused}",
            expected_conflict(interface_arn, *inputs, *outputs)
        );

        let after = reported_interface(&emulator, interface_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused delete left the interface exactly as it was, got \
             {after:?}"
        );
        assert_no_dangling_references(&emulator, case);
    }
}

/// The counts the refusal states are the store's own, and the conflict is cleared one holder
/// at a time: with two inputs and one output naming the interface the delete is refused
/// stating 2 and 1, then 1 and 1, then 0 and 1 as each holder's removal comes due, and it is
/// served only once nothing names the interface — after which the record stops existing with
/// no configuration left pointing at it.
#[tokio::test]
async fn the_delete_is_served_once_every_holder_is_gone() {
    let emulator = Emulator::new();
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-assoc-counts").await;
    let interface_arn = minted_network_interface(&emulator, "rni-assoc-counts").await;
    let case = "two inputs and one output";

    // Two DIFFERENT input configuration variants, so the count is not of one shape of record.
    let table = rows_naming(&interface_arn, &flow_arn);
    let mut input_arns = Vec::new();
    for (index, (row, _, _, configuration, _)) in table.iter().take(2).enumerate() {
        input_arns.push(
            created_input(
                &emulator,
                row,
                &format!("ri-assoc-counts-{index}"),
                configuration.clone(),
            )
            .await,
        );
    }
    assert_eq!(
        input_arns.len(),
        2,
        "case {case:?}: two inputs name the interface, got {input_arns:?}"
    );
    let output = created_output(
        &emulator,
        case,
        "ro-assoc-counts",
        output_standard(&interface_arn, 5200),
    )
    .await;

    // Each step clears ONE holder, so the counts the refusal states walk down with the store.
    for (step, removed, inputs, outputs) in [
        ("nothing removed yet", None, 2, 1),
        ("one input removed", Some(&input_arns[0]), 1, 1),
        ("both inputs removed", Some(&input_arns[1]), 0, 1),
    ] {
        if let Some(arn) = removed {
            let (status, code, deleted) = delete_input(&emulator, arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "step {step:?}: the delete of {arn:?} expected 200, got {status} {code:?}: \
                 {deleted}"
            );
            // A holder leaves the store one transition span after its delete is accepted,
            // which is when it stops being an association.
            tokio::time::sleep(past_transition_span()).await;
        }

        let reported = reported_interface(&emulator, &interface_arn).await;
        assert_eq!(
            reported
                .as_ref()
                .map(|(_, held_inputs, held_outputs)| (*held_inputs, *held_outputs)),
            Some((inputs, outputs)),
            "step {step:?}: the reader reports the holders the store counts, got {reported:?}"
        );
        let (status, code, refused) = delete_interface(&emulator, &interface_arn).await;
        assert_eq!(
            (status, code.as_deref()),
            (409, Some("ConflictException")),
            "step {step:?}: the delete is still refused, got {status} {code:?}: {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(expected_conflict(&interface_arn, inputs, outputs).as_str()),
            "step {step:?}: expected {:?}, got {refused}",
            expected_conflict(&interface_arn, inputs, outputs)
        );
        assert_no_dangling_references(&emulator, step);
    }

    let (status, code, deleted) = delete_output(&emulator, &output).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the delete of the last holder expected 200, got {status} {code:?}: \
         {deleted}"
    );
    tokio::time::sleep(past_transition_span()).await;

    let (status, code, served) = delete_interface(&emulator, &interface_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: with nothing naming it the delete is served, got {status} {code:?}: \
         {served}"
    );
    assert_eq!(
        (served["arn"].as_str(), served["state"].as_str()),
        (
            Some(interface_arn.as_str()),
            Some(mc::ROUTER_NETWORK_INTERFACE_STATE_DELETING)
        ),
        "case {case:?}: the served delete answers the interface's own ARN in DELETING, got \
         {served}"
    );

    tokio::time::sleep(past_transition_span()).await;
    assert_eq!(
        reported_interface(&emulator, &interface_arn).await,
        None,
        "case {case:?}: the interface whose removal came due is gone"
    );
    assert_no_dangling_references(&emulator, case);
}

/// A record that names NO interface — or names ANOTHER one — is no holder: the delete of an
/// interface nothing names is served while such records are in the store, so the refusal is
/// keyed to the reference rather than to there being router inputs and outputs at all.
///
/// The rows are every input configuration variant, against a DIFFERENT interface and against
/// resources of other kinds (a MediaConnect flow, a MediaLive channel), plus an output that
/// delivers into MediaLive.
#[tokio::test]
async fn a_record_that_does_not_name_the_interface_does_not_block_its_delete() {
    let emulator = Emulator::new();
    let subject = minted_network_interface(&emulator, "rni-assoc-subject").await;
    let other = minted_network_interface(&emulator, "rni-assoc-other").await;
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-assoc-other").await;

    for (index, (case, _, _, configuration, _)) in
        configuration_rows(&other, &flow_arn).into_iter().enumerate()
    {
        created_input(
            &emulator,
            case,
            &format!("ri-assoc-other-{index}"),
            configuration,
        )
        .await;
    }
    created_output(
        &emulator,
        "output on the other interface",
        "ro-assoc-other",
        output_standard(&other, 5300),
    )
    .await;
    created_output(
        &emulator,
        "output into MediaLive",
        "ro-assoc-medialive",
        json!({"MediaLiveInput": {
            "mediaLiveInputArn": MEDIALIVE_INPUT_ARN,
            "mediaLivePipelineId": "PIPELINE_1",
            "destinationTransitEncryption": {"encryptionKeyType": "AUTOMATIC",
                "encryptionKeyConfiguration": {"Automatic": {}}},
        }}),
    )
    .await;

    let reported = reported_interface(&emulator, &subject).await;
    assert_eq!(
        reported
            .as_ref()
            .map(|(_, inputs, outputs)| (*inputs, *outputs)),
        Some((0, 0)),
        "nothing names the subject interface, got {reported:?}"
    );

    let (status, code, served) = delete_interface(&emulator, &subject).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "an interface nothing names is deleted, got {status} {code:?}: {served}"
    );

    // And the OTHER interface, which those records do name, is still refused.
    let (status, code, refused) = delete_interface(&emulator, &other).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "the interface those records name is still refused, got {status} {code:?}: {refused}"
    );

    tokio::time::sleep(past_transition_span()).await;
    assert_eq!(
        reported_interface(&emulator, &subject).await,
        None,
        "the deleted interface stopped existing"
    );
    assert_no_dangling_references(&emulator, "records naming another interface or none");
}

/// The other way a record could come to name an interface the store does not hold: an
/// interface whose deletion has already been accepted is no longer a target a configuration
/// can be stored against, because the clock owes its removal whether or not anybody calls.
///
/// The rows are every configuration variant that names an interface, on both sides, and each
/// refusal is the one this service words for a reference it cannot resolve
/// (`BadRequestException`, 400, in the validation envelope, naming the member that did not
/// resolve) — the same answer as for an interface the account never held.
#[tokio::test]
async fn an_interface_already_being_deleted_is_no_target_for_a_new_configuration() {
    let emulator = Emulator::new();
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-assoc-deleting").await;
    let interface_arn = minted_network_interface(&emulator, "rni-assoc-deleting").await;

    let (status, code, served) = delete_interface(&emulator, &interface_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "the delete of an interface nothing names is served, got {status} {code:?}: {served}"
    );

    for (index, (case, _, _, configuration, _)) in
        rows_naming(&interface_arn, &flow_arn).into_iter().enumerate()
    {
        let (status, code, refused) = create_router_input(
            &emulator,
            create_body(&format!("ri-assoc-deleting-{index}"), configuration),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: a configuration naming an interface on its way out is refused, \
             got {status} {code:?}: {refused}"
        );
        let message = refused["message"].as_str().unwrap_or_default().to_string();
        assert!(
            message.starts_with(mc::VALIDATION_FAILED_PREFIX)
                && message.contains("NetworkInterfaceArn"),
            "case {case:?}: the refusal carries this service's validation envelope and names \
             the member, got {message:?}"
        );
    }

    let (status, code, refused) = create_router_output(
        &emulator,
        output_body("ro-assoc-deleting", output_standard(&interface_arn, 5400)),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "an output configuration naming an interface on its way out is refused, got {status} \
         {code:?}: {refused}"
    );

    // Past the span the record is gone, and nothing in the store names it.
    tokio::time::sleep(past_transition_span()).await;
    assert_eq!(
        reported_interface(&emulator, &interface_arn).await,
        None,
        "the interface whose removal came due is gone"
    );
    assert_no_dangling_references(&emulator, "creates against an interface being deleted");
}

/// The record a refused delete protected is unchanged by it: the input still reports the
/// configuration it was created with, and the ARN in it still resolves — which is what the
/// refusal kept true.
#[tokio::test]
async fn the_protected_record_still_reads_back_with_a_resolving_reference() {
    let emulator = Emulator::new();
    let (flow_arn, _) = minted_flow_source(&emulator, "flow-assoc-readback").await;
    let interface_arn = minted_network_interface(&emulator, "rni-assoc-readback").await;
    let table = rows_naming(&interface_arn, &flow_arn);
    let (case, member_name, json_name, configuration, _) = table
        .first()
        .cloned()
        .expect("the configuration table has a row that names an interface");
    let input_arn = created_input(&emulator, case, "ri-assoc-readback", configuration).await;

    let (status, code, refused) = delete_interface(&emulator, &interface_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (409, Some("ConflictException")),
        "case {case:?}: the delete is refused, got {status} {code:?}: {refused}"
    );

    let (status, code, read) = get_router_input(&emulator, &input_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: the input reads back, got {status} {code:?}: {read}"
    );
    let reported = union_member(
        &read["routerInput"]["configuration"],
        member_name,
        json_name,
    )["networkInterfaceArn"]
        .as_str();
    assert_eq!(
        reported,
        Some(interface_arn.as_str()),
        "case {case:?}: the input still names the interface it ingests on, got {read}"
    );
    let resolved = reported_interface(&emulator, &interface_arn).await;
    assert_eq!(
        resolved
            .as_ref()
            .map(|(_, inputs, outputs)| (*inputs, *outputs)),
        Some((1, 0)),
        "case {case:?}: the ARN the input names still resolves and counts it, got {resolved:?}"
    );
    assert_no_dangling_references(&emulator, case);
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of what a core network reports about the segment actions of the
//! policy document in force on it, driven over HTTP through the real router (the harness
//! lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a core network's reported topology is the materialization of the
//! policy document in force, so each segment's `SharedSegments` are the segments the
//! document's `segment-actions` share its routes with, and each network function group's
//! `Segments` are the segments those same actions send through the group — `SendVia` for
//! the `send-via` action and `SendTo` for the `send-to` one — with an empty list where the
//! document declares no such action.
//!
//! The table below spans that class: documents differing in how `share-with` is spelled
//! (an explicit list, `*`, an `except` object, an empty list), in how many actions act on
//! one segment, in whether an action names a segment or a group the document never
//! declares, in which of the two service-insertion actions is used and in how many groups
//! one action names, and in the order the segments are declared against the order the
//! actions name them.
//!
//! The document's own key names (`segment-actions`, `action`, `segment`, `share-with`,
//! `except`, `via`, `network-function-groups`) and its action names (`share`, `send-via`,
//! `send-to`) are the published Cloud WAN core network policy schema's; the two
//! service-insertion action names are also the values the model enumerates for
//! `SegmentActionServiceInsertion` (`smithy_query shape=ServiceInsertionAction`), whose
//! `Via$NetworkFunctionGroups` is the member a group is named through.

use serde_json::{json, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// One row of the table: a policy document, and the segments and network function groups a
/// core network holding it reports once its policy is in force.
struct SegmentActionCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The policy document the create carries.
    document: Value,
    /// The `Segments` the document's segment actions describe.
    segments: Value,
    /// The `NetworkFunctionGroups` the document's segment actions describe.
    network_function_groups: Value,
}

/// A policy document carrying one edge location, the named `segments`, the named
/// `network-function-groups` and `actions` as its `segment-actions`.
fn document(segments: &[&str], groups: &[&str], actions: Value) -> Value {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2" }],
        },
        "segments": segments.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
        "network-function-groups": groups
            .iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<_>>(),
        "segment-actions": actions,
    })
}

/// The reported segment `name`, present in the one edge location every document here
/// configures, sharing with `shared`.
fn segment(name: &str, shared: &[&str]) -> Value {
    json!({ "Name": name, "EdgeLocations": ["us-west-2"], "SharedSegments": shared })
}

/// The reported network function group `name`, serving `send_via` under the `send-via`
/// action and `send_to` under the `send-to` one.
fn group(name: &str, send_via: &[&str], send_to: &[&str]) -> Value {
    json!({
        "Name": name,
        "EdgeLocations": ["us-west-2"],
        "Segments": { "SendVia": send_via, "SendTo": send_to },
    })
}

/// The table: one row per form a `segment-actions` list takes.
fn segment_action_cases() -> Vec<SegmentActionCase> {
    vec![
        SegmentActionCase {
            name: "a share action naming one segment",
            document: document(
                &["prod", "shared"],
                &[],
                json!([{
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "shared",
                    "share-with": ["prod"],
                }]),
            ),
            // Only the segment the action acts on shares its routes.
            segments: json!([segment("prod", &[]), segment("shared", &["prod"])]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "no segment actions at all",
            document: document(&["prod", "shared"], &[], json!([])),
            segments: json!([segment("prod", &[]), segment("shared", &[])]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "share-with naming every other segment",
            document: document(
                &["alpha", "beta", "gamma"],
                &[],
                json!([{ "action": "share", "mode": "attachment-route", "segment": "alpha", "share-with": "*" }]),
            ),
            segments: json!([
                segment("alpha", &["beta", "gamma"]),
                segment("beta", &[]),
                segment("gamma", &[]),
            ]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "share-with excepting one segment",
            document: document(
                &["alpha", "beta", "gamma", "delta"],
                &[],
                json!([{
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "alpha",
                    "share-with": { "except": ["gamma"] },
                }]),
            ),
            segments: json!([
                segment("alpha", &["beta", "delta"]),
                segment("beta", &[]),
                segment("gamma", &[]),
                segment("delta", &[]),
            ]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "share-with naming nothing",
            document: document(
                &["alpha", "beta"],
                &[],
                json!([{ "action": "share", "mode": "attachment-route", "segment": "alpha", "share-with": [] }]),
            ),
            segments: json!([segment("alpha", &[]), segment("beta", &[])]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "two share actions on one segment, naming a segment twice, itself and one that is not declared",
            document: document(
                &["one", "two", "three"],
                &[],
                json!([
                    {
                        "action": "share",
                        "mode": "attachment-route",
                        "segment": "one",
                        "share-with": ["three", "one", "absent"],
                    },
                    {
                        "action": "share",
                        "mode": "attachment-route",
                        "segment": "one",
                        "share-with": ["two", "three"],
                    },
                ]),
            ),
            // The union of both actions, each segment named once, in the order the
            // document declares the segments — never the sharing segment itself, and never
            // a name no `segments` entry declares.
            segments: json!([
                segment("one", &["two", "three"]),
                segment("two", &[]),
                segment("three", &[]),
            ]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "shared segments follow the declaration order, not the action's",
            document: document(
                &["zulu", "yankee", "xray"],
                &[],
                json!([{
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "zulu",
                    "share-with": ["xray", "yankee"],
                }]),
            ),
            segments: json!([
                segment("zulu", &["yankee", "xray"]),
                segment("yankee", &[]),
                segment("xray", &[]),
            ]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "an action acting on a segment the document never declares",
            document: document(
                &["alpha"],
                &[],
                json!([{
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "absent",
                    "share-with": ["alpha"],
                }]),
            ),
            segments: json!([segment("alpha", &[])]),
            network_function_groups: json!([]),
        },
        SegmentActionCase {
            name: "an action of a kind that shares nothing and sends nothing",
            document: document(
                &["alpha", "beta"],
                &["inspection"],
                json!([{
                    "action": "create-route",
                    "segment": "alpha",
                    "destination-cidr-blocks": ["0.0.0.0/0"],
                }]),
            ),
            segments: json!([segment("alpha", &[]), segment("beta", &[])]),
            network_function_groups: json!([group("inspection", &[], &[])]),
        },
        SegmentActionCase {
            name: "a send-via action naming one group",
            document: document(
                &["prod", "dev"],
                &["inspection"],
                json!([{
                    "action": "send-via",
                    "segment": "prod",
                    "mode": "dual-hop",
                    "via": { "network-function-groups": ["inspection"] },
                }]),
            ),
            segments: json!([segment("prod", &[]), segment("dev", &[])]),
            network_function_groups: json!([group("inspection", &["prod"], &[])]),
        },
        SegmentActionCase {
            name: "a send-via naming two groups beside a send-to, and a share",
            document: document(
                &["alpha", "beta"],
                &["inspection", "egress"],
                json!([
                    {
                        "action": "send-via",
                        "segment": "alpha",
                        "mode": "single-hop",
                        "via": { "network-function-groups": ["inspection", "egress"] },
                    },
                    {
                        "action": "send-to",
                        "segment": "beta",
                        "via": { "network-function-groups": ["inspection"] },
                    },
                    {
                        "action": "share",
                        "mode": "attachment-route",
                        "segment": "beta",
                        "share-with": ["alpha"],
                    },
                ]),
            ),
            // The two actions are reported through separate members, and a share action
            // beside them is still reported on the segment it acts on.
            segments: json!([segment("alpha", &[]), segment("beta", &["alpha"])]),
            network_function_groups: json!([
                group("inspection", &["alpha"], &["beta"]),
                group("egress", &["alpha"], &[]),
            ]),
        },
        SegmentActionCase {
            name: "a send-via naming a group the document never declares, and one naming no group at all",
            document: document(
                &["alpha"],
                &["inspection"],
                json!([
                    {
                        "action": "send-via",
                        "segment": "alpha",
                        "via": { "network-function-groups": ["absent"] },
                    },
                    { "action": "send-to", "segment": "alpha" },
                ]),
            ),
            segments: json!([segment("alpha", &[])]),
            network_function_groups: json!([group("inspection", &[], &[])]),
        },
        SegmentActionCase {
            name: "served segments follow the declaration order, not the actions'",
            document: document(
                &["zulu", "yankee", "xray"],
                &["inspection"],
                json!([
                    {
                        "action": "send-via",
                        "segment": "xray",
                        "via": { "network-function-groups": ["inspection"] },
                    },
                    {
                        "action": "send-via",
                        "segment": "zulu",
                        "via": { "network-function-groups": ["inspection"] },
                    },
                ]),
            ),
            segments: json!([
                segment("zulu", &[]),
                segment("yankee", &[]),
                segment("xray", &[]),
            ]),
            network_function_groups: json!([group("inspection", &["zulu", "xray"], &[])]),
        },
    ]
}

#[tokio::test]
async fn core_network_segment_actions_are_reported_from_the_live_policy() {
    let emulator = Emulator::new();
    let cases = segment_action_cases();
    let mut created: Vec<(&SegmentActionCase, String)> = Vec::new();

    for case in &cases {
        // One core network per global network: a global network holds a single one.
        let network_id = emulator.create_global_network(case.name).await;
        let (status, response) = create_core_network(
            &emulator,
            json!({
                "GlobalNetworkId": network_id,
                "Description": case.name,
                "PolicyDocument": case.document.to_string(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCoreNetwork expected 200, got {status} with {response}",
            case.name
        );
        let id = response["CoreNetwork"]["CoreNetworkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no CoreNetworkId in {response}", case.name))
            .to_string();
        created.push((case, id));
    }

    // A core network reports its topology only once the policy it was created with is in
    // force, which is what the transition the clock owes stands for.
    tokio::time::sleep(PAST_SETTLE).await;

    for (case, id) in &created {
        let (status, body) = emulator.get(&format!("/core-networks/{id}")).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetwork expected 200, got {status} with {body}",
            case.name
        );
        let core_network = &body["CoreNetwork"];
        assert_eq!(
            core_network["State"],
            json!("AVAILABLE"),
            "case {:?}: past the settle span the core network is AVAILABLE, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Segments"], case.segments,
            "case {:?}: expected the segments the policy's segment actions describe, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["NetworkFunctionGroups"], case.network_function_groups,
            "case {:?}: expected the groups the policy's segment actions describe, got {core_network}",
            case.name
        );

        // Reporting the segment actions is a pure read: a second read answers the same.
        let (_, again) = emulator.get(&format!("/core-networks/{id}")).await;
        assert_eq!(
            again, body,
            "case {:?}: GetCoreNetwork must be a pure read, got {body} then {again}",
            case.name
        );
    }
}

/// How many names one `segment-actions` entry may list before the document is refused: the
/// caps `state::MAX_POLICY_SEGMENTS` / `MAX_POLICY_NETWORK_FUNCTION_GROUPS` fix, which are
/// the counts of the things such a list may name, so a longer list can only repeat names.
const MAX_NAMES_PER_ENTRY: usize = 100;

/// How many prefixes one `create-route` entry may declare, and how many destinations it may
/// name, before the document is refused: the caps `state::MAX_POLICY_ROUTE_PREFIXES` /
/// `MAX_POLICY_ROUTE_DESTINATIONS` fix, which are what bound the routes one segment holds
/// and so the listing of its routing information.
const MAX_ROUTE_PREFIXES_PER_ENTRY: usize = 50;
/// See [`MAX_ROUTE_PREFIXES_PER_ENTRY`].
const MAX_ROUTE_DESTINATIONS_PER_ENTRY: usize = 10;

/// `count` segment names, so a `segment-actions` entry can be given a list of a chosen
/// length.
fn names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("name-{index}")).collect()
}

/// One row of the name-list cap table: how an over-long entry is spelled, the bound its
/// list is capped at, and the path a refusal names.
struct NameListCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The `segment-actions` entry carrying `count` names.
    entry: fn(usize) -> Value,
    /// How many entries that list may carry before the document is refused.
    bound: usize,
    /// The `Path` the refusal of an over-long list carries.
    path: &'static str,
}

#[tokio::test]
async fn a_segment_action_name_list_is_capped_at_its_bound() {
    let emulator = Emulator::new();
    let rows = [
        NameListCase {
            name: "share-with",
            entry: |count| {
                json!({
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "alpha",
                    "share-with": names(count),
                })
            },
            path: "$.segment-actions[0].share-with",
            bound: MAX_NAMES_PER_ENTRY,
        },
        NameListCase {
            name: "share-with except",
            entry: |count| {
                json!({
                    "action": "share",
                    "mode": "attachment-route",
                    "segment": "alpha",
                    "share-with": { "except": names(count) },
                })
            },
            path: "$.segment-actions[0].share-with.except",
            bound: MAX_NAMES_PER_ENTRY,
        },
        NameListCase {
            name: "via network-function-groups",
            entry: |count| {
                json!({
                    "action": "send-via",
                    "segment": "alpha",
                    "via": { "network-function-groups": names(count) },
                })
            },
            path: "$.segment-actions[0].via.network-function-groups",
            bound: MAX_NAMES_PER_ENTRY,
        },
        NameListCase {
            name: "create-route destination-cidr-blocks",
            entry: |count| {
                json!({
                    "action": "create-route",
                    "segment": "alpha",
                    "destination-cidr-blocks": names(count),
                    "destinations": ["blackhole"],
                })
            },
            path: "$.segment-actions[0].destination-cidr-blocks",
            bound: MAX_ROUTE_PREFIXES_PER_ENTRY,
        },
        NameListCase {
            name: "create-route destinations",
            entry: |count| {
                json!({
                    "action": "create-route",
                    "segment": "alpha",
                    "destination-cidr-blocks": ["10.0.0.0/16"],
                    "destinations": names(count),
                })
            },
            path: "$.segment-actions[0].destinations",
            bound: MAX_ROUTE_DESTINATIONS_PER_ENTRY,
        },
    ];

    for NameListCase {
        name: case,
        entry,
        bound,
        path,
    } in rows
    {
        let at_bound = document(&["alpha"], &["inspection"], json!([entry(bound)]));
        let network_id = emulator.create_global_network(case).await;
        let (status, accepted) = create_core_network(
            &emulator,
            json!({ "GlobalNetworkId": network_id, "PolicyDocument": at_bound.to_string() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {bound} entries is at the bound and accepted, got {status} with {accepted}"
        );

        let over = document(&["alpha"], &["inspection"], json!([entry(bound + 1)]));
        let over_id = emulator.create_global_network("over the bound").await;
        let (status, refused) = create_core_network(
            &emulator,
            json!({ "GlobalNetworkId": over_id, "PolicyDocument": over.to_string() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: one name past the bound is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "CoreNetworkPolicyException",
            "case {case:?}: expected CoreNetworkPolicyException, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["ErrorCode"],
            json!("TOO_MANY_ENTRIES"),
            "case {case:?}: expected the over-size error code, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["Path"],
            json!(path),
            "case {case:?}: the refusal must name the over-size list, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_policy_only_put_does_not_move_the_reported_segment_actions() {
    let emulator = Emulator::new();
    let network_id = emulator.create_global_network("sharing changes").await;
    let first = document(
        &["prod", "shared"],
        &["inspection"],
        json!([{
            "action": "share",
            "mode": "attachment-route",
            "segment": "shared",
            "share-with": ["prod"],
        }]),
    );
    let (status, response) = create_core_network(
        &emulator,
        json!({
            "GlobalNetworkId": network_id,
            "PolicyDocument": first.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {response}"
    );
    let id = response["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {response}"))
        .to_string();

    tokio::time::sleep(PAST_SETTLE).await;
    let (_, body) = emulator.get(&format!("/core-networks/{id}")).await;
    assert_eq!(
        body["CoreNetwork"]["Segments"],
        json!([segment("prod", &[]), segment("shared", &["prod"])]),
        "the sharing the created policy declares must be reported, got {body}"
    );
    assert_eq!(
        body["CoreNetwork"]["NetworkFunctionGroups"],
        json!([group("inspection", &[], &[])]),
        "the created policy sends no segment through the group, got {body}"
    );

    // A version that is only PUT is not in force, so the reported topology is the one the
    // document in force declares and not the one just stored.
    let second = document(
        &["prod", "shared"],
        &["inspection"],
        json!([{
            "action": "send-via",
            "segment": "prod",
            "via": { "network-function-groups": ["inspection"] },
        }]),
    );
    let (status, put) = emulator
        .post(
            &format!("/core-networks/{id}/core-network-policy"),
            json!({ "PolicyDocument": second.to_string() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {put}"
    );
    let (_, after) = emulator.get(&format!("/core-networks/{id}")).await;
    assert_eq!(
        after["CoreNetwork"]["Segments"], body["CoreNetwork"]["Segments"],
        "only the policy in force shapes the reported sharing, got {after}"
    );
    assert_eq!(
        after["CoreNetwork"]["NetworkFunctionGroups"], body["CoreNetwork"]["NetworkFunctionGroups"],
        "only the policy in force shapes the reported groups, got {after}"
    );
}

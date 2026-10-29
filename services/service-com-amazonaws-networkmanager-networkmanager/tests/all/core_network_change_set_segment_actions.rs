// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the change set a policy version whose document differs in its
//! `segment-actions` describes, driven over HTTP through the real router (the harness lives
//! in `crate::global_network_lifecycle`).
//!
//! The rule under test: a version's change set reports EVERY object the policy that was in
//! force and the version's own document configure differently, and what a document states
//! about one object includes the `segment-actions` entries acting on it — so a segment whose
//! routes are shared with other segments, or whose traffic is served by other network
//! function groups, is `MODIFY`ed (carrying those values on each side), and each static route
//! a `create-route` entry declares for a segment is its own object, `ADD`ed or `REMOVE`d once
//! per prefix and destination it names. Two documents stating the same thing about every
//! object change nothing, however they spell it. Nothing here reaches into
//! `crate::state`: every change is read back through the operation, and what executing it
//! does is read back through `GetCoreNetwork` and `ListCoreNetworkRoutingInformation`.
//!
//! The table below spans that class: documents differing in which kind of action changed
//! (`share`, `send-via` / `send-to`, `create-route`), in the direction of the change (an
//! action added, removed, widened, moved to the other action), in how many segments, groups,
//! prefixes, destinations and edge locations one difference expands over, in the form
//! `share-with` takes (a list, `*`), in whether the action names anything the document
//! declares, and in the ORDER and multiplicity the entries are listed in — which no object's
//! configuration depends on.

use serde_json::{json, Map, Value};

use crate::core_network_policy_versions::core_network_with_policy;
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};

/// The two Regions the documents below configure edges in. Any Region code satisfies
/// `ExternalRegionCode` (`smithy_query shape=CoreNetworkEdge member=EdgeLocation`: length
/// 1..63); these are spelled out so a row's expected paths can name them.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const EUROPE: &str = "eu-west-1";

/// A policy document configuring one edge per entry of `locations`, one segment per entry of
/// `segments` (each present in every one of those edge locations, since none restricts
/// itself), one network function group per entry of `groups`, and `actions` as its
/// `segment-actions`.
///
/// The key names (`version`, `core-network-configuration`, `asn-ranges`, `edge-locations`,
/// `location`, `asn`, `segments`, `network-function-groups`, `segment-actions`) are the
/// published Cloud WAN core network policy schema's, the same ones
/// `crate::core_network_segment_actions` builds with. Every edge names its own `asn`, so no
/// edge takes one from `asn-ranges` and a difference in the actions is never visible as a
/// difference in an edge.
fn document(locations: &[&str], segments: &[&str], groups: &[&str], actions: Value) -> Value {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": locations
                .iter()
                .enumerate()
                .map(|(index, location)| json!({
                    "location": location,
                    "asn": 64512 + i64::try_from(index).unwrap_or_default(),
                }))
                .collect::<Vec<Value>>(),
        },
        "segments": segments
            .iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<Value>>(),
        "network-function-groups": groups
            .iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<Value>>(),
        "segment-actions": actions,
    })
}

/// A `share` entry: the segment whose routes are shared, and how `share-with` names the
/// segments they are shared with.
fn share(segment: &str, with: Value) -> Value {
    json!({
        "action": "share",
        "mode": "attachment-route",
        "segment": segment,
        "share-with": with,
    })
}

/// A service-insertion entry (`send-via` or `send-to`): the segment it acts on, and the
/// network function groups it sends that segment's traffic through.
fn send(action: &str, segment: &str, groups: &[&str]) -> Value {
    json!({
        "action": action,
        "segment": segment,
        "via": { "network-function-groups": groups },
    })
}

/// A `create-route` entry: the prefixes it declares for a segment, and what they reach.
fn create_route(segment: &str, prefixes: &[&str], destinations: &[&str]) -> Value {
    json!({
        "action": "create-route",
        "segment": segment,
        "destination-cidr-blocks": prefixes,
        "destinations": destinations,
    })
}

/// The values a change reports for the segment `name` at `location`: its name and that edge
/// location, plus the segments its routes are shared with and the service insertion actions
/// acting on it where the document declares any (`[]` stands for "none", which the change
/// leaves absent rather than reporting as an empty list).
///
/// Each `insertions` entry is an action name beside the groups it names, which the change
/// reports through `ServiceInsertionAction$Action` and `Via$NetworkFunctionGroups`
/// (`smithy_query shape=ServiceInsertionAction`).
fn segment_values(
    location: &str,
    name: &str,
    shared: &[&str],
    insertions: &[(&str, &[&str])],
) -> Value {
    let mut values = json!({ "SegmentName": name, "EdgeLocations": [location] });
    if !shared.is_empty() {
        values["SharedSegments"] = json!(shared);
    }
    if !insertions.is_empty() {
        values["ServiceInsertionActions"] = json!(insertions
            .iter()
            .map(|(action, groups)| json!({
                "Action": action,
                "NetworkFunctionGroups": groups,
            }))
            .collect::<Vec<Value>>());
    }
    values
}

/// The change one version makes to the segment `name` at `location`.
fn segment_change(action: &str, location: &str, name: &str, previous: Value, new: Value) -> Value {
    let mut change = json!({
        "Type": "CORE_NETWORK_SEGMENT",
        "Action": action,
        "Identifier": name,
        "IdentifierPath": format!("CORE_NETWORK_SEGMENT/{location}/{name}"),
    });
    if action != "ADD" {
        change["PreviousValues"] = previous;
    }
    if action != "REMOVE" {
        change["NewValues"] = new;
    }
    change
}

/// The change one version makes to the static route of `segment` carrying `prefix` and
/// reached through `destination`.
///
/// A `create-route` entry declares the route for the segment, which holds it in its route
/// table in every edge location it is present in, so one declaration is ONE change and names
/// no edge location.
fn route_change(action: &str, segment: &str, prefix: &str, destination: &str) -> Value {
    let values = json!({
        "SegmentName": segment,
        "Cidr": prefix,
        "DestinationIdentifier": destination,
    });
    let mut change = json!({
        "Type": "ATTACHMENT_ROUTE_STATIC",
        "Action": action,
        "Identifier": prefix,
        "IdentifierPath":
            format!("ATTACHMENT_ROUTE_STATIC/{segment}/{prefix}/{destination}"),
    });
    if action == "ADD" {
        change["NewValues"] = values;
    } else {
        change["PreviousValues"] = values;
    }
    change
}

/// The members of one reported `ServiceInsertionAction` this suite asserts on: the action it
/// takes, and the names of the network function groups it sends the segment's traffic
/// through.
fn insertion_summary(insertion: &Value) -> Value {
    json!({
        "Action": insertion["Action"],
        "NetworkFunctionGroups": insertion["Via"]["NetworkFunctionGroups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group["Name"].clone())
                    .collect::<Vec<Value>>()
            })
            .unwrap_or_default(),
    })
}

/// The members of one side's values this suite asserts on. A member the side leaves absent
/// is left out, so a row spells only what the change carries.
fn values_summary(values: &Value) -> Value {
    let mut picked = Map::new();
    for key in [
        "SegmentName",
        "NetworkFunctionGroupName",
        "EdgeLocations",
        "SharedSegments",
        "Cidr",
        "DestinationIdentifier",
    ] {
        if !values[key].is_null() {
            picked.insert(key.to_string(), values[key].clone());
        }
    }
    if let Some(insertions) = values["ServiceInsertionActions"].as_array() {
        picked.insert(
            "ServiceInsertionActions".to_string(),
            Value::Array(insertions.iter().map(insertion_summary).collect()),
        );
    }
    Value::Object(picked)
}

/// One reported change, reduced to the members this suite asserts on.
fn summary(change: &Value) -> Value {
    let mut picked = Map::new();
    for key in ["Type", "Action", "Identifier", "IdentifierPath"] {
        if !change[key].is_null() {
            picked.insert(key.to_string(), change[key].clone());
        }
    }
    for side in ["PreviousValues", "NewValues"] {
        if !change[side].is_null() {
            picked.insert(side.to_string(), values_summary(&change[side]));
        }
    }
    Value::Object(picked)
}

/// The URI one version's change set is addressed at.
fn change_set_uri(core_network_id: &str, version_id: i64) -> String {
    format!("/core-networks/{core_network_id}/core-network-change-sets/{version_id}")
}

/// Every change of one version's change set, reduced by [`summary`] and in the order
/// reported.
async fn change_set(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    version_id: i64,
) -> Vec<Value> {
    let (status, body) = emulator
        .get(&change_set_uri(core_network_id, version_id))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetworkChangeSet expected 200, got {status} with {body}"
    );
    body["CoreNetworkChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkChanges array in {body}"))
        .iter()
        .map(summary)
        .collect()
}

/// `PutCoreNetworkPolicy` of `document`, answering the version id it minted.
async fn put_policy(emulator: &Emulator, case: &str, core_network_id: &str, document: &Value) -> i64 {
    let (status, body) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-policy"),
            json!({ "PolicyDocument": document.to_string() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutCoreNetworkPolicy expected 200, got {status} with {body}"
    );
    body["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no PolicyVersionId in {body}"))
}

/// One row of the table: the document in force, the document submitted on top of it, and the
/// changes the submitted version's change set describes, in `IdentifierPath` order.
struct SegmentActionChangeCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The document the create carries and executes.
    live: Value,
    /// The document a put submits on top of it.
    submitted: Value,
    /// The changes the submitted version describes.
    changes: Vec<Value>,
}

/// The table: one row per difference a `segment-actions` block can carry.
fn segment_action_change_cases() -> Vec<SegmentActionChangeCase> {
    vec![
        SegmentActionChangeCase {
            name: "a share action added modifies the segment whose routes it shares",
            live: document(&[WEST], &["prod", "shared"], &[], json!([])),
            submitted: document(
                &[WEST],
                &["prod", "shared"],
                &[],
                json!([share("shared", json!(["prod"]))]),
            ),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "shared",
                segment_values(WEST, "shared", &[], &[]),
                segment_values(WEST, "shared", &["prod"], &[]),
            )],
        },
        SegmentActionChangeCase {
            name: "a share action removed modifies it back",
            live: document(
                &[WEST],
                &["prod", "shared"],
                &[],
                json!([share("shared", json!(["prod"]))]),
            ),
            submitted: document(&[WEST], &["prod", "shared"], &[], json!([])),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "shared",
                segment_values(WEST, "shared", &["prod"], &[]),
                segment_values(WEST, "shared", &[], &[]),
            )],
        },
        SegmentActionChangeCase {
            name: "share-with widened from one segment to every other one",
            live: document(
                &[WEST],
                &["alpha", "beta", "gamma"],
                &[],
                json!([share("alpha", json!(["beta"]))]),
            ),
            submitted: document(
                &[WEST],
                &["alpha", "beta", "gamma"],
                &[],
                json!([share("alpha", json!("*"))]),
            ),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "alpha",
                segment_values(WEST, "alpha", &["beta"], &[]),
                segment_values(WEST, "alpha", &["beta", "gamma"], &[]),
            )],
        },
        SegmentActionChangeCase {
            name: "the same sharing spelled over two entries, in another order, changes nothing",
            live: document(
                &[WEST],
                &["alpha", "beta", "gamma"],
                &[],
                json!([
                    share("alpha", json!(["beta"])),
                    share("alpha", json!(["gamma"])),
                ]),
            ),
            submitted: document(
                &[WEST],
                &["alpha", "beta", "gamma"],
                &[],
                json!([share("alpha", json!(["gamma", "beta"]))]),
            ),
            changes: Vec::new(),
        },
        SegmentActionChangeCase {
            name: "a send-via action added modifies the segment it acts on",
            live: document(&[WEST], &["prod"], &["inspection"], json!([])),
            submitted: document(
                &[WEST],
                &["prod"],
                &["inspection"],
                json!([send("send-via", "prod", &["inspection"])]),
            ),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "prod",
                segment_values(WEST, "prod", &[], &[]),
                segment_values(WEST, "prod", &[], &[("send-via", &["inspection"])]),
            )],
        },
        SegmentActionChangeCase {
            name: "the same groups under the other action is a different insertion",
            live: document(
                &[WEST],
                &["prod"],
                &["inspection"],
                json!([send("send-via", "prod", &["inspection"])]),
            ),
            submitted: document(
                &[WEST],
                &["prod"],
                &["inspection"],
                json!([send("send-to", "prod", &["inspection"])]),
            ),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "prod",
                segment_values(WEST, "prod", &[], &[("send-via", &["inspection"])]),
                segment_values(WEST, "prod", &[], &[("send-to", &["inspection"])]),
            )],
        },
        SegmentActionChangeCase {
            name: "a second group named by the action modifies the segment",
            live: document(
                &[WEST],
                &["alpha"],
                &["inspection", "egress"],
                json!([send("send-via", "alpha", &["inspection"])]),
            ),
            submitted: document(
                &[WEST],
                &["alpha"],
                &["inspection", "egress"],
                json!([send("send-via", "alpha", &["egress", "inspection"])]),
            ),
            changes: vec![segment_change(
                "MODIFY",
                WEST,
                "alpha",
                segment_values(WEST, "alpha", &[], &[("send-via", &["inspection"])]),
                segment_values(
                    WEST,
                    "alpha",
                    &[],
                    &[("send-via", &["egress", "inspection"])],
                ),
            )],
        },
        SegmentActionChangeCase {
            name: "two insertion entries on one segment listed in the other order change nothing",
            live: document(
                &[WEST],
                &["alpha"],
                &["inspection", "egress"],
                json!([
                    send("send-via", "alpha", &["inspection"]),
                    send("send-to", "alpha", &["egress"]),
                ]),
            ),
            submitted: document(
                &[WEST],
                &["alpha"],
                &["inspection", "egress"],
                json!([
                    send("send-to", "alpha", &["egress"]),
                    send("send-via", "alpha", &["inspection"]),
                ]),
            ),
            changes: Vec::new(),
        },
        SegmentActionChangeCase {
            name: "a create-route entry added declares one route per prefix",
            live: document(&[WEST], &["alpha"], &[], json!([])),
            submitted: document(
                &[WEST],
                &["alpha"],
                &[],
                json!([create_route(
                    "alpha",
                    &["10.0.0.0/16", "10.1.0.0/16"],
                    &["blackhole"],
                )]),
            ),
            changes: vec![
                route_change("ADD", "alpha", "10.0.0.0/16", "blackhole"),
                route_change("ADD", "alpha", "10.1.0.0/16", "blackhole"),
            ],
        },
        SegmentActionChangeCase {
            name: "a create-route entry removed removes the route it declared",
            live: document(
                &[WEST],
                &["alpha"],
                &[],
                json!([create_route("alpha", &["0.0.0.0/0"], &["blackhole"])]),
            ),
            submitted: document(&[WEST], &["alpha"], &[], json!([])),
            changes: vec![route_change("REMOVE", "alpha", "0.0.0.0/0", "blackhole")],
        },
        SegmentActionChangeCase {
            name: "a route declared for a segment spanning two edge locations is one route",
            live: document(&[WEST, EUROPE], &["alpha"], &[], json!([])),
            submitted: document(
                &[WEST, EUROPE],
                &["alpha"],
                &[],
                json!([create_route("alpha", &["10.2.0.0/16"], &["blackhole"])]),
            ),
            // The entry declares the route for the segment, which holds it wherever it is
            // present, so the edge locations it spans do not multiply the declaration.
            changes: vec![route_change("ADD", "alpha", "10.2.0.0/16", "blackhole")],
        },
        SegmentActionChangeCase {
            name: "one entry naming two prefixes and two destinations declares four routes",
            live: document(&[WEST], &["alpha"], &[], json!([])),
            submitted: document(
                &[WEST],
                &["alpha"],
                &[],
                json!([create_route(
                    "alpha",
                    &["172.16.0.0/16", "172.17.0.0/16"],
                    &["blackhole", "attachment-0123456789abcdef0"],
                )]),
            ),
            // In `IdentifierPath` order: by prefix, then by what it is reached through.
            changes: vec![
                route_change("ADD", "alpha", "172.16.0.0/16", "attachment-0123456789abcdef0"),
                route_change("ADD", "alpha", "172.16.0.0/16", "blackhole"),
                route_change("ADD", "alpha", "172.17.0.0/16", "attachment-0123456789abcdef0"),
                route_change("ADD", "alpha", "172.17.0.0/16", "blackhole"),
            ],
        },
        SegmentActionChangeCase {
            name: "an action naming a segment the document never declares changes nothing",
            live: document(&[WEST], &["alpha"], &[], json!([])),
            submitted: document(
                &[WEST],
                &["alpha"],
                &[],
                json!([share("absent", json!(["alpha"]))]),
            ),
            changes: Vec::new(),
        },
        SegmentActionChangeCase {
            name: "a segment added together with a share carries it where it is added",
            live: document(&[WEST], &["alpha"], &[], json!([])),
            submitted: document(
                &[WEST],
                &["alpha", "beta"],
                &[],
                json!([share("beta", json!(["alpha"]))]),
            ),
            changes: vec![segment_change(
                "ADD",
                WEST,
                "beta",
                Value::Null,
                segment_values(WEST, "beta", &["alpha"], &[]),
            )],
        },
        SegmentActionChangeCase {
            name: "every kind of action at once",
            live: document(&[WEST], &["alpha", "beta"], &["inspection"], json!([])),
            submitted: document(
                &[WEST],
                &["alpha", "beta"],
                &["inspection"],
                json!([
                    share("beta", json!(["alpha"])),
                    send("send-via", "alpha", &["inspection"]),
                    create_route("alpha", &["192.0.2.0/24"], &["blackhole"]),
                ]),
            ),
            // In `IdentifierPath` order, which is what the operation reports them in.
            changes: vec![
                route_change("ADD", "alpha", "192.0.2.0/24", "blackhole"),
                segment_change(
                    "MODIFY",
                    WEST,
                    "alpha",
                    segment_values(WEST, "alpha", &[], &[]),
                    segment_values(WEST, "alpha", &[], &[("send-via", &["inspection"])]),
                ),
                segment_change(
                    "MODIFY",
                    WEST,
                    "beta",
                    segment_values(WEST, "beta", &[], &[]),
                    segment_values(WEST, "beta", &["alpha"], &[]),
                ),
            ],
        },
        SegmentActionChangeCase {
            name: "a document stating the same actions changes nothing",
            live: document(
                &[WEST],
                &["alpha", "beta"],
                &["inspection"],
                json!([
                    share("beta", json!(["alpha"])),
                    send("send-to", "alpha", &["inspection"]),
                    create_route("alpha", &["198.51.100.0/24"], &["blackhole"]),
                ]),
            ),
            submitted: document(
                &[WEST],
                &["alpha", "beta"],
                &["inspection"],
                json!([
                    share("beta", json!(["alpha"])),
                    send("send-to", "alpha", &["inspection"]),
                    create_route("alpha", &["198.51.100.0/24"], &["blackhole"]),
                ]),
            ),
            changes: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_version_differing_in_its_segment_actions_reports_what_it_changes() {
    let emulator = Emulator::new();

    for case in segment_action_change_cases() {
        let core_network_id =
            core_network_with_policy(&emulator, case.name, &case.live.to_string()).await;
        let version_id = put_policy(&emulator, case.name, &core_network_id, &case.submitted).await;
        let changes = change_set(&emulator, case.name, &core_network_id, version_id).await;
        assert_eq!(
            changes, case.changes,
            "case {:?}: expected the changes the two documents differ in, got {changes:#?}",
            case.name
        );

        // The change events of that version describe the SAME objects: one event per change,
        // in the same order.
        let (status, events) = emulator
            .get(&format!(
                "/core-networks/{core_network_id}/core-network-change-events/{version_id}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetworkChangeEvents expected 200, got {status} with {events}",
            case.name
        );
        let paths: Vec<Value> = events["CoreNetworkChangeEvents"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no events array in {events}", case.name))
            .iter()
            .map(|event| event["IdentifierPath"].clone())
            .collect();
        let expected: Vec<Value> = case
            .changes
            .iter()
            .map(|change| change["IdentifierPath"].clone())
            .collect();
        assert_eq!(
            paths, expected,
            "case {:?}: expected one event per change, got {paths:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_change_set_describes_what_executing_the_version_puts_in_force() {
    let emulator = Emulator::new();
    let case = "a version changing only the segment actions";
    let live = document(&[WEST], &["prod", "shared"], &[], json!([]));
    let submitted = document(
        &[WEST],
        &["prod", "shared"],
        &[],
        json!([
            share("shared", json!(["prod"])),
            create_route("shared", &["203.0.113.0/24"], &["blackhole"]),
        ]),
    );
    let core_network_id = core_network_with_policy(&emulator, case, &live.to_string()).await;
    let version_id = put_policy(&emulator, case, &core_network_id, &submitted).await;

    let changes = change_set(&emulator, case, &core_network_id, version_id).await;
    assert_eq!(
        changes,
        vec![
            route_change("ADD", "shared", "203.0.113.0/24", "blackhole"),
            segment_change(
                "MODIFY",
                WEST,
                "shared",
                segment_values(WEST, "shared", &[], &[]),
                segment_values(WEST, "shared", &["prod"], &[]),
            ),
        ],
        "case {case:?}: the change set must list the sharing and the route, got {changes:#?}"
    );

    // Executing it is what puts the document in force, so what the change set listed is what
    // the core network and its routing information then report.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, executed) = emulator
        .post(
            &format!(
                "/core-networks/{core_network_id}/core-network-change-sets/{version_id}/execute"
            ),
            json!({}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ExecuteCoreNetworkChangeSet expected 200, got {status} with {executed}"
    );
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, body) = emulator
        .get(&format!("/core-networks/{core_network_id}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCoreNetwork expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["CoreNetwork"]["Segments"],
        json!([
            { "Name": "prod", "EdgeLocations": [WEST], "SharedSegments": [] },
            { "Name": "shared", "EdgeLocations": [WEST], "SharedSegments": ["prod"] },
        ]),
        "case {case:?}: the executed version must share the segment's routes, got {body}"
    );

    let (status, routes) = emulator
        .post(
            &format!("/core-networks/{core_network_id}/core-network-routing-information"),
            json!({ "SegmentName": "shared", "EdgeLocation": WEST }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCoreNetworkRoutingInformation expected 200, got {status} with {routes}"
    );
    let prefixes: Vec<Value> = routes["CoreNetworkRoutingInformation"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no routing information array in {routes}"))
        .iter()
        .map(|route| route["Prefix"].clone())
        .collect();
    assert_eq!(
        prefixes,
        vec![json!("203.0.113.0/24")],
        "case {case:?}: the executed version must declare the static route the change set listed, got {routes}"
    );
}

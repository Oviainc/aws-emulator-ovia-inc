// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetCoreNetworkChangeSet`, driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the change set of a core network's policy version is the
//! DIFFERENCE between the policy that was in force when that version was stored and the
//! version's own document, object by object of what a document configures: each configured
//! edge, each named segment and network function group in each edge location it is present
//! in, and the `attachment-policies` block as a whole, keyed by the `IdentifierPath` that
//! uniquely identifies it. An object only the submitted policy configures is `ADD`, one
//! only the policy in force configured is `REMOVE`, one both configure differently is
//! `MODIFY` carrying both sets of values, and one both configure the same way is no change;
//! the first version of a core network was submitted against no policy, so its change set
//! adds every object it configures. A policy version the core network does not hold is a
//! 404, never an empty change set. These are the SAME changes `GetCoreNetworkChangeEvents`
//! reports as the events of that version's execution.
//!
//! The table below spans that class: documents differing in how many edge locations they
//! configure, whether an edge names its own ASN or takes one from `asn-ranges`, whether it
//! carries inside CIDR blocks, whether a named entry restricts itself to some edge
//! locations, how many segments and network function groups there are, whether the document
//! configures attachment policies, and a document whose named entries have no edge location
//! to live in at all.

use serde_json::{json, Map, Value};

use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// One row of the change-set table: a policy document and the change set its version
/// describes.
struct ChangeSetCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The policy document the create carries.
    document: Value,
    /// The changes the version describes, in `IdentifierPath` order — the order the
    /// operation reports them in, since the path is what identifies a change.
    changes: Value,
}

/// The table: one row per form a policy document takes.
///
/// The document's own key names (`core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `segments`, `network-function-groups`, `location`, `asn`,
/// `inside-cidr-blocks`) are the published Cloud WAN core network policy schema's. The
/// `ChangeType` names (`CORE_NETWORK_EDGE`, `CORE_NETWORK_SEGMENT`,
/// `NETWORK_FUNCTION_GROUP`) and the `ChangeAction` `ADD` are the model's own
/// (`smithy_query shape=ChangeType,ChangeAction`).
fn change_set_cases() -> Vec<ChangeSetCase> {
    vec![
        ChangeSetCase {
            name: "one edge naming its own ASN and inside CIDR blocks, one segment",
            document: json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64555"],
                    "edge-locations": [{
                        "location": "us-west-2",
                        "asn": 64512,
                        "inside-cidr-blocks": ["10.0.0.0/16"],
                    }],
                },
                "segments": [{ "name": "shared" }],
            }),
            changes: json!([
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "us-west-2",
                    "IdentifierPath": "CORE_NETWORK_EDGE/us-west-2",
                    "NewValues": {
                        "EdgeLocations": ["us-west-2"],
                        "Asn": 64512,
                        "InsideCidrBlocks": ["10.0.0.0/16"],
                    },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "shared",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/us-west-2/shared",
                    "NewValues": { "SegmentName": "shared", "EdgeLocations": ["us-west-2"] },
                },
            ]),
        },
        ChangeSetCase {
            name: "two edges taking ASNs from the range, one restricted segment",
            document: json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64513"],
                    "edge-locations": [{ "location": "us-east-1" }, { "location": "eu-west-1" }],
                },
                "segments": [
                    { "name": "prod", "edge-locations": ["us-east-1"] },
                    { "name": "dev" },
                ],
            }),
            // A segment that restricts itself to no edge location is added in EVERY edge
            // location the core network has, so `dev` is two changes and `prod` one.
            changes: json!([
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "eu-west-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/eu-west-1",
                    "NewValues": { "EdgeLocations": ["eu-west-1"], "Asn": 64513 },
                },
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "us-east-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/us-east-1",
                    "NewValues": { "EdgeLocations": ["us-east-1"], "Asn": 64512 },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "dev",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/eu-west-1/dev",
                    "NewValues": { "SegmentName": "dev", "EdgeLocations": ["eu-west-1"] },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "dev",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/us-east-1/dev",
                    "NewValues": { "SegmentName": "dev", "EdgeLocations": ["us-east-1"] },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "prod",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/us-east-1/prod",
                    "NewValues": { "SegmentName": "prod", "EdgeLocations": ["us-east-1"] },
                },
            ]),
        },
        ChangeSetCase {
            name: "a network function group beside a segment",
            document: json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64520"],
                    "edge-locations": [{ "location": "ap-south-1" }],
                },
                "segments": [{ "name": "core" }],
                "network-function-groups": [{ "name": "inspection" }],
            }),
            changes: json!([
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "ap-south-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/ap-south-1",
                    "NewValues": { "EdgeLocations": ["ap-south-1"], "Asn": 64520 },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "core",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/ap-south-1/core",
                    "NewValues": { "SegmentName": "core", "EdgeLocations": ["ap-south-1"] },
                },
                {
                    "Type": "NETWORK_FUNCTION_GROUP",
                    "Action": "ADD",
                    "Identifier": "inspection",
                    "IdentifierPath": "NETWORK_FUNCTION_GROUP/ap-south-1/inspection",
                    "NewValues": {
                        "NetworkFunctionGroupName": "inspection",
                        "EdgeLocations": ["ap-south-1"],
                    },
                },
            ]),
        },
        ChangeSetCase {
            name: "edges only: a configuration that names no segment or group",
            document: json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64514"],
                    "edge-locations": [
                        { "location": "ca-central-1" },
                        { "location": "sa-east-1", "asn": 64514 },
                    ],
                },
            }),
            changes: json!([
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "ca-central-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/ca-central-1",
                    "NewValues": { "EdgeLocations": ["ca-central-1"], "Asn": 64512 },
                },
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "sa-east-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/sa-east-1",
                    "NewValues": { "EdgeLocations": ["sa-east-1"], "Asn": 64514 },
                },
            ]),
        },
        ChangeSetCase {
            name: "a document that configures attachment policies",
            // The `attachment-policies` block configures no object of its own, so the
            // change it makes names none: one change for the block, carrying neither an
            // `Identifier` nor any values. The rule's own key names
            // (`rule-number`, `conditions`, `type`, `action`, `association-method`) are
            // the published policy schema's, as `crate::attachment_acceptance` spells them.
            document: json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64513"],
                    "edge-locations": [{ "location": "us-west-1" }],
                },
                "segments": [{ "name": "shared" }],
                "attachment-policies": [{
                    "rule-number": 100,
                    "conditions": [{ "type": "any" }],
                    "action": { "association-method": "constant", "segment": "shared" },
                }],
            }),
            changes: json!([
                {
                    "Type": "ATTACHMENT_POLICIES_CONFIGURATION",
                    "Action": "ADD",
                    "IdentifierPath": "ATTACHMENT_POLICIES_CONFIGURATION",
                },
                {
                    "Type": "CORE_NETWORK_EDGE",
                    "Action": "ADD",
                    "Identifier": "us-west-1",
                    "IdentifierPath": "CORE_NETWORK_EDGE/us-west-1",
                    "NewValues": { "EdgeLocations": ["us-west-1"], "Asn": 64512 },
                },
                {
                    "Type": "CORE_NETWORK_SEGMENT",
                    "Action": "ADD",
                    "Identifier": "shared",
                    "IdentifierPath": "CORE_NETWORK_SEGMENT/us-west-1/shared",
                    "NewValues": { "SegmentName": "shared", "EdgeLocations": ["us-west-1"] },
                },
            ]),
        },
        ChangeSetCase {
            name: "named entries with no edge location to live in",
            // A configuration carrying no `edge-locations` is accepted (only `version` and
            // `core-network-configuration` are required), and a segment restricted to no
            // location is then present nowhere — so the version changes nothing.
            document: json!({
                "version": "2021.12",
                "core-network-configuration": { "asn-ranges": ["64512-64555"] },
                "segments": [{ "name": "nowhere" }],
                "network-function-groups": [{ "name": "also-nowhere" }],
            }),
            changes: json!([]),
        },
    ]
}

/// The URI of one core network's change set for `version`.
fn change_set_uri(core_network_id: &str, version: i64) -> String {
    format!("/core-networks/{core_network_id}/core-network-change-sets/{version}")
}

/// Create a core network holding `document` and return its minted id.
async fn core_network_with(emulator: &Emulator, case: &str, document: &Value) -> String {
    let network_id = emulator.create_global_network(case).await;
    let (status, response) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network_id,
            "Description": case,
            "PolicyDocument": document.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCoreNetwork expected 200, got {status} with {response}"
    );
    response["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CoreNetworkId in {response}"))
        .to_string()
}

/// The members of one reported change this suite asserts on: its identity, and the values
/// the policy in force gave the member it changes (`PreviousValues`) beside the ones the
/// submitted policy gives it (`NewValues`). A member the change leaves absent is left out,
/// so a row of the tables below spells only what the change carries.
fn summary(change: &Value) -> Value {
    let mut picked = Map::new();
    for key in ["Type", "Action", "Identifier", "IdentifierPath"] {
        if !change[key].is_null() {
            picked.insert(key.to_string(), change[key].clone());
        }
    }
    for side in ["PreviousValues", "NewValues"] {
        if change[side].is_null() {
            continue;
        }
        let mut values = Map::new();
        for key in [
            "SegmentName",
            "NetworkFunctionGroupName",
            "EdgeLocations",
            "Asn",
            "InsideCidrBlocks",
        ] {
            let held = &change[side][key];
            if !held.is_null() {
                values.insert(key.to_string(), held.clone());
            }
        }
        picked.insert(side.to_string(), Value::Object(values));
    }
    Value::Object(picked)
}

/// The summaries of every change in a `GetCoreNetworkChangeSet` response.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["CoreNetworkChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CoreNetworkChanges array in {body}"))
        .iter()
        .map(summary)
        .collect()
}

#[tokio::test]
async fn the_change_set_of_a_policy_version_is_derived_from_its_policy_document() {
    let emulator = Emulator::new();

    for case in change_set_cases() {
        let core_network_id = core_network_with(&emulator, case.name, &case.document).await;
        let (status, body) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetworkChangeSet expected 200, got {status} with {body}",
            case.name
        );
        assert_eq!(
            Value::Array(summaries(case.name, &body)),
            case.changes,
            "case {:?}: expected the change set the document describes, got {body}",
            case.name
        );
        assert!(
            body["NextToken"].is_null(),
            "case {:?}: a complete change set carries no NextToken, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_change_set_pages_through_every_change_exactly_once() {
    let emulator = Emulator::new();
    // The row with five changes, so the walk crosses several pages.
    let case = change_set_cases()
        .into_iter()
        .max_by_key(|case| case.changes.as_array().map_or(0, Vec::len))
        .expect("the table is not empty");
    let expected: Vec<Value> = case
        .changes
        .as_array()
        .expect("the row's changes are an array")
        .clone();
    let core_network_id = core_network_with(&emulator, case.name, &case.document).await;

    for page_size in [1_usize, 2, expected.len(), expected.len() + 1] {
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        // One more request than pages can be needed, and never more: the loop is bounded
        // by the change count whatever the service answers.
        for _ in 0..=expected.len() {
            let mut uri = format!("{}?maxResults={page_size}", change_set_uri(&core_network_id, 1));
            if let Some(held) = token.as_deref() {
                uri.push_str(&format!("&nextToken={held}"));
            }
            let (status, body) = emulator.get(&uri).await;
            assert_eq!(
                status, 200,
                "case {page_size}-per-page: expected 200, got {status} with {body}"
            );
            let page = summaries(case.name, &body);
            assert!(
                page.len() <= page_size,
                "case {page_size}-per-page: a page must hold at most {page_size} changes, got {body}"
            );
            seen.extend(page);
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {page_size}-per-page: the walk must terminate, got a token after {} changes",
            seen.len()
        );
        assert_eq!(
            Value::Array(seen.clone()),
            case.changes,
            "case {page_size}-per-page: paging must report every change exactly once, in order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_change_set_is_a_property_of_the_version_not_of_the_core_network_state() {
    let emulator = Emulator::new();
    let case = change_set_cases()
        .into_iter()
        .next()
        .expect("the table is not empty");
    let core_network_id = core_network_with(&emulator, case.name, &case.document).await;

    // Read while the core network is still `CREATING`, then again once the transition the
    // clock owes has fallen due: a change set describes what the submitted policy version
    // does, so the two answers are the same one.
    let (status, creating) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
    assert_eq!(
        status, 200,
        "while CREATING: expected 200, got {status} with {creating}"
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, available) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
    assert_eq!(
        status, 200,
        "once AVAILABLE: expected 200, got {status} with {available}"
    );
    assert_eq!(
        summaries(case.name, &creating),
        summaries(case.name, &available),
        "the change set must not depend on the core network's state, got {creating} then {available}"
    );
    assert_eq!(
        Value::Array(summaries(case.name, &available)),
        case.changes,
        "expected the change set the document describes, got {available}"
    );
}

#[tokio::test]
async fn a_change_set_the_core_network_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let case = change_set_cases()
        .into_iter()
        .next()
        .expect("the table is not empty");
    let core_network_id = core_network_with(&emulator, case.name, &case.document).await;

    // A version number the core network does not hold: the one past its only version, the
    // one below it, and one far beyond.
    for version in [0_i64, 2, 97] {
        let (status, body) = emulator
            .get(&change_set_uri(&core_network_id, version))
            .await;
        assert_eq!(
            status, 404,
            "case version {version}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case version {version}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("core-network-policy"),
            "case version {version}: the refusal must name the policy, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(format!("{core_network_id}/{version}")),
            "case version {version}: the refusal must name the version asked for, got {body}"
        );
    }

    // A core network created without a policy document holds no policy version at all, so
    // even version 1 is not found there.
    let network_id = emulator.create_global_network("no policy document").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({ "GlobalNetworkId": network_id, "Description": "no policy document" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork without a policy document: expected 200, got {status} with {created}"
    );
    let bare_id = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();
    let (status, body) = emulator.get(&change_set_uri(&bare_id, 1)).await;
    assert_eq!(
        status, 404,
        "case no policy document: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("core-network-policy"),
        "case no policy document: the refusal must name the policy, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!(format!("{bare_id}/1")),
        "case no policy document: the refusal must name the version asked for, got {body}"
    );

    // An unknown core network is not found as a CORE NETWORK, whatever version it is asked
    // for: the resource the path addresses is checked first.
    let (status, body) = emulator
        .get(&change_set_uri("core-network-0123456789abcdef0", 1))
        .await;
    assert_eq!(
        status, 404,
        "case unknown core network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("core-network"),
        "case unknown core network: the refusal must name the core network, got {body}"
    );
    assert_eq!(
        body["ResourceId"],
        json!("core-network-0123456789abcdef0"),
        "case unknown core network: the refusal must name the id asked for, got {body}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let emulator = Emulator::new();
    let case = change_set_cases()
        .into_iter()
        .next()
        .expect("the table is not empty");
    let core_network_id = core_network_with(&emulator, case.name, &case.document).await;

    // Rows: a token that is not hex, hex that decodes to text this service never minted,
    // and a truncated token.
    for token in ["not-a-token", "6e6f742d6f7572732d656974686572", "6e6"] {
        let (status, body) = emulator
            .get(&format!(
                "{}?nextToken={token}",
                change_set_uri(&core_network_id, 1)
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!("NextToken"),
            "case {token:?}: the refusal must name NextToken, got {body}"
        );
    }
}

/// How many edge locations one `segments[]` / `network-function-groups[]` entry may
/// restrict itself to, and how long each of those location strings may be: the service's
/// own caps — the same two the `core-network-configuration`'s own edge-location list is
/// held to, since an entry is expanded once per location it names, both in the core
/// network's topology and in the change set of its policy version.
const MAX_ENTRY_EDGE_LOCATIONS: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_EDGE_LOCATIONS;
/// See [`MAX_ENTRY_EDGE_LOCATIONS`].
const MAX_LOCATION_CHARS: usize =
    service_com_amazonaws_networkmanager_networkmanager::state::MAX_POLICY_LOCATION_CHARS;

/// A policy document whose one segment restricts itself to `locations` locations, each
/// `width` characters long.
fn restricted_segment_document(locations: usize, width: usize) -> Value {
    let listed: Vec<String> = (0..locations)
        .map(|index| {
            let label = format!("r{index}");
            format!("{label}{}", "x".repeat(width.saturating_sub(label.len())))
        })
        .collect();
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2" }],
        },
        "segments": [{ "name": "wide", "edge-locations": listed }],
    })
}

#[tokio::test]
async fn the_edge_locations_one_entry_restricts_itself_to_are_capped() {
    let emulator = Emulator::new();

    // At the bound: accepted, and the change set holds one change per location the entry
    // names (plus the one configured edge), so the expansion stays inside the cap.
    let document = restricted_segment_document(MAX_ENTRY_EDGE_LOCATIONS, 8);
    let core_network_id = core_network_with(&emulator, "at the bound", &document).await;
    let (status, body) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
    assert_eq!(
        status, 200,
        "case at the bound: expected 200, got {status} with {body}"
    );
    let changes = summaries("at the bound", &body);
    assert_eq!(
        changes.len(),
        MAX_ENTRY_EDGE_LOCATIONS + 1,
        "case at the bound: expected one change per named location plus the configured edge, got {body}"
    );

    // Each row: a document one step past one of the two caps, and the path and error code
    // the refusal must name.
    for (case, document, path, code) in [
        (
            "one location past the entry's cap",
            restricted_segment_document(MAX_ENTRY_EDGE_LOCATIONS + 1, 8),
            "$.segments[0].edge-locations",
            "TOO_MANY_ENTRIES",
        ),
        (
            "one character past a location's cap",
            restricted_segment_document(1, MAX_LOCATION_CHARS + 1),
            "$.segments[0].edge-locations[0]",
            "INVALID_VALUE",
        ),
    ] {
        let network_id = emulator.create_global_network(case).await;
        let (status, refused) = create_core_network(
            &emulator,
            json!({ "GlobalNetworkId": network_id, "PolicyDocument": document.to_string() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "CoreNetworkPolicyException",
            "case {case:?}: expected CoreNetworkPolicyException, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["Path"],
            json!(path),
            "case {case:?}: the refusal must name the offending node, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["ErrorCode"],
            json!(code),
            "case {case:?}: expected the {code} error code, got {refused}"
        );
    }

    // A location exactly at its cap is accepted, so the refusal above is the bound and not
    // one short of it.
    let at_width = restricted_segment_document(1, MAX_LOCATION_CHARS);
    let core_network_id = core_network_with(&emulator, "a location at its cap", &at_width).await;
    let (status, body) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
    assert_eq!(
        status, 200,
        "case a location at its cap: expected 200, got {status} with {body}"
    );
    assert_eq!(
        summaries("a location at its cap", &body).len(),
        2,
        "case a location at its cap: expected the edge and the segment it restricts itself to, got {body}"
    );
}

/// `PutCoreNetworkPolicy` of `document`, answering the `PolicyVersionId` it minted.
async fn put_policy_version(
    emulator: &Emulator,
    case: &str,
    core_network_id: &str,
    document: &Value,
) -> i64 {
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

#[tokio::test]
async fn a_submitted_version_reports_its_differences_from_the_policy_in_force() {
    let emulator = Emulator::new();
    let case = "a submitted version against the live one";

    // The version in force: two edges with their own ASNs, one segment everywhere and one
    // restricted to a single edge location.
    let live = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [
                { "location": "us-west-2", "asn": 64512 },
                { "location": "eu-west-1", "asn": 64513 },
            ],
        },
        "segments": [{ "name": "prod" }, { "name": "legacy", "edge-locations": ["eu-west-1"] }],
    });
    // The submitted version: one edge's ASN changed, `legacy` dropped, `dev` added in one
    // edge location. Everything else — the `us-west-2` edge and `prod` in both locations —
    // is identical, so it is no change at all.
    let submitted = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [
                { "location": "us-west-2", "asn": 64512 },
                { "location": "eu-west-1", "asn": 64520 },
            ],
        },
        "segments": [{ "name": "prod" }, { "name": "dev", "edge-locations": ["us-west-2"] }],
    });

    let core_network_id = core_network_with(&emulator, case, &live).await;
    let version_id = put_policy_version(&emulator, case, &core_network_id, &submitted).await;
    assert_eq!(
        version_id, 2,
        "case {case:?}: a put appends the next version, got {version_id}"
    );

    let (status, body) = emulator
        .get(&change_set_uri(&core_network_id, version_id))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {body}"
    );
    assert_eq!(
        Value::Array(summaries(case, &body)),
        json!([
            {
                "Type": "CORE_NETWORK_EDGE",
                "Action": "MODIFY",
                "Identifier": "eu-west-1",
                "IdentifierPath": "CORE_NETWORK_EDGE/eu-west-1",
                "PreviousValues": { "EdgeLocations": ["eu-west-1"], "Asn": 64513 },
                "NewValues": { "EdgeLocations": ["eu-west-1"], "Asn": 64520 },
            },
            {
                "Type": "CORE_NETWORK_SEGMENT",
                "Action": "REMOVE",
                "Identifier": "legacy",
                "IdentifierPath": "CORE_NETWORK_SEGMENT/eu-west-1/legacy",
                "PreviousValues": { "SegmentName": "legacy", "EdgeLocations": ["eu-west-1"] },
            },
            {
                "Type": "CORE_NETWORK_SEGMENT",
                "Action": "ADD",
                "Identifier": "dev",
                "IdentifierPath": "CORE_NETWORK_SEGMENT/us-west-2/dev",
                "NewValues": { "SegmentName": "dev", "EdgeLocations": ["us-west-2"] },
            },
        ]),
        "case {case:?}: expected only the members the two policies disagree on, got {body}"
    );

    // A put leaves the live version alone, so the change set of the version in force is
    // still every member it added against the nothing before it — one change set per
    // version, each against the policy that version was submitted against.
    let (status, first) = emulator.get(&change_set_uri(&core_network_id, 1)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the live version's change set expected 200, got {status} with {first}"
    );
    let first_paths: Vec<Value> = summaries(case, &first)
        .iter()
        .map(|change| change["IdentifierPath"].clone())
        .collect();
    assert_eq!(
        Value::Array(first_paths),
        json!([
            "CORE_NETWORK_EDGE/eu-west-1",
            "CORE_NETWORK_EDGE/us-west-2",
            "CORE_NETWORK_SEGMENT/eu-west-1/legacy",
            "CORE_NETWORK_SEGMENT/eu-west-1/prod",
            "CORE_NETWORK_SEGMENT/us-west-2/prod",
        ]),
        "case {case:?}: the first version adds every member it describes, got {first}"
    );
    assert!(
        summaries(case, &first)
            .iter()
            .all(|change| change["Action"] == json!("ADD")),
        "case {case:?}: nothing precedes the first policy, so every change adds, got {first}"
    );
}

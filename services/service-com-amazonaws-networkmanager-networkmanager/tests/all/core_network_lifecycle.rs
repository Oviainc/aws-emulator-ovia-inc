// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the core-network lifecycle, driven over HTTP through the real
//! router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: `CreateCoreNetwork` mints a core network inside the named global
//! network and stores it, a core network is `CREATING` until the transition the clock owes
//! falls due and `AVAILABLE` after it, and everything it then reports — its segments, its
//! network function groups and its edges — is DERIVED from the policy document it holds,
//! by the rules of the published core network policy schema. The tables below span that
//! class: documents differing in how many edge locations they configure, whether an edge
//! names its own ASN or takes one from `asn-ranges`, whether it carries inside CIDR
//! blocks, whether a segment restricts itself to some edge locations, how many segments
//! and network function groups there are, and no policy document at all.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::{escape, tags_uri};

/// One row of the topology table: a policy document (or none) and the topology the rules
/// derive from it.
struct PolicyCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The policy document the create carries, or `None` for a create without one.
    document: Option<Value>,
    /// The `Edges` the document describes, in the order the document lists them.
    edges: Value,
    /// The `Segments` the document describes.
    segments: Value,
    /// The `NetworkFunctionGroups` the document describes.
    network_function_groups: Value,
}

/// The table: one row per form a policy document takes.
///
/// The document's own key names (`core-network-configuration`, `asn-ranges`,
/// `edge-locations`, `segments`, `network-function-groups`, `location`, `asn`,
/// `inside-cidr-blocks`) are the published Cloud WAN core network policy schema's, which
/// is where the derivation rules come from too.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            name: "one edge naming its own ASN and inside CIDR blocks",
            document: Some(json!({
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
            })),
            edges: json!([{
                "EdgeLocation": "us-west-2",
                "Asn": 64512,
                "InsideCidrBlocks": ["10.0.0.0/16"],
            }]),
            // A segment that restricts itself to no edge location exists in every edge
            // location the core network has.
            segments: json!([{ "Name": "shared", "EdgeLocations": ["us-west-2"] }]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "two edges taking ASNs from the range, one restricted segment",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64513"],
                    "edge-locations": [{ "location": "us-east-1" }, { "location": "eu-west-1" }],
                },
                "segments": [
                    { "name": "prod", "edge-locations": ["us-east-1"] },
                    { "name": "dev" },
                ],
            })),
            edges: json!([
                { "EdgeLocation": "us-east-1", "Asn": 64512 },
                { "EdgeLocation": "eu-west-1", "Asn": 64513 },
            ]),
            segments: json!([
                { "Name": "prod", "EdgeLocations": ["us-east-1"] },
                { "Name": "dev", "EdgeLocations": ["us-east-1", "eu-west-1"] },
            ]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "a range assignment skips the ASN another edge claims",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64514"],
                    "edge-locations": [
                        { "location": "ap-south-1", "asn": 64513 },
                        { "location": "ap-southeast-2" },
                        { "location": "ca-central-1" },
                    ],
                },
                "segments": [],
            })),
            edges: json!([
                { "EdgeLocation": "ap-south-1", "Asn": 64513 },
                { "EdgeLocation": "ap-southeast-2", "Asn": 64512 },
                { "EdgeLocation": "ca-central-1", "Asn": 64514 },
            ]),
            segments: json!([]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "a single-number ASN range, and a network function group",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64600"],
                    "edge-locations": [{ "location": "eu-central-1" }],
                },
                "segments": [{ "name": "shared" }],
                "network-function-groups": [{ "name": "inspection" }],
            })),
            edges: json!([{ "EdgeLocation": "eu-central-1", "Asn": 64600 }]),
            segments: json!([{ "Name": "shared", "EdgeLocations": ["eu-central-1"] }]),
            network_function_groups: json!([{
                "Name": "inspection",
                "EdgeLocations": ["eu-central-1"],
            }]),
        },
        PolicyCase {
            name: "more edges than the ranges can serve",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64700-64700"],
                    "edge-locations": [{ "location": "sa-east-1" }, { "location": "me-south-1" }],
                },
                "segments": [{ "name": "shared", "edge-locations": [] }],
            })),
            // The second edge location gets no ASN: the ranges name exactly one, and an
            // ASN nothing allocated is absent rather than invented.
            edges: json!([
                { "EdgeLocation": "sa-east-1", "Asn": 64700 },
                { "EdgeLocation": "me-south-1" },
            ]),
            segments: json!([{ "Name": "shared", "EdgeLocations": [] }]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "an ASN range outside the 32-bit ASN domain assigns nothing",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    // AS 0 is reserved and an AS number is 32-bit, so neither range names
                    // an assignable ASN.
                    "asn-ranges": ["0-0", "4294967296-4294967300"],
                    "edge-locations": [{ "location": "us-west-1" }],
                },
                "segments": [{ "name": "shared" }],
            })),
            edges: json!([{ "EdgeLocation": "us-west-1" }]),
            segments: json!([{ "Name": "shared", "EdgeLocations": ["us-west-1"] }]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "an edge claiming the largest number there is keeps it, and the walk ends",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": [9_223_372_036_854_775_807_i64],
                    "edge-locations": [
                        { "location": "us-west-2", "asn": 9_223_372_036_854_775_807_i64 },
                        { "location": "us-east-2" },
                    ],
                },
                "segments": [],
            })),
            // The range lies outside the ASN domain, so it assigns nothing and the second
            // edge location gets no ASN: an ASN claimed by another edge is never handed
            // out twice, and the walk over an exhausted range terminates.
            edges: json!([
                { "EdgeLocation": "us-west-2", "Asn": 9_223_372_036_854_775_807_i64 },
                { "EdgeLocation": "us-east-2" },
            ]),
            segments: json!([]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "a location and a segment name exactly at their longest",
            document: Some(json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512"],
                    // 63 characters is the longest an `ExternalRegionCode` may be, and 256
                    // the longest a segment `ConstrainedString` name may be.
                    "edge-locations": [{ "location": "e".repeat(63) }],
                },
                "segments": [{ "name": "s".repeat(256) }],
            })),
            edges: json!([{ "EdgeLocation": "e".repeat(63), "Asn": 64512 }]),
            segments: json!([{ "Name": "s".repeat(256), "EdgeLocations": ["e".repeat(63)] }]),
            network_function_groups: json!([]),
        },
        PolicyCase {
            name: "no policy document at all",
            document: None,
            edges: json!([]),
            segments: json!([]),
            network_function_groups: json!([]),
        },
    ]
}

/// Create a core network in its own global network, returning the create response.
async fn create_core_network(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/core-networks", body).await
}

/// How many entries of one list a policy document may carry before it is refused: the
/// caps `state::MAX_POLICY_EDGE_LOCATIONS` / `MAX_POLICY_SEGMENTS` /
/// `MAX_POLICY_NETWORK_FUNCTION_GROUPS` fix, which bound the topology a stored document
/// expands into (its edge-location count times its segment count).
const MAX_LIST_ENTRIES: usize = 100;

/// A policy document carrying `entries` entries in the list `key`, each a named object,
/// and one edge location so a segment has somewhere to be.
fn document_with(key: &str, entries: usize) -> String {
    let listed: Vec<Value> = (0..entries)
        .map(|index| json!({ "name": format!("entry-{index}"), "location": format!("r-{index}") }))
        .collect();
    let mut document = json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2" }],
        },
    });
    if key == "core-network-configuration.edge-locations" {
        document["core-network-configuration"]["edge-locations"] = json!(listed);
    } else {
        document[key] = json!(listed);
    }
    document.to_string()
}

#[tokio::test]
async fn a_policy_document_list_is_capped_at_its_bound() {
    let emulator = Emulator::new();
    // Each row: the list, the path a refusal names, and the entry counts at the bound and
    // one past it.
    for (key, path) in [
        (
            "core-network-configuration.edge-locations",
            "$.core-network-configuration.edge-locations",
        ),
        ("segments", "$.segments"),
        ("network-function-groups", "$.network-function-groups"),
    ] {
        let network_id = emulator.create_global_network(key).await;
        let (status, accepted) = create_core_network(
            &emulator,
            json!({
                "GlobalNetworkId": network_id,
                "PolicyDocument": document_with(key, MAX_LIST_ENTRIES),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {key:?}: {MAX_LIST_ENTRIES} entries is at the bound and accepted, got {status} with {accepted}"
        );

        let over_id = emulator.create_global_network("over the bound").await;
        let (status, refused) = create_core_network(
            &emulator,
            json!({
                "GlobalNetworkId": over_id,
                "PolicyDocument": document_with(key, MAX_LIST_ENTRIES + 1),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {key:?}: one entry past the bound is refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "CoreNetworkPolicyException",
            "case {key:?}: expected CoreNetworkPolicyException, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["Path"],
            json!(path),
            "case {key:?}: the refusal must name the over-size list, got {refused}"
        );
        assert_eq!(
            refused["Errors"][0]["ErrorCode"],
            json!("TOO_MANY_ENTRIES"),
            "case {key:?}: expected the over-size error code, got {refused}"
        );
    }
}

#[tokio::test]
async fn core_network_topology_is_derived_from_the_stored_policy_document() {
    let emulator = Emulator::new();
    let cases = policy_cases();
    let mut created: Vec<(&PolicyCase, String)> = Vec::new();

    for case in &cases {
        let network_id = emulator.create_global_network(case.name).await;
        let mut request = json!({ "GlobalNetworkId": network_id, "Description": case.name });
        if let Some(document) = case.document.as_ref() {
            request["PolicyDocument"] = json!(document.to_string());
        }
        let (status, response) = create_core_network(&emulator, request).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCoreNetwork expected 200, got {status} with {response}",
            case.name
        );
        let core_network = &response["CoreNetwork"];
        let id = core_network["CoreNetworkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no CoreNetworkId in {response}", case.name))
            .to_string();
        assert!(
            is_minted_id(&id, "core-network"),
            "case {:?}: expected a core-network-<17 hex> id, got {id:?}",
            case.name
        );
        assert_eq!(
            core_network["CoreNetworkArn"],
            json!(expected_arn("core-network", &id)),
            "case {:?}: the ARN must name the minted id, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["GlobalNetworkId"],
            json!(network_id),
            "case {:?}: the core network must back-reference its parent, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Description"],
            json!(case.name),
            "case {:?}: Description must be stored, got {core_network}",
            case.name
        );
        assert!(
            core_network["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {:?}: CreatedAt must be a timestamp, got {core_network}",
            case.name
        );
        // The transition the clock owes has not fallen due yet, so the policy is not in
        // force and the core network reports no topology.
        assert_eq!(
            core_network["State"],
            json!("CREATING"),
            "case {:?}: a just-created core network is CREATING, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Edges"],
            json!([]),
            "case {:?}: a CREATING core network has no edges yet, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Segments"],
            json!([]),
            "case {:?}: a CREATING core network has no segments yet, got {core_network}",
            case.name
        );
        created.push((case, id));
    }

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
            core_network["CoreNetworkId"],
            json!(id),
            "case {:?}: GetCoreNetwork must answer the stored record, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["State"],
            json!("AVAILABLE"),
            "case {:?}: past the settle span the core network is AVAILABLE, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Edges"], case.edges,
            "case {:?}: expected the edges the policy configures, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["Segments"], case.segments,
            "case {:?}: expected the segments the policy names, got {core_network}",
            case.name
        );
        assert_eq!(
            core_network["NetworkFunctionGroups"], case.network_function_groups,
            "case {:?}: expected the network function groups the policy names, got {core_network}",
            case.name
        );
    }

    // A read never changes what the next caller sees: the same GET answers the same
    // record, state and topology.
    for (case, id) in &created {
        let (_, first) = emulator.get(&format!("/core-networks/{id}")).await;
        let (_, second) = emulator.get(&format!("/core-networks/{id}")).await;
        assert_eq!(
            first, second,
            "case {:?}: GetCoreNetwork must be a pure read, got {first} then {second}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_unparseable_or_malformed_policy_document_is_refused() {
    let emulator = Emulator::new();
    // Each row: the document as the request carries it, and the JSON paths the refusal
    // names. A structural rule of the published schema is broken in every one.
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "not JSON at all",
            "{ this is not json".to_string(),
            vec!["$"],
        ),
        (
            "a JSON array, not an object",
            json!([{ "version": "2021.12" }]).to_string(),
            vec!["$"],
        ),
        (
            "no version and no configuration",
            json!({}).to_string(),
            vec!["$.version", "$.core-network-configuration"],
        ),
        (
            "a configuration that is not an object",
            json!({ "version": "2021.12", "core-network-configuration": "none" }).to_string(),
            vec!["$.core-network-configuration"],
        ),
        (
            "segments that are not an array",
            json!({
                "version": "2021.12",
                "core-network-configuration": { "asn-ranges": ["64512-64555"] },
                "segments": { "name": "shared" },
            })
            .to_string(),
            vec!["$.segments"],
        ),
        (
            "a segment without a name",
            json!({
                "version": "2021.12",
                "core-network-configuration": { "asn-ranges": ["64512-64555"] },
                "segments": [{ "name": "shared" }, { "edge-locations": ["us-west-2"] }],
            })
            .to_string(),
            vec!["$.segments[1].name"],
        ),
        (
            "an edge location entry naming no location",
            json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64555"],
                    "edge-locations": [{ "asn": 64512 }],
                },
            })
            .to_string(),
            vec!["$.core-network-configuration.edge-locations[0].location"],
        ),
        (
            "a location longer than an ExternalRegionCode may be",
            json!({
                "version": "2021.12",
                "core-network-configuration": {
                    "asn-ranges": ["64512-64555"],
                    // `CoreNetworkEdge$EdgeLocation` is an `ExternalRegionCode`, len 1..63
                    // (`smithy_query shape=CoreNetworkEdge`), so 64 characters is one too
                    // many and an empty one is one too few.
                    "edge-locations": [{ "location": "e".repeat(64) }, { "location": "" }],
                },
            })
            .to_string(),
            vec![
                "$.core-network-configuration.edge-locations[0].location",
                "$.core-network-configuration.edge-locations[1].location",
            ],
        ),
        (
            "a segment name longer than a ConstrainedString may be",
            json!({
                "version": "2021.12",
                "core-network-configuration": { "asn-ranges": ["64512-64555"] },
                // `CoreNetworkSegment$Name` is a `ConstrainedString`, len 0..256.
                "segments": [{ "name": "s".repeat(257) }],
            })
            .to_string(),
            vec!["$.segments[0].name"],
        ),
    ];

    for (case, document, paths) in cases {
        let network_id = emulator.create_global_network(case).await;
        let (status, body) = create_core_network(
            &emulator,
            json!({ "GlobalNetworkId": network_id, "PolicyDocument": document }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a malformed policy document is refused with 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "CoreNetworkPolicyException",
            "case {case:?}: expected CoreNetworkPolicyException, got {body}"
        );
        let reported: Vec<String> = body["Errors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Errors list in {body}"))
            .iter()
            .map(|error| error["Path"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            paths.iter().map(|path| (*path).to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the offending paths, got {body}"
        );

        // Nothing was stored: the global network still holds no core network, so a
        // well-formed create is still accepted.
        let (status, body) = create_core_network(
            &emulator,
            json!({ "GlobalNetworkId": network_id, "PolicyDocument": json!({
                "version": "2021.12",
                "core-network-configuration": { "asn-ranges": ["64512-64555"] },
            }).to_string() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refusal must store nothing, so a good create follows it, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_create_is_idempotent_in_its_client_token() {
    let emulator = Emulator::new();
    let network_id = emulator.create_global_network("idempotency").await;
    let request = json!({
        "GlobalNetworkId": network_id,
        "Description": "repeated",
        "ClientToken": "token-one",
        "Tags": [{ "Key": "env", "Value": "test" }],
    });

    let (status, first) = create_core_network(&emulator, request.clone()).await;
    assert_eq!(
        status, 200,
        "the first create expected 200, got {status} with {first}"
    );
    let id = first["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {first}"))
        .to_string();

    // A retry of the SAME request answers the record the first call minted, whatever
    // order the tags arrive in.
    let mut retry = request.clone();
    retry["Tags"] = json!([{ "Key": "env", "Value": "test" }]);
    let (status, again) = create_core_network(&emulator, retry).await;
    assert_eq!(
        status, 200,
        "a retry with the same client token expected 200, got {status} with {again}"
    );
    assert_eq!(
        again["CoreNetwork"]["CoreNetworkId"],
        json!(id),
        "a retry must answer the first record, got {again}"
    );
    assert_eq!(
        again["CoreNetwork"]["CreatedAt"], first["CoreNetwork"]["CreatedAt"],
        "a retry must answer the stored record, not a fresh one, got {again}"
    );

    // The same token asking for something else is a conflict, and stores nothing.
    let mut different = request.clone();
    different["Description"] = json!("something else");
    let (status, conflict) = create_core_network(&emulator, different).await;
    assert_eq!(
        status, 409,
        "the same token with a different request is a conflict, got {status} with {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );

    // A create carrying no token at all is never deduplicated against one that does, but
    // this global network already holds its one core network, so it is the quota that
    // refuses it.
    let (status, quota) = create_core_network(
        &emulator,
        json!({ "GlobalNetworkId": network_id, "Description": "second" }),
    )
    .await;
    assert_eq!(
        status, 402,
        "a second core network in one global network is refused, got {status} with {quota}"
    );
    assert_eq!(
        error_code(&quota),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {quota}"
    );
    assert_eq!(
        quota["LimitCode"],
        json!("core-networks-per-global-network"),
        "the refusal must name the quota it enforces, got {quota}"
    );

    // The store still holds exactly the first record, with its tags.
    let (status, read) = emulator.get(&format!("/core-networks/{id}")).await;
    assert_eq!(
        status, 200,
        "GetCoreNetwork expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["CoreNetwork"]["Description"],
        json!("repeated"),
        "the refused requests must not have changed the record, got {read}"
    );
    assert_eq!(
        read["CoreNetwork"]["Tags"],
        json!([{ "Key": "env", "Value": "test" }]),
        "the tags of the create must be stored, got {read}"
    );

    // Tags are held on the record itself, so the shared tag reader answers them too.
    let arn = expected_arn("core-network", &id);
    let (status, tags) = emulator.get(&tags_uri(&arn)).await;
    assert_eq!(
        status, 200,
        "ListTagsForResource expected 200, got {status} with {tags}"
    );
    assert_eq!(
        tags["TagList"],
        json!([{ "Key": "env", "Value": "test" }]),
        "ListTagsForResource must answer the core network's stored tags, got {tags}"
    );
}

#[tokio::test]
async fn a_core_network_needs_a_parent_and_blocks_its_deletion() {
    let emulator = Emulator::new();

    // An unknown global network: this operation models no ResourceNotFoundException, so
    // the refusal is the ValidationException naming the member that carries it.
    let (status, body) = create_core_network(
        &emulator,
        json!({ "GlobalNetworkId": "global-network-00000000000000bad" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown global network is refused with 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Fields"][0]["Name"],
        json!("GlobalNetworkId"),
        "the refusal must name the offending member, got {body}"
    );

    // An unknown core network is a 404 from the reader.
    let (status, body) = emulator
        .get("/core-networks/core-network-00000000000000bad")
        .await;
    assert_eq!(
        status, 404,
        "GetCoreNetwork on an unknown id expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["ResourceType"],
        json!("core-network"),
        "the refusal must name the resource type, got {body}"
    );

    // A global network that owns a core network cannot be deleted.
    let network_id = emulator.create_global_network("owns a core network").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({ "GlobalNetworkId": network_id, "Description": "blocking child" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let (status, blocked) = emulator
        .delete(&format!("/global-networks/{network_id}"))
        .await;
    assert_eq!(
        status, 409,
        "deleting a global network that still owns a core network is refused, got {status} with {blocked}"
    );
    assert_eq!(
        error_code(&blocked),
        "ConflictException",
        "expected ConflictException, got {blocked}"
    );

    // And the core network is counted as the network resource it is.
    let (status, counts) = emulator
        .get(&format!(
            "/global-networks/{network_id}/network-resource-count?resourceType={}",
            escape("core-network")
        ))
        .await;
    assert_eq!(
        status, 200,
        "GetNetworkResourceCounts expected 200, got {status} with {counts}"
    );
    assert_eq!(
        counts["NetworkResourceCounts"][0]["Count"],
        json!(1),
        "the stored core network must be counted, got {counts}"
    );
}

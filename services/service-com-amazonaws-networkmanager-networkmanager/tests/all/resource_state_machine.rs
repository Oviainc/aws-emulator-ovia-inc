// SPDX-License-Identifier: CC0-1.0

//! The lifecycle STATE MACHINE of this service's resources, driven end-to-end over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! Three rules are pinned here, each over the whole SET of resources it holds for rather
//! than over one operation:
//!
//! 1. An `Update*` call reconfigures the addressed resource and that reconfiguration is not
//!    instant: every resource whose state enum carries `UPDATING` (`smithy_query
//!    shape=GlobalNetworkState,SiteState,LinkState,DeviceState,ConnectionState,CoreNetworkState`)
//!    is answered in `UPDATING`, and reads `AVAILABLE` again once the transition the clock
//!    owes falls due, with the members the update wrote still in place. The table below has
//!    one row per such resource, and the rows differ in which members the update carries
//!    (one, several, a nested structure, a numeric one) and in the reader the resource is
//!    read back through.
//! 2. A resource AWS answers READY stays ready: `CreateCoreNetwork` reports its core
//!    network `AVAILABLE`, with the topology its policy document configures already in
//!    force, and nothing of the clock moves it afterwards.
//! 3. A `Delete*` call reports the record it removed in `DELETING` — as it was last
//!    configured, not as it was created — and the resource is gone from its reader
//!    afterwards, which for a single-resource reader is the modelled
//!    `ResourceNotFoundException` (404); the identity that refusal reports is tabled by
//!    `crate::resource_not_found_message`.
//!
//! Nothing reaches into `crate::state`: every resource is created, mutated and read back
//! through operations.

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, fixture, id_of};
use crate::core_network_lifecycle::create_core_network;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator, PAST_SETTLE};
use crate::link_lifecycle::create_link_ok;

/// The transitional state an outstanding reconfiguration puts a resource in.
pub(crate) const UPDATING: &str = "UPDATING";
/// The settled state a resource reads once nothing is outstanding on it.
pub(crate) const AVAILABLE: &str = "AVAILABLE";
/// The state a delete reports the record it removed in.
const DELETING: &str = "DELETING";

/// The policy document the core network rows hold. Its key names
/// (`core-network-configuration`, `asn-ranges`, `edge-locations`, `segments`, `location`,
/// `asn`) are the published Cloud WAN core network policy schema's.
fn policy_document() -> Value {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [{ "location": "us-west-2", "asn": 64512 }],
        },
        "segments": [{ "name": "shared" }],
    })
}

/// The `Edges` the document above configures, once it is in force.
fn policy_edges() -> Value {
    json!([{ "EdgeLocation": "us-west-2", "Asn": 64512 }])
}

/// The `Segments` the document above names, once it is in force.
fn policy_segments() -> Value {
    json!([{ "Name": "shared", "EdgeLocations": ["us-west-2"], "SharedSegments": [] }])
}

/// How one row's resource is read back: either a single-resource reader answering the
/// record under one member, or a listing the row's record has to be picked out of by id.
///
/// Shared with the suites that table a rule over resources of several kinds
/// (`crate::update_no_op`), so one reader description serves them all.
pub(crate) enum Reader {
    /// `GET <uri>` answering `{ <member>: { .. } }`.
    One { uri: String, member: &'static str },
    /// `GET <uri>` answering `{ <member>: [ .. ] }`, where the row's record is the entry
    /// whose `id_member` is the row's id.
    Listed {
        uri: String,
        member: &'static str,
        id_member: &'static str,
    },
}

/// One row of the update table: a resource, the update to make on it, and the members that
/// update has to have written.
struct UpdateCase {
    case: &'static str,
    /// The `PATCH` the row makes, as URI and body.
    uri: String,
    body: Value,
    /// The member of the update's own answer the resource arrives under.
    answer_member: &'static str,
    /// The id the row's resource was minted with.
    id: String,
    /// How the row reads its resource back.
    reader: Reader,
    /// The members the update wrote, asserted on the update's answer AND on the record
    /// read back once the transition the clock owes has fallen due.
    written: Vec<(&'static str, Value)>,
}

/// The `State` of a resource shape, named so a failure says which row it belonged to.
pub(crate) fn state_of(case: &str, resource: &Value) -> String {
    resource["State"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no State in {resource}"))
        .to_string()
}

/// Read the row's resource back through its reader.
pub(crate) async fn read_back(emulator: &Emulator, case: &str, id: &str, reader: &Reader) -> Value {
    match reader {
        Reader::One { uri, member } => {
            let (status, body) = emulator.get(uri).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {uri} expected 200, got {status} with {body}"
            );
            body[*member].clone()
        }
        Reader::Listed {
            uri,
            member,
            id_member,
        } => {
            let (status, body) = emulator.get(uri).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reading {uri} expected 200, got {status} with {body}"
            );
            body[*member]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no {member} array in {body}"))
                .iter()
                .find(|entry| entry[*id_member].as_str() == Some(id))
                .unwrap_or_else(|| panic!("case {case:?}: {id} is not listed in {body}"))
                .clone()
        }
    }
}

/// Seed one resource of every updatable kind and the update each row makes on it.
async fn update_cases(emulator: &Emulator) -> Vec<UpdateCase> {
    // A global network with a site, a link and a device of its own, plus the connection
    // fixture's own global network, and a core network in a third one (a global network
    // holds at most one core network, so it gets its own parent).
    let network = emulator.create_global_network("state-machine").await;
    let site = emulator.create_site(&network, "created-site").await;
    let link = create_link_ok(
        emulator,
        &network,
        &site,
        &json!({
            "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 },
            "Description": "created-link",
        }),
    )
    .await;
    let device = create_device_ok(
        emulator,
        &network,
        json!({ "SiteId": site, "Description": "created-device" }),
    )
    .await;

    let seed = fixture(emulator, "state-machine-connection").await;
    let connection = id_of(
        &create_connection_ok(
            emulator,
            &seed.network,
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
                "Description": "created-connection",
            }),
        )
        .await,
    );

    let core_parent = emulator.create_global_network("state-machine-core").await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": core_parent,
            "Description": "created-core-network",
            "PolicyDocument": policy_document().to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkId in {created}"))
        .to_string();

    vec![
        UpdateCase {
            case: "a global network, one member",
            uri: format!("/global-networks/{network}"),
            body: json!({ "Description": "updated-global-network" }),
            answer_member: "GlobalNetwork",
            id: network.clone(),
            reader: Reader::Listed {
                uri: "/global-networks".to_string(),
                member: "GlobalNetworks",
                id_member: "GlobalNetworkId",
            },
            written: vec![("Description", json!("updated-global-network"))],
        },
        UpdateCase {
            case: "a site, a member and a nested structure",
            uri: format!("/global-networks/{network}/sites/{site}"),
            body: json!({
                "Description": "updated-site",
                "Location": { "Address": "1 Main St", "Latitude": "47.6", "Longitude": "-122.3" },
            }),
            answer_member: "Site",
            id: site.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/sites"),
                member: "Sites",
                id_member: "SiteId",
            },
            written: vec![
                ("Description", json!("updated-site")),
                (
                    "Location",
                    json!({ "Address": "1 Main St", "Latitude": "47.6", "Longitude": "-122.3" }),
                ),
            ],
        },
        UpdateCase {
            case: "a link, three members including a numeric structure",
            uri: format!("/global-networks/{network}/links/{link}"),
            body: json!({
                "Description": "updated-link",
                "Provider": "updated-provider",
                "Bandwidth": { "UploadSpeed": 100, "DownloadSpeed": 200 },
            }),
            answer_member: "Link",
            id: link.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/links"),
                member: "Links",
                id_member: "LinkId",
            },
            written: vec![
                ("Description", json!("updated-link")),
                ("Provider", json!("updated-provider")),
                (
                    "Bandwidth",
                    json!({ "UploadSpeed": 100, "DownloadSpeed": 200 }),
                ),
            ],
        },
        UpdateCase {
            case: "a device, two members",
            uri: format!("/global-networks/{network}/devices/{device}"),
            body: json!({ "Description": "updated-device", "Model": "updated-model" }),
            answer_member: "Device",
            id: device.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/devices"),
                member: "Devices",
                id_member: "DeviceId",
            },
            written: vec![
                ("Description", json!("updated-device")),
                ("Model", json!("updated-model")),
            ],
        },
        UpdateCase {
            case: "a connection, a member and a reference",
            uri: format!(
                "/global-networks/{}/connections/{connection}",
                seed.network
            ),
            body: json!({ "Description": "updated-connection", "LinkId": seed.link }),
            answer_member: "Connection",
            id: connection.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{}/connections", seed.network),
                member: "Connections",
                id_member: "ConnectionId",
            },
            written: vec![
                ("Description", json!("updated-connection")),
                ("LinkId", json!(seed.link.clone())),
            ],
        },
        UpdateCase {
            case: "a core network, the one member it takes",
            uri: format!("/core-networks/{core_network}"),
            body: json!({ "Description": "updated-core-network" }),
            answer_member: "CoreNetwork",
            id: core_network.clone(),
            reader: Reader::One {
                uri: format!("/core-networks/{core_network}"),
                member: "CoreNetwork",
            },
            // The policy in force stays in force while the update is outstanding, so the
            // derived topology is part of what the row reads back.
            written: vec![
                ("Description", json!("updated-core-network")),
                ("Edges", policy_edges()),
                ("Segments", policy_segments()),
            ],
        },
    ]
}

#[tokio::test]
async fn an_update_answers_the_resource_updating_and_it_settles_back_to_available() {
    let emulator = Emulator::new();
    let rows = update_cases(&emulator).await;

    // Each row's own create may still owe its PENDING -> AVAILABLE transition, so the rows
    // wait it out first: the UPDATING below is then the update's own.
    tokio::time::sleep(PAST_SETTLE).await;

    for row in &rows {
        let before = read_back(&emulator, row.case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(row.case, &before),
            AVAILABLE,
            "case {:?}: expected a settled resource before the update, got {before}",
            row.case
        );

        let (status, answer) = emulator.patch(&row.uri, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {answer}",
            row.case
        );
        let resource = &answer[row.answer_member];
        assert_eq!(
            state_of(row.case, resource),
            UPDATING,
            "case {:?}: expected the update to answer {UPDATING}, got {resource}",
            row.case
        );
        for (member, expected) in &row.written {
            assert_eq!(
                resource[*member], *expected,
                "case {:?}: expected {member} {expected}, got {resource}",
                row.case
            );
        }

        // The same state is what the next caller sees: the update WROTE the transition
        // rather than reporting one only in its own answer.
        let reread = read_back(&emulator, row.case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(row.case, &reread),
            UPDATING,
            "case {:?}: the stored record must be {UPDATING} too, got {reread}",
            row.case
        );
    }

    // No call is made while the clock runs: the transition each update owes falls due by
    // itself.
    tokio::time::sleep(PAST_SETTLE).await;

    for row in &rows {
        let settled = read_back(&emulator, row.case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(row.case, &settled),
            AVAILABLE,
            "case {:?}: expected {AVAILABLE} once the transition fell due, got {settled}",
            row.case
        );
        for (member, expected) in &row.written {
            assert_eq!(
                settled[*member], *expected,
                "case {:?}: expected the stored {member} to be {expected}, got {settled}",
                row.case
            );
        }
    }
}

/// One row of the create-state table: the policy document the create carries (or none),
/// and the topology it configures.
struct CreateCase {
    case: &'static str,
    document: Option<Value>,
    edges: Value,
    segments: Value,
}

#[tokio::test]
async fn a_created_core_network_is_available_at_once_and_stays_available() {
    let emulator = Emulator::new();
    let rows = vec![
        CreateCase {
            case: "a create carrying a policy document",
            document: Some(policy_document()),
            edges: policy_edges(),
            segments: policy_segments(),
        },
        CreateCase {
            case: "a create carrying no policy document",
            document: None,
            edges: json!([]),
            segments: json!([]),
        },
    ];

    let mut created = Vec::new();
    for row in &rows {
        let parent = emulator.create_global_network(row.case).await;
        let mut request = json!({ "GlobalNetworkId": parent });
        if let Some(document) = &row.document {
            request["PolicyDocument"] = json!(document.to_string());
        }
        let (status, answer) = create_core_network(&emulator, request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {answer}",
            row.case
        );
        let core_network = &answer["CoreNetwork"];
        assert_eq!(
            state_of(row.case, core_network),
            AVAILABLE,
            "case {:?}: a created core network is {AVAILABLE}, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Edges"], row.edges,
            "case {:?}: the policy the create carried is in force at once, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Segments"], row.segments,
            "case {:?}: the policy the create carried is in force at once, got {core_network}",
            row.case
        );
        created.push((
            row,
            core_network["CoreNetworkId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no CoreNetworkId in {answer}", row.case))
                .to_string(),
        ));
    }

    // A resource answered ready stays ready: no span of the clock moves it anywhere.
    tokio::time::sleep(PAST_SETTLE).await;
    for (row, id) in &created {
        let (status, body) = emulator.get(&format!("/core-networks/{id}")).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetCoreNetwork expected 200, got {status} with {body}",
            row.case
        );
        let core_network = &body["CoreNetwork"];
        assert_eq!(
            state_of(row.case, core_network),
            AVAILABLE,
            "case {:?}: the core network must still be {AVAILABLE}, got {core_network}",
            row.case
        );
        assert_eq!(
            core_network["Edges"], row.edges,
            "case {:?}: the topology must be the policy's, got {core_network}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_delete_reports_the_record_it_removed_as_deleting_and_the_resource_is_then_gone() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("teardown").await;
    let site = emulator.create_site(&network, "created-site").await;

    // The site is RECONFIGURED before it is deleted, so the delete's answer shows whether
    // it reports the record as last configured or as created.
    let (status, updated) = emulator
        .patch(
            &format!("/global-networks/{network}/sites/{site}"),
            json!({ "Description": "updated-site" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateSite: expected 200, got {status} with {updated}"
    );

    // A core network of its own global network, deleted through the single-resource reader
    // that then has to refuse.
    let core_parent = emulator.create_global_network("teardown-core").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({
            "GlobalNetworkId": core_parent,
            "PolicyDocument": policy_document().to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkId in {created}"))
        .to_string();

    // DeleteSite: the removed record, in DELETING, carrying what the update wrote.
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network}/sites/{site}"))
        .await;
    assert_eq!(
        status, 200,
        "case DeleteSite: expected 200, got {status} with {deleted}"
    );
    let removed = &deleted["Site"];
    assert_eq!(
        state_of("DeleteSite", removed),
        DELETING,
        "case DeleteSite: expected {DELETING}, got {removed}"
    );
    assert_eq!(
        removed["Description"],
        json!("updated-site"),
        "case DeleteSite: the delete must report the site as last configured, got {removed}"
    );
    let (status, sites) = emulator
        .get(&format!("/global-networks/{network}/sites"))
        .await;
    assert_eq!(
        status, 200,
        "case DeleteSite: GetSites expected 200, got {status} with {sites}"
    );
    assert!(
        !ids_of(&sites, "Sites", "SiteId").contains(&site),
        "case DeleteSite: the deleted site must be gone, got {sites}"
    );

    // DeleteCoreNetwork: the reader of a single resource answers the modelled 404
    // afterwards, naming the resource the request addressed.
    let (status, deleted) = emulator
        .delete(&format!("/core-networks/{core_network}"))
        .await;
    assert_eq!(
        status, 200,
        "case DeleteCoreNetwork: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        state_of("DeleteCoreNetwork", &deleted["CoreNetwork"]),
        DELETING,
        "case DeleteCoreNetwork: expected {DELETING}, got {deleted}"
    );
    let (status, gone) = emulator.get(&format!("/core-networks/{core_network}")).await;
    assert_eq!(
        status, 404,
        "case DeleteCoreNetwork: GetCoreNetwork expected 404, got {status} with {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case DeleteCoreNetwork: wrong error code in {gone}"
    );
    assert_eq!(
        gone["ResourceId"].as_str(),
        Some("coreNetworkId"),
        "case DeleteCoreNetwork: the refusal must identify the resource by the member that \
         addressed it, got {gone}"
    );

    // DeleteGlobalNetwork: the container, now that the child that blocked it is gone.
    let (status, deleted) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "case DeleteGlobalNetwork: expected 200, got {status} with {deleted}"
    );
    assert_eq!(
        state_of("DeleteGlobalNetwork", &deleted["GlobalNetwork"]),
        DELETING,
        "case DeleteGlobalNetwork: expected {DELETING}, got {deleted}"
    );
    let (status, networks) = emulator.get("/global-networks").await;
    assert_eq!(
        status, 200,
        "case DeleteGlobalNetwork: DescribeGlobalNetworks expected 200, got {status} with {networks}"
    );
    assert!(
        !ids_of(&networks, "GlobalNetworks", "GlobalNetworkId").contains(&network),
        "case DeleteGlobalNetwork: the deleted global network must be gone, got {networks}"
    );
}

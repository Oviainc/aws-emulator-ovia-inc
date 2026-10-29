// SPDX-License-Identifier: CC0-1.0

//! The CHANGE-FREE update, over every resource of this service an `Update*` reconfigures,
//! driven end-to-end over HTTP through the real router.
//!
//! One rule is pinned here: an update that asks for the state the addressed resource is
//! ALREADY in leaves the service no change to apply, so it writes nothing — it does not
//! move the resource into the non-terminal state a reconfiguration is outstanding in, does
//! not restart the transition the clock owes, and leaves every member, the id, the ARN,
//! `CreatedAt` and the tags exactly as the call found them. A request asks for no change
//! when it carries no mutable member at all, or carries values the record already holds —
//! a nested structure equal member for member, or a reference naming the resource the
//! record already names (the empty string included, where the record names none).
//!
//! The rows below are the resources whose state enum carries `UPDATING` (`smithy_query
//! shape=GlobalNetworkState,SiteState,LinkState,DeviceState,ConnectionState,CoreNetworkState`),
//! and they differ along the kind of resource, the members its update takes (one, several,
//! a nested structure, a numeric one, a reference) and the reader it is read back through.
//! Each row also makes ONE request that DOES ask for a change, so a row cannot pass by an
//! updater that never records a transition at all, and the same rows are driven again with
//! a reconfiguration already outstanding, where writing nothing also means not restarting
//! the span the clock owes.
//!
//! Nothing reaches into `crate::state`: every resource is created, updated and read back
//! through operations. The harness is the one `global_network_lifecycle` defines, and the
//! readers are `crate::resource_state_machine`'s.

use std::time::Duration;

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, fixture, id_of};
use crate::core_network_lifecycle::create_core_network;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};
use crate::link_lifecycle::create_link_ok;
use crate::resource_state_machine::{read_back, state_of, Reader, AVAILABLE, UPDATING};

/// The state a resource reads while the create's own transition is still outstanding.
const PENDING: &str = "PENDING";

/// Short of the span the clock owes an accepted change, so a request made after waiting
/// this long reaches a resource that is still reconfiguring.
const WITHIN_SPAN: Duration = Duration::from_millis(250);

/// Waited from inside that span, this carries the clock past the instant the accepted
/// change's own transition fell due while staying short of where restarting the span would
/// have pushed it — so a resource read here says which of the two happened.
const PAST_ORIGINAL_SPAN: Duration = Duration::from_millis(400);

/// The `Description` every seeded resource of this suite starts with, so a request can
/// supply the value the record already holds.
const SEEDED_DESCRIPTION: &str = "seeded description";

/// The `Location` the seeded site and device hold.
fn seeded_location() -> Value {
    json!({ "Address": "1 Main St", "Latitude": "47.6062", "Longitude": "-122.3321" })
}

/// The `AWSLocation` the seeded device holds.
fn seeded_aws_location() -> Value {
    json!({ "Zone": "us-west-2a" })
}

/// The `Bandwidth` the seeded link holds.
fn seeded_bandwidth() -> Value {
    json!({ "UploadSpeed": 10, "DownloadSpeed": 20 })
}

/// The policy document the core network row is created with. Its key names
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

/// One row of the table: a settled resource, the requests that ask it for NO change, and
/// one that does.
struct NoOpCase {
    case: &'static str,
    /// The `PATCH` URI the row's updater is addressed by.
    uri: String,
    /// The member of the update's own answer the resource arrives under.
    answer_member: &'static str,
    /// The id the row's resource was minted with.
    id: String,
    /// How the row reads its resource back.
    reader: Reader,
    /// The requests that ask for the state the resource is already in, each labelled.
    change_free: Vec<(&'static str, Value)>,
    /// A request that DOES ask for a change, with the member it has to write and the value
    /// that member must then hold.
    change: (Value, &'static str, Value),
}

/// Seed one resource of every kind an `Update*` of this service reconfigures.
async fn no_op_cases(emulator: &Emulator) -> Vec<NoOpCase> {
    let network = emulator.create_global_network(SEEDED_DESCRIPTION).await;

    let (status, site) = emulator
        .post(
            &format!("/global-networks/{network}/sites"),
            json!({ "Description": SEEDED_DESCRIPTION, "Location": seeded_location() }),
        )
        .await;
    assert_eq!(status, 200, "CreateSite: expected 200, got {status} {site}");
    let site = site["Site"]["SiteId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateSite: no SiteId in {site}"))
        .to_string();

    let link = create_link_ok(
        emulator,
        &network,
        &site,
        &json!({
            "Description": SEEDED_DESCRIPTION,
            "Type": "broadband",
            "Provider": "AnyCompany Networks",
            "Bandwidth": seeded_bandwidth(),
        }),
    )
    .await;

    let device = create_device_ok(
        emulator,
        &network,
        json!({
            "SiteId": site,
            "Description": SEEDED_DESCRIPTION,
            "Type": "router",
            "Vendor": "AnyCompany",
            "Model": "AC-1000",
            "SerialNumber": "SN-0001",
            "AWSLocation": seeded_aws_location(),
            "Location": seeded_location(),
        }),
    )
    .await;

    // A connection needs two devices and the links to tie them over, which the connection
    // suite's fixture builds in a global network of its own.
    let seed = fixture(emulator, "no-op-connection").await;
    let connection = id_of(
        &create_connection_ok(
            emulator,
            &seed.network,
            json!({
                "DeviceId": seed.device,
                "ConnectedDeviceId": seed.connected_device,
                "Description": SEEDED_DESCRIPTION,
                "LinkId": seed.link,
            }),
        )
        .await,
    );

    // A global network holds at most one core network, so this row gets its own parent.
    let core_parent = emulator.create_global_network("no-op-core").await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": core_parent,
            "Description": SEEDED_DESCRIPTION,
            "PolicyDocument": policy_document().to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateCoreNetwork: no CoreNetworkId in {created}"))
        .to_string();

    vec![
        NoOpCase {
            case: "a global network, the one member it takes",
            uri: format!("/global-networks/{network}"),
            answer_member: "GlobalNetwork",
            id: network.clone(),
            reader: Reader::Listed {
                uri: "/global-networks".to_string(),
                member: "GlobalNetworks",
                id_member: "GlobalNetworkId",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the Description the record holds",
                    json!({ "Description": SEEDED_DESCRIPTION }),
                ),
            ],
            change: (
                json!({ "Description": "reconfigured" }),
                "Description",
                json!("reconfigured"),
            ),
        },
        NoOpCase {
            case: "a site, a member and a nested structure",
            uri: format!("/global-networks/{network}/sites/{site}"),
            answer_member: "Site",
            id: site.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/sites"),
                member: "Sites",
                id_member: "SiteId",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the Location the record holds, member for member",
                    json!({ "Location": seeded_location() }),
                ),
                (
                    "both members, each the value the record holds",
                    json!({
                        "Description": SEEDED_DESCRIPTION,
                        "Location": seeded_location(),
                    }),
                ),
            ],
            change: (
                json!({ "Location": { "Address": "9 Elm St" } }),
                "Location",
                json!({ "Address": "9 Elm St" }),
            ),
        },
        NoOpCase {
            case: "a link, three descriptors and a numeric structure",
            uri: format!("/global-networks/{network}/links/{link}"),
            answer_member: "Link",
            id: link.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/links"),
                member: "Links",
                id_member: "LinkId",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the Bandwidth the record holds, speed for speed",
                    json!({ "Bandwidth": seeded_bandwidth() }),
                ),
                (
                    "every member, each the value the record holds",
                    json!({
                        "Description": SEEDED_DESCRIPTION,
                        "Type": "broadband",
                        "Provider": "AnyCompany Networks",
                        "Bandwidth": seeded_bandwidth(),
                    }),
                ),
            ],
            change: (
                json!({ "Bandwidth": { "UploadSpeed": 50, "DownloadSpeed": 20 } }),
                "Bandwidth",
                json!({ "UploadSpeed": 50, "DownloadSpeed": 20 }),
            ),
        },
        NoOpCase {
            case: "a device, five descriptors, two structures and a reference",
            uri: format!("/global-networks/{network}/devices/{device}"),
            answer_member: "Device",
            id: device.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{network}/devices"),
                member: "Devices",
                id_member: "DeviceId",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the site the device already sits at",
                    json!({ "SiteId": site.clone() }),
                ),
                (
                    "the AWSLocation the record holds",
                    json!({ "AWSLocation": seeded_aws_location() }),
                ),
                (
                    "every member, each the value the record holds",
                    json!({
                        "SiteId": site.clone(),
                        "Description": SEEDED_DESCRIPTION,
                        "Type": "router",
                        "Vendor": "AnyCompany",
                        "Model": "AC-1000",
                        "SerialNumber": "SN-0001",
                        "AWSLocation": seeded_aws_location(),
                        "Location": seeded_location(),
                    }),
                ),
            ],
            change: (
                json!({ "Vendor": "Example Networks" }),
                "Vendor",
                json!("Example Networks"),
            ),
        },
        NoOpCase {
            case: "a connection, a member and two references",
            uri: format!(
                "/global-networks/{}/connections/{connection}",
                seed.network
            ),
            answer_member: "Connection",
            id: connection.clone(),
            reader: Reader::Listed {
                uri: format!("/global-networks/{}/connections", seed.network),
                member: "Connections",
                id_member: "ConnectionId",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the link the connection already names",
                    json!({ "LinkId": seed.link.clone() }),
                ),
                (
                    "an emptied reference the record already names none for",
                    json!({ "ConnectedLinkId": "" }),
                ),
                (
                    "every member, each the value the record holds",
                    json!({
                        "Description": SEEDED_DESCRIPTION,
                        "LinkId": seed.link.clone(),
                        "ConnectedLinkId": "",
                    }),
                ),
            ],
            change: (
                json!({ "ConnectedLinkId": seed.connected_link.clone() }),
                "ConnectedLinkId",
                json!(seed.connected_link.clone()),
            ),
        },
        NoOpCase {
            case: "a core network, the one member it takes",
            uri: format!("/core-networks/{core_network}"),
            answer_member: "CoreNetwork",
            id: core_network.clone(),
            reader: Reader::One {
                uri: format!("/core-networks/{core_network}"),
                member: "CoreNetwork",
            },
            change_free: vec![
                ("no mutable member at all", json!({})),
                (
                    "the Description the record holds",
                    json!({ "Description": SEEDED_DESCRIPTION }),
                ),
            ],
            change: (
                json!({ "Description": "reconfigured" }),
                "Description",
                json!("reconfigured"),
            ),
        },
    ]
}

#[tokio::test]
async fn an_update_that_asks_for_no_change_writes_nothing_and_moves_no_state() {
    let emulator = Emulator::new();
    let rows = no_op_cases(&emulator).await;

    // Every create's own transition is past before the first update, so a resource that
    // reads anything but AVAILABLE below was moved by one of these calls.
    tokio::time::sleep(PAST_SETTLE).await;

    for row in &rows {
        let case = row.case;
        let settled = read_back(&emulator, case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(case, &settled),
            AVAILABLE,
            "case {case:?}: expected a settled resource before the updates, got {settled}"
        );

        for (label, body) in &row.change_free {
            let (status, answer) = emulator.patch(&row.uri, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {label:?}: expected 200, got {status} with {answer}"
            );
            let resource = &answer[row.answer_member];
            assert_eq!(
                state_of(case, resource),
                AVAILABLE,
                "case {case:?} / {label:?}: a change-free update owes no transition, got {resource}"
            );
            // The whole record, member for member: a write anywhere in it — a moved state,
            // a restamped instant, a rewritten member — fails here.
            assert_eq!(
                *resource, settled,
                "case {case:?} / {label:?}: expected the record untouched ({settled}), got {resource}"
            );

            // And the next caller sees the same record: nothing was written to the store
            // either.
            let reread = read_back(&emulator, case, &row.id, &row.reader).await;
            assert_eq!(
                reread, settled,
                "case {case:?} / {label:?}: expected the stored record untouched ({settled}), got {reread}"
            );
        }

        // The same updater DOES record a transition for a request that asks for one, so the
        // rows above are not passing against an updater that never moves a state at all.
        let (changing_body, member, expected) = &row.change;
        let (status, answer) = emulator.patch(&row.uri, changing_body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?} / the changing request: expected 200, got {status} with {answer}"
        );
        let resource = &answer[row.answer_member];
        assert_eq!(
            state_of(case, resource),
            UPDATING,
            "case {case:?} / the changing request: an accepted change owes the transition out of {UPDATING}, got {resource}"
        );
        assert_eq!(
            resource[*member], *expected,
            "case {case:?} / the changing request: expected {member} {expected}, got {resource}"
        );

        // Once that transition falls due the resource is settled again, holding what the
        // change wrote — and nothing was called in between.
        tokio::time::sleep(PAST_SETTLE).await;
        let reread = read_back(&emulator, case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(case, &reread),
            AVAILABLE,
            "case {case:?} / the changing request: expected {AVAILABLE} once the transition is due, got {reread}"
        );
        assert_eq!(
            reread[*member], *expected,
            "case {case:?} / the changing request: expected the written {member} {expected} to survive, got {reread}"
        );
    }
}

#[tokio::test]
async fn a_change_free_update_neither_restarts_nor_extends_an_outstanding_transition() {
    let emulator = Emulator::new();
    let rows = no_op_cases(&emulator).await;

    // Every create's own transition is past, so the only transition outstanding below is
    // the one the changing request on each row starts.
    tokio::time::sleep(PAST_SETTLE).await;

    // One timeline covers every row: each gets its reconfiguration started here, so the
    // instant the clock owes its transition at is the same distance away for all of them.
    let mut outstanding = Vec::new();
    for row in &rows {
        let case = row.case;
        let (changing_body, member, expected) = &row.change;
        let (status, answer) = emulator.patch(&row.uri, changing_body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the changing request expected 200, got {status} with {answer}"
        );
        let resource = answer[row.answer_member].clone();
        assert_eq!(
            state_of(case, &resource),
            UPDATING,
            "case {case:?}: an accepted change owes the transition out of {UPDATING}, got {resource}"
        );
        assert_eq!(
            resource[*member], *expected,
            "case {case:?}: expected the changing request to write {member} {expected}, got {resource}"
        );
        outstanding.push(resource);
    }

    // Still inside the span, so the change-free requests below reach a resource that is
    // reconfiguring. Each row asks for the state it is in NOW: nothing at all, and the
    // value its own outstanding change wrote.
    tokio::time::sleep(WITHIN_SPAN).await;
    for (row, started) in rows.iter().zip(&outstanding) {
        let case = row.case;
        let (_, member, expected) = &row.change;
        let change_free = [
            ("no mutable member at all".to_string(), json!({})),
            (
                format!("the {member} the outstanding change wrote"),
                json!({ *member: expected.clone() }),
            ),
        ];
        for (label, body) in &change_free {
            let (status, answer) = emulator.patch(&row.uri, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {label:?}: expected 200, got {status} with {answer}"
            );
            let resource = &answer[row.answer_member];
            assert_eq!(
                *resource, *started,
                "case {case:?} / {label:?}: expected the record the outstanding change left ({started}), got {resource}"
            );
        }
    }

    // Past the instant the ORIGINAL change's transition fell due, but short of where a
    // restarted span would have pushed it: a resource still reconfiguring here was moved
    // by a request that asked for nothing.
    tokio::time::sleep(PAST_ORIGINAL_SPAN).await;
    for row in &rows {
        let case = row.case;
        let (_, member, expected) = &row.change;
        let reread = read_back(&emulator, case, &row.id, &row.reader).await;
        assert_eq!(
            state_of(case, &reread),
            AVAILABLE,
            "case {case:?}: a change-free update must not restart the span the accepted change owed, got {reread}"
        );
        assert_eq!(
            reread[*member], *expected,
            "case {case:?}: expected the written {member} {expected} to survive, got {reread}"
        );
    }
}

#[tokio::test]
async fn an_update_that_asks_for_no_change_leaves_a_still_settling_resource_pending() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network(SEEDED_DESCRIPTION).await;
    let site = emulator.create_site(&network, SEEDED_DESCRIPTION).await;
    let link = create_link_ok(
        &emulator,
        &network,
        &site,
        &json!({ "Description": SEEDED_DESCRIPTION, "Bandwidth": seeded_bandwidth() }),
    )
    .await;

    // No wait: the transition each create owes is still outstanding, so each resource is
    // PENDING — and a change-free update must leave it that way rather than reporting a
    // reconfiguration nothing asked for.
    let rows: Vec<(&str, String, &str, Value)> = vec![
        (
            "a still settling site",
            format!("/global-networks/{network}/sites/{site}"),
            "Site",
            json!({ "Description": SEEDED_DESCRIPTION }),
        ),
        (
            "a still settling link",
            format!("/global-networks/{network}/links/{link}"),
            "Link",
            json!({ "Description": SEEDED_DESCRIPTION, "Bandwidth": seeded_bandwidth() }),
        ),
        (
            "a still settling link, no member at all",
            format!("/global-networks/{network}/links/{link}"),
            "Link",
            json!({}),
        ),
    ];

    for (case, uri, answer_member, body) in &rows {
        let (status, answer) = emulator.patch(uri, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {answer}"
        );
        let resource = &answer[*answer_member];
        assert_eq!(
            state_of(case, resource),
            PENDING,
            "case {case:?}: a change-free update must leave the create's own transition the only one outstanding, got {resource}"
        );
    }

    // Once the creates' transitions fall due, both resources settle — nothing is waiting on
    // an update that asked for nothing.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, uri, answer_member, _) in &rows {
        let (status, answer) = emulator.patch(uri, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?} after settling: expected 200, got {status} with {answer}"
        );
        let resource = &answer[*answer_member];
        assert_eq!(
            state_of(case, resource),
            AVAILABLE,
            "case {case:?} after settling: expected {AVAILABLE}, got {resource}"
        );
    }
}

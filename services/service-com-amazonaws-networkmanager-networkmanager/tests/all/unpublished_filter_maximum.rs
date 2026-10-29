// SPDX-License-Identifier: CC0-1.0

//! The id-list filters of this service whose documentation publishes NO maximum number of
//! values, driven end-to-end over HTTP.
//!
//! The rule under test: a list-valued filter member is refused for carrying too many values
//! only where the service documents a maximum for that member; a member whose documentation
//! publishes none is served the listing it selects at whatever length the request carries.
//! `crate::filter_id_list_maximum` pins the first half of that rule — the members that DO
//! publish "The maximum is 10." are refused past it — and this suite pins the second half
//! for the members that publish nothing of the kind:
//!
//! * `GetConnectionsRequest$ConnectionIds` — "One or more connection IDs."
//!   (`smithy_query query="doc" shape="GetConnectionsRequest" member="ConnectionIds"`)
//! * `GetTransitGatewayConnectPeerAssociationsRequest$TransitGatewayConnectPeerArns` — "One
//!   or more transit gateway Connect peer Amazon Resource Names (ARNs)." (`smithy_query
//!   query="doc" shape="GetTransitGatewayConnectPeerAssociationsRequest"
//!   member="TransitGatewayConnectPeerArns"`)
//!
//! The third member of the class,
//! `GetConnectPeerAssociationsRequest$ConnectPeerIds` ("The IDs of the Connect peers."),
//! is left to `crate::connect_peer_association_lifecycle`'s own fixture, which is what
//! stands a Connect peer up: a peer needs a core network and a Connect attachment per peer,
//! so a dozen of them would rebuild that whole suite here for no further row of this rule.
//!
//! The rows are the shared length table `crate::filter_maximum_table` defines, crossed with
//! its two kinds of value (ids that name stored records and ids of the same form that name
//! nothing). Its landmark — the number the OTHER filters publish — is only a landmark here:
//! these members publish no bound, so every row, at it, one past it and far past it with
//! values repeated, is answered the page it selects. The last test keeps both halves of the
//! rule in one emulator, so what decides the refusal is the MEMBER and not the length.
//!
//! Nothing reaches into `crate::state`: every record is created and read back through an
//! operation. The harness is the one `global_network_lifecycle` defines — one shared engine,
//! this service, the production dispatch.

use serde_json::{json, Value};

use crate::connection_lifecycle::{create_connection_ok, get_connections, id_of, ids_in};
use crate::device_lifecycle::create_device_ok;
use crate::filter_maximum_table::{
    assert_filter_refusal, filtered_uri, unknown_ids, FIXTURE_COUNT, KINDS, MAX_FILTER_VALUES, ROWS,
};
use crate::global_network_lifecycle::Emulator;
use crate::transit_gateway_connect_peer_association_lifecycle::{
    arns_of, associate, connect_peer_arn, register_gateway,
};
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The partition and Region the Connect peer fixture lives in: one registered transit
/// gateway of that Region and account is what lets its peers be associated at all, so
/// holding them fixed keeps the fixture to a single registration.
const PARTITION: &str = "aws";
const REGION: &str = "us-west-2";

/// One listing whose id-list filter member publishes no maximum.
struct Listing {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The filter member whose documentation publishes no maximum.
    member: &'static str,
    /// The query parameter that member binds to (`@httpQuery`).
    query: &'static str,
    /// The listing's URI, without a query string.
    uri: String,
    /// Every value the filter can name that IS stored, in the order the listing answers
    /// them.
    stored: Vec<String>,
    /// Values of the same form that name nothing stored.
    unknown: Vec<String>,
    /// The identifiers one page carries, in the order returned.
    read: fn(&Value) -> Vec<String>,
}

/// The `ConnectionId`s one `GetConnections` page carries.
fn listed_connections(body: &Value) -> Vec<String> {
    ids_in(body)
}

/// The `TransitGatewayConnectPeerArn`s one `GetTransitGatewayConnectPeerAssociations` page
/// carries.
fn listed_peer_arns(body: &Value) -> Vec<String> {
    arns_of("the transit gateway Connect peer association listing", body)
}

/// A global network with two devices and [`FIXTURE_COUNT`] connections between them.
async fn connections(emulator: &Emulator) -> Listing {
    let network = emulator.create_global_network("connection ids").await;
    let near = create_device_ok(emulator, &network, json!({ "Description": "near" })).await;
    let far = create_device_ok(emulator, &network, json!({ "Description": "far" })).await;
    let mut stored: Vec<String> = Vec::new();
    for _ in 0..FIXTURE_COUNT {
        stored.push(id_of(
            &create_connection_ok(
                emulator,
                &network,
                json!({ "DeviceId": near, "ConnectedDeviceId": far }),
            )
            .await,
        ));
    }
    Listing {
        operation: "GetConnections",
        member: "ConnectionIds",
        query: "connectionIds",
        uri: format!("/global-networks/{network}/connections"),
        stored,
        unknown: unknown_ids("connection"),
        read: listed_connections,
    }
}

/// A global network with one device, one registered transit gateway and [`FIXTURE_COUNT`]
/// Connect peers of that gateway tied to the device.
async fn connect_peer_associations(emulator: &Emulator) -> Listing {
    let network = emulator
        .create_global_network("transit gateway connect peer arns")
        .await;
    let device = create_device_ok(emulator, &network, json!({ "Description": "peered" })).await;
    let account = own_account();
    register_gateway(
        emulator,
        &network,
        &transit_gateway_arn(PARTITION, REGION, &account, 0, 17),
    )
    .await;

    let peer = |index: usize| connect_peer_arn(PARTITION, REGION, &account, index, 17);
    let mut stored: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        let arn = peer(index);
        let (status, body) = associate(
            emulator,
            &network,
            json!({ "TransitGatewayConnectPeerArn": arn, "DeviceId": device }),
        )
        .await;
        assert_eq!(
            status, 200,
            "AssociateTransitGatewayConnectPeer({arn}): expected 200, got {status} with {body}"
        );
        stored.push(arn);
    }
    // ARNs of the same form for peers nothing was associated with: the indices past the
    // ones the fixture tied.
    let unknown: Vec<String> = (0..FIXTURE_COUNT)
        .map(|index| peer(FIXTURE_COUNT + index))
        .collect();

    Listing {
        operation: "GetTransitGatewayConnectPeerAssociations",
        member: "TransitGatewayConnectPeerArns",
        query: "transitGatewayConnectPeerArns",
        uri: format!("/global-networks/{network}/transit-gateway-connect-peer-associations"),
        stored,
        unknown,
        read: listed_peer_arns,
    }
}

#[tokio::test]
async fn a_filter_member_publishing_no_maximum_is_served_at_every_length() {
    let emulator = Emulator::new();
    let listings = [
        connections(&emulator).await,
        connect_peer_associations(&emulator).await,
    ];

    for listing in &listings {
        let operation = listing.operation;
        let member = listing.member;
        assert_eq!(
            listing.stored.len(),
            FIXTURE_COUNT,
            "case {operation}: the fixture must hold {FIXTURE_COUNT} values, got {:?}",
            listing.stored
        );

        for kind in &KINDS {
            let available = if kind.unknown {
                &listing.unknown
            } else {
                &listing.stored
            };
            for row in &ROWS {
                let case = format!("{operation}/{member}/{}/{}", kind.name, row.name);
                let values = row.values(available);
                let (status, body) = emulator
                    .get(&filtered_uri(&listing.uri, listing.query, &values, ""))
                    .await;

                // No length is refused: the member publishes no maximum, so the listing is
                // computed however many values the filter carried.
                assert_eq!(
                    status,
                    200,
                    "case {case} ({} values): expected 200, got {status} with {body}",
                    values.len()
                );

                // The page holds the stored records the filter names, in the listing's own
                // key order and one entry per record however often the filter repeats it;
                // an absent filter matches everything, and values naming nothing select
                // nothing.
                let expected: Vec<String> = if values.is_empty() {
                    listing.stored.clone()
                } else {
                    listing
                        .stored
                        .iter()
                        .filter(|held| values.contains(held))
                        .cloned()
                        .collect()
                };
                assert_eq!(
                    (listing.read)(&body),
                    expected,
                    "case {case}: expected {expected:?}, got {body}"
                );
                assert!(
                    body["NextToken"].is_null(),
                    "case {case}: a page holding every selected record carries no token, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_long_filter_narrows_with_the_other_selectors_and_changes_nothing() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("long filter").await;
    let near = create_device_ok(&emulator, &network, json!({ "Description": "near" })).await;
    let far = create_device_ok(&emulator, &network, json!({ "Description": "far" })).await;
    // A third device that no connection names, so a filter on it selects none.
    let apart = create_device_ok(&emulator, &network, json!({ "Description": "apart" })).await;
    let mut held: Vec<String> = Vec::new();
    for _ in 0..FIXTURE_COUNT {
        held.push(id_of(
            &create_connection_ok(
                &emulator,
                &network,
                json!({ "DeviceId": near, "ConnectedDeviceId": far }),
            )
            .await,
        ));
    }

    // Every row carries MORE ids than the documented filters of this service allow, so each
    // is a request the invented bound would have refused; what narrows the answer is the
    // rest of the query string.
    let ids: Vec<String> = held
        .iter()
        .take(MAX_FILTER_VALUES + 1)
        .map(String::clone)
        .collect();
    let filter: String = ids
        .iter()
        .map(|id| format!("connectionIds={id}"))
        .collect::<Vec<String>>()
        .join("&");
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("the ids alone", filter.clone(), ids.clone()),
        (
            "an end of every connection",
            format!("{filter}&deviceId={near}"),
            ids.clone(),
        ),
        (
            "the other end of every connection",
            format!("{filter}&deviceId={far}"),
            ids.clone(),
        ),
        (
            "a device no connection names",
            format!("{filter}&deviceId={apart}"),
            Vec::new(),
        ),
        (
            "a page smaller than the filter",
            format!("{filter}&maxResults=2"),
            ids.iter().take(2).map(String::clone).collect(),
        ),
    ];

    for (case, query, expected) in rows {
        let (status, body) = get_connections(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_in(&body),
            expected,
            "case {case}: expected {expected:?}, got {body}"
        );
    }

    // And nothing any of those reads did changed what a later caller sees: the unfiltered
    // listing still answers every connection the fixture created.
    let (status, body) = get_connections(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case the unfiltered read after the long filters: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_in(&body),
        held,
        "case the unfiltered read after the long filters: expected every connection, got {body}"
    );
}

#[tokio::test]
async fn the_refusal_past_the_maximum_follows_the_member_and_not_the_length() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("published or not").await;
    let near = create_device_ok(&emulator, &network, json!({ "Description": "near" })).await;
    let far = create_device_ok(&emulator, &network, json!({ "Description": "far" })).await;
    let mut held: Vec<String> = Vec::new();
    for _ in 0..=MAX_FILTER_VALUES {
        held.push(id_of(
            &create_connection_ok(
                &emulator,
                &network,
                json!({ "DeviceId": near, "ConnectedDeviceId": far }),
            )
            .await,
        ));
    }
    let mut sites: Vec<String> = Vec::new();
    for index in 0..=MAX_FILTER_VALUES {
        sites.push(emulator.create_site(&network, &format!("site {index}")).await);
    }

    // One value past the landmark on a member that PUBLISHES the maximum: the modelled
    // refusal, naming that member and carrying no page.
    let query: Vec<String> = sites.iter().map(|id| format!("siteIds={id}")).collect();
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{network}/sites?{}",
            query.join("&")
        ))
        .await;
    assert_filter_refusal(
        "GetSites/SiteIds one past the published maximum",
        status,
        &body,
        "SiteIds",
        "Sites",
    );

    // The very same number of values on a member that publishes none: the page it selects.
    let query: Vec<String> = held.iter().map(|id| format!("connectionIds={id}")).collect();
    let (status, body) = get_connections(&emulator, &network, &query.join("&")).await;
    assert_eq!(
        status, 200,
        "case GetConnections/ConnectionIds at the same length: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_in(&body),
        held,
        "case GetConnections/ConnectionIds at the same length: expected every connection, got {body}"
    );
}

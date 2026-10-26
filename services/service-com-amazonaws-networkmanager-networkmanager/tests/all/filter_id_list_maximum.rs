// SPDX-License-Identifier: CC0-1.0

//! The documented maximum on the id-list filters of this service's reporting operations,
//! driven over HTTP.
//!
//! The rule under test: an id-list filter whose member documents "The maximum is 10" is
//! honoured up to that many ids and REFUSED past it with the modelled
//! `ValidationException` (HTTP 400) naming that member, before any listing is computed.
//! The model expresses the bound nowhere — `GlobalNetworkIdList`, `SiteIdList`,
//! `LinkIdList` and `DeviceIdList` carry a length on their member STRING only — so the
//! rule holds for every such filter, not only the one the issue cited. It reaches
//! `GetConnectionsRequest$ConnectionIds` too, whose own documentation is only "One or more
//! connection IDs." (`smithy_query query="doc" shape="GetConnectionsRequest"
//! member="ConnectionIds"`): that filter takes the class's bound, not a sentence of its
//! own.
//!
//! The table of lengths is the shared one `crate::filter_maximum_table` defines, with rows
//! differing along the NUMBER of ids the filter carries: none, a few, the bound
//! itself, one past it and far past it, plus a row whose values repeat (the bound counts
//! the values the list carries, not the distinct resources they name). This suite spans
//! every id-list filter this crate's implemented listings take over those rows.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::json;

use crate::connection_lifecycle::{create_connection_ok, id_of};
use crate::device_lifecycle::create_device_ok;
use crate::filter_maximum_table::{assert_filter_refusal, FIXTURE_COUNT, MAX_FILTER_VALUES, ROWS};
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// One listing operation that takes a documented id-list filter.
struct Filter {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The input member the documented maximum sits on; a refusal must name it.
    member: &'static str,
    /// The query parameter that member binds to (`@httpQuery`, e.g. `globalNetworkIds`).
    query: &'static str,
    /// The output list the page carries, and the id member of its items.
    list: &'static str,
    id_member: &'static str,
    /// The listing's URI, without a query string.
    uri: String,
    /// Every id of the kind this filter names, in the order the listing answers them.
    ids: Vec<String>,
}

/// The URI a row addresses: the listing plus one query parameter per filter value, the way
/// a client binds a list to `@httpQuery` (ids of this service are hex and need no escaping).
fn uri_for(filter: &Filter, values: &[String]) -> String {
    if values.is_empty() {
        return filter.uri.clone();
    }
    let query: Vec<String> = values
        .iter()
        .map(|id| format!("{}={id}", filter.query))
        .collect();
    format!("{}?{}", filter.uri, query.join("&"))
}

/// Create the fixture — [`FIXTURE_COUNT`] global networks, and that many sites, links,
/// devices and connections inside the first of them — and describe each filter over it.
async fn filters(emulator: &Emulator) -> Vec<Filter> {
    let mut networks: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        networks.push(
            emulator
                .create_global_network(&format!("network {index}"))
                .await,
        );
    }
    let parent = networks[0].clone();

    let mut sites: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        sites.push(emulator.create_site(&parent, &format!("site {index}")).await);
    }

    let mut links: Vec<String> = Vec::new();
    let mut devices: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        links.push(create_link_ok(emulator, &parent, &sites[0], &plain_link()).await);
        devices.push(
            create_device_ok(
                emulator,
                &parent,
                json!({ "Description": format!("device {index}") }),
            )
            .await,
        );
    }

    // A connection ties two of the global network's devices together, so the connections
    // come after the devices and all hang off the same pair.
    let mut connections: Vec<String> = Vec::new();
    for _ in 0..FIXTURE_COUNT {
        connections.push(id_of(
            &create_connection_ok(
                emulator,
                &parent,
                json!({ "DeviceId": devices[0], "ConnectedDeviceId": devices[1] }),
            )
            .await,
        ));
    }

    vec![
        Filter {
            operation: "DescribeGlobalNetworks",
            member: "GlobalNetworkIds",
            query: "globalNetworkIds",
            list: "GlobalNetworks",
            id_member: "GlobalNetworkId",
            uri: "/global-networks".to_string(),
            ids: networks,
        },
        Filter {
            operation: "GetSites",
            member: "SiteIds",
            query: "siteIds",
            list: "Sites",
            id_member: "SiteId",
            uri: format!("/global-networks/{parent}/sites"),
            ids: sites,
        },
        Filter {
            operation: "GetLinks",
            member: "LinkIds",
            query: "linkIds",
            list: "Links",
            id_member: "LinkId",
            uri: format!("/global-networks/{parent}/links"),
            ids: links,
        },
        Filter {
            operation: "GetDevices",
            member: "DeviceIds",
            query: "deviceIds",
            list: "Devices",
            id_member: "DeviceId",
            uri: format!("/global-networks/{parent}/devices"),
            ids: devices,
        },
        Filter {
            operation: "GetConnections",
            member: "ConnectionIds",
            query: "connectionIds",
            list: "Connections",
            id_member: "ConnectionId",
            uri: format!("/global-networks/{parent}/connections"),
            ids: connections,
        },
    ]
}

#[tokio::test]
async fn id_list_filters_are_honoured_up_to_the_documented_maximum_and_refused_past_it() {
    let emulator = Emulator::new();
    let filters = filters(&emulator).await;

    for filter in &filters {
        let operation = filter.operation;
        let member = filter.member;
        assert_eq!(
            filter.ids.len(),
            FIXTURE_COUNT,
            "case {operation}: the fixture must hold {FIXTURE_COUNT} ids, got {:?}",
            filter.ids
        );

        for row in &ROWS {
            let case = format!("{operation}/{}", row.name);
            let values = row.values(&filter.ids);
            let uri = uri_for(filter, &values);
            let (status, body) = emulator.get(&uri).await;

            if values.len() > MAX_FILTER_VALUES {
                // Past the documented maximum: the modelled ValidationException, naming the
                // member that carried the list, and no page at all — as
                // `crate::filter_maximum_table` states it for every member of this class.
                assert_filter_refusal(&case, status, &body, member, filter.list);
                continue;
            }

            // At or below the maximum: the listing is computed and filtered by the ids the
            // request named. An absent filter matches everything.
            assert_eq!(
                status, 200,
                "case {case} ({} ids): expected 200, got {status} with {body}",
                values.len()
            );
            let expected: Vec<String> = if values.is_empty() {
                filter.ids.clone()
            } else {
                values.clone()
            };
            assert_eq!(
                ids_of(&body, filter.list, filter.id_member),
                expected,
                "case {case}: expected exactly {expected:?}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_past_the_maximum_is_refused_before_the_listing_is_computed() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("ordering").await;

    // The filter's own bound is judged against the request, so ids that name nothing —
    // and a page size that would otherwise bound the answer to one item — do not change
    // the refusal: no listing runs.
    // Ids of the minted shape (`<kind>-<17 lowercase hex>`) that name nothing: the leading
    // `f` puts them above anything the monotonic id counter can mint.
    let unknown: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("global-network-f{index:016x}"))
        .collect();
    let query: Vec<String> = unknown
        .iter()
        .map(|id| format!("globalNetworkIds={id}"))
        .collect();
    let (status, body) = emulator
        .get(&format!("/global-networks?{}&maxResults=1", query.join("&")))
        .await;
    assert_eq!(
        status, 400,
        "case unknown ids past the maximum: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case unknown ids past the maximum: expected ValidationException, got {body}"
    );

    // The same ids inside the bound are a selection that matches nothing — the filter
    // selects among the stored networks rather than naming a resource the request
    // addresses — so they answer an empty page. That is the proof the refusal above is the
    // LENGTH and nothing else: one id fewer and the very same values are served.
    let query: Vec<String> = unknown
        .iter()
        .take(MAX_FILTER_VALUES)
        .map(|id| format!("globalNetworkIds={id}"))
        .collect();
    let (status, body) = emulator
        .get(&format!("/global-networks?{}", query.join("&")))
        .await;
    assert_eq!(
        status, 200,
        "case unknown ids inside the maximum: expected 200, got {status} with {body}"
    );
    assert!(
        ids_of(&body, "GlobalNetworks", "GlobalNetworkId").is_empty(),
        "case unknown ids inside the maximum: expected an empty page, got {body}"
    );

    // The network the fixture created is still listed, so nothing the refused request
    // carried changed what a later caller sees.
    let (status, body) = emulator.get("/global-networks").await;
    assert_eq!(
        status, 200,
        "case unfiltered read after a refusal: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
        vec![network],
        "case unfiltered read after a refusal: expected the created network, got {body}"
    );
}

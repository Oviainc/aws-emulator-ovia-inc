// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetNetworkResourceCounts`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! `resourceType` / `maxResults` / `nextToken` query bindings and the error envelopes are
//! exercised as a client hits them. Nothing reaches into `crate::state`: every expected
//! count is built from the resources the test created through `CreateSite` / `CreateLink`.
//!
//! The rule under test: a `ResourceType` outside the set the member's documentation
//! enumerates is refused with HTTP 400 / `ValidationException`, and a supported one is
//! answered with the number of resources of that type the addressed global network holds
//! in the store.

use serde_json::Value;

use crate::global_network_lifecycle::{error_code, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::resource_tagging::escape;

/// Every value `ResourceType` accepts, from the enumeration in the member's own
/// documentation (`smithy_query query="doc" shape="GetNetworkResourceCountsRequest"
/// member="ResourceType"`): three Direct Connect types (`dxcon`, `dx-gateway`, `dx-vif`),
/// eight Network Manager types (`attachment`, `connect-peer`, `connection`,
/// `core-network`, `device`, `link`, `peering`, `site`) and six Amazon VPC types
/// (`customer-gateway`, `transit-gateway`, `transit-gateway-attachment`,
/// `transit-gateway-connect-peer`, `transit-gateway-route-table`, `vpn-connection`).
///
/// Written here in lexicographic order, which is the order a request that names no
/// `ResourceType` pages them in.
const SUPPORTED_TYPES: [&str; 17] = [
    "attachment",
    "connect-peer",
    "connection",
    "core-network",
    "customer-gateway",
    "device",
    "dx-gateway",
    "dx-vif",
    "dxcon",
    "link",
    "peering",
    "site",
    "transit-gateway",
    "transit-gateway-attachment",
    "transit-gateway-connect-peer",
    "transit-gateway-route-table",
    "vpn-connection",
];

/// The `GetNetworkResourceCounts` URI of `network`, with `query` (already escaped)
/// appended when it is not empty.
fn counts_uri(network: &str, query: &str) -> String {
    let path = format!("/global-networks/{network}/network-resource-count");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// The `(ResourceType, Count)` pairs one answer carries, in the order returned.
fn counts_of(body: &Value) -> Vec<(String, i64)> {
    body["NetworkResourceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a NetworkResourceCounts array in {body}"))
        .iter()
        .map(|entry| {
            let resource_type = entry["ResourceType"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ResourceType in {entry}"))
                .to_string();
            let count = entry["Count"]
                .as_i64()
                .unwrap_or_else(|| panic!("expected a Count in {entry}"));
            (resource_type, count)
        })
        .collect()
}

/// `GetNetworkResourceCounts(ResourceType=resource_type)` against `network`.
async fn count_of_type(emulator: &Emulator, network: &str, resource_type: &str) -> (u16, Value) {
    let query = format!("resourceType={}", escape(resource_type));
    emulator.get(&counts_uri(network, &query)).await
}

#[tokio::test]
async fn a_resource_type_outside_the_documented_set_is_refused_with_the_modelled_validation_error() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("counts").await;
    emulator.create_site(&network, "hq").await;

    // Each row is a different WAY of being outside the documented set: an invented name,
    // a documented name in the wrong case, pluralised, padded, misspelled with the wrong
    // separator, a prefix of a documented name, and identifiers that are not types.
    for (case, resource_type) in [
        ("an invented type", "not-a-real-type"),
        ("a documented type in the wrong case", "Site"),
        ("a documented type pluralised", "sites"),
        ("a documented type with a trailing space", "site "),
        ("an underscore instead of a hyphen", "core_network"),
        ("a prefix of a documented type", "transit"),
        ("a resource id instead of a type", "site-00000000000000001"),
        (
            "an ARN instead of a type",
            "arn:aws:networkmanager::111122223333:site/site-00000000000000001",
        ),
    ] {
        let (status, body) = count_of_type(&emulator, &network, resource_type).await;
        assert_eq!(
            status, 400,
            "case {case} ({resource_type:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({resource_type:?}): wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn every_supported_type_reports_the_count_its_own_global_network_holds() {
    let emulator = Emulator::new();

    // The rows differ in the inventory the global network holds: none, sites only, one
    // site with links, and several sites each with links. Every row runs on the same
    // emulator, so a count that leaked across global networks would show up here.
    for (case, sites, links_per_site) in [
        ("an empty network", 0, 0),
        ("one site, no links", 1, 0),
        ("one site with links", 1, 3),
        ("three sites with links each", 3, 2),
    ] {
        let network = emulator.create_global_network(case).await;
        let mut expected_links = 0;
        for site_index in 0..sites {
            let site = emulator
                .create_site(&network, &format!("{case}-site-{site_index}"))
                .await;
            for _ in 0..links_per_site {
                create_link_ok(&emulator, &network, &site, &plain_link()).await;
                expected_links += 1;
            }
        }

        for resource_type in SUPPORTED_TYPES {
            // Only the kinds this service stores can be non-zero; every other documented
            // kind is held nowhere, so its count is the zero the store really holds.
            let expected = match resource_type {
                "site" => sites,
                "link" => expected_links,
                _ => 0,
            };
            let (status, body) = count_of_type(&emulator, &network, resource_type).await;
            assert_eq!(
                status, 200,
                "case {case}/{resource_type}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                counts_of(&body),
                vec![(resource_type.to_string(), i64::from(expected))],
                "case {case}/{resource_type}: expected the one count {expected}, got {body}"
            );
            assert!(
                body["NextToken"].is_null(),
                "case {case}/{resource_type}: a single-type answer has no next page, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn omitting_the_resource_type_reports_every_supported_type_once_whatever_the_page_size() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("inventory").await;
    let site = emulator.create_site(&network, "hq").await;
    create_link_ok(&emulator, &network, &site, &plain_link()).await;
    create_link_ok(&emulator, &network, &site, &plain_link()).await;

    // `MaxResults` is modelled `range 1..500`, so the rows are no page size at all, each
    // published bound, and sizes either side of the catalogue's own length.
    for page_size in [None, Some(1), Some(2), Some(16), Some(17), Some(18), Some(500)] {
        let mut collected: Vec<(String, i64)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(next) = &token {
                query.push(format!("nextToken={}", escape(next)));
            }
            let (status, body) = emulator.get(&counts_uri(&network, &query.join("&"))).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size:?}: expected 200, got {status} with {body}"
            );
            let page = counts_of(&body);
            assert!(
                !page.is_empty(),
                "case maxResults={page_size:?}: a page while items remain is never empty, got {body}"
            );
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case maxResults={page_size:?}: page of {} exceeds the requested size, got {body}",
                    page.len()
                );
            }
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= SUPPORTED_TYPES.len() + 1,
                "case maxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        let listed: Vec<String> = collected
            .iter()
            .map(|(resource_type, _)| resource_type.clone())
            .collect();
        let expected_types: Vec<String> = SUPPORTED_TYPES.iter().map(|t| (*t).to_string()).collect();
        assert_eq!(
            listed, expected_types,
            "case maxResults={page_size:?}: expected every documented type exactly once in order, got {listed:?}"
        );
        for (resource_type, count) in &collected {
            let expected = match resource_type.as_str() {
                "site" => 1,
                "link" => 2,
                _ => 0,
            };
            assert_eq!(
                *count, expected,
                "case maxResults={page_size:?}/{resource_type}: expected {expected}, got {count}"
            );
        }
    }
}

#[tokio::test]
async fn a_global_network_the_store_does_not_hold_is_refused_rather_than_counted() {
    let emulator = Emulator::new();
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{deleted}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    for (case, network) in [
        ("never created", "global-network-00000000000000099".to_string()),
        ("deleted", deleted),
        ("not an id of this service", "nope".to_string()),
    ] {
        // This operation models no `ResourceNotFoundException`
        // (`smithy_query operation="GetNetworkResourceCounts"` lists AccessDenied /
        // InternalServer / Throttling / Validation only), so the refusal is the 400.
        for resource_type in ["site", ""] {
            let query = if resource_type.is_empty() {
                String::new()
            } else {
                format!("resourceType={resource_type}")
            };
            let (status, body) = emulator.get(&counts_uri(&network, &query)).await;
            assert_eq!(
                status, 400,
                "case {case} (resourceType={resource_type:?}): expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case} (resourceType={resource_type:?}): wrong error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn counting_is_a_read_so_a_later_caller_sees_the_same_inventory() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("stable").await;
    let site = emulator.create_site(&network, "hq").await;
    create_link_ok(&emulator, &network, &site, &plain_link()).await;

    let (_, first) = emulator.get(&counts_uri(&network, "")).await;
    let first_counts = counts_of(&first);
    for repeat in 1..=3 {
        let (status, again) = emulator.get(&counts_uri(&network, "")).await;
        assert_eq!(
            status, 200,
            "case repeat {repeat}: expected 200, got {status} with {again}"
        );
        assert_eq!(
            counts_of(&again),
            first_counts,
            "case repeat {repeat}: counting changed the inventory, got {again}"
        );
    }

    // The resources themselves are untouched by the reads.
    let (status, sites) = emulator
        .get(&format!("/global-networks/{network}/sites"))
        .await;
    assert_eq!(
        status, 200,
        "GetSites after counting: expected 200, got {status} with {sites}"
    );
    assert_eq!(
        sites["Sites"].as_array().map(Vec::len),
        Some(1),
        "GetSites after counting: expected the one site to survive, got {sites}"
    );
}

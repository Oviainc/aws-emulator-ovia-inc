// SPDX-License-Identifier: CC0-1.0

//! The ONE page bound every `@paginated` operation of this service reports a page by,
//! driven end to end over awsJson1_0 through all five of them.
//!
//! The rule under test: a page of a paginated operation of this service reports at most
//! the `maxResults` the request named, and at most the top of the modelled `MaxResults`
//! range when it named none — "The maximum number of results to return. If you specify
//! MaxResults in the request, the response includes information up to the limit
//! specified" (`smithy_query(query="operation",operation="ListEnvironments")`, which
//! also gives the range `MaxResults` is declared over: 1..100). The same member, with
//! the same doc and the same range, is on `ListEnvironments`,
//! `ListEnvironmentConnectors`, `ListEnvironmentHosts`, `ListEnvironmentVlans` and
//! `ListVmEntitlements`, so one bound answers for all five — and one value outside that
//! range is refused by all five, whichever side of it the caller lands on.
//!
//! Each row of the table is a different `maxResults`, which is what the rule
//! generalises over: none named, the floor of the range, a size the collection is not a
//! multiple of, a size that divides it, one larger than the collection and the ceiling
//! of the range. The collections differ in size too (three environments, three hosts,
//! three connectors, three entitlements, ten VLAN subnets), so a page of the same size
//! is checked against several collection lengths.
//!
//! The harness (`emulator`, `call`, `create_settled`) is the one `environment_lifecycle`
//! already owns, and the connector helper the one `entitlement_connector` owns, so every
//! test drives the service the way a client does.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::entitlement_connector::connector_in;
use crate::environment_lifecycle::{call, create_settled, emulator, error_type, Emulator};

/// Every `ConnectorType` the model declares, from the catalog this crate generated from
/// the model (`smithy_query(query="shape", shape="ConnectorType")`).
const CONNECTOR_TYPES: &[&str] =
    service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::CONNECTOR_TYPE_VALUES;

/// The type of connector an entitlement is created and listed through.
const VCENTER: &str = "VCENTER";

/// The only `EntitlementType` the model declares
/// (`smithy_query(query="shape", shape="EntitlementType")`).
const WINDOWS_SERVER: &str = "WINDOWS_SERVER";

/// The top of the modelled `MaxResults` range — "maxResults (MaxResults, range 1..100)"
/// (`smithy_query(query="operation",operation="ListEnvironments")`) — and so the bound a
/// request that names no `maxResults` gets.
const MAX_RESULTS_CEILING: i64 = 100;

/// The bottom of that same modelled range.
const MAX_RESULTS_FLOOR: i64 = 1;

/// One paginated operation of this service, with the request that addresses the
/// collection it pages over and how many items that collection holds.
struct Listing {
    /// The operation name the request targets.
    operation: &'static str,
    /// The output member the page arrives in.
    member: &'static str,
    /// Everything the request carries apart from `maxResults` and `nextToken`.
    request: Value,
    /// How many items the addressed collection holds.
    total: usize,
}

/// How a case names itself in an assertion message.
fn case_of(listing: &Listing, max_results: Option<i64>) -> String {
    format!(
        "{} with maxResults {max_results:?} over {} items",
        listing.operation, listing.total
    )
}

/// Build one request: the listing's own members, plus the `maxResults` the case names
/// (omitted when it names none) and a `nextToken` when one is being resumed.
fn request_for(listing: &Listing, max_results: Option<i64>, next_token: Option<&str>) -> Value {
    let mut request = listing.request.clone();
    if let Some(bound) = max_results {
        request["maxResults"] = json!(bound);
    }
    if let Some(token) = next_token {
        request["nextToken"] = json!(token);
    }
    request
}

/// Page through one listing with the `maxResults` the case names, returning every entry
/// reported (as its JSON text, so entries of any of the five shapes compare) and the
/// length of each page in the order the pages arrived.
async fn walk(
    router: &Emulator,
    listing: &Listing,
    max_results: Option<i64>,
) -> (Vec<String>, Vec<usize>) {
    let case = case_of(listing, max_results);
    let mut entries: Vec<String> = Vec::new();
    let mut page_lengths: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let request = request_for(listing, max_results, next_token.as_deref());
        let (status, body) = call(router, listing.operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
        let page = body[listing.member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no {} in {body}", listing.member));
        page_lengths.push(page.len());
        entries.extend(page.iter().map(Value::to_string));
        assert!(
            page_lengths.len() <= listing.total + 1,
            "case {case}: pagination did not terminate after {} pages, got {page_lengths:?}",
            page_lengths.len()
        );
        match body["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    (entries, page_lengths)
}

/// The five collections this suite pages over, in one emulator: three environments, and
/// on one of them three hosts, one connector of each modelled type, three entitlements
/// and the ten VLAN subnets every create provisions.
async fn populated(router: &Emulator) -> Vec<Listing> {
    // Settled, because a list addressed to an environment that is still initializing is
    // refused (`tests/all/list_while_initializing.rs` owns that rule), and because a
    // host can only be added once the environment is deployed.
    let subject = create_settled(router, Some("paged-subject")).await;
    for index in 0..2 {
        create_settled(router, Some(&format!("paged-other-{index}"))).await;
    }

    for index in 0..3 {
        let (status, body) = call(
            router,
            "CreateEnvironmentHost",
            &json!({
                "environmentId": subject,
                "host": {
                    "hostName": format!("esx-{index}"),
                    "keyName": "evs-key",
                    "instanceType": "i4i.metal",
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "host {index}: expected HTTP 200, got {status} ({}) {body}",
            error_type(&body)
        );
    }

    // One connector of each type: only one LIVE connector of a type can exist in an
    // environment (`tests/all/connectors.rs` owns that rule).
    for connector_type in CONNECTOR_TYPES {
        connector_in(router, "the paged connectors", &subject, connector_type).await;
    }
    let vcenter = {
        let (status, body) = call(
            router,
            "ListEnvironmentConnectors",
            &json!({ "environmentId": subject }),
        )
        .await;
        assert_eq!(status, 200, "the connector list: got {status} {body}");
        body["connectors"]
            .as_array()
            .unwrap_or_else(|| panic!("the connector list: no connectors in {body}"))
            .iter()
            .find(|connector| connector["type"] == json!(VCENTER))
            .and_then(|connector| connector["connectorId"].as_str())
            .unwrap_or_else(|| panic!("the connector list: no {VCENTER} connector in {body}"))
            .to_string()
    };

    let (status, body) = call(
        router,
        "CreateEntitlement",
        &json!({
            "environmentId": subject,
            "connectorId": vcenter,
            "entitlementType": WINDOWS_SERVER,
            "vmIds": ["vm-1", "vm-2", "vm-3"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the entitlements: expected HTTP 200, got {status} ({}) {body}",
        error_type(&body)
    );

    vec![
        Listing {
            operation: "ListEnvironments",
            member: "environmentSummaries",
            request: json!({}),
            total: 3,
        },
        Listing {
            operation: "ListEnvironmentHosts",
            member: "environmentHosts",
            request: json!({ "environmentId": subject }),
            total: 3,
        },
        Listing {
            operation: "ListEnvironmentConnectors",
            member: "connectors",
            request: json!({ "environmentId": subject }),
            total: CONNECTOR_TYPES.len(),
        },
        Listing {
            operation: "ListEnvironmentVlans",
            member: "environmentVlans",
            // The ten VLAN subnets `InitialVlans` names, one per member.
            request: json!({ "environmentId": subject }),
            total: 10,
        },
        Listing {
            operation: "ListVmEntitlements",
            member: "entitlements",
            request: json!({
                "environmentId": subject,
                "connectorId": vcenter,
                "entitlementType": WINDOWS_SERVER,
            }),
            total: 3,
        },
    ]
}

#[tokio::test]
async fn every_paginated_operation_bounds_its_page_by_the_max_results_the_request_named() {
    let router = emulator();
    let listings = populated(&router).await;

    for listing in &listings {
        for max_results in [
            None,
            Some(MAX_RESULTS_FLOOR),
            Some(2),
            Some(3),
            Some(4),
            Some(MAX_RESULTS_CEILING),
        ] {
            let case = case_of(listing, max_results);
            let bound = max_results.unwrap_or(MAX_RESULTS_CEILING) as usize;
            let (entries, page_lengths) = walk(&router, listing, max_results).await;

            let expected_pages = listing.total.div_ceil(bound).max(1);
            assert_eq!(
                page_lengths.len(),
                expected_pages,
                "case {case}: a bound of {bound} spans the collection in \
                 {expected_pages} pages, got {page_lengths:?}"
            );
            for (index, length) in page_lengths.iter().enumerate() {
                let expected = if index + 1 < page_lengths.len() {
                    bound
                } else {
                    listing.total.saturating_sub(bound * index)
                };
                assert_eq!(
                    *length, expected,
                    "case {case}: page {index} must report {expected} items, \
                     got {length} (pages {page_lengths:?})"
                );
            }
            let distinct: BTreeSet<&String> = entries.iter().collect();
            assert_eq!(
                (distinct.len(), entries.len()),
                (listing.total, listing.total),
                "case {case}: the pages together must report every item exactly once, \
                 got {entries:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused_by_every_paginated_operation() {
    let router = emulator();
    let listings = populated(&router).await;

    // Each published bound of the modelled 1..100 range, and the value one step outside
    // it on each side.
    let rows: [(i64, u16); 4] = [
        (MAX_RESULTS_FLOOR - 1, 400),
        (MAX_RESULTS_FLOOR, 200),
        (MAX_RESULTS_CEILING, 200),
        (MAX_RESULTS_CEILING + 1, 400),
    ];
    for listing in &listings {
        for (max_results, expected_status) in rows {
            let case = case_of(listing, Some(max_results));
            let request = request_for(listing, Some(max_results), None);
            let (status, body) = call(&router, listing.operation, &request).await;
            assert_eq!(
                status, expected_status,
                "case {case}: expected HTTP {expected_status}, got {status} ({}) {body}",
                error_type(&body)
            );
            if expected_status == 400 {
                assert!(
                    error_type(&body).contains("ValidationException"),
                    "case {case}: expected a ValidationException, got {} in {body}",
                    error_type(&body)
                );
                assert_eq!(
                    body[listing.member],
                    Value::Null,
                    "case {case}: a refused request reports no page, got {}",
                    body[listing.member]
                );
            } else {
                let page = body[listing.member]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case}: no {} in {body}", listing.member));
                assert_eq!(
                    page.len(),
                    listing.total.min(max_results as usize),
                    "case {case}: a page is bounded by the smaller of the bound and the \
                     collection, got {}",
                    page.len()
                );
            }
        }
    }
}

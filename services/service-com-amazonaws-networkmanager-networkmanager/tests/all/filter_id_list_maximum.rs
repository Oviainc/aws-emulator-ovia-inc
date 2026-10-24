// SPDX-License-Identifier: CC0-1.0

//! The documented maximum on the id-list filters of this service's reporting operations,
//! driven over HTTP.
//!
//! The rule under test: an id-list filter whose member documents "The maximum is 10" is
//! honoured up to that many ids and REFUSED past it with the modelled
//! `ValidationException` (HTTP 400) naming that member, before any listing is computed.
//! The model expresses the bound nowhere — `GlobalNetworkIdList`, `SiteIdList`,
//! `LinkIdList` and `DeviceIdList` carry a length on their member STRING only — so the
//! rule holds for every such filter, not only the one the issue cited.
//!
//! The table below spans all four filters this crate's implemented listings take, with
//! rows differing along the NUMBER of ids the filter carries: none, a few, the bound
//! itself, one past it and far past it, plus a row whose values repeat (the bound counts
//! the values the list carries, not the distinct resources they name).
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// The documented maximum number of ids one of these filters may name: "The IDs of one or
/// more global networks. The maximum is 10."
/// (`smithy_query query="doc" shape="DescribeGlobalNetworksRequest"
/// member="GlobalNetworkIds"`), and the same sentence on `GetSitesRequest$SiteIds`,
/// `GetLinksRequest$LinkIds` and `GetDevicesRequest$DeviceIds`.
const MAX_FILTER_IDS: usize = 10;

/// How many resources of each kind the fixture creates: two more than the bound, so a row
/// at the bound, a row one past it and a row past it with distinct ids are all expressible.
const FIXTURE_COUNT: usize = MAX_FILTER_IDS + 2;

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

/// One row of the table: how many filter values the request carries, and whether the
/// values repeat.
struct Row {
    name: &'static str,
    /// `None` for a request that omits the filter entirely.
    count: Option<usize>,
    /// Whether the row cycles the available ids to reach `count` (so values repeat).
    repeating: bool,
}

/// The rows, spanning the whole class the rule generalises over: the filter absent, well
/// inside the bound, exactly at it, one past it, far past it, and past it with repeated
/// values.
const ROWS: [Row; 8] = [
    Row {
        name: "filter absent",
        count: None,
        repeating: false,
    },
    Row {
        name: "one id",
        count: Some(1),
        repeating: false,
    },
    Row {
        name: "two ids",
        count: Some(2),
        repeating: false,
    },
    Row {
        name: "one below the maximum",
        count: Some(MAX_FILTER_IDS - 1),
        repeating: false,
    },
    Row {
        name: "exactly the maximum",
        count: Some(MAX_FILTER_IDS),
        repeating: false,
    },
    Row {
        name: "one past the maximum",
        count: Some(MAX_FILTER_IDS + 1),
        repeating: false,
    },
    Row {
        name: "two past the maximum",
        count: Some(MAX_FILTER_IDS + 2),
        repeating: false,
    },
    Row {
        name: "far past the maximum, with repeated ids",
        count: Some(MAX_FILTER_IDS * 3),
        repeating: true,
    },
];

impl Row {
    /// The ids this row sends, taken from `available`: a prefix of it, or — when the row
    /// repeats — as many values as the row asks for, cycling through it.
    fn values(&self, available: &[String]) -> Vec<String> {
        let count = self.count.unwrap_or(0);
        if self.repeating {
            available
                .iter()
                .cycle()
                .take(count)
                .map(String::clone)
                .collect()
        } else {
            assert!(
                count <= available.len(),
                "case {}: the fixture holds {} ids, too few for {count}",
                self.name,
                available.len()
            );
            available[..count].to_vec()
        }
    }
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

/// Create the fixture — [`FIXTURE_COUNT`] global networks, and that many sites, links and
/// devices inside the first of them — and describe each filter over it.
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
            let case = row.name;
            let values = row.values(&filter.ids);
            let uri = uri_for(filter, &values);
            let (status, body) = emulator.get(&uri).await;

            if values.len() > MAX_FILTER_IDS {
                // Past the documented maximum: the modelled ValidationException, naming the
                // member that carried the list, and no page at all.
                assert_eq!(
                    status, 400,
                    "case {operation}/{case} ({} ids): expected 400, got {status} with {body}",
                    values.len()
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {operation}/{case}: expected ValidationException, got {body}"
                );
                assert_eq!(
                    body["Reason"],
                    json!("FieldValidationFailed"),
                    "case {operation}/{case}: expected a field-level Reason, got {body}"
                );
                assert_eq!(
                    body["Fields"][0]["Name"],
                    json!(member),
                    "case {operation}/{case}: the refusal must name {member}, got {body}"
                );
                assert!(
                    body["Message"]
                        .as_str()
                        .is_some_and(|text| text.contains(member)
                            && text.contains(&MAX_FILTER_IDS.to_string())),
                    "case {operation}/{case}: the message must name {member} and the maximum, got {body}"
                );
                assert!(
                    body[filter.list].is_null(),
                    "case {operation}/{case}: a refused request must carry no page, got {body}"
                );
                continue;
            }

            // At or below the maximum: the listing is computed and filtered by the ids the
            // request named. An absent filter matches everything.
            assert_eq!(
                status, 200,
                "case {operation}/{case} ({} ids): expected 200, got {status} with {body}",
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
                "case {operation}/{case}: expected exactly {expected:?}, got {body}"
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
    let unknown: Vec<String> = (0..=MAX_FILTER_IDS)
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

    // The same request inside the bound is answered normally — ids that name nothing
    // simply match nothing — so the refusal above is the length, not the ids.
    let query: Vec<String> = unknown
        .iter()
        .take(MAX_FILTER_IDS)
        .map(|id| format!("globalNetworkIds={id}"))
        .collect();
    let (status, body) = emulator
        .get(&format!("/global-networks?{}", query.join("&")))
        .await;
    assert_eq!(
        status, 200,
        "case unknown ids inside the maximum: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["GlobalNetworks"],
        json!([]),
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

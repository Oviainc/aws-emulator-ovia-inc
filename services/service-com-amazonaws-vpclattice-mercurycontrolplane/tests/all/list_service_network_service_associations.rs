// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListServiceNetworkServiceAssociations`.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through
//! the router that mounts the service — and compares the listing against what was
//! actually persisted by `CreateServiceNetworkServiceAssociation` and read back by
//! `GetServiceNetworkServiceAssociation`, so nothing here can pass on a canned body.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. The listing reports exactly the persisted associations that match every filter
//!    the request sends, in either modelled identifier form (bare id or ARN), and each
//!    summary carries that association's persisted members.
//! 2. One stable order and a round-tripping `nextToken`: every page size reports each
//!    matching association exactly once.
//! 3. The listing follows state — an association deleted after it was created is gone
//!    from the next listing, and a filter naming nothing persisted is an empty page.
//! 4. A request carrying neither identifier, or a `nextToken` this service never
//!    minted, is refused with the modelled `ValidationException` (400).

use crate::vpclattice_service_slice::{expected_arn, is_lattice_id, is_timestamp, Answer, Lattice};
use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

/// The listing's path, with the filters this case sends. The identifier values carry
/// `:` and `/` (an ARN form), which a query string admits unescaped.
fn listing_path(filters: &[(&str, String)]) -> String {
    if filters.is_empty() {
        return "/servicenetworkserviceassociations".to_string();
    }
    let query: Vec<String> = filters
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("/servicenetworkserviceassociations?{}", query.join("&"))
}

/// One page of the listing.
async fn list_page(lattice: &Lattice, filters: &[(&str, String)]) -> Answer {
    lattice.call("GET", &listing_path(filters), None).await
}

/// Every association id the listing reports for `filters`, paging with `max_results`
/// until the service stops handing back a `nextToken`.
async fn list_every_association_id(
    lattice: &Lattice,
    filters: &[(&str, String)],
    max_results: Option<u32>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One page per persisted association is the worst case; the bound stops a broken
    // token round-trip from looping forever instead of failing.
    for _ in 0..64 {
        let mut page_filters: Vec<(&str, String)> = filters.to_vec();
        if let Some(max) = max_results {
            page_filters.push(("maxResults", max.to_string()));
        }
        if let Some(next) = &token {
            page_filters.push(("nextToken", next.clone()));
        }
        let answer = list_page(lattice, &page_filters).await;
        assert_eq!(
            answer.status, 200,
            "case filters={filters:?} maxResults={max_results:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let items = answer.body["items"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case filters={filters:?}: the required member items is missing from {}",
                    answer.body
                )
            });
        if let Some(max) = max_results {
            assert!(
                items.len() <= max as usize,
                "case filters={filters:?} maxResults={max}: a page carried {} items",
                items.len()
            );
        }
        for item in &items {
            ids.push(
                item["id"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("a ServiceNetworkServiceAssociationSummary without an id: {item}")
                    })
                    .to_string(),
            );
        }
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!(
        "case filters={filters:?}: the listing never stopped paging, so nextToken does not \
         round-trip to a last page"
    );
}

/// A topology of persisted resources: two service networks and three services, with
/// four associations between them, so every filter has both a match and a non-match.
struct Topology {
    /// (network name, network id)
    networks: Vec<(String, String)>,
    /// (service name, service id)
    services: Vec<(String, String)>,
    /// (association id, network id, service id)
    associations: Vec<(String, String, String)>,
}

async fn build_topology(lattice: &Lattice) -> Topology {
    let mut networks = Vec::new();
    for name in ["list-net-a", "list-net-b"] {
        let created = lattice
            .create_service_network(serde_json::json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 201,
            "case create network {name}: expected 201, got {} / {}",
            created.status, created.body
        );
        networks.push((name.to_string(), created.str_field("id")));
    }

    let mut services = Vec::new();
    for name in ["list-svc-one", "list-svc-two", "list-svc-three"] {
        let created = lattice
            .create_service(serde_json::json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 201,
            "case create service {name}: expected 201, got {} / {}",
            created.status, created.body
        );
        services.push((name.to_string(), created.str_field("id")));
    }

    // network a is associated with every service; network b only with the first one.
    let pairs = [(0_usize, 0_usize), (0, 1), (0, 2), (1, 0)];
    let mut associations = Vec::new();
    for (network, service) in pairs {
        let created = lattice
            .associate(&services[service].1, &networks[network].1)
            .await;
        assert_eq!(
            created.status, 200,
            "case associate {} with {}: expected 200, got {} / {}",
            services[service].0, networks[network].0, created.status, created.body
        );
        associations.push((
            created.str_field("id"),
            networks[network].1.clone(),
            services[service].1.clone(),
        ));
    }

    Topology {
        networks,
        services,
        associations,
    }
}

/// One row of the filter table: the case name, the filters the request sends, and the
/// association ids state says must come back.
type FilterCase = (&'static str, Vec<(&'static str, String)>, Vec<String>);

/// Rule 1: each filter selects exactly the persisted associations that match it, in
/// either modelled identifier form.
#[tokio::test]
async fn the_listing_reports_exactly_the_associations_matching_every_filter() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    let network_a = &topology.networks[0].1;
    let network_b = &topology.networks[1].1;
    let service_one = &topology.services[0].1;
    let service_three = &topology.services[2].1;

    let ids_of = |network: Option<&str>, service: Option<&str>| -> Vec<String> {
        let mut matching: Vec<String> = topology
            .associations
            .iter()
            .filter(|(_, net, svc)| {
                network.is_none_or(|wanted| wanted == net.as_str())
                    && service.is_none_or(|wanted| wanted == svc.as_str())
            })
            .map(|(id, _, _)| id.clone())
            .collect();
        matching.sort();
        matching
    };

    // (case, filters sent, the association ids state says must come back)
    let cases: Vec<FilterCase> = vec![
        (
            "by service network id",
            vec![("serviceNetworkIdentifier", network_a.clone())],
            ids_of(Some(network_a), None),
        ),
        (
            "by the other service network id",
            vec![("serviceNetworkIdentifier", network_b.clone())],
            ids_of(Some(network_b), None),
        ),
        (
            "by service network ARN",
            vec![(
                "serviceNetworkIdentifier",
                expected_arn(state::KIND_SERVICE_NETWORK, network_a),
            )],
            ids_of(Some(network_a), None),
        ),
        (
            "by service id",
            vec![("serviceIdentifier", service_one.clone())],
            ids_of(None, Some(service_one)),
        ),
        (
            "by a service associated with one network only",
            vec![("serviceIdentifier", service_three.clone())],
            ids_of(None, Some(service_three)),
        ),
        (
            "by service ARN",
            vec![(
                "serviceIdentifier",
                expected_arn(state::KIND_SERVICE, service_one),
            )],
            ids_of(None, Some(service_one)),
        ),
        (
            "by both identifiers, one pair",
            vec![
                ("serviceNetworkIdentifier", network_b.clone()),
                ("serviceIdentifier", service_one.clone()),
            ],
            ids_of(Some(network_b), Some(service_one)),
        ),
        (
            "by both identifiers, a pair that was never associated",
            vec![
                ("serviceNetworkIdentifier", network_b.clone()),
                ("serviceIdentifier", service_three.clone()),
            ],
            Vec::new(),
        ),
        (
            "by a well-formed identifier naming nothing persisted",
            vec![(
                "serviceNetworkIdentifier",
                "sn-00000000000000000".to_string(),
            )],
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in cases {
        let mut reported = list_every_association_id(&lattice, &filters, None).await;
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the associations {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 1's summary contract: every member of a reported summary is the persisted value
/// `GetServiceNetworkServiceAssociation` answers with.
#[tokio::test]
async fn each_summary_carries_the_persisted_association() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    for (association_id, network_id, service_id) in &topology.associations {
        let read = lattice
            .call(
                "GET",
                &format!("/servicenetworkserviceassociations/{association_id}"),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case get {association_id}: expected 200, got {} / {}",
            read.status, read.body
        );

        let listed = list_page(
            &lattice,
            &[
                ("serviceNetworkIdentifier", network_id.clone()),
                ("serviceIdentifier", service_id.clone()),
            ],
        )
        .await;
        let items = listed.body["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case list the pair of {association_id}: expected exactly one summary, got {}",
            listed.body
        );
        let summary = &items[0];

        // Every member the summary shape declares, against the record behind it.
        for member in [
            "id",
            "arn",
            "status",
            "createdBy",
            "serviceId",
            "serviceName",
            "serviceArn",
            "serviceNetworkId",
            "serviceNetworkName",
            "serviceNetworkArn",
        ] {
            assert_eq!(
                summary[member], read.body[member],
                "case summary member {member:?} of {association_id}: expected the persisted \
                 {:?}, got {:?}",
                read.body[member], summary[member]
            );
        }
        assert_eq!(
            summary["dnsEntry"], read.body["dnsEntry"],
            "case dnsEntry of {association_id}: expected the persisted {:?}, got {:?}",
            read.body["dnsEntry"], summary["dnsEntry"]
        );
        assert!(
            is_timestamp(&summary["createdAt"]),
            "case createdAt of {association_id}: expected an instant, got {:?}",
            summary["createdAt"]
        );
        assert!(
            is_lattice_id(
                summary["id"].as_str().unwrap_or_default(),
                state::PREFIX_SERVICE_ASSOCIATION
            ),
            "case id of {association_id}: expected a modelled snsa- id, got {:?}",
            summary["id"]
        );
        assert_eq!(
            summary["arn"].as_str().unwrap_or_default(),
            expected_arn(state::KIND_SERVICE_ASSOCIATION, association_id),
            "case arn of {association_id}: expected the ARN of the request's region, got {:?}",
            summary["arn"]
        );
        assert!(
            summary["dnsEntry"]["domainName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case dnsEntry.domainName of {association_id}: expected the service's DNS name, \
             got {:?}",
            summary["dnsEntry"]
        );
    }
}

/// Rule 2: every page size reports each matching association exactly once, in one
/// stable order, and `nextToken` pages the rest.
#[tokio::test]
async fn every_page_size_reports_each_association_exactly_once() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;
    let network_a = topology.networks[0].1.clone();
    let filters = vec![("serviceNetworkIdentifier", network_a.clone())];

    let unpaged = list_every_association_id(&lattice, &filters, None).await;
    assert_eq!(
        unpaged.len(),
        3,
        "case unpaged: expected the three associations of the network, got {unpaged:?}"
    );

    // Each row is a different page size — including one, one that divides the result
    // set exactly, the whole set, and more than the whole set.
    for max_results in [1_u32, 2, 3, 4, 100] {
        let paged = list_every_association_id(&lattice, &filters, Some(max_results)).await;
        assert_eq!(
            paged, unpaged,
            "case maxResults={max_results}: expected the same associations in the same order \
             as the unpaged listing {unpaged:?}, got {paged:?}"
        );
    }

    // The first page of a one-item listing hands back a token, and the token is opaque:
    // it is not one of the ids it pages past.
    let first = list_page(
        &lattice,
        &[
            ("serviceNetworkIdentifier", network_a),
            ("maxResults", "1".to_string()),
        ],
    )
    .await;
    let token = first.body["nextToken"].as_str().unwrap_or_else(|| {
        panic!(
            "case maxResults=1 of three associations: expected a nextToken, got {}",
            first.body
        )
    });
    assert!(
        !unpaged.iter().any(|id| id == token),
        "case the cursor is opaque: expected a token that is not an association id, got {token:?}"
    );
}

/// Rule 3: the listing follows state — deleting an association clears it from the next
/// listing, and the remaining ones are untouched.
#[tokio::test]
async fn deleting_an_association_clears_it_from_the_listing() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;
    let network_a = topology.networks[0].1.clone();
    let filters = vec![("serviceNetworkIdentifier", network_a)];

    let mut remaining = list_every_association_id(&lattice, &filters, None).await;
    assert_eq!(
        remaining.len(),
        3,
        "case before any delete: expected three associations, got {remaining:?}"
    );

    while let Some(removed) = remaining.pop() {
        let deleted = lattice
            .call(
                "DELETE",
                &format!("/servicenetworkserviceassociations/{removed}"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case delete {removed}: expected 200, got {} / {}",
            deleted.status, deleted.body
        );
        let mut after = list_every_association_id(&lattice, &filters, None).await;
        after.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            after, expected,
            "case after deleting {removed}: expected the listing to report {expected:?}, \
             got {after:?}"
        );
    }

    // Everything of that network is gone, and the required `items` member is still there.
    let empty = list_page(&lattice, &filters).await;
    assert_eq!(
        empty.status, 200,
        "case the emptied listing: expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body["items"],
        serde_json::json!([]),
        "case the emptied listing: expected an empty items list, got {}",
        empty.body
    );
    assert!(
        empty.body["nextToken"].is_null(),
        "case the emptied listing: expected no nextToken, got {}",
        empty.body
    );
}

/// Rule 4: a request this operation refuses is the modelled `ValidationException`
/// (HTTP 400), never a 501 and never a silent whole-store listing.
#[tokio::test]
async fn a_refused_request_is_the_modelled_validation_exception() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;
    let network_a = topology.networks[0].1.clone();

    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("neither identifier", Vec::new()),
        (
            "a nextToken this service never minted",
            vec![
                ("serviceNetworkIdentifier", network_a.clone()),
                ("nextToken", "not-a-cursor".to_string()),
            ],
        ),
        (
            "a nextToken that is hex but names no page",
            vec![
                ("serviceNetworkIdentifier", network_a),
                ("nextToken", "6162636465".to_string()),
            ],
        ),
    ];

    for (case, filters) in cases {
        let answer = list_page(&lattice, &filters).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected ValidationException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }
}

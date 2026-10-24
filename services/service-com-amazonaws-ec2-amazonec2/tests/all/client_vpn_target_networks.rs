// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeClientVpnTargetNetworks`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeClientVpnTargetNetworks`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE for ONE endpoint: it answers
//! the target network associations the store holds for the `ClientVpnEndpointId` the request
//! names — narrowed by the `AssociationIds` and `Filters` it names and cut into
//! `MaxResults`-sized pages — refusing an endpoint id that is not an endpoint id at all or
//! that this account holds no endpoint for, and reporting an empty set for an endpoint that
//! holds no association. Every table row is a distinct input along what that generalises
//! over: which endpoint, which filter, how many values, how many filters, which ids, which
//! page size, and which form of unresolvable id.

use std::time::Duration;

use ec2::ops::types::{ClientVpnEndpoint, TargetNetwork};
use ec2::state::{
    ClientVpnTargetNetworkSpec as TargetNetworkSpec, ComAmazonawsEc2Amazonec2State as Ec2State,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed};

/// The Region every call here is signed for. The endpoint and association stores are per
/// account rather than per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DescribeClientVpnTargetNetworks";

/// The XML element each reported association's id arrives in
/// (`TargetNetwork$AssociationId`'s `xmlName`).
const ID_ELEMENT: &str = "associationId";

/// The ec2Query form field the `AssociationIds` list is flattened into:
/// `DescribeClientVpnTargetNetworksRequest$AssociationIds` carries no `@xmlName` or
/// `@ec2QueryName`, so the key is the member name itself (`AssociationIds.1`, `.2`, …),
/// unlike `Filters`, which the model renames to `Filter`.
const ASSOCIATION_IDS_PARAM: &str = "AssociationIds";

/// Long enough past [`TRANSITION_DELAY`] that the `associating` -> `associated` move the
/// clock owes an association is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// One `ClientVpnEndpointId` parameter — the operation's one required member, so every call
/// below carries it.
fn endpoint_param(endpoint_id: &str) -> Vec<(String, String)> {
    vec![(
        "ClientVpnEndpointId".to_string(),
        endpoint_id.to_string(),
    )]
}

/// `endpoint_param` plus `extra`, the form every table row sends.
fn params_for(endpoint_id: &str, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut params = endpoint_param(endpoint_id);
    params.extend(extra.iter().cloned());
    params
}

/// The association ids a successful call for `endpoint_id` reports, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_id: &str,
    extra: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, &params_for(endpoint_id, extra)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// What one seeded association joins: the subnet that is the target network, the VPC that
/// subnet is in, and the security groups applied to the association.
struct AssociationRow {
    target_network_id: &'static str,
    vpc_id: &'static str,
    security_groups: &'static [&'static str],
}

/// The associations of the endpoint the filter, id and paging tables are written against.
/// They differ along every attribute the documented filters select on — the subnet and the
/// VPC — and along how many security groups the association carries: two pairs share a VPC,
/// so a filter on it reports more than a single row, and seven rows are more than the
/// model's minimum page size, so paging cuts them.
fn endpoint_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            target_network_id: "subnet-00000000000000001",
            vpc_id: "vpc-00000000000000001",
            security_groups: &["sg-00000000000000001"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000002",
            vpc_id: "vpc-00000000000000001",
            security_groups: &["sg-00000000000000001", "sg-00000000000000002"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000003",
            vpc_id: "vpc-00000000000000002",
            security_groups: &[],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000004",
            vpc_id: "vpc-00000000000000002",
            security_groups: &["sg-00000000000000003"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000005",
            vpc_id: "vpc-00000000000000003",
            security_groups: &["sg-00000000000000004"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000006",
            vpc_id: "vpc-00000000000000003",
            security_groups: &["sg-00000000000000004", "sg-00000000000000005"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000007",
            vpc_id: "vpc-00000000000000004",
            security_groups: &["sg-00000000000000006"],
        },
    ]
}

/// The associations of the OTHER endpoint: the same subnet and VPC as the first row above,
/// so a report scoped to one endpoint can only be right because it reads the association's
/// `ClientVpnEndpointId` rather than its subnet or VPC.
fn other_endpoint_rows() -> Vec<AssociationRow> {
    vec![
        AssociationRow {
            target_network_id: "subnet-00000000000000001",
            vpc_id: "vpc-00000000000000001",
            security_groups: &["sg-00000000000000001"],
        },
        AssociationRow {
            target_network_id: "subnet-00000000000000008",
            vpc_id: "vpc-00000000000000005",
            security_groups: &[],
        },
    ]
}

/// The two endpoints the suite describes and the association ids each holds, in store order.
struct Seeded {
    endpoint: String,
    other_endpoint: String,
    ids: Vec<String>,
    other_ids: Vec<String>,
}

/// Write one Client VPN endpoint, the way `CreateClientVpnEndpoint` would, and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, description: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(description.to_string()),
            client_cidr_block: Some("10.100.0.0/22".to_string()),
            ..Default::default()
        })
        .expect("the store must take the Client VPN endpoint")
}

/// Associate `rows` with `endpoint_id`, the way `AssociateClientVpnTargetNetwork` would, and
/// answer the association ids they were minted with in the order they were written.
fn seed_associations(
    engine: &engine::InMemoryEngine,
    endpoint_id: &str,
    rows: &[AssociationRow],
) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    rows.iter()
        .map(|row| {
            let association = guard
                .insert_client_vpn_target_network(TargetNetworkSpec {
                    client_vpn_endpoint_id: endpoint_id.to_string(),
                    target_network_id: row.target_network_id.to_string(),
                    vpc_id: row.vpc_id.to_string(),
                    security_groups: row
                        .security_groups
                        .iter()
                        .map(|group| (*group).to_string())
                        .collect(),
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the store must take the association",
                        row.target_network_id
                    )
                });
            association
                .association_id
                .expect("a stored association carries the id it was minted with")
        })
        .collect()
}

/// Both endpoints with their associations, as every table below reads them.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let endpoint = seed_endpoint(engine, "the endpoint under test");
    let other_endpoint = seed_endpoint(engine, "another endpoint of the same account");
    let ids = seed_associations(engine, &endpoint, &endpoint_rows());
    let other_ids = seed_associations(engine, &other_endpoint, &other_endpoint_rows());
    Seeded {
        endpoint,
        other_endpoint,
        ids,
        other_ids,
    }
}

/// The ids at `indices` of `ids`, in ascending index order — the order the store reports in,
/// whatever order a request asked for them.
fn ids_at(ids: &[String], indices: &[usize]) -> Vec<String> {
    let mut selected: Vec<usize> = indices.to_vec();
    selected.sort_unstable();
    selected
        .iter()
        .map(|index| ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An endpoint with no target network associated reports HTTP 200 with an empty set — which
/// is a different answer from the refusal an unknown endpoint id gets — and describing is a
/// READ: two calls report the same thing and nothing is written.
#[tokio::test]
async fn an_endpoint_with_no_target_network_reports_an_empty_set_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "an endpoint with nothing associated");

    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "unassociated endpoint: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "unassociated endpoint: expected no association, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "unassociated endpoint: a complete report carries no NextToken, got {body}"
    );

    let second = described_ids(&router, "second read", &endpoint, &[]).await;
    assert_eq!(
        second, reported,
        "a read must not change what a later caller sees: {reported:?} then {second:?}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .client_vpn_target_networks
            .len(),
        0,
        "describing target networks must not write an association to the store"
    );
}

/// Every association the store holds FOR THE ENDPOINT NAMED is reported, in store order,
/// with every member of the modelled shape — and the other endpoint's associations are not,
/// even the one on the same subnet and VPC. An association written after a call shows up in
/// the next one, so the answer is computed from the store rather than fixed.
#[tokio::test]
async fn every_association_of_the_named_endpoint_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    // Past the move the clock owes each association, so the status reported below is the
    // settled one rather than whichever side of the span the call landed on.
    tokio::time::sleep(past_the_transition()).await;

    let (status, body) = describe(&router, &endpoint_param(&seeded.endpoint)).await;
    assert_eq!(
        status, 200,
        "all associations: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert_eq!(
        reported, seeded.ids,
        "all associations: expected every association of the endpoint in store order, got \
         {reported:?} from {body}"
    );
    for id in &seeded.other_ids {
        assert!(
            !reported.contains(id),
            "all associations: {id:?} belongs to another endpoint and must not be reported, \
             got {reported:?}"
        );
    }

    // Every member the modelled `TargetNetwork` carries a value for, read back off the wire.
    assert_eq!(
        elements(&body, "clientVpnEndpointId"),
        vec![seeded.endpoint.clone(); seeded.ids.len()],
        "all associations: each row names the endpoint it belongs to, got {body}"
    );
    let rows = endpoint_rows();
    assert_eq!(
        elements(&body, "targetNetworkId"),
        rows.iter()
            .map(|row| row.target_network_id.to_string())
            .collect::<Vec<String>>(),
        "all associations: each row names its target network, got {body}"
    );
    assert_eq!(
        elements(&body, "vpcId"),
        rows.iter()
            .map(|row| row.vpc_id.to_string())
            .collect::<Vec<String>>(),
        "all associations: each row names the VPC its target network is in, got {body}"
    );
    assert_eq!(
        elements(&body, "code"),
        vec!["associated".to_string(); seeded.ids.len()],
        "all associations: each settled row reports the associated status, got {body}"
    );
    for row in &rows {
        for group in row.security_groups {
            assert!(
                body.contains(&format!("<item>{group}</item>")),
                "case {:?}: expected the security group {group:?} to be reported, got {body}",
                row.target_network_id
            );
        }
    }

    let late = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_target_network(TargetNetworkSpec {
            client_vpn_endpoint_id: seeded.endpoint.clone(),
            target_network_id: "subnet-00000000000000009".to_string(),
            vpc_id: "vpc-00000000000000009".to_string(),
            security_groups: Vec::new(),
        })
        .and_then(|association| association.association_id)
        .expect("after a later write: the store must take the association");
    let after = described_ids(&router, "after a later write", &seeded.endpoint, &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late:?} to be reported too, got {after:?}"
    );
}

/// Each endpoint reports only its OWN associations, whichever endpoint is asked about.
#[tokio::test]
async fn the_report_is_scoped_to_the_endpoint_the_request_names() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases = [
        ("the endpoint under test", &seeded.endpoint, &seeded.ids),
        ("the other endpoint", &seeded.other_endpoint, &seeded.other_ids),
    ];
    for (case, endpoint, expected) in cases {
        let reported = described_ids(&router, case, endpoint, &[]).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The `associating` an association starts in is what a caller describing it straight after
/// the association sees; the `associated` the clock owes it is what a caller sees once the
/// due instant has passed, and it stays there.
#[tokio::test]
async fn the_status_moves_from_associating_to_associated_once_the_clock_owes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "an endpoint whose association is settling");
    let rows = endpoint_rows();
    let seeded = seed_associations(&engine, &endpoint, &rows[..2]);

    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "straight after associating: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "code"),
        vec!["associating".to_string(); seeded.len()],
        "straight after associating: expected every association to still be associating, got \
         {body}"
    );

    tokio::time::sleep(past_the_transition()).await;
    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "past the transition: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "code"),
        vec!["associated".to_string(); seeded.len()],
        "past the transition: expected every association to be associated, got {body}"
    );

    let (status, again) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "after the transition: expected HTTP 200, got {status} with body {again}"
    );
    assert_eq!(
        elements(&again, "code"),
        elements(&body, "code"),
        "a settled status must not move again: {body} then {again}"
    );
}

// ---------------------------------------------------------------------------
// The endpoint id
// ---------------------------------------------------------------------------

/// A `ClientVpnEndpointId` this service cannot resolve is refused, and the refusal says
/// WHICH of the two problems it is: a value that is not an endpoint id at all answers
/// `InvalidClientVpnEndpointId.Malformed`, a well-formed id this account holds no endpoint
/// for answers `InvalidClientVpnEndpointId.NotFound` — never an empty set, which is the
/// answer for an endpoint that exists and holds no association.
#[tokio::test]
async fn an_endpoint_id_that_cannot_be_resolved_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the two well-formed id lengths EC2 issues (8 and 17 hex digits) and the all-zero
    // placeholder the AWS documentation itself uses, none of which names an endpoint of this
    // account; then values that are not endpoint ids at all — another resource's prefix, a
    // bare prefix, a suffix of the wrong length, a non-hex suffix, an uppercase suffix, the
    // id of an ASSOCIATION, and an empty value.
    let cases: [(&str, &str); 11] = [
        ("cvpn-endpoint-0123abcd", "InvalidClientVpnEndpointId.NotFound"),
        (
            "cvpn-endpoint-0123456789abcdef0",
            "InvalidClientVpnEndpointId.NotFound",
        ),
        (
            "cvpn-endpoint-00000000000000000",
            "InvalidClientVpnEndpointId.NotFound",
        ),
        (
            "vpc-00000000000000001",
            "InvalidClientVpnEndpointId.Malformed",
        ),
        ("cvpn-endpoint", "InvalidClientVpnEndpointId.Malformed"),
        ("cvpn-endpoint-", "InvalidClientVpnEndpointId.Malformed"),
        (
            "cvpn-endpoint-0123456789abcdef",
            "InvalidClientVpnEndpointId.Malformed",
        ),
        (
            "cvpn-endpoint-0123456789abcdefg",
            "InvalidClientVpnEndpointId.Malformed",
        ),
        (
            "cvpn-endpoint-0123456789ABCDEF0",
            "InvalidClientVpnEndpointId.Malformed",
        ),
        ("cvpn-assoc-00000000000000001", "InvalidClientVpnEndpointId.Malformed"),
        ("", "InvalidClientVpnEndpointId.Malformed"),
    ];
    for (endpoint_id, expected_code) in cases {
        let (status, body) = describe(&router, &endpoint_param(endpoint_id)).await;
        assert_eq!(
            status, 400,
            "case {endpoint_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {endpoint_id:?}: expected {expected_code}, got {body}"
        );
    }

    // The endpoint the account DOES hold is still reported, so the refusals above are about
    // the id rather than about the operation.
    let reported = described_ids(&router, "the stored endpoint", &seeded.endpoint, &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "the stored endpoint: expected {:?}, got {reported:?}",
        seeded.ids
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_associations_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = endpoint_rows();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeClientVpnTargetNetworksRequest$Filters` documents,
    // each asserted on an association the issue never cited, with one and with several
    // values, and with a value no association holds.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "association-id",
            vec![seeded.ids[4].clone()],
            vec![4],
        ),
        (
            "association-id",
            vec![seeded.ids[1].clone(), seeded.ids[6].clone()],
            vec![1, 6],
        ),
        (
            // An association of the OTHER endpoint, named by its own id: the endpoint scope
            // holds whatever the filter says.
            "association-id",
            vec![seeded.other_ids[0].clone()],
            Vec::new(),
        ),
        (
            "target-network-id",
            vec![rows[2].target_network_id.to_string()],
            vec![2],
        ),
        (
            "target-network-id",
            vec![
                rows[0].target_network_id.to_string(),
                rows[3].target_network_id.to_string(),
                rows[5].target_network_id.to_string(),
            ],
            vec![0, 3, 5],
        ),
        (
            "vpc-id",
            vec![rows[0].vpc_id.to_string()],
            vec![0, 1],
        ),
        (
            "vpc-id",
            vec![rows[4].vpc_id.to_string(), rows[6].vpc_id.to_string()],
            vec![4, 5, 6],
        ),
        (
            // Every VPC the endpoint's associations name: the filter narrows nothing.
            "vpc-id",
            rows.iter()
                .map(|row| row.vpc_id.to_string())
                .collect::<Vec<String>>(),
            all.clone(),
        ),
        (
            "vpc-id",
            vec!["vpc-00000000000000099".to_string()],
            Vec::new(),
        ),
        (
            // "Filter names and values are case-sensitive"
            // (`DescribeClientVpnTargetNetworksRequest$Filters`).
            "target-network-id",
            vec![rows[0].target_network_id.to_uppercase()],
            Vec::new(),
        ),
    ];
    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            &case,
            &seeded.endpoint,
            &filters(&[(name, &borrowed)]),
        )
        .await;
        let expected = ids_at(&seeded.ids, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the associations that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the associations matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("vpc-id", &["vpc-00000000000000001"][..]),
                ("target-network-id", &["subnet-00000000000000002"][..]),
            ],
            vec![1],
        ),
        (
            vec![
                ("vpc-id", &["vpc-00000000000000003"][..]),
                (
                    "target-network-id",
                    &["subnet-00000000000000005", "subnet-00000000000000006"][..],
                ),
            ],
            vec![4, 5],
        ),
        (
            // The two filters select disjoint sets, so their AND is empty.
            vec![
                ("vpc-id", &["vpc-00000000000000001"][..]),
                ("target-network-id", &["subnet-00000000000000007"][..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("vpc-id", &["vpc-00000000000000002"][..]),
                (
                    "target-network-id",
                    &["subnet-00000000000000003", "subnet-00000000000000004"][..],
                ),
                ("vpc-id", &["vpc-00000000000000002"][..]),
            ],
            vec![2, 3],
        ),
    ];
    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported =
            described_ids(&router, &case, &seeded.endpoint, &filters(&entries)).await;
        let expected = ids_at(&seeded.ids, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's `InvalidFilter`,
/// rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name
    // with the wrong case, the plural, a member of the described shape the `Filters` doc
    // does NOT name, the tag filters this operation documents neither of (an association
    // carries no tags), and an empty name.
    let cases = [
        "association",
        "association_id",
        "Association-Id",
        "vpc-ids",
        "client-vpn-endpoint-id",
        "status.code",
        "tag-key",
        "tag:Name",
        "",
    ];
    for name in cases {
        let (status, body) = describe(
            &router,
            &params_for(&seeded.endpoint, &filters(&[(name, &["anything"])])),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFilter",
            "case {name:?}: expected InvalidFilter, got {body}"
        );
    }
}

/// One row of the bounds table: what the case is called, the form fields to send, and the
/// HTTP status the request must answer with.
type BoundsCase = (String, Vec<(String, String)>, u16);

/// The request-controlled factors of the store scan are bounded: each of the crate's caps is
/// accepted at its published value and refused one past it, so no request can ask for
/// unbounded filtering work.
#[tokio::test]
async fn the_filter_and_id_factors_are_accepted_at_their_caps_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the number of filters, the values in one filter, the bytes in one value and the
    // number of association ids — each at the crate's cap (`ec2::state`) and one past it.
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("vpc-id", &["vpc-00000000000000001"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("vpc-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("vpc-id", &borrowed)])
    };
    let long_value = |bytes: usize| -> Vec<(String, String)> {
        let value = "v".repeat(bytes);
        filters(&[("vpc-id", &[value.as_str()])])
    };
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("cvpn-assoc-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed(ASSOCIATION_IDS_PARAM, &borrowed)
    };
    let cases: Vec<BoundsCase> = vec![
        (
            format!("{} filters", ec2::state::MAX_FILTERS),
            many_filters(ec2::state::MAX_FILTERS),
            200,
        ),
        (
            format!("{} filters", ec2::state::MAX_FILTERS + 1),
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES),
            many_values(ec2::state::MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} filter values", ec2::state::MAX_FILTER_VALUES + 1),
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {}-byte filter value", ec2::state::MAX_FILTER_VALUE_BYTES),
            long_value(ec2::state::MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!(
                "a {}-byte filter value",
                ec2::state::MAX_FILTER_VALUE_BYTES + 1
            ),
            long_value(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
        (
            format!("{} association ids", ec2::state::MAX_RESOURCE_IDS),
            many_ids(ec2::state::MAX_RESOURCE_IDS),
            200,
        ),
        (
            format!("{} association ids", ec2::state::MAX_RESOURCE_IDS + 1),
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];
    for (case, extra, expected_status) in cases {
        let (status, body) = describe(&router, &params_for(&seeded.endpoint, &extra)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// AssociationIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the associations named, in store order whatever order they
/// were asked for; an id that names no association of the endpoint contributes no row.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_associations_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![6],
        vec![2, 4],
        vec![4, 2],
        vec![5, 1, 3],
        (0..seeded.ids.len()).collect(),
    ];
    for asked in cases {
        let case = format!("AssociationIds={asked:?}");
        let ids: Vec<String> = asked.iter().map(|index| seeded.ids[*index].clone()).collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            &case,
            &seeded.endpoint,
            &indexed(ASSOCIATION_IDS_PARAM, &borrowed),
        )
        .await;
        assert_eq!(
            reported,
            ids_at(&seeded.ids, &asked),
            "case {case:?}: expected the ids named in store order, got {reported:?}"
        );
    }

    // An id that names nothing of this endpoint — one never minted, and one belonging to the
    // other endpoint — contributes no row rather than refusing the call.
    let unknown = "cvpn-assoc-00000000000000099";
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("an unminted id", vec![unknown.to_string()], Vec::new()),
        (
            "an unminted id beside a known one",
            vec![unknown.to_string(), seeded.ids[0].clone()],
            vec![0],
        ),
        (
            "another endpoint's association",
            vec![seeded.other_ids[1].clone()],
            Vec::new(),
        ),
    ];
    for (case, ids, expected_indices) in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reported = described_ids(
            &router,
            case,
            &seeded.endpoint,
            &indexed(ASSOCIATION_IDS_PARAM, &borrowed),
        )
        .await;
        let expected = ids_at(&seeded.ids, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every association of the endpoint
/// exactly once, in store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_association_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeClientVpnTargetNetworksMaxResults` is modelled `range 5..1000`, so the rows
    // are its published lower bound, a size that divides the set exactly, one that does not,
    // and the upper bound, which holds everything.
    for max_results in [5, 6, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params_for(&seeded.endpoint, &params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            walked.extend(elements(&body, ID_ELEMENT));
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match element(&body, "nextToken") {
                None => break,
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token));
                }
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every association exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the endpoint's seven associations are in these three VPCs, so the model's
    // minimum page size cuts the filtered set into two pages.
    let selected_vpcs: &[&str] = &[
        "vpc-00000000000000001",
        "vpc-00000000000000002",
        "vpc-00000000000000003",
    ];
    let page_params = || {
        let mut params = filters(&[("vpc-id", selected_vpcs)]);
        params.extend(max_results_param(5));
        params
    };

    let (status, body) = describe(&router, &params_for(&seeded.endpoint, &page_params())).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, ID_ELEMENT),
        ids_at(&seeded.ids, &[0, 1, 2, 3, 4]),
        "filtered first page: expected the first five of those VPCs, got {body}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth association follows, so a token is minted");

    let mut params = page_params();
    params.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &seeded.endpoint, &params).await;
    assert_eq!(
        second,
        ids_at(&seeded.ids, &[5]),
        "filtered second page: expected the remaining association of those VPCs, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The published bounds (`DescribeClientVpnTargetNetworksMaxResults`, `range 5..1000`)
    // and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(
            &router,
            &params_for(&seeded.endpoint, &max_results_param(max_results)),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor — and before the endpoint id is resolved, so the refusal does not
/// depend on the rest of the request.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // association id a caller might guess the token is, and that id hex-encoded (the shape
    // the token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
    ];
    for token in cases {
        let mut params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params_for(&seeded.endpoint, &params)).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );

        // The same token with an endpoint id that names nothing: the token is still what is
        // refused.
        params.push((
            "ClientVpnEndpointId".to_string(),
            "cvpn-endpoint-00000000000000000".to_string(),
        ));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?} with an unknown endpoint: expected HTTP 400, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?} with an unknown endpoint: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing, whatever endpoint id the request names; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the endpoint the account holds, and ids it does not — a permission probe is
    // answered before the endpoint is resolved, so a caller checking its permissions gets
    // the same answer whether or not the endpoint exists.
    let cases = [
        seeded.endpoint.clone(),
        "cvpn-endpoint-00000000000000000".to_string(),
        "not-an-endpoint-id".to_string(),
    ];
    for endpoint_id in cases {
        let (status, body) = describe(
            &router,
            &params_for(
                &endpoint_id,
                &[("DryRun".to_string(), "true".to_string())],
            ),
        )
        .await;
        assert_eq!(
            status, 412,
            "case {endpoint_id:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {endpoint_id:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {endpoint_id:?}: a dry run reports no association, got {body}"
        );
    }

    // `DryRun=false` is an ordinary call, and the store is exactly as it was.
    let reported = described_ids(
        &router,
        "DryRun=false",
        &seeded.endpoint,
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected {:?}, got {reported:?}",
        seeded.ids
    );
    let held: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_target_networks
        .values()
        .map(|record| {
            record
                .view
                .association_id
                .clone()
                .unwrap_or_default()
        })
        .collect();
    let mut expected = seeded.ids.clone();
    expected.extend(seeded.other_ids.clone());
    expected.sort();
    assert_eq!(
        held, expected,
        "describing must neither add nor drop an association: expected {expected:?}, got \
         {held:?}"
    );
}

/// The modelled shape a stored association is kept as is the one the wire reports, so a
/// round trip through the store loses no member: what the writer stored is what the describe
/// answers.
#[tokio::test]
async fn a_stored_association_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoint = seed_endpoint(&engine, "an endpoint with one association");
    let stored: TargetNetwork = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_target_network(TargetNetworkSpec {
            client_vpn_endpoint_id: endpoint.clone(),
            target_network_id: "subnet-0000000000000000a".to_string(),
            vpc_id: "vpc-0000000000000000a".to_string(),
            security_groups: vec!["sg-0000000000000000a".to_string()],
        })
        .expect("the store must take the association");

    let (status, body) = describe(&router, &endpoint_param(&endpoint)).await;
    assert_eq!(
        status, 200,
        "round trip: expected HTTP 200, got {status} with body {body}"
    );
    let expected: Vec<(&str, String)> = vec![
        (
            ID_ELEMENT,
            stored.association_id.clone().unwrap_or_default(),
        ),
        (
            "clientVpnEndpointId",
            stored.client_vpn_endpoint_id.clone().unwrap_or_default(),
        ),
        (
            "targetNetworkId",
            stored.target_network_id.clone().unwrap_or_default(),
        ),
        ("vpcId", stored.vpc_id.clone().unwrap_or_default()),
    ];
    for (xml_name, value) in expected {
        assert_eq!(
            element(&body, xml_name).as_deref(),
            Some(value.as_str()),
            "round trip: expected <{xml_name}>{value}</{xml_name}>, got {body}"
        );
    }
    let status_code = stored
        .status
        .as_ref()
        .and_then(|status| status.code.clone())
        .unwrap_or_default();
    assert_eq!(
        element(&body, "code").as_deref(),
        Some(status_code.as_str()),
        "round trip: expected the stored status {status_code:?}, got {body}"
    );
}

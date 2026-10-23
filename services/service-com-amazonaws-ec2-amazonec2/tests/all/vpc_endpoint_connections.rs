// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcEndpointConnections`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeVpcEndpointConnections`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the connections to the account's own VPC endpoint services, narrowed by the `Filters`
//! the request names and cut into `MaxResults`-sized pages. Every table row is a distinct
//! input along what that generalises over — which documented filter, which spelling of a
//! state, how many values, how many filters, which Region the call is made in, which page
//! size — so no single recorded request is the specification.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{DnsEntry, PayerResponsibilityEntry, STATE_VALUES, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, VPC_ENDPOINT_STATE_FILTER_VALUES,
    VpcEndpointConnectionSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, unescaped};

/// The Region the endpoint services are hosted in, which every call here is signed for
/// unless it is testing what the Region of the call itself selects.
const REGION: &str = "us-east-1";

/// An account other than the emulator's own, as the owner of a consumer endpoint: a VPC
/// endpoint connection to your service may come from any account, and
/// `VpcEndpointConnection$VpcEndpointOwner` is "the ID of the Amazon Web Services account
/// that owns the VPC endpoint".
const OTHER_ACCOUNT: &str = "210987654321";

/// The hosted zone a connection's DNS entry names. One value for every seeded connection:
/// nothing here selects on it, it is carried to prove a stored list round-trips.
const HOSTED_ZONE_ID: &str = "Z7HUB22UULQXV";

/// One `DescribeVpcEndpointConnections` call carrying `params`, signed for `region`, as its
/// HTTP status and XML body.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeVpcEndpointConnections", params).await
}

/// One call signed for the Region the services are hosted in.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    describe_in(router, REGION, params).await
}

/// The endpoint ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "vpcEndpointId")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded connections carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded connection is created with, beyond the ids the store mints itself.
struct ConnectionFixture {
    /// Which of the seeded endpoint services this connection is to.
    service_index: usize,
    /// The account that owns the endpoint.
    owner: String,
    /// The endpoint's state, as the model's `State` enum spells it.
    state: &'static str,
    /// The Region the endpoint is in.
    region: &'static str,
    /// The endpoint's IP address type, or `None` for an endpoint that carries none.
    ip_address_type: Option<&'static str>,
    /// The Network Load Balancers behind the service.
    network_load_balancer_arns: Vec<String>,
    /// The Gateway Load Balancers behind the service.
    gateway_load_balancer_arns: Vec<String>,
    tags: Vec<Tag>,
}

/// A load balancer ARN of `kind` behind the service at `service_index`.
fn load_balancer_arn(kind: &str, service_index: usize, ordinal: usize) -> String {
    engine::emulator_arn(
        "elasticloadbalancing",
        REGION,
        &format!("loadbalancer/{kind}/svc-{service_index}-{ordinal}/0123456789abcdef"),
    )
}

/// The connections the filter and paging tables are written against. They differ along
/// every attribute the documented filters select on — service, owner, state, Region and IP
/// address type — and along the shapes of the lists they carry.
fn fixtures() -> Vec<ConnectionFixture> {
    let account = engine::emulator_account_id().to_string();
    vec![
        ConnectionFixture {
            service_index: 0,
            owner: account.clone(),
            state: "Available",
            region: REGION,
            ip_address_type: Some("ipv4"),
            network_load_balancer_arns: vec![load_balancer_arn("net", 0, 0)],
            gateway_load_balancer_arns: Vec::new(),
            tags: vec![tag("Name", "web")],
        },
        ConnectionFixture {
            service_index: 0,
            owner: account.clone(),
            state: "PendingAcceptance",
            region: REGION,
            ip_address_type: Some("ipv6"),
            network_load_balancer_arns: vec![load_balancer_arn("net", 0, 0)],
            gateway_load_balancer_arns: Vec::new(),
            tags: vec![tag("Name", "batch")],
        },
        ConnectionFixture {
            service_index: 1,
            owner: account.clone(),
            state: "PendingAcceptance",
            region: REGION,
            ip_address_type: Some("dualstack"),
            network_load_balancer_arns: Vec::new(),
            gateway_load_balancer_arns: vec![load_balancer_arn("gwy", 1, 0)],
            tags: Vec::new(),
        },
        ConnectionFixture {
            service_index: 1,
            owner: account.clone(),
            state: "Rejected",
            region: "us-west-2",
            ip_address_type: Some("ipv4"),
            network_load_balancer_arns: Vec::new(),
            gateway_load_balancer_arns: vec![load_balancer_arn("gwy", 1, 0)],
            tags: Vec::new(),
        },
        ConnectionFixture {
            service_index: 1,
            owner: OTHER_ACCOUNT.to_string(),
            state: "Deleting",
            region: "eu-west-1",
            ip_address_type: None,
            network_load_balancer_arns: Vec::new(),
            gateway_load_balancer_arns: vec![load_balancer_arn("gwy", 1, 0)],
            tags: Vec::new(),
        },
        ConnectionFixture {
            service_index: 2,
            owner: OTHER_ACCOUNT.to_string(),
            state: "Available",
            region: REGION,
            ip_address_type: Some("ipv4"),
            network_load_balancer_arns: vec![
                load_balancer_arn("net", 2, 0),
                load_balancer_arn("net", 2, 1),
            ],
            gateway_load_balancer_arns: Vec::new(),
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        ConnectionFixture {
            service_index: 2,
            owner: account.clone(),
            state: "Failed",
            region: REGION,
            ip_address_type: Some("ipv6"),
            network_load_balancer_arns: vec![load_balancer_arn("net", 2, 0)],
            gateway_load_balancer_arns: Vec::new(),
            tags: Vec::new(),
        },
        ConnectionFixture {
            service_index: 2,
            owner: account,
            state: "Deleted",
            region: REGION,
            ip_address_type: Some("ipv4"),
            network_load_balancer_arns: vec![load_balancer_arn("net", 2, 0)],
            gateway_load_balancer_arns: Vec::new(),
            tags: Vec::new(),
        },
    ]
}

/// The ids the seeded connections were given.
struct Seeded {
    /// The `VpcEndpointId` of each fixture, in fixture order — which is also store order,
    /// the ids being minted in that order.
    endpoint_ids: Vec<String>,
    /// The `VpcEndpointConnectionId` the store minted for each fixture.
    connection_ids: Vec<String>,
    /// The `ServiceId` of each seeded endpoint service.
    service_ids: Vec<String>,
}

/// Write the seeded connections into the engine's store, the way an operation creating an
/// endpoint to one of the account's services would, and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let service_ids: Vec<String> = (0..3).map(|_| guard.next_id("vpce-svc")).collect();
    let mut endpoint_ids = Vec::new();
    let mut connection_ids = Vec::new();
    for (index, fixture) in fixtures().into_iter().enumerate() {
        let endpoint_id = guard.next_id("vpce");
        let service_id = service_ids[fixture.service_index].clone();
        let connection_id = guard.insert_vpc_endpoint_connection(VpcEndpointConnectionSpec {
            service_id: service_id.clone(),
            vpc_endpoint_id: endpoint_id.clone(),
            vpc_endpoint_owner: fixture.owner,
            vpc_endpoint_state: fixture.state.to_string(),
            // A fixed instant, so the reported timestamp is the one that was stored rather
            // than whatever the clock said during the test.
            creation_timestamp: SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000 + index as u64),
            vpc_endpoint_region: fixture.region.to_string(),
            ip_address_type: fixture.ip_address_type.map(str::to_string),
            dns_entries: vec![DnsEntry {
                dns_name: Some(format!(
                    "{endpoint_id}-{service_id}.{}.vpce.amazonaws.com",
                    fixture.region
                )),
                hosted_zone_id: Some(HOSTED_ZONE_ID.to_string()),
            }],
            network_load_balancer_arns: fixture.network_load_balancer_arns,
            gateway_load_balancer_arns: fixture.gateway_load_balancer_arns,
            payer_responsibilities: vec![PayerResponsibilityEntry {
                scope: Some("vpc-endpoint-charges".to_string()),
                payer_responsibility_type: Some("vpc-endpoint-account".to_string()),
            }],
            tags: fixture.tags,
        });
        endpoint_ids.push(endpoint_id);
        connection_ids.push(connection_id);
    }

    Seeded {
        endpoint_ids,
        connection_ids,
        service_ids,
    }
}

/// The endpoint ids of the fixtures at `indices`, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.endpoint_ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has created no VPC endpoint service, so it has no connection to one
/// either: the report is empty, and describing is a READ — two calls answer the same and
/// the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_no_connection_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "vpcEndpointId").is_empty(),
        "fresh account: expected no connection, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty report, so no token is due, got {body}"
    );

    let (second_status, second) = describe(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second}"
    );
    assert_eq!(
        elements(&second, "vpcEndpointId"),
        Vec::<String>::new(),
        "a read must not change what a later caller sees, got {second}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .vpc_endpoint_connections
            .is_empty(),
        "describing connections must not write to the store"
    );
}

/// Every connection the store holds is reported, in store order, with the members it was
/// stored with — and a connection written after a call shows up in the next one, so the
/// answer is computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_connection_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all connections", &[]).await;
    assert_eq!(
        reported, seeded.endpoint_ids,
        "all connections: expected every stored connection in store order, got {reported:?}"
    );

    // One connection, member by member: everything the write set round-trips through the
    // describe, including the lists and the state's modelled spelling.
    let (status, body) = describe(
        &router,
        &filters(&[("vpc-endpoint-id", &[seeded.endpoint_ids[5].as_str()])]),
    )
    .await;
    assert_eq!(status, 200, "one connection: got {status} with body {body}");
    let expected_members = [
        ("vpcEndpointId", seeded.endpoint_ids[5].clone()),
        ("vpcEndpointConnectionId", seeded.connection_ids[5].clone()),
        ("serviceId", seeded.service_ids[2].clone()),
        ("vpcEndpointOwner", OTHER_ACCOUNT.to_string()),
        ("vpcEndpointState", "Available".to_string()),
        ("vpcEndpointRegion", REGION.to_string()),
        ("ipAddressType", "ipv4".to_string()),
        ("hostedZoneId", HOSTED_ZONE_ID.to_string()),
        ("scope", "vpc-endpoint-charges".to_string()),
        ("payerResponsibilityType", "vpc-endpoint-account".to_string()),
        ("key", "Name".to_string()),
        ("value", "web".to_string()),
        ("creationTimestamp", "2023-11-14T22:13:25Z".to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "one connection: expected <{member}>{value}</{member}>, got {body}"
        );
    }
    // Both Network Load Balancer ARNs of that service are reported.
    for ordinal in 0..2 {
        let arn = load_balancer_arn("net", 2, ordinal);
        assert!(
            body.contains(&arn),
            "one connection: expected the load balancer ARN {arn}, got {body}"
        );
    }

    // A connection stored after the first call is reported by the next one.
    let late_endpoint_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let endpoint_id = guard.next_id("vpce");
        guard.insert_vpc_endpoint_connection(VpcEndpointConnectionSpec {
            service_id: seeded.service_ids[0].clone(),
            vpc_endpoint_id: endpoint_id.clone(),
            vpc_endpoint_owner: OTHER_ACCOUNT.to_string(),
            vpc_endpoint_state: "PendingAcceptance".to_string(),
            creation_timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_100),
            vpc_endpoint_region: REGION.to_string(),
            ip_address_type: Some("ipv4".to_string()),
            dns_entries: Vec::new(),
            network_load_balancer_arns: Vec::new(),
            gateway_load_balancer_arns: Vec::new(),
            payer_responsibilities: Vec::new(),
            tags: vec![tag("Name", "late")],
        });
        endpoint_id
    };

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.endpoint_ids.clone();
    expected.push(late_endpoint_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_endpoint_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects the connections whose own attribute matches a value;
/// values within one filter are OR'd and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_connections_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.endpoint_ids.len()).collect();

    // One row per filter name `DescribeVpcEndpointConnectionsRequest$Filters` documents,
    // with rows for the value counts, the wildcards and the two spellings of a state.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("ip-address-type", vec!["ipv4".into()], vec![0, 3, 5, 7]),
        ("ip-address-type", vec!["ipv6".into()], vec![1, 6]),
        // `IpAddressType`'s third member, which the filter's own doc list does not name.
        ("ip-address-type", vec!["dualstack".into()], vec![2]),
        (
            "ip-address-type",
            vec!["ipv4".into(), "ipv6".into()],
            vec![0, 1, 3, 5, 6, 7],
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("ip-address-type", vec!["IPV4".into()], vec![]),
        (
            "service-id",
            vec![seeded.service_ids[0].clone()],
            vec![0, 1],
        ),
        (
            "service-id",
            vec![seeded.service_ids[1].clone()],
            vec![2, 3, 4],
        ),
        (
            "service-id",
            vec![
                seeded.service_ids[0].clone(),
                seeded.service_ids[2].clone(),
            ],
            vec![0, 1, 5, 6, 7],
        ),
        (
            "vpc-endpoint-id",
            vec![seeded.endpoint_ids[3].clone()],
            vec![3],
        ),
        // EC2's own `*` wildcard, which every endpoint id matches.
        ("vpc-endpoint-id", vec!["vpce-*".into()], all.clone()),
        (
            "vpc-endpoint-owner",
            vec![account],
            vec![0, 1, 2, 3, 6, 7],
        ),
        ("vpc-endpoint-owner", vec![OTHER_ACCOUNT.into()], vec![4, 5]),
        (
            "vpc-endpoint-region",
            vec![REGION.into()],
            vec![0, 1, 2, 5, 6, 7],
        ),
        ("vpc-endpoint-region", vec!["us-west-2".into()], vec![3]),
        // "the Region of the endpoint or `cross-region` to find endpoints for other
        // Regions": the endpoints outside the Region the service is hosted in.
        ("vpc-endpoint-region", vec!["cross-region".into()], vec![3, 4]),
        // The state under the spelling the model's `State` enum publishes...
        ("vpc-endpoint-state", vec!["Available".into()], vec![0, 5]),
        // ...and under the spelling the filter's own doc list publishes.
        ("vpc-endpoint-state", vec!["available".into()], vec![0, 5]),
        (
            "vpc-endpoint-state",
            vec!["pendingAcceptance".into()],
            vec![1, 2],
        ),
        (
            "vpc-endpoint-state",
            vec!["rejected".into(), "failed".into()],
            vec![3, 6],
        ),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("vpc-endpoint-state", vec![], vec![]),
        ("vpc-endpoint-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// `cross-region` is measured against the Region the call is made in, not against a fixed
/// Region: the same store answers a different set to a call signed for another Region.
#[tokio::test]
async fn cross_region_is_measured_against_the_region_the_call_is_made_in() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the Region the services are hosted in, and the Regions two of the seeded
    // endpoints are in — in each case `cross-region` is every OTHER endpoint.
    let cases = [
        (REGION, vec![3, 4]),
        ("us-west-2", vec![0, 1, 2, 4, 5, 6, 7]),
        ("eu-west-1", vec![0, 1, 2, 3, 5, 6, 7]),
    ];
    for (region, expected_indices) in cases {
        let (status, body) = describe_in(
            &router,
            region,
            &filters(&[("vpc-endpoint-region", &["cross-region"])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with body {body}"
        );
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            elements(&body, "vpcEndpointId"),
            expected,
            "case {region:?}: expected {expected:?}, got {body}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the connections that
/// must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the connections matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let service_one = seeded.service_ids[1].clone();
    let service_one_values = [service_one.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("service-id", &service_one_values[..]),
                ("vpc-endpoint-state", &["pendingAcceptance"][..]),
            ],
            vec![2],
        ),
        (
            vec![
                ("vpc-endpoint-owner", &[OTHER_ACCOUNT][..]),
                ("vpc-endpoint-region", &["cross-region"][..]),
            ],
            vec![4],
        ),
        (
            vec![
                ("ip-address-type", &["ipv4", "ipv6"][..]),
                ("vpc-endpoint-region", &[REGION][..]),
                ("vpc-endpoint-state", &["available", "failed"][..]),
            ],
            vec![0, 5, 6],
        ),
        // Each filter matches something, but no connection matches both.
        (
            vec![
                ("vpc-endpoint-state", &["available"][..]),
                ("vpc-endpoint-region", &["us-west-2"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// Every state the `vpc-endpoint-state` filter documents selects the connection stored in
/// the `State` enum member that names the same state — under both spellings, and for every
/// entry of the table rather than the one the issue cited.
#[tokio::test]
async fn every_documented_state_selects_the_connection_in_that_state() {
    let (router, engine) = super::ec2_query::emulator();

    // The modelled half of each pair must be a member of the `State` enum the connection's
    // `VpcEndpointState` targets — `STATE_VALUES` is that enum, generated from the model.
    for (documented, modelled) in VPC_ENDPOINT_STATE_FILTER_VALUES {
        assert!(
            STATE_VALUES.contains(&modelled),
            "case {documented:?}: {modelled:?} is not a member of the model's State enum \
             {STATE_VALUES:?}"
        );
    }

    // One connection per documented state, each to the same service.
    let mut endpoint_ids: Vec<(&str, &str, String)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let service_id = guard.next_id("vpce-svc");
        for (documented, modelled) in VPC_ENDPOINT_STATE_FILTER_VALUES {
            let endpoint_id = guard.next_id("vpce");
            guard.insert_vpc_endpoint_connection(VpcEndpointConnectionSpec {
                service_id: service_id.clone(),
                vpc_endpoint_id: endpoint_id.clone(),
                vpc_endpoint_owner: OTHER_ACCOUNT.to_string(),
                vpc_endpoint_state: modelled.to_string(),
                creation_timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_200),
                vpc_endpoint_region: REGION.to_string(),
                ip_address_type: Some("ipv4".to_string()),
                dns_entries: Vec::new(),
                network_load_balancer_arns: Vec::new(),
                gateway_load_balancer_arns: Vec::new(),
                payer_responsibilities: Vec::new(),
                tags: Vec::new(),
            });
            endpoint_ids.push((documented, modelled, endpoint_id));
        }
    }

    for (documented, modelled, endpoint_id) in &endpoint_ids {
        for value in [documented, modelled] {
            let case = format!("vpc-endpoint-state={value:?}");
            let reported =
                described_ids(&router, &case, &filters(&[("vpc-endpoint-state", &[value])])).await;
            assert_eq!(
                reported,
                vec![endpoint_id.clone()],
                "case {case:?}: expected only {endpoint_id:?}, got {reported:?}"
            );
        }
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: the tag filters this operation does NOT document (other describes do), the
    // underscore spelling, a documented name with the wrong case, near-misses of two
    // documented names, a filter of another describe, and an empty name.
    let cases = [
        "tag:Name",
        "tag-key",
        "vpc_endpoint_state",
        "Vpc-Endpoint-State",
        "vpc-endpoint-states",
        "ip-address-types",
        "service-name",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: expected the message to name the filter, got {body}"
        );
        assert!(
            elements(&body, "vpcEndpointId").is_empty(),
            "case {name:?}: a refused request reports no connection, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every connection exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_connection_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: the documented smallest page size, one that does not divide the store, one
    // that holds it exactly, and the documented largest.
    for max_results in [5, 6, 8, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "vpcEndpointId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.endpoint_ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                None => break,
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
            }
        }
        assert_eq!(
            walked, seeded.endpoint_ids,
            "case {case:?}: expected every connection exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is cut from the FILTERED set, not from the whole store: `MaxResults` and a filter
/// compose, and the token resumes within the filtered set.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("vpc-endpoint-region", &[REGION])]);
    params.extend(max_results_param(5));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert_eq!(
        elements(&body, "vpcEndpointId"),
        ids_at(&seeded, &[0, 1, 2, 5, 6]),
        "filtered first page: expected the first five in-Region connections, got {body}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut resumed = filters(&[("vpc-endpoint-region", &[REGION])]);
    resumed.extend(max_results_param(5));
    resumed.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &resumed).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[7]),
        "filtered second page: expected the one in-Region connection left, got {second:?}"
    );
}

/// `MaxResults` below the documented minimum is refused; each documented bound itself is
/// accepted, and a value above the maximum is NOT refused — "if MaxResults is given a value
/// larger than 1,000, only 1,000 results are returned".
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_below_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // "This value can be between 5 and 1,000": each published bound, one below the lower
    // one, and one above the upper one.
    let cases = [(1, 400), (4, 400), (5, 200), (1000, 200), (1001, 200)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case MaxResults={max_results}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

/// A page size above the documented maximum returns that maximum: "if MaxResults is given a
/// value larger than 1,000, only 1,000 results are returned".
#[tokio::test]
async fn a_page_size_above_the_maximum_returns_at_most_one_thousand() {
    let (router, engine) = super::ec2_query::emulator();

    // One more connection than the documented maximum page size, so the cap is observable.
    let stored = 1001;
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let service_id = guard.next_id("vpce-svc");
        for _ in 0..stored {
            let endpoint_id = guard.next_id("vpce");
            guard.insert_vpc_endpoint_connection(VpcEndpointConnectionSpec {
                service_id: service_id.clone(),
                vpc_endpoint_id: endpoint_id,
                vpc_endpoint_owner: OTHER_ACCOUNT.to_string(),
                vpc_endpoint_state: "PendingAcceptance".to_string(),
                creation_timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_300),
                vpc_endpoint_region: REGION.to_string(),
                ip_address_type: Some("ipv4".to_string()),
                dns_entries: Vec::new(),
                network_load_balancer_arns: Vec::new(),
                gateway_load_balancer_arns: Vec::new(),
                payer_responsibilities: Vec::new(),
                tags: Vec::new(),
            });
        }
    }

    // Rows: a page size past the maximum, and no page size at all — both answer the
    // largest page this service serves.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("MaxResults=1001", max_results_param(1001)),
        ("no MaxResults", Vec::new()),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let page = elements(&body, "vpcEndpointId");
        assert_eq!(
            page.len(),
            1000,
            "case {case:?}: expected the 1,000-result maximum, got {} results",
            page.len()
        );
        assert!(
            element(&body, "nextToken").is_some(),
            "case {case:?}: a connection follows the page, so a token is due"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // endpoint id a caller might guess the token is, and that id hex-encoded (the shape the
    // token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.endpoint_ids[0].clone(),
        ::hex::encode(&seeded.endpoint_ids[0]),
        ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.endpoint_ids[0])),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` on a request that would have succeeded answers `DryRunOperation` and reports
/// nothing; a request that would have been REFUSED is refused with its own reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let dry_run = vec![("DryRun".to_string(), "true".to_string())];
    let (status, body) = describe(&router, &dry_run).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "vpcEndpointId").is_empty(),
        "a dry run reports no connection, got {body}"
    );

    // Rows: a dry run whose filter name is undefined, and one whose page size is below the
    // documented minimum — each refused for that reason rather than with DryRunOperation.
    let mut undefined_filter = filters(&[("tag:Name", &["web"])]);
    undefined_filter.extend(dry_run.clone());
    let mut small_page = max_results_param(4);
    small_page.extend(dry_run);
    for (case, params) in [
        ("an undefined filter", undefined_filter),
        ("MaxResults=4", small_page),
    ] {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

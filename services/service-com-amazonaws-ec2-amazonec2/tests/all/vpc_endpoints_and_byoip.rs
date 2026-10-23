// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the VPC endpoint and BYOIP surfaces: accepting a connection to one
//! of the account's own endpoint services, describing the account's endpoints, and the two
//! BYOIP operations, driven over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a batch operation judges each item of its list on its own and reports the ones it could
//!   not act on in `Unsuccessful`, with the code this service answers that cause with
//!   elsewhere — one bad id does not refuse the items beside it;
//! * a describe REPORTS THE STORE, narrowed by the ids and the documented filters the
//!   request names;
//! * an operation that acts on a resource in one particular state refuses it in any other
//!   with `IncorrectState`, and leaves the transient state of the move behind with the clock
//!   owing the terminal one.
//!
//! Each table row is a distinct input along what those generalise over: which state the
//! resource is in, which documented filter selects it, which address family the range
//! belongs to, and which member of the request is at fault.

use std::time::{Duration, SystemTime};

use ec2::ops::types::{AsnAssociation, ByoipCidr, IpamPool, ServiceConfiguration, Tag, VpcEndpoint};
use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, BYOIP_CIDR_STATE_ADVERTISED,
    BYOIP_CIDR_STATE_PENDING_ADVERTISING, BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State, IPAM_SCOPE_TYPE_PUBLIC, TRANSITION_DELAY,
    VPC_ENDPOINT_STATE_AVAILABLE, VPC_ENDPOINT_STATE_PENDING,
    VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE, VpcEndpointConnectionSpec, canonical_byoip_cidr,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// An account other than the emulator's own.
const OTHER_ACCOUNT: &str = "210987654321";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `(name, value)` request member.
fn member(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A tag, as the seeded resources carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

// ---------------------------------------------------------------------------
// AcceptVpcEndpointConnections
// ---------------------------------------------------------------------------

/// The ids the connection fixtures were given: two of the account's own endpoint services,
/// and one connection per row of the table below.
struct Connections {
    /// The service the acceptance is made against, and a second one beside it.
    services: Vec<String>,
    /// `pendingAcceptance` to the first service — the one connection that can be accepted.
    pending: String,
    /// Already `Available` to the first service.
    available: String,
    /// `pendingAcceptance`, but to the OTHER service.
    other_service: String,
}

/// Seed two endpoint services and three connections differing in the state they are in and
/// the service they are to.
fn seed_connections(engine: &engine::InMemoryEngine) -> Connections {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let services: Vec<String> = (0..2)
        .map(|index| {
            guard
                .insert_vpc_endpoint_service(ServiceConfiguration {
                    service_name: Some(format!("com.amazonaws.vpce.{REGION}.vpce-svc-{index}")),
                    // The `ServiceState` enum's `Available` member.
                    service_state: Some("Available".to_string()),
                    acceptance_required: Some(true),
                    tags: Some(vec![tag("Name", "shared")]),
                    ..Default::default()
                })
                .expect("the store holds the seeded endpoint service")
        })
        .collect();

    let mut connect = |service_index: usize, connection_state: &str| -> String {
        let endpoint_id = guard.next_id(ec2::state::VPC_ENDPOINT_ID_PREFIX);
        guard.insert_vpc_endpoint_connection(VpcEndpointConnectionSpec {
            service_id: services[service_index].clone(),
            vpc_endpoint_id: endpoint_id.clone(),
            vpc_endpoint_owner: OTHER_ACCOUNT.to_string(),
            vpc_endpoint_state: connection_state.to_string(),
            creation_timestamp: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            vpc_endpoint_region: REGION.to_string(),
            ip_address_type: Some("ipv4".to_string()),
            dns_entries: Vec::new(),
            network_load_balancer_arns: Vec::new(),
            gateway_load_balancer_arns: Vec::new(),
            payer_responsibilities: Vec::new(),
            tags: Vec::new(),
        });
        endpoint_id
    };
    let pending = connect(0, VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE);
    let available = connect(0, VPC_ENDPOINT_STATE_AVAILABLE);
    let other_service = connect(1, VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE);
    Connections {
        services,
        pending,
        available,
        other_service,
    }
}

/// The state `DescribeVpcEndpointConnections` reports for one endpoint.
async fn connection_state(router: &MultiServiceRouter, endpoint_id: &str) -> String {
    let (status, body) = invoke(
        router,
        "DescribeVpcEndpointConnections",
        &filters(&[("vpc-endpoint-id", &[endpoint_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "describing {endpoint_id}: got {status} with body {body}"
    );
    element(&body, "vpcEndpointState").unwrap_or_default()
}

/// One acceptance naming four endpoints accepts the one that can be accepted and reports
/// the other three in `Unsuccessful`, each with the code of its own cause — and the accepted
/// connection is then `Pending` when read back through the describe, `Available` once the
/// clock has passed the move it owes.
#[tokio::test]
async fn accepting_endpoint_connections_accepts_what_it_can_and_reports_the_rest() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed_connections(&engine);
    let unknown = format!("{}-00000000000000000", ec2::state::VPC_ENDPOINT_ID_PREFIX);

    // Rows: the one acceptable connection, one already in service, one to another service,
    // and one no endpoint exists for.
    let named = [
        seeded.pending.as_str(),
        seeded.available.as_str(),
        seeded.other_service.as_str(),
        unknown.as_str(),
    ];
    let mut params = vec![member("ServiceId", &seeded.services[0])];
    params.extend(indexed("VpcEndpointId", &named));

    let (status, body) = invoke(&router, "AcceptVpcEndpointConnections", &params).await;
    assert_eq!(status, 200, "accept: got {status} with body {body}");
    assert_eq!(
        elements(&body, "resourceId"),
        vec![
            seeded.available.clone(),
            seeded.other_service.clone(),
            unknown.clone(),
        ],
        "accept: expected exactly the three endpoints that could not be accepted, got {body}"
    );
    assert_eq!(
        elements(&body, "code"),
        vec![
            ec2::state::INCORRECT_STATE.to_string(),
            ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND.to_string(),
            ec2::state::INVALID_VPC_ENDPOINT_ID_NOT_FOUND.to_string(),
        ],
        "accept: expected one code per cause, got {body}"
    );

    // The accepted connection is in the transient state now, read back through the describe.
    assert_eq!(
        connection_state(&router, &seeded.pending).await,
        VPC_ENDPOINT_STATE_PENDING,
        "the accepted connection must be in the transient state of an acceptance"
    );
    // Nothing else moved.
    assert_eq!(
        connection_state(&router, &seeded.other_service).await,
        VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE,
        "a connection to another service must not be accepted"
    );

    // The move the clock owed falls due on its own.
    tokio::time::sleep(past_the_transition()).await;
    assert_eq!(
        connection_state(&router, &seeded.pending).await,
        VPC_ENDPOINT_STATE_AVAILABLE,
        "the clock owed the accepted connection its available state"
    );

    // And accepting it again now reports it as unsuccessful rather than accepting twice.
    let mut again = vec![member("ServiceId", &seeded.services[0])];
    again.extend(indexed("VpcEndpointId", &[seeded.pending.as_str()]));
    let (status, body) = invoke(&router, "AcceptVpcEndpointConnections", &again).await;
    assert_eq!(status, 200, "second accept: got {status} with body {body}");
    assert_eq!(
        elements(&body, "code"),
        vec![ec2::state::INCORRECT_STATE.to_string()],
        "second accept: expected IncorrectState for the connection already in service, got \
         {body}"
    );
}

// ---------------------------------------------------------------------------
// DescribeVpcEndpoints
// ---------------------------------------------------------------------------

/// What one seeded VPC endpoint is written with. The rows differ along every attribute the
/// documented filters select on.
struct EndpointFixture {
    endpoint_type: &'static str,
    vpc_id: &'static str,
    service_name: &'static str,
    service_region: &'static str,
    state: &'static str,
    ip_address_type: Option<&'static str>,
    tags: Vec<Tag>,
}

/// The endpoints the filter table is written against.
fn endpoint_fixtures() -> Vec<EndpointFixture> {
    vec![
        EndpointFixture {
            endpoint_type: "Interface",
            vpc_id: "vpc-0000000000000000a",
            service_name: "com.amazonaws.us-east-1.s3",
            service_region: REGION,
            state: VPC_ENDPOINT_STATE_AVAILABLE,
            ip_address_type: Some("ipv4"),
            tags: vec![tag("Name", "web"), tag("Tier", "public")],
        },
        EndpointFixture {
            endpoint_type: "Gateway",
            vpc_id: "vpc-0000000000000000a",
            service_name: "com.amazonaws.us-east-1.dynamodb",
            service_region: REGION,
            state: VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE,
            // A gateway endpoint carries no IP address type.
            ip_address_type: None,
            tags: vec![tag("Name", "data")],
        },
        EndpointFixture {
            endpoint_type: "GatewayLoadBalancer",
            vpc_id: "vpc-0000000000000000b",
            service_name: "com.amazonaws.vpce.us-west-2.vpce-svc-abc",
            service_region: "us-west-2",
            state: "Deleting",
            ip_address_type: Some("ipv6"),
            tags: Vec::new(),
        },
        EndpointFixture {
            endpoint_type: "Resource",
            vpc_id: "vpc-0000000000000000b",
            service_name: "com.amazonaws.us-east-1.rds",
            service_region: REGION,
            state: "Failed",
            ip_address_type: Some("dualstack"),
            tags: vec![tag("Tier", "private")],
        },
    ]
}

/// Write the endpoint fixtures into the store, the way an operation creating one would, and
/// report the ids they were given, in store order.
fn seed_endpoints(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    endpoint_fixtures()
        .into_iter()
        .map(|fixture| {
            guard
                .insert_vpc_endpoint(VpcEndpoint {
                    vpc_endpoint_type: Some(fixture.endpoint_type.to_string()),
                    vpc_id: Some(fixture.vpc_id.to_string()),
                    service_name: Some(fixture.service_name.to_string()),
                    service_region: Some(fixture.service_region.to_string()),
                    state: Some(fixture.state.to_string()),
                    ip_address_type: fixture.ip_address_type.map(str::to_string),
                    owner_id: Some(engine::emulator_account_id().to_string()),
                    tags: Some(fixture.tags),
                    ..Default::default()
                })
                .expect("the store holds the seeded endpoint")
        })
        .collect()
}

/// The endpoint ids at `indices`, in store order.
fn ids_at(seeded: &[String], indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded[*index].clone()).collect()
}

/// The endpoint ids one describe reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = invoke(router, "DescribeVpcEndpoints", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "vpcEndpointId")
}

/// A fresh account has created no VPC endpoint, so the report is empty; every endpoint the
/// store holds is then reported in store order, and one written later shows up in the next
/// call — so the answer is computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_endpoint_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    assert!(
        described_ids(&router, "a fresh account", &[]).await.is_empty(),
        "a fresh account holds no VPC endpoint"
    );

    let seeded = seed_endpoints(&engine);
    assert_eq!(
        described_ids(&router, "every endpoint", &[]).await,
        seeded,
        "expected every stored endpoint in store order"
    );

    // One endpoint, member by member: what the write set round-trips through the describe.
    let (status, body) = invoke(
        &router,
        "DescribeVpcEndpoints",
        &indexed("VpcEndpointId", &[seeded[0].as_str()]),
    )
    .await;
    assert_eq!(status, 200, "one endpoint: got {status} with body {body}");
    for (element_name, expected) in [
        ("vpcEndpointId", seeded[0].as_str()),
        ("vpcEndpointType", "Interface"),
        ("vpcId", "vpc-0000000000000000a"),
        ("serviceName", "com.amazonaws.us-east-1.s3"),
        ("state", VPC_ENDPOINT_STATE_AVAILABLE),
        ("ipAddressType", "ipv4"),
        ("serviceRegion", REGION),
        ("key", "Name"),
        ("value", "web"),
    ] {
        assert_eq!(
            element(&body, element_name).as_deref(),
            Some(expected),
            "one endpoint: expected <{element_name}> to carry {expected:?}, got {body}"
        );
    }

    let late = engine
        .state::<Ec2State>()
        .lock()
        .insert_vpc_endpoint(VpcEndpoint {
            vpc_endpoint_type: Some("Interface".to_string()),
            state: Some(VPC_ENDPOINT_STATE_AVAILABLE.to_string()),
            ..Default::default()
        })
        .expect("the store holds the later endpoint");
    let mut expected = seeded.clone();
    expected.push(late.clone());
    assert_eq!(
        described_ids(&router, "after a later write", &[]).await,
        expected,
        "expected {late:?} to be reported too"
    );
}

/// Each documented filter selects the endpoints whose own attribute matches a value; values
/// within one filter are OR'd, several filters are AND'd, and the comparison is
/// case-sensitive with EC2's `*`/`?` wildcards.
#[tokio::test]
async fn each_documented_filter_selects_the_endpoints_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed_endpoints(&engine);

    // One row per filter name `DescribeVpcEndpointsRequest$Filters` documents, with rows
    // for the value counts, the wildcards, the two spellings of a state and the tag family.
    let cases: Vec<(&str, Vec<&str>, Vec<usize>)> = vec![
        ("ip-address-type", vec!["ipv4"], vec![0]),
        ("ip-address-type", vec!["ipv6"], vec![2]),
        ("ip-address-type", vec!["ipv4", "ipv6"], vec![0, 2]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("ip-address-type", vec!["IPV4"], vec![]),
        (
            "service-name",
            vec!["com.amazonaws.us-east-1.s3"],
            vec![0],
        ),
        ("service-name", vec!["com.amazonaws.us-east-1.*"], vec![0, 1, 3]),
        ("service-region", vec![REGION], vec![0, 1, 3]),
        ("service-region", vec!["us-west-2"], vec![2]),
        ("vpc-id", vec!["vpc-0000000000000000b"], vec![2, 3]),
        ("vpc-endpoint-type", vec!["Interface"], vec![0]),
        ("vpc-endpoint-type", vec!["Gateway"], vec![1]),
        ("vpc-endpoint-type", vec!["GatewayLoadBalancer"], vec![2]),
        ("vpc-endpoint-type", vec!["Resource"], vec![3]),
        ("vpc-endpoint-id", vec![seeded[1].as_str()], vec![1]),
        // EC2's own `*` wildcard, which every endpoint id matches.
        ("vpc-endpoint-id", vec!["vpce-*"], vec![0, 1, 2, 3]),
        // The modelled spelling of the state, and the one the filter documents.
        ("vpc-endpoint-state", vec!["Available"], vec![0]),
        ("vpc-endpoint-state", vec!["available"], vec![0]),
        ("vpc-endpoint-state", vec!["pendingAcceptance"], vec![1]),
        ("vpc-endpoint-state", vec!["deleting"], vec![2]),
        ("vpc-endpoint-state", vec!["failed"], vec![3]),
        ("tag-key", vec!["Name"], vec![0, 1]),
        ("tag-key", vec!["Tier"], vec![0, 3]),
        ("tag:Name", vec!["web"], vec![0]),
        ("tag:Tier", vec!["private"], vec![3]),
        ("tag:Tier", vec!["public", "private"], vec![0, 3]),
        // A documented filter carrying no value at all: there is no value for the attribute
        // to match, so it selects nothing rather than everything.
        ("vpc-endpoint-type", vec![], vec![]),
    ];
    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let reported = described_ids(&router, &case, &filters(&[(name, &values)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Two filters are joined with an AND.
    let both = filters(&[
        ("vpc-id", &["vpc-0000000000000000a"]),
        ("vpc-endpoint-state", &["available"]),
    ]);
    let reported = described_ids(&router, "vpc-id AND vpc-endpoint-state", &both).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[0]),
        "expected only the available endpoint of that VPC, got {reported:?}"
    );
}

/// A filter name this operation does not document is refused with `InvalidParameterValue`
/// rather than silently reported as an empty set.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed_endpoints(&engine);

    // Rows: a `tag:` prefix with no key, a documented name with the wrong case, the
    // underscore spelling, a near-miss, a filter of another describe, and an empty name.
    for name in [
        "tag:",
        "Vpc-Endpoint-Type",
        "vpc_endpoint_type",
        "vpc-endpoint-types",
        "vpc-endpoint-owner",
        "",
    ] {
        let (status, body) = invoke(
            &router,
            "DescribeVpcEndpoints",
            &filters(&[(name, &["anything"])]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// Whatever the page size, walking the pages covers every endpoint exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_endpoint_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed_endpoints(&engine);

    // Rows: one page per endpoint, a size that does not divide the store, one that holds it
    // exactly, and the documented maximum.
    for max_results in [1, 3, 4, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = vec![member("MaxResults", &max_results.to_string())];
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = invoke(&router, "DescribeVpcEndpoints", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "vpcEndpointId");
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                None => break,
                Some(token) => {
                    params = vec![
                        member("MaxResults", &max_results.to_string()),
                        member("NextToken", token),
                    ];
                }
            }
        }
        assert_eq!(
            walked, seeded,
            "case {case:?}: expected every endpoint exactly once in store order"
        );
    }

    // A token this service did not issue is refused rather than read as a cursor.
    let (status, body) = invoke(
        &router,
        "DescribeVpcEndpoints",
        &[member("NextToken", "not-a-token")],
    )
    .await;
    assert_eq!(status, 400, "a forged token: got {status} with body {body}");
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "a forged token: expected InvalidPaginationToken, got {body}"
    );
}

// ---------------------------------------------------------------------------
// BYOIP address ranges
// ---------------------------------------------------------------------------

/// The ASN one seeded range is associated with.
const ASSOCIATED_ASN: &str = "64512";

/// Seed one BYOIP range in `state`, of either address family, and answer its canonical CIDR.
fn seed_byoip_cidr(engine: &engine::InMemoryEngine, cidr: &str, state: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_byoip_cidr(ByoipCidrRecord {
            byoip_cidr: ByoipCidr {
                cidr: Some(cidr.to_string()),
                description: Some("brought from our own space".to_string()),
                state: Some(state.to_string()),
                network_border_group: Some(REGION.to_string()),
                asn_associations: Some(vec![AsnAssociation {
                    asn: Some(ASSOCIATED_ASN.to_string()),
                    cidr: Some(cidr.to_string()),
                    state: Some("associated".to_string()),
                    status_message: None,
                }]),
                pool_id: Some("ipv4pool-ec2-0123456789abcdef0".to_string()),
                ..Default::default()
            },
            released_at: None,
            pending: None,
        });
    canonical_byoip_cidr(cidr)
        .unwrap_or_else(|problem| panic!("{cidr} is an address range this store takes: {problem:?}"))
}

/// The state of the range in an answered BYOIP response.
///
/// The range's own `<state>` follows its `<asnAssociationSet>`, whose items each carry a
/// `<state>` of their own, so it is the LAST one in the body.
fn range_state(body: &str) -> Option<String> {
    elements(body, "state").into_iter().next_back()
}

/// An address range is advertisable out of `provisioned` and out of no other state,
/// whichever address family it belongs to; the advertisement leaves it in the transient
/// state with the clock owing it `advertised`.
#[tokio::test]
async fn a_range_is_advertisable_only_out_of_provisioned() {
    // Rows: each family in the one state an advertisement may start from, and every other
    // state a provisioned range can be in — including the one the model says "cannot be
    // advertised".
    let cases = [
        ("192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED, 200),
        ("2001:db8::/56", BYOIP_CIDR_STATE_PROVISIONED, 200),
        ("192.0.2.0/24", BYOIP_CIDR_STATE_ADVERTISED, 400),
        (
            "192.0.2.0/24",
            BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
            400,
        ),
        ("192.0.2.0/24", "pending-provision", 400),
        ("192.0.2.0/24", "failed-provision", 400),
    ];
    for (cidr, state, expected_status) in cases {
        let case = format!("{cidr} in state {state}");
        let (router, engine) = super::ec2_query::emulator();
        let stored = seed_byoip_cidr(&engine, cidr, state);

        let (status, body) = invoke(
            &router,
            "AdvertiseByoipCidr",
            &[member("Cidr", &stored)],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                ec2::state::INCORRECT_STATE,
                "case {case:?}: expected IncorrectState, got {body}"
            );
            continue;
        }
        assert_eq!(
            element(&body, "cidr").as_deref(),
            Some(stored.as_str()),
            "case {case:?}: expected the range's canonical CIDR, got {body}"
        );
        assert_eq!(
            range_state(&body).as_deref(),
            Some(BYOIP_CIDR_STATE_PENDING_ADVERTISING),
            "case {case:?}: expected the transient state of an advertisement, got {body}"
        );
        // The members the range was stored with come back with it.
        assert_eq!(
            element(&body, "description").as_deref(),
            Some("brought from our own space"),
            "case {case:?}: expected the stored description, got {body}"
        );

        // The clock owes it `advertised`; the next call is what materialises the move, and
        // an advertised range cannot be advertised again.
        tokio::time::sleep(past_the_transition()).await;
        let (status, body) = invoke(
            &router,
            "AdvertiseByoipCidr",
            &[member("Cidr", &stored)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an advertised range cannot be advertised again, got {status} {body}"
        );
        let advertised = engine
            .state::<Ec2State>()
            .lock()
            .byoip_cidrs
            .get(&stored)
            .map(|record| record.state().to_string())
            .unwrap_or_default();
        assert_eq!(
            advertised, BYOIP_CIDR_STATE_ADVERTISED,
            "case {case:?}: the clock owed this range its advertised state"
        );
    }
}

/// The optional members of an advertisement are checked against the range the store holds:
/// a network border group it was not provisioned in, and an ASN it is not associated with,
/// are each refused rather than written.
#[tokio::test]
async fn an_advertisement_is_checked_against_the_stored_range() {
    // Rows: the group and the ASN the range carries (accepted), and one of each it does
    // not. Each row runs against its own freshly provisioned range, so one row cannot move
    // another out of the state an advertisement starts from.
    let cases: Vec<(Vec<(String, String)>, u16)> = vec![
        (vec![member("NetworkBorderGroup", REGION)], 200),
        (vec![member("NetworkBorderGroup", "us-west-2-lax-1")], 400),
        (vec![member("Asn", ASSOCIATED_ASN)], 200),
        (vec![member("Asn", "64513")], 400),
    ];
    for (extra, expected_status) in cases {
        let case = format!("{extra:?}");
        let (router, engine) = super::ec2_query::emulator();
        let stored = seed_byoip_cidr(&engine, "192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED);

        let mut params = vec![member("Cidr", &stored)];
        params.extend(extra);
        let (status, body) = invoke(&router, "AdvertiseByoipCidr", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_PARAMETER_VALUE,
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            // A refused advertisement writes nothing, so the range is still provisioned.
            let unchanged = engine
                .state::<Ec2State>()
                .lock()
                .byoip_cidrs
                .get(&stored)
                .map(|record| record.state().to_string())
                .unwrap_or_default();
            assert_eq!(
                unchanged, BYOIP_CIDR_STATE_PROVISIONED,
                "case {case:?}: a refused advertisement must not move the range"
            );
        }
    }
}

/// Moving a range into an IPAM pool: every member of the request is checked against the
/// pool and the range the store holds, and the move itself is visible to the operation that
/// reports the range afterwards.
#[tokio::test]
async fn moving_a_range_to_ipam_is_checked_and_then_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id().to_string();
    let (ipv4_pool, ipv6_pool, private_pool) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let mut pool = |family: &str, owner: &str, scope: &str| -> String {
            guard
                .insert_ipam_pool(IpamPool {
                    owner_id: Some(owner.to_string()),
                    address_family: Some(family.to_string()),
                    ipam_scope_type: Some(scope.to_string()),
                    ipam_region: Some(REGION.to_string()),
                    // The `IpamPoolState` enum's `create-complete` member.
                    state: Some("create-complete".to_string()),
                    public_ip_source: Some("byoip".to_string()),
                    ..Default::default()
                })
                .expect("the store holds the seeded IPAM pool")
        };
        (
            pool(ADDRESS_FAMILY_IPV4, &account, IPAM_SCOPE_TYPE_PUBLIC),
            pool(ADDRESS_FAMILY_IPV6, &account, IPAM_SCOPE_TYPE_PUBLIC),
            // The `IpamScopeType` enum's other member: a pool holding the account's own
            // internal space.
            pool(ADDRESS_FAMILY_IPV4, &account, "private"),
        )
    };
    let ipv4 = seed_byoip_cidr(&engine, "192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED);
    let ipv6 = seed_byoip_cidr(&engine, "2001:db8::/56", BYOIP_CIDR_STATE_PROVISIONED);

    // Rows: an owner that does not own the pool, a pool of the other address family, and
    // an IPv6 range (which "cannot" be moved at all) — each a value of the request the pool
    // or the range does not admit.
    let refusals: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "an owner that does not own the pool",
            vec![
                member("Cidr", &ipv4),
                member("IpamPoolId", &ipv4_pool),
                member("IpamPoolOwner", OTHER_ACCOUNT),
            ],
        ),
        (
            "a pool of the other address family",
            vec![
                member("Cidr", &ipv4),
                member("IpamPoolId", &ipv6_pool),
                member("IpamPoolOwner", &account),
            ],
        ),
        (
            "an IPv6 range",
            vec![
                member("Cidr", &ipv6),
                member("IpamPoolId", &ipv6_pool),
                member("IpamPoolOwner", &account),
            ],
        ),
        (
            "a pool in the private scope",
            vec![
                member("Cidr", &ipv4),
                member("IpamPoolId", &private_pool),
                member("IpamPoolOwner", &account),
            ],
        ),
    ];
    for (case, params) in refusals {
        let (status, body) = invoke(&router, "MoveByoipCidrToIpam", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // A range nothing provisioned is not a value the request got wrong — it is a range this
    // account does not hold, which is the store's own `InvalidCidr.NotFound`.
    let (status, body) = invoke(
        &router,
        "MoveByoipCidrToIpam",
        &[
            member("Cidr", "198.51.100.0/24"),
            member("IpamPoolId", &ipv4_pool),
            member("IpamPoolOwner", &account),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a range nothing provisioned: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_CIDR_NOT_FOUND,
        "a range nothing provisioned: expected InvalidCidr.NotFound, got {body}"
    );

    // The move itself.
    let params = vec![
        member("Cidr", &ipv4),
        member("IpamPoolId", &ipv4_pool),
        member("IpamPoolOwner", &account),
    ];
    let (status, body) = invoke(&router, "MoveByoipCidrToIpam", &params).await;
    assert_eq!(status, 200, "move: got {status} with body {body}");
    assert_eq!(
        element(&body, "ipamPoolId").as_deref(),
        Some(ipv4_pool.as_str()),
        "move: expected the range to name the pool it was moved into, got {body}"
    );

    // A second move would name a second pool for one range.
    let (status, body) = invoke(&router, "MoveByoipCidrToIpam", &params).await;
    assert_eq!(
        status, 400,
        "a second move: expected a refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INCORRECT_STATE,
        "a second move: expected IncorrectState, got {body}"
    );

    // And the move is visible to the other operation that reports the range: the range is
    // still provisioned, so it is still advertisable, and it now names its IPAM pool.
    let (status, body) = invoke(&router, "AdvertiseByoipCidr", &[member("Cidr", &ipv4)]).await;
    assert_eq!(status, 200, "advertise after the move: got {status} {body}");
    assert_eq!(
        element(&body, "ipamPoolId").as_deref(),
        Some(ipv4_pool.as_str()),
        "advertise after the move: expected the stored IPAM pool id, got {body}"
    );
}

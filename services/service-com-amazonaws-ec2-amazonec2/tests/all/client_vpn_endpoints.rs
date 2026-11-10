// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeClientVpnEndpoints`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeClientVpnEndpoints`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: it answers the Client VPN
//! endpoints the account holds, as the modelled shape they were stored as — narrowed by the
//! `ClientVpnEndpointIds` and `Filters` the request names and cut into `MaxResults`-sized
//! pages — refusing an endpoint id that is not an endpoint id at all or that this account
//! holds no endpoint for, and reporting an empty set for an account that holds none. Every
//! table row is a distinct input along what that generalises over: how many endpoints are
//! stored, which ids are asked for and in which order, which documented filter, how many
//! values, how many filters, which page size, and which form of unresolvable id.

use ec2::ops::types::{ClientVpnEndpoint, ClientVpnEndpointStatus, Tag};
use ec2::state::{
    CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE, ComAmazonawsEc2Amazonec2State as Ec2State,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};

/// The Region every call here is signed for. The endpoint store is per account rather than
/// per Region, so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ACTION: &str = "DescribeClientVpnEndpoints";

/// The XML element each reported endpoint's id arrives in
/// (`ClientVpnEndpoint$ClientVpnEndpointId`'s `xmlName`).
const ID_ELEMENT: &str = "clientVpnEndpointId";

/// The ec2Query form field the `ClientVpnEndpointIds` list is flattened into:
/// `DescribeClientVpnEndpointsRequest$ClientVpnEndpointIds` is renamed by the model to
/// `ClientVpnEndpointId`, so the key is that name indexed (`ClientVpnEndpointId.1`, `.2`, …).
const ENDPOINT_IDS_PARAM: &str = "ClientVpnEndpointId";

/// One call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, ACTION, params).await
}

/// The endpoint ids a successful call reports, in the order it reported them.
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
    elements(&body, ID_ELEMENT)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// One seeded endpoint: the members a writer chooses, differing along every attribute the
/// documented filters select on and along the ones the describe reports back.
struct EndpointRow {
    description: &'static str,
    client_cidr_block: &'static str,
    /// "The transport protocol used by the Client VPN endpoint"
    /// (`ClientVpnEndpoint$TransportProtocol`, `tcp | udp`) — `None` for an endpoint stored
    /// without one, which the `transport-protocol` filter must then select for no value.
    transport_protocol: Option<&'static str>,
    vpn_port: Option<i32>,
    tags: &'static [(&'static str, &'static str)],
}

/// The endpoints every table below is written against. They differ along the attribute each
/// documented filter selects on (the transport protocol: two `udp`, two `tcp` and one with
/// none) and along what else they carry, and seven rows are more than the model's minimum
/// page size, so paging cuts them.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            description: "the first endpoint",
            client_cidr_block: "10.100.0.0/22",
            transport_protocol: Some("udp"),
            vpn_port: Some(443),
            tags: &[("Name", "first")],
        },
        EndpointRow {
            description: "an endpoint speaking tcp",
            client_cidr_block: "10.101.0.0/22",
            transport_protocol: Some("tcp"),
            vpn_port: Some(443),
            tags: &[],
        },
        EndpointRow {
            description: "an endpoint on the alternate port",
            client_cidr_block: "10.102.0.0/16",
            transport_protocol: Some("udp"),
            vpn_port: Some(1194),
            tags: &[("Name", "third"), ("Team", "network")],
        },
        EndpointRow {
            description: "a second tcp endpoint",
            client_cidr_block: "10.103.0.0/22",
            transport_protocol: Some("tcp"),
            vpn_port: Some(1194),
            tags: &[("Team", "network")],
        },
        EndpointRow {
            description: "an endpoint with a wide client range",
            client_cidr_block: "10.104.0.0/12",
            transport_protocol: Some("udp"),
            vpn_port: None,
            tags: &[],
        },
        EndpointRow {
            // Stored with no transport protocol at all: an attribute an endpoint carries no
            // value for matches no filter value.
            description: "an endpoint with no transport protocol",
            client_cidr_block: "10.105.0.0/22",
            transport_protocol: None,
            vpn_port: None,
            tags: &[("Name", "sixth")],
        },
        EndpointRow {
            description: "the last endpoint",
            client_cidr_block: "10.106.0.0/22",
            transport_protocol: Some("udp"),
            vpn_port: Some(443),
            tags: &[],
        },
    ]
}

/// Write one endpoint, the way `CreateClientVpnEndpoint` would, and answer the
/// `ClientVpnEndpointId` it was minted with.
fn seed_endpoint(engine: &engine::InMemoryEngine, row: &EndpointRow) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some(row.description.to_string()),
            client_cidr_block: Some(row.client_cidr_block.to_string()),
            transport_protocol: row.transport_protocol.map(str::to_string),
            vpn_port: row.vpn_port,
            tags: Some(
                row.tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
            ..Default::default()
        })
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the store must take the Client VPN endpoint",
                row.description
            )
        })
}

/// Every row of [`endpoint_rows`], in the order it was written: the ids they were minted
/// with.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    endpoint_rows()
        .iter()
        .map(|row| seed_endpoint(engine, row))
        .collect()
}

/// The ids at `indices` of `ids`, in ascending index order — the order the store reports in,
/// whatever order a request asked for them.
fn ids_at(ids: &[String], indices: &[usize]) -> Vec<String> {
    let mut selected: Vec<usize> = indices.to_vec();
    selected.sort_unstable();
    selected.iter().map(|index| ids[*index].clone()).collect()
}

/// The `ClientVpnEndpointId.N` fields naming `indices` of `ids`.
fn id_params(ids: &[String], indices: &[usize]) -> Vec<(String, String)> {
    let asked: Vec<String> = indices.iter().map(|index| ids[*index].clone()).collect();
    let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
    indexed(ENDPOINT_IDS_PARAM, &borrowed)
}

/// How many endpoints the store holds, as the "a read writes nothing" assertions read it.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_endpoints
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// An account holding no Client VPN endpoint reports HTTP 200 with an empty set and no
/// token, and describing is a READ: two calls report the same thing and nothing is written.
#[tokio::test]
async fn an_account_with_no_endpoint_reports_an_empty_set_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "empty account: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, ID_ELEMENT);
    assert!(
        reported.is_empty(),
        "empty account: expected no endpoint, got {reported:?} from {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "empty account: a complete report carries no NextToken, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second, reported,
        "a read must not change what a later caller sees: {reported:?} then {second:?}"
    );
    assert!(
        stored_ids(&engine).is_empty(),
        "describing endpoints must not write one to the store, got {:?}",
        stored_ids(&engine)
    );
}

/// Every endpoint the store holds is reported, in store order, and an endpoint written after
/// a call shows up in the next one — so the answer is computed from the store rather than
/// fixed. Describing writes nothing either way.
#[tokio::test]
async fn every_stored_endpoint_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "every endpoint", &[]).await;
    assert_eq!(
        reported, seeded,
        "every endpoint: expected every stored endpoint in store order, got {reported:?}"
    );

    let late = seed_endpoint(
        &engine,
        &EndpointRow {
            description: "an endpoint created after the first read",
            client_cidr_block: "10.199.0.0/22",
            transport_protocol: Some("tcp"),
            vpn_port: Some(443),
            tags: &[],
        },
    );
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.clone();
    expected.push(late.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late:?} to be reported too, got {after:?}"
    );
    assert_eq!(
        stored_ids(&engine),
        expected,
        "describing must neither add nor drop an endpoint"
    );
}

/// The modelled shape a stored endpoint is kept as is the one the wire reports, so a round
/// trip through the store loses no member: what the writer stored is what the describe
/// answers, including the `pending-associate` an endpoint with no target network sits in and
/// the attributes a row carries no value for, which are reported as absent rather than as a
/// default.
#[tokio::test]
async fn a_stored_endpoint_round_trips_through_the_describe() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let rows = endpoint_rows();

    for (index, row) in rows.iter().enumerate() {
        let case = row.description;
        let (status, body) = describe(&router, &id_params(&seeded, &[index])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, ID_ELEMENT),
            vec![seeded[index].clone()],
            "case {case:?}: expected exactly the endpoint asked for, got {body}"
        );
        assert_eq!(
            element(&body, "description").as_deref(),
            Some(row.description),
            "case {case:?}: expected the stored description, got {body}"
        );
        assert_eq!(
            element(&body, "clientCidrBlock").as_deref(),
            Some(row.client_cidr_block),
            "case {case:?}: expected the stored client CIDR block, got {body}"
        );
        assert_eq!(
            element(&body, "transportProtocol").as_deref(),
            row.transport_protocol,
            "case {case:?}: expected the stored transport protocol {:?}, got {body}",
            row.transport_protocol
        );
        assert_eq!(
            element(&body, "vpnPort"),
            row.vpn_port.map(|port| port.to_string()),
            "case {case:?}: expected the stored VPN port {:?}, got {body}",
            row.vpn_port
        );
        assert_eq!(
            element(&body, "code").as_deref(),
            Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE),
            "case {case:?}: expected an endpoint with no target network to sit in \
             {CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE}, got {body}"
        );
        for (key, value) in row.tags {
            assert!(
                body.contains(&format!("<key>{key}</key>"))
                    && body.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag {key}={value} to be reported, got {body}"
            );
        }
    }

    // A member a writer DID set on an endpoint is reported for it too, so the round trip is
    // the stored shape rather than the subset the rows above happen to fill.
    let full = engine
        .state::<Ec2State>()
        .lock()
        .insert_client_vpn_endpoint(ClientVpnEndpoint {
            description: Some("an endpoint with the rest of the shape filled in".to_string()),
            client_cidr_block: Some("10.200.0.0/22".to_string()),
            transport_protocol: Some("tcp".to_string()),
            vpn_port: Some(443),
            dns_name: Some("cvpn-endpoint.prod.clientvpn.us-east-1.amazonaws.com".to_string()),
            dns_servers: Some(vec!["10.200.0.2".to_string()]),
            split_tunnel: Some(true),
            vpn_protocol: Some("openvpn".to_string()),
            server_certificate_arn: Some(
                "arn:aws:acm:us-east-1:000000000000:certificate/0123abcd".to_string(),
            ),
            vpc_id: Some("vpc-00000000000000001".to_string()),
            security_group_ids: Some(vec!["sg-00000000000000001".to_string()]),
            session_timeout_hours: Some(8),
            status: Some(ClientVpnEndpointStatus {
                code: Some(CLIENT_VPN_ENDPOINT_STATUS_PENDING_ASSOCIATE.to_string()),
                message: Some("the endpoint has no target network associated".to_string()),
            }),
            ..Default::default()
        })
        .expect("the store must take the fully populated endpoint");
    let (status, body) = describe(
        &router,
        &indexed(ENDPOINT_IDS_PARAM, &[full.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fully populated endpoint: expected HTTP 200, got {status} with body {body}"
    );
    let expected: Vec<(&str, &str)> = vec![
        ("dnsName", "cvpn-endpoint.prod.clientvpn.us-east-1.amazonaws.com"),
        ("vpnProtocol", "openvpn"),
        ("splitTunnel", "true"),
        (
            "serverCertificateArn",
            "arn:aws:acm:us-east-1:000000000000:certificate/0123abcd",
        ),
        ("vpcId", "vpc-00000000000000001"),
        ("sessionTimeoutHours", "8"),
        ("message", "the endpoint has no target network associated"),
    ];
    for (xml_name, value) in expected {
        assert_eq!(
            element(&body, xml_name).as_deref(),
            Some(value),
            "the fully populated endpoint: expected <{xml_name}>{value}</{xml_name}>, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// ClientVpnEndpointIds
// ---------------------------------------------------------------------------

/// The ids narrow the report to the endpoints named, in store order whatever order they were
/// asked for.
#[tokio::test]
async fn the_ids_narrow_the_report_to_the_endpoints_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![5],
        vec![2, 4],
        vec![4, 2],
        vec![6, 1, 3],
        (0..seeded.len()).collect(),
    ];
    for asked in cases {
        let case = format!("ClientVpnEndpointIds={asked:?}");
        let reported = described_ids(&router, &case, &id_params(&seeded, &asked)).await;
        assert_eq!(
            reported,
            ids_at(&seeded, &asked),
            "case {case:?}: expected the ids named in store order, got {reported:?}"
        );
    }
}

/// A `ClientVpnEndpointId` this account holds no endpoint for is refused
/// `InvalidClientVpnEndpointId.NotFound` — whatever FORM the value takes, because this
/// surface resolves the member by lookup alone — and never with an absent row, which would
/// read as "the account holds no such endpoint" for a request that named one.
#[tokio::test]
async fn an_endpoint_id_that_cannot_be_resolved_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows along what the rule generalises over — the FORM of the value: the two id lengths
    // EC2 issues (8 and 17 hex digits), the all-zero placeholder the AWS documentation itself
    // uses, another resource's prefix, a bare prefix, a suffix of the wrong length, a non-hex
    // suffix, an uppercase suffix, the id of a target network ASSOCIATION, and an empty
    // value. None of them names an endpoint of this account, so all are refused alike.
    let cases: [&str; 11] = [
        "cvpn-endpoint-0123abcd",
        "cvpn-endpoint-0123456789abcdef0",
        "cvpn-endpoint-00000000000000000",
        "vpc-00000000000000001",
        "cvpn-endpoint",
        "cvpn-endpoint-",
        "cvpn-endpoint-0123456789abcdef",
        "cvpn-endpoint-0123456789abcdefg",
        "cvpn-endpoint-0123456789ABCDEF0",
        "cvpn-assoc-00000000000000001",
        "",
    ];
    for endpoint_id in cases {
        let expected_code = "InvalidClientVpnEndpointId.NotFound";
        // The message the refusal carries, as the Client VPN surface words it: it names the
        // id the request sent.
        let expected_message = format!("Endpoint {endpoint_id} does not exist");
        // Alone, and beside an id the account DOES hold: an unresolvable id is refused
        // either way rather than quietly dropped from the result.
        let alone = indexed(ENDPOINT_IDS_PARAM, &[endpoint_id]);
        let beside = indexed(ENDPOINT_IDS_PARAM, &[seeded[0].as_str(), endpoint_id]);
        for (position, params) in [("alone", alone), ("beside a known id", beside)] {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 400,
                "case {endpoint_id:?} {position}: expected HTTP 400, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {endpoint_id:?} {position}: expected {expected_code}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {endpoint_id:?} {position}: expected the message \
                 {expected_message:?}, got {body}"
            );
        }
    }

    // The endpoints the account DOES hold are still reported, so the refusals above are
    // about the id rather than about the operation.
    let reported = described_ids(&router, "the stored endpoints", &[]).await;
    assert_eq!(
        reported, seeded,
        "the stored endpoints: expected {seeded:?}, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_endpoints_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // One row per filter name `DescribeClientVpnEndpointsRequest$Filters` documents, with
    // one and with several values, and with a value no endpoint holds.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("endpoint-id", vec![seeded[4].clone()], vec![4]),
        (
            "endpoint-id",
            vec![seeded[1].clone(), seeded[6].clone()],
            vec![1, 6],
        ),
        (
            "endpoint-id",
            vec!["cvpn-endpoint-00000000000000000".to_string()],
            Vec::new(),
        ),
        ("transport-protocol", vec!["tcp".to_string()], vec![1, 3]),
        (
            "transport-protocol",
            vec!["udp".to_string()],
            vec![0, 2, 4, 6],
        ),
        (
            // Both documented protocols: every endpoint that carries one, which is not every
            // endpoint stored.
            "transport-protocol",
            vec!["tcp".to_string(), "udp".to_string()],
            vec![0, 1, 2, 3, 4, 6],
        ),
        (
            // "Filter names and values are case-sensitive"
            // (`DescribeClientVpnEndpointsRequest$Filters`).
            "transport-protocol",
            vec!["TCP".to_string()],
            Vec::new(),
        ),
        (
            // A protocol no endpoint speaks.
            "transport-protocol",
            vec!["sctp".to_string()],
            Vec::new(),
        ),
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

/// One row of the AND table: the filters to send, and the indices of the endpoints that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the endpoints matching every one are
/// reported — and `ClientVpnEndpointIds` narrows the same way, so the two compose.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let first = seeded[0].clone();
    let second = seeded[1].clone();
    // Bound here rather than inline: the table below borrows these slices, so they must
    // outlive it.
    let first_only = [first.as_str()];
    let first_two = [first.as_str(), second.as_str()];
    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("transport-protocol", &["udp"][..]),
                ("endpoint-id", &first_only[..]),
            ],
            vec![0],
        ),
        (
            // The two filters select disjoint sets, so their AND is empty.
            vec![
                ("transport-protocol", &["tcp"][..]),
                ("endpoint-id", &first_only[..]),
            ],
            Vec::new(),
        ),
        (
            vec![
                ("endpoint-id", &first_two[..]),
                ("transport-protocol", &["tcp", "udp"][..]),
            ],
            vec![0, 1],
        ),
        (
            // The same filter twice: an AND of a condition with itself.
            vec![
                ("transport-protocol", &["tcp"][..]),
                ("transport-protocol", &["tcp"][..]),
            ],
            vec![1, 3],
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

    // A filter and an id list compose: the id list narrows to two endpoints, the filter to
    // the one of them speaking tcp.
    let mut params = id_params(&seeded, &[0, 1]);
    params.extend(filters(&[("transport-protocol", &["tcp"])]));
    let reported = described_ids(&router, "ids and a filter", &params).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[1]),
        "ids and a filter: expected the tcp endpoint of the two named, got {reported:?}"
    );
}

/// A filter name this operation does not document is refused with EC2's `InvalidFilter`,
/// rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: a near-miss of a documented name, the underscore spelling, a documented name
    // with the wrong case, the plural, members of the described shape the `Filters` doc does
    // NOT name, a tag prefix with no key, and an empty name. The `tag:<key>` / `tag-key` pair
    // is SERVED — the endpoint carries tags — so it is covered by
    // `describe_filters_on_reported_members` instead.
    let cases = [
        "endpoint",
        "endpoint_id",
        "Endpoint-Id",
        "endpoint-ids",
        "client-vpn-endpoint-id",
        "transport-protocols",
        "vpc-id",
        "status.code",
        "tag:",
        "tag-keys",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
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

    // The documented names are still served, so the refusals above are about the name.
    let reported = described_ids(
        &router,
        "a documented filter",
        &filters(&[("transport-protocol", &["tcp"])]),
    )
    .await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[1, 3]),
        "a documented filter: expected the tcp endpoints, got {reported:?}"
    );
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
    // number of endpoint ids — each at the crate's cap (`ec2::state`) and one past it. The
    // id rows name ids the account does not hold, so what the cap refuses is the LIST's
    // size: at the cap the request is read far enough to resolve an id, which is the 400 the
    // unknown-id rule gives, so the accepted case is asserted as "not the cap's refusal".
    let many_filters = |count: usize| -> Vec<(String, String)> {
        let entries: Vec<(&str, &[&str])> = (0..count)
            .map(|_| ("transport-protocol", &["udp"][..]))
            .collect();
        filters(&entries)
    };
    let many_values = |count: usize| -> Vec<(String, String)> {
        let values: Vec<String> = (0..count).map(|index| format!("udp-{index}")).collect();
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        filters(&[("transport-protocol", &borrowed)])
    };
    let long_value = |bytes: usize| -> Vec<(String, String)> {
        let value = "v".repeat(bytes);
        filters(&[("transport-protocol", &[value.as_str()])])
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
    ];
    for (case, extra, expected_status) in cases {
        let (status, body) = describe(&router, &extra).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
    }

    // The id list's own cap: at it, the request is refused for the UNKNOWN id it names
    // rather than for its size; one past it, for the size.
    let many_ids = |count: usize| -> Vec<(String, String)> {
        let ids: Vec<String> = (0..count)
            .map(|index| format!("cvpn-endpoint-{index:017x}"))
            .collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        indexed(ENDPOINT_IDS_PARAM, &borrowed)
    };
    let id_cases = [
        (
            ec2::state::MAX_RESOURCE_IDS,
            "InvalidClientVpnEndpointId.NotFound",
        ),
        (ec2::state::MAX_RESOURCE_IDS + 1, "InvalidParameterValue"),
    ];
    for (count, expected_code) in id_cases {
        let (status, body) = describe(&router, &many_ids(count)).await;
        assert_eq!(
            status, 400,
            "case {count} endpoint ids: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {count} endpoint ids: expected {expected_code}, got {body}"
        );
    }

    // The store is untouched by every refusal above.
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "a refused request must leave the store exactly as it was"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every endpoint exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_endpoint_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeClientVpnEndpointMaxResults` is modelled `range 5..1000`, so the rows are its
    // published lower bound, a size that divides the set exactly, one that does not, and the
    // upper bound, which holds everything.
    for max_results in [5, 6, 7, 1000] {
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
            walked.extend(elements(&body, ID_ELEMENT));
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
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
            walked, seeded,
            "case {case:?}: expected every endpoint exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store: `MaxResults` and a filter
/// compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Six of the seven endpoints carry a transport protocol, so the model's minimum page
    // size cuts that filtered set into two pages.
    let page_params = || {
        let mut params = filters(&[("transport-protocol", &["tcp", "udp"])]);
        params.extend(max_results_param(5));
        params
    };

    let (status, body) = describe(&router, &page_params()).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, ID_ELEMENT),
        ids_at(&seeded, &[0, 1, 2, 3, 4]),
        "filtered first page: expected the first five endpoints carrying a protocol, got {body}"
    );
    let token = element(&body, "nextToken")
        .expect("filtered first page: a sixth endpoint follows, so a token is minted");

    let mut params = page_params();
    params.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &params).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[6]),
        "filtered second page: expected the remaining endpoint carrying a protocol, got \
         {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let _seeded = seed(&engine);

    // The published bounds (`DescribeClientVpnEndpointMaxResults`, `range 5..1000`) and one
    // past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor — and before the endpoint ids are resolved, so the refusal does not
/// depend on the rest of the request.
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
        seeded[0].clone(),
        ::hex::encode(&seeded[0]),
    ];
    for token in cases {
        let mut params = vec![("NextToken".to_string(), token.clone())];
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

        // The same token beside an endpoint id that names nothing: the token is still what
        // is refused.
        params.extend(indexed(
            ENDPOINT_IDS_PARAM,
            &["cvpn-endpoint-00000000000000000"],
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
/// nothing, whatever ids the request names; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: no id at all, an endpoint the account holds, and ids it does not — a permission
    // probe is answered before the ids are resolved, so a caller checking its permissions
    // gets the same answer whether or not the endpoint exists.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("no id", Vec::new()),
        ("a stored endpoint", id_params(&seeded, &[0])),
        (
            "an unknown endpoint",
            indexed(ENDPOINT_IDS_PARAM, &["cvpn-endpoint-00000000000000000"]),
        ),
        (
            "a malformed endpoint id",
            indexed(ENDPOINT_IDS_PARAM, &["not-an-endpoint-id"]),
        ),
    ];
    for (case, extra) in cases {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        params.extend(extra);
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            elements(&body, ID_ELEMENT).is_empty(),
            "case {case:?}: a dry run reports no endpoint, got {body}"
        );
    }

    // `DryRun=false` is an ordinary call, and the store is exactly as it was.
    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded,
        "DryRun=false: expected {seeded:?}, got {reported:?}"
    );
    assert_eq!(
        stored_ids(&engine),
        seeded,
        "describing must neither add nor drop an endpoint"
    );
}

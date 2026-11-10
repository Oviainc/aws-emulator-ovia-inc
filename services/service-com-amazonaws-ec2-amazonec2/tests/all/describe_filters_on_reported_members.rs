// SPDX-License-Identifier: CC0-1.0

//! The filters a describe serves for the MEMBERS of the records it reports, over the wire the
//! service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML;
//! README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a describe narrows on the members of the records it reports, so the
//! filter names it serves include the reported record's OWN identifier and — for a record that
//! carries tags — the `tag:<key>` / `tag-key` pair, whether or not that operation's own
//! documentation enumerates them. A name no member of the reported record answers is still
//! refused, so an unsupported filter can never read as "no results".
//!
//! The rows span what that generalises over: two operations whose documentation omits such a
//! filter (`DescribeClientVpnEndpoints`, whose reported `ClientVpnEndpoint` carries `Tags`, and
//! `DescribeCapacityReservationBillingRequests`, whose reported record is keyed by a
//! `CapacityReservationId`), each driven by the filter name, the number of values, a matching
//! and a non-matching value, a wildcard, and the documented filters beside them — because the
//! rule is about which member a filter selects on, not about one request. The tag family is
//! the crate's one resolver (`state::is_tag_filter_name` / `state::tag_filter_values`), so
//! `tag:<key>`, `tag-key` and `tag-value` are all exercised.

use std::time::{Duration, UNIX_EPOCH};

use ec2::ops::types::{ClientVpnEndpoint, Tag};
use ec2::state::{
    CapacityReservationBillingRequestSpec, ComAmazonawsEc2Amazonec2State as Ec2State,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call, elements, error_code, filters, indexed};

/// The Region every call here is signed for. Neither store is per Region, so the Region only
/// selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The code EC2 answers for a filter name an operation does not serve: `DescribeClientVpnEndpoints`
/// states the set it serves ([`ec2::state::invalid_filter`]), while the billing-request describe
/// names the filter.
const INVALID_FILTER: &str = "InvalidFilter";

/// The code the billing-request describe answers for a filter name it does not serve.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

// ---------------------------------------------------------------------------
// DescribeClientVpnEndpoints: a reported resource that carries tags
// ---------------------------------------------------------------------------

/// One seeded Client VPN endpoint: its transport protocol and the tags it carries.
struct EndpointRow {
    transport_protocol: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The endpoints the Client VPN table is written against: they differ along the tag KEYS they
/// carry, the tag VALUES under one key, and how many tags they carry at all — which is what a
/// tag filter selects on — and along the documented `transport-protocol` beside it.
fn endpoint_rows() -> Vec<EndpointRow> {
    vec![
        EndpointRow {
            transport_protocol: "udp",
            tags: &[("Name", "first"), ("Team", "network")],
        },
        EndpointRow {
            transport_protocol: "tcp",
            tags: &[("Name", "second")],
        },
        EndpointRow {
            transport_protocol: "tcp",
            tags: &[("Team", "network")],
        },
        EndpointRow {
            transport_protocol: "udp",
            tags: &[],
        },
    ]
}

/// Write the endpoints into the store and answer the ids they were minted with, in row order.
fn seed_endpoints(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    endpoint_rows()
        .iter()
        .map(|row| {
            guard
                .insert_client_vpn_endpoint(ClientVpnEndpoint {
                    transport_protocol: Some(row.transport_protocol.to_string()),
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
                .expect("the store must take the Client VPN endpoint")
        })
        .collect()
}

/// The ids at `indices`, sorted — the order the describe pages in.
fn ids_at(ids: &[String], indices: &[usize]) -> Vec<String> {
    let mut selected: Vec<String> = indices.iter().map(|index| ids[*index].clone()).collect();
    selected.sort();
    selected
}

/// One row of a filter table: the case, the `(name, values)` filters the request carries, and
/// the indices of the seeded records that must be reported.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// The tag filters a Client VPN endpoint answers select on the tags it carries, beside the two
/// filters the operation's own documentation enumerates.
#[tokio::test]
async fn client_vpn_endpoints_are_narrowed_by_the_tags_they_carry() {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed_endpoints(&engine);

    // (the case, the filters, the rows that must be reported)
    let rows: Vec<FilterCase<'_>> = vec![
        ("one tag key and value", vec![("tag:Name", &["first"][..])], vec![0]),
        (
            "a tag value two endpoints share",
            vec![("tag:Team", &["network"][..])],
            vec![0, 2],
        ),
        (
            "two values for one tag key",
            vec![("tag:Name", &["first", "second"][..])],
            vec![0, 1],
        ),
        (
            "a tag key, whatever its value",
            vec![("tag-key", &["Team"][..])],
            vec![0, 2],
        ),
        (
            "two tag keys",
            vec![("tag-key", &["Name", "Team"][..])],
            vec![0, 1, 2],
        ),
        (
            "a tag value nothing carries",
            vec![("tag:Name", &["nobody"][..])],
            vec![],
        ),
        (
            "a tag value under any key",
            vec![("tag-value", &["network"][..])],
            vec![0, 2],
        ),
        (
            "a tag key nothing carries",
            vec![("tag:Owner", &["anyone"][..])],
            vec![],
        ),
        (
            "a wildcard over a tag value",
            vec![("tag:Name", &["*s*"][..])],
            vec![0, 1],
        ),
        (
            "a tag filter AND a documented one",
            vec![("tag:Team", &["network"][..]), ("transport-protocol", &["tcp"][..])],
            vec![2],
        ),
        (
            "the documented filter alone",
            vec![("transport-protocol", &["udp"][..])],
            vec![0, 3],
        ),
    ];

    for (case, entries, expected) in rows {
        let params = filters(&entries);
        let (status, body) = call(&router, REGION, "DescribeClientVpnEndpoints", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filter on a member of the reported endpoint must be served, got \
             HTTP {status} with body {body}"
        );
        let mut reported = elements(&body, "clientVpnEndpointId");
        reported.sort();
        assert_eq!(
            reported,
            ids_at(&ids, &expected),
            "case {case:?}: expected the endpoints the filter selects, got {reported:?} from \
             {body}"
        );
    }
}

/// The endpoint-id filter and the id list select the same endpoint, and a name no member of the
/// reported endpoint answers is still refused.
#[tokio::test]
async fn an_unserved_client_vpn_filter_name_is_still_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed_endpoints(&engine);

    // The documented id filter still selects, and so does the id list beside it.
    for (case, params) in [
        (
            "the endpoint-id filter",
            filters(&[("endpoint-id", &[ids[1].as_str()])]),
        ),
        (
            "the ClientVpnEndpointIds list",
            indexed("ClientVpnEndpointId", &[ids[1].as_str()]),
        ),
    ] {
        let (status, body) = call(&router, REGION, "DescribeClientVpnEndpoints", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "clientVpnEndpointId"),
            vec![ids[1].clone()],
            "case {case:?}: expected the one endpoint named, got {body}"
        );
    }

    // A name no member answers, and a tag prefix with no key, are refused rather than read as
    // "no results" — and the tag family is case-sensitive, like every other filter name.
    for case in ["vpc-id", "tag:", "tag-keys", "TAG-KEY", "Tag:Name"] {
        let params = filters(&[(case, &["anything"])]);
        let (status, body) = call(&router, REGION, "DescribeClientVpnEndpoints", &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, INVALID_FILTER),
            "case {case:?}: a filter name no member of the reported endpoint answers must be \
             refused, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeCapacityReservationBillingRequests: the reported record's own id
// ---------------------------------------------------------------------------

/// The account every seeded billing request is initiated by: the emulator's own, so the
/// `odcr-owner` role reports each of them.
fn calling_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Write three billing requests the calling account initiated and answer their Capacity
/// Reservation ids, in store order.
fn seed_billing_requests(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let statuses = ["pending", "accepted", "pending"];
    let mut ids = Vec::new();
    for (index, status) in statuses.iter().enumerate() {
        let capacity_reservation_id = guard.next_id("cr");
        let stored = guard.insert_capacity_reservation_billing_request(
            CapacityReservationBillingRequestSpec {
                capacity_reservation_id: capacity_reservation_id.clone(),
                requested_by: calling_account(),
                unused_reservation_billing_owner_id: "210987654321".to_string(),
                last_update_time: UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64),
                status: match *status {
                    "accepted" => "accepted",
                    _ => "pending",
                },
                status_message: None,
                capacity_reservation_info: None,
            },
        );
        assert!(stored, "the store must take the billing request");
        ids.push(capacity_reservation_id);
    }
    ids
}

/// A billing request is narrowed by the Capacity Reservation id it is keyed by, the same id its
/// `CapacityReservationIds` list names.
#[tokio::test]
async fn billing_requests_are_narrowed_by_the_reservation_id_they_report() {
    let (router, engine) = super::ec2_query::emulator();
    let ids = seed_billing_requests(&engine);
    let role = vec![("Role".to_string(), "odcr-owner".to_string())];

    // (the case, the filter values, the rows that must be reported)
    let rows: Vec<(&str, Vec<&str>, Vec<usize>)> = vec![
        ("one id", vec![ids[0].as_str()], vec![0]),
        (
            "two ids",
            vec![ids[0].as_str(), ids[2].as_str()],
            vec![0, 2],
        ),
        ("an id no request is held for", vec!["cr-0fedcba9876543210"], vec![]),
        ("a wildcard over every id", vec!["cr-*"], vec![0, 1, 2]),
    ];

    for (case, values, expected) in rows {
        let mut params = role.clone();
        params.extend(filters(&[("capacity-reservation-id", &values)]));
        let (status, body) = call(
            &router,
            REGION,
            "DescribeCapacityReservationBillingRequests",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filter on the reported record's own id must be served, got HTTP \
             {status} with body {body}"
        );
        let mut reported = elements(&body, "capacityReservationId");
        reported.sort();
        assert_eq!(
            reported,
            ids_at(&ids, &expected),
            "case {case:?}: expected the requests the filter selects, got {reported:?} from \
             {body}"
        );
    }

    // The id filter and the documented `status` filter are joined with an AND, and a name no
    // member of the reported record answers is still refused.
    let mut both = role.clone();
    both.extend(filters(&[
        ("capacity-reservation-id", &["cr-*"]),
        ("status", &["accepted"]),
    ]));
    let (status, body) = call(
        &router,
        REGION,
        "DescribeCapacityReservationBillingRequests",
        &both,
    )
    .await;
    assert_eq!(
        status, 200,
        "the two filters together must be served, got HTTP {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "capacityReservationId"),
        vec![ids[1].clone()],
        "expected only the accepted request, got {body}"
    );

    for name in ["instance-type", "tag:Name", "tag-key", "capacity-reservation-ids"] {
        let mut params = role.clone();
        params.extend(filters(&[(name, &["anything"])]));
        let (status, body) = call(
            &router,
            REGION,
            "DescribeCapacityReservationBillingRequests",
            &params,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, INVALID_PARAMETER_VALUE),
            "case {name:?}: a filter name no member of the reported record answers must be \
             refused, got {status} with body {body}"
        );
    }
}

/// Neither describe writes: the same request answers the same rows twice, and the stores hold
/// what they held.
#[tokio::test]
async fn filtered_describes_write_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let endpoints = seed_endpoints(&engine);
    let requests = seed_billing_requests(&engine);

    for _ in 0..2 {
        let (status, body) = call(
            &router,
            REGION,
            "DescribeClientVpnEndpoints",
            &filters(&[("tag-key", &["Name"])]),
        )
        .await;
        let mut reported = elements(&body, "clientVpnEndpointId");
        reported.sort();
        assert_eq!(
            (status, reported),
            (200, ids_at(&endpoints, &[0, 1])),
            "a read must report the same endpoints every time, got {body}"
        );
    }

    let held: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .client_vpn_endpoints
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        held.len(),
        endpoints.len(),
        "a read must leave the endpoint store as it was, got {held:?}"
    );
    let billing: usize = engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservation_billing_requests
        .len();
    assert_eq!(
        billing,
        requests.len(),
        "a read must leave the billing-request store as it was"
    );
}

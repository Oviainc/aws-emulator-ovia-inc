// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeCapacityReservationTopology`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=DescribeCapacityReservationTopology`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Four rules are under test:
//!
//! 1. The report is COMPUTED FROM THE STORED RESERVATIONS of the Region the request was
//!    signed for: one row per reservation, every member of it read off that reservation and
//!    off the clock, narrowed by the ids and the two documented filters the request names
//!    and cut into `MaxResults`-sized pages.
//! 2. A row's `NetworkNodes` are the PATH THROUGH THE TOPOLOGY TREE the reservation's
//!    capacity hangs off: hashed per account, stable across reads, and shared between two
//!    reservations exactly down to the level at which their placements stop agreeing — and
//!    absent for a reservation in a state other than `active` or `pending`, which holds no
//!    capacity on any host.
//! 3. A request outside a documented rule is REFUSED with EC2's own code for it.
//! 4. The report is a READ: repeated calls answer the same thing, and what every other
//!    reader of a Capacity Reservation reports is left alone.
//!
//! Every table row is a distinct input along what those rules generalise over — the instance
//! type and family, which member names the Availability Zone, the platform, the instance
//! count, the tenancy, the zone itself, the Region, the filter and its wildcards, the page
//! size, and the state the reservation is in — so no single recorded request is the
//! specification.
//!
//! Every response is read with the XML parser rather than by substring, so a member of one
//! row is never confused with an identically named member of another row's node set.

use std::time::{Duration, SystemTime};

use ec2::state::{availability_zones_in_region, iso8601_utc};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, filters, indexed};
use super::wire::{child_text, document, is_long_id, set_items};

/// The Region every call here is signed for unless the case says otherwise. A Capacity
/// Reservation reserves capacity in one Availability Zone, so the Region decides both which
/// zones a create may name and which reservations the topology reports.
const REGION: &str = "us-east-1";

/// The three Availability Zones of [`REGION`] the seeds reserve capacity in. Each one is
/// resolved against the Region's own zone catalog by [`zone_id_of`], which panics on a zone
/// the Region does not have — so these names are held to that catalog rather than merely
/// written here.
const ZONES: [&str; 3] = ["us-east-1a", "us-east-1b", "us-east-1c"];

/// How many levels the reported path through the topology tree has: the node of the
/// Availability Zone's network, the hosts of that zone serving the reservation's instance
/// type, and the hosts its own capacity is on.
const TOPOLOGY_DEPTH: usize = 3;

/// One row of the filter table: what it covers, the `Filters` entries it sends, and the
/// reservation ids those must select.
type FilterRow = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<String>,
);

/// One row of the refusal table: what it covers, the form fields it sends, and the error
/// code and HTTP status it must be answered with.
type RefusalRow = (&'static str, Vec<(String, String)>, &'static str, u16);

/// One form field.
fn field(name: &str, value: impl ToString) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// Form fields from `(name, value)` pairs.
fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One `DescribeCapacityReservationTopology` call in [`REGION`].
async fn topology(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeCapacityReservationTopology",
        params,
    )
    .await
}

/// One `CreateCapacityReservation` call signed for `region`, as the id it minted.
async fn reserve_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) =
        super::ec2_query::call(router, region, "CreateCapacityReservation", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "capacityReservationId")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: the create must report an id, got {body}"))
        .to_string()
}

/// One `CreateCapacityReservation` call in [`REGION`].
async fn reserve(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    reserve_in(router, REGION, case, params).await
}

/// One reported topology row, as the response wrote it. An absent optional member is `None`
/// and an empty one is `Some("")`, which is the difference an assertion on a member a
/// reservation does not carry has to see.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    capacity_reservation_id: String,
    capacity_block_id: Option<String>,
    state: String,
    instance_type: String,
    group_name: Option<String>,
    network_nodes: Vec<String>,
    availability_zone: String,
    availability_zone_id: String,
}

/// The rows a successful response carries, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Row> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "capacityReservationSet")
        .into_iter()
        .map(|item| Row {
            capacity_reservation_id: child_text(item, "capacityReservationId").unwrap_or_default(),
            capacity_block_id: child_text(item, "capacityBlockId"),
            state: child_text(item, "state").unwrap_or_default(),
            instance_type: child_text(item, "instanceType").unwrap_or_default(),
            group_name: child_text(item, "groupName"),
            network_nodes: set_items(item, "networkNodeSet")
                .into_iter()
                .map(|node| node.text().unwrap_or_default().to_string())
                .collect(),
            availability_zone: child_text(item, "availabilityZone").unwrap_or_default(),
            availability_zone_id: child_text(item, "availabilityZoneId").unwrap_or_default(),
        })
        .collect()
}

/// The rows of a call that must succeed.
async fn reported_ok(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Row> {
    let (status, body) = topology(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported(case, &body)
}

/// The ids a call that must succeed reported, in order.
async fn reported_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    reported_ok(router, case, params)
        .await
        .into_iter()
        .map(|row| row.capacity_reservation_id)
        .collect()
}

/// The states `DescribeCapacityReservations` reports for every reservation of [`REGION`], in
/// the order it reports them — the "nothing moved" reading rule 4 compares.
async fn described_states(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeCapacityReservations", &[]).await;
    assert_eq!(
        status, 200,
        "the describe must succeed, got {status} with body {body}"
    );
    let parsed = document("the reservations themselves", &body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "state")
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// The id of the Availability Zone named `zone_name`, read out of the Region's own zone
/// catalog (`ec2::state::availability_zones_in_region`) rather than written out here, so the
/// expectation follows the one definition of a zone's two spellings.
fn zone_id_of(region: &str, zone_name: &str) -> String {
    availability_zones_in_region(region)
        .into_iter()
        .find(|zone| zone.zone_name == zone_name)
        .map(|zone| zone.zone_id)
        .unwrap_or_else(|| panic!("{region} must have the Availability Zone {zone_name}"))
}

/// A reservation written straight into the store whose `EndDate` has already passed.
///
/// A create refuses an end date that is not in the future, so this is the one state a
/// reservation cannot be driven into over the wire; seeding the record is what lets the
/// expiry be observed from the dates the record carries rather than from a sleep racing
/// them. Every other member is an ordinary one, so the row the report answers differs from
/// a live reservation's in its state alone.
fn seed_expired(engine: &engine::InMemoryEngine, zone_name: &str) -> String {
    let now = SystemTime::now();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .insert_capacity_reservation(ec2::state::CapacityReservationSpec {
            instance_type: "m5.large".to_string(),
            instance_platform: "Linux/UNIX".to_string(),
            availability_zone: zone_name.to_string(),
            availability_zone_id: zone_id_of(REGION, zone_name),
            region: REGION.to_string(),
            tenancy: ec2::state::DEFAULT_INSTANCE_TENANCY.to_string(),
            instance_count: 1,
            ebs_optimized: false,
            ephemeral_storage: false,
            end_date: Some(now - Duration::from_secs(60)),
            instance_match_criteria: ec2::state::DEFAULT_INSTANCE_MATCH_CRITERIA.to_string(),
            created_at: now - Duration::from_secs(600),
            start_date: None,
            commitment: None,
            delivery_preference: None,
            state: ec2::state::CAPACITY_RESERVATION_STATE_ACTIVE,
            tags: Vec::new(),
            interruption_info: None,
            zero_size_preference: None,
            pending: None,
        })
        .unwrap_or_else(|| panic!("the store must accept one seeded reservation"))
}

/// One seeded reservation of the table below: the create it is made by, and what the
/// topology must then report for it.
struct Seed {
    case: &'static str,
    params: Vec<(String, String)>,
    instance_type: &'static str,
    zone_name: &'static str,
}

/// The reservations rules 1 and 2 are asserted over.
///
/// The rows differ along everything those rules generalise over: the instance type and its
/// family — entries of the modelled `InstanceType` catalog
/// (`smithy_query(query="shape",shape="InstanceType")`) — the platform, the instance count,
/// the tenancy, the match criteria, which Availability Zone of the Region holds the capacity,
/// and whether the create named that zone by its name or by its id. Two rows share a zone AND
/// an instance type, and two more share only a zone, which is what makes the levels of the
/// tree observable.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            case: "a general purpose type, the zone named by name",
            params: fields(&[
                ("InstanceType", "m5.large"),
                ("InstancePlatform", "Linux/UNIX"),
                ("InstanceCount", "2"),
                ("AvailabilityZone", ZONES[0]),
            ]),
            instance_type: "m5.large",
            zone_name: ZONES[0],
        },
        Seed {
            case: "an accelerated type, the zone named by id, dedicated tenancy",
            params: {
                let mut params = fields(&[
                    ("InstanceType", "p4d.24xlarge"),
                    ("InstancePlatform", "Windows"),
                    ("InstanceCount", "4"),
                    ("Tenancy", "dedicated"),
                    ("InstanceMatchCriteria", "targeted"),
                ]);
                params.push(field("AvailabilityZoneId", zone_id_of(REGION, ZONES[1])));
                params
            },
            instance_type: "p4d.24xlarge",
            zone_name: ZONES[1],
        },
        Seed {
            case: "a Graviton type in a third zone",
            params: fields(&[
                ("InstanceType", "r6g.medium"),
                ("InstancePlatform", "SUSE Linux"),
                ("InstanceCount", "8"),
                ("AvailabilityZone", ZONES[2]),
            ]),
            instance_type: "r6g.medium",
            zone_name: ZONES[2],
        },
        Seed {
            case: "a second reservation of the first zone and instance type",
            params: fields(&[
                ("InstanceType", "m5.large"),
                ("InstancePlatform", "Red Hat Enterprise Linux"),
                ("InstanceCount", "1"),
                ("AvailabilityZone", ZONES[0]),
            ]),
            instance_type: "m5.large",
            zone_name: ZONES[0],
        },
        Seed {
            case: "another instance type in the first zone",
            params: fields(&[
                ("InstanceType", "c5.xlarge"),
                ("InstancePlatform", "Ubuntu Pro"),
                ("InstanceCount", "3"),
                ("AvailabilityZone", ZONES[0]),
            ]),
            instance_type: "c5.xlarge",
            zone_name: ZONES[0],
        },
    ]
}

/// Seed [`seeds`] into `router` and answer the id each row minted, in table order.
async fn seed_all(router: &MultiServiceRouter) -> Vec<String> {
    let mut ids = Vec::new();
    for row in seeds() {
        ids.push(reserve(router, row.case, &row.params).await);
    }
    ids
}

/// Rule 1: one row per stored reservation of the Region, every member of it read off that
/// reservation — and rule 2's shape of a node id.
#[tokio::test]
async fn the_topology_reports_each_stored_reservation_of_the_region() {
    let (router, _engine) = super::ec2_query::emulator();
    let table = seeds();
    let ids = seed_all(&router).await;

    // A reservation of ANOTHER Region: a reservation belongs to the Region of the zone its
    // capacity is in, so it must not be reported here.
    let elsewhere = reserve_in(
        &router,
        "eu-west-2",
        "a reservation of another Region",
        &fields(&[
            ("InstanceType", "c5.xlarge"),
            ("InstancePlatform", "Linux/UNIX"),
            ("InstanceCount", "1"),
            ("AvailabilityZone", "eu-west-2a"),
        ]),
    )
    .await;

    let rows = reported_ok(&router, "every reservation of the Region", &[]).await;
    let mut expected_ids = ids.clone();
    expected_ids.sort();
    let got_ids: Vec<String> = rows
        .iter()
        .map(|row| row.capacity_reservation_id.clone())
        .collect();
    assert_eq!(
        got_ids, expected_ids,
        "expected every reservation of {REGION} in id order, got {got_ids:?}"
    );
    assert!(
        !got_ids.contains(&elsewhere),
        "expected the reservation of another Region to be left out, got {got_ids:?}"
    );

    for (seed, id) in table.iter().zip(ids.iter()) {
        let case = seed.case;
        let row = rows
            .iter()
            .find(|row| &row.capacity_reservation_id == id)
            .unwrap_or_else(|| panic!("case {case:?}: expected a row for {id}, got {rows:?}"));
        assert_eq!(
            row.instance_type, seed.instance_type,
            "case {case:?}: expected the reservation's instance type, got {:?}",
            row.instance_type
        );
        assert_eq!(
            row.availability_zone, seed.zone_name,
            "case {case:?}: expected the reservation's zone, got {:?}",
            row.availability_zone
        );
        assert_eq!(
            row.availability_zone_id,
            zone_id_of(REGION, seed.zone_name),
            "case {case:?}: expected that zone's id, got {:?}",
            row.availability_zone_id
        );
        assert_eq!(
            row.state, "active",
            "case {case:?}: expected the state the clock has the reservation in, got {:?}",
            row.state
        );
        // "The ID of the Capacity Block … only supported for UltraServer instances", and
        // "the name of the placement group that the Capacity Reservation is in": a
        // reservation that is in neither carries neither.
        assert_eq!(
            row.capacity_block_id, None,
            "case {case:?}: expected no Capacity Block on a reservation of none, got {:?}",
            row.capacity_block_id
        );
        assert_eq!(
            row.group_name, None,
            "case {case:?}: expected no placement group on a reservation in none, got {:?}",
            row.group_name
        );
        assert_eq!(
            row.network_nodes.len(),
            TOPOLOGY_DEPTH,
            "case {case:?}: expected {TOPOLOGY_DEPTH} network nodes, got {:?}",
            row.network_nodes
        );
        for node in &row.network_nodes {
            assert!(
                is_long_id("nn", node),
                "case {case:?}: expected a network node id of the form nn-<17 hex>, got {node:?}"
            );
        }
    }
}

/// Rule 2: the path is shared exactly down to the level at which two reservations stop
/// sharing a placement, and it is the same on every read.
#[tokio::test]
async fn the_node_path_is_shared_exactly_where_the_placement_is() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_all(&router).await;
    let rows = reported_ok(&router, "the whole Region", &[]).await;
    let nodes = |id: &str| -> Vec<String> {
        rows.iter()
            .find(|row| row.capacity_reservation_id == id)
            .map(|row| row.network_nodes.clone())
            .unwrap_or_else(|| panic!("expected a row for {id}, got {rows:?}"))
    };

    // (case, the two reservations, how many leading levels their placements share). The rows
    // walk every level the tree has: a zone and an instance type in common, a zone only,
    // nothing at all, and a reservation against itself.
    let cases: Vec<(&str, &str, &str, usize)> = vec![
        (
            "the same zone and instance type share every level but the hosts",
            &ids[0],
            &ids[3],
            TOPOLOGY_DEPTH - 1,
        ),
        (
            "the same zone with another instance type shares the zone's node only",
            &ids[0],
            &ids[4],
            1,
        ),
        ("another zone shares nothing", &ids[1], &ids[2], 0),
        (
            "a reservation shares its whole path with itself",
            &ids[2],
            &ids[2],
            TOPOLOGY_DEPTH,
        ),
    ];

    for (case, left_id, right_id, shared) in cases {
        let left = nodes(left_id);
        let right = nodes(right_id);
        let common = left
            .iter()
            .zip(right.iter())
            .take_while(|(left, right)| left == right)
            .count();
        assert_eq!(
            common, shared,
            "case {case:?}: expected {shared} shared leading nodes, got {common} \
             ({left:?} against {right:?})"
        );
        if left_id != right_id {
            assert_ne!(
                left.last(),
                right.last(),
                "case {case:?}: expected two reservations to sit on different hosts, \
                 got {left:?} against {right:?}"
            );
        }
    }

    // The path is DERIVED from the placement, so a second read reports it unchanged.
    let again = reported_ok(&router, "a second read", &[]).await;
    assert_eq!(
        again, rows,
        "expected a second read to report the same topology, got {again:?}"
    );
}

/// Rule 1's narrowing half: the ids and the two documented filters select the rows.
#[tokio::test]
async fn the_topology_narrows_by_ids_and_by_every_documented_filter() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_all(&router).await;
    let (first, second, third, fourth, fifth) = (
        ids[0].clone(),
        ids[1].clone(),
        ids[2].clone(),
        ids[3].clone(),
        ids[4].clone(),
    );
    let sorted = |mut wanted: Vec<String>| -> Vec<String> {
        wanted.sort();
        wanted
    };

    // (case, the filter entries, the ids they must select). The rows walk both documented
    // filter names, each value form the doc gives — a whole instance type, an instance family
    // through the `*` wildcard, the `?` wildcard — and both ways values combine.
    let cases: Vec<FilterRow> = vec![
        (
            "instance-type, a whole type",
            vec![("instance-type", &["p4d.24xlarge"][..])],
            vec![second.clone()],
        ),
        (
            "instance-type, an instance family through the * wildcard",
            vec![("instance-type", &["m5*"][..])],
            sorted(vec![first.clone(), fourth.clone()]),
        ),
        (
            "instance-type, the ? wildcard inside a value",
            vec![("instance-type", &["?6g.medium"][..])],
            vec![third.clone()],
        ),
        (
            "availability-zone",
            vec![("availability-zone", &[ZONES[2]][..])],
            vec![third.clone()],
        ),
        (
            "availability-zone, the zone three reservations share",
            vec![("availability-zone", &[ZONES[0]][..])],
            sorted(vec![first.clone(), fourth.clone(), fifth.clone()]),
        ),
        (
            "two values of one filter are joined with OR",
            vec![("instance-type", &["p4d.24xlarge", "r6g.medium"][..])],
            sorted(vec![second.clone(), third.clone()]),
        ),
        (
            "two filters are joined with AND",
            vec![
                ("instance-type", &["m5.large"][..]),
                ("availability-zone", &[ZONES[1]][..]),
            ],
            Vec::new(),
        ),
        (
            "a filter no reservation matches",
            vec![("instance-type", &["c6i.large"][..])],
            Vec::new(),
        ),
    ];

    for (case, entries, want) in cases {
        let got = reported_ids(&router, case, &filters(&entries)).await;
        assert_eq!(got, want, "case {case:?}: expected {want:?}, got {got:?}");
    }

    // The ids the request names, which narrow the same way.
    let got = reported_ids(
        &router,
        "two of the five ids",
        &indexed("CapacityReservationId", &[&third, &first]),
    )
    .await;
    assert_eq!(
        got,
        sorted(vec![first.clone(), third.clone()]),
        "expected the two named reservations in id order, got {got:?}"
    );
}

/// Rule 3: each refusal answers EC2's own code for the rule the request broke, rather than
/// an empty report.
#[tokio::test]
async fn the_topology_refuses_what_it_cannot_answer() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_all(&router).await;

    // Eleven ids: one past the documented maximum of ten.
    let too_many: Vec<(String, String)> = (0..11)
        .map(|index| {
            field(
                &format!("CapacityReservationId.{}", index + 1),
                format!("cr-{:017x}", index + 1),
            )
        })
        .collect();

    // (case, the request's fields, the expected code, the expected status).
    let cases: Vec<RefusalRow> = vec![
        (
            "a filter name this operation does not document, though its sibling does",
            filters(&[("state", &["active"][..])]),
            "InvalidParameterValue",
            400,
        ),
        (
            "a filter name in the wrong case",
            filters(&[("Instance-Type", &["m5.large"][..])]),
            "InvalidParameterValue",
            400,
        ),
        (
            // A well-formed id — seventeen lowercase hex digits under a leading `0`, the
            // form `ComAmazonawsEc2Amazonec2State::next_id` mints — that no reservation of
            // the account was minted under.
            "an id no reservation of the account has",
            indexed("CapacityReservationId", &["cr-01234567890abcdef"]),
            "InvalidCapacityReservationId.NotFound",
            400,
        ),
        (
            // The all-zero id every client reaches for as a placeholder is not an id EC2
            // could have issued, so it is malformed rather than merely absent.
            "a value that is not a Capacity Reservation id at all",
            indexed("CapacityReservationId", &["cr-00000000000000000"]),
            "InvalidCapacityReservationId.Malformed",
            400,
        ),
        (
            "a NextToken this service did not issue",
            vec![field("NextToken", "not-a-token")],
            "InvalidPaginationToken",
            400,
        ),
        (
            "MaxResults beside the Capacity Reservation ids",
            {
                let mut params = indexed("CapacityReservationId", &[ids[0].as_str()]);
                params.push(field("MaxResults", 1));
                params
            },
            "InvalidParameterCombination",
            400,
        ),
        (
            "one more id than the documented maximum",
            too_many,
            "InvalidParameterValue",
            400,
        ),
        (
            "a dry run of a report that would have succeeded",
            vec![field("DryRun", "true")],
            "DryRunOperation",
            412,
        ),
    ];

    for (case, params, expected_code, expected_status) in cases {
        let (status, body) = topology(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?}"
        );
    }
}

/// Rule 1's paging half: `MaxResults` cuts the report into pages, `NextToken` walks them
/// without repeating or dropping a reservation, and a request that names no page size gets
/// the documented default of ten.
#[tokio::test]
async fn the_topology_pages_the_store() {
    let (router, _engine) = super::ec2_query::emulator();

    // One more reservation than the documented default page holds, so the default's own
    // bound is observable.
    let mut ids: Vec<String> = Vec::new();
    for count in 1..=11 {
        let params = vec![
            field("InstanceType", "m5.large"),
            field("InstancePlatform", "Linux/UNIX"),
            field("InstanceCount", count),
            field("AvailabilityZone", ZONES[0]),
        ];
        ids.push(reserve(&router, "a page of reservations", &params).await);
    }
    ids.sort();

    let first_page = reported_ids(&router, "no page size", &[]).await;
    assert_eq!(
        first_page,
        ids[..10].to_vec(),
        "expected the documented default of ten per page, got {first_page:?}"
    );

    // One row per page size: smaller than the store, and its published maximum.
    for page_size in [1_usize, 3, 10] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![field("MaxResults", page_size)];
            if let Some(token) = token.as_deref() {
                params.push(field("NextToken", token));
            }
            let (status, body) = topology(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} rows per page, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|row| row.capacity_reservation_id));
            token = super::ec2_query::member(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: expected the walk to finish, it has made {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: expected the walk to report every reservation once, in id order, \
             got {walked:?}"
        );
    }
}

/// Rule 2's other half: a reservation in a state other than `active` or `pending` holds no
/// capacity on any host, so it is reported with no network nodes at all — and a reservation
/// that holds a Capacity Block's capacity reports that block's id.
#[tokio::test]
async fn a_reservation_holding_no_capacity_reports_no_network_nodes() {
    let (router, engine) = super::ec2_query::emulator();

    // A Capacity Block purchase: its reservation is `payment-pending` until the payment
    // settles, and it carries the block it holds the capacity of.
    let case = "a Capacity Block purchase";
    let search = vec![
        field("InstanceType", "p5.48xlarge"),
        field("CapacityDurationHours", 24),
        field(
            "StartDateRange",
            iso8601_utc(SystemTime::now() + Duration::from_secs(2 * 60 * 60)),
        ),
        field("MaxResults", 1),
    ];
    let (status, body) =
        super::ec2_query::call(&router, REGION, "DescribeCapacityBlockOfferings", &search).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the search for an offering must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let offering_id = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "capacityBlockOfferingId")
        .and_then(|node| node.text())
        .unwrap_or_else(|| panic!("case {case:?}: expected an offering in {body}"))
        .to_string();
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "PurchaseCapacityBlock",
        &[
            field("CapacityBlockOfferingId", &offering_id),
            field("InstancePlatform", "Linux/UNIX"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the purchase must succeed, got {status} with body {body}"
    );

    let rows = reported_ok(&router, case, &[]).await;
    let row = rows
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the purchase to be reported"));
    assert_eq!(
        row.state, "payment-pending",
        "case {case:?}: expected the state the purchase left the reservation in, got {:?}",
        row.state
    );
    assert!(
        row.capacity_block_id.is_some(),
        "case {case:?}: expected the Capacity Block the reservation holds, got {row:?}"
    );
    assert!(
        row.network_nodes.is_empty(),
        "case {case:?}: expected no network nodes while the reservation holds no capacity, \
         got {:?}",
        row.network_nodes
    );

    // The other end of the window: a reservation whose end date has passed is `expired`,
    // which is also a state that places it on no host. The reservation is seeded straight
    // into the store, because a create refuses an end date that is not in the future — so
    // the case is decided by the dates the record carries rather than by a sleep racing
    // them.
    let case = "a reservation whose end date has passed";
    let expired_id = seed_expired(&engine, ZONES[0]);
    // An ordinary active reservation of the same zone and instance type, so the contrast in
    // one read is the state and nothing else.
    let live_id = reserve(
        &router,
        case,
        &fields(&[
            ("InstanceType", "m5.large"),
            ("InstancePlatform", "Linux/UNIX"),
            ("InstanceCount", "1"),
            ("AvailabilityZone", ZONES[0]),
        ]),
    )
    .await;

    let rows = reported_ok(&router, case, &[]).await;
    let row_for = |id: &str| -> Row {
        rows.iter()
            .find(|row| row.capacity_reservation_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a row for {id}, got {rows:?}"))
    };

    let live = row_for(&live_id);
    assert_eq!(
        live.state, "active",
        "case {case:?}: expected the fresh reservation to be active, got {:?}",
        live.state
    );
    assert_eq!(
        live.network_nodes.len(),
        TOPOLOGY_DEPTH,
        "case {case:?}: expected a path while the reservation is active, got {:?}",
        live.network_nodes
    );

    let expired = row_for(&expired_id);
    assert_eq!(
        expired.state, "expired",
        "case {case:?}: expected expired once the end date had passed, got {:?}",
        expired.state
    );
    assert!(
        expired.network_nodes.is_empty(),
        "case {case:?}: expected no network nodes once the reservation had expired, got {:?}",
        expired.network_nodes
    );
}

/// Rule 4: the report is a read, so what every other reader of a Capacity Reservation
/// reports is the same after it as before it.
#[tokio::test]
async fn reading_the_topology_changes_nothing_a_later_caller_sees() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_all(&router).await;

    let before = described_states(&router).await;
    for attempt in 0..3 {
        let case = format!("read {attempt}");
        let rows = reported_ok(&router, &case, &[]).await;
        assert_eq!(
            rows.len(),
            ids.len(),
            "case {case:?}: expected every reservation on every read, got {rows:?}"
        );
    }
    let after = described_states(&router).await;
    assert_eq!(
        after, before,
        "expected the reservations' states to be left alone, got {after:?} after {before:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSecondarySubnets`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeSecondarySubnets`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's secondary subnets, narrowed by the `SecondarySubnetIds` and `Filters` the
//! request names and cut into `MaxResults`-sized pages. Every table row is a distinct input
//! along what that generalises over — which documented filter, which state of the model's
//! own catalogs, how many values, how many filters, how many CIDR associations, which ids,
//! which page size — so no single recorded request is the specification.

use std::time::Instant;

use ec2::ops::types::{
    SECONDARY_NETWORK_TYPE_VALUES, SECONDARY_SUBNET_CIDR_BLOCK_ASSOCIATION_STATE_VALUES,
    SECONDARY_SUBNET_STATE_VALUES, Tag,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES,
    MAX_FILTERS, MAX_RESOURCE_IDS, SECONDARY_NETWORK_TYPE_RDMA,
    SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED, SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
    SecondaryNetworkSpec, SecondarySubnetCidrSpec, SecondarySubnetSpec,
    availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for, and the Region the seeded subnets' ARNs name.
const REGION: &str = "us-east-1";

/// Create one secondary network for the seeded subnets to belong to, so a subnet's
/// `SecondaryNetworkId` names a network this store really holds. `ordinal` keeps the
/// networks' own CIDR blocks apart.
///
/// Shared with [`super::malformed_before_lookup`], which needs one stored network to resolve
/// and one record to copy under a malformed key: both suites write a network the one way, so
/// the spec members live here only.
pub(super) fn create_network(guard: &mut Ec2State, ordinal: u8) -> String {
    guard.insert_secondary_network(SecondaryNetworkSpec {
        ipv4_cidr_block: format!("10.{ordinal}.0.0/16"),
        network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        // Already past its `create-in-progress` -> `create-complete` move: nothing here
        // reads the network's own state, the subnets are what is under test.
        create_complete_at: Instant::now(),
    })
}

/// One `DescribeSecondarySubnets` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecondarySubnets", params).await
}

/// The secondary subnet ids a successful response reports, in the order it reported them.
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
    elements(&body, "secondarySubnetId")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded secondary subnets carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// One IPv4 CIDR block association of a seeded subnet.
fn cidr(cidr_block: &str, state: &str) -> SecondarySubnetCidrSpec {
    SecondarySubnetCidrSpec {
        cidr_block: cidr_block.to_string(),
        state: state.to_string(),
        state_reason: None,
    }
}

/// What one seeded secondary subnet is created with, beyond the ids the store mints itself.
struct SubnetFixture {
    /// Which of the seeded secondary networks owns this subnet.
    network_index: usize,
    /// The zone of [`availability_zones_in_region`] this subnet is in.
    zone_index: usize,
    /// The subnet's state, as the model's `SecondarySubnetState` enum spells it.
    state: &'static str,
    /// The reason for that state, for the states that have one.
    state_reason: Option<&'static str>,
    /// The IPv4 CIDR blocks associated with the subnet — none, one or several.
    ipv4_cidr_blocks: Vec<SecondarySubnetCidrSpec>,
    tags: Vec<Tag>,
}

/// The secondary subnets the filter and paging tables are written against. They differ
/// along every attribute the documented filters select on — network, state, the CIDR
/// associations and their states, and the tags — and along how many associations they carry.
fn seeded_fixtures() -> Vec<SubnetFixture> {
    vec![
        SubnetFixture {
            network_index: 0,
            zone_index: 0,
            state: "create-complete",
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.0.0.0/24", "associated")],
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        SubnetFixture {
            network_index: 0,
            zone_index: 1,
            state: "create-in-progress",
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.0.1.0/24", "associating")],
            tags: vec![tag("Name", "batch")],
        },
        SubnetFixture {
            network_index: 1,
            zone_index: 2,
            state: "create-complete",
            state_reason: None,
            // Several associations on one subnet: a filter on either block selects it.
            ipv4_cidr_blocks: vec![
                cidr("10.0.2.0/24", "associated"),
                cidr("10.0.3.0/24", "disassociated"),
            ],
            tags: Vec::new(),
        },
        SubnetFixture {
            network_index: 1,
            zone_index: 0,
            state: "create-failed",
            state_reason: Some("The secondary network had no capacity in the zone"),
            // A subnet whose creation failed carries no CIDR association at all, so every
            // `ipv4-cidr-block-association.*` filter must pass it over.
            ipv4_cidr_blocks: Vec::new(),
            tags: Vec::new(),
        },
        SubnetFixture {
            network_index: 1,
            zone_index: 1,
            state: "delete-in-progress",
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.0.4.0/24", "disassociating")],
            tags: vec![tag("Name", "web")],
        },
        SubnetFixture {
            network_index: 2,
            zone_index: 2,
            state: "create-complete",
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.0.5.0/24", "association-failed")],
            tags: vec![tag("Owner", "TeamB")],
        },
        SubnetFixture {
            network_index: 2,
            zone_index: 0,
            state: "delete-complete",
            state_reason: None,
            ipv4_cidr_blocks: vec![cidr("10.0.6.0/24", "disassociated")],
            tags: Vec::new(),
        },
        SubnetFixture {
            network_index: 2,
            zone_index: 1,
            state: "delete-failed",
            state_reason: Some("The secondary subnet still has attached interfaces"),
            ipv4_cidr_blocks: vec![cidr("10.0.7.0/24", "disassociation-failed")],
            tags: Vec::new(),
        },
    ]
}

/// The ids the seeded secondary subnets and their networks were given.
struct Seeded {
    /// The `SecondarySubnetId` of each fixture, in fixture order — which is also store
    /// order, the ids being minted in that order.
    ids: Vec<String>,
    /// The `SecondaryNetworkId` of each seeded secondary network.
    network_ids: Vec<String>,
    /// The association ids reported for each fixture, in fixture order.
    association_ids: Vec<Vec<String>>,
}

/// Write the seeded secondary subnets into the engine's store, the way an operation
/// creating one would, and report the ids they were given.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let network_ids: Vec<String> = (0..3)
        .map(|ordinal| create_network(&mut guard, ordinal))
        .collect();
    let zones = availability_zones_in_region(REGION);
    let mut ids = Vec::new();
    let mut association_ids = Vec::new();
    for fixture in seeded_fixtures() {
        let zone = &zones[fixture.zone_index];
        let id = guard.insert_secondary_subnet(SecondarySubnetSpec {
            secondary_network_id: network_ids[fixture.network_index].clone(),
            secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            region: REGION.to_string(),
            ipv4_cidr_blocks: fixture.ipv4_cidr_blocks,
            state: fixture.state.to_string(),
            state_reason: fixture.state_reason.map(str::to_string),
            tags: fixture.tags,
        })
        .expect("the seeded store is far below its secondary subnet ceiling");
        let associations = guard
            .secondary_subnets
            .get(&id)
            .and_then(|subnet| subnet.ipv4_cidr_block_associations.clone())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|association| association.association_id)
            .collect();
        ids.push(id);
        association_ids.push(associations);
    }

    Seeded {
        ids,
        network_ids,
        association_ids,
    }
}

/// The ids of the fixtures at `indices`, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|index| seeded.ids[*index].clone()).collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has no secondary network, so it has no secondary subnet either: the
/// report is empty, and describing is a READ — two calls answer the same and the store is
/// unchanged.
#[tokio::test]
async fn a_fresh_account_reports_no_secondary_subnet_and_reading_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "secondarySubnetId").is_empty(),
        "fresh account: expected no secondary subnet, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "fresh account: nothing follows an empty report, so no token is due, got {body}"
    );

    let second = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second,
        Vec::<String>::new(),
        "a read must not change what a later caller sees, got {second:?}"
    );
    assert!(
        engine.state::<Ec2State>().lock().secondary_subnets.is_empty(),
        "describing secondary subnets must not write to the store"
    );
}

/// Every secondary subnet the store holds is reported, in store order, with the members it
/// was stored with — and one written after a call shows up in the next one, so the answer is
/// computed from the store rather than fixed.
#[tokio::test]
async fn every_stored_secondary_subnet_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all secondary subnets", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all secondary subnets: expected every stored subnet in store order, got {reported:?}"
    );

    // One subnet, member by member: everything the write set round-trips through the
    // describe, including the two CIDR associations and their states.
    let zones = availability_zones_in_region(REGION);
    let (status, body) = describe(
        &router,
        &indexed("SecondarySubnetId", &[seeded.ids[2].as_str()]),
    )
    .await;
    assert_eq!(status, 200, "one subnet: got {status} with body {body}");
    let expected_members = [
        ("secondarySubnetId", seeded.ids[2].clone()),
        (
            "secondarySubnetArn",
            engine::emulator_arn("ec2", REGION, &format!("secondary-subnet/{}", seeded.ids[2])),
        ),
        ("secondaryNetworkId", seeded.network_ids[1].clone()),
        ("secondaryNetworkType", SECONDARY_NETWORK_TYPE_RDMA.to_string()),
        ("ownerId", engine::emulator_account_id().to_string()),
        ("availabilityZone", zones[2].zone_name.clone()),
        ("availabilityZoneId", zones[2].zone_id.clone()),
        ("state", "create-complete".to_string()),
        ("cidrBlock", "10.0.2.0/24".to_string()),
        ("associationId", seeded.association_ids[2][0].clone()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "one subnet: expected <{member}>{value}</{member}>, got {body}"
        );
    }
    assert_eq!(
        elements(&body, "cidrBlock"),
        vec!["10.0.2.0/24".to_string(), "10.0.3.0/24".to_string()],
        "one subnet: expected both stored CIDR blocks in stored order, got {body}"
    );

    // A state reason is reported for the subnet that has one, and the tags for the ones
    // that carry them.
    let (status, failed) = describe(
        &router,
        &indexed("SecondarySubnetId", &[seeded.ids[3].as_str()]),
    )
    .await;
    assert_eq!(status, 200, "failed subnet: got {status} with body {failed}");
    assert_eq!(
        element(&failed, "stateReason").map(|reason| unescaped(&reason)),
        Some("The secondary network had no capacity in the zone".to_string()),
        "failed subnet: expected the stored state reason, got {failed}"
    );
    assert!(
        elements(&failed, "cidrBlock").is_empty(),
        "failed subnet: it was stored with no association, got {failed}"
    );

    // A subnet stored after the first call is reported by the next one.
    let late_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let network_id = create_network(&mut guard, 9);
        guard.insert_secondary_subnet(SecondarySubnetSpec {
            secondary_network_id: network_id,
            secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
            availability_zone: zones[0].zone_name.clone(),
            availability_zone_id: zones[0].zone_id.clone(),
            region: REGION.to_string(),
            ipv4_cidr_blocks: vec![cidr(
                "10.0.8.0/24",
                SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED,
            )],
            state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
            state_reason: None,
            tags: vec![tag("Name", "late")],
        })
        .expect("the seeded store is far below its secondary subnet ceiling")
    };

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

/// The words this crate stores for a secondary subnet are members of the model's own
/// catalogs, so a filter value a caller reads out of the API documentation can match them.
#[test]
fn the_stored_catalog_words_are_members_of_the_modelled_enums() {
    let cases = [
        (
            "SecondarySubnetState",
            SECONDARY_SUBNET_STATE_CREATE_COMPLETE,
            SECONDARY_SUBNET_STATE_VALUES,
        ),
        (
            "SecondarySubnetCidrBlockAssociationState",
            SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED,
            SECONDARY_SUBNET_CIDR_BLOCK_ASSOCIATION_STATE_VALUES,
        ),
        (
            "SecondaryNetworkType",
            SECONDARY_NETWORK_TYPE_RDMA,
            SECONDARY_NETWORK_TYPE_VALUES,
        ),
    ];
    for (enum_name, stored, catalog) in cases {
        assert!(
            catalog.contains(&stored),
            "case {enum_name:?}: {stored:?} is not a member of {catalog:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects the secondary subnets whose own attribute matches a
/// value; values within one filter are OR'd and the comparison is case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_subnets_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeSecondarySubnetsRequest$Filters` documents, with
    // extra rows for value counts, wildcards, case sensitivity and the empty value list.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "ipv4-cidr-block-association.association-id",
            vec![seeded.association_ids[4][0].clone()],
            vec![4],
        ),
        (
            "ipv4-cidr-block-association.association-id",
            // The second association of the subnet that has two.
            vec![seeded.association_ids[2][1].clone()],
            vec![2],
        ),
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.0.0.0/24".into()],
            vec![0],
        ),
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.0.3.0/24".into()],
            vec![2],
        ),
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.0.0.0/24".into(), "10.0.4.0/24".into()],
            vec![0, 4],
        ),
        // EC2's own `*` wildcard, which every stored block matches — and the one subnet
        // stored with no association matches none of them.
        (
            "ipv4-cidr-block-association.cidr-block",
            vec!["10.0.*".into()],
            vec![0, 1, 2, 4, 5, 6, 7],
        ),
        (
            "ipv4-cidr-block-association.state",
            vec!["associated".into()],
            vec![0, 2],
        ),
        (
            "ipv4-cidr-block-association.state",
            vec!["disassociated".into()],
            vec![2, 6],
        ),
        (
            "ipv4-cidr-block-association.state",
            vec!["associating".into(), "disassociating".into()],
            vec![1, 4],
        ),
        // "Filter values are case-sensitive" (`Filter$Values`).
        (
            "ipv4-cidr-block-association.state",
            vec!["Associated".into()],
            vec![],
        ),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["210987654321".into()], vec![]),
        (
            "secondary-network-id",
            vec![seeded.network_ids[0].clone()],
            vec![0, 1],
        ),
        (
            "secondary-network-id",
            vec![seeded.network_ids[1].clone()],
            vec![2, 3, 4],
        ),
        (
            "secondary-network-id",
            vec![
                seeded.network_ids[0].clone(),
                seeded.network_ids[2].clone(),
            ],
            vec![0, 1, 5, 6, 7],
        ),
        ("secondary-network-type", vec!["rdma".into()], all.clone()),
        ("secondary-network-type", vec!["efa".into()], vec![]),
        ("secondary-subnet-id", vec![seeded.ids[5].clone()], vec![5]),
        (
            "secondary-subnet-id",
            vec![seeded.ids[0].clone(), seeded.ids[7].clone()],
            vec![0, 7],
        ),
        (
            "secondary-subnet-arn",
            vec![engine::emulator_arn(
                "ec2",
                REGION,
                &format!("secondary-subnet/{}", seeded.ids[6]),
            )],
            vec![6],
        ),
        // Every ARN of this account's secondary subnets, by wildcard.
        (
            "secondary-subnet-arn",
            vec![format!(
                "arn:aws:ec2:{REGION}:{}:secondary-subnet/*",
                engine::emulator_account_id()
            )],
            all.clone(),
        ),
        ("state", vec!["create-complete".into()], vec![0, 2, 5]),
        (
            "state",
            vec!["create-failed".into(), "delete-failed".into()],
            vec![3, 7],
        ),
        ("tag:Name", vec!["web".into()], vec![0, 4]),
        ("tag:Name", vec!["batch".into()], vec![1]),
        ("tag:Owner", vec!["TeamA".into(), "TeamB".into()], vec![0, 5]),
        ("tag:Owner", vec!["TeamC".into()], vec![]),
        ("tag-key", vec!["Name".into()], vec![0, 1, 4]),
        ("tag-key", vec!["Owner".into()], vec![0, 5]),
        ("tag-key", vec!["Name".into(), "Owner".into()], vec![0, 1, 4, 5]),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("secondary-subnet-id", vec![], vec![]),
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

/// One row of the AND table: the filters to send, and the indices of the subnets that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the subnets matching every one are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let network_one = seeded.network_ids[1].clone();
    let network_one_values = [network_one.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("secondary-network-id", &network_one_values[..]),
                ("state", &["create-complete"][..]),
            ],
            vec![2],
        ),
        (
            vec![
                ("secondary-network-type", &["rdma"][..]),
                ("tag-key", &["Name"][..]),
                ("state", &["create-complete", "delete-in-progress"][..]),
            ],
            vec![0, 4],
        ),
        (
            vec![
                ("ipv4-cidr-block-association.state", &["associated"][..]),
                ("tag:Name", &["web"][..]),
            ],
            vec![0],
        ),
        // Each filter matches something, but no subnet matches both.
        (
            vec![
                ("state", &["create-failed"][..]),
                ("ipv4-cidr-block-association.state", &["associated"][..]),
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

/// Every member of the model's `SecondarySubnetState` catalog selects the subnet stored in
/// that state — the whole catalog, not the states the fixtures above happen to use.
#[tokio::test]
async fn every_modelled_subnet_state_selects_the_subnet_stored_in_it() {
    let (router, engine) = super::ec2_query::emulator();

    // One subnet per catalog member, so each state has exactly one subnet to select.
    let mut by_subnet_state: Vec<(&str, String)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let network_id = create_network(&mut guard, 0);
        let zones = availability_zones_in_region(REGION);
        for (index, state) in SECONDARY_SUBNET_STATE_VALUES.iter().enumerate() {
            let zone = &zones[index % zones.len()];
            let id = guard.insert_secondary_subnet(SecondarySubnetSpec {
                secondary_network_id: network_id.clone(),
                secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: REGION.to_string(),
                ipv4_cidr_blocks: Vec::new(),
                state: (*state).to_string(),
                state_reason: None,
                tags: Vec::new(),
            })
            .expect("the seeded store is far below its secondary subnet ceiling");
            by_subnet_state.push((state, id));
        }
    }

    for (state, id) in &by_subnet_state {
        let case = format!("state={state:?}");
        let reported = described_ids(&router, &case, &filters(&[("state", &[state])])).await;
        assert_eq!(
            reported,
            vec![id.clone()],
            "case {case:?}: expected only {id:?}, got {reported:?}"
        );
    }
}

/// Every member of the model's `SecondarySubnetCidrBlockAssociationState` catalog selects
/// the subnet whose association is in it — the whole catalog, not just the states the
/// fixtures above happen to use. Each subnet here sits in the same subnet state, so only
/// the association's own state tells them apart.
#[tokio::test]
async fn every_modelled_association_state_selects_the_subnet_whose_block_is_in_it() {
    let (router, engine) = super::ec2_query::emulator();

    let mut by_association_state: Vec<(&str, String)> = Vec::new();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let network_id = create_network(&mut guard, 0);
        let zones = availability_zones_in_region(REGION);
        for (index, state) in SECONDARY_SUBNET_CIDR_BLOCK_ASSOCIATION_STATE_VALUES
            .iter()
            .enumerate()
        {
            let zone = &zones[index % zones.len()];
            let id = guard.insert_secondary_subnet(SecondarySubnetSpec {
                secondary_network_id: network_id.clone(),
                secondary_network_type: SECONDARY_NETWORK_TYPE_RDMA.to_string(),
                availability_zone: zone.zone_name.clone(),
                availability_zone_id: zone.zone_id.clone(),
                region: REGION.to_string(),
                ipv4_cidr_blocks: vec![cidr(&format!("10.{index}.0.0/24"), state)],
                state: SECONDARY_SUBNET_STATE_CREATE_COMPLETE.to_string(),
                state_reason: None,
                tags: Vec::new(),
            })
            .expect("the seeded store is far below its secondary subnet ceiling");
            by_association_state.push((state, id));
        }
    }

    for (state, id) in &by_association_state {
        let case = format!("ipv4-cidr-block-association.state={state:?}");
        let reported = described_ids(
            &router,
            &case,
            &filters(&[("ipv4-cidr-block-association.state", &[state])]),
        )
        .await;
        assert_eq!(
            reported,
            vec![id.clone()],
            "case {case:?}: expected only {id:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: near-misses of two documented names, the underscore spelling, a documented
    // name with the wrong case, filters of other describes, the tag filter without its key
    // separator, the tag filter with a separator but no key, and an empty name.
    let cases = [
        "secondary-subnet-ids",
        "secondary-network-types",
        "secondary_subnet_id",
        "Secondary-Subnet-Id",
        "cidr-block",
        "availability-zone",
        "vpc-id",
        "tag",
        "tag:",
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
            "case {name:?}: the refusal must name the filter, got {body}"
        );
        assert!(
            elements(&body, "secondarySubnetId").is_empty(),
            "case {name:?}: a refused request reports no subnet, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// SecondarySubnetIds
// ---------------------------------------------------------------------------

/// `SecondarySubnetIds` narrows the report to the ids named, in store order whatever order
/// they were asked for, and composes with a filter.
#[tokio::test]
async fn secondary_subnet_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: one id, several in store order, the same several reversed, every id, and a
    // repeated id.
    let cases: Vec<(Vec<usize>, Vec<usize>)> = vec![
        (vec![3], vec![3]),
        (vec![1, 5, 6], vec![1, 5, 6]),
        (vec![6, 5, 1], vec![1, 5, 6]),
        ((0..seeded.ids.len()).collect(), (0..seeded.ids.len()).collect()),
        (vec![2, 2], vec![2]),
    ];
    for (asked, expected_indices) in cases {
        let asked_ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = asked_ids.iter().map(String::as_str).collect();
        let case = format!("SecondarySubnetIds={borrowed:?}");
        let reported =
            described_ids(&router, &case, &indexed("SecondarySubnetId", &borrowed)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // The ids and the filters are joined: only the named subnets that also match.
    let asked = ids_at(&seeded, &[0, 1, 2]);
    let borrowed: Vec<&str> = asked.iter().map(String::as_str).collect();
    let mut params = indexed("SecondarySubnetId", &borrowed);
    params.extend(filters(&[("state", &["create-complete"])]));
    let reported = described_ids(&router, "ids and a filter", &params).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &[0, 2]),
        "ids and a filter: expected the named subnets that match, got {reported:?}"
    );
}

/// An id that names no secondary subnet of the account is refused with
/// `InvalidSecondarySubnetID.NotFound`, rather than reported as an absent row.
#[tokio::test]
async fn an_unknown_secondary_subnet_id_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has, a shorter id form, something that is not an id
    // at all, an ordinary subnet id, and an unknown id carried beside a known one in each
    // order.
    let cases: Vec<Vec<String>> = vec![
        vec!["snet-0ffffffffffffffff".to_string()],
        vec!["snet-0abc1234".to_string()],
        vec!["not-a-secondary-subnet-id".to_string()],
        vec!["subnet-00000000000000001".to_string()],
        vec![known.clone(), "snet-0ffffffffffffffff".to_string()],
        vec!["snet-0ffffffffffffffff".to_string(), known],
    ];
    for ids in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("SecondarySubnetIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("SecondarySubnetId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSecondarySubnetID.NotFound",
            "case {case:?}: expected InvalidSecondarySubnetID.NotFound, got {body}"
        );
        assert!(
            elements(&body, "secondarySubnetId").is_empty(),
            "case {case:?}: a refused request reports no subnet, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The caps on what one request may carry
// ---------------------------------------------------------------------------

/// One row of the request-size cap table: the case name, the form fields to send, and the
/// HTTP status they must answer with.
type CapCase = (String, Vec<(String, String)>, u16);

/// Each cap this operation puts on a request-controlled factor — how many filters, how many
/// values in one filter, how many bytes in one value, how many ids — is accepted at its
/// bound and refused one past it with `InvalidParameterValue`. The model bounds none of the
/// four, so the handler is the only place they exist.
#[tokio::test]
async fn the_request_size_caps_are_accepted_at_their_bound_and_refused_past_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // `count` copies of one documented filter.
    let filter_count = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .flat_map(|index| {
                vec![
                    (format!("Filter.{}.Name", index + 1), "state".to_string()),
                    (
                        format!("Filter.{}.Value.1", index + 1),
                        "create-complete".to_string(),
                    ),
                ]
            })
            .collect()
    };
    // One documented filter carrying `count` values.
    let value_count = |count: usize| -> Vec<(String, String)> {
        let mut params = vec![("Filter.1.Name".to_string(), "state".to_string())];
        params.extend((0..count).map(|index| {
            (
                format!("Filter.1.Value.{}", index + 1),
                format!("state-{index}"),
            )
        }));
        params
    };
    // One filter value of `bytes` bytes.
    let value_bytes = |bytes: usize| -> Vec<(String, String)> {
        vec![
            (
                "Filter.1.Name".to_string(),
                "secondary-subnet-id".to_string(),
            ),
            ("Filter.1.Value.1".to_string(), "a".repeat(bytes)),
        ]
    };
    // `count` ids, each naming a subnet the store really holds, so the only thing that can
    // refuse the request is the cap itself.
    let id_count = |count: usize| -> Vec<(String, String)> {
        (0..count)
            .map(|index| (format!("SecondarySubnetId.{}", index + 1), known.clone()))
            .collect()
    };

    let cases: Vec<CapCase> = vec![
        (format!("{MAX_FILTERS} filters"), filter_count(MAX_FILTERS), 200),
        (
            format!("{} filters", MAX_FILTERS + 1),
            filter_count(MAX_FILTERS + 1),
            400,
        ),
        (
            format!("{MAX_FILTER_VALUES} values"),
            value_count(MAX_FILTER_VALUES),
            200,
        ),
        (
            format!("{} values", MAX_FILTER_VALUES + 1),
            value_count(MAX_FILTER_VALUES + 1),
            400,
        ),
        (
            format!("a {MAX_FILTER_VALUE_BYTES}-byte value"),
            value_bytes(MAX_FILTER_VALUE_BYTES),
            200,
        ),
        (
            format!("a {}-byte value", MAX_FILTER_VALUE_BYTES + 1),
            value_bytes(MAX_FILTER_VALUE_BYTES + 1),
            400,
        ),
        (format!("{MAX_RESOURCE_IDS} ids"), id_count(MAX_RESOURCE_IDS), 200),
        (
            format!("{} ids", MAX_RESOURCE_IDS + 1),
            id_count(MAX_RESOURCE_IDS + 1),
            400,
        ),
    ];

    for (case, params, expected_status) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            assert!(
                elements(&body, "secondarySubnetId").is_empty(),
                "case {case:?}: a refused request reports no subnet, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every secondary subnet exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_subnet_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeSecondarySubnetsMaxResults` is modelled `range 5..1000`, so the rows are its
    // smallest published size, one that does not divide the store, one that holds it
    // exactly, and the largest published size.
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
            let page = elements(&body, "secondarySubnetId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every subnet exactly once in store order, got {walked:?}"
        );
    }
}

/// A page is cut from the FILTERED set, not from the whole store: `MaxResults` and a filter
/// compose, and the token resumes within the filtered set.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[
        ("secondary-network-type", &["rdma"]),
        ("ipv4-cidr-block-association.cidr-block", &["10.0.*"]),
    ]);
    params.extend(max_results_param(5));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert_eq!(
        elements(&body, "secondarySubnetId"),
        ids_at(&seeded, &[0, 1, 2, 4, 5]),
        "filtered first page: expected the first five subnets with a CIDR block, got {body}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut resumed = filters(&[("ipv4-cidr-block-association.cidr-block", &["10.0.*"])]);
    resumed.extend(max_results_param(5));
    resumed.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &resumed).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[6, 7]),
        "filtered second page: expected the subnets left after the first page, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 5..1000`) and one past each.
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

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape the
    // token takes, without the marker and checksum a minted token carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0])),
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
/// nothing; a request that would have been REFUSED is refused with its own reason instead,
/// and `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

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
        elements(&body, "secondarySubnetId").is_empty(),
        "a dry run reports no subnet, got {body}"
    );

    // A dry run whose filter name is undefined is refused for that reason instead.
    let mut undefined_filter = filters(&[("cidr-block", &["10.0.0.0/24"])]);
    undefined_filter.extend(dry_run);
    let (status, body) = describe(&router, &undefined_filter).await;
    assert_eq!(
        status, 400,
        "a dry run with an undefined filter: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a dry run with an undefined filter: expected InvalidParameterValue, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported, seeded.ids,
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}

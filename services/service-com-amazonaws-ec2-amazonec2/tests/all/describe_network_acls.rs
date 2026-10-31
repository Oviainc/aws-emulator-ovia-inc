// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeNetworkAcls`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeNetworkAcls`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's network ACLs narrowed by the `NetworkAclIds` and `Filters` the request
//! names and cut into `MaxResults`-sized pages. Every table row is a distinct input along
//! what that generalises over — which filter, how many values, how many filters, which ids,
//! which page size — so the default VPC's own ACL is one row among many rather than the
//! specification.
//!
//! The store side of the same rule is covered too: a VPC comes with a default network ACL
//! and a subnet is associated with it, so a VPC and the subnets created through the wire
//! show up in this report, and both go away again when they are deleted.

use ec2::ops::types::{IcmpTypeCode, NetworkAclEntry, PortRange, Tag};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, NetworkAclSpec, SubnetSpec,
    availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};
use super::wire::{child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for. The network ACL store holds the account's
/// ACLs rather than one Region's, and the default VPC that a fresh account already has is
/// seeded for the emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// One `DescribeNetworkAcls` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeNetworkAcls", params).await
}

/// The network ACL ids a body reports, in the order it reported them.
///
/// Read off the TREE rather than by scanning for `<networkAclId>`: each association inside
/// an ACL carries a `networkAclId` of its own, so only the direct child of a reported ACL
/// is that ACL's id.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .map(|item| child_text(item, "networkAclId").unwrap_or_default())
        .collect()
}

/// The ids a successful `DescribeNetworkAcls` reports for `params`.
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
    reported_ids(case, &body)
}

/// One entry of a reported network ACL, as the members the tables assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedEntry {
    rule_number: String,
    rule_action: String,
    protocol: String,
    cidr_block: Option<String>,
    ipv6_cidr_block: Option<String>,
    egress: String,
    port_range: Option<(String, String)>,
    icmp_type_code: Option<(String, String)>,
}

/// One reported network ACL, as the members the tables assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedAcl {
    network_acl_id: String,
    vpc_id: String,
    owner_id: String,
    is_default: String,
    entries: Vec<ReportedEntry>,
    /// Each association as `(associationId, subnetId)`.
    associations: Vec<(String, String)>,
    /// Each tag as `(key, value)`.
    tags: Vec<(String, String)>,
}

/// Every network ACL a body reports, read off the tree so a member nested in an entry or an
/// association stays inside it.
fn reported_acls(case: &str, body: &str) -> Vec<ReportedAcl> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "networkAclSet")
        .into_iter()
        .map(|item| {
            let text = |name: &str| child_text(item, name).unwrap_or_default();
            ReportedAcl {
                network_acl_id: text("networkAclId"),
                vpc_id: text("vpcId"),
                owner_id: text("ownerId"),
                is_default: text("default"),
                entries: set_items(item, "entrySet")
                    .into_iter()
                    .map(|entry| ReportedEntry {
                        rule_number: child_text(entry, "ruleNumber").unwrap_or_default(),
                        rule_action: child_text(entry, "ruleAction").unwrap_or_default(),
                        protocol: child_text(entry, "protocol").unwrap_or_default(),
                        cidr_block: child_text(entry, "cidrBlock"),
                        ipv6_cidr_block: child_text(entry, "ipv6CidrBlock"),
                        egress: child_text(entry, "egress").unwrap_or_default(),
                        port_range: super::wire::child(entry, "portRange").map(|range| {
                            (
                                child_text(range, "from").unwrap_or_default(),
                                child_text(range, "to").unwrap_or_default(),
                            )
                        }),
                        icmp_type_code: super::wire::child(entry, "icmpTypeCode").map(|icmp| {
                            (
                                child_text(icmp, "type").unwrap_or_default(),
                                child_text(icmp, "code").unwrap_or_default(),
                            )
                        }),
                    })
                    .collect(),
                associations: set_items(item, "associationSet")
                    .into_iter()
                    .map(|association| {
                        (
                            child_text(association, "networkAclAssociationId").unwrap_or_default(),
                            child_text(association, "subnetId").unwrap_or_default(),
                        )
                    })
                    .collect(),
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
            }
        })
        .collect()
}

/// The one network ACL `network_acl_id` names, as reported.
async fn describe_one(case: &str, router: &MultiServiceRouter, network_acl_id: &str) -> ReportedAcl {
    let params = indexed("NetworkAclId", &[network_acl_id]);
    let (status, body) = describe(router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let mut reported = reported_acls(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the ACL {network_acl_id:?}, got {reported:?}"
    );
    reported.remove(0)
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded ACLs carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// One network ACL entry, carrying nothing beyond the rule itself: the range, the port
/// range and the ICMP type and code are added by the builders below, so an entry only ever
/// holds the members its direction and protocol imply.
fn rule(rule_number: i32, egress: bool, rule_action: &str, protocol: &str) -> NetworkAclEntry {
    NetworkAclEntry {
        cidr_block: None,
        egress: Some(egress),
        icmp_type_code: None,
        ipv6_cidr_block: None,
        port_range: None,
        protocol: Some(protocol.to_string()),
        rule_action: Some(rule_action.to_string()),
        rule_number: Some(rule_number),
    }
}

/// `entry` with "the IPv4 network range to allow or deny, in CIDR notation"
/// (`NetworkAclEntry$CidrBlock`).
fn with_cidr(mut entry: NetworkAclEntry, cidr_block: &str) -> NetworkAclEntry {
    entry.cidr_block = Some(cidr_block.to_string());
    entry
}

/// `entry` with "the IPv6 network range to allow or deny"
/// (`NetworkAclEntry$Ipv6CidrBlock`).
fn with_ipv6_cidr(mut entry: NetworkAclEntry, ipv6_cidr_block: &str) -> NetworkAclEntry {
    entry.ipv6_cidr_block = Some(ipv6_cidr_block.to_string());
    entry
}

/// `entry` with "the range of ports the rule applies to"
/// (`NetworkAclEntry$PortRange`), which only a TCP or UDP entry carries.
fn with_ports(mut entry: NetworkAclEntry, from: i32, to: i32) -> NetworkAclEntry {
    entry.port_range = Some(PortRange {
        from: Some(from),
        to: Some(to),
    });
    entry
}

/// `entry` with "the ICMP type and code" (`NetworkAclEntry$IcmpTypeCode`), which only an
/// ICMP entry carries.
fn with_icmp(mut entry: NetworkAclEntry, icmp_type: i32, code: i32) -> NetworkAclEntry {
    entry.icmp_type_code = Some(IcmpTypeCode {
        r#type: Some(icmp_type),
        code: Some(code),
    });
    entry
}

/// The entries every default network ACL carries: in each direction, allow everything at
/// rule 100 and the catch-all deny at 32767 (Amazon VPC User Guide, "Default network ACL"),
/// reported egress-first and then ingress, each by ascending rule number — the order an
/// ACL's `entrySet` comes back in.
fn default_entries() -> Vec<ReportedEntry> {
    [true, false]
        .into_iter()
        .flat_map(|egress| {
            [("100", "allow"), ("32767", "deny")].map(|(rule_number, rule_action)| ReportedEntry {
                rule_number: rule_number.to_string(),
                rule_action: rule_action.to_string(),
                protocol: "-1".to_string(),
                cidr_block: Some("0.0.0.0/0".to_string()),
                ipv6_cidr_block: None,
                egress: egress.to_string(),
                // A reported entry carries both members; an entry whose protocol has no ICMP
                // numbers and no ports carries them EMPTY rather than not at all.
                port_range: Some((String::new(), String::new())),
                icmp_type_code: Some((String::new(), String::new())),
            })
        })
        .collect()
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every network ACL id, in the order `DescribeNetworkAcls` reports them (the store's
    /// key order): the default VPC's own ACL, then the fixture VPC's, then the four the
    /// fixtures below add.
    ids: Vec<String>,
    /// The VPC a fresh account already has.
    default_vpc_id: String,
    /// The VPC the fixture ACLs are in.
    fixture_vpc_id: String,
    /// A default subnet of the default VPC.
    default_subnet_id: String,
    /// The fixture VPC's subnet that is still associated with that VPC's default ACL.
    fixture_subnet_id: String,
    /// The fixture VPC's subnet whose association was moved to the `web` ACL, and the id
    /// that association was given.
    moved_subnet_id: String,
    moved_association_id: String,
}

/// Write the seeded network ACLs into the engine's store, the way a creating operation
/// would, and report the ids they were given.
///
/// The ACLs differ along every attribute the documented filters select on: the VPC, whether
/// they are the VPC's default, which subnets they are associated with, their tags, and the
/// entries they hold — direction, rule number, action, protocol, IPv4 and IPv6 range, port
/// range and ICMP type and code.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let default_vpc_id = guard
        .default_vpc_id(REGION)
        .expect("a fresh account has its default VPC");
    let default_subnet_id = guard
        .subnets
        .iter()
        .find(|(_, subnet)| subnet.vpc_id.as_deref() == Some(default_vpc_id.as_str()))
        .map(|(subnet_id, _)| subnet_id.clone())
        .expect("the default VPC has a default subnet in each zone");

    let fixture_vpc_id =
        guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let zones = availability_zones_in_region(REGION);
    let mut fixture_subnets = Vec::new();
    for (index, cidr_block) in ["10.0.0.0/24", "10.0.1.0/24"].into_iter().enumerate() {
        fixture_subnets.push(guard.insert_subnet(SubnetSpec {
            vpc_id: fixture_vpc_id.clone(),
            cidr_block: cidr_block.to_string(),
            availability_zone: zones[index].zone_name.clone(),
            availability_zone_id: zones[index].zone_id.clone(),
            region: REGION.to_string(),
            state: "available".to_string(),
            default_for_az: false,
            map_public_ip_on_launch: false,
            tags: Vec::new(),
        }));
    }

    // A web tier's ACL: TCP 80-443 from one range, then the catch-all deny, and everything
    // out.
    let web = guard
        .insert_network_acl(NetworkAclSpec {
            vpc_id: fixture_vpc_id.clone(),
            is_default: false,
            entries: vec![
                with_ports(
                    with_cidr(rule(100, false, "allow", "6"), "10.0.0.0/16"),
                    80,
                    443,
                ),
                with_cidr(rule(32767, false, "deny", "-1"), "0.0.0.0/0"),
                with_cidr(rule(100, true, "allow", "-1"), "0.0.0.0/0"),
            ],
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        })
        .expect("a fresh store has room for a network ACL");

    // A batch tier's ACL: UDP 53, ICMP echo, and an IPv6 deny on the way out.
    guard
        .insert_network_acl(NetworkAclSpec {
            vpc_id: fixture_vpc_id.clone(),
            is_default: false,
            entries: vec![
                with_ports(with_cidr(rule(110, false, "allow", "17"), "0.0.0.0/0"), 53, 53),
                with_icmp(with_cidr(rule(120, false, "allow", "1"), "0.0.0.0/0"), 8, -1),
                with_ipv6_cidr(rule(32767, true, "deny", "-1"), "::/0"),
            ],
            tags: vec![tag("Name", "batch")],
        })
        .expect("a fresh store has room for a network ACL");

    // An untagged ACL: a denied SSH range and an IPv6 allow.
    guard
        .insert_network_acl(NetworkAclSpec {
            vpc_id: fixture_vpc_id.clone(),
            is_default: false,
            entries: vec![
                with_ports(
                    with_cidr(rule(200, false, "deny", "6"), "192.0.2.0/24"),
                    22,
                    22,
                ),
                with_ipv6_cidr(rule(210, false, "allow", "-1"), "::/0"),
            ],
            tags: Vec::new(),
        })
        .expect("a fresh store has room for a network ACL");

    // A non-default ACL in the DEFAULT VPC, so `vpc-id` and `default` select different sets.
    guard
        .insert_network_acl(NetworkAclSpec {
            vpc_id: default_vpc_id.clone(),
            is_default: false,
            entries: vec![with_cidr(rule(100, false, "allow", "-1"), "0.0.0.0/0")],
            tags: vec![tag("Project", "atlas")],
        })
        .expect("a fresh store has room for a network ACL");

    // The second fixture subnet is associated with the `web` ACL instead of its VPC's
    // default one — a subnet is associated with exactly one ACL, so the default association
    // goes before the new one is recorded.
    guard.disassociate_subnet_from_network_acls(&fixture_subnets[1]);
    let moved_association_id = guard
        .associate_subnet_with_network_acl(&fixture_subnets[1], &web)
        .expect("the web ACL is in the store");

    Seeded {
        ids: guard.network_acls.keys().cloned().collect(),
        default_vpc_id,
        fixture_vpc_id,
        default_subnet_id,
        fixture_subnet_id: fixture_subnets[0].clone(),
        moved_subnet_id: fixture_subnets[1].clone(),
        moved_association_id,
    }
}

/// The ids at `indices` of a seeded store, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account's default VPC already has the default network ACL AWS gives every VPC —
/// `IsDefault`, allowing all traffic in both directions, and associated with each of the
/// VPC's default subnets — and describing it is a READ: two calls report the same ACL and
/// the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_the_default_network_acl_of_its_default_vpc() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let reported = reported_acls("fresh account", &first);
    assert_eq!(
        reported.len(),
        1,
        "fresh account: expected exactly the default VPC's own network ACL, got {reported:?}"
    );
    let acl = &reported[0];

    // The VPC the ACL belongs to is the one `DescribeVpcs` reports as the default, so the
    // two describes agree about the account's default network.
    let (vpc_status, vpc_body) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    assert_eq!(
        vpc_status, 200,
        "fresh account: expected HTTP 200 from DescribeVpcs, got {vpc_status} with body {vpc_body}"
    );
    let default_vpc_id = element(&vpc_body, "vpcId").unwrap_or_default();

    assert!(
        is_long_id("acl", &acl.network_acl_id),
        "fresh account: expected an acl- id, got {acl:?}"
    );
    assert_eq!(
        acl.vpc_id, default_vpc_id,
        "fresh account: expected the default VPC's id, got {acl:?}"
    );
    assert_eq!(
        acl.is_default, "true",
        "fresh account: the ACL a VPC comes with is the default one, got {acl:?}"
    );
    assert_eq!(
        acl.owner_id,
        engine::emulator_account_id(),
        "fresh account: expected the account that owns the ACL, got {acl:?}"
    );
    assert_eq!(
        acl.entries,
        default_entries(),
        "fresh account: expected the default ACL's allow-everything and catch-all deny \
         entries, got {acl:?}"
    );
    assert_eq!(
        acl.tags,
        Vec::new(),
        "fresh account: nothing has tagged the ACL the service created, got {acl:?}"
    );

    // Every default subnet of the VPC is associated with it: "if you don't explicitly
    // associate a subnet with a network ACL, the subnet is automatically associated with the
    // default network ACL".
    let (subnet_status, subnet_body) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[default_vpc_id.as_str()])]),
    )
    .await;
    assert_eq!(
        subnet_status, 200,
        "fresh account: expected HTTP 200 from DescribeSubnets, got {subnet_status} with body \
         {subnet_body}"
    );
    let subnet_ids = elements(&subnet_body, "subnetId");
    let associated: Vec<String> = acl
        .associations
        .iter()
        .map(|(_, subnet_id)| subnet_id.clone())
        .collect();
    assert_eq!(
        associated, subnet_ids,
        "fresh account: expected every default subnet associated with the default ACL, got \
         {acl:?}"
    );
    for (association_id, subnet_id) in &acl.associations {
        assert!(
            is_long_id("aclassoc", association_id),
            "fresh account: expected an aclassoc- id for the association of {subnet_id:?}, got \
             {association_id:?}"
        );
    }

    let (second_status, second) = describe(&router, &[]).await;
    assert_eq!(
        second_status, 200,
        "second read: expected HTTP 200, got {second_status} with body {second}"
    );
    assert_eq!(
        reported_acls("second read", &second),
        reported,
        "a read must not change what a later caller sees"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().network_acls.len(),
        1,
        "describing network ACLs must not write to the store"
    );
}

/// The rule's store side, over the wire: a VPC a caller creates comes with its own default
/// network ACL, each subnet created in it is associated with that ACL, deleting a subnet
/// takes its association away, and deleting the VPC takes the ACL away.
///
/// The rows differ in the shape of the VPC: the size of its block and how many subnets are
/// created in it.
#[tokio::test]
async fn a_created_vpc_comes_with_a_default_network_acl_that_its_subnets_are_associated_with() {
    let (router, _engine) = super::ec2_query::emulator();

    let cases: [(&str, &[&str]); 3] = [
        ("10.1.0.0/16", &["10.1.0.0/24"]),
        ("10.2.0.0/24", &[]),
        ("10.3.0.0/16", &["10.3.0.0/24", "10.3.1.0/24", "10.3.2.0/24"]),
    ];
    for (vpc_cidr_block, subnet_blocks) in cases {
        let case = format!("VPC {vpc_cidr_block} with {} subnet(s)", subnet_blocks.len());
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "CreateVpc",
            &[param("CidrBlock", vpc_cidr_block)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
        );
        let vpc_id = super::wire::member(&case, &body, "vpcId");

        let of_vpc = filters(&[("vpc-id", &[vpc_id.as_str()])]);
        let ids = described_ids(&router, &case, &of_vpc).await;
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected the one default ACL the VPC comes with, got {ids:?}"
        );
        let network_acl_id = ids[0].clone();
        let acl = describe_one(&case, &router, &network_acl_id).await;
        assert_eq!(
            acl.is_default, "true",
            "case {case:?}: the ACL a VPC comes with is the default one, got {acl:?}"
        );
        assert_eq!(
            acl.entries,
            default_entries(),
            "case {case:?}: expected the default ACL's entries, got {acl:?}"
        );
        assert_eq!(
            acl.associations,
            Vec::new(),
            "case {case:?}: a VPC with no subnet yet has nothing associated, got {acl:?}"
        );

        let mut subnet_ids = Vec::new();
        for subnet_block in subnet_blocks {
            let (subnet_status, subnet_body) = super::ec2_query::call(
                &router,
                REGION,
                "CreateSubnet",
                &[param("VpcId", &vpc_id), param("CidrBlock", subnet_block)],
            )
            .await;
            assert_eq!(
                subnet_status, 200,
                "case {case:?}: expected HTTP 200 from CreateSubnet, got {subnet_status} with \
                 body {subnet_body}"
            );
            subnet_ids.push(super::wire::member(&case, &subnet_body, "subnetId"));
        }

        let acl = describe_one(&case, &router, &network_acl_id).await;
        let associated: Vec<String> = acl
            .associations
            .iter()
            .map(|(_, subnet_id)| subnet_id.clone())
            .collect();
        assert_eq!(
            associated, subnet_ids,
            "case {case:?}: expected every subnet of the VPC associated with its default ACL, \
             got {acl:?}"
        );

        for subnet_id in &subnet_ids {
            let (delete_status, delete_body) = super::ec2_query::call(
                &router,
                REGION,
                "DeleteSubnet",
                &[param("SubnetId", subnet_id)],
            )
            .await;
            assert_eq!(
                delete_status, 200,
                "case {case:?}: expected HTTP 200 from DeleteSubnet, got {delete_status} with \
                 body {delete_body}"
            );
        }
        let acl = describe_one(&case, &router, &network_acl_id).await;
        assert_eq!(
            acl.associations,
            Vec::new(),
            "case {case:?}: a deleted subnet's association must not outlive it, got {acl:?}"
        );

        let (delete_status, delete_body) =
            super::ec2_query::call(&router, REGION, "DeleteVpc", &[param("VpcId", &vpc_id)]).await;
        assert_eq!(
            delete_status, 200,
            "case {case:?}: expected HTTP 200 from DeleteVpc, got {delete_status} with body \
             {delete_body}"
        );
        let after = described_ids(&router, &case, &of_vpc).await;
        assert_eq!(
            after,
            Vec::<String>::new(),
            "case {case:?}: deleting the VPC deletes its network ACL, got {after:?}"
        );
    }
}

/// Every network ACL the store holds is reported, in store order, and one written after a
/// call shows up in the next one with the members it was stored with — the answer is
/// computed from the store, not fixed.
#[tokio::test]
async fn every_stored_network_acl_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all network ACLs", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all network ACLs: expected every stored ACL in store order, got {reported:?}"
    );

    let late_entries = vec![with_ports(
        with_cidr(rule(300, false, "deny", "17"), "198.51.100.0/24"),
        161,
        162,
    )];
    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_network_acl(NetworkAclSpec {
            vpc_id: seeded.fixture_vpc_id.clone(),
            is_default: false,
            entries: late_entries,
            tags: vec![tag("Name", "late")],
        })
        .expect("the seeded store has room for one more network ACL");

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );

    // Every member that write set round-trips through the describe.
    let acl = describe_one("the late ACL", &router, &late_id).await;
    assert_eq!(
        acl,
        ReportedAcl {
            network_acl_id: late_id.clone(),
            vpc_id: seeded.fixture_vpc_id.clone(),
            owner_id: engine::emulator_account_id().to_string(),
            is_default: "false".to_string(),
            entries: vec![ReportedEntry {
                rule_number: "300".to_string(),
                rule_action: "deny".to_string(),
                protocol: "17".to_string(),
                cidr_block: Some("198.51.100.0/24".to_string()),
                ipv6_cidr_block: None,
                egress: "false".to_string(),
                port_range: Some(("161".to_string(), "162".to_string())),
                icmp_type_code: None,
            }],
            associations: Vec::new(),
            tags: vec![("Name".to_string(), "late".to_string())],
        },
        "the late ACL: expected the record as it was stored"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute; values within one filter are OR'd,
/// and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_network_acls_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = (0..seeded.ids.len()).collect();

    // One row per filter name `DescribeNetworkAclsRequest$Filters` documents, each asserted
    // on an ACL the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "association.association-id",
            vec![seeded.moved_association_id.clone()],
            vec![2],
        ),
        (
            "association.network-acl-id",
            vec![seeded.ids[1].clone()],
            vec![1],
        ),
        (
            "association.subnet-id",
            vec![seeded.default_subnet_id.clone()],
            vec![0],
        ),
        (
            "association.subnet-id",
            vec![seeded.fixture_subnet_id.clone()],
            vec![1],
        ),
        (
            "association.subnet-id",
            vec![seeded.moved_subnet_id.clone()],
            vec![2],
        ),
        // Two subnets in one filter: joined with OR.
        (
            "association.subnet-id",
            vec![
                seeded.fixture_subnet_id.clone(),
                seeded.moved_subnet_id.clone(),
            ],
            vec![1, 2],
        ),
        ("default", vec!["true".into()], vec![0, 1]),
        ("default", vec!["false".into()], vec![2, 3, 4, 5]),
        ("entry.cidr", vec!["0.0.0.0/0".into()], vec![0, 1, 2, 3, 5]),
        ("entry.cidr", vec!["10.0.0.0/16".into()], vec![2]),
        ("entry.cidr", vec!["192.0.2.0/24".into()], vec![4]),
        // A range no entry names: an empty set, not an error.
        ("entry.cidr", vec!["10.9.0.0/24".into()], vec![]),
        ("entry.egress", vec!["true".into()], vec![0, 1, 2, 3]),
        ("entry.egress", vec!["false".into()], all.clone()),
        ("entry.icmp.code", vec!["-1".into()], vec![3]),
        ("entry.icmp.type", vec!["8".into()], vec![3]),
        ("entry.icmp.type", vec!["0".into()], vec![]),
        ("entry.ipv6-cidr", vec!["::/0".into()], vec![3, 4]),
        ("entry.port-range.from", vec!["80".into()], vec![2]),
        ("entry.port-range.from", vec!["53".into()], vec![3]),
        ("entry.port-range.from", vec!["22".into()], vec![4]),
        ("entry.port-range.to", vec!["443".into()], vec![2]),
        ("entry.port-range.to", vec!["22".into()], vec![4]),
        // "The protocol specified in the entry (tcp | udp | icmp or a protocol number)":
        // both spellings select the entry that holds that protocol number.
        ("entry.protocol", vec!["6".into()], vec![2, 4]),
        ("entry.protocol", vec!["tcp".into()], vec![2, 4]),
        ("entry.protocol", vec!["17".into()], vec![3]),
        ("entry.protocol", vec!["udp".into()], vec![3]),
        ("entry.protocol", vec!["1".into()], vec![3]),
        ("entry.protocol", vec!["icmp".into()], vec![3]),
        ("entry.protocol", vec!["-1".into()], all.clone()),
        ("entry.rule-action", vec!["deny".into()], vec![
            0, 1, 2, 3, 4,
        ]),
        ("entry.rule-action", vec!["allow".into()], all.clone()),
        ("entry.rule-number", vec!["32767".into()], vec![0, 1, 2, 3]),
        ("entry.rule-number", vec!["110".into()], vec![3]),
        ("entry.rule-number", vec!["200".into()], vec![4]),
        ("entry.rule-number", vec!["100".into()], vec![0, 1, 2, 5]),
        ("network-acl-id", vec![seeded.ids[3].clone()], vec![3]),
        (
            "network-acl-id",
            vec![seeded.ids[0].clone(), seeded.ids[4].clone()],
            vec![0, 4],
        ),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".into()], vec![]),
        ("tag-key", vec!["Owner".into()], vec![2]),
        ("tag-key", vec!["Name".into()], vec![2, 3]),
        ("tag-key", vec!["Project".into()], vec![5]),
        ("tag:Name", vec!["web".into()], vec![2]),
        ("tag:Name", vec!["batch".into()], vec![3]),
        ("tag:Owner", vec!["TeamA".into()], vec![2]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("tag:Owner", vec!["teama".into()], vec![]),
        ("vpc-id", vec![seeded.default_vpc_id.clone()], vec![0, 5]),
        (
            "vpc-id",
            vec![seeded.fixture_vpc_id.clone()],
            vec![1, 2, 3, 4],
        ),
        // Filter values carry EC2's wildcards: `*` for zero or more characters, `?` for
        // exactly one ("List and filter using the CLI and API", the guide the modelled
        // `Filter` shape links to).
        ("entry.cidr", vec!["10.*".into()], vec![2]),
        ("entry.cidr", vec!["192.0.2.?/24".into()], vec![4]),
        ("tag:Name", vec!["we*".into()], vec![2]),
        ("tag:Name", vec!["*atch".into()], vec![3]),
        ("tag-key", vec!["*".into()], vec![2, 3, 5]),
        ("network-acl-id", vec!["acl-*".into()], all.clone()),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("default", vec![], vec![]),
        ("entry.cidr", vec![], vec![]),
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

/// One row of the AND table: the filters to send, and the indices of the ACLs that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the network ACLs matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let fixture_vpc_id = seeded.fixture_vpc_id.clone();
    let fixture_vpc_values = [fixture_vpc_id.as_str()];

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("default", &["false"][..]),
                ("vpc-id", &fixture_vpc_values[..]),
            ],
            vec![2, 3, 4],
        ),
        (
            vec![
                ("entry.protocol", &["tcp"][..]),
                ("entry.rule-action", &["deny"][..]),
            ],
            vec![2, 4],
        ),
        (
            vec![
                ("entry.protocol", &["tcp"][..]),
                ("entry.rule-action", &["deny"][..]),
                ("tag-key", &["Name"][..]),
            ],
            vec![2],
        ),
        // Each filter matches something, but no ACL matches both.
        (
            vec![("default", &["true"][..]), ("tag-key", &["Name"][..])],
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

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a defined name, a filter of a different describe, the underscore
    // spelling, a defined name with the wrong case, a defined name's singular, the tag
    // filter without its key separator, the tag filter with a separator but no key, and an
    // empty name.
    let cases = [
        "network-acl-ids",
        "is-default",
        "entry_cidr",
        "Entry.Cidr",
        "entry.cidrs",
        "association.subnet",
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
    }
}

// ---------------------------------------------------------------------------
// NetworkAclIds
// ---------------------------------------------------------------------------

/// `NetworkAclIds` narrows the report to the ids named, in store order whatever order they
/// were asked for.
#[tokio::test]
async fn network_acl_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![5],
        vec![2, 4],
        vec![4, 2],
        vec![3, 1, 5],
        (0..seeded.ids.len()).collect(),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("NetworkAclIds={borrowed:?}");
        let reported = described_ids(&router, &case, &indexed("NetworkAclId", &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        let expected = ids_at(&seeded, &expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A `NetworkAclIds` entry that is not a network ACL id at all is refused
/// `InvalidNetworkAclID.Malformed` BEFORE any lookup, and a well-formed id that names no ACL
/// of the account is refused `InvalidNetworkAclID.NotFound` — even alongside ids that do
/// exist.
#[tokio::test]
async fn a_network_acl_id_that_names_no_network_acl_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has and the 8-hex-digit id form of one (both absent
    // rather than malformed), an unknown id carried beside a known one in each order, then
    // values outside the issued id form at all — another resource's id, the prefix with no
    // suffix, the wrong digit count, a non-hex suffix and free text.
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec!["acl-0ffffffffffffffff".to_string()],
            "InvalidNetworkAclID.NotFound",
        ),
        (
            vec!["acl-0abc1234".to_string()],
            "InvalidNetworkAclID.NotFound",
        ),
        (
            vec![known.clone(), "acl-0ffffffffffffffff".to_string()],
            "InvalidNetworkAclID.NotFound",
        ),
        (
            vec!["acl-0ffffffffffffffff".to_string(), known.clone()],
            "InvalidNetworkAclID.NotFound",
        ),
        (
            vec![seeded.default_vpc_id.clone()],
            "InvalidNetworkAclID.Malformed",
        ),
        (vec!["acl-".to_string()], "InvalidNetworkAclID.Malformed"),
        (
            vec!["acl-0abc123".to_string()],
            "InvalidNetworkAclID.Malformed",
        ),
        (
            vec!["acl-0ghijklmnopqrstuv".to_string()],
            "InvalidNetworkAclID.Malformed",
        ),
        (
            vec!["not-a-network-acl-id".to_string()],
            "InvalidNetworkAclID.Malformed",
        ),
        (
            vec![known, "not-a-network-acl-id".to_string()],
            "InvalidNetworkAclID.Malformed",
        ),
    ];
    for (ids, expected_code) in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("NetworkAclIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("NetworkAclId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            reported_ids(&case, &body).is_empty(),
            "case {case:?}: a refused request reports no network ACL, got {body}"
        );
    }
}

/// The request-controlled factors of the filter match are bounded: a request carrying more
/// filters, more values in one filter, more bytes in one value or more ids than this
/// emulator evaluates is refused rather than served, and each bound's last accepted input
/// still answers.
#[tokio::test]
async fn a_request_past_the_match_bounds_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // At each bound: the largest accepted input, then one past it. The filters repeat a
    // documented name, so what differs between the two rows of a pair is only the count.
    let at_bound = filters(
        &(0..ec2::state::MAX_FILTERS)
            .map(|_| ("owner-id", &[][..]))
            .collect::<Vec<_>>(),
    );
    let (status, body) = describe(&router, &at_bound).await;
    assert_eq!(
        status, 200,
        "MAX_FILTERS filters: expected HTTP 200, got {status} with body {body}"
    );
    let past_bound = filters(
        &(0..=ec2::state::MAX_FILTERS)
            .map(|_| ("owner-id", &[][..]))
            .collect::<Vec<_>>(),
    );
    let (status, body) = describe(&router, &past_bound).await;
    assert_eq!(
        status, 400,
        "MAX_FILTERS + 1 filters: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "MAX_FILTERS + 1 filters: expected InvalidParameterValue, got {body}"
    );

    let values: Vec<String> = (0..=ec2::state::MAX_FILTER_VALUES)
        .map(|index| format!("111122223{index:03}"))
        .collect();
    let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
    let (status, body) = describe(
        &router,
        &filters(&[("owner-id", &borrowed[..ec2::state::MAX_FILTER_VALUES])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "MAX_FILTER_VALUES values: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = describe(&router, &filters(&[("owner-id", &borrowed)])).await;
    assert_eq!(
        status, 400,
        "MAX_FILTER_VALUES + 1 values: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "MAX_FILTER_VALUES + 1 values: expected InvalidParameterValue, got {body}"
    );

    let oversize = "x".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1);
    let (status, body) = describe(&router, &filters(&[("tag:Name", &[&oversize])])).await;
    assert_eq!(
        status, 400,
        "an oversize filter value: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "an oversize filter value: expected InvalidParameterValue, got {body}"
    );
    let at_bound = "x".repeat(ec2::state::MAX_FILTER_VALUE_BYTES);
    let (status, body) = describe(&router, &filters(&[("tag:Name", &[&at_bound])])).await;
    assert_eq!(
        status, 200,
        "a filter value at the byte bound: expected HTTP 200, got {status} with body {body}"
    );

    // One id past the id-list bound, with the rest of the list well-formed, so what is
    // refused is the COUNT rather than an id's form.
    let ids: Vec<String> = (0..=ec2::state::MAX_RESOURCE_IDS)
        .map(|index| format!("acl-{index:017x}"))
        .collect();
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &indexed("NetworkAclId", &borrowed)).await;
    assert_eq!(
        status, 400,
        "MAX_RESOURCE_IDS + 1 ids: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "MAX_RESOURCE_IDS + 1 ids: expected InvalidParameterValue, got {body}"
    );

    // The bounds refuse nothing an ordinary request carries.
    let reported = described_ids(&router, "an ordinary request", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "an ordinary request: expected the whole store, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every network ACL exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_network_acl_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeNetworkAclsMaxResults` is modelled `range 5..1000`, so the rows are its
    // lower published bound, a size that holds the store exactly, one that does not divide
    // it, and the size that holds everything.
    for max_results in [5, 6, 1000] {
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
            let page = reported_ids(&case, &body);
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
            match super::ec2_query::envelope_children(&case, &body, "DescribeNetworkAcls", "nextToken")
                .first()
            {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected every network ACL exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page size is a cut of the filtered set, not of the whole store: `MaxResults` and a
/// filter compose, and a token is issued only while a further ACL follows the page.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set_and_a_token_follows_only_a_truncated_page() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("default", &["false"])]);
    params.extend(max_results_param(5));
    let filtered = described_ids(&router, "filtered first page", &params).await;
    assert_eq!(
        filtered,
        ids_at(&seeded, &[2, 3, 4, 5]),
        "filtered first page: expected the four non-default ACLs, got {filtered:?}"
    );
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    assert!(
        super::ec2_query::envelope_children(
            "filtered first page",
            &body,
            "DescribeNetworkAcls",
            "nextToken"
        )
        .is_empty(),
        "filtered first page: the filtered set ends here, so no token is due, got {body}"
    );

    // The whole store at the same page size IS truncated, so a token is due and resuming
    // from it starts at the ACL after the page.
    let (status, body) = describe(&router, &max_results_param(5)).await;
    assert_eq!(status, 200, "first page: got {status} {body}");
    let token =
        super::ec2_query::envelope_children("first page", &body, "DescribeNetworkAcls", "nextToken")
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("first page: expected a NextToken in {body}"));
    let mut resumed = max_results_param(5);
    resumed.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "second page", &resumed).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[5]),
        "second page: expected the ACLs after the first five, got {second:?}"
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

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        reported_ids("DryRun=true", &body).is_empty(),
        "DryRun=true: a dry run reports no network ACL, got {body}"
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

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSecurityGroups`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeSecurityGroups`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with is
//! the account's security groups narrowed by the `GroupIds`, `GroupNames` and `Filters` the
//! request names and cut into `MaxResults`-sized pages, and a fresh account's store
//! already holds the default group its default VPC comes with. Every table row is a
//! distinct input along what that generalises over — which filter, how many values, how
//! many filters, which ids and names, which page size — so the default group is one row
//! among many rather than the specification.

use ec2::ops::types::{IpPermission, IpRange, Ipv6Range, PrefixListId, Tag, UserIdGroupPair};
use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupSpec};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. The security-group store holds the account's
/// groups rather than one Region's, and the default VPC's default group is seeded for the
/// emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The prefix list an inbound rule of one seeded group allows access from.
const SEEDED_INBOUND_PREFIX_LIST: &str = "pl-0123456789abcdef0";

/// The prefix list an outbound rule of that group allows access to.
const SEEDED_OUTBOUND_PREFIX_LIST: &str = "pl-0abcdef0123456789";

/// One `DescribeSecurityGroups` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecurityGroups", params).await
}

/// The group ids a response reports, in the order it reported them.
///
/// Read off each group's own `securityGroupArn` rather than off `<groupId>`: a `groupId`
/// element also appears inside a rule that references another group, so the ARN — one per
/// reported group, ending in that group's id — is what identifies the rows themselves.
fn reported_ids(xml: &str) -> Vec<String> {
    elements(xml, "securityGroupArn")
        .iter()
        .filter_map(|arn| arn.rsplit('/').next().map(str::to_string))
        .collect()
}

/// The group ids a successful response reports, in the order it reported them.
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
    reported_ids(&body)
}

/// A tag, as the seeded groups carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// A rule with nothing in it, for the builders below to fill one member of.
fn empty_rule() -> IpPermission {
    IpPermission {
        ip_protocol: None,
        from_port: None,
        to_port: None,
        user_id_group_pairs: Some(Vec::new()),
        ip_ranges: Some(Vec::new()),
        ipv6_ranges: Some(Vec::new()),
        prefix_list_ids: Some(Vec::new()),
    }
}

/// A rule allowing `protocol` on ports `from`..`to` from or to an IPv4 CIDR block.
fn cidr_rule(protocol: &str, ports: Option<(i32, i32)>, cidr_ip: &str) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: ports.map(|(from, _)| from),
        to_port: ports.map(|(_, to)| to),
        ip_ranges: Some(vec![IpRange {
            cidr_ip: Some(cidr_ip.to_string()),
            description: None,
        }]),
        ..empty_rule()
    }
}

/// A rule allowing `protocol` from or to an IPv6 CIDR block.
fn ipv6_rule(protocol: &str, cidr_ipv6: &str) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        ipv6_ranges: Some(vec![Ipv6Range {
            cidr_ipv6: Some(cidr_ipv6.to_string()),
            description: None,
        }]),
        ..empty_rule()
    }
}

/// A rule allowing `protocol` on ports `from`..`to` from or to a prefix list.
fn prefix_list_rule(protocol: &str, ports: (i32, i32), prefix_list_id: &str) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: Some(ports.0),
        to_port: Some(ports.1),
        prefix_list_ids: Some(vec![PrefixListId {
            prefix_list_id: Some(prefix_list_id.to_string()),
            description: None,
        }]),
        ..empty_rule()
    }
}

/// A rule allowing `protocol` on ports `from`..`to` from or to another security group,
/// named by id, by name, or by both.
fn group_rule(
    protocol: &str,
    ports: (i32, i32),
    group_id: Option<&str>,
    group_name: Option<&str>,
    user_id: &str,
) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: Some(ports.0),
        to_port: Some(ports.1),
        user_id_group_pairs: Some(vec![UserIdGroupPair {
            group_id: group_id.map(str::to_string),
            group_name: group_name.map(str::to_string),
            user_id: Some(user_id.to_string()),
            description: None,
            vpc_id: None,
            vpc_peering_connection_id: None,
            peering_status: None,
        }]),
        ..empty_rule()
    }
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every group id, in the order `DescribeSecurityGroups` reports them (the store's key
    /// order): the default VPC's default group first, then the fixtures in creation order.
    ids: Vec<String>,
    /// The VPC the default group — and the fixtures that share it — is in.
    default_vpc_id: String,
    /// The second VPC, which the remaining fixtures are in.
    fixture_vpc_id: String,
}

/// Write the fixture groups into the engine's store, the way a creating operation would,
/// and report what the store then holds. They differ along every attribute the documented
/// filters select on: name, description, VPC, tags, and inbound and outbound rules naming
/// IPv4 and IPv6 blocks, prefix lists, other groups by id and by name, every protocol and
/// single ports — plus one group with no rules at all.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let account = engine::emulator_account_id().to_string();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let default_vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    let fixture_vpc_id = guard.insert_vpc("10.0.0.0/16", false, "default", "available", Vec::new());

    // 1: an ordinary web tier in the default VPC.
    let web = guard.insert_security_group(SecurityGroupSpec {
        vpc_id: default_vpc_id.clone(),
        group_name: "web".to_string(),
        description: "web tier".to_string(),
        region: REGION.to_string(),
        tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        ip_permissions: vec![cidr_rule("tcp", Some((80, 80)), "0.0.0.0/0")],
        ip_permissions_egress: vec![cidr_rule("-1", None, "0.0.0.0/0")],
    });
    // 2: a database tier whose inbound rule references the web group by id.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: default_vpc_id.clone(),
        group_name: "db".to_string(),
        description: "database tier".to_string(),
        region: REGION.to_string(),
        tags: vec![tag("Owner", "TeamB")],
        ip_permissions: vec![group_rule("tcp", (5432, 5432), Some(&web), None, &account)],
        ip_permissions_egress: vec![cidr_rule("tcp", Some((443, 443)), "10.0.0.0/8")],
    });
    // 3: IPv6 only, in the other VPC.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: fixture_vpc_id.clone(),
        group_name: "ipv6".to_string(),
        description: "ipv6 only".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: vec![ipv6_rule("icmpv6", "::/0")],
        ip_permissions_egress: vec![
            cidr_rule("-1", None, "0.0.0.0/0"),
            ipv6_rule("-1", "::/0"),
        ],
    });
    // 4: prefix lists on both sides.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: fixture_vpc_id.clone(),
        group_name: "prefixed".to_string(),
        description: "prefix list access".to_string(),
        region: REGION.to_string(),
        tags: vec![tag("Project", "atlas")],
        ip_permissions: vec![prefix_list_rule("udp", (53, 53), SEEDED_INBOUND_PREFIX_LIST)],
        ip_permissions_egress: vec![prefix_list_rule(
            "tcp",
            (443, 443),
            SEEDED_OUTBOUND_PREFIX_LIST,
        )],
    });
    // 5: every protocol from anywhere.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: fixture_vpc_id.clone(),
        group_name: "all-traffic".to_string(),
        description: "everything".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: vec![cidr_rule("-1", None, "0.0.0.0/0")],
        ip_permissions_egress: vec![cidr_rule("-1", None, "0.0.0.0/0")],
    });
    // 6: no rules at all, so every rule filter must pass it over.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: fixture_vpc_id.clone(),
        group_name: "empty".to_string(),
        description: "no rules at all".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    });
    // 7: references another group by NAME inbound and by ID outbound.
    guard.insert_security_group(SecurityGroupSpec {
        vpc_id: default_vpc_id.clone(),
        group_name: "named-ref".to_string(),
        description: "references by name".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: vec![group_rule("tcp", (22, 22), None, Some("web"), &account)],
        ip_permissions_egress: vec![group_rule("tcp", (9092, 9092), Some(&web), None, &account)],
    });

    Seeded {
        ids: guard.security_groups.keys().cloned().collect(),
        default_vpc_id,
        fixture_vpc_id,
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

/// A fresh account's default VPC already comes with its default security group — AWS
/// creates it with the VPC — and describing it is a READ: two calls report the same group
/// and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_the_default_group_of_its_default_vpc_and_reading_changes_nothing()
{
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {body}"
    );
    let first_ids = reported_ids(&body);
    assert_eq!(
        first_ids.len(),
        1,
        "fresh account: expected exactly the default group, got {first_ids:?} from {body}"
    );

    // The VPC it belongs to is the one `DescribeVpcs` reports as the default, so the two
    // describes agree about the account's default network.
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

    let expected_members = [
        ("groupId", first_ids[0].clone()),
        ("groupName", "default".to_string()),
        ("groupDescription", "default VPC security group".to_string()),
        ("vpcId", default_vpc_id),
        ("ownerId", engine::emulator_account_id().to_string()),
        (
            "securityGroupArn",
            engine::emulator_arn("ec2", REGION, &format!("security-group/{}", first_ids[0])),
        ),
        // "All inbound traffic from network interfaces assigned to the same security
        // group", so the inbound rule names this group itself, for every protocol.
        ("ipProtocol", "-1".to_string()),
        // "All outbound IPv4 traffic", so the outbound rule names every address.
        ("cidrIp", "0.0.0.0/0".to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "fresh account: expected <{member}>{value}</{member}>, got {body}"
        );
    }
    // The inbound rule's own reference is to this group, by id.
    let referenced = elements(&body, "groupId");
    assert!(
        referenced.iter().filter(|id| **id == first_ids[0]).count() >= 2,
        "fresh account: expected the inbound rule to reference the group itself, got \
         {referenced:?} from {body}"
    );

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().security_groups.len(),
        1,
        "describing security groups must not write to the store"
    );
}

/// Every group the store holds is reported, in store order, and a group written after a
/// call shows up in the next one with the members it was stored with — the answer is
/// computed from the store, not fixed.
#[tokio::test]
async fn every_stored_group_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all groups", &[]).await;
    assert_eq!(
        reported, seeded.ids,
        "all groups: expected every stored group in store order, got {reported:?}"
    );

    let account = engine::emulator_account_id().to_string();
    let late_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_security_group(SecurityGroupSpec {
            vpc_id: seeded.fixture_vpc_id.clone(),
            group_name: "late".to_string(),
            description: "written after the first read".to_string(),
            region: REGION.to_string(),
            tags: vec![tag("Name", "late")],
            ip_permissions: vec![cidr_rule("tcp", Some((9092, 9094)), "192.168.0.0/24")],
            ip_permissions_egress: vec![ipv6_rule("udp", "2600:1f14::/36")],
        });

    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = seeded.ids.clone();
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );

    // Every member that write set round-trips through the describe.
    let (status, body) = describe(&router, &indexed("GroupId", &[&late_id])).await;
    assert_eq!(
        status, 200,
        "the late group: expected HTTP 200, got {status} with body {body}"
    );
    let expected_members = [
        ("groupId", late_id.clone()),
        ("groupName", "late".to_string()),
        (
            "groupDescription",
            "written after the first read".to_string(),
        ),
        ("vpcId", seeded.fixture_vpc_id.clone()),
        ("ownerId", account),
        (
            "securityGroupArn",
            engine::emulator_arn("ec2", REGION, &format!("security-group/{late_id}")),
        ),
        ("ipProtocol", "tcp".to_string()),
        ("fromPort", "9092".to_string()),
        ("toPort", "9094".to_string()),
        ("cidrIp", "192.168.0.0/24".to_string()),
        ("cidrIpv6", "2600:1f14::/36".to_string()),
        ("key", "Name".to_string()),
        ("value", "late".to_string()),
    ];
    for (member, value) in expected_members {
        assert!(
            body.contains(&format!("<{member}>{value}</{member}>")),
            "the late group: expected <{member}>{value}</{member}>, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute; values within one filter are OR'd,
/// and the comparison is exact and case-sensitive with EC2's `*` wildcard.
#[tokio::test]
async fn each_documented_filter_selects_the_groups_whose_attribute_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let web = seeded.ids[1].clone();
    let default_group = seeded.ids[0].clone();

    // (filter name, values, the indices of the groups it must select)
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("description", vec!["web tier".to_string()], vec![1]),
        ("description", vec!["*tier".to_string()], vec![1, 2]),
        ("group-name", vec!["db".to_string()], vec![2]),
        (
            "group-name",
            vec!["default".to_string(), "web".to_string()],
            vec![0, 1],
        ),
        ("group-name", vec!["DB".to_string()], vec![]),
        ("group-name", Vec::new(), vec![]),
        ("group-id", vec![seeded.ids[3].clone()], vec![3]),
        ("owner-id", vec![account.clone()], vec![0, 1, 2, 3, 4, 5, 6, 7]),
        (
            "vpc-id",
            vec![seeded.default_vpc_id.clone()],
            vec![0, 1, 2, 7],
        ),
        (
            "vpc-id",
            vec![seeded.fixture_vpc_id.clone()],
            vec![3, 4, 5, 6],
        ),
        ("tag-key", vec!["Owner".to_string()], vec![1, 2]),
        ("tag:Owner", vec!["TeamA".to_string()], vec![1]),
        ("tag:Project", vec!["atlas".to_string()], vec![4]),
        ("ip-permission.cidr", vec!["0.0.0.0/0".to_string()], vec![1, 5]),
        ("ip-permission.from-port", vec!["80".to_string()], vec![1]),
        ("ip-permission.to-port", vec!["5432".to_string()], vec![2]),
        ("ip-permission.protocol", vec!["-1".to_string()], vec![0, 5]),
        (
            "ip-permission.protocol",
            vec!["tcp".to_string()],
            vec![1, 2, 7],
        ),
        (
            "ip-permission.protocol",
            vec!["tcp".to_string(), "udp".to_string()],
            vec![1, 2, 4, 7],
        ),
        ("ip-permission.group-id", vec![web.clone()], vec![2]),
        (
            "ip-permission.group-id",
            vec![default_group.clone()],
            vec![0],
        ),
        ("ip-permission.group-name", vec!["web".to_string()], vec![7]),
        (
            "ip-permission.user-id",
            vec![account.clone()],
            vec![0, 2, 7],
        ),
        ("ip-permission.ipv6-cidr", vec!["::/0".to_string()], vec![3]),
        (
            "ip-permission.prefix-list-id",
            vec![SEEDED_INBOUND_PREFIX_LIST.to_string()],
            vec![4],
        ),
        (
            "egress.ip-permission.cidr",
            vec!["0.0.0.0/0".to_string()],
            vec![0, 1, 3, 5],
        ),
        (
            "egress.ip-permission.cidr",
            vec!["10.0.0.0/8".to_string()],
            vec![2],
        ),
        (
            "egress.ip-permission.from-port",
            vec!["443".to_string()],
            vec![2, 4],
        ),
        (
            "egress.ip-permission.to-port",
            vec!["9092".to_string()],
            vec![7],
        ),
        (
            "egress.ip-permission.protocol",
            vec!["tcp".to_string()],
            vec![2, 4, 7],
        ),
        (
            "egress.ip-permission.ipv6-cidr",
            vec!["::/0".to_string()],
            vec![3],
        ),
        (
            "egress.ip-permission.prefix-list-id",
            vec![SEEDED_OUTBOUND_PREFIX_LIST.to_string()],
            vec![4],
        ),
        ("egress.ip-permission.group-id", vec![web], vec![7]),
        (
            "egress.ip-permission.user-id",
            vec![account.clone()],
            vec![7],
        ),
        (
            "egress.ip-permission.group-name",
            vec!["web".to_string()],
            vec![],
        ),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("filter {name} = {values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of [`multiple_filters_are_joined_with_an_and`]: the filters to send, and the
/// indices of the groups they must select together.
type AndCase = (Vec<(&'static str, Vec<String>)>, Vec<usize>);

/// Several filters are joined with an AND: a group is reported only when every one of them
/// holds for it.
#[tokio::test]
async fn multiple_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let default_vpc = seeded.default_vpc_id.clone();
    let fixture_vpc = seeded.fixture_vpc_id.clone();
    let cases: Vec<AndCase> = vec![
        (
            vec![
                ("vpc-id", vec![default_vpc.clone()]),
                ("ip-permission.protocol", vec!["tcp".to_string()]),
            ],
            vec![1, 2, 7],
        ),
        (
            vec![
                ("vpc-id", vec![default_vpc.clone()]),
                ("ip-permission.protocol", vec!["tcp".to_string()]),
                ("tag-key", vec!["Owner".to_string()]),
            ],
            vec![1, 2],
        ),
        (
            vec![
                ("vpc-id", vec![fixture_vpc]),
                ("ip-permission.cidr", vec!["0.0.0.0/0".to_string()]),
            ],
            vec![5],
        ),
        (
            // Each filter holds for some group, but no group satisfies both.
            vec![
                ("group-name", vec!["empty".to_string()]),
                ("ip-permission.protocol", vec!["tcp".to_string()]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("filters {entries:?}");
        let borrowed: Vec<(&str, Vec<&str>)> = entries
            .iter()
            .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
            .collect();
        let params: Vec<(&str, &[&str])> = borrowed
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(&router, &case, &filters(&params)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not document is refused — whether or not any group
/// would have matched it, so an unsupported filter can never read as "no results".
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Each row is a name that is NOT one of the 24 documented ones: a misspelling, a
    // filter of another resource, an alternative spelling this operation does not publish,
    // and a bare `tag:` with no key.
    for name in [
        "groupName",
        "subnet-id",
        "ip-permission.cidr-block",
        "tag:",
        "",
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["anything"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: expected the message to name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// GroupIds and GroupNames
// ---------------------------------------------------------------------------

/// `GroupIds` and `GroupNames` narrow the store to the groups they name, and a group named
/// by either one is reported.
#[tokio::test]
async fn group_ids_and_group_names_select_the_groups_they_name() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // (the ids named, the names named, the indices expected)
    let cases: Vec<(Vec<usize>, Vec<&str>, Vec<usize>)> = vec![
        (vec![], vec![], vec![0, 1, 2, 3, 4, 5, 6, 7]),
        (vec![3], vec![], vec![3]),
        (vec![5, 1], vec![], vec![1, 5]),
        (vec![], vec!["default"], vec![0]),
        (vec![], vec!["web", "empty"], vec![1, 6]),
        (vec![2], vec!["ipv6"], vec![2, 3]),
        // The same group by both: it is reported once, not twice.
        (vec![1], vec!["web"], vec![1]),
    ];

    for (id_indices, names, expected_indices) in cases {
        let case = format!("ids {id_indices:?} names {names:?}");
        let ids = ids_at(&seeded, &id_indices);
        let mut params = indexed(
            "GroupId",
            &ids.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        params.extend(indexed("GroupName", &names));
        let reported = described_ids(&router, &case, &params).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// An id or a name no group of the account carries is an error, not an absent row.
#[tokio::test]
async fn an_unknown_group_id_or_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // (the parameter, the value it carries)
    let cases = [
        ("GroupId", "sg-00000000000000999".to_string()),
        ("GroupId", "sg-0123abcd".to_string()),
        ("GroupName", "no-such-group".to_string()),
        // A known id under the wrong parameter is as unknown as any other name.
        ("GroupName", seeded.ids[1].clone()),
    ];

    for (parameter, value) in cases {
        let case = format!("{parameter} = {value}");
        let (status, body) = describe(&router, &indexed(parameter, &[&value])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidGroup.NotFound",
            "case {case:?}: expected InvalidGroup.NotFound, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The security group '{value}' does not exist")),
            "case {case:?}: expected the message to name the group, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination, DryRun and tokens
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports the whole store exactly once, in store order; a request naming no
/// `MaxResults` is not paged at all ("If this parameter is not specified, then all items
/// are returned").
#[tokio::test]
async fn max_results_pages_the_store_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The model publishes `range 5..1000` for `MaxResults`, so the rows are the smallest
    // page the operation accepts, a page the store does not fill, exactly the store's size
    // and the largest page published.
    for page_size in [5, 6, 8, 1000] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![("MaxResults".to_string(), page_size.to_string())];
            if let Some(next) = token.clone() {
                params.push(("NextToken".to_string(), next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} groups in a page, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= 10,
                "case {case:?}: pagination did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, seeded.ids,
            "case {case:?}: expected the whole store once in store order, got {walked:?}"
        );
    }

    let unpaged = described_ids(&router, "no MaxResults", &[]).await;
    assert_eq!(
        unpaged, seeded.ids,
        "no MaxResults: expected every group in one answer, got {unpaged:?}"
    );
}

/// A `NextToken` this service did not issue is refused rather than read as a cursor.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // A token is opaque: hex that decodes to nothing of this service's, a plain group id,
    // and text that is not hex at all.
    for token in ["00", &seeded.ids[0], "not-a-token"] {
        let (status, body) = describe(
            &router,
            &[("NextToken".to_string(), (*token).to_string())],
        )
        .await;
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

/// `DryRun` reports the permission outcome and reads nothing: "If you have the required
/// permissions, the error response is DryRunOperation".
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(&router, &[("DryRun".to_string(), "true".to_string())]).await;
    assert_eq!(
        status, 412,
        "dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry run: expected DryRunOperation, got {body}"
    );

    // And the store is untouched, so a later caller sees exactly what it did before.
    let after = described_ids(&router, "after a dry run", &[]).await;
    assert_eq!(
        after, seeded.ids,
        "after a dry run: expected the store unchanged, got {after:?}"
    );
}

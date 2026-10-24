// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeSecurityGroupRules`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeSecurityGroupRules`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that a security group's rules are RESOURCES OF THEIR OWN: every
//! single source or destination a group grants is one rule, with its own minted
//! `SecurityGroupRuleId` and ARN, carrying the direction, protocol and port range of the
//! permission it came from and the one grant it names — and this operation reports those
//! rules out of state, narrowed by the `SecurityGroupRuleIds` and `Filters` the request
//! names and cut into `MaxResults`-sized pages. Every table row is a distinct input along
//! what that generalises over — the grant's kind (IPv4 block, IPv6 block, prefix list,
//! referenced group), how many grants one permission names, the direction, the protocol,
//! the port range, whether the grant carries a description, which filter, which ids and
//! which page size — so the allow-all outbound rule a new group carries is one row among
//! many rather than the specification.
//!
//! The last two suites pin the bounds the store is held to: the published per-direction
//! rule quota, and the caller-chosen byte budget a delete gives back.

use std::collections::BTreeSet;

use ec2::ops::types::{IpPermission, IpRange, Ipv6Range, PrefixListId, Tag, UserIdGroupPair};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, STORED_RECORD_OVERHEAD_BYTES, SecurityGroupSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, indexed, unescaped};
use super::wire::{child, child_text, document, is_long_id, param};

/// The Region every call here is signed for. The security-group store holds the account's
/// groups rather than one Region's, and the default VPC's default group is seeded for the
/// emulator's default Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// A prefix list one seeded group's rules name.
const SEEDED_PREFIX_LIST: &str = "pl-0123456789abcdef0";

/// A second prefix list, so a prefix-list permission can name more than one.
const OTHER_PREFIX_LIST: &str = "pl-0abcdef0123456789";

/// One `DescribeSecurityGroupRules` call carrying `params`, as its HTTP status and XML
/// body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecurityGroupRules", params).await
}

/// The account every ARN this emulator mints names.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

// ---------------------------------------------------------------------------
// Reading the reported rules off the ec2Query wire
// ---------------------------------------------------------------------------

/// One security group rule as a response reports it. Every member is read as text, the way
/// the wire carries it, so a row's expectation is written exactly as the XML must read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    rule_id: String,
    group_id: String,
    group_owner_id: Option<String>,
    is_egress: Option<String>,
    ip_protocol: Option<String>,
    from_port: Option<String>,
    to_port: Option<String>,
    cidr_ipv4: Option<String>,
    cidr_ipv6: Option<String>,
    prefix_list_id: Option<String>,
    referenced_group_id: Option<String>,
    referenced_user_id: Option<String>,
    description: Option<String>,
    arn: Option<String>,
}

/// The grant a rule names and the shape of the permission it came from — everything about
/// a rule except the id and ARN the service mints, which a row cannot predict.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Grant {
    is_egress: &'static str,
    ip_protocol: &'static str,
    from_port: Option<&'static str>,
    to_port: Option<&'static str>,
    cidr_ipv4: Option<&'static str>,
    cidr_ipv6: Option<&'static str>,
    prefix_list_id: Option<&'static str>,
    /// Whether the rule references a group, and which: `Some(index)` names the seeded group
    /// at that index of [`Seeded::ids`], so a row can expect a reference without knowing
    /// the minted id.
    referenced_group: Option<usize>,
    description: Option<&'static str>,
}

/// A grant with nothing in it, for the rows below to fill the members one rule carries.
fn grant() -> Grant {
    Grant {
        is_egress: "false",
        ip_protocol: "-1",
        from_port: None,
        to_port: None,
        cidr_ipv4: None,
        cidr_ipv6: None,
        prefix_list_id: None,
        referenced_group: None,
        description: None,
    }
}

/// The rules a response reports, in the order it reported them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "securityGroupRuleSet")
    else {
        panic!("case {case:?}: no <securityGroupRuleSet> in {body}");
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| Reported {
            rule_id: child_text(item, "securityGroupRuleId").unwrap_or_default(),
            group_id: child_text(item, "groupId").unwrap_or_default(),
            group_owner_id: child_text(item, "groupOwnerId"),
            is_egress: child_text(item, "isEgress"),
            ip_protocol: child_text(item, "ipProtocol"),
            from_port: child_text(item, "fromPort"),
            to_port: child_text(item, "toPort"),
            cidr_ipv4: child_text(item, "cidrIpv4"),
            cidr_ipv6: child_text(item, "cidrIpv6"),
            prefix_list_id: child_text(item, "prefixListId"),
            referenced_group_id: child(item, "referencedGroupInfo")
                .and_then(|info| child_text(info, "groupId")),
            referenced_user_id: child(item, "referencedGroupInfo")
                .and_then(|info| child_text(info, "userId")),
            description: child_text(item, "description"),
            arn: child_text(item, "securityGroupRuleArn"),
        })
        .collect()
}

/// The rules a successful response reports, in the order it reported them.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Reported> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported(case, &body)
}

/// Every rule of `group_id`, as the `group-id` filter selects them.
async fn rules_of(router: &MultiServiceRouter, case: &str, group_id: &str) -> Vec<Reported> {
    described(router, case, &filters(&[("group-id", &[group_id])])).await
}

// ---------------------------------------------------------------------------
// The store the tables are written against
// ---------------------------------------------------------------------------

/// One row of the rule-id selection table: the case, the ids the request names, and the
/// ids it must answer with.
type SelectionCase = (&'static str, Vec<String>, Vec<String>);

/// The filters of one row of the filter table: each filter's name and its values.
type FilterEntries = Vec<(&'static str, Vec<String>)>;

/// One row of the filter table: the case, the filters it sends, and the rule ids it must
/// answer with.
type FilterCase = (&'static str, FilterEntries, Vec<String>);

/// The grant of one reported rule, as the default-group assertion compares them: whether it
/// is outbound, its protocol, the IPv4 block it names and the group it references.
type ReportedGrant = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every group id, in the order the store holds them: the default VPC's default group
    /// first, then the fixtures in creation order.
    ids: Vec<String>,
}

/// A tag, as a seeded rule carries them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// A permission with nothing in it, for the builders below to fill one grant kind of.
fn empty_permission() -> IpPermission {
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

/// `protocol` on `ports` from or to every IPv4 block of `blocks`, each with its own
/// description.
fn ipv4_permission(
    protocol: &str,
    ports: Option<(i32, i32)>,
    blocks: &[(&str, Option<&str>)],
) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: ports.map(|(from, _)| from),
        to_port: ports.map(|(_, to)| to),
        ip_ranges: Some(
            blocks
                .iter()
                .map(|(cidr_ip, description)| IpRange {
                    cidr_ip: Some((*cidr_ip).to_string()),
                    description: description.map(str::to_string),
                })
                .collect(),
        ),
        ..empty_permission()
    }
}

/// `protocol` on `ports` from or to every IPv6 block of `blocks`.
fn ipv6_permission(
    protocol: &str,
    ports: Option<(i32, i32)>,
    blocks: &[(&str, Option<&str>)],
) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: ports.map(|(from, _)| from),
        to_port: ports.map(|(_, to)| to),
        ipv6_ranges: Some(
            blocks
                .iter()
                .map(|(cidr_ipv6, description)| Ipv6Range {
                    cidr_ipv6: Some((*cidr_ipv6).to_string()),
                    description: description.map(str::to_string),
                })
                .collect(),
        ),
        ..empty_permission()
    }
}

/// `protocol` on `ports` from or to every prefix list of `lists`.
fn prefix_list_permission(
    protocol: &str,
    ports: Option<(i32, i32)>,
    lists: &[&str],
) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: ports.map(|(from, _)| from),
        to_port: ports.map(|(_, to)| to),
        prefix_list_ids: Some(
            lists
                .iter()
                .map(|prefix_list_id| PrefixListId {
                    prefix_list_id: Some((*prefix_list_id).to_string()),
                    description: None,
                })
                .collect(),
        ),
        ..empty_permission()
    }
}

/// `protocol` on `ports` from or to every security group of `groups`, by id.
fn group_permission(protocol: &str, ports: Option<(i32, i32)>, groups: &[&str]) -> IpPermission {
    IpPermission {
        ip_protocol: Some(protocol.to_string()),
        from_port: ports.map(|(from, _)| from),
        to_port: ports.map(|(_, to)| to),
        user_id_group_pairs: Some(
            groups
                .iter()
                .map(|group_id| UserIdGroupPair {
                    group_id: Some((*group_id).to_string()),
                    user_id: Some(account_id()),
                    group_name: None,
                    description: None,
                    vpc_id: None,
                    vpc_peering_connection_id: None,
                    peering_status: None,
                })
                .collect(),
        ),
        ..empty_permission()
    }
}

/// The permissions one fixture group holds, in each direction, built from the seeded group
/// ids so a row can reference another group: `(inbound, outbound)`.
type Permissions = fn(&[String]) -> (Vec<IpPermission>, Vec<IpPermission>);

/// One fixture group: what it is called in an assertion message, the permissions it holds
/// in each direction, and the rules those permissions must become.
struct GroupCase {
    case: &'static str,
    group_name: &'static str,
    permissions: Permissions,
    /// The rules the group must report, in `SecurityGroupRuleId` order — which is the order
    /// the store minted them in: inbound permissions first, then outbound, and inside one
    /// permission IPv4 blocks, IPv6 blocks, prefix lists and referenced groups.
    expected: Vec<Grant>,
}

/// The fixture groups, spanning every grant kind, both directions, several protocols and
/// port ranges, permissions naming one grant and several, and a group with no rules at all.
fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            case: "one inbound IPv4 block on a single port",
            group_name: "web",
            permissions: |_| {
                (
                    vec![ipv4_permission("tcp", Some((80, 80)), &[("0.0.0.0/0", None)])],
                    Vec::new(),
                )
            },
            expected: vec![Grant {
                ip_protocol: "tcp",
                from_port: Some("80"),
                to_port: Some("80"),
                cidr_ipv4: Some("0.0.0.0/0"),
                ..grant()
            }],
        },
        GroupCase {
            case: "one permission naming three IPv4 blocks is three rules",
            group_name: "three-blocks",
            permissions: |_| {
                (
                    vec![ipv4_permission(
                        "tcp",
                        Some((443, 443)),
                        &[
                            ("10.0.0.0/8", Some("the private range")),
                            ("192.168.0.0/16", None),
                            ("203.0.113.0/24", Some("the documentation range")),
                        ],
                    )],
                    Vec::new(),
                )
            },
            expected: vec![
                Grant {
                    ip_protocol: "tcp",
                    from_port: Some("443"),
                    to_port: Some("443"),
                    cidr_ipv4: Some("10.0.0.0/8"),
                    description: Some("the private range"),
                    ..grant()
                },
                Grant {
                    ip_protocol: "tcp",
                    from_port: Some("443"),
                    to_port: Some("443"),
                    cidr_ipv4: Some("192.168.0.0/16"),
                    ..grant()
                },
                Grant {
                    ip_protocol: "tcp",
                    from_port: Some("443"),
                    to_port: Some("443"),
                    cidr_ipv4: Some("203.0.113.0/24"),
                    description: Some("the documentation range"),
                    ..grant()
                },
            ],
        },
        GroupCase {
            case: "an IPv6 block and a port range, outbound",
            group_name: "ipv6-egress",
            permissions: |_| {
                (
                    Vec::new(),
                    vec![ipv6_permission(
                        "udp",
                        Some((1024, 65535)),
                        &[("::/0", Some("anywhere"))],
                    )],
                )
            },
            expected: vec![Grant {
                is_egress: "true",
                ip_protocol: "udp",
                from_port: Some("1024"),
                to_port: Some("65535"),
                cidr_ipv6: Some("::/0"),
                description: Some("anywhere"),
                ..grant()
            }],
        },
        GroupCase {
            case: "two prefix lists in one permission",
            group_name: "prefixed",
            permissions: |_| {
                (
                    vec![prefix_list_permission(
                        "udp",
                        Some((53, 53)),
                        &[SEEDED_PREFIX_LIST, OTHER_PREFIX_LIST],
                    )],
                    Vec::new(),
                )
            },
            expected: vec![
                Grant {
                    ip_protocol: "udp",
                    from_port: Some("53"),
                    to_port: Some("53"),
                    prefix_list_id: Some(SEEDED_PREFIX_LIST),
                    ..grant()
                },
                Grant {
                    ip_protocol: "udp",
                    from_port: Some("53"),
                    to_port: Some("53"),
                    prefix_list_id: Some(OTHER_PREFIX_LIST),
                    ..grant()
                },
            ],
        },
        GroupCase {
            case: "every grant kind in one group, both directions",
            group_name: "mixed",
            // Index 0 is the default group of the account's default VPC.
            permissions: |ids| {
                (
                    vec![
                        ipv4_permission("icmp", Some((-1, -1)), &[("172.16.0.0/12", None)]),
                        group_permission("tcp", Some((22, 22)), &[ids[0].as_str()]),
                    ],
                    vec![ipv4_permission("-1", None, &[("0.0.0.0/0", None)])],
                )
            },
            expected: vec![
                Grant {
                    ip_protocol: "icmp",
                    from_port: Some("-1"),
                    to_port: Some("-1"),
                    cidr_ipv4: Some("172.16.0.0/12"),
                    ..grant()
                },
                Grant {
                    ip_protocol: "tcp",
                    from_port: Some("22"),
                    to_port: Some("22"),
                    referenced_group: Some(0),
                    ..grant()
                },
                Grant {
                    is_egress: "true",
                    ip_protocol: "-1",
                    cidr_ipv4: Some("0.0.0.0/0"),
                    ..grant()
                },
            ],
        },
        GroupCase {
            case: "no rules at all",
            group_name: "empty",
            permissions: |_| (Vec::new(), Vec::new()),
            expected: Vec::new(),
        },
    ]
}

/// Write the fixture groups into the engine's store, the way a creating operation would,
/// and report what the store then holds.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    let mut ids: Vec<String> = guard.security_groups.keys().cloned().collect();

    for case in group_cases() {
        let (ip_permissions, ip_permissions_egress) = (case.permissions)(&ids);
        let group_id = guard.insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.clone(),
            group_name: case.group_name.to_string(),
            description: case.case.to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions,
            ip_permissions_egress,
        });
        ids.push(group_id);
    }

    Seeded { ids }
}

/// `grant` as the response must report it for the rule of `group_id`.
fn expected_of(grant: &Grant, group_id: &str, ids: &[String]) -> Reported {
    Reported {
        // The id and ARN are the service's own; the caller checks their FORM instead.
        rule_id: String::new(),
        group_id: group_id.to_string(),
        group_owner_id: Some(account_id()),
        is_egress: Some(grant.is_egress.to_string()),
        ip_protocol: Some(grant.ip_protocol.to_string()),
        from_port: grant.from_port.map(str::to_string),
        to_port: grant.to_port.map(str::to_string),
        cidr_ipv4: grant.cidr_ipv4.map(str::to_string),
        cidr_ipv6: grant.cidr_ipv6.map(str::to_string),
        prefix_list_id: grant.prefix_list_id.map(str::to_string),
        referenced_group_id: grant.referenced_group.map(|index| ids[index].clone()),
        referenced_user_id: grant.referenced_group.map(|_| account_id()),
        description: grant.description.map(str::to_string),
        arn: None,
    }
}

/// `reported` with the members no row can predict blanked, so a row's expectation compares
/// against exactly what it states.
fn comparable(rule: &Reported) -> Reported {
    Reported {
        rule_id: String::new(),
        arn: None,
        ..rule.clone()
    }
}

// ---------------------------------------------------------------------------
// Every grant is a rule of its own
// ---------------------------------------------------------------------------

/// Each single source or destination a group grants is reported as ONE rule, carrying the
/// direction, protocol and port range of the permission it came from, the grant it names,
/// its own description, a minted `sgr-` id and an ARN naming that id.
#[tokio::test]
async fn every_grant_a_group_makes_is_reported_as_a_rule_of_its_own() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = account_id();

    // The default group is seeded before the fixtures, so the fixture at table row `index`
    // is the group at `index + 1`.
    for (index, group) in group_cases().into_iter().enumerate() {
        let group_id = &seeded.ids[index + 1];
        let rules = rules_of(&router, group.case, group_id).await;
        let expected: Vec<Reported> = group
            .expected
            .iter()
            .map(|grant| expected_of(grant, group_id, &seeded.ids))
            .collect();
        let got: Vec<Reported> = rules.iter().map(comparable).collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:#?}, got {rules:#?}",
            case = group.case
        );
        for rule in &rules {
            assert!(
                is_long_id("sgr", &rule.rule_id),
                "case {case:?}: expected an sgr- long id, got {id:?}",
                case = group.case,
                id = rule.rule_id
            );
            let arn = rule.arn.clone().unwrap_or_default();
            assert!(
                arn.ends_with(&format!(":{account}:security-group-rule/{}", rule.rule_id)),
                "case {case:?}: expected an ARN naming the account and the rule, got {arn:?}",
                case = group.case
            );
        }
    }
}

/// A new security group carries exactly the one documented allow-all outbound rule, and the
/// default group of a VPC carries that rule plus the inbound rule that references itself —
/// both reported through this operation with ids of their own.
#[tokio::test]
async fn a_new_group_reports_its_default_egress_rule_and_the_default_group_reports_both() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = account_id();

    // The rows differ in the name and description a create chooses, which nothing about the
    // default rule depends on.
    let created: Vec<(&str, &str, &str)> = vec![
        ("a plain group", "web-tier", "HTTP from the load balancer"),
        ("a name with spaces", "edge group 7", "the edge"),
        ("a one-character name", "a", "b"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, group_name, description) in created {
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "CreateSecurityGroup",
            &[
                param("GroupName", group_name),
                param("GroupDescription", description),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} \
             with body {body}"
        );
        let group_id = element(&body, "groupId")
            .unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"));

        let rules = rules_of(&router, case, &group_id).await;
        assert_eq!(
            rules.len(),
            1,
            "case {case:?}: expected exactly one rule on a new group, got {rules:#?}"
        );
        let rule = &rules[0];
        assert_eq!(
            (
                rule.is_egress.as_deref(),
                rule.ip_protocol.as_deref(),
                rule.cidr_ipv4.as_deref(),
                rule.from_port.as_deref(),
                rule.to_port.as_deref(),
                rule.group_owner_id.as_deref(),
            ),
            (
                Some("true"),
                Some("-1"),
                Some("0.0.0.0/0"),
                None,
                None,
                Some(account.as_str()),
            ),
            "case {case:?}: expected the allow-all outbound rule, got {rule:#?}"
        );
        minted.push(rule.rule_id.clone());
    }

    // Every rule of the account has an id of its own, across groups.
    let all = described(&router, "the whole store", &[]).await;
    let ids: BTreeSet<&str> = all.iter().map(|rule| rule.rule_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        all.len(),
        "expected every reported rule to have its own id, got {all:#?}"
    );
    for rule_id in &minted {
        assert!(
            ids.contains(rule_id.as_str()),
            "expected the whole store to include {rule_id:?}, got {ids:?}"
        );
    }

    // The default group of the default VPC: the inbound rule references the group ITSELF,
    // and the outbound one allows everything.
    let (status, groups) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupName.1", ec2::state::DEFAULT_SECURITY_GROUP_NAME)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the default group must be describable, got {status} with body {groups}"
    );
    let default_group = element(&groups, "securityGroupArn")
        .and_then(|arn| arn.rsplit('/').next().map(str::to_string))
        .expect("the default group reports its ARN");
    let rules = rules_of(&router, "the default group", &default_group).await;
    let described_grants: Vec<ReportedGrant> = rules
        .iter()
        .map(|rule| {
            (
                rule.is_egress.clone(),
                rule.ip_protocol.clone(),
                rule.cidr_ipv4.clone(),
                rule.referenced_group_id.clone(),
            )
        })
        .collect();
    let expected_grants: Vec<ReportedGrant> = vec![
        (
            Some("true".to_string()),
            Some("-1".to_string()),
            Some("0.0.0.0/0".to_string()),
            None,
        ),
        (
            Some("false".to_string()),
            Some("-1".to_string()),
            None,
            Some(default_group.clone()),
        ),
    ];
    assert_eq!(
        described_grants, expected_grants,
        "the default group: expected the self-referencing inbound rule and the allow-all \
         outbound rule, got {rules:#?}"
    );
}

/// Reading the rules changes nothing a later caller sees: two identical describes answer
/// the same rules, with the same ids.
#[tokio::test]
async fn describing_the_rules_leaves_the_store_as_it_was() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    let first = described(&router, "the first read", &[]).await;
    let second = described(&router, "the second read", &[]).await;
    assert_eq!(
        first, second,
        "expected two reads to report the same rules, got {first:#?} then {second:#?}"
    );
    let held = engine.state::<Ec2State>().lock().security_group_rules.len();
    assert_eq!(
        held,
        first.len(),
        "expected the store to hold exactly the rules reported, got {held} for {first:#?}"
    );
}

// ---------------------------------------------------------------------------
// SecurityGroupRuleIds and Filters
// ---------------------------------------------------------------------------

/// `SecurityGroupRuleIds` narrows the answer to the rules it names, and names them in any
/// order; an id no rule of the account carries is an error, not an absent row.
#[tokio::test]
async fn security_group_rule_ids_select_the_rules_they_name() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);
    let all: Vec<String> = described(&router, "the whole store", &[])
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();
    assert!(
        all.len() >= 8,
        "the fixture must hold enough rules to select among, got {all:?}"
    );

    // (case, the ids named, the ids expected — always in store order)
    let cases: Vec<SelectionCase> = vec![
        ("none named", Vec::new(), all.clone()),
        ("one id", vec![all[2].clone()], vec![all[2].clone()]),
        (
            "two ids, in reverse order",
            vec![all[4].clone(), all[1].clone()],
            vec![all[1].clone(), all[4].clone()],
        ),
        (
            "the same id twice",
            vec![all[3].clone(), all[3].clone()],
            vec![all[3].clone()],
        ),
        (
            "every id",
            all.iter().rev().cloned().collect(),
            all.clone(),
        ),
    ];

    for (case, named, expected) in cases {
        let params = indexed(
            "SecurityGroupRuleId",
            &named.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let got: Vec<String> = described(&router, case, &params)
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }

    // An id of the right shape that names nothing, one of the wrong shape, and a group id
    // under the rule-id parameter.
    for unknown in [
        "sgr-00000000000000999".to_string(),
        "sgr-0123abcd".to_string(),
        "not-an-id".to_string(),
        all[0].clone().replace("sgr-", "sg-"),
    ] {
        let (status, body) = describe(&router, &indexed("SecurityGroupRuleId", &[&unknown])).await;
        assert_eq!(
            status, 400,
            "case {unknown:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSecurityGroupRuleId.NotFound",
            "case {unknown:?}: expected InvalidSecurityGroupRuleId.NotFound, got {body}"
        );
        assert!(
            unescaped(&body).contains(&format!("The security group rule ID '{unknown}' does not")),
            "case {unknown:?}: expected the message to name the rule, got {body}"
        );
    }
}

/// Each documented filter selects the rules whose attribute matches one of its values, and
/// several filters are joined with AND.
#[tokio::test]
async fn each_documented_filter_selects_the_rules_whose_attribute_matches() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Tag one rule and leave the rest untagged, the way a tagging writer would, so the
    // `tag:<key>` filter has something to select on.
    let all: Vec<String> = described(&router, "the whole store", &[])
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();
    let tagged = all[1].clone();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let record = guard
            .security_group_rules
            .get_mut(&tagged)
            .expect("the rule just reported is in the store");
        record.rule.tags = Some(vec![tag("Owner", "TeamA")]);
    }

    let default_group_rules: Vec<String> = rules_of(&router, "the default group", &seeded.ids[0])
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();
    let mixed_group = seeded.ids[5].clone();
    let mixed_rules: Vec<String> = rules_of(&router, "the mixed group", &mixed_group)
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();

    // (case, the filters, the rule ids expected in store order)
    let cases: Vec<FilterCase> = vec![
        (
            "group-id of the default group",
            vec![("group-id", vec![seeded.ids[0].clone()])],
            default_group_rules.clone(),
        ),
        (
            "group-id of a group with no rules",
            vec![("group-id", vec![seeded.ids[6].clone()])],
            Vec::new(),
        ),
        (
            "group-id naming two groups",
            vec![(
                "group-id",
                vec![seeded.ids[0].clone(), mixed_group.clone()],
            )],
            default_group_rules
                .iter()
                .chain(mixed_rules.iter())
                .cloned()
                .collect(),
        ),
        (
            "group-id of no group at all",
            vec![("group-id", vec!["sg-00000000000000999".to_string()])],
            Vec::new(),
        ),
        (
            "security-group-rule-id",
            vec![("security-group-rule-id", vec![all[2].clone()])],
            vec![all[2].clone()],
        ),
        (
            "security-group-rule-id with no values selects nothing",
            vec![("security-group-rule-id", Vec::new())],
            Vec::new(),
        ),
        (
            "tag:Owner",
            vec![("tag:Owner", vec!["TeamA".to_string()])],
            vec![tagged.clone()],
        ),
        (
            "tag:Owner with the wrong value",
            vec![("tag:Owner", vec!["TeamB".to_string()])],
            Vec::new(),
        ),
        (
            "tag:Owner on a key nothing carries",
            vec![("tag:Project", vec!["atlas".to_string()])],
            Vec::new(),
        ),
        (
            "two filters are joined with AND",
            vec![
                ("group-id", vec![seeded.ids[0].clone()]),
                (
                    "security-group-rule-id",
                    vec![default_group_rules[0].clone()],
                ),
            ],
            vec![default_group_rules[0].clone()],
        ),
        (
            "two filters nothing satisfies at once",
            vec![
                ("group-id", vec![mixed_group.clone()]),
                (
                    "security-group-rule-id",
                    vec![default_group_rules[0].clone()],
                ),
            ],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in cases {
        let borrowed: Vec<(&str, Vec<&str>)> = entries
            .iter()
            .map(|(name, values)| (*name, values.iter().map(String::as_str).collect()))
            .collect();
        let params = filters(
            &borrowed
                .iter()
                .map(|(name, values)| (*name, values.as_slice()))
                .collect::<Vec<_>>(),
        );
        let got: Vec<String> = described(&router, case, &params)
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// A filter name this operation does not document is refused, never read as "no results" —
/// including a name another describe of this service documents.
#[tokio::test]
async fn an_undocumented_filter_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    for name in [
        "Group-Id",
        "group-ids",
        "description",
        "ip-permission.protocol",
        "tag-key",
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
// The store is bounded
// ---------------------------------------------------------------------------

/// A group holds at most the published per-direction rule quota, and a permission that
/// would take it past that quota stores NOTHING rather than part of itself — so the store
/// can never grow past the bound and a group is never left with half of what was asked for.
/// The capacity predicate a writer asks first answers `false` exactly at the ceiling, and
/// counts the two directions apart.
///
/// The rows are the published bound itself, one grant past it, and far past it.
#[tokio::test]
async fn a_group_holds_at_most_the_published_rules_per_direction() {
    let ceiling = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;

    for asked in [ceiling, ceiling + 1, ceiling * 4] {
        let case = format!("{asked} inbound grants");
        let (router, engine) = super::ec2_query::emulator();
        // Each grant is a distinct block, so the rules differ in nothing but the block.
        let blocks: Vec<String> = (0..asked)
            .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
            .collect();
        let grants: Vec<(&str, Option<&str>)> = blocks
            .iter()
            .map(|block| (block.as_str(), None))
            .collect();
        let group_id = {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let vpc_id = guard
                .vpcs
                .values()
                .find(|vpc| vpc.is_default == Some(true))
                .and_then(|vpc| vpc.vpc_id.clone())
                .expect("a fresh account has its default VPC");
            // The group is created with no rules of its own — `CreateSecurityGroup` takes
            // none from the request — and the permission under test is then stored the way
            // an authorize stores one, which is where the per-direction quota applies.
            let group_id = guard.insert_security_group(SecurityGroupSpec {
                vpc_id,
                group_name: "at-the-ceiling".to_string(),
                description: case.clone(),
                region: REGION.to_string(),
                tags: Vec::new(),
                ip_permissions: Vec::new(),
                ip_permissions_egress: Vec::new(),
            });
            let minted = guard.insert_security_group_rules(
                &group_id,
                false,
                &ipv4_permission("tcp", Some((443, 443)), &grants),
                true,
                REGION,
            );
            let expected_mint = if asked <= ceiling { asked } else { 0 };
            assert_eq!(
                minted.len(),
                expected_mint,
                "case {case:?}: expected the insert to mint {expected_mint} ids, got {count}",
                count = minted.len()
            );
            group_id
        };

        // All of the permission or none of it: a batch that fits is stored whole, a batch
        // that does not fit is not stored at all.
        let expected = if asked <= ceiling { asked } else { 0 };
        let held = rules_of(&router, &case, &group_id).await;
        assert_eq!(
            held.len(),
            expected,
            "case {case:?}: expected {expected} inbound rules under the {ceiling} ceiling, \
             got {count}",
            count = held.len()
        );

        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        assert_eq!(
            guard.security_group_rule_count(&group_id, false),
            expected,
            "case {case:?}: expected the store to hold {expected} inbound rules"
        );
        // Capacity is what is left under the ceiling in THIS direction.
        let left = ceiling - expected;
        assert!(
            guard.has_security_group_rule_capacity(&group_id, false, left),
            "case {case:?}: expected room for the {left} rules still under the ceiling"
        );
        assert!(
            !guard.has_security_group_rule_capacity(&group_id, false, left + 1),
            "case {case:?}: expected no room for one rule more than the ceiling"
        );
        // The other direction is counted apart, because the quota is published per
        // direction.
        assert!(
            guard.has_security_group_rule_capacity(&group_id, true, ceiling),
            "case {case:?}: expected the outbound direction to still hold {ceiling} rules"
        );
        assert_eq!(
            guard.security_group_rule_count(&group_id, true),
            0,
            "case {case:?}: the group was given no outbound permission"
        );
    }
}

/// A permission that does not fit under the quota is refused whole: nothing is stored, no
/// id is answered, and what the group already held is untouched.
#[tokio::test]
async fn a_permission_that_does_not_fit_stores_nothing() {
    let ceiling = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let group_id = seeded.ids[1].clone();
    let before: Vec<String> = rules_of(&router, "before", &group_id)
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();

    // The rows are a batch one grant past the ceiling and one far past it; both are
    // refused, and in neither case is any part of them stored.
    for asked in [ceiling + 1, ceiling * 3] {
        let case = format!("{asked} grants at once");
        let blocks: Vec<String> = (0..asked)
            .map(|index| format!("172.{}.{}.0/24", 16 + index / 256, index % 256))
            .collect();
        let grants: Vec<(&str, Option<&str>)> = blocks
            .iter()
            .map(|block| (block.as_str(), None))
            .collect();
        let minted = {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let permission = ipv4_permission("tcp", Some((9000, 9000)), &grants);
            guard.insert_security_group_rules(&group_id, false, &permission, true, REGION)
        };
        assert!(
            minted.is_empty(),
            "case {case:?}: expected no rule to be minted, got {minted:?}"
        );
        let after: Vec<String> = rules_of(&router, &case, &group_id)
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        assert_eq!(
            after, before,
            "case {case:?}: expected the group's rules unchanged, got {after:?}"
        );
    }
}

/// A rule a REQUEST named is charged against the store's byte budget, and the delete of its
/// group gives back exactly what the insert took; a rule the SERVICE wrote is not charged,
/// because that budget bounds caller-supplied data.
///
/// The group under test is created THROUGH the API rather than seeded into the store, so
/// its own caller-chosen bytes are charged the way a request's are and the delete has
/// exactly them — and the rules' — to give back.
#[tokio::test]
async fn a_rule_a_request_named_is_charged_and_the_delete_gives_it_back() {
    let (router, engine) = super::ec2_query::emulator();
    let budget = || engine.state::<Ec2State>().lock().stored_bytes();
    let vpc_id = engine
        .state::<Ec2State>()
        .lock()
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");

    let before_all = budget();
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", "charged-rules"),
            param("GroupDescription", "the group the charged rules belong to"),
            param("VpcId", &vpc_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the create must succeed, got {status} with body {body}"
    );
    let group_id =
        element(&body, "groupId").expect("CreateSecurityGroup reports the group it minted");

    // The rows differ in the grant kind and in how much caller text the grant carries.
    let cases: Vec<(&str, IpPermission, usize)> = vec![
        (
            "an IPv4 block with a description",
            ipv4_permission(
                "tcp",
                Some((8080, 8080)),
                &[("198.51.100.0/24", Some("an audited source"))],
            ),
            "198.51.100.0/24".len() + "an audited source".len(),
        ),
        (
            "an IPv6 block with no description",
            ipv6_permission("tcp", Some((8081, 8081)), &[("2001:db8::/32", None)]),
            "2001:db8::/32".len(),
        ),
        (
            "two prefix lists",
            prefix_list_permission(
                "udp",
                Some((5353, 5353)),
                &[SEEDED_PREFIX_LIST, OTHER_PREFIX_LIST],
            ),
            SEEDED_PREFIX_LIST.len() + OTHER_PREFIX_LIST.len(),
        ),
    ];

    for (case, permission, expected) in cases {
        let before = budget();
        let minted = {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            guard.insert_security_group_rules(&group_id, false, &permission, true, REGION)
        };
        assert!(
            !minted.is_empty(),
            "case {case:?}: the permission must mint at least one rule"
        );
        let after = budget();
        // Each minted rule costs the fixed per-record overhead every store of this crate
        // charges, on top of the caller-chosen bytes of the grant it names.
        let expected = expected + STORED_RECORD_OVERHEAD_BYTES * minted.len();
        assert_eq!(
            after - before,
            expected,
            "case {case:?}: expected the store to charge {expected} caller-chosen bytes, \
             it went from {before} to {after}"
        );
        // And the minted rules are what the operation reports for the group.
        let reported_ids: Vec<String> = rules_of(&router, case, &group_id)
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        for rule_id in &minted {
            assert!(
                reported_ids.contains(rule_id),
                "case {case:?}: expected {rule_id:?} among {reported_ids:?}"
            );
        }
    }

    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DeleteSecurityGroup",
        &[param("GroupId", &group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete must succeed, got {status} with body {body}"
    );
    let after_delete = budget();
    assert_eq!(
        after_delete, before_all,
        "expected the delete to give back exactly what the create and the inserts charged, \
         the budget went from {before_all} to {after_delete}"
    );
}

// ---------------------------------------------------------------------------
// Pagination, tokens and DryRun
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports every rule exactly once, in store order; a request naming no `MaxResults`
/// is not paged at all.
#[tokio::test]
async fn max_results_pages_the_rules_and_the_token_resumes_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);
    let unpaged: Vec<String> = described(&router, "no MaxResults", &[])
        .await
        .into_iter()
        .map(|rule| rule.rule_id)
        .collect();

    // The model publishes `range 5..1000` for `MaxResults`, so the rows are the smallest
    // page the operation accepts, a page the store does not fill, and the largest page
    // published.
    for page_size in [5, 7, unpaged.len(), 1000] {
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
            let page = reported(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} rules in a page, got {page:#?}"
            );
            walked.extend(page.into_iter().map(|rule| rule.rule_id));
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= 20,
                "case {case:?}: pagination did not terminate, walked {walked:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, unpaged,
            "case {case:?}: expected every rule once in store order, got {walked:?}"
        );
    }
}

/// A request that names NO `MaxResults` is bounded by the documented maximum rather than by
/// the store: the first page carries at most `MAX_PAGE_SIZE` rules and a `NextToken`, and
/// the pages the token walks together report every rule exactly once.
///
/// The store is filled past one page through the published per-direction quota, so the row
/// is "more rules than a page holds" rather than any particular count.
#[tokio::test]
async fn a_request_naming_no_page_size_gets_at_most_the_documented_maximum() {
    use ec2::state::{MAX_PAGE_SIZE, MAX_SECURITY_GROUP_RULES_PER_DIRECTION};

    let case = "no MaxResults over a store of more than one page";
    let (router, engine) = super::ec2_query::emulator();
    let held = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let vpc_id = guard
            .vpcs
            .values()
            .find(|vpc| vpc.is_default == Some(true))
            .and_then(|vpc| vpc.vpc_id.clone())
            .expect("a fresh account has its default VPC");
        // Each group takes the published per-direction maximum, so this many groups carry
        // more rules than one page holds however large that page is.
        let groups = MAX_PAGE_SIZE / MAX_SECURITY_GROUP_RULES_PER_DIRECTION + 2;
        for group in 0..groups {
            let group_id = guard.insert_security_group(SecurityGroupSpec {
                vpc_id: vpc_id.clone(),
                group_name: format!("paged-{group}"),
                description: format!("group {group} of a store larger than one page"),
                region: REGION.to_string(),
                tags: Vec::new(),
                ip_permissions: Vec::new(),
                ip_permissions_egress: Vec::new(),
            });
            let blocks: Vec<String> = (0..MAX_SECURITY_GROUP_RULES_PER_DIRECTION)
                .map(|index| format!("10.{group}.{index}.0/24"))
                .collect();
            let grants: Vec<(&str, Option<&str>)> = blocks
                .iter()
                .map(|cidr| (cidr.as_str(), None))
                .collect();
            let permission = ipv4_permission("tcp", Some((443, 443)), &grants);
            guard.insert_security_group_rules(&group_id, false, &permission, false, REGION);
        }
        guard.security_group_rules.len()
    };
    assert!(
        held > MAX_PAGE_SIZE,
        "case {case:?}: the store must hold more than one page, it holds {held}"
    );

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let first = reported(case, &body);
    assert_eq!(
        first.len(),
        MAX_PAGE_SIZE,
        "case {case:?}: expected the documented maximum of {MAX_PAGE_SIZE} rules in a page \
         the request named no size for, got {}",
        first.len()
    );
    let mut walked: Vec<String> = first.into_iter().map(|rule| rule.rule_id).collect();
    let mut token = element(&body, "nextToken");
    assert!(
        token.is_some(),
        "case {case:?}: a page that does not reach the end must carry a token, got {body}"
    );

    let mut pages = 1;
    while let Some(next) = token.clone() {
        let (status, body) = describe(&router, &[param("NextToken", &next)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for page {pages}, got {status} with body {body}"
        );
        let page = reported(case, &body);
        assert!(
            page.len() <= MAX_PAGE_SIZE,
            "case {case:?}: expected at most {MAX_PAGE_SIZE} rules in page {pages}, got {}",
            page.len()
        );
        walked.extend(page.into_iter().map(|rule| rule.rule_id));
        token = element(&body, "nextToken");
        pages += 1;
        assert!(
            pages <= held / MAX_PAGE_SIZE + 2,
            "case {case:?}: pagination did not terminate, walked {} rules",
            walked.len()
        );
    }
    let distinct: BTreeSet<String> = walked.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        held,
        "case {case:?}: expected the pages to report each of the {held} rules exactly once, \
         got {} rows and {} distinct ids",
        walked.len(),
        distinct.len()
    );
}

/// A `NextToken` this service did not issue is refused rather than read as a cursor.
#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for token in ["00", &seeded.ids[0], "not-a-token"] {
        let (status, body) = describe(&router, &[param("NextToken", token)]).await;
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
    seed(&engine);
    let before = described(&router, "before a dry run", &[]).await;

    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry run: expected DryRunOperation, got {body}"
    );

    let after = described(&router, "after a dry run", &[]).await;
    assert_eq!(
        after, before,
        "after a dry run: expected the store unchanged, got {after:#?}"
    );
}

// ---------------------------------------------------------------------------
// A rule belongs to its group
// ---------------------------------------------------------------------------

/// Deleting a security group takes its rules with it, and leaves every other group's rules
/// where they were: a rule can only be named through the group it belongs to.
#[tokio::test]
async fn deleting_a_group_takes_its_rules_with_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Each row deletes one fixture group; the rows differ in how many rules the group holds
    // (several, one, none).
    for index in [5usize, 1, 6] {
        let group_id = seeded.ids[index].clone();
        let case = format!("deleting {group_id}");
        let doomed: Vec<String> = rules_of(&router, &case, &group_id)
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        let others: Vec<String> = described(&router, &case, &[])
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .filter(|rule_id| !doomed.contains(rule_id))
            .collect();

        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DeleteSecurityGroup",
            &[param("GroupId", &group_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteSecurityGroup, got {status} \
             with body {body}"
        );

        let remaining: Vec<String> = described(&router, &case, &[])
            .await
            .into_iter()
            .map(|rule| rule.rule_id)
            .collect();
        assert_eq!(
            remaining, others,
            "case {case:?}: expected only the other groups' rules to remain, got {remaining:?}"
        );
        for rule_id in &doomed {
            let (status, body) =
                describe(&router, &indexed("SecurityGroupRuleId", &[rule_id])).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the removed rule {rule_id:?} to be unknown, got \
                 {status} with body {body}"
            );
        }
    }
}

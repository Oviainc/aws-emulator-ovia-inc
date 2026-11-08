// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifySecurityGroupRules`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=ModifySecurityGroupRules`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that a security group rule is a RESOURCE whose contents this
//! operation REPLACES in place: each rule the request names keeps its `SecurityGroupRuleId`,
//! its ARN, its group, its owner, its direction and its tags, while the protocol, the port
//! range, the one source or destination it names and its description become the ones the
//! request describes — and the rule's TYPE (which of `CidrIpv4`, `CidrIpv6`, `PrefixListId`
//! and `ReferencedGroupId` it names) may not change. Every table row is a distinct input
//! along what that generalises over: the rule type, the direction, which members change,
//! the protocol, whether a port range is carried at all, the CIDR's canonical form, the
//! description, and how many rules one call modifies — so the one case a recording happens
//! to contain is one row among many rather than the specification.
//!
//! The replacement is read back through `DescribeSecurityGroupRules`, and through
//! `DescribeSecurityGroups` for the aggregate view of the same rules, so the round trip is
//! proved through readers the modify does not write.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The id of the prefix list `REGION` publishes at position `index` of its own catalog, in
/// `PrefixListName` order.
///
/// A rule may only grant a prefix list the account can SEE, so the rows below name one of
/// those rather than an id of the right shape that resolves to nothing.
fn published_prefix_list(index: usize) -> String {
    let published = ec2::state::service_prefix_lists_in_region(REGION);
    published
        .get(index)
        .map(|list| list.prefix_list_id.clone())
        .unwrap_or_else(|| panic!("the Region publishes no prefix list at index {index}"))
}

// ---------------------------------------------------------------------------
// The four rule types, as the request names them and as a rule reports them
// ---------------------------------------------------------------------------

/// Which source a security group rule names. "You must specify exactly one of the following
/// parameters, based on the rule type: CidrIpv4, CidrIpv6, PrefixListId, ReferencedGroupId"
/// (`SecurityGroupRuleRequest`), which is also the type a modification may not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ipv4,
    Ipv6,
    PrefixList,
    ReferencedGroup,
}

impl Kind {
    /// The `SecurityGroupRuleRequest` member this kind of source is sent in.
    fn request_member(self) -> &'static str {
        match self {
            Self::Ipv4 => "CidrIpv4",
            Self::Ipv6 => "CidrIpv6",
            Self::PrefixList => "PrefixListId",
            Self::ReferencedGroup => "ReferencedGroupId",
        }
    }

    /// The `IpPermission` list and member an AUTHORIZE names this kind of source in, so the
    /// rule a row modifies is created with the type that row is about.
    fn permission_member(self) -> (&'static str, &'static str) {
        match self {
            Self::Ipv4 => ("IpRanges", "CidrIp"),
            Self::Ipv6 => ("Ipv6Ranges", "CidrIpv6"),
            Self::PrefixList => ("PrefixListIds", "PrefixListId"),
            Self::ReferencedGroup => ("Groups", "GroupId"),
        }
    }

    /// The source as the assertions below spell one: `<kind>=<value>`.
    fn expected(self, value: &str) -> String {
        let tag = match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::PrefixList => "prefix-list",
            Self::ReferencedGroup => "group",
        };
        format!("{tag}={value}")
    }
}

/// One security group rule as `DescribeSecurityGroupRules` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    id: String,
    group_id: String,
    owner_id: String,
    is_egress: String,
    protocol: String,
    from_port: Option<String>,
    to_port: Option<String>,
    /// The one source the rule names, as [`Kind::expected`] spells one.
    source: String,
    description: Option<String>,
    arn: String,
    tags: Vec<(String, String)>,
}

/// The rules a `DescribeSecurityGroupRules` body reports, in the order it reported them.
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
        .map(|item| {
            let source = if let Some(cidr) = child_text(item, "cidrIpv4") {
                format!("ipv4={cidr}")
            } else if let Some(cidr) = child_text(item, "cidrIpv6") {
                format!("ipv6={cidr}")
            } else if let Some(prefix_list_id) = child_text(item, "prefixListId") {
                format!("prefix-list={prefix_list_id}")
            } else if let Some(group_id) =
                child(item, "referencedGroupInfo").and_then(|info| child_text(info, "groupId"))
            {
                format!("group={group_id}")
            } else {
                "none".to_string()
            };
            let tags = child(item, "tagSet")
                .map(|set| {
                    set.children()
                        .filter(|node| node.is_element() && node.tag_name().name() == "item")
                        .map(|tag| {
                            (
                                child_text(tag, "key").unwrap_or_default(),
                                child_text(tag, "value").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Reported {
                id: child_text(item, "securityGroupRuleId").unwrap_or_default(),
                group_id: child_text(item, "groupId").unwrap_or_default(),
                owner_id: child_text(item, "groupOwnerId").unwrap_or_default(),
                is_egress: child_text(item, "isEgress").unwrap_or_default(),
                protocol: child_text(item, "ipProtocol").unwrap_or_default(),
                from_port: child_text(item, "fromPort"),
                to_port: child_text(item, "toPort"),
                source,
                description: child_text(item, "description"),
                arn: child_text(item, "securityGroupRuleArn").unwrap_or_default(),
                tags,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// `CreateSecurityGroup` for `group_name` in the account's default VPC, answering its id.
async fn create_group(case: &str, router: &MultiServiceRouter, group_name: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "a group whose rules are modified"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} \
         with body {body}"
    );
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// One rule to authorize before modifying it: the side, the protocol, the port range, the
/// kind of source, the source itself, a description and the tags the rule carries.
#[derive(Debug, Clone)]
struct Authorize<'a> {
    is_egress: bool,
    protocol: &'a str,
    ports: Option<(i32, i32)>,
    kind: Kind,
    source: &'a str,
    description: Option<&'a str>,
    tags: &'a [(&'a str, &'a str)],
}

impl<'a> Authorize<'a> {
    /// The plainest rule a row can start from: one inbound TCP rule granting one IPv4 range.
    fn ipv4(protocol: &'a str, ports: Option<(i32, i32)>, source: &'a str) -> Self {
        Self {
            is_egress: false,
            protocol,
            ports,
            kind: Kind::Ipv4,
            source,
            description: None,
            tags: &[],
        }
    }
}

/// Authorize the ONE rule `rule` describes on `group_id` and answer the
/// `SecurityGroupRuleId` the service minted for it.
///
/// The rule a row modifies is created through the operation that creates one, so what a row
/// starts from is a rule the service itself stored rather than a record a test wrote.
async fn authorize_one(
    case: &str,
    router: &MultiServiceRouter,
    group_id: &str,
    rule: &Authorize<'_>,
) -> String {
    let (list, member) = rule.kind.permission_member();
    let mut params = vec![
        param("GroupId", group_id),
        param("IpPermissions.1.IpProtocol", rule.protocol),
        param(&format!("IpPermissions.1.{list}.1.{member}"), rule.source),
    ];
    if let Some((from_port, to_port)) = rule.ports {
        params.push(param("IpPermissions.1.FromPort", &from_port.to_string()));
        params.push(param("IpPermissions.1.ToPort", &to_port.to_string()));
    }
    if let Some(description) = rule.description {
        params.push(param(
            &format!("IpPermissions.1.{list}.1.Description"),
            description,
        ));
    }
    if !rule.tags.is_empty() {
        params.push(param(
            "TagSpecification.1.ResourceType",
            "security-group-rule",
        ));
        for (index, (key, value)) in rule.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{position}.Value"),
                value,
            ));
        }
    }
    let action = if rule.is_egress {
        "AuthorizeSecurityGroupEgress"
    } else {
        "AuthorizeSecurityGroupIngress"
    };
    let (status, body) = super::ec2_query::call(router, REGION, action, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    element(&body, "securityGroupRuleId")
        .unwrap_or_else(|| panic!("case {case:?}: no <securityGroupRuleId> in {body}"))
}

/// One `ModifySecurityGroupRules` call carrying `params`.
///
/// `SecurityGroupRules` is sent as `SecurityGroupRule.<n>` — the name the wire gives that
/// list (`ModifySecurityGroupRulesRequest$SecurityGroupRules` carries `xmlName`
/// `SecurityGroupRule`), the way a client's own encoder writes it.
async fn modify(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "ModifySecurityGroupRules", params).await
}

/// One entry of a `SecurityGroupRules` list, at `position` (counting from 1): the rule id it
/// names and the members of the rule it describes.
fn update_params(
    position: usize,
    rule_id: &str,
    protocol: Option<&str>,
    ports: Option<(i32, i32)>,
    source: Option<(Kind, &str)>,
    description: Option<&str>,
) -> Vec<Param> {
    let base = format!("SecurityGroupRule.{position}");
    let mut params = vec![param(&format!("{base}.SecurityGroupRuleId"), rule_id)];
    if let Some(protocol) = protocol {
        params.push(param(&format!("{base}.SecurityGroupRule.IpProtocol"), protocol));
    }
    if let Some((from_port, to_port)) = ports {
        params.push(param(
            &format!("{base}.SecurityGroupRule.FromPort"),
            &from_port.to_string(),
        ));
        params.push(param(
            &format!("{base}.SecurityGroupRule.ToPort"),
            &to_port.to_string(),
        ));
    }
    if let Some((kind, value)) = source {
        params.push(param(
            &format!("{base}.SecurityGroupRule.{}", kind.request_member()),
            value,
        ));
    }
    if let Some(description) = description {
        params.push(param(
            &format!("{base}.SecurityGroupRule.Description"),
            description,
        ));
    }
    params
}

/// The rules of `group_id`, as `DescribeSecurityGroupRules` reports them through the
/// `group-id` filter.
async fn rules_of(case: &str, router: &MultiServiceRouter, group_id: &str) -> Vec<Reported> {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroupRules",
        &filters(&[("group-id", &[group_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroupRules, got {status} \
         with body {body}"
    );
    reported(case, &body)
}

/// The one rule `rule_id` names, as `DescribeSecurityGroupRules` reports it.
async fn rule(case: &str, router: &MultiServiceRouter, group_id: &str, rule_id: &str) -> Reported {
    rules_of(case, router, group_id)
        .await
        .into_iter()
        .find(|reported| reported.id == rule_id)
        .unwrap_or_else(|| panic!("case {case:?}: no rule {rule_id} of group {group_id}"))
}

/// The `<ipPermissions>` or `<ipPermissionsEgress>` section of `group_id` as
/// `DescribeSecurityGroups` reports it — the AGGREGATE view of the same rules, which no
/// modify writes directly.
async fn aggregated(
    case: &str,
    router: &MultiServiceRouter,
    group_id: &str,
    is_egress: bool,
) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupId.1", group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} \
         with body {body}"
    );
    let (open, close) = if is_egress {
        ("<ipPermissionsEgress>", "</ipPermissionsEgress>")
    } else {
        ("<ipPermissions>", "</ipPermissions>")
    };
    let start = body
        .find(open)
        .unwrap_or_else(|| panic!("case {case:?}: no {open} in {body}"));
    let end = body[start..]
        .find(close)
        .unwrap_or_else(|| panic!("case {case:?}: no {close} in {body}"))
        + start;
    body[start..end].to_string()
}

/// The `<return>` member a successful modify answers with.
fn returned(case: &str, body: &str) -> String {
    element(body, "return").unwrap_or_else(|| panic!("case {case:?}: no <return> in {body}"))
}

// ---------------------------------------------------------------------------
// One rule, replaced in place
// ---------------------------------------------------------------------------

/// The members of one rule a row names: the protocol, the port range, the source and the
/// description.
type Members = (&'static str, Option<(i32, i32)>, &'static str, Option<&'static str>);

/// What one row of the replacement table starts from and asks for.
struct Replace {
    name: &'static str,
    is_egress: bool,
    kind: Kind,
    /// The rule the row authorizes first.
    was: Members,
    /// The modification it then sends.
    asks: Members,
    /// The source the modified rule is expected to REPORT, which differs from the one the
    /// request sent where AWS canonicalises it.
    stored_source: &'static str,
    /// The protocol the modified rule is expected to report, which differs from the one the
    /// request sent where the model spells it in one case.
    stored_protocol: &'static str,
}

/// Every modifiable member of one rule becomes what the request describes, while the rule
/// keeps its identity — whatever its type, its direction, which members change and how the
/// request spelled them.
#[tokio::test]
async fn one_rule_keeps_its_identity_and_takes_the_requested_members() {
    let rows = [
        Replace {
            name: "an inbound IPv4 rule takes a new range, protocol and port range",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((22, 22)), "10.0.0.0/8", Some("ssh")),
            asks: ("udp", Some((53, 55)), "192.168.0.0/16", Some("dns")),
            stored_source: "192.168.0.0/16",
            stored_protocol: "udp",
        },
        Replace {
            name: "an outbound IPv4 rule takes a new range only",
            is_egress: true,
            kind: Kind::Ipv4,
            was: ("tcp", Some((443, 443)), "203.0.113.0/24", None),
            asks: ("tcp", Some((443, 443)), "198.51.100.0/24", None),
            stored_source: "198.51.100.0/24",
            stored_protocol: "tcp",
        },
        Replace {
            name: "a ported rule becomes an all-protocols rule with no port range",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((8080, 8090)), "10.1.0.0/16", None),
            asks: ("-1", None, "10.1.0.0/16", Some("everything")),
            stored_source: "10.1.0.0/16",
            stored_protocol: "-1",
        },
        Replace {
            name: "an all-protocols rule takes a protocol and a port range",
            is_egress: true,
            kind: Kind::Ipv4,
            was: ("-1", None, "10.2.0.0/16", None),
            asks: ("tcp", Some((25, 25)), "10.2.0.0/16", None),
            stored_source: "10.2.0.0/16",
            stored_protocol: "tcp",
        },
        Replace {
            name: "an ICMP rule takes a new type and code",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("icmp", Some((8, 0)), "10.3.0.0/16", None),
            asks: ("icmp", Some((-1, -1)), "10.3.0.0/16", Some("all icmp")),
            stored_source: "10.3.0.0/16",
            stored_protocol: "icmp",
        },
        Replace {
            name: "a protocol number replaces a protocol name",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((80, 80)), "10.4.0.0/16", None),
            asks: ("47", None, "10.4.0.0/16", None),
            stored_source: "10.4.0.0/16",
            stored_protocol: "47",
        },
        Replace {
            name: "a protocol name in another case is stored in the model's",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((80, 80)), "10.5.0.0/16", None),
            asks: ("UDP", Some((80, 80)), "10.5.0.0/16", None),
            stored_source: "10.5.0.0/16",
            stored_protocol: "udp",
        },
        Replace {
            name: "a non-canonical IPv4 range is canonicalised",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((80, 80)), "10.6.0.0/16", None),
            asks: ("tcp", Some((80, 80)), "100.68.0.18/18", None),
            stored_source: "100.68.0.0/18",
            stored_protocol: "tcp",
        },
        Replace {
            name: "an IPv6 rule takes a new IPv6 range and a protocol with no ports",
            is_egress: false,
            kind: Kind::Ipv6,
            was: (
                "tcp",
                Some((443, 443)),
                "2001:db8::/32",
                Some("the v6 estate"),
            ),
            asks: ("icmpv6", None, "2600:1f14::/36", None),
            stored_source: "2600:1f14::/36",
            stored_protocol: "icmpv6",
        },
        Replace {
            name: "a non-canonical IPv6 range is canonicalised",
            is_egress: true,
            kind: Kind::Ipv6,
            was: ("tcp", Some((443, 443)), "2001:db8:1::/48", None),
            asks: ("tcp", Some((8443, 8443)), "2600:1f14:abcd::1/36", None),
            stored_source: "2600:1f14:a000::/36",
            stored_protocol: "tcp",
        },
        Replace {
            name: "a description is dropped when the modification carries none",
            is_egress: false,
            kind: Kind::Ipv4,
            was: ("tcp", Some((3306, 3306)), "10.7.0.0/16", Some("mysql")),
            asks: ("tcp", Some((3306, 3306)), "10.7.0.0/16", None),
            stored_source: "10.7.0.0/16",
            stored_protocol: "tcp",
        },
    ];

    for row in rows {
        let case = row.name;
        let (router, _engine) = emulator();
        let group_id = create_group(case, &router, "modified").await;
        let (protocol, ports, source, description) = row.was;
        let tags: &[(&str, &str)] = &[("Owner", "TeamA")];
        let rule_id = authorize_one(
            case,
            &router,
            &group_id,
            &Authorize {
                is_egress: row.is_egress,
                protocol,
                ports,
                kind: row.kind,
                source,
                description,
                tags,
            },
        )
        .await;
        let before_all = rules_of(case, &router, &group_id).await;
        let before = before_all
            .iter()
            .find(|reported| reported.id == rule_id)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no rule {rule_id} to modify"));

        let (protocol, ports, source, description) = row.asks;
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(update_params(
            1,
            &rule_id,
            Some(protocol),
            ports,
            Some((row.kind, source)),
            description,
        ));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            returned(case, &body),
            "true",
            "case {case:?}: expected <return>true</return>, got {body}"
        );

        let after = rule(case, &router, &group_id, &rule_id).await;
        // The rule is the SAME resource: the members the service owns are untouched.
        assert_eq!(
            (
                after.id.as_str(),
                after.arn.as_str(),
                after.group_id.as_str(),
                after.owner_id.as_str(),
                after.is_egress.as_str(),
            ),
            (
                before.id.as_str(),
                before.arn.as_str(),
                before.group_id.as_str(),
                before.owner_id.as_str(),
                before.is_egress.as_str(),
            ),
            "case {case:?}: expected the rule's id, ARN, group, owner and direction to \
             survive the modification, got {after:?} from {before:?}"
        );
        assert_eq!(
            after.tags,
            vec![("Owner".to_string(), "TeamA".to_string())],
            "case {case:?}: expected the rule's tags to survive the modification, got \
             {actual:?}",
            actual = after.tags
        );
        // And it now says what the request described.
        assert_eq!(
            after.protocol, row.stored_protocol,
            "case {case:?}: expected protocol {expected:?}, got {actual:?}",
            expected = row.stored_protocol,
            actual = after.protocol
        );
        let expected_ports = ports.map_or((None, None), |(from_port, to_port)| {
            (Some(from_port.to_string()), Some(to_port.to_string()))
        });
        assert_eq!(
            (after.from_port.clone(), after.to_port.clone()),
            expected_ports,
            "case {case:?}: expected port range {expected_ports:?}, got {from:?}..{to:?}",
            from = after.from_port,
            to = after.to_port
        );
        assert_eq!(
            after.source,
            row.kind.expected(row.stored_source),
            "case {case:?}: expected source {expected:?}, got {actual:?}",
            expected = row.kind.expected(row.stored_source),
            actual = after.source
        );
        assert_eq!(
            after.description.as_deref(),
            description,
            "case {case:?}: expected description {description:?}, got {actual:?}",
            actual = after.description
        );
        // The group holds exactly the rules it held: a modification stores no further rule.
        let held = rules_of(case, &router, &group_id).await;
        assert_eq!(
            held.len(),
            before_all.len(),
            "case {case:?}: expected the modification to leave the rule count at {expected}, \
             got {held:?}",
            expected = before_all.len()
        );
        // The aggregate view the group reports is recomputed from the stored rules, so it
        // carries the new source — and no longer the old one, where the two differ.
        let section = aggregated(case, &router, &group_id, row.is_egress).await;
        assert!(
            section.contains(row.stored_source),
            "case {case:?}: expected the group's aggregated permissions to name \
             {expected:?}, got {section}",
            expected = row.stored_source
        );
        if row.was.2 != row.stored_source {
            assert!(
                !section.contains(row.was.2),
                "case {case:?}: expected the group's aggregated permissions to drop the old \
                 source {old:?}, got {section}",
                old = row.was.2
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The two source kinds that resolve against other resources
// ---------------------------------------------------------------------------

/// Which resource of a row's fixture a source names, so a row can name one without knowing
/// the id this run minted for it.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The prefix list the Region publishes at this position of its catalog.
    PublishedList(usize),
    /// The group the rule belongs to — a rule may reference its own group.
    Itself,
    /// A second group of the same VPC.
    Peer,
    /// A third group of the same VPC.
    OtherPeer,
}

/// A prefix-list rule and a referenced-group rule are modified the same way, each resolving
/// its new source against the resource it names.
#[tokio::test]
async fn a_rule_may_be_pointed_at_another_prefix_list_or_group() {
    struct Row {
        name: &'static str,
        kind: Kind,
        was: Target,
        asks: Target,
    }

    let rows = [
        Row {
            name: "a prefix-list rule names another published list",
            kind: Kind::PrefixList,
            was: Target::PublishedList(0),
            asks: Target::PublishedList(1),
        },
        Row {
            name: "a referenced-group rule names another group",
            kind: Kind::ReferencedGroup,
            was: Target::Peer,
            asks: Target::OtherPeer,
        },
        Row {
            name: "a referenced-group rule may name its own group",
            kind: Kind::ReferencedGroup,
            was: Target::Peer,
            asks: Target::Itself,
        },
        Row {
            name: "a referenced-group rule may be pointed back at the group it had",
            kind: Kind::ReferencedGroup,
            was: Target::OtherPeer,
            asks: Target::Peer,
        },
    ];

    for row in rows {
        let case = row.name;
        let (router, _engine) = emulator();
        let group_id = create_group(case, &router, "modified").await;
        let peer = create_group(case, &router, "peer").await;
        let other_peer = create_group(case, &router, "other-peer").await;
        let resolve = |target: Target| match target {
            Target::PublishedList(index) => published_prefix_list(index),
            Target::Itself => group_id.clone(),
            Target::Peer => peer.clone(),
            Target::OtherPeer => other_peer.clone(),
        };
        let was = resolve(row.was);
        let asks = resolve(row.asks);
        let rule_id = authorize_one(
            case,
            &router,
            &group_id,
            &Authorize {
                is_egress: false,
                protocol: "tcp",
                ports: Some((5432, 5432)),
                kind: row.kind,
                source: &was,
                description: None,
                tags: &[],
            },
        )
        .await;
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(update_params(
            1,
            &rule_id,
            Some("tcp"),
            Some((5432, 5432)),
            Some((row.kind, &asks)),
            None,
        ));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = rule(case, &router, &group_id, &rule_id).await;
        assert_eq!(
            after.source,
            row.kind.expected(&asks),
            "case {case:?}: expected source {expected:?}, got {actual:?}",
            expected = row.kind.expected(&asks),
            actual = after.source
        );
    }
}

// ---------------------------------------------------------------------------
// Several rules in one call
// ---------------------------------------------------------------------------

/// One call modifies every rule it names, on both sides of the group at once, and nothing
/// else.
#[tokio::test]
async fn one_call_modifies_every_rule_it_names_and_nothing_else() {
    let case = "three rules, two sides, one call";
    let (router, _engine) = emulator();
    let group_id = create_group(case, &router, "modified").await;
    let first = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize::ipv4("tcp", Some((22, 22)), "10.0.0.0/8"),
    )
    .await;
    let second = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize {
            kind: Kind::Ipv6,
            source: "2001:db8::/32",
            ..Authorize::ipv4("tcp", Some((443, 443)), "")
        },
    )
    .await;
    let third = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize {
            is_egress: true,
            ..Authorize::ipv4("udp", Some((123, 123)), "10.9.0.0/16")
        },
    )
    .await;
    let untouched = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize {
            description: Some("left alone"),
            ..Authorize::ipv4("tcp", Some((3306, 3306)), "10.8.0.0/16")
        },
    )
    .await;

    let mut params = vec![param("GroupId", &group_id)];
    params.extend(update_params(
        1,
        &first,
        Some("tcp"),
        Some((2222, 2222)),
        Some((Kind::Ipv4, "10.0.0.0/8")),
        None,
    ));
    params.extend(update_params(
        2,
        &second,
        Some("-1"),
        None,
        Some((Kind::Ipv6, "::/0")),
        None,
    ));
    params.extend(update_params(
        3,
        &third,
        Some("udp"),
        Some((514, 514)),
        Some((Kind::Ipv4, "10.9.0.0/16")),
        Some("syslog"),
    ));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    let expected = [
        (&first, "tcp", Some("2222"), "ipv4=10.0.0.0/8", None),
        (&second, "-1", None, "ipv6=::/0", None),
        (
            &third,
            "udp",
            Some("514"),
            "ipv4=10.9.0.0/16",
            Some("syslog"),
        ),
        (
            &untouched,
            "tcp",
            Some("3306"),
            "ipv4=10.8.0.0/16",
            Some("left alone"),
        ),
    ];
    for (rule_id, protocol, from_port, source, description) in expected {
        let after = rule(case, &router, &group_id, rule_id).await;
        assert_eq!(
            (
                after.protocol.as_str(),
                after.from_port.as_deref(),
                after.source.as_str(),
                after.description.as_deref(),
            ),
            (protocol, from_port, source, description),
            "case {case:?}: rule {rule_id} reported {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a modification may not do
// ---------------------------------------------------------------------------

/// What the refusal table is written against: one group with one inbound IPv4 rule and one
/// inbound IPv6 rule, and a second group with a rule of its own.
struct Fixture {
    group_id: String,
    ipv4_rule: String,
    ipv6_rule: String,
    other_group_rule: String,
}

/// One row of the refusal table: the case, the group id it sends (the fixture's unless it
/// names one), the entries it sends, and the EC2 error code it must earn.
struct Refusal {
    name: &'static str,
    group_id: Option<&'static str>,
    /// Built per row, because most rows name the rule the fixture authorized in THIS run.
    entries: fn(&Fixture) -> Vec<Param>,
    code: &'static str,
}

/// A refused modification earns the EC2 code for what is wrong with it, and writes nothing.
#[tokio::test]
async fn a_modification_the_service_refuses_writes_nothing() {
    /// The plainest well-formed modification of the fixture's IPv4 rule, for the rows that
    /// are about something OTHER than the rule they describe.
    fn sound(fixture: &Fixture) -> Vec<Param> {
        update_params(
            1,
            &fixture.ipv4_rule,
            Some("-1"),
            None,
            Some((Kind::Ipv4, "10.0.0.0/8")),
            None,
        )
    }

    let rows = [
        Refusal {
            name: "a group id that is no security group id",
            group_id: Some("not-a-group"),
            entries: sound,
            code: "InvalidGroupId.Malformed",
        },
        Refusal {
            name: "a well-formed group id no group answers to",
            group_id: Some("sg-0123456789abcdef0"),
            entries: sound,
            code: "InvalidGroup.NotFound",
        },
        Refusal {
            name: "a rule id that is no security group rule id",
            group_id: None,
            entries: |_| {
                update_params(
                    1,
                    "not-a-rule",
                    Some("-1"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidSecurityGroupRuleId.Malformed",
        },
        Refusal {
            name: "a well-formed rule id no rule answers to",
            group_id: None,
            entries: |_| {
                update_params(
                    1,
                    "sgr-0123456789abcdef0",
                    Some("-1"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidSecurityGroupRuleId.NotFound",
        },
        Refusal {
            name: "a rule of another group",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.other_group_rule,
                    Some("-1"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidSecurityGroupRuleId.NotFound",
        },
        Refusal {
            name: "one rule named twice",
            group_id: None,
            entries: |fixture| {
                let mut params = sound(fixture);
                params.extend(update_params(
                    2,
                    &fixture.ipv4_rule,
                    Some("udp"),
                    Some((53, 53)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                ));
                params
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "an entry carrying no rule at all",
            group_id: None,
            entries: |fixture| update_params(1, &fixture.ipv4_rule, None, None, None, None),
            code: "MissingParameter",
        },
        Refusal {
            name: "a rule naming no source",
            group_id: None,
            entries: |fixture| {
                update_params(1, &fixture.ipv4_rule, Some("-1"), None, None, None)
            },
            code: "MissingParameter",
        },
        Refusal {
            name: "a rule naming two sources",
            group_id: None,
            entries: |fixture| {
                let mut params = sound(fixture);
                params.push(param(
                    "SecurityGroupRule.1.SecurityGroupRule.CidrIpv6",
                    "2001:db8::/32",
                ));
                params
            },
            code: "InvalidParameterCombination",
        },
        Refusal {
            name: "an IPv4 rule given an IPv6 range changes the rule type",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("-1"),
                    None,
                    Some((Kind::Ipv6, "2001:db8::/32")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "an IPv6 rule given a prefix list changes the rule type",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv6_rule,
                    Some("-1"),
                    None,
                    Some((Kind::PrefixList, "pl-0123456789abcdef0")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "an IPv4 rule given a referenced group changes the rule type",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("-1"),
                    None,
                    Some((Kind::ReferencedGroup, "sg-0123456789abcdef0")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "a rule naming no protocol",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    None,
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "MissingParameter",
        },
        Refusal {
            name: "a protocol no rule may carry",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("sctp"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "a protocol number past the one byte a protocol number holds",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("256"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "tcp with no port range",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("tcp"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "a port range that ends before it starts",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("tcp"),
                    Some((90, 80)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidPermission.Malformed",
        },
        Refusal {
            name: "a port past the highest port number",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("tcp"),
                    Some((80, 65_536)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "a range that is no CIDR block",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("-1"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/33")),
                    None,
                )
            },
            code: "InvalidParameterValue",
        },
        Refusal {
            name: "a description outside the alphabet a rule's description admits",
            group_id: None,
            entries: |fixture| {
                update_params(
                    1,
                    &fixture.ipv4_rule,
                    Some("-1"),
                    None,
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    // `~` is outside the punctuation a rule's description admits
                    // (`IpRange$Description`).
                    Some("tilde~here"),
                )
            },
            code: "InvalidParameterValue",
        },
    ];

    for row in rows {
        let case = row.name;
        let (router, _engine) = emulator();
        let fixture = refusal_fixture(case, &router).await;
        let before = rules_of(case, &router, &fixture.group_id).await;
        let group_id = row
            .group_id
            .map(str::to_string)
            .unwrap_or_else(|| fixture.group_id.clone());
        let mut params = vec![param("GroupId", &group_id)];
        params.extend((row.entries)(&fixture));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected error code {expected:?}, got {body}",
            expected = row.code
        );
        let after = rules_of(case, &router, &fixture.group_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to write nothing, got {after:?}"
        );
    }
}

/// The store the refusal table is written against.
async fn refusal_fixture(case: &str, router: &MultiServiceRouter) -> Fixture {
    let group_id = create_group(case, router, "modified").await;
    let other = create_group(case, router, "other").await;
    let ipv4_rule = authorize_one(
        case,
        router,
        &group_id,
        &Authorize::ipv4("tcp", Some((22, 22)), "10.0.0.0/8"),
    )
    .await;
    let ipv6_rule = authorize_one(
        case,
        router,
        &group_id,
        &Authorize {
            kind: Kind::Ipv6,
            source: "2001:db8::/32",
            ..Authorize::ipv4("tcp", Some((443, 443)), "")
        },
    )
    .await;
    let other_group_rule = authorize_one(
        case,
        router,
        &other,
        &Authorize::ipv4("tcp", Some((80, 80)), "10.7.0.0/16"),
    )
    .await;
    Fixture {
        group_id,
        ipv4_rule,
        ipv6_rule,
        other_group_rule,
    }
}

/// A prefix-list rule pointed at an id the account can see no prefix list for is refused with
/// the prefix list's own not-found class, and nothing is written.
#[tokio::test]
async fn a_prefix_list_the_account_cannot_see_is_refused() {
    let case = "a prefix list the account cannot see";
    let (router, _engine) = emulator();
    let group_id = create_group(case, &router, "modified").await;
    let published = published_prefix_list(0);
    let rule_id = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize {
            kind: Kind::PrefixList,
            source: &published,
            ..Authorize::ipv4("tcp", Some((80, 80)), "")
        },
    )
    .await;
    let before = rules_of(case, &router, &group_id).await;
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(update_params(
        1,
        &rule_id,
        Some("tcp"),
        Some((80, 80)),
        Some((Kind::PrefixList, "pl-0f0f0f0f0f0f0f0f0")),
        None,
    ));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPrefixListID.NotFound",
        "case {case:?}: expected InvalidPrefixListID.NotFound, got {body}"
    );
    let after = rules_of(case, &router, &group_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the refusal to write nothing, got {after:?}"
    );
}

/// A referenced-group rule pointed at a group the account holds none of is refused, and
/// nothing is written.
#[tokio::test]
async fn a_referenced_group_the_account_does_not_hold_is_refused() {
    let case = "a referenced group the account does not hold";
    let (router, _engine) = emulator();
    let group_id = create_group(case, &router, "modified").await;
    let peer = create_group(case, &router, "peer").await;
    let rule_id = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize {
            kind: Kind::ReferencedGroup,
            source: &peer,
            ..Authorize::ipv4("tcp", Some((5432, 5432)), "")
        },
    )
    .await;
    let before = rules_of(case, &router, &group_id).await;
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(update_params(
        1,
        &rule_id,
        Some("tcp"),
        Some((5432, 5432)),
        Some((Kind::ReferencedGroup, "sg-0123456789abcdef0")),
        None,
    ));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidGroup.NotFound",
        "case {case:?}: expected InvalidGroup.NotFound, got {body}"
    );
    let after = rules_of(case, &router, &group_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the refusal to write nothing, got {after:?}"
    );
}

/// A modification that would leave the group holding one rule twice is refused with
/// `InvalidPermission.Duplicate`, whether the rule it would collide with is one the group
/// keeps or one the same call would write.
#[tokio::test]
async fn a_modification_that_would_duplicate_a_rule_is_refused() {
    /// The two rules each row starts from: both inbound IPv4 rules granting one range, told
    /// apart only by their port range, so either can be moved onto the other's.
    struct Row {
        name: &'static str,
        /// The entries the call sends, given the first rule's id and the second's.
        entries: fn(&str, &str) -> Vec<Param>,
    }

    let rows = [
        Row {
            name: "onto a rule the group keeps",
            entries: |first, _second| {
                update_params(
                    1,
                    first,
                    Some("tcp"),
                    Some((443, 443)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                )
            },
        },
        Row {
            name: "onto what the same call writes elsewhere",
            entries: |first, second| {
                let mut params = update_params(
                    1,
                    first,
                    Some("tcp"),
                    Some((8080, 8080)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                );
                params.extend(update_params(
                    2,
                    second,
                    Some("tcp"),
                    Some((8080, 8080)),
                    Some((Kind::Ipv4, "10.0.0.0/8")),
                    None,
                ));
                params
            },
        },
    ];

    for row in rows {
        let case = row.name;
        let (router, _engine) = emulator();
        let group_id = create_group(case, &router, "modified").await;
        let first = authorize_one(
            case,
            &router,
            &group_id,
            &Authorize::ipv4("tcp", Some((22, 22)), "10.0.0.0/8"),
        )
        .await;
        let second = authorize_one(
            case,
            &router,
            &group_id,
            &Authorize::ipv4("tcp", Some((443, 443)), "10.0.0.0/8"),
        )
        .await;
        let before = rules_of(case, &router, &group_id).await;
        let mut params = vec![param("GroupId", &group_id)];
        params.extend((row.entries)(&first, &second));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPermission.Duplicate",
            "case {case:?}: expected InvalidPermission.Duplicate, got {body}"
        );
        let after = rules_of(case, &router, &group_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to write nothing, got {after:?}"
        );
    }
}

/// A rule may be modified again: the second modification sees what the first wrote, and the
/// rule the request named is the one that moved.
#[tokio::test]
async fn a_rule_may_be_modified_again() {
    let case = "two modifications in a row";
    let (router, _engine) = emulator();
    let group_id = create_group(case, &router, "modified").await;
    let rule_id = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize::ipv4("tcp", Some((22, 22)), "10.0.0.0/8"),
    )
    .await;
    for (protocol, port, cidr) in [("udp", 53, "10.1.0.0/16"), ("tcp", 22, "10.0.0.0/8")] {
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(update_params(
            1,
            &rule_id,
            Some(protocol),
            Some((port, port)),
            Some((Kind::Ipv4, cidr)),
            None,
        ));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for {protocol}, got {status} with body {body}"
        );
        let after = rule(case, &router, &group_id, &rule_id).await;
        assert_eq!(
            (
                after.protocol.clone(),
                after.from_port.clone(),
                after.source.clone(),
            ),
            (
                protocol.to_string(),
                Some(port.to_string()),
                format!("ipv4={cidr}"),
            ),
            "case {case:?}: after modifying to {protocol}, the rule reported {after:?}"
        );
    }
}

/// `DryRun` reports the outcome of a request that would have succeeded and writes nothing:
/// "if you have the required permissions, the error response is DryRunOperation"
/// (`$DryRun`).
#[tokio::test]
async fn a_dry_run_writes_nothing() {
    let case = "a dry run of a modification that would succeed";
    let (router, _engine) = emulator();
    let group_id = create_group(case, &router, "modified").await;
    let rule_id = authorize_one(
        case,
        &router,
        &group_id,
        &Authorize::ipv4("tcp", Some((22, 22)), "10.0.0.0/8"),
    )
    .await;
    let before = rules_of(case, &router, &group_id).await;
    let mut params = vec![param("GroupId", &group_id), param("DryRun", "true")];
    params.extend(update_params(
        1,
        &rule_id,
        Some("udp"),
        Some((53, 53)),
        Some((Kind::Ipv4, "10.1.0.0/16")),
        None,
    ));
    let (status, body) = modify(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    let after = rules_of(case, &router, &group_id).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the dry run to write nothing, got {after:?}"
    );
}

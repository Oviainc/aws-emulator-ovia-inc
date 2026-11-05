// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AuthorizeSecurityGroupEgress`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=AuthorizeSecurityGroupEgress`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rule under test is that the operation ADDS ONE OUTBOUND RULE PER DESTINATION the
//! request's `IpPermissions` name: each rule carries the permission's protocol and port
//! range and that one destination (an IPv4 range, an IPv6 range, a prefix list or another
//! security group), is minted with a service-owned `SecurityGroupRuleId` and ARN, and is
//! stored — so it comes back both as the rule the call answers with and inside the group's
//! `IpPermissionsEgress` through `DescribeSecurityGroups`. Every table row is a distinct
//! input along what that generalises over — which protocol, whether it carries a port
//! range, which kind of destination, how many destinations per permission, how many
//! permissions, the CIDR's canonical form, the description — so the one case a recording
//! happens to contain is one row among many rather than the specification.
//!
//! What `DescribeSecurityGroups` selects and pages over is its own suite's subject
//! ([`super::describe_security_groups`]); this suite reads it only to prove the round trip.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;

use super::ec2_query::{element, error_code};
use super::wire::{child, child_text, document, is_long_id, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The id of the prefix list `REGION` publishes at position `index` of its own catalog, in
/// `PrefixListName` order.
///
/// A rule may only grant a prefix list the account can SEE — the lists it created and the
/// ones the Region publishes for its services, which is what `DescribeManagedPrefixLists`
/// reports — so the rows below name one of those rather than an id of the right shape that
/// resolves to nothing.
fn published_prefix_list(index: usize) -> String {
    let published = ec2::state::service_prefix_lists_in_region(REGION);
    published
        .get(index)
        .map(|list| list.prefix_list_id.clone())
        .unwrap_or_else(|| panic!("the Region publishes no prefix list at index {index}"))
}

/// The kind of destination a rule names. "You must specify exactly one of the following
/// destinations: an IPv4 or IPv6 address range, a prefix list, or a security group"
/// (`AuthorizeSecurityGroupEgress`), and a group may be named by id or by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ipv4,
    Ipv6,
    PrefixList,
    GroupById,
    GroupByName,
}

impl Kind {
    /// The word the expectations below name this kind of destination by.
    fn tag(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::PrefixList => "prefix-list",
            // A rule references a group by id however the request named it, so the two
            // ways of naming one describe the same destination.
            Self::GroupById | Self::GroupByName => "group",
        }
    }
}

/// One destination of one permission: what the request carries for it, what the stored rule
/// is expected to carry, and the description the request gives that destination.
#[derive(Debug, Clone)]
struct Dest {
    kind: Kind,
    requested: String,
    stored: String,
    description: Option<String>,
}

impl Dest {
    /// An IPv4 range the service stores exactly as the request wrote it.
    fn ipv4(cidr: &str) -> Self {
        Self {
            kind: Kind::Ipv4,
            requested: cidr.to_string(),
            stored: cidr.to_string(),
            description: None,
        }
    }

    /// An IPv4 range whose canonical form differs from the requested one.
    fn ipv4_canonicalised(requested: &str, stored: &str) -> Self {
        Self {
            kind: Kind::Ipv4,
            requested: requested.to_string(),
            stored: stored.to_string(),
            description: None,
        }
    }

    /// An IPv6 range the service stores exactly as the request wrote it.
    fn ipv6(cidr: &str) -> Self {
        Self {
            kind: Kind::Ipv6,
            requested: cidr.to_string(),
            stored: cidr.to_string(),
            description: None,
        }
    }

    /// An IPv6 range whose canonical form differs from the requested one.
    fn ipv6_canonicalised(requested: &str, stored: &str) -> Self {
        Self {
            kind: Kind::Ipv6,
            requested: requested.to_string(),
            stored: stored.to_string(),
            description: None,
        }
    }

    /// A prefix list, by id.
    fn prefix_list(prefix_list_id: &str) -> Self {
        Self {
            kind: Kind::PrefixList,
            requested: prefix_list_id.to_string(),
            stored: prefix_list_id.to_string(),
            description: None,
        }
    }

    /// Another security group, named by id.
    fn group_by_id(group_id: &str) -> Self {
        Self {
            kind: Kind::GroupById,
            requested: group_id.to_string(),
            stored: group_id.to_string(),
            description: None,
        }
    }

    /// Another security group, named by name — which resolves to `group_id`.
    fn group_by_name(group_name: &str, group_id: &str) -> Self {
        Self {
            kind: Kind::GroupByName,
            requested: group_name.to_string(),
            stored: group_id.to_string(),
            description: None,
        }
    }

    /// The same destination, with a description on it.
    fn described(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// The destination as the assertions below spell one: `<kind>=<value>`.
    fn expected(&self) -> String {
        format!("{}={}", self.kind.tag(), self.stored)
    }
}

/// One permission of a request: the protocol, the port range it carries (if any), and the
/// destinations it names.
#[derive(Debug, Clone)]
struct Permission {
    /// The protocol as the REQUEST spells it.
    protocol: &'static str,
    /// The protocol as the stored rule is expected to spell it — the same, unless the
    /// request spelled a name in another case.
    stored_protocol: &'static str,
    ports: Option<(i32, i32)>,
    destinations: Vec<Dest>,
}

impl Permission {
    fn new(protocol: &'static str, ports: Option<(i32, i32)>, destinations: Vec<Dest>) -> Self {
        Self {
            protocol,
            stored_protocol: protocol,
            ports,
            destinations,
        }
    }

    /// A permission whose protocol the service stores in another form than the request
    /// wrote it.
    fn canonicalised(
        protocol: &'static str,
        stored_protocol: &'static str,
        ports: Option<(i32, i32)>,
        destinations: Vec<Dest>,
    ) -> Self {
        Self {
            protocol,
            stored_protocol,
            ports,
            destinations,
        }
    }

    /// The form fields for this permission at `position` (counting from 1), as ec2Query
    /// flattens a list into indexed keys: each destination kind is a list of its own, so
    /// each carries its own index.
    fn params(&self, position: usize) -> Vec<Param> {
        let base = format!("IpPermissions.{position}");
        let mut params = vec![param(&format!("{base}.IpProtocol"), self.protocol)];
        if let Some((from_port, to_port)) = self.ports {
            params.push(param(&format!("{base}.FromPort"), &from_port.to_string()));
            params.push(param(&format!("{base}.ToPort"), &to_port.to_string()));
        }
        let mut ipv4 = 0;
        let mut ipv6 = 0;
        let mut prefix_lists = 0;
        let mut groups = 0;
        for destination in &self.destinations {
            let (list, member, index) = match destination.kind {
                Kind::Ipv4 => {
                    ipv4 += 1;
                    ("IpRanges", "CidrIp", ipv4)
                }
                Kind::Ipv6 => {
                    ipv6 += 1;
                    ("Ipv6Ranges", "CidrIpv6", ipv6)
                }
                Kind::PrefixList => {
                    prefix_lists += 1;
                    ("PrefixListIds", "PrefixListId", prefix_lists)
                }
                Kind::GroupById => {
                    groups += 1;
                    ("Groups", "GroupId", groups)
                }
                Kind::GroupByName => {
                    groups += 1;
                    ("Groups", "GroupName", groups)
                }
            };
            params.push(param(
                &format!("{base}.{list}.{index}.{member}"),
                &destination.requested,
            ));
            if let Some(description) = destination.description.as_deref() {
                params.push(param(
                    &format!("{base}.{list}.{index}.Description"),
                    description,
                ));
            }
        }
        params
    }
}

/// The form fields for a whole `IpPermissions` list.
fn permission_params(permissions: &[Permission]) -> Vec<Param> {
    permissions
        .iter()
        .enumerate()
        .flat_map(|(index, permission)| permission.params(index + 1))
        .collect()
}

/// One security group rule as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedRule {
    id: String,
    group_id: String,
    owner_id: String,
    is_egress: String,
    protocol: String,
    from_port: Option<String>,
    to_port: Option<String>,
    /// The one destination the rule names, as [`Dest::expected`] spells one.
    destination: String,
    description: Option<String>,
    arn: String,
    tags: Vec<(String, String)>,
}

/// One rule a request is expected to have added: the destination it names, and the
/// permission members it inherits.
#[derive(Debug, Clone)]
struct ExpectedRule {
    /// The destination as [`Dest::expected`] spells one.
    destination: String,
    protocol: &'static str,
    from_port: Option<String>,
    to_port: Option<String>,
    description: Option<String>,
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `AuthorizeSecurityGroupEgress` call carrying `params`.
async fn authorize(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "AuthorizeSecurityGroupEgress", params).await
}

/// `CreateSecurityGroup` for `group_name` in the account's default VPC, answering its id.
async fn create_group(case: &str, router: &MultiServiceRouter, group_name: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "a group to authorise egress on"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSecurityGroup, got {status} \
         with body {body}"
    );
    element(&body, "groupId")
        .unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// The rules a response reports, in the order it reported them.
fn reported_rules(case: &str, body: &str) -> Vec<ReportedRule> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "securityGroupRuleSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            let destination = if let Some(cidr) = child_text(item, "cidrIpv4") {
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
            ReportedRule {
                id: child_text(item, "securityGroupRuleId").unwrap_or_default(),
                group_id: child_text(item, "groupId").unwrap_or_default(),
                owner_id: child_text(item, "groupOwnerId").unwrap_or_default(),
                is_egress: child_text(item, "isEgress").unwrap_or_default(),
                protocol: child_text(item, "ipProtocol").unwrap_or_default(),
                from_port: child_text(item, "fromPort"),
                to_port: child_text(item, "toPort"),
                destination,
                description: child_text(item, "description"),
                arn: child_text(item, "securityGroupRuleArn").unwrap_or_default(),
                tags,
            }
        })
        .collect()
}

/// The `<ipPermissionsEgress>` section of a `DescribeSecurityGroups` body, where the
/// group's outbound rules are reported.
fn egress_section(case: &str, body: &str) -> String {
    let open = "<ipPermissionsEgress>";
    let close = "</ipPermissionsEgress>";
    let start = body
        .find(open)
        .unwrap_or_else(|| panic!("case {case:?}: no {open} in {body}"));
    let end = body[start..]
        .find(close)
        .unwrap_or_else(|| panic!("case {case:?}: no {close} in {body}"))
        + start;
    body[start..end].to_string()
}

/// `DescribeSecurityGroups` narrowed to `group_id`, requiring success.
async fn describe(case: &str, router: &MultiServiceRouter, group_id: &str) -> String {
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
    body
}

/// The element a destination is reported by inside a group's aggregated permissions, and
/// the value it carries there.
fn aggregated_element(destination: &Dest) -> (&'static str, &str) {
    match destination.kind {
        Kind::Ipv4 => ("cidrIp", destination.stored.as_str()),
        Kind::Ipv6 => ("cidrIpv6", destination.stored.as_str()),
        Kind::PrefixList => ("prefixListId", destination.stored.as_str()),
        Kind::GroupById | Kind::GroupByName => ("groupId", destination.stored.as_str()),
    }
}

/// The rule ids the store holds for `group_id` on the outbound side.
fn stored_egress_rule_ids(engine: &engine::InMemoryEngine, group_id: &str) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .security_group_rules
        .values()
        .filter(|record| {
            record.rule.group_id.as_deref() == Some(group_id)
                && record.rule.is_egress == Some(true)
        })
        .map(|record| {
            record
                .rule
                .security_group_rule_id
                .clone()
                .unwrap_or_default()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// One rule per destination, stored and reported
// ---------------------------------------------------------------------------

/// Every destination of every permission becomes one stored outbound rule carrying that
/// permission's protocol and port range, whatever the protocol, the destination kind, the
/// number of destinations or the number of permissions.
#[tokio::test]
async fn each_destination_of_each_permission_becomes_one_stored_rule() {
    struct Case {
        name: &'static str,
        /// Built per case, because a rule that references a group names an id the store
        /// minted in THIS run.
        permissions: fn(&str) -> Vec<Permission>,
    }

    let cases = [
        Case {
            name: "tcp, one port, one IPv4 host",
            permissions: |_| {
                vec![Permission::new(
                    "tcp",
                    Some((443, 443)),
                    vec![Dest::ipv4("203.0.113.7/32").described("to the gateway")],
                )]
            },
        },
        Case {
            name: "udp, a port range, two IPv4 ranges",
            permissions: |_| {
                vec![Permission::new(
                    "udp",
                    Some((53, 55)),
                    vec![Dest::ipv4("10.0.0.0/8"), Dest::ipv4("192.168.0.0/16")],
                )]
            },
        },
        Case {
            name: "every protocol, an IPv4 range, no ports",
            permissions: |_| {
                vec![Permission::new(
                    "-1",
                    None,
                    vec![Dest::ipv4("198.18.0.0/15")],
                )]
            },
        },
        Case {
            name: "icmp, a type and a code",
            permissions: |_| {
                vec![Permission::new(
                    "icmp",
                    Some((8, 0)),
                    vec![Dest::ipv4("10.1.0.0/16")],
                )]
            },
        },
        Case {
            name: "icmp, all types and all codes",
            permissions: |_| {
                vec![Permission::new(
                    "icmp",
                    Some((-1, -1)),
                    vec![Dest::ipv4("10.2.0.0/16")],
                )]
            },
        },
        Case {
            name: "icmpv6 without a port range, an IPv6 range",
            permissions: |_| {
                vec![Permission::new(
                    "icmpv6",
                    None,
                    vec![Dest::ipv6("2600:1f14::/36").described("to the v6 estate")],
                )]
            },
        },
        Case {
            name: "a protocol number, an IPv6 range",
            permissions: |_| vec![Permission::new("47", None, vec![Dest::ipv6("::/0")])],
        },
        Case {
            name: "a prefix list",
            permissions: |_| {
                vec![Permission::new(
                    "tcp",
                    Some((80, 80)),
                    vec![Dest::prefix_list(&published_prefix_list(0)).described("S3 endpoints")],
                )]
            },
        },
        Case {
            name: "another group, by id",
            permissions: |target| {
                vec![Permission::new(
                    "tcp",
                    Some((5432, 5432)),
                    vec![Dest::group_by_id(target).described("to the database tier")],
                )]
            },
        },
        Case {
            name: "another group, by name",
            permissions: |target| {
                vec![Permission::new(
                    "tcp",
                    Some((9092, 9092)),
                    vec![Dest::group_by_name("target", target)],
                )]
            },
        },
        Case {
            name: "one permission naming four kinds of destination",
            permissions: |target| {
                vec![Permission::new(
                    "tcp",
                    Some((8080, 8090)),
                    vec![
                        Dest::ipv4("172.16.0.0/12"),
                        Dest::ipv6("2001:db8::/32"),
                        Dest::prefix_list(&published_prefix_list(1)),
                        Dest::group_by_id(target),
                    ],
                )]
            },
        },
        Case {
            name: "three permissions at once",
            permissions: |_| {
                vec![
                    Permission::new("tcp", Some((22, 22)), vec![Dest::ipv4("10.3.0.0/16")]),
                    Permission::new("udp", Some((123, 123)), vec![Dest::ipv4("10.4.0.0/16")]),
                    Permission::new("-1", None, vec![Dest::ipv6("2001:db8:1::/48")]),
                ]
            },
        },
        Case {
            name: "a protocol name in another case is stored in the model's",
            permissions: |_| {
                vec![Permission::canonicalised(
                    "TCP",
                    "tcp",
                    Some((25, 25)),
                    vec![Dest::ipv4("10.9.0.0/16")],
                )]
            },
        },
        Case {
            name: "non-canonical CIDR blocks are canonicalised",
            permissions: |_| {
                vec![Permission::new(
                    "tcp",
                    Some((443, 443)),
                    vec![
                        Dest::ipv4_canonicalised("100.68.0.18/18", "100.68.0.0/18"),
                        Dest::ipv6_canonicalised("2600:1f14:abcd::1/36", "2600:1f14:a000::/36"),
                    ],
                )]
            },
        },
    ];

    for case in cases {
        let (router, engine) = emulator();
        let target = create_group(case.name, &router, "target").await;
        let group_id = create_group(case.name, &router, "authorised").await;
        let permissions = (case.permissions)(&target);

        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&permissions));
        let (status, body) = authorize(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {name:?}: expected <return>true</return>, got {body}",
            name = case.name
        );

        // One rule per destination, each carrying its permission's protocol and ports.
        let expected: Vec<ExpectedRule> = permissions
            .iter()
            .flat_map(|permission| {
                permission.destinations.iter().map(move |destination| {
                    ExpectedRule {
                        destination: destination.expected(),
                        protocol: permission.stored_protocol,
                        from_port: permission.ports.map(|(from, _)| from.to_string()),
                        to_port: permission.ports.map(|(_, to)| to.to_string()),
                        description: destination.description.clone(),
                    }
                })
            })
            .collect();
        let reported = reported_rules(case.name, &body);
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {name:?}: expected {count} rules, got {reported:?} from {body}",
            name = case.name,
            count = expected.len()
        );
        for ExpectedRule {
            destination,
            protocol,
            from_port,
            to_port,
            description,
        } in &expected
        {
            let rule = reported
                .iter()
                .find(|rule| &rule.destination == destination)
                .unwrap_or_else(|| {
                    panic!(
                        "case {name:?}: no rule for {destination}, got {reported:?}",
                        name = case.name
                    )
                });
            assert_eq!(
                &rule.protocol, protocol,
                "case {name:?}: expected protocol {protocol:?} for {destination}, got {rule:?}",
                name = case.name
            );
            assert_eq!(
                &rule.from_port, from_port,
                "case {name:?}: expected fromPort {from_port:?} for {destination}, got {rule:?}",
                name = case.name
            );
            assert_eq!(
                &rule.to_port, to_port,
                "case {name:?}: expected toPort {to_port:?} for {destination}, got {rule:?}",
                name = case.name
            );
            assert_eq!(
                &rule.description, description,
                "case {name:?}: expected description {description:?} for {destination}, \
                 got {rule:?}",
                name = case.name
            );
            assert_eq!(
                rule.is_egress, "true",
                "case {name:?}: an authorised EGRESS rule must report isEgress true, \
                 got {rule:?}",
                name = case.name
            );
            assert_eq!(
                rule.group_id, group_id,
                "case {name:?}: expected the rule on {group_id}, got {rule:?}",
                name = case.name
            );
            assert_eq!(
                rule.owner_id,
                engine::emulator_account_id(),
                "case {name:?}: expected this account to own the rule, got {rule:?}",
                name = case.name
            );
            assert!(
                is_long_id("sgr", &rule.id),
                "case {name:?}: expected a service-minted sgr- id, got {rule:?}",
                name = case.name
            );
            assert_eq!(
                rule.arn,
                engine::emulator_arn(
                    "ec2",
                    REGION,
                    &format!("security-group-rule/{}", rule.id)
                ),
                "case {name:?}: expected the ARN to name the rule, got {rule:?}",
                name = case.name
            );
        }

        // Every id is distinct, and the store holds exactly the rules answered with —
        // beside the allow-all outbound rule the group was created with.
        let mut ids: Vec<String> = reported.iter().map(|rule| rule.id.clone()).collect();
        let minted = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            minted,
            "case {name:?}: expected {minted} distinct rule ids, got {reported:?}",
            name = case.name
        );
        let mut stored = stored_egress_rule_ids(&engine, &group_id);
        stored.sort();
        assert_eq!(
            stored.len(),
            minted + 1,
            "case {name:?}: expected the {minted} new rules beside the allow-all outbound \
             rule the group was created with, got {stored:?}",
            name = case.name
        );
        for id in &ids {
            assert!(
                stored.contains(id),
                "case {name:?}: the store must hold the answered rule {id}, got {stored:?}",
                name = case.name
            );
        }

        // The round trip: every destination is inside the group's aggregated outbound
        // permissions, which is where `DescribeSecurityGroups` reports a group's rules.
        let described = describe(case.name, &router, &group_id).await;
        let egress = egress_section(case.name, &described);
        for permission in &permissions {
            for destination in &permission.destinations {
                let (member, value) = aggregated_element(destination);
                assert!(
                    egress.contains(&format!("<{member}>{value}</{member}>")),
                    "case {name:?}: expected <{member}>{value}</{member}> in the group's \
                     outbound rules, got {egress}",
                    name = case.name
                );
            }
            assert!(
                egress.contains(&format!(
                    "<ipProtocol>{protocol}</ipProtocol>",
                    protocol = permission.stored_protocol
                )),
                "case {name:?}: expected the aggregated rules to carry protocol {protocol:?}, \
                 got {egress}",
                name = case.name,
                protocol = permission.stored_protocol
            );
        }
    }
}

/// Rules authorised in two calls accumulate: the second call answers only its own rules,
/// while the group reports both — the answer is computed from the store, not fixed.
#[tokio::test]
async fn a_second_authorize_adds_to_the_rules_the_group_already_holds() {
    let (router, engine) = emulator();
    let group_id = create_group("accumulate", &router, "accumulate").await;

    let first = vec![Permission::new(
        "tcp",
        Some((443, 443)),
        vec![Dest::ipv4("10.0.0.0/8")],
    )];
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(permission_params(&first));
    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 200,
        "first call: expected HTTP 200, got {status} with body {body}"
    );
    let first_ids: Vec<String> = reported_rules("first call", &body)
        .iter()
        .map(|rule| rule.id.clone())
        .collect();

    let second = vec![Permission::new(
        "tcp",
        Some((443, 443)),
        vec![Dest::ipv4("172.16.0.0/12")],
    )];
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(permission_params(&second));
    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 200,
        "second call: expected HTTP 200, got {status} with body {body}"
    );
    let second_rules = reported_rules("second call", &body);
    assert_eq!(
        second_rules.len(),
        1,
        "second call: expected only the rule it added, got {second_rules:?}"
    );
    assert!(
        !first_ids.contains(&second_rules[0].id),
        "second call: expected a fresh rule id, got {second_rules:?} after {first_ids:?}"
    );

    let stored = stored_egress_rule_ids(&engine, &group_id);
    assert_eq!(
        stored.len(),
        3,
        "expected the allow-all outbound rule and both authorised rules, got {stored:?}"
    );
    // Two rules for one protocol and port range are reported as ONE permission with two
    // ranges, the way AWS groups a group's rules.
    let described = describe("accumulate", &router, &group_id).await;
    let egress = egress_section("accumulate", &described);
    for cidr in ["10.0.0.0/8", "172.16.0.0/12"] {
        assert!(
            egress.contains(&format!("<cidrIp>{cidr}</cidrIp>")),
            "expected {cidr} in the group's outbound rules, got {egress}"
        );
    }
}

/// The tags a request assigns go on every rule it creates, and come back on each of them.
#[tokio::test]
async fn the_requested_tags_are_assigned_to_every_rule_created() {
    let (router, _engine) = emulator();
    let group_id = create_group("tags", &router, "tagged").await;

    let permissions = vec![Permission::new(
        "tcp",
        Some((443, 443)),
        vec![Dest::ipv4("10.0.0.0/8"), Dest::ipv6("2001:db8::/32")],
    )];
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(permission_params(&permissions));
    params.push(param(
        "TagSpecification.1.ResourceType",
        ec2::state::RESOURCE_TYPE_SECURITY_GROUP_RULE,
    ));
    params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
    params.push(param("TagSpecification.1.Tag.1.Value", "egress"));
    params.push(param("TagSpecification.1.Tag.2.Key", "Owner"));
    params.push(param("TagSpecification.1.Tag.2.Value", "TeamA"));

    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 200,
        "tags: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_rules("tags", &body);
    assert_eq!(
        reported.len(),
        2,
        "tags: expected one rule per destination, got {reported:?}"
    );
    for rule in &reported {
        assert_eq!(
            rule.tags,
            vec![
                ("Name".to_string(), "egress".to_string()),
                ("Owner".to_string(), "TeamA".to_string()),
            ],
            "tags: expected both tags on every rule, got {rule:?}"
        );
    }
}

/// A rule the group already holds is refused as a duplicate, and the store is left exactly
/// as it was — including the allow-all outbound rule every group is created with, which is
/// a rule like any other and so cannot be authorised a second time.
#[tokio::test]
async fn a_rule_the_group_already_holds_is_refused_and_changes_nothing() {
    struct Case {
        name: &'static str,
        permissions: Vec<Permission>,
    }

    let cases = [
        Case {
            name: "the rule the group was created with",
            permissions: vec![Permission::new("-1", None, vec![Dest::ipv4("0.0.0.0/0")])],
        },
        Case {
            name: "a rule authorised a moment ago",
            permissions: vec![Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv4("10.0.0.0/8")],
            )],
        },
        Case {
            name: "a rule authorised a moment ago, named non-canonically",
            permissions: vec![Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv4_canonicalised("10.0.0.18/8", "10.0.0.0/8")],
            )],
        },
    ];

    for case in cases {
        let (router, engine) = emulator();
        let group_id = create_group(case.name, &router, "duplicated").await;

        // Whatever the case authorises twice, the group holds the rule first: either from
        // its creation, or from this call.
        let first = vec![Permission::new(
            "tcp",
            Some((443, 443)),
            vec![Dest::ipv4("10.0.0.0/8")],
        )];
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&first));
        let (status, body) = authorize(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first authorize must succeed, got {status} with {body}",
            name = case.name
        );
        let before = stored_egress_rule_ids(&engine, &group_id);

        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&case.permissions));
        let (status, body) = authorize(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INVALID_PERMISSION_DUPLICATE,
            "case {name:?}: expected {expected}, got {body}",
            name = case.name,
            expected = ec2::state::INVALID_PERMISSION_DUPLICATE
        );
        assert_eq!(
            stored_egress_rule_ids(&engine, &group_id),
            before,
            "case {name:?}: a refused authorize must add nothing",
            name = case.name
        );
    }
}

/// One call cannot add the same rule twice either: the whole request is refused and nothing
/// is added.
#[tokio::test]
async fn one_call_naming_the_same_rule_twice_is_refused_and_changes_nothing() {
    let (router, engine) = emulator();
    let group_id = create_group("twice", &router, "twice").await;
    let before = stored_egress_rule_ids(&engine, &group_id);

    let permissions = vec![
        Permission::new("tcp", Some((443, 443)), vec![Dest::ipv4("10.0.0.0/8")]),
        Permission::new(
            "tcp",
            Some((443, 443)),
            vec![Dest::ipv4_canonicalised("10.0.0.5/8", "10.0.0.0/8")],
        ),
    ];
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(permission_params(&permissions));
    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_PERMISSION_DUPLICATE,
        "expected {expected}, got {body}",
        expected = ec2::state::INVALID_PERMISSION_DUPLICATE
    );
    assert_eq!(
        stored_egress_rule_ids(&engine, &group_id),
        before,
        "a refused authorize must add nothing"
    );
}

/// A `DryRun` request that would have succeeded answers `DryRunOperation` and adds nothing.
#[tokio::test]
async fn a_dry_run_that_would_have_succeeded_adds_nothing() {
    let (router, engine) = emulator();
    let group_id = create_group("dry run", &router, "dry-run").await;
    let before = stored_egress_rule_ids(&engine, &group_id);

    let permissions = vec![Permission::new(
        "tcp",
        Some((443, 443)),
        vec![Dest::ipv4("10.0.0.0/8")],
    )];
    let mut params = vec![param("GroupId", &group_id), param("DryRun", "true")];
    params.extend(permission_params(&permissions));
    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 412,
        "dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "dry run: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_egress_rule_ids(&engine, &group_id),
        before,
        "a dry run must add nothing"
    );
}

/// Each request the service cannot satisfy is refused with the EC2 error code and HTTP
/// status that names why, and adds nothing. Every row is a different way one request can be
/// wrong: a member left out, a member that is not supported, a value outside its published
/// range, a resource that does not exist.
#[tokio::test]
async fn a_request_the_service_cannot_satisfy_is_refused_and_adds_nothing() {
    /// One row: the case, the form fields BESIDE `GroupId`, the error code and the status.
    /// `GroupId` is added for every row that does not name one itself.
    struct Case {
        name: &'static str,
        params: Vec<Param>,
        names_group: bool,
        code: &'static str,
        status: u16,
    }

    let one_tcp_rule = || {
        permission_params(&[Permission::new(
            "tcp",
            Some((443, 443)),
            vec![Dest::ipv4("10.0.0.0/8")],
        )])
    };

    let cases = vec![
        Case {
            name: "an empty GroupId",
            params: {
                let mut params = vec![param("GroupId", "")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: true,
            code: ec2::state::MISSING_PARAMETER,
            status: 400,
        },
        Case {
            name: "a group this account does not hold",
            params: {
                let mut params = vec![param("GroupId", "sg-00000000000000999")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: true,
            code: ec2::state::INVALID_GROUP_NOT_FOUND,
            status: 400,
        },
        Case {
            name: "a GroupId that is not a security group id",
            params: {
                let mut params = vec![param("GroupId", "not-a-group")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: true,
            code: ec2::state::INVALID_GROUP_ID_MALFORMED,
            status: 400,
        },
        Case {
            name: "a GroupId carrying the right prefix and the wrong number of digits",
            params: {
                let mut params = vec![param("GroupId", "sg-0123")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: true,
            code: ec2::state::INVALID_GROUP_ID_MALFORMED,
            status: 400,
        },
        Case {
            name: "no IpPermissions",
            params: Vec::new(),
            names_group: false,
            code: ec2::state::MISSING_PARAMETER,
            status: 400,
        },
        Case {
            name: "the unsupported CidrIp member",
            params: {
                let mut params = vec![param("CidrIp", "10.0.0.0/8")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "the unsupported IpProtocol member",
            params: {
                let mut params = vec![param("IpProtocol", "tcp")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "the unsupported FromPort member",
            params: {
                let mut params = vec![param("FromPort", "443")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "the unsupported ToPort member",
            params: {
                let mut params = vec![param("ToPort", "443")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "the unsupported SourceSecurityGroupName member",
            params: {
                let mut params = vec![param("SourceSecurityGroupName", "other")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "the unsupported SourceSecurityGroupOwnerId member",
            params: {
                let mut params = vec![param("SourceSecurityGroupOwnerId", "123456789012")];
                params.extend(one_tcp_rule());
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a permission with no protocol",
            params: vec![param("IpPermissions.1.IpRanges.1.CidrIp", "10.0.0.0/8")],
            names_group: false,
            code: ec2::state::MISSING_PARAMETER,
            status: 400,
        },
        Case {
            name: "a protocol that is neither a name nor a number",
            params: permission_params(&[Permission::new(
                "sctpish",
                None,
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a protocol number past the published range",
            params: permission_params(&[Permission::new(
                "256",
                None,
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "tcp with no port range",
            params: permission_params(&[Permission::new(
                "tcp",
                None,
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "one end of a port range without the other",
            params: {
                let mut params = permission_params(&[Permission::new(
                    "47",
                    None,
                    vec![Dest::ipv4("10.0.0.0/8")],
                )]);
                params.push(param("IpPermissions.1.FromPort", "443"));
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PERMISSION_MALFORMED,
            status: 400,
        },
        Case {
            name: "a port past the highest TCP port",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((80, 65_536)),
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "an ICMP type past the highest one",
            params: permission_params(&[Permission::new(
                "icmp",
                Some((256, 0)),
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a port range that ends before it starts",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 80)),
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PERMISSION_MALFORMED,
            status: 400,
        },
        Case {
            name: "all ICMP types with a single code",
            params: permission_params(&[Permission::new(
                "icmp",
                Some((-1, 0)),
                vec![Dest::ipv4("10.0.0.0/8")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PERMISSION_MALFORMED,
            status: 400,
        },
        Case {
            name: "a permission naming no destination at all",
            params: permission_params(&[Permission::new("tcp", Some((443, 443)), Vec::new())]),
            names_group: false,
            code: ec2::state::MISSING_PARAMETER,
            status: 400,
        },
        Case {
            name: "an IPv4 block that is not a CIDR block",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv4("10.0.0.0/33")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "an IPv6 block that is not a CIDR block",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv6("2001:db8::/129")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a prefix list id that is not one",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::prefix_list("not-a-prefix-list")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a well-formed prefix list id no prefix list answers to",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::prefix_list("pl-00000000000000bad")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PREFIX_LIST_ID_NOT_FOUND,
            status: 400,
        },
        Case {
            name: "a referenced group this account does not hold",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::group_by_id("sg-00000000000000999")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_GROUP_NOT_FOUND,
            status: 400,
        },
        Case {
            name: "a referenced group id that is not a security group id",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::group_by_id("not-a-group")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_GROUP_ID_MALFORMED,
            status: 400,
        },
        Case {
            name: "a referenced group name no group in the VPC answers to",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::group_by_name("no-such-group", "unused")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_GROUP_NOT_FOUND,
            status: 400,
        },
        Case {
            name: "a referenced group named neither by id nor by name",
            params: vec![
                param("IpPermissions.1.IpProtocol", "tcp"),
                param("IpPermissions.1.FromPort", "443"),
                param("IpPermissions.1.ToPort", "443"),
                param("IpPermissions.1.Groups.1.Description", "nothing named"),
            ],
            names_group: false,
            code: ec2::state::INVALID_PERMISSION_MALFORMED,
            status: 400,
        },
        Case {
            name: "a description one character past the published bound",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv4("10.0.0.0/8")
                    .described(&"d".repeat(ec2::state::MAX_SECURITY_GROUP_TEXT_CHARS + 1))],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a description outside the published alphabet",
            params: permission_params(&[Permission::new(
                "tcp",
                Some((443, 443)),
                vec![Dest::ipv4("10.0.0.0/8").described("no tildes ~ allowed")],
            )]),
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
        Case {
            name: "a tag specification naming another resource type",
            params: {
                let mut params = one_tcp_rule();
                params.push(param(
                    "TagSpecification.1.ResourceType",
                    ec2::state::RESOURCE_TYPE_SECURITY_GROUP,
                ));
                params.push(param("TagSpecification.1.Tag.1.Key", "Name"));
                params.push(param("TagSpecification.1.Tag.1.Value", "nope"));
                params
            },
            names_group: false,
            code: ec2::state::INVALID_PARAMETER_VALUE,
            status: 400,
        },
    ];

    for case in cases {
        let (router, engine) = emulator();
        let group_id = create_group(case.name, &router, "refusals").await;
        let before = stored_egress_rule_ids(&engine, &group_id);

        let mut params: Vec<Param> = Vec::new();
        if !case.names_group {
            params.push(param("GroupId", &group_id));
        }
        params.extend(case.params.clone());
        let (status, body) = authorize(&router, &params).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected HTTP {expected}, got {status} with body {body}",
            name = case.name,
            expected = case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: expected {expected}, got {body}",
            name = case.name,
            expected = case.code
        );
        assert_eq!(
            stored_egress_rule_ids(&engine, &group_id),
            before,
            "case {name:?}: a refused authorize must add nothing",
            name = case.name
        );
    }
}

/// A group holds at most the published number of rules on one side: the call that would
/// take it past that is refused, and the one that fills it exactly is not. The group starts
/// with the one allow-all outbound rule it was created with, so the published bound is
/// reached one rule earlier.
#[tokio::test]
async fn the_published_rules_per_group_bound_is_the_ceiling() {
    let bound = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    let cases = [
        ("one rule short of the bound", bound - 2, true),
        ("exactly the bound", bound - 1, true),
        ("one rule past the bound", bound, false),
    ];

    for (name, requested, allowed) in cases {
        let (router, engine) = emulator();
        let group_id = create_group(name, &router, "bounded").await;
        let before = stored_egress_rule_ids(&engine, &group_id);
        assert_eq!(
            before.len(),
            1,
            "case {name:?}: a new group starts with its one allow-all outbound rule, \
             got {before:?}"
        );

        // One permission naming `requested` distinct single-address destinations, so one
        // call asks for exactly that many rules.
        let destinations: Vec<Dest> = (0..requested)
            .map(|index| Dest::ipv4(&format!("10.{}.{}.0/24", index / 256, index % 256)))
            .collect();
        let permissions = vec![Permission::new("tcp", Some((443, 443)), destinations)];
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&permissions));
        let (status, body) = authorize(&router, &params).await;

        if allowed {
            assert_eq!(
                status, 200,
                "case {name:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                stored_egress_rule_ids(&engine, &group_id).len(),
                requested + 1,
                "case {name:?}: expected every requested rule to be stored"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {name:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED,
                "case {name:?}: expected {expected}, got {body}",
                expected = ec2::state::RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED
            );
            assert_eq!(
                stored_egress_rule_ids(&engine, &group_id),
                before,
                "case {name:?}: a refused authorize must add nothing"
            );
        }
    }
}

/// An authorised outbound rule is an OUTBOUND rule: it is not reported among the group's
/// inbound rules, and the inbound side is left empty — "when you create a security group,
/// it has no inbound rules".
#[tokio::test]
async fn an_authorised_rule_is_added_to_the_outbound_side_only() {
    let (router, engine) = emulator();
    let group_id = create_group("sides", &router, "sided").await;

    let permissions = vec![Permission::new(
        "tcp",
        Some((443, 443)),
        vec![Dest::ipv4("198.51.100.0/24")],
    )];
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(permission_params(&permissions));
    let (status, body) = authorize(&router, &params).await;
    assert_eq!(
        status, 200,
        "sides: expected HTTP 200, got {status} with body {body}"
    );

    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .security_group_rule_count(&group_id, false),
        0,
        "sides: authorising an outbound rule must leave the inbound side empty"
    );
    let described = describe("sides", &router, &group_id).await;
    let egress = egress_section("sides", &described);
    assert!(
        egress.contains("<cidrIp>198.51.100.0/24</cidrIp>"),
        "sides: expected the rule among the outbound rules, got {egress}"
    );
    let ingress_onwards = described
        .split("<ipPermissions>")
        .nth(1)
        .unwrap_or_default()
        .split("</ipPermissions>")
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        !ingress_onwards.contains("198.51.100.0/24"),
        "sides: the rule must not be reported among the inbound rules, got \
         {ingress_onwards}"
    );
}

/// The rule: every caller-chosen string an authorised rule RETAINS is charged against the
/// emulator's byte budget — the destination it names as well as its description and its
/// tags — so storing rules is bounded in bytes and not only in records, and the allow-all
/// outbound rule the SERVICE gives a new group costs that budget nothing.
///
/// The axis is what the rule carries: a destination alone, a destination with a
/// description, one with a description and tags, and a longer (IPv6) destination — so the
/// charge is read as a function of the request rather than of one case.
#[tokio::test]
async fn the_store_charges_the_caller_chosen_bytes_each_authorised_rule_retains() {
    /// One charge row: the case name, the destination the rule names, and the tag pairs
    /// the request assigns to it.
    type ChargeCase = (&'static str, Dest, Vec<(&'static str, &'static str)>);

    let cases: [ChargeCase; 4] = [
        ("a destination alone", Dest::ipv4("10.1.0.0/16"), Vec::new()),
        (
            "a destination and a description",
            Dest {
                kind: Kind::Ipv4,
                requested: "10.2.0.0/16".to_string(),
                stored: "10.2.0.0/16".to_string(),
                description: Some("allow https out".to_string()),
            },
            Vec::new(),
        ),
        (
            "a destination, a description and two tags",
            Dest {
                kind: Kind::Ipv4,
                requested: "10.3.0.0/16".to_string(),
                stored: "10.3.0.0/16".to_string(),
                description: Some("allow https out".to_string()),
            },
            vec![("Name", "egress"), ("Owner", "TeamA")],
        ),
        (
            "a longer destination",
            Dest::ipv6("2001:db8:1234:1a00::/56"),
            Vec::new(),
        ),
    ];

    for (case, destination, tags) in cases {
        let (router, engine) = emulator();
        let group_id = create_group(case, &router, "charged").await;

        // The group and its service-chosen allow-all outbound rule are already stored, so
        // the delta below is the authorised rule's charge alone.
        let before = engine.state::<Ec2State>().lock().stored_bytes();

        // Every stored record costs the fixed per-record overhead this store charges, plus
        // the caller-chosen strings the rule retains — its protocol among them, which every
        // rule of the permission keeps a copy of.
        let protocol = "tcp";
        let expected = ec2::state::STORED_RECORD_OVERHEAD_BYTES
            + protocol.len()
            + destination.stored.len()
            + destination.description.as_deref().map_or(0, str::len)
            + tags
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>();

        let permissions = vec![Permission::new(
            protocol,
            Some((443, 443)),
            vec![destination.clone()],
        )];
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(permission_params(&permissions));
        if !tags.is_empty() {
            params.push(param(
                "TagSpecification.1.ResourceType",
                ec2::state::RESOURCE_TYPE_SECURITY_GROUP_RULE,
            ));
            for (index, (key, value)) in tags.iter().enumerate() {
                let position = index + 1;
                params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
                params.push(param(
                    &format!("TagSpecification.1.Tag.{position}.Value"),
                    value,
                ));
            }
        }

        let (status, body) = authorize(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let after = engine.state::<Ec2State>().lock().stored_bytes();
        assert_eq!(
            after - before,
            expected,
            "case {case:?}: expected the store to charge {expected} caller-chosen bytes, \
             it went from {before} to {after}"
        );
    }
}

/// The rule: the published rules-per-group bound is the bound on what the GROUP holds, not
/// on what one request asks for, so it holds across calls — the authorize that would take
/// the group past it is refused however many calls the rules arrived in, and the rules
/// already stored are left exactly as they were.
#[tokio::test]
async fn the_rules_per_group_bound_holds_across_calls() {
    let bound = ec2::state::MAX_SECURITY_GROUP_RULES_PER_DIRECTION;
    // (case, how many rules each call asks for) — the rows differ in how the same total is
    // split between calls, which is what "per group, not per request" generalises over.
    let cases: [(&str, Vec<usize>); 3] = [
        ("one rule per call", vec![1; bound]),
        ("two calls", vec![bound / 2, bound / 2]),
        ("many then one", vec![bound - 10, 9, 1]),
    ];

    for (case, calls) in cases {
        let (router, engine) = emulator();
        let group_id = create_group(case, &router, "across-calls").await;
        // A new group already holds its one allow-all outbound rule, so the group reaches
        // the bound one rule before the calls have asked for `bound` of them.
        let mut stored = stored_egress_rule_ids(&engine, &group_id).len();
        let mut next = 0usize;

        for (index, asked) in calls.iter().enumerate() {
            let destinations: Vec<Dest> = (0..*asked)
                .map(|offset| {
                    let which = next + offset;
                    Dest::ipv4(&format!("10.{}.{}.0/24", which / 256, which % 256))
                })
                .collect();
            next += *asked;
            let permissions = vec![Permission::new("tcp", Some((8443, 8443)), destinations)];
            let mut params = vec![param("GroupId", &group_id)];
            params.extend(permission_params(&permissions));
            let (status, body) = authorize(&router, &params).await;

            let fits = stored + *asked <= bound;
            if fits {
                assert_eq!(
                    status, 200,
                    "case {case:?} call {index}: expected HTTP 200, got {status} with body \
                     {body}"
                );
                stored += *asked;
            } else {
                assert_eq!(
                    status, 400,
                    "case {case:?} call {index}: expected HTTP 400 once the group is at the \
                     bound, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    ec2::state::RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED,
                    "case {case:?} call {index}: expected {expected}, got {body}",
                    expected = ec2::state::RULES_PER_SECURITY_GROUP_LIMIT_EXCEEDED
                );
            }
            assert_eq!(
                stored_egress_rule_ids(&engine, &group_id).len(),
                stored,
                "case {case:?} call {index}: the group must hold exactly the rules the \
                 accepted calls added, and never more than {bound}"
            );
            assert!(
                stored <= bound,
                "case {case:?} call {index}: expected at most {bound} outbound rules, got \
                 {stored}"
            );
        }
    }
}

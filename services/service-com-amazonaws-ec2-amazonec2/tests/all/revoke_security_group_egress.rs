// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RevokeSecurityGroupEgress`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=RevokeSecurityGroupEgress`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a revoke removes from the named group exactly those OUTBOUND rules
//! the request names — a rule being a protocol, a port range and ONE destination, matched on
//! those values and not on its description — and nothing else, leaving the group's inbound
//! rules where they were; a rule the group does not hold is refused with
//! `InvalidPermission.NotFound` and revokes nothing when the group is in a nondefault VPC,
//! and is reported under `unknownIpPermissionSet` when it is in the default VPC.
//!
//! Each row of the first table is a different input along what that generalises over: the
//! protocol and how it is spelled (name, IANA number, all-protocols), whether the rule
//! carries a port range, which kind of destination it names (an IPv4 block, an IPv6 block, a
//! prefix list, another group), whether it carries a description, and whether the request
//! names it by its rule properties or by the `SecurityGroupRuleId` the service minted for it.
//!
//! The later suites pin the two halves a revoke is refused on: a rule the group does not
//! hold, and a request whose shape names no revoke this operation serves (the EC2-Classic
//! members it does not support, both ways of naming rules at once, a group id that is
//! malformed or holds no group).

use ec2::ops::types::{IpPermission, IpRange, Ipv6Range, PrefixListId, UserIdGroupPair};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupRuleAtom, SecurityGroupRuleSource,
    SecurityGroupSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code};

/// The Region every call here is signed for; it is the Region a seeded group's ARN names.
const REGION: &str = "us-west-2";

/// The inbound rule every seeded group here holds — what an OUTBOUND revoke must leave
/// exactly as it was.
const INBOUND_PORT: i32 = 22;
const INBOUND_CIDR: &str = "10.9.0.0/16";

/// A group of the same account an outbound rule can name as its destination, and a prefix
/// list another can: both well-formed ids of their kind, so a stored rule naming one is one
/// a request could really carry.
const DESTINATION_GROUP: &str = "sg-0123456789abcdef0";
const DESTINATION_PREFIX_LIST: &str = "pl-0123456789abcdef0";

/// One `RevokeSecurityGroupEgress` call, as its HTTP status and XML body.
async fn revoke(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "RevokeSecurityGroupEgress", params).await
}

fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// What one outbound rule allows traffic TO: exactly one destination, the way `IpPermission`
/// carries one.
#[derive(Debug, Clone, Copy)]
enum Destination {
    Ipv4(&'static str),
    Ipv6(&'static str),
    PrefixList(&'static str),
    /// Another security group, by id.
    Group(&'static str),
}

/// One outbound rule, as a row writes it: a protocol, an optional port range, one
/// destination and the description it may carry.
#[derive(Debug, Clone, Copy)]
struct Rule {
    protocol: &'static str,
    ports: Option<(i32, i32)>,
    destination: Destination,
    description: Option<&'static str>,
}

/// A rule with no port range and no description.
const fn rule(protocol: &'static str, destination: Destination) -> Rule {
    Rule {
        protocol,
        ports: None,
        destination,
        description: None,
    }
}

/// The same with a port range.
const fn ranged(protocol: &'static str, ports: (i32, i32), destination: Destination) -> Rule {
    Rule {
        protocol,
        ports: Some(ports),
        destination,
        description: None,
    }
}

impl Rule {
    /// The rule as the stored `IpPermission` that holds it.
    fn permission(self) -> IpPermission {
        let description = self.description.map(str::to_string);
        let mut permission = IpPermission {
            ip_protocol: Some(self.protocol.to_string()),
            from_port: self.ports.map(|(from, _)| from),
            to_port: self.ports.map(|(_, to)| to),
            user_id_group_pairs: Some(Vec::new()),
            ip_ranges: Some(Vec::new()),
            ipv6_ranges: Some(Vec::new()),
            prefix_list_ids: Some(Vec::new()),
        };
        match self.destination {
            Destination::Ipv4(cidr) => {
                permission.ip_ranges = Some(vec![IpRange {
                    description,
                    cidr_ip: Some(cidr.to_string()),
                }]);
            }
            Destination::Ipv6(cidr) => {
                permission.ipv6_ranges = Some(vec![Ipv6Range {
                    description,
                    cidr_ipv6: Some(cidr.to_string()),
                }]);
            }
            Destination::PrefixList(prefix_list_id) => {
                permission.prefix_list_ids = Some(vec![PrefixListId {
                    description,
                    prefix_list_id: Some(prefix_list_id.to_string()),
                }]);
            }
            Destination::Group(group_id) => {
                permission.user_id_group_pairs = Some(vec![UserIdGroupPair {
                    description,
                    group_id: Some(group_id.to_string()),
                    ..UserIdGroupPair::default()
                }]);
            }
        }
        permission
    }

    /// The rule as the `IpPermissions.1.…` form fields of a request. The keys are the ones
    /// the model's `@ec2QueryName`s resolve to — note `Groups.N` for `UserIdGroupPairs`
    /// (`smithy_query(query="shape",shape="IpPermission")`).
    fn permission_params(self) -> Vec<(String, String)> {
        let mut params = vec![param("IpPermissions.1.IpProtocol", self.protocol)];
        if let Some((from, to)) = self.ports {
            params.push(param("IpPermissions.1.FromPort", &from.to_string()));
            params.push(param("IpPermissions.1.ToPort", &to.to_string()));
        }
        match self.destination {
            Destination::Ipv4(cidr) => {
                params.push(param("IpPermissions.1.IpRanges.1.CidrIp", cidr));
            }
            Destination::Ipv6(cidr) => {
                params.push(param("IpPermissions.1.Ipv6Ranges.1.CidrIpv6", cidr));
            }
            Destination::PrefixList(prefix_list_id) => params.push(param(
                "IpPermissions.1.PrefixListIds.1.PrefixListId",
                prefix_list_id,
            )),
            Destination::Group(group_id) => {
                params.push(param("IpPermissions.1.Groups.1.GroupId", group_id));
            }
        }
        params
    }

    /// The marker a described group carries for this rule's destination, so a row can assert
    /// the rule is gone from the reporting half too.
    fn destination_marker(self) -> String {
        match self.destination {
            Destination::Ipv4(cidr) => format!("<cidrIp>{cidr}</cidrIp>"),
            Destination::Ipv6(cidr) => format!("<cidrIpv6>{cidr}</cidrIpv6>"),
            Destination::PrefixList(id) => format!("<prefixListId>{id}</prefixListId>"),
            Destination::Group(id) => format!("<groupId>{id}</groupId>"),
        }
    }
}

/// How the request names the rule it revokes: the two documented ways — "you can specify
/// rules using either rule IDs or security group rule properties".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedBy {
    Properties,
    RuleId,
}

/// One stored rule, rendered so a test can compare rule SETS as text: the canonical
/// protocol, the port range and the one destination.
fn rendered(rule: &SecurityGroupRuleAtom) -> String {
    let ports = match (rule.from_port, rule.to_port) {
        (Some(from), Some(to)) => format!("{from}-{to}"),
        (from, to) => format!("{from:?}-{to:?}"),
    };
    let source = match &rule.source {
        SecurityGroupRuleSource::Ipv4(cidr) => format!("ipv4 {cidr}"),
        SecurityGroupRuleSource::Ipv6(cidr) => format!("ipv6 {cidr}"),
        SecurityGroupRuleSource::PrefixList(id) => format!("prefix-list {id}"),
        SecurityGroupRuleSource::ReferencedGroup(id) => format!("group {id}"),
    };
    format!("{} {ports} {source}", rule.ip_protocol)
}

/// The rules the store holds for `group_id` in one direction, each beside the
/// `SecurityGroupRuleId` it is stored under — the rules as the service matches on them.
fn stored_rules(
    engine: &engine::InMemoryEngine,
    group_id: &str,
    is_egress: bool,
) -> Vec<(String, SecurityGroupRuleAtom)> {
    engine
        .state::<Ec2State>()
        .lock()
        .held_security_group_rules(group_id, is_egress)
}

/// Those rules rendered and sorted, which is what a row compares before and after.
fn held(engine: &engine::InMemoryEngine, group_id: &str, is_egress: bool) -> Vec<String> {
    let mut held: Vec<String> = stored_rules(engine, group_id, is_egress)
        .iter()
        .map(|(_, atom)| rendered(atom))
        .collect();
    held.sort();
    held
}

/// The id of the account's default VPC — the VPC whose groups a revoke is FORGIVING about.
///
/// A Region's ambient default network is materialised the first time the account asks in
/// that Region, so it is read through the store's own one writer for it
/// (`ensure_default_network`): what the handler falls back on and what the test expects
/// cannot then be two different VPCs.
fn default_vpc_id(engine: &engine::InMemoryEngine) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .ensure_default_network(REGION)
        .expect("an account has a default VPC in every Region")
}

/// A second, nondefault VPC, written the way `CreateVpc` would.
fn insert_vpc(engine: &engine::InMemoryEngine, cidr_block: &str) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_vpc(cidr_block, false, "default", "available", REGION, Vec::new())
}

/// Write a group holding `outbound` as its outbound rules and one inbound rule, the way a
/// create plus an authorize would leave it, and answer its `GroupId`.
fn seed_group(engine: &engine::InMemoryEngine, vpc_id: &str, outbound: &[Rule]) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.to_string(),
            group_name: "fixture".to_string(),
            description: "the fixture group".to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: vec![ranged(
                "tcp",
                (INBOUND_PORT, INBOUND_PORT),
                Destination::Ipv4(INBOUND_CIDR),
            )
            .permission()],
            ip_permissions_egress: outbound.iter().copied().map(Rule::permission).collect(),
        })
}

/// The `DescribeSecurityGroups` body for one group — the reporting half a revoke has to be
/// visible through.
async fn described(router: &MultiServiceRouter, case: &str, group_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeSecurityGroups",
        &[param("GroupId.1", group_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} with \
         body {body}"
    );
    body
}

/// The `<ipPermissionsEgress>` section of a described group: its OUTBOUND rules only, so an
/// inbound rule naming the same destination cannot be mistaken for an outbound one that
/// survived.
fn outbound_section(xml: &str) -> String {
    let Some(start) = xml.find("<ipPermissionsEgress>") else {
        return String::new();
    };
    let rest = &xml[start..];
    rest.find("</ipPermissionsEgress>")
        .map_or(String::new(), |end| rest[..end].to_string())
}

/// `sgr-` and 17 lowercase hexadecimal digits, the long EC2 resource id form.
fn is_rule_id(value: &str) -> bool {
    value
        .strip_prefix("sgr-")
        .is_some_and(|digits| digits.len() == 17 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One row of the table: the outbound rules the group starts with, which of them the request
/// names, how that rule is spelled in the request, and which of the two documented ways
/// names it.
struct RevokeCase {
    case: &'static str,
    seeded: &'static [Rule],
    /// The index into `seeded` of the rule the request names.
    target: usize,
    /// The target rule as the REQUEST spells it — the same rule, spelled the row's way.
    spelled: Rule,
    named_by: NamedBy,
}

const REVOKE_CASES: &[RevokeCase] = &[
    RevokeCase {
        case: "the all-traffic rule a new group carries, which has no port range",
        seeded: &[
            rule("-1", Destination::Ipv4("0.0.0.0/0")),
            ranged("tcp", (443, 443), Destination::Ipv4("10.1.0.0/16")),
        ],
        target: 0,
        spelled: rule("-1", Destination::Ipv4("0.0.0.0/0")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a single-port tcp rule",
        seeded: &[
            ranged("tcp", (443, 443), Destination::Ipv4("10.1.0.0/16")),
            ranged("tcp", (443, 443), Destination::Ipv4("10.2.0.0/16")),
        ],
        target: 0,
        spelled: ranged("tcp", (443, 443), Destination::Ipv4("10.1.0.0/16")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a udp port range, with the protocol spelled in upper case",
        seeded: &[ranged(
            "udp",
            (9092, 9094),
            Destination::Ipv4("192.168.0.0/24"),
        )],
        target: 0,
        // "The IP protocol name (tcp, udp, icmp)" is spelled in lower case by the model, so
        // a name is the same protocol in any case.
        spelled: ranged("UDP", (9092, 9094), Destination::Ipv4("192.168.0.0/24")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a protocol named by its IANA number, no port range",
        seeded: &[
            // 47 is GRE, a protocol with no port range and no name of its own in the model.
            rule("47", Destination::Ipv4("198.51.100.0/24")),
            rule("-1", Destination::Ipv4("0.0.0.0/0")),
        ],
        target: 0,
        spelled: rule("47", Destination::Ipv4("198.51.100.0/24")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "an IPv6 rule, spelled in upper case",
        seeded: &[
            ranged("tcp", (443, 443), Destination::Ipv6("2600:1f14::/36")),
            ranged("tcp", (443, 443), Destination::Ipv6("2001:db8::/32")),
        ],
        target: 0,
        spelled: ranged("tcp", (443, 443), Destination::Ipv6("2600:1F14::/36")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a prefix list destination",
        seeded: &[ranged(
            "tcp",
            (1024, 1024),
            Destination::PrefixList(DESTINATION_PREFIX_LIST),
        )],
        target: 0,
        spelled: ranged(
            "tcp",
            (1024, 1024),
            Destination::PrefixList(DESTINATION_PREFIX_LIST),
        ),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "another security group as the destination",
        seeded: &[ranged(
            "tcp",
            (5432, 5432),
            Destination::Group(DESTINATION_GROUP),
        )],
        target: 0,
        spelled: ranged("tcp", (5432, 5432), Destination::Group(DESTINATION_GROUP)),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a rule carrying a description, revoked without naming it",
        seeded: &[Rule {
            description: Some("dns out of the network"),
            ..ranged("udp", (53, 53), Destination::Ipv4("10.3.0.0/16"))
        }],
        target: 0,
        spelled: ranged("udp", (53, 53), Destination::Ipv4("10.3.0.0/16")),
        named_by: NamedBy::Properties,
    },
    RevokeCase {
        case: "a rule named by the id the service minted for it",
        seeded: &[
            ranged("tcp", (8080, 8080), Destination::Ipv4("10.4.0.0/16")),
            rule("-1", Destination::Ipv4("0.0.0.0/0")),
        ],
        target: 0,
        spelled: ranged("tcp", (8080, 8080), Destination::Ipv4("10.4.0.0/16")),
        named_by: NamedBy::RuleId,
    },
];

/// A revoke removes exactly the outbound rules the request names, leaves the group's other
/// rules — in both directions — where they were, and reports what it removed.
#[tokio::test]
async fn a_revoke_removes_exactly_the_outbound_rules_the_request_names() {
    for row in REVOKE_CASES {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        // A nondefault VPC: every rule this row names is one the group holds, so the strict
        // half of the rule is the one under test here.
        let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
        let group_id = seed_group(&engine, &vpc_id, row.seeded);

        let before = held(&engine, &group_id, true);
        assert_eq!(
            before.len(),
            row.seeded.len(),
            "case {case:?}: expected the seeded outbound rules to be held, got {before:?}"
        );
        let inbound_before = held(&engine, &group_id, false);
        let stored = stored_rules(&engine, &group_id, true);
        let target_rendered = rendered(&stored[row.target].1);

        let mut params = vec![param("GroupId", &group_id)];
        match row.named_by {
            NamedBy::Properties => params.extend(row.spelled.permission_params()),
            NamedBy::RuleId => {
                params.push(param("SecurityGroupRuleId.1", &stored[row.target].0));
            }
        }
        let (status, body) = revoke(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return>, got {body}"
        );
        assert!(
            !body.contains("<unknownIpPermissionSet>"),
            "case {case:?}: every named rule was held, so nothing is unknown, got {body}"
        );

        // The one rule it took, reported with the id the store minted for it and the values
        // the STORED rule had — including the description the request did not name.
        let reported_ids = elements(&body, "securityGroupRuleId");
        assert_eq!(
            reported_ids.len(),
            1,
            "case {case:?}: expected exactly one revoked rule, got {reported_ids:?} from \
             {body}"
        );
        assert!(
            is_rule_id(&reported_ids[0]),
            "case {case:?}: expected an sgr- rule id, got {:?}",
            reported_ids[0]
        );
        assert_eq!(
            element(&body, "groupId").as_deref(),
            Some(group_id.as_str()),
            "case {case:?}: expected the revoked rule to name its group, got {body}"
        );
        assert_eq!(
            element(&body, "isEgress").as_deref(),
            Some("true"),
            "case {case:?}: an egress revoke reports outbound rules, got {body}"
        );
        if let Some(description) = row.seeded[row.target].description {
            assert_eq!(
                element(&body, "description").as_deref(),
                Some(description),
                "case {case:?}: expected the stored description {description:?}, got {body}"
            );
        }

        // The store now holds every outbound rule but that one, the inbound rules are
        // untouched, and the reporting half agrees.
        let after = held(&engine, &group_id, true);
        let expected: Vec<String> = before
            .iter()
            .filter(|rule| **rule != target_rendered)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?} to be left outbound, got {after:?}"
        );
        let inbound_after = held(&engine, &group_id, false);
        assert_eq!(
            inbound_after, inbound_before,
            "case {case:?}: expected the inbound rules to be untouched \
             ({inbound_before:?}), got {inbound_after:?}"
        );
        let described = described(&router, case, &group_id).await;
        let marker = row.seeded[row.target].destination_marker();
        assert!(
            !outbound_section(&described).contains(&marker),
            "case {case:?}: expected {marker} to be gone from the group's outbound rules, \
             got {described}"
        );
    }
}

/// A rule the group does not hold: refused with `InvalidPermission.NotFound` in a nondefault
/// VPC, with nothing revoked, and reported under `unknownIpPermissionSet` in the default VPC
/// — where the revoke still succeeds and still takes nothing.
#[tokio::test]
async fn a_rule_the_group_does_not_hold_follows_its_vpc() {
    // (case, whether the group sits in the account's default VPC, the code expected)
    let rows: [(&str, bool, Option<&str>); 2] = [
        (
            "a nondefault VPC refuses the whole revoke",
            false,
            Some("InvalidPermission.NotFound"),
        ),
        ("the default VPC reports the rule as unknown", true, None),
    ];

    for (case, in_default_vpc, expected_code) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = if in_default_vpc {
            default_vpc_id(&engine)
        } else {
            insert_vpc(&engine, "10.0.0.0/16")
        };
        let seeded: &[Rule] = &[rule("-1", Destination::Ipv4("0.0.0.0/0"))];
        let group_id = seed_group(&engine, &vpc_id, seeded);
        let before = held(&engine, &group_id, true);

        let mut params = vec![param("GroupId", &group_id)];
        params.extend(ranged("tcp", (443, 443), Destination::Ipv4("10.8.0.0/16")).permission_params());
        let (status, body) = revoke(&router, &params).await;
        match expected_code {
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                let reported = error_code(&body);
                assert_eq!(
                    reported, code,
                    "case {case:?}: expected the code {code}, got {reported:?} from {body}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                assert!(
                    body.contains("<unknownIpPermissionSet>"),
                    "case {case:?}: expected the rule to be reported as unknown, got {body}"
                );
                assert!(
                    elements(&body, "securityGroupRuleId").is_empty(),
                    "case {case:?}: expected no rule to be revoked, got {body}"
                );
            }
        }
        let after = held(&engine, &group_id, true);
        assert_eq!(
            after, before,
            "case {case:?}: expected the group's outbound rules to be untouched \
             ({before:?}), got {after:?}"
        );
    }
}

/// One row of the refusal table: the parameters the request carries beside its group, and
/// the error code EC2 answers it with. `group_id` is set only by a row refused for the GROUP
/// it names — an empty value among them, since `GroupId` is modelled `clientOptional` and so
/// reaches the handler; every other row names the seeded group.
struct RefusalCase {
    case: &'static str,
    group_id: Option<&'static str>,
    params: fn() -> Vec<(String, String)>,
    code: &'static str,
}

const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "no group named at all",
        group_id: Some(""),
        params: || rule("-1", Destination::Ipv4("0.0.0.0/0")).permission_params(),
        code: "MissingParameter",
    },
    RefusalCase {
        case: "an EC2-Classic flat member, which this operation does not support",
        group_id: None,
        params: || vec![param("CidrIp", "0.0.0.0/0")],
        code: "InvalidParameterValue",
    },
    RefusalCase {
        case: "the flat protocol member, the same way",
        group_id: None,
        params: || vec![param("IpProtocol", "-1")],
        code: "InvalidParameterValue",
    },
    RefusalCase {
        case: "both ways of naming rules at once",
        group_id: None,
        params: || {
            let mut params = vec![param("SecurityGroupRuleId.1", "sgr-0123456789abcdef0")];
            params.extend(rule("-1", Destination::Ipv4("0.0.0.0/0")).permission_params());
            params
        },
        code: "InvalidParameterCombination",
    },
    RefusalCase {
        case: "a request naming no rule at all",
        group_id: None,
        params: Vec::new,
        code: "MissingParameter",
    },
    RefusalCase {
        case: "a permission naming no protocol",
        group_id: None,
        params: || vec![param("IpPermissions.1.IpRanges.1.CidrIp", "0.0.0.0/0")],
        code: "MissingParameter",
    },
    RefusalCase {
        case: "a permission naming no destination",
        group_id: None,
        params: || vec![param("IpPermissions.1.IpProtocol", "-1")],
        code: "MissingParameter",
    },
    RefusalCase {
        case: "a rule id of another kind",
        group_id: None,
        params: || vec![param("SecurityGroupRuleId.1", "vol-0123456789abcdef0")],
        code: "InvalidSecurityGroupRuleId.Malformed",
    },
    RefusalCase {
        case: "a rule id no rule of the group carries",
        group_id: None,
        params: || vec![param("SecurityGroupRuleId.1", "sgr-0123456789abcdef0")],
        code: "InvalidSecurityGroupRuleId.NotFound",
    },
    RefusalCase {
        case: "a group id that is not one",
        group_id: Some("not-a-group"),
        params: || rule("-1", Destination::Ipv4("0.0.0.0/0")).permission_params(),
        code: "InvalidGroupId.Malformed",
    },
    RefusalCase {
        case: "a well-formed group id this account holds no group for",
        group_id: Some("sg-0fedcba987654321f"),
        params: || rule("-1", Destination::Ipv4("0.0.0.0/0")).permission_params(),
        code: "InvalidGroup.NotFound",
    },
];

/// A request whose shape names no revoke this operation serves is refused with the code EC2
/// answers for it, and the group's rules are left exactly as they were.
#[tokio::test]
async fn a_request_naming_no_servable_revoke_is_refused() {
    for row in REFUSAL_CASES {
        let case = row.case;
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
        let seeded: &[Rule] = &[rule("-1", Destination::Ipv4("0.0.0.0/0"))];
        let group_id = seed_group(&engine, &vpc_id, seeded);
        let before = held(&engine, &group_id, true);

        let named = row.group_id.unwrap_or(group_id.as_str());
        let mut params = vec![param("GroupId", named)];
        params.extend((row.params)());
        let (status, body) = revoke(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let reported = error_code(&body);
        assert_eq!(
            reported, row.code,
            "case {case:?}: expected the code {}, got {reported:?} from {body}",
            row.code
        );
        let after = held(&engine, &group_id, true);
        assert_eq!(
            after, before,
            "case {case:?}: expected the group's outbound rules to be untouched \
             ({before:?}), got {after:?}"
        );
    }
}

/// A dry run of a revoke that would have succeeded answers `DryRunOperation` and removes
/// nothing: "if you have the required permissions, the error response is DryRunOperation"
/// (`$DryRun`).
#[tokio::test]
async fn a_dry_run_of_a_servable_revoke_removes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
    let seeded: &[Rule] = &[rule("-1", Destination::Ipv4("0.0.0.0/0"))];
    let group_id = seed_group(&engine, &vpc_id, seeded);
    let before = held(&engine, &group_id, true);
    let case = "a dry run of the all-traffic outbound rule";

    let mut params = vec![param("GroupId", &group_id), param("DryRun", "true")];
    params.extend(rule("-1", Destination::Ipv4("0.0.0.0/0")).permission_params());
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected the dry run to be refused, got {status} with body {body}"
    );
    let reported = error_code(&body);
    assert_eq!(
        reported, "DryRunOperation",
        "case {case:?}: expected the code DryRunOperation, got {reported:?} from {body}"
    );
    let after = held(&engine, &group_id, true);
    assert_eq!(
        after, before,
        "case {case:?}: expected the group's outbound rules to be untouched ({before:?}), \
         got {after:?}"
    );
}

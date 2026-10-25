// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RevokeSecurityGroupIngress`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=RevokeSecurityGroupIngress`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a revoke removes from the named group exactly those INBOUND rules
//! the request names — a rule being a protocol, a port range and ONE source, matched on
//! those values and not on its description — and nothing else; a rule the group does not
//! hold is refused with `InvalidPermission.NotFound` and revokes nothing when the group is
//! in a nondefault VPC, and is reported under `unknownIpPermissionSet` when it is in the
//! default VPC.
//!
//! Each table row is a different input along what that generalises over: which protocol
//! and how it is spelled (name, IANA number, `all`), whether the rule carries a port range,
//! which kind of source (IPv4 block, IPv6 block, prefix list, another group), which request
//! form names it (the flat members or `IpPermissions.N`), how many sources one stored
//! permission aggregates, and which member of a rule the request gets wrong.

use ec2::ops::types::{IpPermission, IpRange, Ipv6Range, PrefixListId, UserIdGroupPair};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupRuleAtom, SecurityGroupRuleSource,
    SecurityGroupSpec,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, indexed};

/// The Region every call here is signed for; it is the Region a seeded group's ARN names.
const REGION: &str = "us-west-2";

/// One `RevokeSecurityGroupIngress` call, as its HTTP status and XML body.
async fn revoke(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "RevokeSecurityGroupIngress", params).await
}

/// The `DescribeSecurityGroups` body for one group — the reporting half a revoke has to be
/// visible through.
async fn described(router: &MultiServiceRouter, case: &str, group_id: &str) -> String {
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeSecurityGroups", &indexed("GroupId", &[group_id]))
            .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSecurityGroups, got {status} with \
         body {body}"
    );
    body
}

fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The `<ipPermissions>` section of a described group: its INBOUND rules only, so a source
/// a group also names in an outbound rule (every group's allow-all egress names
/// `0.0.0.0/0`) cannot be mistaken for an inbound one that survived.
fn inbound_section(xml: &str) -> String {
    element(xml, "ipPermissions").unwrap_or_default()
}

/// What one rule allows traffic from: exactly one source, the way `IpPermission` carries
/// one.
#[derive(Debug, Clone)]
enum Source {
    Ipv4(String),
    Ipv6(String),
    PrefixList(String),
    /// Another security group, by id.
    Group(String),
    /// Another security group, by name — the default-VPC spelling of the same source
    /// (`UserIdGroupPair$GroupName`).
    GroupName(String),
}

/// One security group rule, as a row writes it: a protocol, an optional port range and one
/// source. The same type is used to SEED a rule (in the spelling AWS stores) and to NAME
/// one in a request (in whatever spelling the row chooses).
#[derive(Debug, Clone)]
struct Rule {
    protocol: String,
    ports: Option<(i32, i32)>,
    source: Source,
    description: Option<String>,
}

impl Rule {
    fn new(protocol: &str, ports: Option<(i32, i32)>, source: Source) -> Self {
        Self {
            protocol: protocol.to_string(),
            ports,
            source,
            description: None,
        }
    }

    fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// The rule as the stored `IpPermission` that holds it.
    fn permission(&self) -> IpPermission {
        let mut permission = IpPermission {
            ip_protocol: Some(self.protocol.clone()),
            from_port: self.ports.map(|(from, _)| from),
            to_port: self.ports.map(|(_, to)| to),
            user_id_group_pairs: Some(Vec::new()),
            ip_ranges: Some(Vec::new()),
            ipv6_ranges: Some(Vec::new()),
            prefix_list_ids: Some(Vec::new()),
        };
        let description = self.description.clone();
        match &self.source {
            Source::Ipv4(cidr) => {
                permission.ip_ranges = Some(vec![IpRange {
                    description,
                    cidr_ip: Some(cidr.clone()),
                }]);
            }
            Source::Ipv6(cidr) => {
                permission.ipv6_ranges = Some(vec![Ipv6Range {
                    description,
                    cidr_ipv6: Some(cidr.clone()),
                }]);
            }
            Source::PrefixList(prefix_list_id) => {
                permission.prefix_list_ids = Some(vec![PrefixListId {
                    description,
                    prefix_list_id: Some(prefix_list_id.clone()),
                }]);
            }
            Source::Group(group_id) => {
                permission.user_id_group_pairs = Some(vec![UserIdGroupPair {
                    description,
                    group_id: Some(group_id.clone()),
                    ..UserIdGroupPair::default()
                }]);
            }
            Source::GroupName(group_name) => {
                permission.user_id_group_pairs = Some(vec![UserIdGroupPair {
                    description,
                    group_name: Some(group_name.clone()),
                    ..UserIdGroupPair::default()
                }]);
            }
        }
        permission
    }

    /// The rule as the `IpPermissions.1.…` form fields of a request.
    ///
    /// The keys are the ones the model's `@ec2QueryName`s resolve to — note `Groups.N` for
    /// `UserIdGroupPairs`
    /// (`smithy_query(query="shape",shape="IpPermission")`).
    fn permission_params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("IpPermissions.1.IpProtocol", &self.protocol)];
        if let Some((from, to)) = self.ports {
            params.push(param("IpPermissions.1.FromPort", &from.to_string()));
            params.push(param("IpPermissions.1.ToPort", &to.to_string()));
        }
        match &self.source {
            Source::Ipv4(cidr) => params.push(param("IpPermissions.1.IpRanges.1.CidrIp", cidr)),
            Source::Ipv6(cidr) => {
                params.push(param("IpPermissions.1.Ipv6Ranges.1.CidrIpv6", cidr));
            }
            Source::PrefixList(prefix_list_id) => params.push(param(
                "IpPermissions.1.PrefixListIds.1.PrefixListId",
                prefix_list_id,
            )),
            Source::Group(group_id) => {
                params.push(param("IpPermissions.1.Groups.1.GroupId", group_id));
            }
            Source::GroupName(group_name) => {
                params.push(param("IpPermissions.1.Groups.1.GroupName", group_name));
            }
        }
        params
    }

    /// The rule as the FLAT request members — `IpProtocol`, `FromPort`, `ToPort` and either
    /// `CidrIp` or `SourceSecurityGroupName`, the older spelling of the same request.
    fn flat_params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("IpProtocol", &self.protocol)];
        if let Some((from, to)) = self.ports {
            params.push(param("FromPort", &from.to_string()));
            params.push(param("ToPort", &to.to_string()));
        }
        match &self.source {
            Source::Ipv4(cidr) => params.push(param("CidrIp", cidr)),
            Source::GroupName(group_name) => {
                params.push(param("SourceSecurityGroupName", group_name));
            }
            other => panic!("the flat form names only a CIDR block or a source group, not {other:?}"),
        }
        params
    }
}

/// Which spelling of the request a row uses. Both name the same rules, so every row that
/// can be written either way asserts the same outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `IpProtocol` / `FromPort` / `ToPort` / `CidrIp` directly on the request.
    Flat,
    /// `IpPermissions.1.…`.
    Permissions,
}

impl Form {
    fn params(self, rule: &Rule) -> Vec<(String, String)> {
        match self {
            Self::Flat => rule.flat_params(),
            Self::Permissions => rule.permission_params(),
        }
    }
}

/// One stored rule, rendered so a test can compare rule SETS as text: the canonical
/// protocol, the port range and the one source.
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

/// The inbound rules the store holds for `group_id`, rendered and sorted.
fn held_inbound(engine: &engine::InMemoryEngine, group_id: &str) -> Vec<String> {
    let mut held: Vec<String> = stored_rules(engine, group_id)
        .iter()
        .map(|(_, atom)| rendered(atom))
        .collect();
    held.sort();
    held
}

/// The OUTBOUND rules the store holds for `group_id`, rendered and sorted — what an ingress
/// revoke must leave exactly as it was.
fn held_outbound(engine: &engine::InMemoryEngine, group_id: &str) -> Vec<String> {
    let mut held: Vec<String> = engine
        .state::<Ec2State>()
        .lock()
        .held_security_group_rules(group_id, true)
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

/// Write a group holding `rules` as its inbound rules, the way a creating operation plus an
/// authorize would leave it, and answer its `GroupId`.
fn seed_group(
    engine: &engine::InMemoryEngine,
    vpc_id: &str,
    group_name: &str,
    rules: &[Rule],
) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.to_string(),
            group_name: group_name.to_string(),
            description: format!("the {group_name} group"),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: rules.iter().map(Rule::permission).collect(),
            ip_permissions_egress: vec![ec2::state::allow_all_egress()],
        })
}

/// The inbound rules the store holds for `group_id`, each beside the
/// `SecurityGroupRuleId` it is stored under — the rules as the service matches on them.
fn stored_rules(
    engine: &engine::InMemoryEngine,
    group_id: &str,
) -> Vec<(String, SecurityGroupRuleAtom)> {
    engine
        .state::<Ec2State>()
        .lock()
        .held_security_group_rules(group_id, false)
}

/// `sgr-` and 17 lowercase hexadecimal digits, the long EC2 resource id form.
fn is_rule_id(value: &str) -> bool {
    value
        .strip_prefix("sgr-")
        .is_some_and(|digits| digits.len() == 17 && digits.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// One row of the table: the rules the group starts with, which of them the request names,
/// how it spells that rule, and which request form it uses.
struct RevokeCase {
    case: &'static str,
    seeded: Vec<Rule>,
    /// The index into `seeded` of the rule the request names.
    target: usize,
    /// The target rule as the REQUEST spells it — the same rule, spelled the row's way.
    spelled: Rule,
    form: Form,
}

/// The table: one row per way of naming a rule that the rule generalises over.
fn revoke_cases() -> Vec<RevokeCase> {
    vec![
        RevokeCase {
            case: "a single-port tcp rule, by the flat members",
            seeded: vec![
                Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into())),
                Rule::new("tcp", Some((443, 443)), Source::Ipv4("10.1.0.0/16".into())),
            ],
            target: 0,
            spelled: Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into())),
            form: Form::Flat,
        },
        RevokeCase {
            case: "a udp port range, through IpPermissions",
            seeded: vec![
                Rule::new("udp", Some((9092, 9094)), Source::Ipv4("192.168.0.0/24".into())),
                Rule::new("udp", Some((53, 53)), Source::Ipv4("192.168.1.0/24".into())),
            ],
            target: 0,
            spelled: Rule::new("udp", Some((9092, 9094)), Source::Ipv4("192.168.0.0/24".into())),
            form: Form::Permissions,
        },
        RevokeCase {
            case: "an all-protocols rule, which carries no port range",
            seeded: vec![
                Rule::new("-1", None, Source::Ipv4("0.0.0.0/0".into())),
                Rule::new("tcp", Some((80, 80)), Source::Ipv4("10.2.0.0/16".into())),
            ],
            target: 0,
            // "Use -1 to specify all" (`$IpProtocol`): the all-protocols rule of a group,
            // named the one way the model spells it.
            spelled: Rule::new("-1", None, Source::Ipv4("0.0.0.0/0".into())),
            form: Form::Flat,
        },
        RevokeCase {
            case: "tcp spelled in upper case",
            seeded: vec![Rule::new(
                "tcp",
                Some((8080, 8090)),
                Source::Ipv4("203.0.113.0/24".into()),
            )],
            target: 0,
            // "The IP protocol name (tcp, udp, icmp, icmpv6)" (`$IpProtocol`) is spelled in
            // lower case by the model, so a name is the same protocol in any case.
            spelled: Rule::new("TCP", Some((8080, 8090)), Source::Ipv4("203.0.113.0/24".into())),
            form: Form::Flat,
        },
        RevokeCase {
            case: "a protocol named by its IANA number, no port range",
            seeded: vec![Rule::new(
                // "or number (see Protocol Numbers)" (`$IpProtocol`): 47 is GRE, a protocol
                // with no port range and no name of its own in the model.
                "47",
                None,
                Source::Ipv4("198.51.100.0/24".into()),
            )],
            target: 0,
            spelled: Rule::new("47", None, Source::Ipv4("198.51.100.0/24".into())),
            form: Form::Flat,
        },
        RevokeCase {
            case: "icmp with its type and code, both wild",
            seeded: vec![Rule::new(
                "icmp",
                // "If the protocol is ICMP, this is the ICMP type or -1 (all ICMP types)"
                // (`$FromPort`) and the code or -1 (`$ToPort`).
                Some((-1, -1)),
                Source::Ipv4("192.0.2.0/24".into()),
            )],
            target: 0,
            spelled: Rule::new("ICMP", Some((-1, -1)), Source::Ipv4("192.0.2.0/24".into())),
            form: Form::Flat,
        },
        RevokeCase {
            case: "an IPv6 rule, spelled in upper case",
            seeded: vec![
                Rule::new("tcp", Some((443, 443)), Source::Ipv6("2600:1f14::/36".into())),
                Rule::new("tcp", Some((443, 443)), Source::Ipv6("2001:db8::/32".into())),
            ],
            target: 0,
            spelled: Rule::new("tcp", Some((443, 443)), Source::Ipv6("2600:1F14::/36".into())),
            form: Form::Permissions,
        },
        RevokeCase {
            case: "an icmpv6 rule, no port range",
            seeded: vec![Rule::new("icmpv6", None, Source::Ipv6("2001:db8:1::/48".into()))],
            target: 0,
            spelled: Rule::new("icmpv6", None, Source::Ipv6("2001:DB8:1::/48".into())),
            form: Form::Permissions,
        },
        RevokeCase {
            case: "a prefix list rule",
            seeded: vec![
                Rule::new(
                    "tcp",
                    Some((1024, 1024)),
                    Source::PrefixList("pl-0123456789abcdef0".into()),
                ),
                Rule::new(
                    "tcp",
                    Some((1024, 1024)),
                    Source::PrefixList("pl-0abcdef0123456789".into()),
                ),
            ],
            target: 0,
            spelled: Rule::new(
                "tcp",
                Some((1024, 1024)),
                Source::PrefixList("pl-0123456789abcdef0".into()),
            ),
            form: Form::Permissions,
        },
        RevokeCase {
            case: "a rule carrying a description, revoked without naming it",
            seeded: vec![Rule::new("tcp", Some((3306, 3306)), Source::Ipv4("10.5.0.0/16".into()))
                .with_description("the database tier")],
            target: 0,
            spelled: Rule::new("tcp", Some((3306, 3306)), Source::Ipv4("10.5.0.0/16".into())),
            form: Form::Flat,
        },
    ]
}

/// A revoke removes exactly the rules the request names, leaves every other rule of the
/// group where it was, and reports what it removed.
#[tokio::test]
async fn a_revoke_removes_exactly_the_rules_the_request_names() {
    for RevokeCase {
        case,
        seeded,
        target,
        spelled,
        form,
    } in revoke_cases()
    {
        let (router, engine) = super::ec2_query::emulator();
        // A nondefault VPC: every rule this row names is one the group holds, so the strict
        // half of the rule is the one under test here.
        let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
        let group_id = seed_group(&engine, &vpc_id, "fixture", &seeded);

        let before = held_inbound(&engine, &group_id);
        assert_eq!(
            before.len(),
            seeded.len(),
            "case {case:?}: expected the seeded rules to be held, got {before:?}"
        );
        let target_rendered = rendered(&stored_rules(&engine, &group_id)[target].1);

        let mut params = vec![param("GroupId", &group_id)];
        params.extend(form.params(&spelled));
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

        // The one rule it took, reported with an id of its own and the values the STORED
        // rule had — including the description the request did not name.
        let reported_ids = elements(&body, "securityGroupRuleId");
        assert_eq!(
            reported_ids.len(),
            1,
            "case {case:?}: expected exactly one revoked rule, got {reported_ids:?} from {body}"
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
            Some("false"),
            "case {case:?}: an ingress revoke reports inbound rules, got {body}"
        );
        if let Some(description) = seeded[target].description.as_deref() {
            assert_eq!(
                element(&body, "description").as_deref(),
                Some(description),
                "case {case:?}: expected the stored description {description:?}, got {body}"
            );
        }

        // The store now holds every rule but that one, and the reporting half agrees.
        let after = held_inbound(&engine, &group_id);
        let expected: Vec<String> = before
            .iter()
            .filter(|held| **held != target_rendered)
            .cloned()
            .collect();
        assert_eq!(
            after, expected,
            "case {case:?}: expected {expected:?} to be left, got {after:?}"
        );
        let body = described(&router, case, &group_id).await;
        let source_marker = match &seeded[target].source {
            Source::Ipv4(cidr) => format!("<cidrIp>{cidr}</cidrIp>"),
            Source::Ipv6(cidr) => format!("<cidrIpv6>{cidr}</cidrIpv6>"),
            Source::PrefixList(id) => format!("<prefixListId>{id}</prefixListId>"),
            Source::Group(id) | Source::GroupName(id) => format!("<groupId>{id}</groupId>"),
        };
        assert!(
            !inbound_section(&body).contains(&source_marker),
            "case {case:?}: expected {source_marker} to be gone from the group, got {body}"
        );
    }
}

/// A stored permission that aggregates several sources is ONE rule per source: revoking one
/// of them leaves the others, and the permission goes only when its last source does.
#[tokio::test]
async fn one_source_of_an_aggregated_permission_is_one_rule() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
    // One permission, three sources: two IPv4 blocks and one IPv6 block.
    let aggregated = IpPermission {
        ip_protocol: Some("tcp".to_string()),
        from_port: Some(22),
        to_port: Some(22),
        user_id_group_pairs: Some(Vec::new()),
        ip_ranges: Some(vec![
            IpRange {
                description: None,
                cidr_ip: Some("10.2.0.0/16".to_string()),
            },
            IpRange {
                description: None,
                cidr_ip: Some("10.3.0.0/16".to_string()),
            },
        ]),
        ipv6_ranges: Some(vec![Ipv6Range {
            description: None,
            cidr_ipv6: Some("2001:db8::/32".to_string()),
        }]),
        prefix_list_ids: Some(Vec::new()),
    };
    let group_id = engine
        .state::<Ec2State>()
        .lock()
        .insert_security_group(SecurityGroupSpec {
            vpc_id,
            group_name: "aggregated".to_string(),
            description: "one permission, three sources".to_string(),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: vec![aggregated],
            ip_permissions_egress: vec![ec2::state::allow_all_egress()],
        });

    assert_eq!(
        held_inbound(&engine, &group_id),
        vec![
            "tcp 22-22 ipv4 10.2.0.0/16".to_string(),
            "tcp 22-22 ipv4 10.3.0.0/16".to_string(),
            "tcp 22-22 ipv6 2001:db8::/32".to_string(),
        ],
        "case \"the seed\": expected one rule per source"
    );

    // One of the three.
    let (status, body) = revoke(
        &router,
        &[
            param("GroupId", &group_id),
            param("IpProtocol", "tcp"),
            param("FromPort", "22"),
            param("ToPort", "22"),
            param("CidrIp", "10.2.0.0/16"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"one of three sources\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        held_inbound(&engine, &group_id),
        vec![
            "tcp 22-22 ipv4 10.3.0.0/16".to_string(),
            "tcp 22-22 ipv6 2001:db8::/32".to_string(),
        ],
        "case \"one of three sources\": expected the other two sources to be left"
    );

    // The last two, in one call: the permission itself goes with its last source rather
    // than being left behind as a rule with no source.
    let mut params = vec![param("GroupId", &group_id)];
    params.extend([
        param("IpPermissions.1.IpProtocol", "tcp"),
        param("IpPermissions.1.FromPort", "22"),
        param("IpPermissions.1.ToPort", "22"),
        param("IpPermissions.1.IpRanges.1.CidrIp", "10.3.0.0/16"),
        param("IpPermissions.1.Ipv6Ranges.1.CidrIpv6", "2001:db8::/32"),
    ]);
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"the last two sources\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "securityGroupRuleId").len(),
        2,
        "case \"the last two sources\": expected two revoked rules, got {body}"
    );
    assert!(
        held_inbound(&engine, &group_id).is_empty(),
        "case \"the last two sources\": expected no inbound rule left, got {:?}",
        held_inbound(&engine, &group_id)
    );
    let described = described(&router, "the last two sources", &group_id).await;
    let inbound = inbound_section(&described);
    assert!(
        !inbound.contains("<cidrIp>10.3.0.0/16</cidrIp>")
            && !inbound.contains("<cidrIpv6>2001:db8::/32</cidrIpv6>"),
        "case \"the last two sources\": expected both sources gone, got {described}"
    );
}

/// A rule that names another security group, by id and — in the default VPC — by name.
#[tokio::test]
async fn a_rule_that_references_another_group_is_revoked_by_id_or_by_name() {
    let (router, engine) = super::ec2_query::emulator();
    let default_vpc = default_vpc_id(&engine);
    let source_group = seed_group(&engine, &default_vpc, "source", &[]);
    let holder = seed_group(
        &engine,
        &default_vpc,
        "holder",
        &[
            Rule::new("tcp", Some((5432, 5432)), Source::Group(source_group.clone())),
            Rule::new("tcp", Some((6379, 6379)), Source::Group(source_group.clone())),
        ],
    );

    // By id, through IpPermissions.
    let mut params = vec![param("GroupId", &holder)];
    params.extend(
        Rule::new("tcp", Some((5432, 5432)), Source::Group(source_group.clone())).permission_params(),
    );
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"by referenced group id\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "referencedGroupId").as_deref(),
        Some(source_group.as_str()),
        "case \"by referenced group id\": expected the revoked rule to name the referenced \
         group, got {body}"
    );
    assert_eq!(
        held_inbound(&engine, &holder),
        vec![format!("tcp 6379-6379 group {source_group}")],
        "case \"by referenced group id\": expected only the other rule to be left"
    );

    // By NAME, through the flat members: the same group, in the default VPC's spelling.
    let mut params = vec![param("GroupId", &holder)];
    params.extend(
        Rule::new(
            "tcp",
            Some((6379, 6379)),
            Source::GroupName("source".into()),
        )
        .flat_params(),
    );
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"by source group name\": expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        held_inbound(&engine, &holder).is_empty(),
        "case \"by source group name\": expected no inbound rule left, got {:?}",
        held_inbound(&engine, &holder)
    );
}

/// One row of the mismatch table: how the request gets the rule wrong.
struct MismatchCase {
    case: &'static str,
    /// The rule the request names, which the group does NOT hold.
    named: Rule,
}

/// Every member of a rule is part of its identity, so getting any one of them wrong names a
/// rule the group does not hold.
fn mismatch_cases() -> Vec<MismatchCase> {
    vec![
        MismatchCase {
            case: "the wrong protocol",
            named: Rule::new("udp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into())),
        },
        MismatchCase {
            case: "the wrong start port",
            named: Rule::new("tcp", Some((23, 22)), Source::Ipv4("10.0.0.0/24".into())),
        },
        MismatchCase {
            case: "the wrong end port",
            named: Rule::new("tcp", Some((22, 23)), Source::Ipv4("10.0.0.0/24".into())),
        },
        MismatchCase {
            case: "no port range where the rule has one",
            named: Rule::new("tcp", None, Source::Ipv4("10.0.0.0/24".into())),
        },
        MismatchCase {
            case: "the wrong CIDR block",
            named: Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.9.0.0/24".into())),
        },
        MismatchCase {
            case: "the right values but the wrong kind of source",
            named: Rule::new("tcp", Some((22, 22)), Source::Ipv6("2001:db8::/32".into())),
        },
        MismatchCase {
            case: "a prefix list the group's rules never named",
            named: Rule::new(
                "tcp",
                Some((22, 22)),
                Source::PrefixList("pl-0999999999999999f".into()),
            ),
        },
    ]
}

/// The one rule every group in the mismatch table holds.
fn mismatch_seed() -> Vec<Rule> {
    vec![Rule::new(
        "tcp",
        Some((22, 22)),
        Source::Ipv4("10.0.0.0/24".into()),
    )]
}

/// In a NONDEFAULT VPC a rule the group does not hold is refused with
/// `InvalidPermission.NotFound`, and nothing is revoked.
#[tokio::test]
async fn a_rule_the_group_does_not_hold_is_refused_in_a_nondefault_vpc() {
    for MismatchCase { case, named } in mismatch_cases() {
        let (router, engine) = super::ec2_query::emulator();
        let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
        let group_id = seed_group(&engine, &vpc_id, "fixture", &mismatch_seed());
        let before = held_inbound(&engine, &group_id);

        let mut params = vec![param("GroupId", &group_id)];
        params.extend(named.permission_params());
        let (status, body) = revoke(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPermission.NotFound",
            "case {case:?}: expected InvalidPermission.NotFound, got {body}"
        );
        assert_eq!(
            held_inbound(&engine, &group_id),
            before,
            "case {case:?}: a refused revoke must revoke nothing"
        );
    }
}

/// In the DEFAULT VPC the same mismatch is no error: the rules that did match are revoked
/// and the ones that did not are reported under `unknownIpPermissionSet`.
#[tokio::test]
async fn a_rule_the_group_does_not_hold_is_reported_in_the_default_vpc() {
    for MismatchCase { case, named } in mismatch_cases() {
        let (router, engine) = super::ec2_query::emulator();
        let default_vpc = default_vpc_id(&engine);
        let group_id = seed_group(&engine, &default_vpc, "fixture", &mismatch_seed());

        // Two rules in one request: the one the group holds, and the row's mismatch.
        let held = Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into()));
        let mut params = vec![param("GroupId", &group_id)];
        params.extend(held.permission_params());
        params.extend(
            named
                .permission_params()
                .iter()
                .map(|(key, value)| param(&key.replace("IpPermissions.1.", "IpPermissions.2."), value)),
        );
        let (status, body) = revoke(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the default VPC returns no error, got {status} with body {body}"
        );
        assert!(
            body.contains("<unknownIpPermissionSet>"),
            "case {case:?}: expected the unrevoked rule to be reported, got {body}"
        );
        assert_eq!(
            elements(&body, "securityGroupRuleId").len(),
            1,
            "case {case:?}: expected the rule that DID match to be revoked, got {body}"
        );
        assert!(
            held_inbound(&engine, &group_id).is_empty(),
            "case {case:?}: expected the matching rule to be gone, got {:?}",
            held_inbound(&engine, &group_id)
        );
    }
}

/// A rule id names one rule of one group: the rule it names is revoked, an id no rule of
/// the group answers to is refused, and a value that is not a rule id at all is refused as
/// malformed.
#[tokio::test]
async fn a_revoke_by_rule_id_removes_the_rule_that_id_names() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
    let seeded = vec![
        Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into())),
        Rule::new("udp", Some((53, 53)), Source::Ipv4("10.1.0.0/16".into())),
        Rule::new("tcp", Some((443, 443)), Source::Ipv6("2001:db8::/32".into())),
    ];
    let group_id = seed_group(&engine, &vpc_id, "fixture", &seeded);
    let other_group = seed_group(
        &engine,
        &vpc_id,
        "other",
        &[Rule::new(
            "tcp",
            Some((22, 22)),
            Source::Ipv4("10.0.0.0/24".into()),
        )],
    );

    let rules = stored_rules(&engine, &group_id);
    let ids: Vec<String> = rules.iter().map(|(rule_id, _)| rule_id.clone()).collect();
    assert!(
        ids.iter().all(|id| is_rule_id(id)),
        "case \"the seeded ids\": expected sgr- ids, got {ids:?}"
    );
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "case \"the seeded ids\": a group's rules have distinct ids, got {ids:?}"
    );

    // Each id names exactly its own rule: revoke the second and the third, keep the first.
    let (status, body) = revoke(
        &router,
        &[
            param("GroupId", &group_id),
            param("SecurityGroupRuleId.1", &ids[1]),
            param("SecurityGroupRuleId.2", &ids[2]),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"two rule ids\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "securityGroupRuleId");
    assert_eq!(
        reported,
        vec![ids[1].clone(), ids[2].clone()],
        "case \"two rule ids\": expected the ids the request named, got {body}"
    );
    assert_eq!(
        held_inbound(&engine, &group_id),
        vec![rendered(&rules[0].1)],
        "case \"two rule ids\": expected only the rule the request did not name to be left"
    );

    // The id of ANOTHER group's rule is not a rule of this group, even though the two rules
    // have the same protocol, ports and source.
    let other_rules = stored_rules(&engine, &other_group);
    let other_id = other_rules[0].0.clone();
    assert_ne!(
        other_id, ids[0],
        "case \"another group's rule\": one rule of two groups has two ids"
    );
    for (case, rule_id, expected_code) in [
        (
            "another group's rule id",
            other_id.as_str(),
            "InvalidSecurityGroupRuleId.NotFound",
        ),
        (
            "an id already revoked",
            ids[1].as_str(),
            "InvalidSecurityGroupRuleId.NotFound",
        ),
        (
            "a well-formed id nothing was minted under",
            "sgr-0000000000000dead",
            "InvalidSecurityGroupRuleId.NotFound",
        ),
        (
            "a security group id in the rule id's place",
            other_group.as_str(),
            "InvalidSecurityGroupRuleId.Malformed",
        ),
        (
            "not an id at all",
            "the-first-rule",
            "InvalidSecurityGroupRuleId.Malformed",
        ),
    ] {
        let before = held_inbound(&engine, &group_id);
        let (status, body) = revoke(
            &router,
            &[param("GroupId", &group_id), param("SecurityGroupRuleId.1", rule_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            held_inbound(&engine, &group_id),
            before,
            "case {case:?}: a refused revoke must revoke nothing"
        );
    }
}

/// One row of the refusal table: the case name, the whole request it sends, and the HTTP
/// status and EC2 error code the refusal must carry.
type RefusalCase = (&'static str, Vec<(String, String)>, u16, &'static str);

/// What a revoke refuses, and what a dry run of one does: each row is a request that cannot
/// be served, and none of them may leave the store changed.
#[tokio::test]
async fn a_revoke_refuses_what_it_cannot_serve_and_a_dry_run_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
    let seeded = vec![Rule::new(
        "tcp",
        Some((22, 22)),
        Source::Ipv4("10.0.0.0/24".into()),
    )];
    let group_id = seed_group(&engine, &vpc_id, "fixture", &seeded);
    let held_params = Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into())).flat_params();

    let mut rows: Vec<RefusalCase> = vec![
        (
            "no group at all",
            held_params.clone(),
            400,
            "MissingParameter",
        ),
        (
            "a group id this account holds no group for",
            {
                let mut params = vec![param("GroupId", "sg-00000000000000099")];
                params.extend(held_params.clone());
                params
            },
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a group name no group of the default VPC answers to",
            {
                let mut params = vec![param("GroupName", "no-such-group")];
                params.extend(held_params.clone());
                params
            },
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "an id and a name that disagree",
            {
                let mut params =
                    vec![param("GroupId", &group_id), param("GroupName", "not-the-fixture")];
                params.extend(held_params.clone());
                params
            },
            400,
            "InvalidParameterCombination",
        ),
        (
            "a group but no rule",
            vec![param("GroupId", &group_id)],
            400,
            "MissingParameter",
        ),
        (
            "a rule with no source",
            vec![param("GroupId", &group_id), param("IpProtocol", "tcp")],
            400,
            "MissingParameter",
        ),
        (
            "a CIDR block and a source group together",
            {
                let mut params = vec![param("GroupId", &group_id)];
                params.extend(held_params.clone());
                params.push(param("SourceSecurityGroupName", "fixture"));
                params
            },
            400,
            "InvalidParameterCombination",
        ),
        (
            "rule ids and rule properties together",
            {
                let mut params = vec![
                    param("GroupId", &group_id),
                    param("SecurityGroupRuleId.1", "sgr-0000000000000dead"),
                ];
                params.extend(held_params.clone());
                params
            },
            400,
            "InvalidParameterCombination",
        ),
        (
            // `SourceSecurityGroupOwnerId` is a rule property like any other, so it clashes
            // with rule ids the same way the rest of the flat members do.
            "rule ids and a source group owner together",
            vec![
                param("GroupId", &group_id),
                param("SecurityGroupRuleId.1", "sgr-0000000000000dead"),
                param("SourceSecurityGroupOwnerId", "123456789012"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            // The two spellings of rule properties name two DIFFERENT rules; serving one of
            // them would answer 200 to a request whose other rule was never looked at.
            "IpPermissions and the flat members together",
            {
                let mut params = vec![
                    param("GroupId", &group_id),
                    param("IpPermissions.1.IpProtocol", "tcp"),
                    param("IpPermissions.1.FromPort", "22"),
                    param("IpPermissions.1.ToPort", "22"),
                    param("IpPermissions.1.IpRanges.1.CidrIp", "10.0.0.0/24"),
                ];
                params.extend(
                    Rule::new("udp", Some((53, 53)), Source::Ipv4("10.9.0.0/16".into()))
                        .flat_params(),
                );
                params
            },
            400,
            "InvalidParameterCombination",
        ),
        (
            // The protocol is part of a rule's identity, so a permission naming none names
            // no rule — and must not be read as the all-protocols rule the group holds.
            "a permission that names no protocol",
            vec![
                param("GroupId", &group_id),
                param("IpPermissions.1.IpRanges.1.CidrIp", "10.0.0.0/24"),
            ],
            400,
            "MissingParameter",
        ),
        (
            // "Not an id of this kind" is asked before "no such rule", for the whole list.
            "a malformed rule id after a well-formed unknown one",
            vec![
                param("GroupId", &group_id),
                param("SecurityGroupRuleId.1", "sgr-0000000000000dead"),
                param("SecurityGroupRuleId.2", "not-a-rule-id"),
            ],
            400,
            "InvalidSecurityGroupRuleId.Malformed",
        ),
    ];
    // The caller-sized lists are capped at the documented maximum this crate holds every
    // caller-sized list to (`state::MAX_RESOURCE_IDS`): one row per list, each just past
    // the cap, plus one AT the cap to pin that the bound admits the maximum itself and the
    // request is then answered on its merits rather than refused for its size.
    let cap = ec2::state::MAX_RESOURCE_IDS;
    let bogus_rule_id = |index: usize| format!("sgr-{index:017x}");
    rows.push((
        "one rule id more than a request may name",
        {
            let mut params = vec![param("GroupId", &group_id)];
            params.extend((0..=cap).map(|index| {
                (
                    format!("SecurityGroupRuleId.{}", index + 1),
                    bogus_rule_id(index),
                )
            }));
            params
        },
        400,
        "InvalidParameterValue",
    ));
    rows.push((
        "a rule id list at the cap, which is answered on its merits",
        {
            let mut params = vec![param("GroupId", &group_id)];
            params.extend((0..cap).map(|index| {
                (
                    format!("SecurityGroupRuleId.{}", index + 1),
                    bogus_rule_id(index),
                )
            }));
            params
        },
        400,
        "InvalidSecurityGroupRuleId.NotFound",
    ));
    rows.push((
        "one permission more than a request may name",
        {
            let mut params = vec![param("GroupId", &group_id)];
            for index in 0..=cap {
                let position = index + 1;
                params.push(param(
                    &format!("IpPermissions.{position}.IpProtocol"),
                    "tcp",
                ));
                params.push(param(
                    &format!("IpPermissions.{position}.IpRanges.1.CidrIp"),
                    "10.0.0.0/24",
                ));
            }
            params
        },
        400,
        "InvalidParameterValue",
    ));
    rows.push((
        "one permission naming more sources than a request may name",
        {
            let mut params = vec![
                param("GroupId", &group_id),
                param("IpPermissions.1.IpProtocol", "tcp"),
            ];
            params.extend((0..=cap).map(|index| {
                (
                    format!("IpPermissions.1.IpRanges.{}.CidrIp", index + 1),
                    format!("10.{}.{}.0/24", index / 256, index % 256),
                )
            }));
            params
        },
        400,
        "InvalidParameterValue",
    ));
    // A dry run of a request that WOULD have succeeded: 412 `DryRunOperation`, and the rule
    // is still there afterwards.
    rows.push((
        "a dry run of a revoke that would have succeeded",
        {
            let mut params = vec![param("GroupId", &group_id), param("DryRun", "true")];
            params.extend(held_params.clone());
            params
        },
        412,
        "DryRunOperation",
    ));

    for (case, params, expected_status, expected_code) in rows {
        let before = held_inbound(&engine, &group_id);
        let (status, body) = revoke(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            held_inbound(&engine, &group_id),
            before,
            "case {case:?}: a request that was refused must leave the store alone"
        );
    }

    // And the same request without `DryRun` does the work the dry run reported it could.
    let mut params = vec![param("GroupId", &group_id)];
    params.extend(held_params);
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"the dry run's request for real\": expected HTTP 200, got {status} with \
         body {body}"
    );
    assert!(
        held_inbound(&engine, &group_id).is_empty(),
        "case \"the dry run's request for real\": expected the rule to be gone, got {:?}",
        held_inbound(&engine, &group_id)
    );
}

/// A revoke touches the INBOUND rules only: the group's outbound rules, and every other
/// group, are left exactly as they were.
#[tokio::test]
async fn a_revoke_leaves_the_outbound_rules_and_the_other_groups_alone() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = insert_vpc(&engine, "10.0.0.0/16");
    let shared = Rule::new("tcp", Some((22, 22)), Source::Ipv4("10.0.0.0/24".into()));
    let group_id = seed_group(&engine, &vpc_id, "fixture", std::slice::from_ref(&shared));
    let neighbour = seed_group(&engine, &vpc_id, "neighbour", std::slice::from_ref(&shared));
    // The same rule outbound as well, so a revoke that read the wrong list would be caught.
    engine
        .state::<Ec2State>()
        .lock()
        .insert_security_group_rules(&group_id, true, &shared.permission(), &[], true, REGION);

    let outbound_before = held_outbound(&engine, &group_id);
    assert!(
        outbound_before.contains(&"tcp 22-22 ipv4 10.0.0.0/24".to_string()),
        "case \"the seed\": expected the outbound rule to be held, got {outbound_before:?}"
    );

    let mut params = vec![param("GroupId", &group_id)];
    params.extend(shared.flat_params());
    let (status, body) = revoke(&router, &params).await;
    assert_eq!(
        status, 200,
        "case \"inbound only\": expected HTTP 200, got {status} with body {body}"
    );

    assert!(
        held_inbound(&engine, &group_id).is_empty(),
        "case \"inbound only\": expected the inbound rule to be gone"
    );
    let outbound_after = held_outbound(&engine, &group_id);
    assert_eq!(
        outbound_after, outbound_before,
        "case \"inbound only\": an ingress revoke must leave the outbound rules alone"
    );
    assert_eq!(
        held_inbound(&engine, &neighbour),
        vec!["tcp 22-22 ipv4 10.0.0.0/24".to_string()],
        "case \"inbound only\": another group's identical rule must be left alone"
    );
    let described = described(&router, "inbound only", &neighbour).await;
    assert!(
        inbound_section(&described).contains("<cidrIp>10.0.0.0/24</cidrIp>"),
        "case \"inbound only\": expected the neighbour to still report its rule, got \
         {described}"
    );
}

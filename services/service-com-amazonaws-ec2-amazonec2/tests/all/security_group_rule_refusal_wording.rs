// SPDX-License-Identifier: CC0-1.0

//! How `AuthorizeSecurityGroupIngress` and `AuthorizeSecurityGroupEgress` WORD the two
//! refusals a request describing a rule earns, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! Two rules are under test, and both are about the rule the caller has to fix rather than
//! about anything the service minted:
//!
//! 1. A rule the group would hold twice is refused `InvalidPermission.Duplicate`, and the
//!    refusal names the rule by what a rule IS — "a protocol, from and to ports, and source
//!    (CIDR range, security group, or prefix list)" (`RevokeSecurityGroupIngress`) with the
//!    action every security group rule carries: `the specified rule "peer: <peer>,
//!    <protocol>[, <port range>], ALLOW" already exists`. The same sentence whether the rule
//!    is one the group already held or one the SAME request described twice, because it is
//!    the same refusal, and whether or not the rule was ever stored — so no rule id appears
//!    in it.
//! 2. An address range that is not a CIDR block is refused `InvalidParameterValue`, `CIDR
//!    block <value> is malformed`, naming the block as the REQUEST wrote it.
//!
//! The rows differ along what those generalise over: the DIRECTION the rule is authorized in,
//! the protocol (a name, a name whose port range is an ICMP type and code, a protocol number,
//! and every protocol at once), whether the rule carries a port range, which kind of peer
//! names the source (an IPv4 range, an IPv6 range, a prefix list, another group by id and by
//! name), the spelling of a peer whose canonical form differs from the requested one, and —
//! for the malformed block — which way the value fails to be a CIDR block, of either address
//! family, in the `IpPermissions` form and in the flat `CidrIp` form, including a value far
//! longer than any block, whose echo is bounded.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, emulator, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The two operations that authorize a rule: "adds the specified inbound (ingress) rules" and
/// "outbound (egress) rules to a security group".
const INGRESS: &str = "AuthorizeSecurityGroupIngress";
const EGRESS: &str = "AuthorizeSecurityGroupEgress";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// A security group of the account's default VPC, by its minted id.
async fn create_group(case: &str, router: &MultiServiceRouter, group_name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "the group whose refusals are read"),
        ],
    )
    .await;
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// The ids of the rules the store holds for `group_id`, on both sides, as
/// `DescribeSecurityGroupRules` reports them — what a refusal must leave untouched.
async fn stored_rule_ids(case: &str, router: &MultiServiceRouter, group_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeSecurityGroupRules",
        &[
            param("Filter.1.Name", "group-id"),
            param("Filter.1.Value.1", group_id),
        ],
    )
    .await;
    let mut ids = elements(&body, "securityGroupRuleId");
    ids.sort();
    ids
}

/// What a rule's one source names — "you must specify exactly one of the following sources:
/// an IPv4 or IPv6 address range, a prefix list, or a security group"
/// (`AuthorizeSecurityGroupIngress`), where a group may be named by id or by name.
#[derive(Debug, Clone)]
enum Peer {
    Ipv4(String),
    Ipv6(String),
    PrefixList(String),
    GroupById(String),
    GroupByName(String),
}

impl Peer {
    /// The form fields naming this source inside the permission at `base`.
    fn params(&self, base: &str) -> Vec<Param> {
        let (list, member, value) = match self {
            Self::Ipv4(cidr) => ("IpRanges", "CidrIp", cidr),
            Self::Ipv6(cidr) => ("Ipv6Ranges", "CidrIpv6", cidr),
            Self::PrefixList(id) => ("PrefixListIds", "PrefixListId", id),
            Self::GroupById(id) => ("Groups", "GroupId", id),
            Self::GroupByName(name) => ("Groups", "GroupName", name),
        };
        vec![param(&format!("{base}.{list}.1.{member}"), value)]
    }
}

/// One rule a request describes: the protocol, the port range it carries (if any), and its
/// one source.
#[derive(Debug, Clone)]
struct Rule {
    protocol: String,
    ports: Option<(i32, i32)>,
    peer: Peer,
}

impl Rule {
    fn new(protocol: &str, ports: Option<(i32, i32)>, peer: Peer) -> Self {
        Self {
            protocol: protocol.to_string(),
            ports,
            peer,
        }
    }

    /// The form fields for this rule as the permission at `position` (counting from 1).
    fn params(&self, position: usize) -> Vec<Param> {
        let base = format!("IpPermissions.{position}");
        let mut params = vec![param(&format!("{base}.IpProtocol"), &self.protocol)];
        if let Some((from_port, to_port)) = self.ports {
            params.push(param(&format!("{base}.FromPort"), &from_port.to_string()));
            params.push(param(&format!("{base}.ToPort"), &to_port.to_string()));
        }
        params.extend(self.peer.params(&base));
        params
    }
}

/// `GroupId=<group_id>` followed by the permissions `rules` describe.
fn request_params(group_id: &str, rules: &[Rule]) -> Vec<Param> {
    let mut params = vec![param("GroupId", group_id)];
    for (index, rule) in rules.iter().enumerate() {
        params.extend(rule.params(index + 1));
    }
    params
}

/// The whole sentence a duplicate-rule refusal must carry, for a rule described as
/// `descriptor`.
fn duplicate_message(descriptor: &str) -> String {
    format!("the specified rule \"{descriptor}\" already exists")
}

/// Assert that `body` is the duplicate refusal for the rule described as `descriptor`.
fn assert_duplicate(case: &str, status: u16, body: &str, descriptor: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(body);
    assert_eq!(
        code,
        ec2::state::INVALID_PERMISSION_DUPLICATE,
        "case {case:?}: expected InvalidPermission.Duplicate, got {code:?} in {body}"
    );
    let expected = duplicate_message(descriptor);
    let message = error_message(body);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// One row of the duplicate table: the rule, the direction it is authorized in, and the
/// sentence the second attempt at it must answer.
#[derive(Debug, Clone)]
struct DuplicateRow {
    case: &'static str,
    action: &'static str,
    rule: Rule,
    descriptor: String,
}

/// A rule the group already holds is refused `InvalidPermission.Duplicate`, named by its
/// peer, protocol, port range and action rather than by the id the store minted for it — and
/// the group keeps exactly the rules it had.
#[tokio::test]
async fn a_rule_the_group_already_holds_is_refused_naming_that_rule() {
    let (router, _engine) = emulator();

    // The two sources that are resources rather than literals: a prefix list the account can
    // see, and another group of the account, which a rule may name by id or by name.
    let prefix_list_id = ec2::state::service_prefix_lists_in_region(REGION)
        .first()
        .map(|published| published.prefix_list_id.clone())
        .expect("every Region publishes a prefix list");
    let peer_group_id = create_group("the referenced group", &router, "referenced-peer").await;

    let rows: Vec<DuplicateRow> = vec![
        DuplicateRow {
            case: "a TCP port range from an IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new(
                "tcp",
                Some((2222, 2222)),
                Peer::Ipv4("203.0.113.0/24".to_string()),
            ),
            descriptor: "peer: 203.0.113.0/24, TCP, from port: 2222, to port: 2222, ALLOW"
                .to_string(),
        },
        DuplicateRow {
            case: "a UDP port range from an IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new("udp", Some((53, 53)), Peer::Ipv4("10.0.0.0/8".to_string())),
            descriptor: "peer: 10.0.0.0/8, UDP, from port: 53, to port: 53, ALLOW".to_string(),
        },
        DuplicateRow {
            // "If the protocol is ICMP, this is the ICMP type" (`IpPermission$FromPort`) and
            // "the ICMP code" (`$ToPort`), so the range is described as those.
            case: "an ICMP type and code from an IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new("icmp", Some((8, -1)), Peer::Ipv4("0.0.0.0/0".to_string())),
            descriptor: "peer: 0.0.0.0/0, ICMP, type: 8, code: -1, ALLOW".to_string(),
        },
        DuplicateRow {
            case: "an ICMPv6 type and code from an IPv6 range, inbound",
            action: INGRESS,
            rule: Rule::new(
                "icmpv6",
                Some((128, 0)),
                Peer::Ipv6("2001:db8::/32".to_string()),
            ),
            descriptor: "peer: 2001:db8::/32, ICMPV6, type: 128, code: 0, ALLOW".to_string(),
        },
        DuplicateRow {
            // "To specify all protocols, use -1" (`$IpProtocol`), which carries no port
            // range at all.
            case: "every protocol from an IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new("-1", None, Peer::Ipv4("192.0.2.0/24".to_string())),
            descriptor: "peer: 192.0.2.0/24, ALL, ALLOW".to_string(),
        },
        DuplicateRow {
            // "or number (see Protocol Numbers)" (`$IpProtocol`): a protocol number has one
            // spelling, which is the number itself.
            case: "a protocol number from an IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new("47", None, Peer::Ipv4("10.1.0.0/16".to_string())),
            descriptor: "peer: 10.1.0.0/16, 47, ALLOW".to_string(),
        },
        DuplicateRow {
            // "Amazon Web Services canonicalizes IPv4 and IPv6 CIDRs … if you attempt to add
            // another rule with the non-canonical form of the CIDR … the API throws an
            // duplicate rule error" (`$CidrIp`): the rule is the canonical one, so that is
            // the peer the refusal names.
            case: "a non-canonical IPv4 range, inbound",
            action: INGRESS,
            rule: Rule::new(
                "tcp",
                Some((8080, 8080)),
                Peer::Ipv4("100.68.0.18/18".to_string()),
            ),
            descriptor: "peer: 100.68.0.0/18, TCP, from port: 8080, to port: 8080, ALLOW"
                .to_string(),
        },
        DuplicateRow {
            case: "a prefix list, inbound",
            action: INGRESS,
            rule: Rule::new(
                "tcp",
                Some((80, 80)),
                Peer::PrefixList(prefix_list_id.clone()),
            ),
            descriptor: format!(
                "peer: {prefix_list_id}, TCP, from port: 80, to port: 80, ALLOW"
            ),
        },
        DuplicateRow {
            case: "an IPv6 range, outbound",
            action: EGRESS,
            rule: Rule::new(
                "tcp",
                Some((443, 443)),
                Peer::Ipv6("2001:db8:1::/48".to_string()),
            ),
            descriptor: "peer: 2001:db8:1::/48, TCP, from port: 443, to port: 443, ALLOW"
                .to_string(),
        },
        DuplicateRow {
            case: "another group by id, outbound",
            action: EGRESS,
            rule: Rule::new(
                "tcp",
                Some((22, 22)),
                Peer::GroupById(peer_group_id.clone()),
            ),
            descriptor: format!("peer: {peer_group_id}, TCP, from port: 22, to port: 22, ALLOW"),
        },
        DuplicateRow {
            // "[Default VPC] The name of the security group" (`UserIdGroupPair$GroupName`) is
            // the other way of naming ONE group, and a rule references a group by id — so the
            // refusal names the id the name resolved to.
            case: "another group by name, inbound",
            action: INGRESS,
            rule: Rule::new(
                "udp",
                Some((123, 123)),
                Peer::GroupByName("referenced-peer".to_string()),
            ),
            descriptor: format!("peer: {peer_group_id}, UDP, from port: 123, to port: 123, ALLOW"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One group per row, so the only rule the group holds on that side is this row's.
        let group_id = create_group(case, &router, &format!("duplicate-{index}")).await;
        let params = request_params(&group_id, std::slice::from_ref(&row.rule));

        let first = ok(case, &router, row.action, &params).await;
        assert_eq!(
            element(&first, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected <return>true</return> from {}, got {first}",
            row.action
        );
        let stored = stored_rule_ids(case, &router, &group_id).await;

        let (status, body) = call(&router, row.action, &params).await;
        assert_duplicate(case, status, &body, &row.descriptor);
        // The id the store minted appears nowhere in the refusal: the caller named the rule
        // by its members, so that is what it is told about.
        for rule_id in &stored {
            assert!(
                !body.contains(rule_id.as_str()),
                "case {case:?}: expected no rule id in the refusal, got {rule_id:?} in {body}"
            );
        }
        let after = stored_rule_ids(case, &router, &group_id).await;
        assert_eq!(
            after, stored,
            "case {case:?}: expected the refusal to change nothing, got {after:?} where the \
             group held {stored:?}"
        );
    }
}

/// One row of the same-request table: the rules ONE call describes, and the rule the refusal
/// must name.
#[derive(Debug, Clone)]
struct RepeatRow {
    case: &'static str,
    action: &'static str,
    rules: Vec<Rule>,
    descriptor: &'static str,
}

/// One call that describes the same rule twice earns the same refusal, worded for the rule
/// rather than for the position it was described at — and stores nothing.
#[tokio::test]
async fn one_call_describing_the_same_rule_twice_is_refused_naming_that_rule() {
    let (router, _engine) = emulator();

    let rows: [RepeatRow; 3] = [
        RepeatRow {
            case: "two permissions describing one rule, inbound",
            action: INGRESS,
            rules: vec![
                Rule::new(
                    "tcp",
                    Some((443, 443)),
                    Peer::Ipv4("198.51.100.0/24".to_string()),
                ),
                Rule::new(
                    "tcp",
                    Some((443, 443)),
                    Peer::Ipv4("198.51.100.0/24".to_string()),
                ),
            ],
            descriptor: "peer: 198.51.100.0/24, TCP, from port: 443, to port: 443, ALLOW",
        },
        RepeatRow {
            // The second spelling of one block is the same rule: "Amazon Web Services
            // canonicalizes IPv4 and IPv6 CIDRs" (`$CidrIp`).
            case: "the same block spelled two ways, inbound",
            action: INGRESS,
            rules: vec![
                Rule::new(
                    "tcp",
                    Some((25, 25)),
                    Peer::Ipv4("100.68.0.0/18".to_string()),
                ),
                Rule::new(
                    "tcp",
                    Some((25, 25)),
                    Peer::Ipv4("100.68.0.18/18".to_string()),
                ),
            ],
            descriptor: "peer: 100.68.0.0/18, TCP, from port: 25, to port: 25, ALLOW",
        },
        RepeatRow {
            case: "the same protocol spelled two ways, outbound",
            action: EGRESS,
            rules: vec![
                Rule::new("UDP", Some((514, 514)), Peer::Ipv4("10.2.0.0/16".to_string())),
                Rule::new("udp", Some((514, 514)), Peer::Ipv4("10.2.0.0/16".to_string())),
            ],
            descriptor: "peer: 10.2.0.0/16, UDP, from port: 514, to port: 514, ALLOW",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group_id = create_group(case, &router, &format!("repeated-{index}")).await;
        let before = stored_rule_ids(case, &router, &group_id).await;

        let params = request_params(&group_id, &row.rules);
        let (status, body) = call(&router, row.action, &params).await;
        assert_duplicate(case, status, &body, row.descriptor);

        let after = stored_rule_ids(case, &router, &group_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected nothing stored, got {after:?} where the group held \
             {before:?}"
        );
    }
}

/// One row of the malformed-block table: the block the request carries, and how it is carried.
#[derive(Debug, Clone, Copy)]
struct MalformedRow {
    case: &'static str,
    action: &'static str,
    /// How the request names the range: an `IpPermissions` entry of either family, or the
    /// flat `CidrIp` member, which "to specify multiple rules … use IP permissions instead"
    /// (`$CidrIp`) is the one-rule spelling of.
    form: Form,
    value: &'static str,
}

/// Which member of the request carries the block under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    Ipv4Permission,
    Ipv6Permission,
    FlatCidrIp,
}

/// An address range that is not a CIDR block is refused `InvalidParameterValue`, `CIDR block
/// <value> is malformed` — naming the block as the request wrote it — and stores nothing.
#[tokio::test]
async fn a_range_that_is_no_cidr_block_is_refused_naming_the_block_as_written() {
    let rows: [MalformedRow; 10] = [
        MalformedRow {
            case: "a bare IPv4 address, inbound",
            action: INGRESS,
            form: Form::Ipv4Permission,
            value: "203.0.113.5",
        },
        MalformedRow {
            case: "another bare IPv4 address, outbound",
            action: EGRESS,
            form: Form::Ipv4Permission,
            value: "10.0.0.1",
        },
        MalformedRow {
            case: "an IPv4 prefix longer than an IPv4 address",
            action: INGRESS,
            form: Form::Ipv4Permission,
            value: "10.0.0.0/33",
        },
        MalformedRow {
            case: "an IPv4 prefix that is not a number",
            action: INGRESS,
            form: Form::Ipv4Permission,
            value: "10.0.0.0/eight",
        },
        MalformedRow {
            case: "too few octets for an IPv4 address",
            action: EGRESS,
            form: Form::Ipv4Permission,
            value: "10.0.0/8",
        },
        MalformedRow {
            case: "a value that is no address at all",
            action: INGRESS,
            form: Form::Ipv4Permission,
            value: "not-a-cidr-block",
        },
        MalformedRow {
            case: "a bare IPv6 address",
            action: INGRESS,
            form: Form::Ipv6Permission,
            value: "2001:db8::1",
        },
        MalformedRow {
            case: "an IPv6 prefix longer than an IPv6 address",
            action: EGRESS,
            form: Form::Ipv6Permission,
            value: "2001:db8::/129",
        },
        MalformedRow {
            case: "a bare IPv4 address in the flat form",
            action: INGRESS,
            form: Form::FlatCidrIp,
            value: "198.51.100.7",
        },
        MalformedRow {
            // Longer than any CIDR block of either family, so the refusal names the block
            // only as far as the ceiling the crate holds an echoed block to.
            case: "a value far longer than any CIDR block",
            action: INGRESS,
            form: Form::Ipv4Permission,
            value: "10.0.0.0/8888888888888888888888888888888888888888888888888888888888888888888888",
        },
    ];

    let (router, _engine) = emulator();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group_id = create_group(case, &router, &format!("malformed-{index}")).await;
        let before = stored_rule_ids(case, &router, &group_id).await;

        let mut params = vec![param("GroupId", &group_id)];
        match row.form {
            Form::Ipv4Permission => params.extend(
                Rule::new("tcp", Some((443, 443)), Peer::Ipv4(row.value.to_string())).params(1),
            ),
            Form::Ipv6Permission => params.extend(
                Rule::new("tcp", Some((443, 443)), Peer::Ipv6(row.value.to_string())).params(1),
            ),
            Form::FlatCidrIp => params.extend([
                param("IpProtocol", "tcp"),
                param("FromPort", "443"),
                param("ToPort", "443"),
                param("CidrIp", row.value),
            ]),
        }

        let (status, body) = call(&router, row.action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected InvalidParameterValue, got {code:?} in {body}"
        );
        // The block as the request wrote it, held to the ceiling this crate echoes a
        // refused CIDR block to, so a caller-sized value cannot grow the refusal.
        let echoed: String = row
            .value
            .chars()
            .take(ec2::ops::delete_coip_cidr::MAX_ECHOED_CIDR_CHARS)
            .collect();
        let expected = format!("CIDR block {echoed} is malformed");
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        let after = stored_rule_ids(case, &router, &group_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected nothing stored, got {after:?} where the group held \
             {before:?}"
        );
    }
}

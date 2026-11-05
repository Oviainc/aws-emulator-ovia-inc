// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ReplaceNetworkAclEntry`: the rule a network ACL already holds at
//! one rule number in one direction, overwritten in place and read back through
//! `DescribeNetworkAcls`. Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. "Replaces an entry (rule) in a network ACL" (`ReplaceNetworkAclEntry`): the rule the
//!    ACL holds at the rule number and in the direction the request names takes the
//!    protocol, action, range and port or ICMP numbers the request carries, in the place the
//!    ACL already reported it — so the ACL reports ONE rule at that number afterwards, not
//!    two, and the rule the replacement displaced stops being reported.
//! 2. "The rule number of the entry to replace" (`$RuleNumber`) and "Indicates whether to
//!    replace the egress rule" (`$Egress`): the pair names one entry, so the same number in
//!    the other direction is a different entry a replacement leaves alone, and a number the
//!    ACL holds nothing at is refused with `InvalidNetworkAclEntry.NotFound` rather than
//!    creating a rule — this operation replaces, it never creates — and that refusal names
//!    the direction, the rule number and the ACL it looked in, the three facts that say
//!    which entry did not answer.
//! 3. A rule a caller may create is exactly a rule that may replace one: the published rule
//!    number range, the protocol number, the one address family and the ports or ICMP
//!    numbers a protocol needs are held to the same bounds `CreateNetworkAclEntry` holds
//!    them to, with nothing written for a refused request.
//! 4. A `DryRun` answers `DryRunOperation` and leaves the stored rule exactly as it was.
//! 5. A request with two faults is refused for the one read first: the ACL id's FORM before
//!    any other member, the request's own members before the ACL is looked up, and the ACL
//!    before the entry inside it.
//!
//! Every table row is a distinct input along what those rules generalise over — the
//! direction, the rule number, the protocol, the address family, the range's spelling, the
//! action and the port or ICMP numbers — so no single recorded request is the specification.

use ec2::state::{
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER,
    MAX_PROTOCOL_NUMBER, NETWORK_ACL_ENTRY_RULE_NUMBERS, RULE_ACTION_ALLOW, RULE_ACTION_DENY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{child, child_text, document, param, set_items};

/// The Region every call here is signed for. A network ACL carries no Region member, and the
/// default VPC a fresh account already has is seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// One entry of a reported network ACL, as the tables below assert on it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedEntry {
    rule_number: String,
    rule_action: String,
    protocol: String,
    cidr_block: Option<String>,
    ipv6_cidr_block: Option<String>,
    egress: String,
    port_range: Option<(Option<String>, Option<String>)>,
    icmp_type_code: Option<(Option<String>, Option<String>)>,
}

/// The members of one request that writes a rule, and the entry it must read back as — a row
/// of every table below, sent to `CreateNetworkAclEntry` to put the rule there and to
/// `ReplaceNetworkAclEntry` to overwrite it.
#[derive(Debug, Clone)]
struct Rule {
    case: &'static str,
    rule_number: i32,
    egress: bool,
    rule_action: &'static str,
    protocol: &'static str,
    /// The range exactly as the request spells it, which is not always how it reads back.
    requested_cidr: &'static str,
    /// Whether that range is the IPv6 one (`Ipv6CidrBlock` rather than `CidrBlock`).
    ipv6: bool,
    /// The range as the ACL must report it — the canonical form of the one requested.
    reported_cidr: &'static str,
    port_range: Option<(i32, i32)>,
    icmp_type_code: Option<(i32, i32)>,
}

/// The entries the one ACL in `body` reports, in the order the response reported them.
///
/// Read off the TREE: an entry's members are children of its own `<item>`, so scanning the
/// body for an element name would mix two entries' ports together.
fn reported_entries(case: &str, body: &str) -> Vec<ReportedEntry> {
    let parsed = document(case, body);
    let acls = set_items(parsed.root_element(), "networkAclSet");
    let acl = acls
        .first()
        .copied()
        .unwrap_or_else(|| panic!("case {case:?}: expected one network ACL in {body}"));
    set_items(acl, "entrySet")
        .into_iter()
        .map(|entry| ReportedEntry {
            rule_number: child_text(entry, "ruleNumber").unwrap_or_default(),
            rule_action: child_text(entry, "ruleAction").unwrap_or_default(),
            protocol: child_text(entry, "protocol").unwrap_or_default(),
            cidr_block: child_text(entry, "cidrBlock"),
            ipv6_cidr_block: child_text(entry, "ipv6CidrBlock"),
            egress: child_text(entry, "egress").unwrap_or_default(),
            port_range: child(entry, "portRange")
                .map(|range| (child_text(range, "from"), child_text(range, "to"))),
            icmp_type_code: child(entry, "icmpTypeCode")
                .map(|icmp| (child_text(icmp, "type"), child_text(icmp, "code"))),
        })
        .collect()
}

/// The form fields of one request writing `rule` onto `network_acl_id`.
fn rule_params(network_acl_id: &str, rule: &Rule) -> Vec<(String, String)> {
    let mut params = vec![
        param("NetworkAclId", network_acl_id),
        param("RuleNumber", &rule.rule_number.to_string()),
        param("Egress", &rule.egress.to_string()),
        param("RuleAction", rule.rule_action),
        param("Protocol", rule.protocol),
        param(
            if rule.ipv6 { "Ipv6CidrBlock" } else { "CidrBlock" },
            rule.requested_cidr,
        ),
    ];
    if let Some((from, to)) = rule.port_range {
        params.push(param("PortRange.From", &from.to_string()));
        params.push(param("PortRange.To", &to.to_string()));
    }
    if let Some((icmp_type, code)) = rule.icmp_type_code {
        params.push(param("Icmp.Type", &icmp_type.to_string()));
        params.push(param("Icmp.Code", &code.to_string()));
    }
    params
}

/// The form fields of a `ReplaceNetworkAclEntry` naming `network_acl_id`, `rule_number`, the
/// direction and an `allow` action, with `extra` naming whatever the row under test varies —
/// the protocol, the range and the ports or ICMP numbers. No field is sent twice, so the
/// value a row names is the only one the form parser can read.
fn varied_params(
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
    extra: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut params = vec![
        param("NetworkAclId", network_acl_id),
        param("RuleNumber", &rule_number.to_string()),
        param("Egress", &egress.to_string()),
        param("RuleAction", RULE_ACTION_ALLOW),
    ];
    params.extend(extra);
    params
}

/// One row of the refusal table below: the case, the rule number the request names, the rest
/// of its form fields — the protocol, the range and the ports or ICMP numbers the row varies
/// — and the error code the request earns.
type RefusalRow = (&'static str, i32, Vec<(String, String)>, &'static str);

/// A network ACL id of the ACL's own FORM that the account holds no ACL for: a stored id with
/// its last hexadecimal digit changed, so it is well formed and resolves to nothing.
fn unheld_acl_id(network_acl_id: &str) -> String {
    let mut digits = network_acl_id.to_string();
    let last = digits
        .pop()
        .unwrap_or_else(|| panic!("a stored network ACL id is not empty: {network_acl_id:?}"));
    digits.push(if last == '0' { '1' } else { '0' });
    digits
}

/// `rule` as the ACL must report it once stored.
fn expected_entry(rule: &Rule) -> ReportedEntry {
    ReportedEntry {
        rule_number: rule.rule_number.to_string(),
        rule_action: rule.rule_action.to_string(),
        protocol: rule.protocol.to_string(),
        cidr_block: (!rule.ipv6).then(|| rule.reported_cidr.to_string()),
        ipv6_cidr_block: rule.ipv6.then(|| rule.reported_cidr.to_string()),
        egress: rule.egress.to_string(),
        // A reported entry carries BOTH the port range and the ICMP type and code; the one
        // the rule's protocol has no use for comes back empty rather than absent.
        port_range: Some(rule.port_range.map_or((None, None), |(from, to)| {
            (Some(from.to_string()), Some(to.to_string()))
        })),
        icmp_type_code: Some(rule.icmp_type_code.map_or((None, None), |(icmp_type, code)| {
            (Some(icmp_type.to_string()), Some(code.to_string()))
        })),
    }
}

/// One `CreateNetworkAclEntry` call carrying `params`.
async fn create_entry(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateNetworkAclEntry", params).await
}

/// One `ReplaceNetworkAclEntry` call carrying `params`.
async fn replace_entry(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "ReplaceNetworkAclEntry", params).await
}

/// The XML of the one ACL `network_acl_id` names, asserted to be readable.
async fn acl_body(router: &MultiServiceRouter, case: &str, network_acl_id: &str) -> String {
    let params = indexed("NetworkAclId", &[network_acl_id]);
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {network_acl_id} back, got {status} with \
         body {body}"
    );
    body
}

/// The entries the ACL reports at `rule_number` in the direction `egress` names — a list, so
/// a replacement that stored a SECOND rule at that number is caught rather than hidden by a
/// search that stops at the first match.
async fn entries_at(
    router: &MultiServiceRouter,
    case: &str,
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
) -> Vec<ReportedEntry> {
    let described = acl_body(router, case, network_acl_id).await;
    reported_entries(case, &described)
        .into_iter()
        .filter(|entry| {
            entry.rule_number == rule_number.to_string() && entry.egress == egress.to_string()
        })
        .collect()
}

/// A VPC created over the wire, and a network ACL in it — the ACL every rule below is written
/// onto.
async fn vpc_and_acl(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    let vpc_id =
        element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"));
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "CreateNetworkAcl",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateNetworkAcl in {vpc_id}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("CreateNetworkAcl answered no networkAclId: {body}"))
}

/// `rule` created on the ACL, asserted to have been stored — the rule every replacement below
/// starts from.
async fn given_rule(router: &MultiServiceRouter, network_acl_id: &str, rule: &Rule) {
    let (status, body) = create_entry(router, &rule_params(network_acl_id, rule)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating the rule to replace, got {status} with \
         body {body}",
        case = rule.case
    );
}

/// The replacement table: for each kind of rule, the rule the ACL starts with and the rule it
/// is replaced by. The two differ in everything a replacement can change — the protocol, the
/// action, the range and its family, and the port or ICMP numbers — in both directions.
fn replacement_cases() -> Vec<(Rule, Rule)> {
    vec![
        // Every protocol replaced by TCP over a port, ingress, allow turned into deny: the
        // rule needs a port range once its protocol is one that has ports, and the range's
        // host bits are dropped — "we modify the specified CIDR block to its canonical form"
        // (`CreateNetworkAclEntryRequest$CidrBlock`).
        (
            Rule {
                case: "all protocols, allow, ingress",
                rule_number: *NETWORK_ACL_ENTRY_RULE_NUMBERS.start(),
                egress: false,
                rule_action: RULE_ACTION_ALLOW,
                protocol: ALL_IP_PROTOCOLS,
                requested_cidr: "0.0.0.0/0",
                ipv6: false,
                reported_cidr: "0.0.0.0/0",
                port_range: None,
                icmp_type_code: None,
            },
            Rule {
                case: "replaced by TCP on one port, deny, over a non-canonical range",
                rule_number: *NETWORK_ACL_ENTRY_RULE_NUMBERS.start(),
                egress: false,
                rule_action: RULE_ACTION_DENY,
                protocol: "6",
                requested_cidr: "10.0.1.37/24",
                ipv6: false,
                reported_cidr: "10.0.1.0/24",
                port_range: Some((443, 443)),
                icmp_type_code: None,
            },
        ),
        // UDP over a span of ports replaced by a narrower span, egress: the port range the
        // ACL reports is the replacement's, not the span it displaced.
        (
            Rule {
                case: "UDP over a span of ports, egress",
                rule_number: 120,
                egress: true,
                rule_action: RULE_ACTION_ALLOW,
                protocol: "17",
                requested_cidr: "198.51.100.0/24",
                ipv6: false,
                reported_cidr: "198.51.100.0/24",
                port_range: Some((1024, 65535)),
                icmp_type_code: None,
            },
            Rule {
                case: "replaced by UDP over a narrower span, egress",
                rule_number: 120,
                egress: true,
                rule_action: RULE_ACTION_ALLOW,
                protocol: "17",
                requested_cidr: "203.0.113.0/26",
                ipv6: false,
                reported_cidr: "203.0.113.0/26",
                port_range: Some((53, 53)),
                icmp_type_code: None,
            },
        ),
        // ICMP replaced by ICMPv6 over an IPv6 range: the address family changes, so the
        // reported entry carries the other family's member and the one it had goes away.
        (
            Rule {
                case: "ICMP with a type and code, ingress",
                rule_number: 200,
                egress: false,
                rule_action: RULE_ACTION_DENY,
                protocol: "1",
                requested_cidr: "192.0.2.0/24",
                ipv6: false,
                reported_cidr: "192.0.2.0/24",
                port_range: None,
                icmp_type_code: Some((8, 0)),
            },
            Rule {
                case: "replaced by ICMPv6 over an IPv6 range, ingress",
                rule_number: 200,
                egress: false,
                rule_action: RULE_ACTION_ALLOW,
                protocol: "58",
                requested_cidr: "2001:db8:1234:1a00::/64",
                ipv6: true,
                reported_cidr: "2001:db8:1234:1a00::/64",
                port_range: None,
                icmp_type_code: Some((128, 0)),
            },
        ),
        // The last rule number a caller may write, every protocol in both directions: a rule
        // replaced at the top of the published range, and the range's spelling widened.
        (
            Rule {
                case: "the last rule number, every protocol, egress",
                rule_number: *NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
                egress: true,
                rule_action: RULE_ACTION_DENY,
                protocol: ALL_IP_PROTOCOLS,
                requested_cidr: "10.2.3.4/32",
                ipv6: false,
                reported_cidr: "10.2.3.4/32",
                port_range: None,
                icmp_type_code: None,
            },
            Rule {
                case: "replaced by a rule over every address, GRE, egress",
                rule_number: *NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
                egress: true,
                rule_action: RULE_ACTION_ALLOW,
                // 47 (GRE): a protocol number with neither ports nor ICMP numbers, so the
                // replacement reports both of those empty.
                protocol: "47",
                requested_cidr: ALL_IPV4_ADDRESSES,
                ipv6: false,
                reported_cidr: ALL_IPV4_ADDRESSES,
                port_range: None,
                icmp_type_code: None,
            },
        ),
    ]
}

/// A replacement overwrites the rule the ACL holds at that number and direction: the ACL
/// reports the replacement, reports it ONCE, and stops reporting the rule it displaced.
#[tokio::test]
async fn a_replaced_rule_is_the_only_rule_at_its_number() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.0.0.0/16").await;

    for (before, after) in replacement_cases() {
        given_rule(&router, &acl_id, &before).await;
        let case = after.case;

        // The rule is there as created, which is what the replacement displaces.
        let stored = entries_at(&router, case, &acl_id, before.rule_number, before.egress).await;
        assert_eq!(
            stored,
            vec![expected_entry(&before)],
            "case {case:?}: expected the created rule to be stored, got {stored:?}"
        );

        let (status, body) = replace_entry(&router, &rule_params(&acl_id, &after)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected the success flag, got {body}"
        );

        let stored = entries_at(&router, case, &acl_id, after.rule_number, after.egress).await;
        assert_eq!(
            stored,
            vec![expected_entry(&after)],
            "case {case:?}: expected exactly the replacement at that rule number, got \
             {stored:?}"
        );
    }
}

/// The rule number and direction together name one entry: a replacement leaves the same
/// number in the other direction exactly as it was.
#[tokio::test]
async fn a_replacement_leaves_the_other_direction_alone() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.1.0.0/16").await;

    // The same rule number in each direction: two entries, so the replacement has to pick
    // one.
    let rules: Vec<Rule> = [false, true]
        .into_iter()
        .map(|egress| Rule {
            case: if egress { "egress" } else { "ingress" },
            rule_number: 110,
            egress,
            rule_action: RULE_ACTION_ALLOW,
            protocol: ALL_IP_PROTOCOLS,
            requested_cidr: "0.0.0.0/0",
            ipv6: false,
            reported_cidr: "0.0.0.0/0",
            port_range: None,
            icmp_type_code: None,
        })
        .collect();
    for rule in &rules {
        given_rule(&router, &acl_id, rule).await;
    }

    for replaced in [false, true] {
        let case = if replaced {
            "replacing the egress rule"
        } else {
            "replacing the ingress rule"
        };
        let mut after = rules[usize::from(replaced)].clone();
        after.case = case;
        after.rule_action = RULE_ACTION_DENY;
        after.protocol = "6";
        after.port_range = Some((22, 22));
        let (status, body) = replace_entry(&router, &rule_params(&acl_id, &after)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let stored = entries_at(&router, case, &acl_id, after.rule_number, replaced).await;
        assert_eq!(
            stored,
            vec![expected_entry(&after)],
            "case {case:?}: expected the replacement in that direction, got {stored:?}"
        );
        let untouched = &rules[usize::from(!replaced)];
        let stored = entries_at(&router, case, &acl_id, untouched.rule_number, !replaced).await;
        assert_eq!(
            stored,
            vec![expected_entry(untouched)],
            "case {case:?}: expected the other direction's rule to be left alone, got \
             {stored:?}"
        );

        // Put the direction back as it was, so the next row starts from the same ACL.
        let (status, body) = replace_entry(&router, &rule_params(&acl_id, untouched)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 restoring the rule, got {status} with body \
             {body}"
        );
        let (status, body) =
            replace_entry(&router, &rule_params(&acl_id, &rules[usize::from(replaced)])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 restoring the replaced rule, got {status} with \
             body {body}"
        );
    }
}

/// A rule number the ACL holds nothing at is refused: this operation replaces an entry, it
/// never creates one, so nothing is stored at that number afterwards.
#[tokio::test]
async fn a_rule_the_acl_does_not_hold_is_not_created() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.2.0.0/16").await;

    let held = Rule {
        case: "the one rule the ACL holds",
        rule_number: 100,
        egress: false,
        rule_action: RULE_ACTION_ALLOW,
        protocol: ALL_IP_PROTOCOLS,
        requested_cidr: "0.0.0.0/0",
        ipv6: false,
        reported_cidr: "0.0.0.0/0",
        port_range: None,
        icmp_type_code: None,
    };
    given_rule(&router, &acl_id, &held).await;

    // (case, the rule number, the direction) — a number no rule sits at, the held number in
    // the other direction, and each end of the published range the ACL holds nothing at.
    let cases: [(&str, i32, bool); 4] = [
        ("a rule number the ACL holds nothing at", 105, false),
        ("the held number in the other direction", 100, true),
        (
            "the first publishable number, unheld",
            *NETWORK_ACL_ENTRY_RULE_NUMBERS.start(),
            false,
        ),
        (
            "the last publishable number, unheld",
            *NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
            true,
        ),
    ];
    for (case, rule_number, egress) in cases {
        let missing = Rule {
            case,
            rule_number,
            egress,
            rule_action: RULE_ACTION_DENY,
            protocol: ALL_IP_PROTOCOLS,
            requested_cidr: "192.0.2.0/24",
            ipv6: false,
            reported_cidr: "192.0.2.0/24",
            port_range: None,
            icmp_type_code: None,
        };
        let (status, body) = replace_entry(&router, &rule_params(&acl_id, &missing)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidNetworkAclEntry.NotFound",
            "case {case:?}: expected InvalidNetworkAclEntry.NotFound, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "no {direction} entry with number {rule_number} in network ACL {acl_id}",
                direction = if egress { "egress" } else { "ingress" },
            ),
            "case {case:?}: expected the direction, the rule number and the ACL in the \
             message, got {body}"
        );
        let stored = entries_at(&router, case, &acl_id, rule_number, egress).await;
        assert!(
            stored.is_empty(),
            "case {case:?}: expected the refused replacement not to create a rule, got \
             {stored:?}"
        );
    }

    // The one rule the ACL does hold is untouched by every refusal above.
    let stored = entries_at(&router, held.case, &acl_id, held.rule_number, held.egress).await;
    assert_eq!(
        stored,
        vec![expected_entry(&held)],
        "case {case:?}: expected the held rule to be left alone, got {stored:?}",
        case = held.case
    );
}

/// A rule that may replace one is exactly a rule that may be created: the published rule
/// number range, the protocol number, the one address family and what a protocol needs are
/// held to the same bounds, with the stored rule left as it was for every refusal.
///
/// The catalogue is the one the members publish: "Positive integer from 1 to 32766. The range
/// 32767 to 65535 is reserved for internal use" (`CreateNetworkAclEntryRequest$RuleNumber`),
/// "The protocol number. A value of -1 means all protocols"
/// (`ReplaceNetworkAclEntryRequest$Protocol`) over the one-byte protocol field, and the IPv4
/// and IPv6 ranges a rule matches one of
/// (`ReplaceNetworkAclEntryRequest$CidrBlock`, `$Ipv6CidrBlock`).
#[tokio::test]
async fn a_replacement_is_held_to_what_a_rule_may_be() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.3.0.0/16").await;

    let held = Rule {
        case: "the rule every refusal below must leave alone",
        rule_number: 100,
        egress: false,
        rule_action: RULE_ACTION_ALLOW,
        protocol: ALL_IP_PROTOCOLS,
        requested_cidr: "0.0.0.0/0",
        ipv6: false,
        reported_cidr: "0.0.0.0/0",
        port_range: None,
        icmp_type_code: None,
    };
    given_rule(&router, &acl_id, &held).await;

    let first = *NETWORK_ACL_ENTRY_RULE_NUMBERS.start();
    let last = *NETWORK_ACL_ENTRY_RULE_NUMBERS.end();
    // Every row names the ACL the held rule is on and that rule's direction, so the only
    // fault is the one the case names: (case, the rule number, the rest of the request, the
    // error code it earns).
    let rows: Vec<RefusalRow> = vec![
        (
            "one below the first rule number",
            first - 1,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "one past the last rule number",
            last + 1,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "the reserved catch-all rule number",
            DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "a protocol name rather than a number",
            held.rule_number,
            vec![
                param("Protocol", "tcp"),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "one past the last protocol number",
            held.rule_number,
            vec![
                param("Protocol", &(MAX_PROTOCOL_NUMBER + 1).to_string()),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "a protocol number below zero that is not all protocols",
            held.rule_number,
            vec![
                param("Protocol", "-2"),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
            "InvalidParameterValue",
        ),
        (
            "TCP with no port range",
            held.rule_number,
            vec![param("Protocol", "6"), param("CidrBlock", ALL_IPV4_ADDRESSES)],
            "MissingParameter",
        ),
        (
            "UDP with only the first port of the range",
            held.rule_number,
            vec![
                param("Protocol", "17"),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
                param("PortRange.From", "53"),
            ],
            "MissingParameter",
        ),
        (
            "a port range whose first port is above its last",
            held.rule_number,
            vec![
                param("Protocol", "6"),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
                param("PortRange.From", "443"),
                param("PortRange.To", "80"),
            ],
            "InvalidParameterValue",
        ),
        (
            "ICMP with no type and code",
            held.rule_number,
            vec![param("Protocol", "1"), param("CidrBlock", ALL_IPV4_ADDRESSES)],
            "MissingParameter",
        ),
        (
            "a range that is not CIDR notation",
            held.rule_number,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", "10.0.0.0/33"),
            ],
            "InvalidParameterValue",
        ),
        (
            "an IPv6 range that is not CIDR notation",
            held.rule_number,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("Ipv6CidrBlock", "2001:db8::/129"),
            ],
            "InvalidParameterValue",
        ),
        (
            "both address families at once",
            held.rule_number,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
                param("Ipv6CidrBlock", "2001:db8::/32"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "no range at all",
            held.rule_number,
            vec![param("Protocol", ALL_IP_PROTOCOLS)],
            "MissingParameter",
        ),
    ];
    for (case, rule_number, rest, expected_code) in rows {
        let params = varied_params(&acl_id, rule_number, held.egress, rest);
        let (status, body) = replace_entry(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let stored = entries_at(&router, case, &acl_id, held.rule_number, held.egress).await;
        assert_eq!(
            stored,
            vec![expected_entry(&held)],
            "case {case:?}: expected the stored rule to be left as it was, got {stored:?}"
        );
    }

    // Each published bound of the rule number range IS writable, so the refusals above are
    // the bound and not the whole range: a rule at each bound is replaced in place.
    for (case, rule_number) in [("the first rule number", first), ("the last rule number", last)] {
        let mut rule = held.clone();
        rule.case = case;
        rule.rule_number = rule_number;
        given_rule(&router, &acl_id, &rule).await;
        let mut after = rule.clone();
        after.rule_action = RULE_ACTION_DENY;
        let (status, body) = replace_entry(&router, &rule_params(&acl_id, &after)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at a published bound, got {status} with body \
             {body}"
        );
        let stored = entries_at(&router, case, &acl_id, rule_number, after.egress).await;
        assert_eq!(
            stored,
            vec![expected_entry(&after)],
            "case {case:?}: expected the replacement at that bound, got {stored:?}"
        );
    }
}

/// A `DryRun` of a replacement that would have succeeded answers `DryRunOperation` and leaves
/// the stored rule exactly as it was.
#[tokio::test]
async fn a_dry_run_replaces_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.4.0.0/16").await;

    let before = Rule {
        case: "a dry run leaves this rule alone",
        rule_number: 130,
        egress: true,
        rule_action: RULE_ACTION_ALLOW,
        protocol: ALL_IP_PROTOCOLS,
        requested_cidr: "0.0.0.0/0",
        ipv6: false,
        reported_cidr: "0.0.0.0/0",
        port_range: None,
        icmp_type_code: None,
    };
    given_rule(&router, &acl_id, &before).await;

    let case = before.case;
    let mut after = before.clone();
    after.rule_action = RULE_ACTION_DENY;
    after.protocol = "6";
    after.port_range = Some((80, 80));
    let mut params = rule_params(&acl_id, &after);
    params.push(param("DryRun", "true"));
    let (status, body) = replace_entry(&router, &params).await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 for a dry run, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case {case:?}: expected DryRunOperation, got {body}"
    );
    let stored = entries_at(&router, case, &acl_id, before.rule_number, before.egress).await;
    assert_eq!(
        stored,
        vec![expected_entry(&before)],
        "case {case:?}: expected the dry run to change nothing, got {stored:?}"
    );

    // Without the flag the same request goes through, so the dry run refused a request that
    // would have succeeded.
    let (status, body) = replace_entry(&router, &rule_params(&acl_id, &after)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 without the dry run, got {status} with body {body}"
    );
    let stored = entries_at(&router, case, &acl_id, after.rule_number, after.egress).await;
    assert_eq!(
        stored,
        vec![expected_entry(&after)],
        "case {case:?}: expected the replacement once the flag is gone, got {stored:?}"
    );
}

/// A request with two faults is refused for the one read first: the ACL id's FORM before any
/// other member, the request's own members before the ACL is looked up, and the ACL before
/// the entry inside it.
#[tokio::test]
async fn a_request_with_two_faults_is_refused_for_the_fault_read_first() {
    let (router, _engine) = super::ec2_query::emulator();
    let acl_id = vpc_and_acl(&router, "10.5.0.0/16").await;

    let held = Rule {
        case: "the rule the ACL holds",
        rule_number: 100,
        egress: false,
        rule_action: RULE_ACTION_ALLOW,
        protocol: ALL_IP_PROTOCOLS,
        requested_cidr: "0.0.0.0/0",
        ipv6: false,
        reported_cidr: "0.0.0.0/0",
        port_range: None,
        icmp_type_code: None,
    };
    given_rule(&router, &acl_id, &held).await;

    // An id of the ACL's own form that no ACL in the account has, for the rows that get past
    // the form check.
    let unknown_acl_id = unheld_acl_id(&acl_id);
    // (case, the ACL id, the rule number, the code the fault read FIRST earns). Every row
    // carries a second fault beside the one it is refused for.
    let rows: Vec<(&str, &str, i32, &str)> = vec![
        (
            "a malformed id beside an unwritable rule number",
            "not-an-acl-id",
            0,
            "InvalidNetworkAclID.Malformed",
        ),
        (
            "an unknown id beside an unwritable rule number",
            unknown_acl_id.as_str(),
            0,
            "InvalidParameterValue",
        ),
        (
            "an unknown id beside a rule number nothing sits at",
            unknown_acl_id.as_str(),
            105,
            "InvalidNetworkAclID.NotFound",
        ),
    ];
    for (case, network_acl_id, rule_number, expected_code) in rows {
        let params = varied_params(
            network_acl_id,
            rule_number,
            held.egress,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
        );
        let (status, body) = replace_entry(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    let stored = entries_at(&router, held.case, &acl_id, held.rule_number, held.egress).await;
    assert_eq!(
        stored,
        vec![expected_entry(&held)],
        "case {case:?}: expected the held rule to be left alone, got {stored:?}",
        case = held.case
    );
}

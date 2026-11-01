// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the entries (rules) of a network ACL: `CreateNetworkAclEntry` puts
//! one on a stored ACL, `DeleteNetworkAclEntry` takes it off and `DeleteNetworkAcl` takes the
//! ACL itself away — each read back through `DescribeNetworkAcls`. Driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope, so
//! every call here carries one.
//!
//! The rules under test:
//!
//! 1. A MUTATION of a network ACL is visible in the next read of it: an entry a create
//!    stores appears in the ACL's `entrySet` with the rule number, protocol, action,
//!    direction, range and port or ICMP numbers the request named, beside the entries the
//!    ACL already had, and carries both the port range and the ICMP type and code — EMPTY
//!    for the one the rule's protocol has no use for; an entry a delete removes stops
//!    appearing; and an ACL a delete removes stops existing, so a read of its id answers
//!    `InvalidNetworkAclID.NotFound`.
//! 2. An ACL's entries are reported in ONE order, whatever order they were created in: the
//!    egress rules first, then the ingress rules, each by ascending rule number.
//! 3. "Each network ACL has a set of numbered ingress rules and a separate set of numbered
//!    egress rules" (`CreateNetworkAclEntry`), so a rule number identifies one entry WITHIN a
//!    direction: the same number in the other direction is a different entry, and a repeat in
//!    the same direction is refused rather than overwriting it.
//! 4. "Constraints: Positive integer from 1 to 32766" (`$RuleNumber`): each published bound
//!    is accepted and each value one past it is refused, with nothing stored — and a rule
//!    number a caller may not write is one it may not delete either, so the catch-all deny
//!    each direction ends with stays on the ACL.
//! 5. "You can't delete the default network ACL. You can't delete the ACL if it's associated
//!    with any subnets" (`DeleteNetworkAcl`), and an id that is not a network ACL id at all
//!    is told apart from one the account holds no ACL for.
//! 6. A `DryRun` of either mutation answers `DryRunOperation` and changes nothing.
//! 7. "The protocol number. A value of -1 means all protocols" (`$Protocol`): a rule's
//!    protocol is a NUMBER, so every number the field of the IP header holds is accepted —
//!    whichever way the request spells it, stored as the number it names — and a value that is
//!    not such a number (a protocol NAME, a value past either end) is refused with nothing
//!    stored.
//! 8. A rule matches a range of ONE address family: a request naming both `CidrBlock` and
//!    `Ipv6CidrBlock` is refused, and a range of either family that is not a CIDR block is
//!    refused, with nothing stored.
//! 9. A network ACL holds at most the published number of rules, counted over the whole ACL,
//!    and one more is refused with the quota code rather than displacing a stored rule.
//! 10. A request with two faults is refused for the one the handler reads first: the ACL id's
//!     FORM before any other member, and the members of the request before the ACL is looked
//!     up — the same order `DeleteNetworkAclEntry` reads them in.
//!
//! Every table row is a distinct input along what those rules generalise over — the
//! direction, the rule number, the protocol, the address family, the range's spelling, the
//! action and the port or ICMP numbers — so no single recorded request is the specification.

use ec2::state::{
    ALL_IPV4_ADDRESSES, ALL_IP_PROTOCOLS, DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER,
    MAX_NETWORK_ACL_ENTRIES, MAX_PROTOCOL_NUMBER, NETWORK_ACL_ENTRY_RULE_NUMBERS,
    NETWORK_ACL_PROTOCOL_ICMP, NETWORK_ACL_PROTOCOL_ICMPV6, NETWORK_ACL_PROTOCOL_TCP,
    NETWORK_ACL_PROTOCOL_UDP, RULE_ACTION_ALLOW, RULE_ACTION_DENY,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
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

/// One row of the create table: the request's members, and the entry they must read back as.
#[derive(Debug, Clone)]
struct EntryCase {
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

/// The form fields of one `CreateNetworkAclEntry` request for `row` on `network_acl_id`.
fn entry_params(network_acl_id: &str, row: &EntryCase) -> Vec<(String, String)> {
    let mut params = vec![
        param("NetworkAclId", network_acl_id),
        param("RuleNumber", &row.rule_number.to_string()),
        param("Egress", &row.egress.to_string()),
        param("RuleAction", row.rule_action),
        param("Protocol", row.protocol),
    ];
    params.push(param(
        if row.ipv6 { "Ipv6CidrBlock" } else { "CidrBlock" },
        row.requested_cidr,
    ));
    if let Some((from, to)) = row.port_range {
        params.push(param("PortRange.From", &from.to_string()));
        params.push(param("PortRange.To", &to.to_string()));
    }
    if let Some((icmp_type, code)) = row.icmp_type_code {
        params.push(param("Icmp.Type", &icmp_type.to_string()));
        params.push(param("Icmp.Code", &code.to_string()));
    }
    params
}

/// `row` as the ACL must report it once stored.
fn expected_entry(row: &EntryCase) -> ReportedEntry {
    ReportedEntry {
        rule_number: row.rule_number.to_string(),
        rule_action: row.rule_action.to_string(),
        protocol: row.protocol.to_string(),
        cidr_block: (!row.ipv6).then(|| row.reported_cidr.to_string()),
        ipv6_cidr_block: row.ipv6.then(|| row.reported_cidr.to_string()),
        egress: row.egress.to_string(),
        // A reported entry carries BOTH the port range and the ICMP type and code; the one
        // the rule's protocol has no use for comes back empty rather than absent.
        port_range: Some(
            row.port_range
                .map_or((None, None), |(from, to)| {
                    (Some(from.to_string()), Some(to.to_string()))
                }),
        ),
        icmp_type_code: Some(row.icmp_type_code.map_or((None, None), |(icmp_type, code)| {
            (Some(icmp_type.to_string()), Some(code.to_string()))
        })),
    }
}

/// The catch-all deny every network ACL ends with, in the direction `egress` names — the
/// entries a created ACL comes with, which the entries a caller adds sit among.
fn catch_all_entry(egress: bool) -> ReportedEntry {
    ReportedEntry {
        rule_number: DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER.to_string(),
        rule_action: RULE_ACTION_DENY.to_string(),
        protocol: ALL_IP_PROTOCOLS.to_string(),
        cidr_block: Some(ALL_IPV4_ADDRESSES.to_string()),
        ipv6_cidr_block: None,
        egress: egress.to_string(),
        port_range: Some((None, None)),
        icmp_type_code: Some((None, None)),
    }
}

/// One `CreateNetworkAclEntry` call carrying `params`, as its HTTP status and XML body.
async fn create_entry(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateNetworkAclEntry", params).await
}

/// One `DeleteNetworkAclEntry` call for `rule_number` in the direction `egress` names.
async fn delete_entry(
    router: &MultiServiceRouter,
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![
        param("NetworkAclId", network_acl_id),
        param("RuleNumber", &rule_number.to_string()),
        param("Egress", &egress.to_string()),
    ];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "DeleteNetworkAclEntry", &params).await
}

/// One `DeleteNetworkAcl` call for `network_acl_id`.
async fn delete_acl(
    router: &MultiServiceRouter,
    network_acl_id: &str,
    dry_run: bool,
) -> (u16, String) {
    let mut params = vec![param("NetworkAclId", network_acl_id)];
    if dry_run {
        params.push(param("DryRun", "true"));
    }
    super::ec2_query::call(router, REGION, "DeleteNetworkAcl", &params).await
}

/// One `DescribeNetworkAcls` call naming `network_acl_id`, as its status and body.
async fn describe_one(
    router: &MultiServiceRouter,
    network_acl_id: &str,
) -> (u16, String) {
    let params = indexed("NetworkAclId", &[network_acl_id]);
    super::ec2_query::call(router, REGION, "DescribeNetworkAcls", &params).await
}

/// The XML of the one ACL `network_acl_id` names, asserted to be readable.
async fn acl_body(router: &MultiServiceRouter, case: &str, network_acl_id: &str) -> String {
    let (status, body) = describe_one(router, network_acl_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {network_acl_id} back, got {status} \
         with body {body}"
    );
    body
}

/// A VPC created over the wire, and a network ACL in it — the ACL every entry below is
/// created on. A created ACL is associated with no subnet, which is what lets the delete
/// table below take it away.
async fn vpc_and_acl(router: &MultiServiceRouter, cidr_block: &str) -> (String, String) {
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
    let vpc_id = element(&body, "vpcId")
        .unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"));
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
    let network_acl_id = element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("CreateNetworkAcl answered no networkAclId: {body}"));
    (vpc_id, network_acl_id)
}

/// The create table: one row per kind of rule a caller can write, each read back out of the
/// ACL afterwards.
fn create_cases() -> Vec<EntryCase> {
    vec![
        // The lower published bound of `RuleNumber`, over every protocol.
        EntryCase {
            case: "the lowest rule number, every protocol, ingress",
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
        // TCP, which "must specify a port range", over a range whose host bits are set: "we
        // modify the specified CIDR block to its canonical form".
        EntryCase {
            case: "TCP with a port range and a non-canonical range",
            rule_number: 100,
            egress: false,
            rule_action: RULE_ACTION_ALLOW,
            protocol: "6",
            requested_cidr: "10.0.1.37/24",
            ipv6: false,
            reported_cidr: "10.0.1.0/24",
            port_range: Some((443, 443)),
            icmp_type_code: None,
        },
        // UDP, the other protocol a port range is required for, egress and over a span of
        // ports rather than one.
        EntryCase {
            case: "UDP egress over a span of ports",
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
        // ICMP, which carries a type and a code instead of ports, denying rather than
        // allowing.
        EntryCase {
            case: "ICMP with a type and code",
            rule_number: 200,
            egress: false,
            rule_action: RULE_ACTION_DENY,
            protocol: "1",
            requested_cidr: "192.0.2.0/24",
            ipv6: false,
            reported_cidr: "192.0.2.0/24",
            port_range: None,
            icmp_type_code: Some((8, -1)),
        },
        // ICMPv6 over an IPv6 range, which is the case that needs a type and code.
        EntryCase {
            case: "ICMPv6 over an IPv6 range",
            rule_number: 300,
            egress: true,
            rule_action: RULE_ACTION_ALLOW,
            protocol: "58",
            requested_cidr: "2001:db8:1234:1a00::/64",
            ipv6: true,
            reported_cidr: "2001:db8:1234:1a00::/64",
            port_range: None,
            icmp_type_code: Some((128, 0)),
        },
        // The upper published bound of `RuleNumber`, which still sorts before the catch-all
        // deny the ACL came with.
        EntryCase {
            case: "the highest rule number a caller may write",
            rule_number: *NETWORK_ACL_ENTRY_RULE_NUMBERS.end(),
            egress: true,
            rule_action: RULE_ACTION_DENY,
            protocol: "6",
            requested_cidr: "203.0.113.128/25",
            ipv6: false,
            reported_cidr: "203.0.113.128/25",
            port_range: Some((22, 22)),
            icmp_type_code: None,
        },
    ]
}

/// Every created entry is on the ACL the describe reports, with the members the request
/// named, in the one order an ACL's entries are reported in.
#[tokio::test]
async fn created_entries_are_read_back_in_the_order_ec2_reports_them() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.0.0.0/16").await;

    let cases = create_cases();
    for row in &cases {
        let (status, body) = create_entry(&router, &entry_params(&acl_id, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = row.case
        );

        // The mutation is visible in the very next read.
        let described = acl_body(&router, row.case, &acl_id).await;
        let entries = reported_entries(row.case, &described);
        let expected = expected_entry(row);
        assert!(
            entries.contains(&expected),
            "case {case:?}: expected the created entry {expected:?} on the ACL, \
             got {entries:?}",
            case = row.case
        );
    }

    // And the whole set, in the one order an ACL's entries come back in: the egress rules
    // first, then the ingress rules, each by ascending rule number — whatever order the
    // creates above were made in.
    let described = acl_body(&router, "every entry", &acl_id).await;
    let entries = reported_entries("every entry", &described);
    let mut expected: Vec<ReportedEntry> = cases.iter().map(expected_entry).collect();
    expected.push(catch_all_entry(false));
    expected.push(catch_all_entry(true));
    expected.sort_by_key(|entry| {
        (
            entry.egress != "true",
            entry.rule_number.parse::<i32>().unwrap_or_default(),
        )
    });
    assert_eq!(
        entries, expected,
        "expected the ACL's entries egress-first and then ingress, each by ascending rule \
         number, got {entries:?}"
    );
}

/// A rule number outside the published range is refused, and each published bound is
/// accepted — with nothing stored for the refused ones.
#[tokio::test]
async fn a_rule_number_outside_the_published_range_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.1.0.0/16").await;

    let first = *NETWORK_ACL_ENTRY_RULE_NUMBERS.start();
    let last = *NETWORK_ACL_ENTRY_RULE_NUMBERS.end();
    // (case, the rule number, whether it is accepted) — each published bound, one past each
    // of them, and the reserved rule number the ACL's own catch-all deny sits at.
    let cases: [(&str, i32, bool); 6] = [
        ("one below the first", first - 1, false),
        ("the first", first, true),
        ("the last", last, true),
        ("one past the last", last + 1, false),
        (
            "the reserved catch-all rule number",
            DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER,
            false,
        ),
        ("far above the range", 65_535, false),
    ];
    for (case, rule_number, accepted) in cases {
        let row = EntryCase {
            case,
            rule_number,
            egress: false,
            rule_action: RULE_ACTION_ALLOW,
            protocol: ALL_IP_PROTOCOLS,
            requested_cidr: "0.0.0.0/0",
            ipv6: false,
            reported_cidr: "0.0.0.0/0",
            port_range: None,
            icmp_type_code: None,
        };
        let (status, body) = create_entry(&router, &entry_params(&acl_id, &row)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Value ({rule_number}) for parameter ruleNumber is invalid. Rulenumber \
                 must be in range {first}..{last}"
            ),
            "case {case:?}: expected the published range in the message, got {body}"
        );
        // Nothing was stored for it.
        let described = acl_body(&router, case, &acl_id).await;
        let entries = reported_entries(case, &described);
        assert!(
            !entries
                .iter()
                .any(|entry| entry.rule_number == rule_number.to_string()
                    && entry.egress == "false"
                    && entry.rule_action == RULE_ACTION_ALLOW),
            "case {case:?}: expected the refused entry not to be stored, got {entries:?}"
        );
    }
}

/// A rule number names one entry within a direction: the same number in the other direction
/// is a different entry, and a repeat in the same direction is refused.
#[tokio::test]
async fn one_rule_number_names_one_entry_per_direction() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.2.0.0/16").await;

    for (case, egress) in [("ingress", false), ("egress", true)] {
        let row = EntryCase {
            case,
            rule_number: 100,
            egress,
            rule_action: RULE_ACTION_ALLOW,
            protocol: ALL_IP_PROTOCOLS,
            requested_cidr: "0.0.0.0/0",
            ipv6: false,
            reported_cidr: "0.0.0.0/0",
            port_range: None,
            icmp_type_code: None,
        };
        let (status, body) = create_entry(&router, &entry_params(&acl_id, &row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: rule 100 in each direction is two entries, got {status} with \
             body {body}"
        );

        // The same number in the same direction again: refused, and the stored entry is
        // left as it was.
        let mut repeat = row.clone();
        repeat.rule_action = RULE_ACTION_DENY;
        let (status, body) = create_entry(&router, &entry_params(&acl_id, &repeat)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a repeat, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NetworkAclEntryAlreadyExists",
            "case {case:?}: expected NetworkAclEntryAlreadyExists, got {body}"
        );
        let described = acl_body(&router, case, &acl_id).await;
        let entries = reported_entries(case, &described);
        assert!(
            entries.contains(&expected_entry(&row)),
            "case {case:?}: expected the first entry to be left alone, got {entries:?}"
        );
        assert!(
            !entries.contains(&expected_entry(&repeat)),
            "case {case:?}: expected the refused repeat not to be stored, got {entries:?}"
        );
    }
}

/// A delete takes the entry off the ACL the describe reports, leaves the other direction's
/// entry alone, and a second delete of the same entry is refused.
#[tokio::test]
async fn a_deleted_entry_stops_being_reported() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.3.0.0/16").await;

    // Two entries that differ only in direction, so the delete has to pick one.
    let rows: Vec<EntryCase> = [false, true]
        .into_iter()
        .map(|egress| EntryCase {
            case: if egress { "egress" } else { "ingress" },
            rule_number: 110,
            egress,
            rule_action: RULE_ACTION_ALLOW,
            protocol: "6",
            requested_cidr: "0.0.0.0/0",
            ipv6: false,
            reported_cidr: "0.0.0.0/0",
            port_range: Some((80, 80)),
            icmp_type_code: None,
        })
        .collect();
    for row in &rows {
        let (status, body) = create_entry(&router, &entry_params(&acl_id, row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}",
            case = row.case
        );
    }

    // A dry run of the delete changes nothing.
    let (status, body) = delete_entry(&router, &acl_id, 110, false, true).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    let described = acl_body(&router, "after the dry run", &acl_id).await;
    let entries = reported_entries("after the dry run", &described);
    assert!(
        entries.contains(&expected_entry(&rows[0])),
        "a dry run must not delete anything, got {entries:?}"
    );

    let (status, body) = delete_entry(&router, &acl_id, 110, false, false).await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body}"
    );
    let described = acl_body(&router, "after the delete", &acl_id).await;
    let entries = reported_entries("after the delete", &described);
    assert!(
        !entries.contains(&expected_entry(&rows[0])),
        "after the delete: expected the ingress entry to be gone, got {entries:?}"
    );
    assert!(
        entries.contains(&expected_entry(&rows[1])),
        "after the delete: expected the egress entry at the same rule number to be left \
         alone, got {entries:?}"
    );

    // The same delete again: there is nothing left to delete, so it is refused rather than
    // answered as though it had removed something.
    let (status, body) = delete_entry(&router, &acl_id, 110, false, false).await;
    assert_eq!(
        status, 400,
        "a second delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkAclEntry.NotFound",
        "a second delete: expected InvalidNetworkAclEntry.NotFound, got {body}"
    );

    // The rule numbers a caller may not write are rule numbers a caller may not delete
    // either: the catch-all deny each direction ends with stays on the ACL.
    for (case, rule_number) in [
        (
            "the reserved catch-all rule number",
            DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER,
        ),
        ("one below the first", *NETWORK_ACL_ENTRY_RULE_NUMBERS.start() - 1),
    ] {
        let (status, body) = delete_entry(&router, &acl_id, rule_number, true, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
    let described = acl_body(&router, "the catch-all pair", &acl_id).await;
    let entries = reported_entries("the catch-all pair", &described);
    for egress in [false, true] {
        assert!(
            entries.contains(&catch_all_entry(egress)),
            "expected the catch-all deny in each direction to survive, got {entries:?}"
        );
    }
}

/// One row of the protocol-requirement table: the form fields a create sends beyond the ACL
/// id, and the code the refusal must carry.
struct RefusalCase {
    case: &'static str,
    fields: Vec<(String, String)>,
    code: &'static str,
}

/// An entry whose protocol needs a port range or an ICMP type and code is refused without
/// one, and a range of the wrong shape is refused too.
#[tokio::test]
async fn an_entry_is_held_to_what_its_protocol_needs() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.4.0.0/16").await;

    // One row per protocol requirement the operation documents.
    let cases: [RefusalCase; 6] = [
        RefusalCase {
            case: "TCP with no port range",
            fields: vec![
                param("RuleNumber", "100"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", &NETWORK_ACL_PROTOCOL_TCP.to_string()),
                param("CidrBlock", "0.0.0.0/0"),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "UDP with no port range",
            fields: vec![
                param("RuleNumber", "101"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", &NETWORK_ACL_PROTOCOL_UDP.to_string()),
                param("CidrBlock", "0.0.0.0/0"),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "ICMP with no type and code",
            fields: vec![
                param("RuleNumber", "102"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", &NETWORK_ACL_PROTOCOL_ICMP.to_string()),
                param("CidrBlock", "0.0.0.0/0"),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "ICMPv6 over an IPv6 range with no type and code",
            fields: vec![
                param("RuleNumber", "103"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", &NETWORK_ACL_PROTOCOL_ICMPV6.to_string()),
                param("Ipv6CidrBlock", "::/0"),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "no range at all",
            fields: vec![
                param("RuleNumber", "104"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", ALL_IP_PROTOCOLS),
            ],
            code: "MissingParameter",
        },
        RefusalCase {
            case: "a port range whose first port is above its last",
            fields: vec![
                param("RuleNumber", "105"),
                param("Egress", "false"),
                param("RuleAction", RULE_ACTION_ALLOW),
                param("Protocol", &NETWORK_ACL_PROTOCOL_TCP.to_string()),
                param("CidrBlock", "0.0.0.0/0"),
                param("PortRange.From", "443"),
                param("PortRange.To", "80"),
            ],
            code: "InvalidParameterValue",
        },
    ];
    for row in cases {
        let RefusalCase {
            case,
            fields,
            code: expected_code,
        } = row;
        let mut params = vec![param("NetworkAclId", &acl_id)];
        params.extend(fields);
        let (status, body) = create_entry(&router, &params).await;
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

    // The positive control: the same protocols WITH what they need are accepted.
    let described = acl_body(&router, "nothing stored", &acl_id).await;
    let entries = reported_entries("nothing stored", &described);
    assert_eq!(
        entries.len(),
        2,
        "expected only the ACL's own catch-all pair after the refusals, got {entries:?}"
    );
}

/// A deleted network ACL stops existing: every read of its id answers the not-found refusal,
/// and the ACLs a delete may not touch are refused with the reason that forbids it.
#[tokio::test]
async fn a_deleted_network_acl_is_not_found_afterwards() {
    let (router, engine) = super::ec2_query::emulator();
    let (vpc_id, acl_id) = vpc_and_acl(&router, "10.5.0.0/16").await;

    // The default ACL of that VPC, which the delete may not take away, and a subnet of the
    // VPC — which is one half of the association the dependency refusal is about.
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "CreateSubnet",
        &[param("VpcId", &vpc_id), param("CidrBlock", "10.5.1.0/24")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSubnet: expected HTTP 200, got {status} with body {body}"
    );
    let subnet_id = element(&body, "subnetId")
        .unwrap_or_else(|| panic!("CreateSubnet answered no subnetId: {body}"));
    let (status, body) = super::ec2_query::call(
        &router,
        REGION,
        "DescribeNetworkAcls",
        &super::ec2_query::filters(&[("vpc-id", &[vpc_id.as_str()]), ("default", &["true"])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "describing the default ACL: expected HTTP 200, got {status} with body {body}"
    );
    let default_acl_id = element(&body, "networkAclId").unwrap_or_else(|| {
        panic!("expected the VPC's default network ACL in {body}")
    });

    // "You can't delete the default network ACL": the ACL a VPC comes with.
    let (status, body) = delete_acl(&router, &default_acl_id, false).await;
    assert_eq!(
        status, 400,
        "the default ACL: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "the default ACL: expected InvalidParameterValue, got {body}"
    );

    // "You can't delete the ACL if it's associated with any subnets": the association is the
    // ACL's dependency, cleared by associating that subnet with another ACL or by deleting
    // the subnet. Seeded through the store, because the operation that moves an association
    // between ACLs (`ReplaceNetworkAclAssociation`) is still a scaffold with its own open
    // issue.
    engine
        .state::<Ec2State>()
        .lock()
        .associate_subnet_with_network_acl(&subnet_id, &acl_id)
        .unwrap_or_else(|| panic!("the store associates {subnet_id} with {acl_id}"));
    let (status, body) = delete_acl(&router, &acl_id, false).await;
    assert_eq!(
        status, 400,
        "an associated ACL: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DependencyViolation",
        "an associated ACL: expected DependencyViolation, got {body}"
    );
    let (status, _body) = describe_one(&router, &acl_id).await;
    assert_eq!(status, 200, "a refused delete must leave the ACL alone");
    // With the association cleared the ACL is deletable again, which is what the rest of
    // this test exercises.
    engine
        .state::<Ec2State>()
        .lock()
        .disassociate_subnet_from_network_acls(&subnet_id);

    // An id that is not a network ACL id at all, and a well-formed one the account holds no
    // ACL for: told apart, before and after the delete.
    for (case, id, expected_code) in [
        ("not an id", "not-an-acl", "InvalidNetworkAclID.Malformed"),
        (
            "an id of no ACL",
            "acl-0123456789abcdef0",
            "InvalidNetworkAclID.NotFound",
        ),
    ] {
        let (status, body) = delete_acl(&router, id, false).await;
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

    // A dry run of a delete that would have succeeded leaves the ACL where it is.
    let (status, body) = delete_acl(&router, &acl_id, true).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );
    let (status, _body) = describe_one(&router, &acl_id).await;
    assert_eq!(status, 200, "a dry run must not delete the ACL");

    // The delete itself, and the read-back: the id names nothing afterwards.
    let (status, body) = delete_acl(&router, &acl_id, false).await;
    assert_eq!(
        status, 200,
        "the delete: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = describe_one(&router, &acl_id).await;
    assert_eq!(
        status, 400,
        "after the delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkAclID.NotFound",
        "after the delete: expected InvalidNetworkAclID.NotFound, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("The networkAcl ID '{acl_id}' does not exist"),
        "after the delete: expected the not-found message to name the id the request \
         sent, got {body}"
    );

    // And the delete is idempotent only in the sense every EC2 delete is: a second one is
    // refused, so a cleanup loop can tell the ACL is gone.
    let (status, body) = delete_acl(&router, &acl_id, false).await;
    assert_eq!(
        status, 400,
        "a second delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkAclID.NotFound",
        "a second delete: expected InvalidNetworkAclID.NotFound, got {body}"
    );

    // An entry cannot be created on an ACL that is gone either.
    let row = EntryCase {
        case: "on a deleted ACL",
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
    let (status, body) = create_entry(&router, &entry_params(&acl_id, &row)).await;
    assert_eq!(
        status, 400,
        "an entry on a deleted ACL: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidNetworkAclID.NotFound",
        "an entry on a deleted ACL: expected InvalidNetworkAclID.NotFound, got {body}"
    );
}

/// The form fields of a `CreateNetworkAclEntry` for one ingress allow rule over every IPv4
/// address, with `extra` naming whatever the row under test varies.
fn plain_entry_params(
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

/// The entry the ACL reports at `rule_number` in the direction `egress` names, if it holds
/// one — the read every row below checks its create against.
async fn entry_at(
    router: &MultiServiceRouter,
    case: &str,
    network_acl_id: &str,
    rule_number: i32,
    egress: bool,
) -> Option<ReportedEntry> {
    let described = acl_body(router, case, network_acl_id).await;
    reported_entries(case, &described).into_iter().find(|entry| {
        entry.rule_number == rule_number.to_string() && entry.egress == egress.to_string()
    })
}

/// One row of the protocol table: the `Protocol` field a create sends, and the protocol the
/// stored rule must report — or `None` when the value names no protocol at all.
#[derive(Debug)]
struct ProtocolCase {
    case: &'static str,
    rule_number: i32,
    requested: String,
    reported: Option<String>,
}

/// A rule's protocol is the NUMBER it names: every number the protocol field of the IP header
/// holds is accepted and stored as that number, whichever way the request spells it, and a
/// value that is no such number is refused with nothing stored.
///
/// The catalogue is the one the member publishes: "The protocol number. A value of -1 means
/// all protocols" (`CreateNetworkAclEntryRequest$Protocol`,
/// `smithy_query(query="doc",shape="CreateNetworkAclEntryRequest",member="Protocol")`), over a
/// protocol number no row of the create table above names (47, GRE), each bound of the
/// one-byte protocol field, and each value one past those bounds.
#[tokio::test]
async fn a_rules_protocol_is_the_number_it_names() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.6.0.0/16").await;

    let cases = vec![
        ProtocolCase {
            case: "every protocol",
            rule_number: 10,
            requested: ALL_IP_PROTOCOLS.to_string(),
            reported: Some(ALL_IP_PROTOCOLS.to_string()),
        },
        ProtocolCase {
            case: "the lowest protocol number",
            rule_number: 20,
            requested: "0".to_string(),
            reported: Some("0".to_string()),
        },
        ProtocolCase {
            case: "a protocol number no other row names",
            rule_number: 30,
            requested: "47".to_string(),
            reported: Some("47".to_string()),
        },
        ProtocolCase {
            case: "the highest protocol number",
            rule_number: 40,
            requested: MAX_PROTOCOL_NUMBER.to_string(),
            reported: Some(MAX_PROTOCOL_NUMBER.to_string()),
        },
        ProtocolCase {
            case: "the same number spelled with a leading zero",
            rule_number: 50,
            requested: "047".to_string(),
            reported: Some("47".to_string()),
        },
        ProtocolCase {
            case: "one past the highest protocol number",
            rule_number: 60,
            requested: (MAX_PROTOCOL_NUMBER + 1).to_string(),
            reported: None,
        },
        ProtocolCase {
            case: "one past the lowest, where only every-protocol sits below it",
            rule_number: 70,
            requested: "-2".to_string(),
            reported: None,
        },
        ProtocolCase {
            case: "a protocol name rather than a number",
            rule_number: 80,
            requested: "tcp".to_string(),
            reported: None,
        },
        ProtocolCase {
            case: "a protocol number with a fraction",
            rule_number: 90,
            requested: "6.0".to_string(),
            reported: None,
        },
    ];

    for row in &cases {
        let params = plain_entry_params(
            &acl_id,
            row.rule_number,
            false,
            vec![
                param("Protocol", &row.requested),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
        );
        let (status, body) = create_entry(&router, &params).await;
        let case = row.case;
        match row.reported.as_deref() {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                let stored = entry_at(&router, case, &acl_id, row.rule_number, false)
                    .await
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected the ACL to hold the created rule")
                    });
                assert_eq!(
                    stored.protocol, expected,
                    "case {case:?}: expected the rule to report protocol {expected}, \
                     got {stored:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterValue",
                    "case {case:?}: expected InvalidParameterValue, got {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains(&row.requested) && message.contains("protocol"),
                    "case {case:?}: expected the refusal to name parameter protocol and the \
                     value {requested:?} the request sent, got {message:?}",
                    requested = row.requested,
                );
                assert!(
                    entry_at(&router, case, &acl_id, row.rule_number, false)
                        .await
                        .is_none(),
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

/// One row of the address-family table: the range members a create sends, and the code its
/// refusal must carry.
#[derive(Debug)]
struct RangeCase {
    case: &'static str,
    rule_number: i32,
    cidr_block: Option<&'static str>,
    ipv6_cidr_block: Option<&'static str>,
    code: &'static str,
    /// The member the refusal must name, for a range that is not a CIDR block.
    names: Option<&'static str>,
}

/// A rule matches a range of ONE address family, and that range has to be a CIDR block: a
/// request naming both "the IPv4 network range" (`$CidrBlock`) and "the IPv6 network range"
/// (`$Ipv6CidrBlock`) is refused, as is a range of either family this is not notation for,
/// with nothing stored either way.
#[tokio::test]
async fn an_entry_matches_one_well_formed_range() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.7.0.0/16").await;

    let cases = [
        RangeCase {
            case: "a range of each family at once",
            rule_number: 100,
            cidr_block: Some("10.0.0.0/8"),
            ipv6_cidr_block: Some("2001:db8::/32"),
            code: "InvalidParameterCombination",
            names: None,
        },
        RangeCase {
            case: "an IPv4 prefix longer than an IPv4 address",
            rule_number: 110,
            cidr_block: Some("10.0.0.0/33"),
            ipv6_cidr_block: None,
            code: "InvalidParameterValue",
            names: Some("cidrBlock"),
        },
        RangeCase {
            case: "an IPv4 address with no prefix at all",
            rule_number: 120,
            cidr_block: Some("10.0.0.0"),
            ipv6_cidr_block: None,
            code: "InvalidParameterValue",
            names: Some("cidrBlock"),
        },
        RangeCase {
            case: "an IPv4 range whose address is not an address",
            rule_number: 130,
            cidr_block: Some("10.0.0.256/24"),
            ipv6_cidr_block: None,
            code: "InvalidParameterValue",
            names: Some("cidrBlock"),
        },
        RangeCase {
            case: "an IPv6 prefix longer than an IPv6 address",
            rule_number: 140,
            cidr_block: None,
            ipv6_cidr_block: Some("2001:db8::/129"),
            code: "InvalidParameterValue",
            names: Some("ipv6CidrBlock"),
        },
        RangeCase {
            case: "an IPv6 address with no prefix at all",
            rule_number: 150,
            cidr_block: None,
            ipv6_cidr_block: Some("2001:db8::"),
            code: "InvalidParameterValue",
            names: Some("ipv6CidrBlock"),
        },
    ];

    for row in &cases {
        let mut extra = vec![param("Protocol", ALL_IP_PROTOCOLS)];
        if let Some(cidr_block) = row.cidr_block {
            extra.push(param("CidrBlock", cidr_block));
        }
        if let Some(ipv6_cidr_block) = row.ipv6_cidr_block {
            extra.push(param("Ipv6CidrBlock", ipv6_cidr_block));
        }
        let params = plain_entry_params(&acl_id, row.rule_number, false, extra);
        let (status, body) = create_entry(&router, &params).await;
        let case = row.case;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {expected}, got {body}",
            expected = row.code
        );
        if let Some(member) = row.names {
            let message = error_message(&body);
            let requested = row
                .cidr_block
                .or(row.ipv6_cidr_block)
                .unwrap_or_default();
            assert!(
                message.contains(member) && message.contains(requested),
                "case {case:?}: expected the refusal to name parameter {member} and the \
                 value {requested:?} the request sent, got {message:?}"
            );
        }
        assert!(
            entry_at(&router, case, &acl_id, row.rule_number, false)
                .await
                .is_none(),
            "case {case:?}: a refused create must store nothing"
        );
    }

    // The positive control: the ACL still holds only the catch-all pair it was created with.
    let described = acl_body(&router, "nothing stored", &acl_id).await;
    let entries = reported_entries("nothing stored", &described);
    assert_eq!(
        entries,
        vec![catch_all_entry(true), catch_all_entry(false)],
        "expected only the ACL's own catch-all pair after the refusals, got {entries:?}"
    );
}

/// A network ACL holds at most the published number of rules, counted over the whole ACL: one
/// more is refused with the quota code and displaces nothing.
///
/// "Rules per network ACL" is 20 by default and adjustable to a maximum of 40 (Amazon VPC
/// quotas), named here through `MAX_NETWORK_ACL_ENTRIES`, so the table is that many rules
/// split over the two directions and then one rule too many in each direction.
#[tokio::test]
async fn an_acl_holds_at_most_the_published_number_of_rules() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.8.0.0/16").await;

    // Half the ceiling in each direction, so the count under test is the ACL's and not one
    // direction's.
    let per_direction = MAX_NETWORK_ACL_ENTRIES / 2;
    for index in 0..per_direction {
        for egress in [false, true] {
            let rule_number = i32::try_from(index).expect("a rule number fits in an i32") + 1;
            let params = plain_entry_params(
                &acl_id,
                rule_number,
                egress,
                vec![
                    param("Protocol", ALL_IP_PROTOCOLS),
                    param("CidrBlock", ALL_IPV4_ADDRESSES),
                ],
            );
            let (status, body) = create_entry(&router, &params).await;
            assert_eq!(
                status, 200,
                "rule {rule_number} (egress {egress}): expected HTTP 200 while the ACL has \
                 room, got {status} with body {body}"
            );
        }
    }

    let described = acl_body(&router, "a full ACL", &acl_id).await;
    let entries = reported_entries("a full ACL", &described);
    assert_eq!(
        entries.len(),
        MAX_NETWORK_ACL_ENTRIES + 2,
        "expected the ACL's {MAX_NETWORK_ACL_ENTRIES} rules beside its catch-all pair, \
         got {entries:?}"
    );

    // One rule too many, in either direction.
    for egress in [false, true] {
        let rule_number = i32::try_from(MAX_NETWORK_ACL_ENTRIES)
            .expect("a rule count fits in an i32")
            + 1;
        let params = plain_entry_params(
            &acl_id,
            rule_number,
            egress,
            vec![
                param("Protocol", ALL_IP_PROTOCOLS),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
        );
        let (status, body) = create_entry(&router, &params).await;
        assert_eq!(
            status, 400,
            "one rule too many (egress {egress}): expected HTTP 400, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NetworkAclEntryLimitExceeded",
            "one rule too many (egress {egress}): expected NetworkAclEntryLimitExceeded, \
             got {body}"
        );
        assert!(
            entry_at(&router, "one rule too many", &acl_id, rule_number, egress)
                .await
                .is_none(),
            "one rule too many (egress {egress}): a refused create must store nothing"
        );
    }

    let described = acl_body(&router, "a full ACL afterwards", &acl_id).await;
    let entries = reported_entries("a full ACL afterwards", &described);
    assert_eq!(
        entries.len(),
        MAX_NETWORK_ACL_ENTRIES + 2,
        "a refused create must displace nothing, got {entries:?}"
    );
}

/// A request with two faults is refused for the one the handler reads first: the ACL id's
/// FORM before any other member — an id that is not a network ACL id names nothing that could
/// exist — and the request's own members before the ACL is looked up.
#[tokio::test]
async fn a_request_with_two_faults_is_refused_for_the_fault_read_first() {
    let (router, _engine) = super::ec2_query::emulator();
    let (_vpc_id, acl_id) = vpc_and_acl(&router, "10.9.0.0/16").await;

    // A well-formed id of no ACL, and one that is not a network ACL id at all.
    let unknown_acl_id = "acl-0123456789abcdef0";
    let malformed_acl_id = "not-an-acl";
    let out_of_range_rule_number = *NETWORK_ACL_ENTRY_RULE_NUMBERS.end() + 1;

    for (case, network_acl_id, rule_number, protocol, expected_code) in [
        (
            "an id that is no id, beside a rule number out of range",
            malformed_acl_id,
            out_of_range_rule_number,
            ALL_IP_PROTOCOLS,
            "InvalidNetworkAclID.Malformed",
        ),
        (
            "an id of no ACL, beside a rule number out of range",
            unknown_acl_id,
            out_of_range_rule_number,
            ALL_IP_PROTOCOLS,
            "InvalidParameterValue",
        ),
        (
            "an id of no ACL, beside a protocol that is no protocol",
            unknown_acl_id,
            100,
            "tcp",
            "InvalidParameterValue",
        ),
        (
            "an id of no ACL, with every member in order",
            unknown_acl_id,
            100,
            ALL_IP_PROTOCOLS,
            "InvalidNetworkAclID.NotFound",
        ),
        (
            "a stored ACL, beside a rule number out of range",
            &acl_id,
            out_of_range_rule_number,
            ALL_IP_PROTOCOLS,
            "InvalidParameterValue",
        ),
    ] {
        let params = plain_entry_params(
            network_acl_id,
            rule_number,
            false,
            vec![
                param("Protocol", protocol),
                param("CidrBlock", ALL_IPV4_ADDRESSES),
            ],
        );
        let (status, body) = create_entry(&router, &params).await;
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

    // And nothing above reached the stored ACL.
    let described = acl_body(&router, "two faults", &acl_id).await;
    let entries = reported_entries("two faults", &described);
    assert_eq!(
        entries,
        vec![catch_all_entry(true), catch_all_entry(false)],
        "expected only the ACL's own catch-all pair after the refusals, got {entries:?}"
    );
}

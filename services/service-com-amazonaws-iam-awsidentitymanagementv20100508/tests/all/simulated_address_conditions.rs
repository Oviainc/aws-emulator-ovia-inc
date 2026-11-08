// SPDX-License-Identifier: CC0-1.0

//! A `Condition` clause over an ADDRESS, as `SimulateCustomPolicy` decides it.
//!
//! The rule under test: an IP-address condition operator holds when the address the request
//! carries for the clause's key lies in the network the clause states — so a grant guarded
//! by one applies to every address of the stated block and to no address outside it, and
//! `NotIpAddress` holds exactly where `IpAddress` does not. The address the simulation
//! carries is the one its `ContextEntries` supply: "the value (or values …) to provide to
//! the simulation when the key is referenced by a Condition element in an input policy"
//! (`ContextEntry$ContextKeyValues`), typed `ip` (`ContextKeyTypeEnum`).
//!
//! And, because a `Condition` block states a conjunction, a statement guarded by SEVERAL
//! keys applies only where EVERY one of them holds, whichever operators they are written
//! with.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: where the
//! supplied address lies relative to the stated block (inside, at either edge, one past
//! either edge, nowhere), how the block is written (a prefix, a bare address, a `/32`, the
//! whole space, several alternatives), which IP version each side names, whether the
//! operator is negated, and whether the request supplies the key at all. No single request
//! is the specification.
//!
//! The awsQuery harness, the decision values and the `(action, decision)` reader are the
//! ones the other simulation suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_simulation::{reported, ALLOWED, IMPLICIT_DENY};
use crate::simulated_resource_policy::{attribution, IDENTITY_POLICY};
use crate::user_lifecycle::{call, router};

/// The action every row asks about: one literal action, so the decision a row reports is
/// the clause's and nothing else's.
const ACTION: &str = "s3:ListBucket";

/// The condition key every row's clause is written over: "the full name of a condition
/// context key, including the service prefix. For example, `aws:SourceIp`"
/// (`ContextEntry$ContextKeyName`).
const SOURCE_IP: &str = "aws:SourceIp";

/// How a `MatchedStatements` entry identifies the single document these rows supply: the
/// member it arrived in and its position in that member, counted from one.
const FIRST_SUPPLIED_DOCUMENT: &str = "PolicyInputList.1";

/// One `SimulateCustomPolicy` call: the documents the request supplies, the action it asks
/// about and the condition values it carries.
async fn simulate(
    router: &MultiServiceRouter,
    documents: &[&str],
    context: &[(&str, &str, &str)],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = Vec::new();
    for (position, document) in (1..).zip(documents) {
        owned.push((
            format!("PolicyInputList.member.{position}"),
            (*document).to_string(),
        ));
    }
    owned.push(("ActionNames.member.1".to_string(), ACTION.to_string()));
    for (position, (key, value, kind)) in (1..).zip(context) {
        owned.push((
            format!("ContextEntries.member.{position}.ContextKeyName"),
            (*key).to_string(),
        ));
        owned.push((
            format!("ContextEntries.member.{position}.ContextKeyValues.member.1"),
            (*value).to_string(),
        ));
        owned.push((
            format!("ContextEntries.member.{position}.ContextKeyType"),
            (*kind).to_string(),
        ));
    }
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, "SimulateCustomPolicy", &params).await
}

/// A policy whose only statement grants [`ACTION`] on every resource, guarded by one
/// `Condition` clause: `operator` over [`SOURCE_IP`], stating `stated` exactly as written
/// (a quoted value or a JSON list of them).
fn guarded_grant(operator: &str, stated: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{ACTION}","Resource":"*","Condition":{{"{operator}":{{"{SOURCE_IP}":{stated}}}}}}}]}}"#
    )
}

/// The context keys the response reports as missing a value, in report order.
///
/// `MissingContextValues` carries a member LIST, so the span of each list is found first
/// and the members are read inside it — `<member>` is also how matched statements are
/// carried.
fn missing_keys(body: &str) -> Vec<String> {
    const OPEN: &str = "<MissingContextValues>";
    const CLOSE: &str = "</MissingContextValues>";
    const MEMBER_OPEN: &str = "<member>";
    const MEMBER_CLOSE: &str = "</member>";
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(OPEN) {
        let text = &rest[start + OPEN.len()..];
        let end = text
            .find(CLOSE)
            .unwrap_or_else(|| panic!("{OPEN} is never closed in {body}"));
        let mut list = &text[..end];
        while let Some(member) = list.find(MEMBER_OPEN) {
            let value = &list[member + MEMBER_OPEN.len()..];
            let close = value
                .find(MEMBER_CLOSE)
                .unwrap_or_else(|| panic!("{MEMBER_OPEN} is never closed in {body}"));
            found.push(value[..close].to_string());
            list = &value[close..];
        }
        rest = &text[end..];
    }
    found
}

/// One row of the address table: the clause the grant is guarded by, the address the
/// request supplies for its key, and the decision the simulation must report.
struct AddressCase {
    case: &'static str,
    /// The condition operator the clause is written with.
    operator: &'static str,
    /// The value the clause states, written as the policy carries it: a quoted address or
    /// block, or a JSON list of them.
    stated: &'static str,
    /// The address the request supplies for [`SOURCE_IP`], when it supplies one.
    supplied: Option<&'static str>,
    /// The decision reported for [`ACTION`].
    decision: &'static str,
    /// Whether the result must report [`SOURCE_IP`] as a key the decision needed a value
    /// for and the request supplied none.
    missing_source_ip: bool,
}

/// Rows spanning the readings an address gets against a stated network: inside the block,
/// at each of its edges, one past each edge, outside it altogether, against a bare address
/// and a `/32`, against the whole space, against one of several alternatives, across IP
/// versions, under the negated operator, and with the key withheld.
///
/// The blocks are documentation ranges (RFC 5737 for IPv4, RFC 3849 for IPv6) and the
/// private range of RFC 1918, so no row names an address that routes anywhere.
const ADDRESS_CASES: &[AddressCase] = &[
    AddressCase {
        case: "an address inside the stated block",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.113.5"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the block's own network address",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.113.0"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the block's last address",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.113.255"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the first address past the block",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.114.0"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the last address below the block",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.112.255"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "a single address stated with no prefix, matched",
        operator: "IpAddress",
        stated: r#""198.51.100.7""#,
        supplied: Some("198.51.100.7"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "a single address stated with no prefix, not matched",
        operator: "IpAddress",
        stated: r#""198.51.100.7""#,
        supplied: Some("198.51.100.8"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the same single address stated as a /32 block",
        operator: "IpAddress",
        stated: r#""198.51.100.7/32""#,
        supplied: Some("198.51.100.7"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the whole IPv4 space",
        operator: "IpAddress",
        stated: r#""0.0.0.0/0""#,
        supplied: Some("192.0.2.1"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "one of several stated blocks",
        operator: "IpAddress",
        stated: r#"["10.0.0.0/8","203.0.113.0/24"]"#,
        supplied: Some("10.1.2.3"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "none of several stated blocks",
        operator: "IpAddress",
        stated: r#"["10.0.0.0/8","203.0.113.0/24"]"#,
        supplied: Some("192.0.2.1"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "an IPv6 address inside the stated IPv6 block",
        operator: "IpAddress",
        stated: r#""2001:db8::/32""#,
        supplied: Some("2001:db8::1"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "an IPv6 address outside the stated IPv6 block",
        operator: "IpAddress",
        stated: r#""2001:db8::/32""#,
        supplied: Some("2001:db9::1"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "an IPv4 address against an IPv6 block",
        operator: "IpAddress",
        stated: r#""2001:db8::/32""#,
        supplied: Some("203.0.113.5"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "a supplied value that is no address at all",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("not-an-address"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the negated operator over an address outside the block",
        operator: "NotIpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("192.0.2.1"),
        decision: ALLOWED,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the negated operator over an address inside the block",
        operator: "NotIpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: Some("203.0.113.5"),
        decision: IMPLICIT_DENY,
        missing_source_ip: false,
    },
    AddressCase {
        case: "the key withheld",
        operator: "IpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: None,
        decision: IMPLICIT_DENY,
        missing_source_ip: true,
    },
    AddressCase {
        case: "the key withheld under the negated operator",
        operator: "NotIpAddress",
        stated: r#""203.0.113.0/24""#,
        supplied: None,
        decision: ALLOWED,
        missing_source_ip: false,
    },
];

/// RULE: an address condition operator is decided by the NETWORK the clause states — the
/// grant fires for every address the stated block contains and for no other, and the
/// negated spelling fires exactly where the positive one does not.
#[tokio::test]
async fn an_address_clause_is_decided_against_the_network_it_states() {
    let router = router();

    for case in ADDRESS_CASES {
        let document = guarded_grant(case.operator, case.stated);
        let context: Vec<(&str, &str, &str)> = case
            .supplied
            .map(|value| vec![(SOURCE_IP, value, "ip")])
            .unwrap_or_default();
        let (status, body) = simulate(&router, &[&document], &context).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            reported(&body),
            vec![(ACTION.to_string(), case.decision.to_string())],
            "case {:?}: expected {:?}, got {body}",
            case.case,
            case.decision
        );
        let missing = missing_keys(&body);
        let expected_missing = if case.missing_source_ip {
            vec![SOURCE_IP.to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            missing, expected_missing,
            "case {:?}: expected the missing keys {expected_missing:?}, got {missing:?}: \
             {body}",
            case.case
        );
        // A decision the supplied document granted is attributed to that document, by the
        // member it arrived in and its position there.
        let attributed = attribution(&body);
        let expected_attribution = if case.decision == ALLOWED {
            vec![(
                FIRST_SUPPLIED_DOCUMENT.to_string(),
                IDENTITY_POLICY.to_string(),
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            attributed, expected_attribution,
            "case {:?}: expected the matched statements {expected_attribution:?}, got \
             {attributed:?}: {body}",
            case.case
        );
    }
}

/// One row of the multi-key table: whether each of the two keys the one `Condition` block
/// states is supplied satisfyingly, and the decision that follows.
struct ConjunctionCase {
    case: &'static str,
    /// The value supplied for the string key, when the request supplies one.
    encryption: Option<&'static str>,
    /// The address supplied for [`SOURCE_IP`], when the request supplies one.
    address: Option<&'static str>,
    /// The decision reported for [`ACTION`].
    decision: &'static str,
    /// The keys the result must report as missing a value, named in sorted order: which
    /// clause of the block is read first is the document's business, not the rule's.
    missing: &'static [&'static str],
}

/// The string key the conjunction's other clause is written over, a service key of the
/// action's own service.
const ENCRYPTION_KEY: &str = "s3:x-amz-server-side-encryption";

/// The value that key's clause states.
const ENCRYPTION_VALUE: &str = "aws:kms";

/// The block the conjunction's address clause states.
const CONJUNCTION_BLOCK: &str = "10.0.0.0/24";

/// Rows spanning the four combinations of the two keys of one `Condition` block, plus the
/// two ways a key can fail to satisfy it: a value that does not match, and no value at all.
const CONJUNCTION_CASES: &[ConjunctionCase] = &[
    ConjunctionCase {
        case: "both keys supplied satisfyingly",
        encryption: Some(ENCRYPTION_VALUE),
        address: Some("10.0.0.9"),
        decision: ALLOWED,
        missing: &[],
    },
    ConjunctionCase {
        case: "the address outside the stated block",
        encryption: Some(ENCRYPTION_VALUE),
        address: Some("10.0.1.9"),
        decision: IMPLICIT_DENY,
        missing: &[],
    },
    ConjunctionCase {
        case: "the string key carrying another value",
        encryption: Some("AES256"),
        address: Some("10.0.0.9"),
        decision: IMPLICIT_DENY,
        missing: &[],
    },
    ConjunctionCase {
        case: "the address withheld",
        encryption: Some(ENCRYPTION_VALUE),
        address: None,
        decision: IMPLICIT_DENY,
        missing: &[SOURCE_IP],
    },
    ConjunctionCase {
        case: "the string key withheld",
        encryption: None,
        address: Some("10.0.0.9"),
        decision: IMPLICIT_DENY,
        missing: &[ENCRYPTION_KEY],
    },
    ConjunctionCase {
        case: "neither key supplied",
        encryption: None,
        address: None,
        decision: IMPLICIT_DENY,
        missing: &[SOURCE_IP, ENCRYPTION_KEY],
    },
];

/// RULE: a `Condition` block states a CONJUNCTION — the statement applies only where every
/// key it names holds — so a grant guarded by a string clause and an address clause fires
/// only for a request that satisfies both, whichever of them a request leaves unsatisfied.
#[tokio::test]
async fn every_key_of_one_condition_block_must_hold_for_the_grant_to_fire() {
    let router = router();
    let document = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{ACTION}","Resource":"*","Condition":{{"StringEquals":{{"{ENCRYPTION_KEY}":"{ENCRYPTION_VALUE}"}},"IpAddress":{{"{SOURCE_IP}":"{CONJUNCTION_BLOCK}"}}}}}}]}}"#
    );

    for case in CONJUNCTION_CASES {
        let mut context: Vec<(&str, &str, &str)> = Vec::new();
        if let Some(value) = case.encryption {
            context.push((ENCRYPTION_KEY, value, "string"));
        }
        if let Some(address) = case.address {
            context.push((SOURCE_IP, address, "ip"));
        }
        let (status, body) = simulate(&router, &[&document], &context).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            reported(&body),
            vec![(ACTION.to_string(), case.decision.to_string())],
            "case {:?}: expected {:?}, got {body}",
            case.case,
            case.decision
        );
        let mut missing = missing_keys(&body);
        missing.sort();
        let mut expected: Vec<String> =
            case.missing.iter().map(|key| (*key).to_string()).collect();
        expected.sort();
        assert_eq!(
            missing, expected,
            "case {:?}: expected the missing keys {expected:?}, got {missing:?}: {body}",
            case.case
        );
    }
}

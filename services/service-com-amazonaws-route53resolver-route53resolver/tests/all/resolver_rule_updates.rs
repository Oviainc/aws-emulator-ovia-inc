// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateResolverRule`.
//!
//! The rule these span: an update writes the settings the request names onto
//! the stored rule and leaves every other member at the value it already had
//! ("ResolverRuleId is required, and all other parameters are optional. If you
//! don't specify a parameter, it retains its current value"), so the rule a
//! reader answers afterwards is exactly the one the update wrote — the new
//! target addresses in the order the request listed them, the members it did
//! not name untouched, and the identity the rule was created with unchanged.
//! New settings a rule of that type may not carry are refused, and a refused
//! update writes nothing.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`] and the endpoint fixture is the one in
//! [`super::resolver_rule_deletion`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::MAX_TARGET_IPS;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, ok, parse_time, string_member, Emulator};
use super::resolver_rule_deletion::outbound_endpoint;

/// The Route 53 Resolver service error code for a rule id the account holds no
/// Resolver rule under, as [`super::resolver_rule_error_bodies`] records it for
/// the other operations that resolve a `ResolverRuleId`.
const CODE_RULE_NOT_FOUND: &str = "RSLVR-00703";

/// The members of `ResolverRule` an update never changes: the rule's identity,
/// the selector it was created with, and its ownership.
const UNCHANGED_MEMBERS: &[&str] = &[
    "Id",
    "Arn",
    "CreatorRequestId",
    "DomainName",
    "RuleType",
    "OwnerId",
    "ShareStatus",
    "CreationTime",
];

/// Create an INBOUND Resolver endpoint, which is NOT a direction a rule may
/// route its queries through.
///
/// Its addresses sit in subnets of the caller's VPC, registered in the account's
/// own Amazon EC2 store first, as [`outbound_endpoint`] does for its own.
async fn inbound_endpoint(router: &Emulator, creator: &str) -> String {
    router.hold_subnets(["subnet-00000000000000003", "subnet-00000000000000004"]);
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": "INBOUND",
            "SecurityGroupIds": ["sg-00000000000000002"],
            "IpAddresses": [
                { "SubnetId": "subnet-00000000000000003", "Ip": "10.0.2.10" },
                { "SubnetId": "subnet-00000000000000004", "Ip": "10.0.3.10" },
            ],
        }),
    )
    .await;
    string_member(
        created
            .get("ResolverEndpoint")
            .expect("the create answers a ResolverEndpoint"),
        "Id",
    )
}

/// Create one Resolver rule and return the `ResolverRule` the create answered.
async fn create_rule(router: &MultiServiceRouter, body: Value) -> Value {
    ok(router, "CreateResolverRule", body)
        .await
        .get("ResolverRule")
        .expect("the create answers a ResolverRule")
        .clone()
}

/// Read one Resolver rule back and return the `ResolverRule`.
async fn read_rule(router: &MultiServiceRouter, id: &str) -> Value {
    ok(router, "GetResolverRule", json!({ "ResolverRuleId": id }))
        .await
        .get("ResolverRule")
        .expect("the read answers a ResolverRule")
        .clone()
}

/// Update one Resolver rule and return the `ResolverRule` the update answered.
async fn update_rule(router: &MultiServiceRouter, id: &str, config: Value) -> Value {
    ok(
        router,
        "UpdateResolverRule",
        json!({ "ResolverRuleId": id, "Config": config }),
    )
    .await
    .get("ResolverRule")
    .expect("the update answers a ResolverRule")
    .clone()
}

/// A modelled `Rfc3339TimeString` as a comparable instant, so two timestamps are
/// ordered numerically rather than as text.
fn instant(case: &str, member: &str, rule: &Value) -> (i64, u32) {
    let parsed = parse_time(case, member, &string_member(rule, member));
    (parsed.secs(), parsed.subsec_nanos())
}

/// The target addresses the rule under test is created with: two IPv4
/// addresses, one taking the default port and one naming its own.
fn base_target_ips() -> Value {
    json!([{ "Ip": "192.0.2.1" }, { "Ip": "192.0.2.2", "Port": 5353 }])
}

/// The same two addresses as the service stores them, with the documented
/// defaults filled in.
fn base_stored_target_ips() -> Value {
    json!([
        { "Ip": "192.0.2.1", "Port": 53, "Protocol": "Do53" },
        { "Ip": "192.0.2.2", "Port": 5353, "Protocol": "Do53" },
    ])
}

/// One row of the update table: the `Config` an update carries and the rule the
/// store must hold afterwards.
struct UpdateCase {
    case: &'static str,
    /// The new settings. `{endpoint}` in `ResolverEndpointId` stands for the
    /// SECOND stored OUTBOUND endpoint, the one the rule does not route
    /// through yet.
    config: Value,
    /// The `Name` the rule must report afterwards.
    name: &'static str,
    /// The `TargetIps` it must report afterwards, written out per row rather
    /// than recomputed by the code under test.
    target_ips: Value,
    /// Whether the rule must now route through the second endpoint.
    endpoint_changed: bool,
    /// Whether this update changed anything at all, which is what decides
    /// whether `ModificationTime` may move.
    writes: bool,
}

/// An update writes the settings it names and nothing else: the members the
/// request carries are the rule's new values, every other member keeps the value
/// it had, and `GetResolverRule` answers the very same rule afterwards.
///
/// The table spans what the rule generalises over: which of the three settings
/// the request carries (none, each one alone, all three together), the number of
/// target addresses (fewer, as many, more than the rule held), their family
/// (IPv4, IPv6), their order (listed, reversed, rotated), and `Port`,
/// `Protocol` and `ServerNameIndication` named or defaulted.
#[tokio::test]
async fn an_update_writes_the_named_settings_and_leaves_the_rest() {
    let cases = [
        UpdateCase {
            case: "no setting at all: every member retains its current value",
            config: json!({}),
            name: "rule-under-test",
            target_ips: base_stored_target_ips(),
            endpoint_changed: false,
            writes: false,
        },
        UpdateCase {
            case: "Name alone",
            config: json!({ "Name": "renamed rule" }),
            name: "renamed rule",
            target_ips: base_stored_target_ips(),
            endpoint_changed: false,
            writes: true,
        },
        UpdateCase {
            case: "TargetIps alone: the same two addresses, reversed",
            config: json!({
                "TargetIps": [{ "Ip": "192.0.2.2", "Port": 5353 }, { "Ip": "192.0.2.1" }],
            }),
            name: "rule-under-test",
            target_ips: json!([
                { "Ip": "192.0.2.2", "Port": 5353, "Protocol": "Do53" },
                { "Ip": "192.0.2.1", "Port": 53, "Protocol": "Do53" },
            ]),
            endpoint_changed: false,
            writes: true,
        },
        UpdateCase {
            case: "TargetIps alone: one DoH address replacing two",
            config: json!({
                "TargetIps": [{
                    "Ip": "203.0.113.9",
                    "Port": 443,
                    "Protocol": "DoH",
                    "ServerNameIndication": "dns.example.net",
                }],
            }),
            name: "rule-under-test",
            target_ips: json!([{
                "Ip": "203.0.113.9",
                "Port": 443,
                "Protocol": "DoH",
                "ServerNameIndication": "dns.example.net",
            }]),
            endpoint_changed: false,
            writes: true,
        },
        UpdateCase {
            case: "TargetIps alone: four IPv6 addresses replacing two IPv4 ones",
            config: json!({
                "TargetIps": [
                    { "Ipv6": "2001:db8::4" },
                    { "Ipv6": "2001:db8::1", "Port": 5353 },
                    { "Ipv6": "2001:db8::3" },
                    { "Ipv6": "2001:db8::2" },
                ],
            }),
            name: "rule-under-test",
            target_ips: json!([
                { "Ipv6": "2001:db8::4", "Port": 53, "Protocol": "Do53" },
                { "Ipv6": "2001:db8::1", "Port": 5353, "Protocol": "Do53" },
                { "Ipv6": "2001:db8::3", "Port": 53, "Protocol": "Do53" },
                { "Ipv6": "2001:db8::2", "Port": 53, "Protocol": "Do53" },
            ]),
            endpoint_changed: false,
            writes: true,
        },
        UpdateCase {
            case: "ResolverEndpointId alone",
            config: json!({ "ResolverEndpointId": "{endpoint}" }),
            name: "rule-under-test",
            target_ips: base_stored_target_ips(),
            endpoint_changed: true,
            writes: true,
        },
        UpdateCase {
            case: "all three settings at once",
            config: json!({
                "Name": "fully-updated",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [
                    { "Ip": "198.51.100.3" },
                    { "Ip": "198.51.100.1", "Protocol": "Do53" },
                    { "Ip": "198.51.100.2", "Port": 5354 },
                ],
            }),
            name: "fully-updated",
            target_ips: json!([
                { "Ip": "198.51.100.3", "Port": 53, "Protocol": "Do53" },
                { "Ip": "198.51.100.1", "Port": 53, "Protocol": "Do53" },
                { "Ip": "198.51.100.2", "Port": 5354, "Protocol": "Do53" },
            ]),
            endpoint_changed: true,
            writes: true,
        },
    ];

    let router = emulator();
    let first_endpoint = outbound_endpoint(&router, "ep-update-one").await;
    let second_endpoint = outbound_endpoint(&router, "ep-update-two").await;

    for (index, expected) in cases.iter().enumerate() {
        let case = expected.case;
        // A fresh rule per row, so each row starts from the same rule and no
        // row depends on the one before it.
        let before = create_rule(
            &router,
            json!({
                "CreatorRequestId": format!("req-update-{index}"),
                "Name": "rule-under-test",
                "RuleType": "FORWARD",
                "DomainName": format!("update-{index}.example.com"),
                "ResolverEndpointId": first_endpoint,
                "TargetIps": base_target_ips(),
            }),
        )
        .await;
        let id = string_member(&before, "Id");

        // The reversed-list row names the addresses the rule already holds, in
        // the other order, so what it proves is the order and not the values.
        let mut config = expected.config.clone();
        if config.get("ResolverEndpointId").and_then(Value::as_str) == Some("{endpoint}") {
            config["ResolverEndpointId"] = json!(second_endpoint);
        }

        let answered = update_rule(&router, &id, config).await;
        let stored = read_rule(&router, &id).await;
        assert_eq!(
            answered, stored,
            "case {case:?}: the update must answer the rule a later read reports, got {answered} \
             and {stored}"
        );

        for member in UNCHANGED_MEMBERS {
            assert_eq!(
                stored.get(member),
                before.get(member),
                "case {case:?}: {member} must keep its value, got {stored}"
            );
        }
        assert_eq!(
            string_member(&stored, "Name"),
            expected.name,
            "case {case:?}: expected Name {:?}, got {stored}",
            expected.name
        );
        assert_eq!(
            stored.get("TargetIps"),
            Some(&expected.target_ips),
            "case {case:?}: expected TargetIps {}, got {stored}",
            expected.target_ips
        );
        let expected_endpoint = if expected.endpoint_changed {
            &second_endpoint
        } else {
            &first_endpoint
        };
        assert_eq!(
            string_member(&stored, "ResolverEndpointId"),
            *expected_endpoint,
            "case {case:?}: expected ResolverEndpointId {expected_endpoint:?}, got {stored}"
        );
        // The rule's settings are in effect when the call answers, so its
        // status is the settled one before and after.
        assert_eq!(
            string_member(&stored, "Status"),
            "COMPLETE",
            "case {case:?}: an updated rule must report the settled status, got {stored}"
        );

        let created_at = instant(case, "CreationTime", &stored);
        let modified_at = instant(case, "ModificationTime", &stored);
        assert!(
            modified_at >= created_at,
            "case {case:?}: ModificationTime must not precede CreationTime, got {stored}"
        );
        if expected.writes {
            assert!(
                modified_at >= instant(case, "ModificationTime", &before),
                "case {case:?}: ModificationTime must not move backwards, got {stored}"
            );
        } else {
            assert_eq!(
                string_member(&stored, "ModificationTime"),
                string_member(&before, "ModificationTime"),
                "case {case:?}: an update that changes nothing must not move \
                 ModificationTime, got {stored}"
            );
        }
    }
}

/// The target addresses are stored in the order the request listed them: a rule
/// forwards queries to the resolvers it was given, so the list a read reports is
/// neither sorted nor de-duplicated.
///
/// The rows are permutations of one address set — the order it was first stored
/// in, its reverse, a rotation and a single swap — plus the emulator's cap
/// listed in descending order, which is also the largest list one rule holds.
#[tokio::test]
async fn the_target_address_list_keeps_the_order_the_request_gave() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "ep-update-order").await;
    let rule = create_rule(
        &router,
        json!({
            "CreatorRequestId": "req-update-order",
            "Name": "order-preserved",
            "RuleType": "FORWARD",
            "DomainName": "order.example.com",
            "ResolverEndpointId": endpoint,
            "TargetIps": base_target_ips(),
        }),
    )
    .await;
    let id = string_member(&rule, "Id");

    let five: Vec<String> = (1..=5).map(|last| format!("192.0.2.{last}")).collect();
    let mut rows: Vec<(&str, Vec<String>)> = vec![
        ("the order the addresses are written in", five.clone()),
        (
            "the reverse of that order",
            five.iter().rev().cloned().collect(),
        ),
        (
            "the same addresses rotated by two",
            five[2..].iter().chain(five[..2].iter()).cloned().collect(),
        ),
        (
            "the same addresses with the first two swapped",
            vec![
                five[1].clone(),
                five[0].clone(),
                five[2].clone(),
                five[3].clone(),
                five[4].clone(),
            ],
        ),
        (
            "one address repeated, which is stored as listed",
            vec![five[0].clone(), five[0].clone(), five[1].clone()],
        ),
    ];
    // The emulator's cap on one rule's target-address list, listed from the
    // last address to the first.
    let capped: Vec<String> = (0..MAX_TARGET_IPS)
        .rev()
        .map(|index| format!("10.{}.{}.1", index / 256, index % 256))
        .collect();
    rows.push(("the emulator's cap, in descending order", capped));

    for (case, addresses) in rows {
        let targets: Vec<Value> = addresses.iter().map(|ip| json!({ "Ip": ip })).collect();
        let updated = update_rule(&router, &id, json!({ "TargetIps": targets })).await;
        let reported: Vec<String> = updated
            .get("TargetIps")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(|target| string_member(target, "Ip")).collect())
            .unwrap_or_default();
        assert_eq!(
            reported, addresses,
            "case {case:?}: expected the addresses in the order given, got {reported:?}"
        );
        // And the reader agrees with the writer.
        let stored = read_rule(&router, &id).await;
        assert_eq!(
            stored.get("TargetIps"),
            updated.get("TargetIps"),
            "case {case:?}: the read must report the list the update wrote, got {stored}"
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// Which rule the update addresses: one of the keys the test seeds —
    /// `"forward"`, `"system"`, `"delegate"` — or `"unknown"` for an id the
    /// store holds no rule under.
    rule: &'static str,
    /// The new settings. `{endpoint}` stands for the stored OUTBOUND endpoint
    /// the rule does not route through yet and `{inbound}` for the stored
    /// INBOUND one.
    config: Value,
    expected: &'static str,
}

/// New settings a rule may not carry are refused with the error the operation
/// models for that fault, and the rule is left exactly as it was.
///
/// The rows span the preconditions a rule write is held to: the rule has to
/// exist, the forwarding members the rule's TYPE admits, an endpoint that is a
/// stored OUTBOUND one, an address that names an address, one address family
/// per rule, and the bound on how many addresses one rule holds.
#[tokio::test]
async fn an_update_a_rule_may_not_carry_is_refused_and_writes_nothing() {
    let router = emulator();
    let first_endpoint = outbound_endpoint(&router, "ep-refuse-one").await;
    let second_endpoint = outbound_endpoint(&router, "ep-refuse-two").await;
    let inbound = inbound_endpoint(&router, "ep-refuse-inbound").await;

    let forward = create_rule(
        &router,
        json!({
            "CreatorRequestId": "req-refuse-forward",
            "Name": "refuse-forward",
            "RuleType": "FORWARD",
            "DomainName": "refuse-forward.example.com",
            "ResolverEndpointId": first_endpoint,
            "TargetIps": base_target_ips(),
        }),
    )
    .await;
    let system = create_rule(
        &router,
        json!({
            "CreatorRequestId": "req-refuse-system",
            "Name": "refuse-system",
            "RuleType": "SYSTEM",
            "DomainName": "refuse-system.example.com",
        }),
    )
    .await;
    let delegate = create_rule(
        &router,
        json!({
            "CreatorRequestId": "req-refuse-delegate",
            "Name": "refuse-delegate",
            "RuleType": "DELEGATE",
            "DomainName": "refuse-delegate.example.com",
            "DelegationRecord": "ns.example.com",
        }),
    )
    .await;

    let mut rows = vec![
        RefusalCase {
            case: "an id the store holds no rule under",
            rule: "unknown",
            config: json!({ "Name": "no-such-rule" }),
            expected: "ResourceNotFoundException",
        },
        RefusalCase {
            // "TargetIps is available only when the value of Rule type is
            // FORWARD."
            case: "target addresses for a SYSTEM rule",
            rule: "system",
            config: json!({ "TargetIps": [{ "Ip": "192.0.2.50" }] }),
            expected: "InvalidRequestException",
        },
        RefusalCase {
            case: "an endpoint for a SYSTEM rule",
            rule: "system",
            config: json!({ "ResolverEndpointId": "{endpoint}" }),
            expected: "InvalidRequestException",
        },
        RefusalCase {
            // "Delegate resolver rules need to specify a nameserver name."
            case: "target addresses for a DELEGATE rule",
            rule: "delegate",
            config: json!({ "TargetIps": [{ "Ip": "192.0.2.51" }] }),
            expected: "InvalidRequestException",
        },
        RefusalCase {
            case: "an endpoint that was never created",
            rule: "forward",
            config: json!({ "ResolverEndpointId": "rslvr-out-ffffffffffffffff" }),
            expected: "ResourceNotFoundException",
        },
        RefusalCase {
            // "The ID of the new outbound Resolver endpoint ..."
            case: "an INBOUND endpoint",
            rule: "forward",
            config: json!({ "ResolverEndpointId": "{inbound}" }),
            expected: "InvalidRequestException",
        },
        RefusalCase {
            // "You can specify either Ipv4 or Ipv6 addresses but not both in
            // the same rule."
            case: "target addresses mixing the two families",
            rule: "forward",
            config: json!({
                "TargetIps": [{ "Ip": "192.0.2.52" }, { "Ipv6": "2001:db8::52" }],
            }),
            expected: "InvalidParameterException",
        },
        RefusalCase {
            case: "a target address that names no address at all",
            rule: "forward",
            config: json!({ "TargetIps": [{ "Port": 53 }] }),
            expected: "InvalidParameterException",
        },
    ];
    // One target address past the emulator's cap.
    let past_cap: Vec<Value> = (0..=MAX_TARGET_IPS)
        .map(|index| json!({ "Ip": format!("10.{}.{}.2", index / 256, index % 256) }))
        .collect();
    rows.push(RefusalCase {
        case: "one target address past the emulator's cap",
        rule: "forward",
        config: json!({ "TargetIps": past_cap }),
        expected: "LimitExceededException",
    });

    for row in &rows {
        let case = row.case;
        let addressed = match row.rule {
            "system" => string_member(&system, "Id"),
            "delegate" => string_member(&delegate, "Id"),
            "unknown" => "rslvr-rr-ffffffffffffffff".to_string(),
            _ => string_member(&forward, "Id"),
        };
        let before = if row.rule == "unknown" {
            Value::Null
        } else {
            read_rule(&router, &addressed).await
        };

        let mut config = row.config.clone();
        match config.get("ResolverEndpointId").and_then(Value::as_str) {
            Some("{endpoint}") => config["ResolverEndpointId"] = json!(second_endpoint),
            Some("{inbound}") => config["ResolverEndpointId"] = json!(inbound),
            _ => {}
        }

        let (status, parsed) = call(
            &router,
            "UpdateResolverRule",
            json!({ "ResolverRuleId": addressed, "Config": config }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must answer HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains(row.expected),
            "case {case:?}: expected {}, got {parsed}",
            row.expected
        );

        if row.rule == "unknown" {
            // A rule the store does not hold is reported with the one
            // RSLVR-coded sentence every operation that resolves a
            // `ResolverRuleId` answers.
            let message = string_member(&parsed, "Message");
            assert_eq!(
                message,
                format!(
                    r#"[{CODE_RULE_NOT_FOUND}] Resolver rule with ID "{addressed}" does not exist."#
                ),
                "case {case:?}: expected the coded not-found sentence, got {message:?}"
            );
        } else {
            let after = read_rule(&router, &addressed).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must write nothing, got {after} after {before}"
            );
        }
    }

    // An empty target-address list is outside `TargetList`'s modelled length
    // (minimum 1), so the boundary refuses it before the handler sees it and
    // the rule is untouched.
    let forward_id = string_member(&forward, "Id");
    let before = read_rule(&router, &forward_id).await;
    let (status, parsed) = call(
        &router,
        "UpdateResolverRule",
        json!({ "ResolverRuleId": forward_id, "Config": { "TargetIps": [] } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an empty target-address list\": expected HTTP 400, got {status}: {parsed}"
    );
    assert_eq!(
        read_rule(&router, &forward_id).await,
        before,
        "case \"an empty target-address list\": the rule must be untouched"
    );
}

/// An update is visible to every reader of the rule, not only to
/// `GetResolverRule`: `ListResolverRules` reports the updated rule, and a filter
/// on the member that changed selects it by its NEW value and no longer by the
/// old one.
#[tokio::test]
async fn an_updated_rule_is_what_every_reader_reports() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "ep-update-list").await;
    let rule = create_rule(
        &router,
        json!({
            "CreatorRequestId": "req-update-list",
            "Name": "before-rename",
            "RuleType": "FORWARD",
            "DomainName": "list.example.com",
            "ResolverEndpointId": endpoint,
            "TargetIps": base_target_ips(),
        }),
    )
    .await;
    let id = string_member(&rule, "Id");

    let updated = update_rule(&router, &id, json!({ "Name": "after-rename" })).await;

    for (case, name, expected_count) in [
        ("the name the rule no longer carries", "before-rename", 0),
        ("the name the update wrote", "after-rename", 1),
    ] {
        let listed = ok(
            &router,
            "ListResolverRules",
            json!({ "Filters": [{ "Name": "Name", "Values": [name] }] }),
        )
        .await;
        let rules = listed
            .get("ResolverRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rules.len(),
            expected_count,
            "case {case:?}: expected {expected_count} rule(s), got {listed}"
        );
        if expected_count == 1 {
            assert_eq!(
                rules[0], updated,
                "case {case:?}: the listed rule must be the one the update wrote, got {listed}"
            );
        }
    }
}

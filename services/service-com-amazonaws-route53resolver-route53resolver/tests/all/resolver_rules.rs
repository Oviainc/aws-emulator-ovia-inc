// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver rule slice: `CreateResolverRule`,
//! `GetResolverRule`, `AssociateResolverRule` and `GetResolverRuleAssociation`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared
//! ones in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::{
    DNS_PORT, MAX_TARGET_IPS, PROVISIONING_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, without_status,
    ACCOUNT, REGION,
};

/// Create a Resolver endpoint of `direction` and return its `Id`, so a rule has
/// a real stored endpoint to route its queries through.
async fn endpoint_id(router: &MultiServiceRouter, creator: &str, direction: &str) -> String {
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": direction,
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": [
                { "SubnetId": format!("subnet-{creator}-a"), "Ip": "10.0.0.10" },
                { "SubnetId": format!("subnet-{creator}-b"), "Ip": "10.0.1.10" },
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

/// The `ResolverRule` of a create/get response.
fn rule(body: &Value) -> &Value {
    body.get("ResolverRule")
        .expect("the response carries a ResolverRule")
}

/// The `ResolverRuleAssociation` of an associate/get response.
fn association(body: &Value) -> &Value {
    body.get("ResolverRuleAssociation")
        .expect("the response carries a ResolverRuleAssociation")
}

/// Create a FORWARD rule through `endpoint`, and return its `Id`.
async fn forward_rule(
    router: &MultiServiceRouter,
    creator: &str,
    domain: &str,
    endpoint: &str,
) -> String {
    let created = ok(
        router,
        "CreateResolverRule",
        json!({
            "CreatorRequestId": creator,
            "Name": creator,
            "RuleType": "FORWARD",
            "DomainName": domain,
            "ResolverEndpointId": endpoint,
            "TargetIps": [{ "Ip": "192.0.2.10" }],
        }),
    )
    .await;
    string_member(rule(&created), "Id")
}

/// One row of the create table: a request, the `DomainName` the stored rule must
/// report, and the `TargetIps` it must report once the service has applied the
/// documented defaults.
struct CreateCase {
    case: &'static str,
    /// `{endpoint}` in `ResolverEndpointId` is replaced by the id of the stored
    /// OUTBOUND endpoint.
    body: Value,
    /// The request's `DomainName` as a fully-qualified DNS name, written out per
    /// row rather than recomputed by the code under test.
    domain_name: Option<&'static str>,
    target_ips: Option<Value>,
}

/// A created rule is persisted: every member of the response is computed from
/// the request plus the documented defaults, and `GetResolverRule` answers the
/// very same rule.
///
/// The table spans what the rule generalises over: `RuleType` (every value a
/// caller may create, out of the model's `RuleTypeOption` enum), the number of
/// target addresses, their family (IPv4, IPv6), `Port` and `Protocol` present
/// or defaulted, `Name` present or absent, and a `DomainName` written relative
/// or already fully qualified.
#[tokio::test]
async fn a_created_rule_is_read_back_from_state() {
    let cases = vec![
        CreateCase {
            case: "FORWARD, one IPv4 target with an explicit Port",
            body: json!({
                "CreatorRequestId": "req-forward-one",
                "Name": "forward-one",
                "RuleType": "FORWARD",
                "DomainName": "example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Ip": "192.0.2.10", "Port": 5353 }],
            }),
            domain_name: Some("example.com."),
            target_ips: Some(json!([
                { "Ip": "192.0.2.10", "Port": 5353, "Protocol": "Do53" }
            ])),
        },
        CreateCase {
            case: "FORWARD, three IPv4 targets with no Port",
            body: json!({
                "CreatorRequestId": "req-forward-three",
                "Name": "forward-three",
                "RuleType": "FORWARD",
                "DomainName": "corp.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [
                    { "Ip": "192.0.2.11" },
                    { "Ip": "192.0.2.12" },
                    { "Ip": "192.0.2.13" },
                ],
            }),
            domain_name: Some("corp.example.com."),
            target_ips: Some(json!([
                { "Ip": "192.0.2.11", "Port": DNS_PORT, "Protocol": "Do53" },
                { "Ip": "192.0.2.12", "Port": DNS_PORT, "Protocol": "Do53" },
                { "Ip": "192.0.2.13", "Port": DNS_PORT, "Protocol": "Do53" },
            ])),
        },
        CreateCase {
            case: "FORWARD, IPv6 target over DoH with a server name indication",
            body: json!({
                "CreatorRequestId": "req-forward-v6",
                "RuleType": "FORWARD",
                "DomainName": "v6.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{
                    "Ipv6": "2001:db8::53",
                    "Port": 443,
                    "Protocol": "DoH",
                    "ServerNameIndication": "doh.example.com",
                }],
            }),
            domain_name: Some("v6.example.com."),
            target_ips: Some(json!([{
                "Ipv6": "2001:db8::53",
                "Port": 443,
                "Protocol": "DoH",
                "ServerNameIndication": "doh.example.com",
            }])),
        },
        CreateCase {
            case: "SYSTEM, a subdomain Resolver processes itself",
            body: json!({
                "CreatorRequestId": "req-system",
                "Name": "system-apex",
                "RuleType": "SYSTEM",
                "DomainName": "apex.example.com",
            }),
            domain_name: Some("apex.example.com."),
            target_ips: None,
        },
        CreateCase {
            case: "DELEGATE, keyed on a delegation record",
            body: json!({
                "CreatorRequestId": "req-delegate",
                "RuleType": "DELEGATE",
                "DomainName": "delegated.example.com",
                "DelegationRecord": "ns.example.com",
                "Tags": [{ "Key": "team", "Value": "networking" }],
            }),
            domain_name: Some("delegated.example.com."),
            target_ips: None,
        },
        CreateCase {
            case: "FORWARD, a DomainName the request already wrote fully qualified",
            body: json!({
                "CreatorRequestId": "req-forward-fqdn",
                "RuleType": "FORWARD",
                "DomainName": "fqdn.example.com.",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Ip": "192.0.2.14" }],
            }),
            domain_name: Some("fqdn.example.com."),
            target_ips: Some(json!([
                { "Ip": "192.0.2.14", "Port": DNS_PORT, "Protocol": "Do53" }
            ])),
        },
    ];

    for CreateCase {
        case,
        body,
        domain_name,
        target_ips,
    } in cases
    {
        // Each row gets its own emulator, so the store holds exactly this row.
        let router = emulator();
        let endpoint = endpoint_id(&router, "ep-for-rules", "OUTBOUND").await;

        let mut request = body.clone();
        if request.get("ResolverEndpointId").is_some() {
            request["ResolverEndpointId"] = json!(endpoint);
        }

        let created = ok(&router, "CreateResolverRule", request.clone()).await;
        let made = rule(&created).clone();
        let id = string_member(&made, "Id");

        assert!(
            id.starts_with("rslvr-rr-"),
            "case {case:?}: a Resolver rule id is minted as rslvr-rr-<hex>, got {id:?}"
        );
        assert_eq!(
            string_member(&made, "Arn"),
            format!("arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-rule/{id}"),
            "case {case:?}: the Arn must be rooted in the signed region and this id, got {made}"
        );
        assert_eq!(
            string_member(&made, "Status"),
            "COMPLETE",
            "case {case:?}: the model's ResolverRuleStatus has no provisioning value, got {made}"
        );
        assert_eq!(
            string_member(&made, "ShareStatus"),
            "NOT_SHARED",
            "case {case:?}: a rule this account owns and has not shared, got {made}"
        );
        assert_eq!(
            string_member(&made, "OwnerId"),
            ACCOUNT,
            "case {case:?}: the owner is the emulator account, got {made}"
        );
        for member in ["CreatorRequestId", "RuleType", "Name", "DelegationRecord"] {
            assert_eq!(
                made.get(member),
                request.get(member),
                "case {case:?}: {member} must come back exactly as requested, got {made}"
            );
        }
        assert_eq!(
            made.get("DomainName").and_then(Value::as_str),
            domain_name,
            "case {case:?}: DomainName must be reported as a fully-qualified name, got {made}"
        );
        assert_eq!(
            made.get("TargetIps"),
            target_ips.as_ref(),
            "case {case:?}: TargetIps must be the request plus the documented defaults, got {made}"
        );
        let created_at = parse_time(case, "CreationTime", &string_member(&made, "CreationTime"));
        let modified_at =
            parse_time(case, "ModificationTime", &string_member(&made, "ModificationTime"));
        assert_eq!(
            created_at, modified_at,
            "case {case:?}: a rule nobody has updated carries one timestamp, got {made}"
        );

        // Round trip: the reader answers the record the create wrote, and a
        // rule's status is terminal, so the two responses are identical.
        let fetched = ok(&router, "GetResolverRule", json!({ "ResolverRuleId": id })).await;
        assert_eq!(
            rule(&fetched),
            &made,
            "case {case:?}: GetResolverRule must answer the stored rule, got {fetched}"
        );
    }
}

/// A retried create with the same `CreatorRequestId` resolves to the rule that
/// request already made: "CreatorRequestId ... allows failed requests to be
/// retried without the risk of running the operation twice."
#[tokio::test]
async fn a_repeated_creator_request_id_does_not_create_a_second_rule() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-retry", "OUTBOUND").await;
    let body = json!({
        "CreatorRequestId": "req-retried",
        "Name": "once-only",
        "RuleType": "FORWARD",
        "DomainName": "retry.example.com",
        "ResolverEndpointId": endpoint,
        "TargetIps": [{ "Ip": "192.0.2.20" }],
    });

    let first = ok(&router, "CreateResolverRule", body.clone()).await;
    let second = ok(&router, "CreateResolverRule", body).await;
    assert_eq!(
        rule(&first),
        rule(&second),
        "case \"retried create\": the retry must resolve to the rule the first call made, got {second}"
    );
}

/// The domain name a rule selects on is reported as a fully-qualified DNS name:
/// the requested name followed by exactly one root label (RFC 1035 §3.1), so
/// two requests that differ only in how they wrote the root label describe one
/// domain.
///
/// The table spans what the rule generalises over: the number of labels, a name
/// written relative, one already fully qualified, one carrying more than one
/// root label, and a name whose canonical form reaches `DomainName`'s published
/// maximum length (256, `smithy_query(query="shape",shape="DomainName")`).
#[tokio::test]
async fn a_rule_domain_name_is_reported_fully_qualified() {
    // Four maximum-length DNS labels (63 octets each) joined by dots: 255
    // characters, whose canonical form is exactly DomainName's 256-character
    // maximum.
    let longest = vec!["a".repeat(63); 4].join(".");
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a relative name",
            "example.com".to_string(),
            "example.com.".to_string(),
        ),
        (
            "a name already written fully qualified",
            "example.com.".to_string(),
            "example.com.".to_string(),
        ),
        (
            "a name written with more than one root label",
            "example.com..".to_string(),
            "example.com.".to_string(),
        ),
        (
            "a single label",
            "internal".to_string(),
            "internal.".to_string(),
        ),
        (
            "six labels",
            "a.b.c.d.e.example.com".to_string(),
            "a.b.c.d.e.example.com.".to_string(),
        ),
        (
            "a name whose canonical form is the longest DomainName",
            longest.clone(),
            format!("{longest}."),
        ),
    ];

    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-fqdn", "OUTBOUND").await;

    for (index, (case, requested, expected)) in cases.iter().enumerate() {
        let created = ok(
            &router,
            "CreateResolverRule",
            json!({
                "CreatorRequestId": format!("req-fqdn-{index}"),
                "RuleType": "FORWARD",
                "DomainName": requested,
                "ResolverEndpointId": endpoint,
                "TargetIps": [{ "Ip": "192.0.2.40" }],
            }),
        )
        .await;
        let made = rule(&created).clone();
        let actual = string_member(&made, "DomainName");
        assert_eq!(
            &actual, expected,
            "case {case:?}: the created rule must report {expected:?} for {requested:?}, got {actual:?}"
        );

        // The canonical name is what was stored, not a projection the create
        // alone applies.
        let id = string_member(&made, "Id");
        let fetched = ok(&router, "GetResolverRule", json!({ "ResolverRuleId": id })).await;
        let read = string_member(rule(&fetched), "DomainName");
        assert_eq!(
            &read, expected,
            "case {case:?}: GetResolverRule must report {expected:?} for {requested:?}, got {read:?}"
        );
    }
}

/// A request the documented semantics forbid is refused with the modelled
/// error, and never with a 501 or an empty success.
///
/// Each row cites the sentence it enforces; `{endpoint}` is the stored OUTBOUND
/// endpoint and `{inbound}` the stored INBOUND one.
#[tokio::test]
async fn a_rule_the_documentation_forbids_is_refused() {
    let cases = vec![
        (
            // "Currently, only Resolver can create rules that have a value of
            // RECURSIVE for RuleType."
            "RECURSIVE, which only Resolver may create",
            json!({
                "CreatorRequestId": "req-recursive",
                "RuleType": "RECURSIVE",
                "DomainName": "recursive.example.com",
            }),
            "InvalidRequestException",
        ),
        (
            // "TargetIps is available only when the value of Rule type is FORWARD."
            "SYSTEM with TargetIps",
            json!({
                "CreatorRequestId": "req-system-targets",
                "RuleType": "SYSTEM",
                "DomainName": "system.example.com",
                "TargetIps": [{ "Ip": "192.0.2.30" }],
            }),
            "InvalidRequestException",
        ),
        (
            // A rule type that names no target IPs has nothing to route
            // through an endpoint either, so the endpoint member is refused on
            // its own as well.
            "SYSTEM with a ResolverEndpointId",
            json!({
                "CreatorRequestId": "req-system-endpoint",
                "RuleType": "SYSTEM",
                "DomainName": "system-endpoint.example.com",
                "ResolverEndpointId": "{endpoint}",
            }),
            "InvalidRequestException",
        ),
        (
            "SYSTEM with both a ResolverEndpointId and TargetIps",
            json!({
                "CreatorRequestId": "req-system-endpoint-targets",
                "RuleType": "SYSTEM",
                "DomainName": "system-both.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Ip": "192.0.2.37" }],
            }),
            "InvalidRequestException",
        ),
        (
            "DELEGATE with TargetIps",
            json!({
                "CreatorRequestId": "req-delegate-targets",
                "RuleType": "DELEGATE",
                "DomainName": "delegate.example.com",
                "DelegationRecord": "ns.example.com",
                "TargetIps": [{ "Ip": "192.0.2.31" }],
            }),
            "InvalidRequestException",
        ),
        (
            // A FORWARD rule forwards "to the IP addresses that are specified
            // in TargetIps", so it cannot do without them.
            "FORWARD without TargetIps",
            json!({
                "CreatorRequestId": "req-forward-no-targets",
                "RuleType": "FORWARD",
                "DomainName": "forward.example.com",
                "ResolverEndpointId": "{endpoint}",
            }),
            "InvalidRequestException",
        ),
        (
            // "specifies which Resolver endpoint the queries pass through"
            "FORWARD without ResolverEndpointId",
            json!({
                "CreatorRequestId": "req-forward-no-endpoint",
                "RuleType": "FORWARD",
                "DomainName": "forward.example.com",
                "TargetIps": [{ "Ip": "192.0.2.32" }],
            }),
            "InvalidRequestException",
        ),
        (
            // "DNS queries for this domain name are forwarded to the IP
            // addresses that you specify in TargetIps."
            "FORWARD without DomainName",
            json!({
                "CreatorRequestId": "req-forward-no-domain",
                "RuleType": "FORWARD",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Ip": "192.0.2.33" }],
            }),
            "InvalidRequestException",
        ),
        (
            "SYSTEM without DomainName",
            json!({
                "CreatorRequestId": "req-system-no-domain",
                "RuleType": "SYSTEM",
            }),
            "InvalidRequestException",
        ),
        (
            // "Delegate resolver rules need to specify a nameserver name."
            "DELEGATE without DelegationRecord",
            json!({
                "CreatorRequestId": "req-delegate-no-record",
                "RuleType": "DELEGATE",
                "DomainName": "delegate.example.com",
            }),
            "InvalidRequestException",
        ),
        (
            // "The ID of the outbound Resolver endpoint that you want to use to
            // route DNS queries to the IP addresses that you specify in
            // TargetIps."
            "FORWARD through an INBOUND endpoint",
            json!({
                "CreatorRequestId": "req-forward-inbound",
                "RuleType": "FORWARD",
                "DomainName": "inbound.example.com",
                "ResolverEndpointId": "{inbound}",
                "TargetIps": [{ "Ip": "192.0.2.34" }],
            }),
            "InvalidRequestException",
        ),
        (
            "FORWARD through an endpoint that was never created",
            json!({
                "CreatorRequestId": "req-forward-unknown-endpoint",
                "RuleType": "FORWARD",
                "DomainName": "unknown.example.com",
                "ResolverEndpointId": "rslvr-out-ffffffffffffffff",
                "TargetIps": [{ "Ip": "192.0.2.35" }],
            }),
            "ResourceNotFoundException",
        ),
        (
            // "You can specify either Ipv4 or Ipv6 addresses but not both in
            // the same rule."
            "FORWARD mixing IPv4 and IPv6 targets",
            json!({
                "CreatorRequestId": "req-forward-mixed",
                "RuleType": "FORWARD",
                "DomainName": "mixed.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [
                    { "Ip": "192.0.2.36" },
                    { "Ipv6": "2001:db8::36" },
                ],
            }),
            "InvalidParameterException",
        ),
        (
            "FORWARD with a target address that names no address at all",
            json!({
                "CreatorRequestId": "req-forward-empty-target",
                "RuleType": "FORWARD",
                "DomainName": "empty.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Port": 53 }],
            }),
            "InvalidParameterException",
        ),
    ];

    let router = emulator();
    let outbound = endpoint_id(&router, "ep-outbound", "OUTBOUND").await;
    let inbound = endpoint_id(&router, "ep-inbound", "INBOUND").await;

    for (case, body, expected) in cases {
        let mut request = body.clone();
        if request.get("ResolverEndpointId").and_then(Value::as_str) == Some("{endpoint}") {
            request["ResolverEndpointId"] = json!(outbound);
        }
        if request.get("ResolverEndpointId").and_then(Value::as_str) == Some("{inbound}") {
            request["ResolverEndpointId"] = json!(inbound);
        }
        expect_error(&router, "CreateResolverRule", request, case, expected).await;

        // The refusal wrote nothing: the rule the request would have made is
        // not readable afterwards. Nothing can be read by `CreatorRequestId`,
        // so the retry path is what proves it — a stored rule would come back
        // instead of the same refusal.
        let mut retried = body.clone();
        if retried.get("ResolverEndpointId").and_then(Value::as_str) == Some("{endpoint}") {
            retried["ResolverEndpointId"] = json!(outbound);
        }
        if retried.get("ResolverEndpointId").and_then(Value::as_str) == Some("{inbound}") {
            retried["ResolverEndpointId"] = json!(inbound);
        }
        expect_error(&router, "CreateResolverRule", retried, case, expected).await;
    }
}

/// The target-address list one rule can hold is bounded: `TargetList` carries no
/// modelled maximum, so a create past the emulator's cap is refused with
/// `LimitExceededException` and stores nothing, while the cap itself is served.
#[tokio::test]
async fn the_target_address_list_one_rule_can_hold_is_bounded() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-targets", "OUTBOUND").await;

    for (case, count, stored) in [
        ("one target, the modelled minimum", 1, true),
        ("the emulator's cap", MAX_TARGET_IPS, true),
        ("one past the cap", MAX_TARGET_IPS + 1, false),
    ] {
        let targets: Vec<Value> = (0..count)
            .map(|index| json!({ "Ip": format!("10.{}.{}.1", index / 256, index % 256) }))
            .collect();
        let body = json!({
            "CreatorRequestId": format!("req-targets-{count}"),
            "RuleType": "FORWARD",
            "DomainName": "targets.example.com",
            "ResolverEndpointId": endpoint,
            "TargetIps": targets,
        });

        if stored {
            let created = ok(&router, "CreateResolverRule", body).await;
            let actual = rule(&created)
                .get("TargetIps")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                actual, count,
                "case {case:?}: every requested target address must be stored, got {actual}"
            );
        } else {
            expect_error(
                &router,
                "CreateResolverRule",
                body,
                case,
                "LimitExceededException",
            )
            .await;
        }
    }
}

/// One row of the association table.
struct AssociationCase {
    case: &'static str,
    /// Which seeded rule the association names.
    rule_index: usize,
    vpc_id: &'static str,
    name: Option<&'static str>,
}

/// An association is persisted under the id `AssociateResolverRule` minted, and
/// `GetResolverRuleAssociation` answers exactly that record — "information about
/// the Resolver rule association that you specified".
///
/// The table spans what the rule generalises over: one rule associated with
/// several VPCs, one VPC carrying several rules, and the optional `Name` present
/// or absent.
#[tokio::test]
async fn an_association_is_read_back_by_its_id() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-assoc", "OUTBOUND").await;
    let rules = [
        forward_rule(&router, "rule-alpha", "alpha.example.com", &endpoint).await,
        forward_rule(&router, "rule-beta", "beta.example.com", &endpoint).await,
    ];

    let cases = vec![
        AssociationCase {
            case: "first rule, first VPC, named",
            rule_index: 0,
            vpc_id: "vpc-0000000000000000a",
            name: Some("alpha-in-a"),
        },
        AssociationCase {
            case: "first rule, second VPC, no Name",
            rule_index: 0,
            vpc_id: "vpc-0000000000000000b",
            name: None,
        },
        AssociationCase {
            case: "second rule, first VPC, named",
            rule_index: 1,
            vpc_id: "vpc-0000000000000000a",
            name: Some("beta-in-a"),
        },
    ];

    // Associate every row first, so each read below has to pick its own record
    // out of a store holding all of them.
    let mut made: Vec<(&str, String, Value)> = Vec::new();
    for AssociationCase {
        case,
        rule_index,
        vpc_id,
        name,
    } in &cases
    {
        let mut body = json!({
            "ResolverRuleId": rules[*rule_index],
            "VPCId": vpc_id,
        });
        if let Some(name) = name {
            body["Name"] = json!(name);
        }

        let created = ok(&router, "AssociateResolverRule", body).await;
        let record = association(&created).clone();
        let id = string_member(&record, "Id");
        assert!(
            id.starts_with("rslvr-rrassoc-"),
            "case {case:?}: an association id is minted as rslvr-rrassoc-<hex>, got {id:?}"
        );
        assert_eq!(
            string_member(&record, "Status"),
            "CREATING",
            "case {case:?}: a new association reports the transient status, got {record}"
        );
        made.push((case, id, record));
    }

    for (index, (case, id, _)) in made.iter().enumerate() {
        for (other_index, (_, other_id, _)) in made.iter().enumerate() {
            assert!(
                index == other_index || id != other_id,
                "case {case:?}: every association gets its own id, got {id:?} twice"
            );
        }
    }

    // The clock owes each association its move to COMPLETE; after it, a read
    // answers the stored record.
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    for ((case, id, created), expected) in made.iter().zip(&cases) {
        let fetched = ok(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": id }),
        )
        .await;
        let read = association(&fetched).clone();

        assert_eq!(
            without_status(&read),
            without_status(created),
            "case {case:?}: the read must answer the record the associate wrote, got {fetched}"
        );
        assert_eq!(
            string_member(&read, "ResolverRuleId"),
            rules[expected.rule_index],
            "case {case:?}: the association names the rule it was made for, got {read}"
        );
        assert_eq!(
            string_member(&read, "VPCId"),
            expected.vpc_id,
            "case {case:?}: the association names the VPC it was made for, got {read}"
        );
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            expected.name,
            "case {case:?}: Name comes back exactly as requested, got {read}"
        );
        assert_eq!(
            string_member(&read, "Status"),
            "COMPLETE",
            "case {case:?}: the association settles on its own, got {read}"
        );

        // A read changes nothing for the next caller.
        let again = ok(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": id }),
        )
        .await;
        assert_eq!(
            association(&again),
            &read,
            "case {case:?}: a second read must answer the same association, got {again}"
        );
    }
}

/// The transient `CREATING` is owed to the clock, not to a read: a caller that
/// reads immediately still sees it, and nobody has to ask for it to settle.
#[tokio::test]
async fn the_creating_status_of_an_association_settles_without_anyone_asking() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-settle", "OUTBOUND").await;
    let rule_id = forward_rule(&router, "rule-settle", "settle.example.com", &endpoint).await;

    let created = ok(
        &router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": "vpc-0000000000000settle" }),
    )
    .await;
    let id = string_member(association(&created), "Id");

    let immediate = ok(
        &router,
        "GetResolverRuleAssociation",
        json!({ "ResolverRuleAssociationId": id }),
    )
    .await;
    assert_eq!(
        string_member(association(&immediate), "Status"),
        "CREATING",
        "case \"immediately after associate\": the transient status must still be reported, got {immediate}"
    );

    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;

    let settled = ok(
        &router,
        "GetResolverRuleAssociation",
        json!({ "ResolverRuleAssociationId": id }),
    )
    .await;
    assert_eq!(
        string_member(association(&settled), "Status"),
        "COMPLETE",
        "case \"after the provisioning span\": the association must have settled, got {settled}"
    );
}

/// An association id the store holds nothing under is
/// `ResourceNotFoundException` — never a 501 and never an empty success — and it
/// names the resource type that does not exist.
///
/// The ids span `ResourceId`'s published bounds (length 1..64,
/// `smithy_query(query="shape",shape="ResourceId")`), an id of no Resolver form,
/// and ids that DO name stored resources of the other two kinds, so the lookup
/// is per collection and not "any id this emulator ever minted".
#[tokio::test]
async fn an_unknown_association_id_is_not_found() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-missing", "OUTBOUND").await;
    let rule_id = forward_rule(&router, "rule-missing", "missing.example.com", &endpoint).await;
    // One real association exists, so "not found" is about the id and not about
    // an empty store.
    ok(
        &router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": "vpc-00000000000000real" }),
    )
    .await;

    let unknown = vec![
        ("one character, the minimum ResourceId", "x".to_string()),
        ("sixty-four characters, the maximum ResourceId", "a".repeat(64)),
        (
            "a well-formed association id that was never minted",
            "rslvr-rrassoc-ffffffffffffffff".to_string(),
        ),
        ("the id of a stored Resolver rule", rule_id.clone()),
        ("the id of a stored Resolver endpoint", endpoint.clone()),
    ];

    for (case, id) in unknown {
        let (status, parsed) = call(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a missing association answers HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: a missing association answers ResourceNotFoundException, got {parsed}"
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverRuleAssociation",
            "case {case:?}: the error names the type of resource that doesn't exist, got {parsed}"
        );
    }

    // The same, for the rule reader: an association id is not a rule id.
    for (case, id) in [
        ("a rule id that was never minted", "rslvr-rr-ffffffffffffffff"),
        ("the id of a stored Resolver endpoint", endpoint.as_str()),
    ] {
        let (status, parsed) =
            call(&router, "GetResolverRule", json!({ "ResolverRuleId": id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a missing rule answers HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: a missing rule answers ResourceNotFoundException, got {parsed}"
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverRule",
            "case {case:?}: the error names the type of resource that doesn't exist, got {parsed}"
        );
    }
}

/// An association names a rule that has to exist, and one rule is associated
/// with one VPC at most once: the second attempt is refused and leaves the first
/// association exactly as it was.
#[tokio::test]
async fn associating_is_refused_without_a_rule_and_refused_twice_over() {
    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-dup", "OUTBOUND").await;
    let rule_id = forward_rule(&router, "rule-dup", "dup.example.com", &endpoint).await;

    for (case, rule_name) in [
        ("a rule id that was never minted", "rslvr-rr-ffffffffffffffff"),
        ("the id of a stored Resolver endpoint", endpoint.as_str()),
    ] {
        expect_error(
            &router,
            "AssociateResolverRule",
            json!({ "ResolverRuleId": rule_name, "VPCId": "vpc-0000000000000000c" }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    let vpc = "vpc-0000000000000000d";
    let created = ok(
        &router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": vpc, "Name": "first" }),
    )
    .await;
    let first = association(&created).clone();
    let id = string_member(&first, "Id");

    for (case, name) in [
        ("the same pair again", None),
        ("the same pair under another name", Some("second")),
    ] {
        let mut body = json!({ "ResolverRuleId": rule_id, "VPCId": vpc });
        if let Some(name) = name {
            body["Name"] = json!(name);
        }
        expect_error(
            &router,
            "AssociateResolverRule",
            body,
            case,
            "ResourceExistsException",
        )
        .await;
    }

    let fetched = ok(
        &router,
        "GetResolverRuleAssociation",
        json!({ "ResolverRuleAssociationId": id }),
    )
    .await;
    assert_eq!(
        without_status(association(&fetched)),
        without_status(&first),
        "case \"after the refused duplicates\": the stored association must be untouched, got {fetched}"
    );

    // The same rule in a DIFFERENT VPC is a different association, so it is
    // allowed and gets its own id.
    let other = ok(
        &router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": "vpc-0000000000000000e" }),
    )
    .await;
    let other_id = string_member(association(&other), "Id");
    assert_ne!(
        other_id, id,
        "case \"the same rule in another VPC\": the association must be its own record, got {other}"
    );
}

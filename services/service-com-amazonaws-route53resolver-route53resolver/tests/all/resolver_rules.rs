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
    Emulator, ACCOUNT, REGION,
};

/// Create a Resolver endpoint of `direction` and return its `Id`, so a rule has
/// a real stored endpoint to route its queries through.
///
/// The endpoint's addresses are placed in subnets of the caller's VPC, which
/// `CreateResolverEndpoint` resolves against the account's own Amazon EC2 store,
/// so the subnets this names are registered there first.
async fn endpoint_id(router: &Emulator, creator: &str, direction: &str) -> String {
    let subnets = [format!("subnet-{creator}-a"), format!("subnet-{creator}-b")];
    router.hold_subnets(&subnets);
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": direction,
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": [
                { "SubnetId": subnets[0], "Ip": "10.0.0.10" },
                { "SubnetId": subnets[1], "Ip": "10.0.1.10" },
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

/// The service error code of the "a forwarding rule must say where to forward"
/// fault, as recorded on AWS for the rule write path. It is the per-fault
/// discriminator: the HTTP status (400) and the exception shape name are shared
/// by every other `InvalidRequestException` fault of the same operation.
const RSLVR_CODE_FORWARD_REQUIRES_TARGET: &str = "RSLVR-00706";

/// The service error code of its mirror image, the "only a forwarding rule may
/// say where to forward" fault. Carried for the same reason.
const RSLVR_CODE_SYSTEM_FORBIDS_TARGET: &str = "RSLVR-00707";

/// A rule's forwarding target — the Resolver endpoint to forward through and the
/// IP addresses to forward to — is ONE precondition of the rule type, and a
/// request that breaks it is refused with Route 53 Resolver's own service error
/// code inside the message, whichever of the two members is at fault.
///
/// The table spans what the rule generalises over: the rule type on both sides
/// of the precondition (a type that forwards, a type Resolver answers itself),
/// and which part of the forwarding target the request got wrong — the endpoint,
/// the target addresses, or both. The requests the precondition admits are
/// checked too, so the refusal is keyed on the precondition and not on "a rule
/// of this type was asked for".
#[tokio::test]
async fn a_rule_forwarding_target_precondition_is_refused_with_its_rslvr_error_code() {
    let forward_requires = format!(
        "[{RSLVR_CODE_FORWARD_REQUIRES_TARGET}] Resolver endpoint ID and target IP are required for FORWARD type resolver rule."
    );
    let system_forbids = format!(
        "[{RSLVR_CODE_SYSTEM_FORBIDS_TARGET}] Cannot specify resolver endpoint ID and target IP for SYSTEM type resolver rule."
    );

    let router = emulator();
    let endpoint = endpoint_id(&router, "ep-precondition", "OUTBOUND").await;

    let cases: Vec<(&str, Value, &String)> = vec![
        (
            "FORWARD naming neither the endpoint nor the target addresses",
            json!({
                "RuleType": "FORWARD",
                "DomainName": "neither.example.com",
            }),
            &forward_requires,
        ),
        (
            "FORWARD naming target addresses but no endpoint",
            json!({
                "RuleType": "FORWARD",
                "DomainName": "no-endpoint.example.com",
                "TargetIps": [{ "Ip": "10.1.2.6", "Port": 53 }],
            }),
            &forward_requires,
        ),
        (
            "FORWARD naming an endpoint but no target addresses",
            json!({
                "RuleType": "FORWARD",
                "DomainName": "no-targets.example.com",
                "ResolverEndpointId": endpoint.clone(),
            }),
            &forward_requires,
        ),
        (
            "FORWARD naming several target addresses but no endpoint",
            json!({
                "RuleType": "FORWARD",
                "DomainName": "many-no-endpoint.example.com",
                "TargetIps": [
                    { "Ipv6": "2001:db8::1" },
                    { "Ipv6": "2001:db8::2" },
                ],
            }),
            &forward_requires,
        ),
        (
            "SYSTEM naming target addresses",
            json!({
                "RuleType": "SYSTEM",
                "DomainName": "system-targets.example.com",
                "TargetIps": [{ "Ip": "10.1.2.6", "Port": 53 }],
            }),
            &system_forbids,
        ),
        (
            "SYSTEM naming an endpoint",
            json!({
                "RuleType": "SYSTEM",
                "DomainName": "system-endpoint.example.com",
                "ResolverEndpointId": endpoint.clone(),
            }),
            &system_forbids,
        ),
        (
            "SYSTEM naming both",
            json!({
                "RuleType": "SYSTEM",
                "DomainName": "system-both.example.com",
                "ResolverEndpointId": endpoint.clone(),
                "TargetIps": [{ "Ip": "10.1.2.7" }],
            }),
            &system_forbids,
        ),
    ];

    for (index, (case, body, expected)) in cases.iter().enumerate() {
        let mut request = body.clone();
        request["CreatorRequestId"] = json!(format!("req-precondition-{index}"));
        let (status, parsed) = call(&router, "CreateResolverRule", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        assert_eq!(
            &message, *expected,
            "case {case:?}: expected Message {expected:?}, got {message:?}"
        );
    }

    // The precondition itself, not the rule type, is what decides: a FORWARD
    // rule that names both parts of its forwarding target and a SYSTEM rule that
    // names neither are both served.
    for (case, body) in [
        (
            "FORWARD naming the endpoint and the target addresses",
            json!({
                "CreatorRequestId": "req-precondition-forward-ok",
                "RuleType": "FORWARD",
                "DomainName": "served.example.com",
                "ResolverEndpointId": endpoint.clone(),
                "TargetIps": [{ "Ip": "10.1.2.8" }],
            }),
        ),
        (
            "SYSTEM naming no forwarding target at all",
            json!({
                "CreatorRequestId": "req-precondition-system-ok",
                "RuleType": "SYSTEM",
                "DomainName": "served-system.example.com",
            }),
        ),
    ] {
        let created = ok(&router, "CreateResolverRule", body).await;
        assert!(
            !string_member(rule(&created), "Id").is_empty(),
            "case {case:?}: the rule must be created, got {created}"
        );
    }

    // The codes are per-fault discriminators, so another fault of the same
    // operation (and a read of a rule that does not exist) carries neither.
    let other_faults = [
        (
            "GetResolverRule for an id that was never minted",
            "GetResolverRule",
            json!({ "ResolverRuleId": "rslvr-rr-ffffffffffffffff" }),
        ),
        (
            "DELEGATE carrying target addresses",
            "CreateResolverRule",
            json!({
                "CreatorRequestId": "req-precondition-delegate",
                "RuleType": "DELEGATE",
                "DomainName": "delegate.example.com",
                "DelegationRecord": "ns.example.com",
                "TargetIps": [{ "Ip": "10.1.2.9" }],
            }),
        ),
    ];
    for (case, operation, body) in other_faults {
        let (_, parsed) = call(&router, operation, body).await;
        let message = string_member(&parsed, "Message");
        assert!(
            !message.contains(RSLVR_CODE_FORWARD_REQUIRES_TARGET)
                && !message.contains(RSLVR_CODE_SYSTEM_FORBIDS_TARGET),
            "case {case:?}: another fault must not carry a forwarding-target code, got {message:?}"
        );
    }
}

/// Create the rule table `ListResolverRules` is exercised over, and return
/// `(outbound endpoint, second outbound endpoint, rule ids in creation order)`.
///
/// The rules span what a list query selects on: the rule type, the endpoint a
/// forwarding rule routes through, `Name` present or absent, and a `DomainName`
/// written relative or already fully qualified.
async fn seed_rules(router: &Emulator) -> (String, String, Vec<String>) {
    let first_endpoint = endpoint_id(router, "ep-list-one", "OUTBOUND").await;
    let second_endpoint = endpoint_id(router, "ep-list-two", "OUTBOUND").await;

    let bodies = vec![
        json!({
            "CreatorRequestId": "req-list-forward-one",
            "Name": "list-forward-one",
            "RuleType": "FORWARD",
            "DomainName": "one.list.example.com",
            "ResolverEndpointId": first_endpoint.clone(),
            "TargetIps": [{ "Ip": "192.0.2.50" }],
        }),
        json!({
            "CreatorRequestId": "req-list-forward-two",
            "RuleType": "FORWARD",
            "DomainName": "two.list.example.com.",
            "ResolverEndpointId": second_endpoint.clone(),
            "TargetIps": [{ "Ipv6": "2001:db8::50" }],
        }),
        json!({
            "CreatorRequestId": "req-list-system",
            "Name": "list-system",
            "RuleType": "SYSTEM",
            "DomainName": "apex.one.list.example.com",
        }),
        json!({
            "CreatorRequestId": "req-list-delegate",
            "Name": "list-delegate",
            "RuleType": "DELEGATE",
            "DomainName": "delegated.list.example.com",
            "DelegationRecord": "ns.list.example.com",
        }),
    ];

    let mut ids = Vec::new();
    for body in bodies {
        let created = ok(router, "CreateResolverRule", body).await;
        ids.push(string_member(rule(&created), "Id"));
    }
    (first_endpoint, second_endpoint, ids)
}

/// The `Id`s of the rules one `ListResolverRules` response carries.
///
/// `pub` because [`super::resolver_rule_listing_bounds`] reads the same pages
/// and so needs the same projection.
pub fn listed_ids(body: &Value) -> Vec<String> {
    body.get("ResolverRules")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// `ListResolverRules` enumerates the stored rules in creation order, projects
/// each one exactly as its reader does — the rule domain in the canonical
/// fully-qualified form it is stored in — and restricts the page to the rules
/// whose named member matches one of `Values`.
///
/// The filter names are exactly the catalog the model documents for this
/// operation (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverRules**): `CreatorRequestId`, `DomainName`, `Name`,
/// `ResolverEndpointId`, `Status`, `Type` — each exercised here, in the
/// documented spelling and in the early upper-snake one the same doc says is
/// "still supported". A `DomainName` filter is exercised in both the dotted
/// spelling the doc asks for and the dotless one, which name one domain.
#[tokio::test]
async fn rules_are_listed_from_state_and_filtered_on_every_documented_member() {
    let router = emulator();
    let (first_endpoint, second_endpoint, ids) = seed_rules(&router).await;

    // The unfiltered page is every stored rule, in creation order, and each
    // entry is the same projection `GetResolverRule` answers.
    let listed = ok(&router, "ListResolverRules", json!({})).await;
    assert_eq!(
        listed_ids(&listed),
        ids,
        "case \"no filters\": every stored rule must be listed in creation order, got {listed}"
    );
    for (index, id) in ids.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": id.clone() }),
        )
        .await;
        let from_list = listed
            .get("ResolverRules")
            .and_then(Value::as_array)
            .and_then(|items| items.get(index))
            .expect("the page carries this rule");
        assert_eq!(
            from_list,
            rule(&fetched),
            "case {id:?}: the listed rule must be the stored one, got {from_list}"
        );
        let domain = string_member(from_list, "DomainName");
        assert!(
            domain.ends_with('.'),
            "case {id:?}: a listed rule domain must be fully qualified, got {domain:?}"
        );
    }

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "CreatorRequestId",
            json!([{ "Name": "CreatorRequestId", "Values": ["req-list-forward-two"] }]),
            vec![ids[1].clone()],
        ),
        (
            "CREATOR_REQUEST_ID, the early spelling",
            json!([{ "Name": "CREATOR_REQUEST_ID", "Values": ["req-list-system"] }]),
            vec![ids[2].clone()],
        ),
        (
            "DomainName, the dotted spelling the doc asks for",
            json!([{ "Name": "DomainName", "Values": ["one.list.example.com."] }]),
            vec![ids[0].clone()],
        ),
        (
            "DomainName, the dotless spelling of the same domain",
            json!([{ "Name": "DomainName", "Values": ["one.list.example.com"] }]),
            vec![ids[0].clone()],
        ),
        (
            "DOMAIN_NAME, two domains written in the two spellings",
            json!([{
                "Name": "DOMAIN_NAME",
                "Values": ["two.list.example.com", "apex.one.list.example.com."],
            }]),
            vec![ids[1].clone(), ids[2].clone()],
        ),
        (
            "DomainName, a domain no rule names",
            json!([{ "Name": "DomainName", "Values": ["absent.list.example.com."] }]),
            vec![],
        ),
        (
            "Name",
            json!([{ "Name": "Name", "Values": ["list-delegate"] }]),
            vec![ids[3].clone()],
        ),
        (
            "Name, which a rule created without one does not match",
            json!([{ "Name": "Name", "Values": ["list-forward-one", "list-system"] }]),
            vec![ids[0].clone(), ids[2].clone()],
        ),
        (
            "ResolverEndpointId",
            json!([{ "Name": "ResolverEndpointId", "Values": [first_endpoint.clone()] }]),
            vec![ids[0].clone()],
        ),
        (
            "RESOLVER_ENDPOINT_ID, the early spelling",
            json!([{ "Name": "RESOLVER_ENDPOINT_ID", "Values": [second_endpoint.clone()] }]),
            vec![ids[1].clone()],
        ),
        (
            "Status, the status every stored rule reports",
            json!([{ "Name": "Status", "Values": ["COMPLETE"] }]),
            ids.clone(),
        ),
        (
            "Status, one no rule reports",
            json!([{ "Name": "Status", "Values": ["DELETING"] }]),
            vec![],
        ),
        (
            "Type, the forwarding rules",
            json!([{ "Name": "Type", "Values": ["FORWARD"] }]),
            vec![ids[0].clone(), ids[1].clone()],
        ),
        (
            "TYPE, the early spelling",
            json!([{ "Name": "TYPE", "Values": ["SYSTEM"] }]),
            vec![ids[2].clone()],
        ),
        (
            "Type, two values",
            json!([{ "Name": "Type", "Values": ["SYSTEM", "DELEGATE"] }]),
            vec![ids[2].clone(), ids[3].clone()],
        ),
        (
            "two filters combine with AND, both matching",
            json!([
                { "Name": "Type", "Values": ["FORWARD"] },
                { "Name": "ResolverEndpointId", "Values": [second_endpoint.clone()] },
            ]),
            vec![ids[1].clone()],
        ),
        (
            "two filters combine with AND, disagreeing",
            json!([
                { "Name": "Type", "Values": ["SYSTEM"] },
                { "Name": "ResolverEndpointId", "Values": [first_endpoint.clone()] },
            ]),
            vec![],
        ),
        (
            "an empty value list matches nothing",
            json!([{ "Name": "Type", "Values": [] }]),
            vec![],
        ),
        ("an empty filter list restricts nothing", json!([]), ids.clone()),
    ];

    for (case, filters, expected) in cases {
        let listed = ok(&router, "ListResolverRules", json!({ "Filters": filters })).await;
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {case:?}: the filtered page is wrong, got {listed}"
        );
    }

    // A name outside THIS operation's catalog is refused, not ignored — including
    // the ones that are valid for the endpoint and association lists.
    for case in [
        "Direction",
        "HostVPCId",
        "SecurityGroupIds",
        "IpAddressCount",
        "VPCId",
        "Arn",
        "Id",
    ] {
        expect_error(
            &router,
            "ListResolverRules",
            json!({ "Filters": [{ "Name": case, "Values": ["anything"] }] }),
            case,
            "InvalidParameterException",
        )
        .await;
    }
}

/// `ListResolverRules` pages the collection the query selects: `MaxResults`
/// bounds the page and is echoed back, `NextToken` resumes where the previous
/// page stopped, the last page carries none, and a cursor only means anything
/// for the query that minted it.
#[tokio::test]
async fn paging_walks_every_rule_exactly_once() {
    let router = emulator();
    let (_, _, ids) = seed_rules(&router).await;

    for (case, page_size) in [
        ("one rule per page", 1),
        ("two rules per page", 2),
        ("a page larger than the collection", 10),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let listed = ok(&router, "ListResolverRules", request).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: a page must hold at most MaxResults rules, got {listed}"
            );
            assert_eq!(
                listed.get("MaxResults").and_then(Value::as_i64),
                Some(i64::from(page_size)),
                "case {case:?}: MaxResults must be echoed back, got {listed}"
            );
            walked.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: paging must walk every rule exactly once, in creation order"
        );
    }

    // A cursor is scoped to the query that minted it: the same offset under a
    // different `Filters` is not a page of this query.
    let first = ok(
        &router,
        "ListResolverRules",
        json!({ "MaxResults": 1, "Filters": [{ "Name": "Type", "Values": ["FORWARD"] }] }),
    )
    .await;
    let token = string_member(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "the filtered query must owe a cursor, got {first}"
    );
    for (case, request) in [
        (
            "a cursor minted under a filter, replayed with none",
            json!({ "MaxResults": 1, "NextToken": token.clone() }),
        ),
        (
            "a cursor minted under a filter, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "Type", "Values": ["SYSTEM"] }],
            }),
        ),
        (
            "a cursor this service never minted",
            json!({ "NextToken": "6e6f742d612d746f6b656e" }),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverRules",
            request,
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    // The same cursor under the query that minted it resumes that query.
    let resumed = ok(
        &router,
        "ListResolverRules",
        json!({
            "MaxResults": 1,
            "NextToken": token,
            "Filters": [{ "Name": "Type", "Values": ["FORWARD"] }],
        }),
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        vec![ids[1].clone()],
        "case \"the cursor's own query\": the second page must resume after the first, got {resumed}"
    );
}

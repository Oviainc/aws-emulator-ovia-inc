// SPDX-License-Identifier: CC0-1.0

//! End-to-end probe of the WHOLE life of a DNS Firewall rule group: every call
//! AWS documents for one, in the order a client makes them, from the create to
//! the teardown that takes the group and the domain list its rules referenced
//! out again.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What it pins is that no step of that life is refused for a resource the
//! service itself created: a group with rules on one domain list told apart by
//! `Qtype`, associated with a VPC of the account and disassociated again, has
//! its rules deleted one at a time and is then deleted itself, and the domain
//! list goes last. Each step reports the resource it acted on, so a client
//! following AWS's documented teardown order gets through it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, string_member};

/// The VPC the account holds for the association step, written into the Amazon
/// EC2 store the association resolves its `VpcId` against — that inventory is
/// that service's, so it is reached through its own accessors.
const HELD_VPC: &str = "vpc-0123456789abcdef0";

/// This service, on an engine whose Amazon EC2 inventory holds [`HELD_VPC`].
fn emulator_holding_a_vpc() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    let state = ec2::state::service_state(&*engine)
        .expect("the in-memory engine carries the account's Amazon EC2 store");
    let mut store = state.lock();
    store.vpcs.insert(
        HELD_VPC.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(HELD_VPC.to_string()),
            cidr_block: Some("10.0.0.0/16".to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            ..Default::default()
        },
    );
    drop(store);
    router
}

/// Make one call that must be served, and answer its body.
async fn step(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: the call with {body} must be served, got {status}: {parsed}"
    );
    parsed
}

/// The `Id` of the one resource a response carries under `member`.
fn id_of(body: &Value, member: &str, case: &str) -> String {
    let resource = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry {member}, got {body}"));
    let id = string_member(resource, "Id");
    assert!(
        !id.is_empty(),
        "case {case:?}: {member} must report its Id, got {resource}"
    );
    id
}

/// Every call of a rule group's life is served, and the teardown takes every
/// resource out in AWS's documented order.
#[tokio::test]
async fn the_whole_life_of_a_rule_group_is_served() {
    let router = emulator_holding_a_vpc();

    let group = id_of(
        &step(
            &router,
            "CreateFirewallRuleGroup",
            json!({ "CreatorRequestId": "req-life-group", "Name": "life-group" }),
        )
        .await,
        "FirewallRuleGroup",
        "the create of the group",
    );
    let list = id_of(
        &step(
            &router,
            "CreateFirewallDomainList",
            json!({ "CreatorRequestId": "req-life-list", "Name": "life-list" }),
        )
        .await,
        "FirewallDomainList",
        "the create of the list",
    );
    step(
        &router,
        "UpdateFirewallDomains",
        json!({
            "FirewallDomainListId": list.clone(),
            "Operation": "ADD",
            "Domains": ["ads.example.com", "malware.example.org"],
        }),
    )
    .await;

    // Two rules of the one group matching the one domain list, told apart by
    // the query type — the shape that makes a rule addressable by `Qtype`.
    step(
        &router,
        "CreateFirewallRule",
        json!({
            "CreatorRequestId": "req-life-alert",
            "FirewallRuleGroupId": group.clone(),
            "FirewallDomainListId": list.clone(),
            "Name": "life-rule-alert",
            "Priority": 100,
            "Action": "ALERT",
            "FirewallDomainRedirectionAction": "INSPECT_REDIRECTION_DOMAIN",
        }),
    )
    .await;
    step(
        &router,
        "CreateFirewallRule",
        json!({
            "CreatorRequestId": "req-life-block",
            "FirewallRuleGroupId": group.clone(),
            "FirewallDomainListId": list.clone(),
            "Name": "life-rule-block",
            "Priority": 200,
            "Action": "BLOCK",
            "BlockResponse": "OVERRIDE",
            "BlockOverrideDomain": "sinkhole.example.com",
            "BlockOverrideDnsType": "CNAME",
            "BlockOverrideTtl": 60,
            "Qtype": "A",
        }),
    )
    .await;

    let listed = step(
        &router,
        "ListFirewallRules",
        json!({ "FirewallRuleGroupId": group.clone() }),
    )
    .await;
    assert_eq!(
        listed
            .get("FirewallRules")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        2,
        "case \"the listing of the group's rules\": both created rules must be reported, got {listed}"
    );
    step(
        &router,
        "GetFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group.clone() }),
    )
    .await;
    step(
        &router,
        "UpdateFirewallRule",
        json!({
            "FirewallRuleGroupId": group.clone(),
            "FirewallDomainListId": list.clone(),
            "Qtype": "A",
            "Priority": 300,
        }),
    )
    .await;

    let association = id_of(
        &step(
            &router,
            "AssociateFirewallRuleGroup",
            json!({
                "CreatorRequestId": "req-life-association",
                "FirewallRuleGroupId": group.clone(),
                "VpcId": HELD_VPC,
                "Priority": 101,
                "Name": "life-association",
            }),
        )
        .await,
        "FirewallRuleGroupAssociation",
        "the associate",
    );
    // The association comes up over the span the clock owes it, so the
    // disassociate that ends its life is made once it has settled.
    tokio::time::sleep(
        service_com_amazonaws_route53resolver_route53resolver::state::PROVISIONING_DELAY
            + Duration::from_millis(250),
    )
    .await;
    let associations = step(
        &router,
        "ListFirewallRuleGroupAssociations",
        json!({ "FirewallRuleGroupId": group.clone() }),
    )
    .await;
    assert_eq!(
        associations
            .get("FirewallRuleGroupAssociations")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        1,
        "case \"the listing of the group's associations\": the one association must be reported, got {associations}"
    );
    step(
        &router,
        "DisassociateFirewallRuleGroup",
        json!({ "FirewallRuleGroupAssociationId": association }),
    )
    .await;

    // The teardown, in AWS's documented order: each rule by the source it
    // matches (the query type tells the two apart), then the group, then the
    // domain list they referenced.
    step(
        &router,
        "DeleteFirewallRule",
        json!({
            "FirewallRuleGroupId": group.clone(),
            "FirewallDomainListId": list.clone(),
            "Qtype": "A",
        }),
    )
    .await;
    step(
        &router,
        "DeleteFirewallRule",
        json!({
            "FirewallRuleGroupId": group.clone(),
            "FirewallDomainListId": list.clone(),
        }),
    )
    .await;
    let deleted = step(
        &router,
        "DeleteFirewallRuleGroup",
        json!({ "FirewallRuleGroupId": group.clone() }),
    )
    .await;
    assert_eq!(
        id_of(&deleted, "FirewallRuleGroup", "the delete of the group"),
        group,
        "case \"the delete of the group\": it must answer the group it took out, got {deleted}"
    );
    assert_eq!(
        deleted
            .get("FirewallRuleGroup")
            .and_then(|group| group.get("RuleCount"))
            .and_then(Value::as_i64),
        Some(0),
        "case \"the delete of the group\": a group whose rules were deleted holds none, got {deleted}"
    );
    let dropped = step(
        &router,
        "DeleteFirewallDomainList",
        json!({ "FirewallDomainListId": list.clone() }),
    )
    .await;
    assert_eq!(
        id_of(&dropped, "FirewallDomainList", "the delete of the list"),
        list,
        "case \"the delete of the list\": it must answer the list it took out, got {dropped}"
    );

    // Nothing of the group's life is left: both readers now report the ids
    // missing.
    for (operation, request) in [
        (
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": group.clone() }),
        ),
        (
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": list.clone() }),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {operation:?}: a torn-down resource must be reported missing, got {status}: {body}"
        );
    }
}

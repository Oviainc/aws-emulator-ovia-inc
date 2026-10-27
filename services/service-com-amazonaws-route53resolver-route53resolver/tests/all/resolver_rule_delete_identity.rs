// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DeleteResolverRule` answers and what it takes out
//! of state, whatever kind of rule it is pointed at.
//!
//! The rule these span: a delete answers the resource it removed exactly as the
//! store held it — "Information about the DeleteResolverRule request, including
//! the status of the request" — so every member is the one the reader reported,
//! except the status, which reports what the request leaves the resource in, and
//! the modification time, which the request moves; and it removes that resource
//! only, leaving every other stored rule and the associations of those rules
//! untouched.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, expect_error, ok, parse_time, string_member, without_status};
use super::resolver_rule_deletion::{associate, outbound_endpoint};

/// The `ResolverRule` of a create/get/delete response.
fn rule(body: &Value) -> &Value {
    body.get("ResolverRule")
        .expect("the response carries a ResolverRule")
}

/// The same rule with the two members a delete is allowed to move dropped, so
/// the rest can be compared member by member.
fn configuration(value: &Value) -> Value {
    let mut cloned = without_status(value);
    if let Some(object) = cloned.as_object_mut() {
        object.remove("ModificationTime");
    }
    cloned
}

/// The control rule every row stores beside the rule under test: a SYSTEM rule,
/// which needs no forwarding target, created under a per-row request id.
async fn control_rule(router: &MultiServiceRouter, index: usize) -> String {
    let created = ok(
        router,
        "CreateResolverRule",
        json!({
            "CreatorRequestId": format!("identity-control-{index}"),
            "Name": "control",
            "RuleType": "SYSTEM",
            "DomainName": format!("control-{index}.example.com"),
        }),
    )
    .await;
    string_member(rule(&created), "Id")
}

/// The create requests the rules under test are made with. `{endpoint}` is
/// replaced by the stored OUTBOUND endpoint's id.
///
/// The rows differ along what the rule generalises over: the rule's TYPE, how
/// many target addresses it carries and in which address family, which optional
/// members it was created with, and whether it carries tags. The types are the
/// ones a caller may create, from `RuleTypeOption`
/// (`smithy_query(query="shape",shape="RuleTypeOption")`) minus `RECURSIVE`,
/// which only Resolver itself creates.
fn identity_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "FORWARD, one target address taking every default",
            json!({
                "CreatorRequestId": "identity-forward-one",
                "RuleType": "FORWARD",
                "DomainName": "one.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [{ "Ip": "192.0.2.10" }],
            }),
        ),
        (
            "FORWARD, three target addresses with explicit ports and protocols",
            json!({
                "CreatorRequestId": "identity-forward-many",
                "Name": "three-targets",
                "RuleType": "FORWARD",
                "DomainName": "many.example.com",
                "ResolverEndpointId": "{endpoint}",
                "TargetIps": [
                    { "Ip": "192.0.2.11", "Port": 53, "Protocol": "Do53" },
                    { "Ip": "192.0.2.12", "Port": 5353, "Protocol": "Do53" },
                    { "Ip": "192.0.2.13", "Port": 853, "Protocol": "DoH" },
                ],
                "Tags": [{ "Key": "team", "Value": "networking" }],
            }),
        ),
        (
            "FORWARD, an IPv6 target over DoH with a server name indication",
            json!({
                "CreatorRequestId": "identity-forward-v6",
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
        ),
        (
            "SYSTEM, a subdomain with a name and tags and no forwarding target",
            json!({
                "CreatorRequestId": "identity-system",
                "Name": "apex-system",
                "RuleType": "SYSTEM",
                "DomainName": "apex.example.com",
                "Tags": [
                    { "Key": "owner", "Value": "dns" },
                    { "Key": "tier", "Value": "shared" },
                ],
            }),
        ),
        (
            "DELEGATE, keyed on a delegation record",
            json!({
                "CreatorRequestId": "identity-delegate",
                "RuleType": "DELEGATE",
                "DomainName": "delegated.example.com",
                "DelegationRecord": "ns.example.com",
            }),
        ),
    ]
}

/// The rule a delete takes out of state is the rule it answers with: the
/// configuration the reader reported, carried over member for member, with the
/// status the request leaves the rule in and a modification time that is not
/// behind the one the stored rule had.
///
/// The row's rule is then gone — a read of it and a second delete of it are both
/// "the specified resource doesn't exist" — while a control rule stored beside
/// it, and the VPC association that control rule holds, are untouched.
#[tokio::test]
async fn a_delete_answers_the_rule_it_removed_and_removes_only_that_rule() {
    for (index, (case, body)) in identity_cases().into_iter().enumerate() {
        // One emulator per row, so the store holds exactly this row's rule and
        // the control rule beside it.
        let router = emulator();
        let endpoint = outbound_endpoint(&router, &format!("identity-endpoint-{index}")).await;

        // The control rule: another rule, associated with a VPC, which this
        // row's delete must not touch.
        let control = control_rule(&router, index).await;
        let control_vpc = "vpc-00000000000000030";
        let control_association = associate(&router, &control, control_vpc).await;
        let control_before = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": control.clone() }),
        )
        .await;

        let mut request = body;
        if request.get("ResolverEndpointId").is_some() {
            request["ResolverEndpointId"] = json!(endpoint);
        }
        let created = ok(&router, "CreateResolverRule", request).await;
        let id = string_member(rule(&created), "Id");

        // What the store holds, read back through the reader, is the baseline
        // the delete's answer is compared against.
        let stored = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": id.clone() }),
        )
        .await;
        let stored = rule(&stored).clone();

        let deleted = ok(
            &router,
            "DeleteResolverRule",
            json!({ "ResolverRuleId": id.clone() }),
        )
        .await;
        let deleted = rule(&deleted).clone();

        assert_eq!(
            configuration(&deleted),
            configuration(&stored),
            "case {case:?}: the delete must answer the stored rule member for member, got \
             {deleted}"
        );
        assert_eq!(
            string_member(&deleted, "Status"),
            "DELETING",
            "case {case:?}: the rule a delete took out reports the status of that request, got \
             {deleted}"
        );
        let before = parse_time(
            case,
            "ModificationTime",
            &string_member(&stored, "ModificationTime"),
        );
        let after = parse_time(
            case,
            "ModificationTime",
            &string_member(&deleted, "ModificationTime"),
        );
        assert!(
            after >= before,
            "case {case:?}: the delete must not move ModificationTime backwards, got {after:?} \
             after {before:?}"
        );

        // The rule is gone, and gone only once: a read of it and a repeat of the
        // delete are both the resource that doesn't exist.
        for operation in ["GetResolverRule", "DeleteResolverRule"] {
            expect_error(
                &router,
                operation,
                json!({ "ResolverRuleId": id.clone() }),
                case,
                "ResourceNotFoundException",
            )
            .await;
        }

        // And nothing else moved: the control rule reads back exactly as it did
        // before the delete, and the association it holds still resolves under
        // its own id and still blocks ITS delete.
        let control_after = ok(
            &router,
            "GetResolverRule",
            json!({ "ResolverRuleId": control.clone() }),
        )
        .await;
        assert_eq!(
            rule(&control_after),
            rule(&control_before),
            "case {case:?}: the delete must leave every other stored rule alone, got \
             {control_after}"
        );
        assert_eq!(
            string_member(
                ok(
                    &router,
                    "GetResolverRuleAssociation",
                    json!({ "ResolverRuleAssociationId": control_association.clone() })
                )
                .await
                .get("ResolverRuleAssociation")
                .unwrap_or(&Value::Null),
                "Id"
            ),
            control_association,
            "case {case:?}: the delete must leave another rule's association alone"
        );
        expect_error(
            &router,
            "DeleteResolverRule",
            json!({ "ResolverRuleId": control.clone() }),
            case,
            "ResourceInUseException",
        )
        .await;
    }
}

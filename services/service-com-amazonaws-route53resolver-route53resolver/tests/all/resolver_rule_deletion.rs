// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the write half of the Resolver rule slice that takes a
//! rule and an association back out of state: `DisassociateResolverRule` and
//! `DeleteResolverRule`.
//!
//! The rule these spans: a rule can be deleted only once every VPC it is
//! associated with has been disassociated ("Before you can delete a Resolver
//! rule, you must disassociate it from all the VPCs that you associated the
//! Resolver rule with"), each call answers the resource it took out with the
//! status that request leaves it in, and a resource the store no longer holds —
//! or a rule/VPC pair it never held — is the one "the specified resource doesn't
//! exist".
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, expect_error, ok, string_member};

/// Create a Resolver endpoint a FORWARD rule can route its queries through, and
/// return its `Id`.
async fn outbound_endpoint(router: &MultiServiceRouter, creator: &str) -> String {
    let created = ok(
        router,
        "CreateResolverEndpoint",
        json!({
            "CreatorRequestId": creator,
            "Direction": "OUTBOUND",
            "SecurityGroupIds": ["sg-00000000000000001"],
            "IpAddresses": [
                { "SubnetId": "subnet-00000000000000001", "Ip": "10.0.0.10" },
                { "SubnetId": "subnet-00000000000000002", "Ip": "10.0.1.10" },
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

/// Create a FORWARD rule through `endpoint` under `creator`, and return its `Id`.
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
    string_member(
        created
            .get("ResolverRule")
            .expect("the create answers a ResolverRule"),
        "Id",
    )
}

/// Associate `rule` with `vpc` and return the association's `Id`.
async fn associate(router: &MultiServiceRouter, rule: &str, vpc: &str) -> String {
    let created = ok(
        router,
        "AssociateResolverRule",
        json!({ "ResolverRuleId": rule, "VPCId": vpc, "Name": format!("{vpc}-association") }),
    )
    .await;
    string_member(
        created
            .get("ResolverRuleAssociation")
            .expect("the associate answers a ResolverRuleAssociation"),
        "Id",
    )
}

/// One row of the deletion table: how many VPCs the rule under test is
/// associated with before the delete is attempted.
struct DeleteCase {
    case: &'static str,
    /// The VPCs the rule is associated with, in the order they are associated.
    /// Empty for a rule nothing uses, which is deletable straight away.
    vpcs: &'static [&'static str],
}

const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a rule no VPC is associated with",
        vpcs: &[],
    },
    DeleteCase {
        case: "a rule one VPC is associated with",
        vpcs: &["vpc-00000000000000001"],
    },
    DeleteCase {
        case: "a rule three VPCs are associated with",
        vpcs: &[
            "vpc-00000000000000011",
            "vpc-00000000000000012",
            "vpc-00000000000000013",
        ],
    },
];

/// A rule is deleted only once every VPC it is associated with has been
/// disassociated: until then the delete is refused with the
/// `ResourceInUseException` the operation models, naming how many associations
/// are still in the way, and the rule stays readable.
///
/// The rows differ along what the rule generalises over: how many VPCs use the
/// rule (none, one, several). Each association is then removed one at a time, so
/// the refusal is exercised at every count down to zero.
#[tokio::test]
async fn a_rule_is_deleted_once_no_vpc_is_associated_with_it() {
    for (index, row) in DELETE_CASES.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let endpoint = outbound_endpoint(&router, &format!("endpoint-{index}")).await;
        let rule_id = forward_rule(
            &router,
            &format!("rule-{index}"),
            &format!("delete-{index}.example.com"),
            &endpoint,
        )
        .await;

        let mut associations: Vec<(&str, String)> = Vec::new();
        for vpc in row.vpcs {
            associations.push((vpc, associate(&router, &rule_id, vpc).await));
        }

        // Every count from the one the row set up down to one: the delete is
        // refused while any association is left, and the rule is still there.
        while !associations.is_empty() {
            let (status, refused) = call(
                &router,
                "DeleteResolverRule",
                json!({ "ResolverRuleId": rule_id }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a rule {} VPC(s) use cannot be deleted, got {status}: {refused}",
                associations.len()
            );
            assert!(
                super::harness::error_type(&refused).contains("ResourceInUseException"),
                "case {case:?}: expected ResourceInUseException, got {refused}"
            );
            let message = string_member(&refused, "Message");
            assert!(
                message.contains(&rule_id)
                    && message.contains(&format!("{} VPC(s)", associations.len())),
                "case {case:?}: the refusal must name the rule and how many associations are \
                 still in the way ({}), got {message:?}",
                associations.len()
            );
            assert_eq!(
                string_member(
                    ok(
                        &router,
                        "GetResolverRule",
                        json!({ "ResolverRuleId": rule_id })
                    )
                    .await
                    .get("ResolverRule")
                    .unwrap_or(&Value::Null),
                    "Id"
                ),
                rule_id,
                "case {case:?}: the refused delete must leave the rule readable"
            );

            // One association off: the answer is the association it took out,
            // reporting the status the request leaves it in.
            let (vpc, association_id) = associations.remove(0);
            let removed = ok(
                &router,
                "DisassociateResolverRule",
                json!({ "ResolverRuleId": rule_id, "VPCId": vpc }),
            )
            .await;
            let removed = removed
                .get("ResolverRuleAssociation")
                .unwrap_or(&Value::Null);
            assert_eq!(
                string_member(removed, "Id"),
                association_id,
                "case {case:?}: the disassociate must answer the association it removed, got \
                 {removed}"
            );
            assert_eq!(
                string_member(removed, "Status"),
                "DELETING",
                "case {case:?}: the removed association reports DELETING, got {removed}"
            );
            assert_eq!(
                (
                    string_member(removed, "ResolverRuleId"),
                    string_member(removed, "VPCId"),
                    string_member(removed, "Name"),
                ),
                (rule_id.clone(), vpc.to_string(), format!("{vpc}-association")),
                "case {case:?}: the removed association must carry the pair and the name it was \
                 made with, got {removed}"
            );
            // And it is gone: neither its own id nor the pair resolves again.
            expect_error(
                &router,
                "GetResolverRuleAssociation",
                json!({ "ResolverRuleAssociationId": association_id }),
                case,
                "ResourceNotFoundException",
            )
            .await;
            expect_error(
                &router,
                "DisassociateResolverRule",
                json!({ "ResolverRuleId": rule_id, "VPCId": vpc }),
                case,
                "ResourceNotFoundException",
            )
            .await;
        }

        // Nothing uses the rule now, so the delete carries through and answers
        // the rule it took out, reporting `DELETING`.
        let deleted = ok(
            &router,
            "DeleteResolverRule",
            json!({ "ResolverRuleId": rule_id }),
        )
        .await;
        let deleted = deleted.get("ResolverRule").unwrap_or(&Value::Null);
        assert_eq!(
            string_member(deleted, "Id"),
            rule_id,
            "case {case:?}: the delete must answer the rule it removed, got {deleted}"
        );
        assert_eq!(
            string_member(deleted, "Status"),
            "DELETING",
            "case {case:?}: the deleted rule reports DELETING, got {deleted}"
        );
        assert_eq!(
            string_member(deleted, "DomainName"),
            format!("delete-{index}.example.com."),
            "case {case:?}: the deleted rule must carry the configuration it had, got {deleted}"
        );

        // A read of the deleted rule, and a second delete of it, are both "the
        // specified resource doesn't exist".
        for operation in ["GetResolverRule", "DeleteResolverRule"] {
            expect_error(
                &router,
                operation,
                json!({ "ResolverRuleId": rule_id }),
                case,
                "ResourceNotFoundException",
            )
            .await;
        }
    }
}

/// One row of the refusal table: the pair a disassociate names, and why the
/// store holds no association for it.
struct PairCase {
    case: &'static str,
    /// `{rule}` is replaced by the id of the stored rule, so a row can name the
    /// rule that exists or one that does not.
    rule_id: &'static str,
    vpc_id: &'static str,
}

const PAIR_CASES: &[PairCase] = &[
    PairCase {
        case: "a rule id the store holds no rule under",
        rule_id: "rslvr-rr-0000000000000000",
        vpc_id: "vpc-00000000000000021",
    },
    PairCase {
        case: "the stored rule, and a VPC it was never associated with",
        rule_id: "{rule}",
        vpc_id: "vpc-00000000000000022",
    },
    PairCase {
        case: "a rule id that is not an id this service mints",
        rule_id: "not-a-resolver-rule-id",
        vpc_id: "vpc-00000000000000023",
    },
];

/// `DisassociateResolverRule` addresses an association by the PAIR it was made
/// from, so a pair the store holds no association for is refused with the
/// `ResourceNotFoundException` the operation models — and the association that
/// does exist is untouched by the refusal.
///
/// The rows differ along which half of the pair the store cannot resolve.
#[tokio::test]
async fn a_pair_no_association_was_made_from_is_refused() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-pairs").await;
    let rule_id = forward_rule(&router, "rule-pairs", "pairs.example.com", &endpoint).await;
    let vpc = "vpc-00000000000000020";
    let association_id = associate(&router, &rule_id, vpc).await;

    for row in PAIR_CASES {
        let named_rule = row.rule_id.replace("{rule}", &rule_id);
        expect_error(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": named_rule, "VPCId": row.vpc_id }),
            row.case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // The association the store does hold still resolves, and its own pair still
    // disassociates.
    let read = ok(
        &router,
        "GetResolverRuleAssociation",
        json!({ "ResolverRuleAssociationId": association_id }),
    )
    .await;
    assert_eq!(
        string_member(
            read.get("ResolverRuleAssociation").unwrap_or(&Value::Null),
            "Id"
        ),
        association_id,
        "the refusals must leave the stored association alone, got {read}"
    );
    let removed = ok(
        &router,
        "DisassociateResolverRule",
        json!({ "ResolverRuleId": rule_id, "VPCId": vpc }),
    )
    .await;
    assert_eq!(
        string_member(
            removed
                .get("ResolverRuleAssociation")
                .unwrap_or(&Value::Null),
            "Id"
        ),
        association_id,
        "the pair the association was made from must disassociate it, got {removed}"
    );
}

/// The `CreatorRequestId` of a deleted rule is released: a create that reuses it
/// makes a NEW rule rather than resolving to the one that is gone.
///
/// `CreatorRequestId` exists so "failed requests [can] be retried without the
/// risk of running the operation twice", which is a statement about a rule that
/// EXISTS; once the rule is deleted there is nothing for a repeat of that
/// request to resolve to.
#[tokio::test]
async fn a_deleted_rule_releases_its_creator_request_id() {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, "endpoint-creator").await;
    let creator = "rule-creator-reused";

    let first = forward_rule(&router, creator, "creator.example.com", &endpoint).await;
    // The same request again, while the rule exists, is the retry that member is
    // for: it resolves to the rule it already made.
    let retried = forward_rule(&router, creator, "creator.example.com", &endpoint).await;
    assert_eq!(
        retried, first,
        "a repeat of a CreatorRequestId that already made a rule must resolve to it"
    );

    ok(
        &router,
        "DeleteResolverRule",
        json!({ "ResolverRuleId": first }),
    )
    .await;

    let second = forward_rule(&router, creator, "creator.example.com", &endpoint).await;
    assert_ne!(
        second, first,
        "a create reusing the CreatorRequestId of a DELETED rule must mint a new rule, got \
         {second:?}"
    );
    assert_eq!(
        string_member(
            ok(
                &router,
                "GetResolverRule",
                json!({ "ResolverRuleId": second })
            )
            .await
            .get("ResolverRule")
            .unwrap_or(&Value::Null),
            "Id"
        ),
        second,
        "the new rule must be readable under the id its create answered"
    );
}

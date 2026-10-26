// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisassociateResolverRule`.
//!
//! The rule these pin: the call "removes the association between a specified
//! Resolver rule and a specified VPC", so it takes out of state EXACTLY the one
//! association that ordered pair was made from, answers that record — the pair
//! and the `Name` it was made with — with the status the request leaves it in,
//! and leaves every other association, and both the rule and the VPC
//! themselves, as they were.
//!
//! [`super::resolver_rule_deletion`] drives the same operation on the way to a
//! delete, one association at a time, and
//! [`super::resolver_rule_error_bodies`] asserts the body of its refusal. What
//! is asserted here is the SCOPE of one disassociate: which records it may
//! touch, over the shapes an association graph takes (a lone association, one of
//! several VPCs a rule is associated with, one of several rules a VPC is
//! associated with, one cell of a rule-by-VPC grid) and with the optional `Name`
//! present and absent.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`] and the fixtures the ones in
//! [`super::resolver_rule_deletion`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, expect_error, ok, string_member, without_status};
use super::resolver_rule_deletion::{forward_rule, outbound_endpoint};

/// One association a row sets up: which fixture rule it is of, which fixture VPC
/// it is with, and the `Name` the associate carried (`None` leaves that optional
/// member out of the request).
struct Pair {
    rule: usize,
    vpc: usize,
    name: Option<&'static str>,
}

/// One row of the scope table.
struct DisassociateCase {
    case: &'static str,
    /// How many fixture rules the row needs.
    rules: usize,
    /// How many fixture VPCs the row needs.
    vpcs: usize,
    /// The associations the row makes, in the order it makes them.
    pairs: &'static [Pair],
    /// Index into `pairs` of the association the disassociate takes out.
    target: usize,
}

/// The rows differ along what the rule generalises over: how many associations
/// the store holds, whether the target shares its rule or its VPC with another
/// association, where in creation order it sits, and whether it was made with a
/// `Name`.
const DISASSOCIATE_CASES: &[DisassociateCase] = &[
    DisassociateCase {
        case: "the only association in the store, made with a Name",
        rules: 1,
        vpcs: 1,
        pairs: &[Pair {
            rule: 0,
            vpc: 0,
            name: Some("lone-association"),
        }],
        target: 0,
    },
    DisassociateCase {
        case: "the only association in the store, made without a Name",
        rules: 1,
        vpcs: 1,
        pairs: &[Pair {
            rule: 0,
            vpc: 0,
            name: None,
        }],
        target: 0,
    },
    DisassociateCase {
        case: "the first of three VPCs one rule is associated with",
        rules: 1,
        vpcs: 3,
        pairs: &[
            Pair {
                rule: 0,
                vpc: 0,
                name: Some("first-vpc"),
            },
            Pair {
                rule: 0,
                vpc: 1,
                name: Some("second-vpc"),
            },
            Pair {
                rule: 0,
                vpc: 2,
                name: None,
            },
        ],
        target: 0,
    },
    DisassociateCase {
        case: "the last of three VPCs one rule is associated with",
        rules: 1,
        vpcs: 3,
        pairs: &[
            Pair {
                rule: 0,
                vpc: 0,
                name: Some("first-vpc"),
            },
            Pair {
                rule: 0,
                vpc: 1,
                name: None,
            },
            Pair {
                rule: 0,
                vpc: 2,
                name: Some("third-vpc"),
            },
        ],
        target: 2,
    },
    DisassociateCase {
        case: "the middle of three rules one VPC is associated with",
        rules: 3,
        vpcs: 1,
        pairs: &[
            Pair {
                rule: 0,
                vpc: 0,
                name: Some("first-rule"),
            },
            Pair {
                rule: 1,
                vpc: 0,
                name: Some("second-rule"),
            },
            Pair {
                rule: 2,
                vpc: 0,
                name: Some("third-rule"),
            },
        ],
        target: 1,
    },
    DisassociateCase {
        case: "one cell of a two-rule, two-VPC grid",
        rules: 2,
        vpcs: 2,
        pairs: &[
            Pair {
                rule: 0,
                vpc: 0,
                name: Some("one-one"),
            },
            Pair {
                rule: 0,
                vpc: 1,
                name: Some("one-two"),
            },
            Pair {
                rule: 1,
                vpc: 0,
                name: None,
            },
            Pair {
                rule: 1,
                vpc: 1,
                name: Some("two-two"),
            },
        ],
        target: 2,
    },
];

/// The `ResolverRuleAssociationStatus` values an association that is still in
/// force may report: the transient one a new association carries and the settled
/// one the clock-owed transition moves it to
/// (`smithy_query(query="shape",shape="ResolverRuleAssociationStatus")`).
const IN_FORCE_STATUSES: &[&str] = &["CREATING", "COMPLETE"];

/// The fixture VPC ids a row works with, shaped the way AWS names a VPC.
fn fixture_vpcs(row_index: usize, count: usize) -> Vec<String> {
    (0..count)
        .map(|vpc| format!("vpc-{:017x}", 0x5100 + row_index * 0x10 + vpc))
        .collect()
}

/// Associate `rule` with `vpc`, optionally under `name`, and return the
/// `ResolverRuleAssociation` the call answered.
///
/// [`super::resolver_rule_deletion::associate`] is the shared fixture for the
/// common case; this suite needs the two things that one does not give it — the
/// WHOLE answered record, to compare a survivor against later, and a request
/// with the optional `Name` left out.
async fn associate_pair(
    router: &MultiServiceRouter,
    rule: &str,
    vpc: &str,
    name: Option<&str>,
) -> Value {
    let mut request = json!({ "ResolverRuleId": rule, "VPCId": vpc });
    if let Some(name) = name {
        request
            .as_object_mut()
            .expect("the request body is an object")
            .insert("Name".to_string(), json!(name));
    }
    ok(router, "AssociateResolverRule", request)
        .await
        .get("ResolverRuleAssociation")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `Id`s one `ListResolverRuleAssociations` response carries.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("ResolverRuleAssociations")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| string_member(item, "Id")).collect())
        .unwrap_or_default()
}

/// The fixture rules and VPCs of one row, with the associations it made in
/// creation order.
struct Fixture {
    router: MultiServiceRouter,
    rules: Vec<String>,
    vpcs: Vec<String>,
    made: Vec<Value>,
}

/// Build one row's rules, VPCs and associations on a fresh emulator.
async fn fixture(row_index: usize, rules: usize, vpcs: usize, pairs: &[Pair]) -> Fixture {
    let router = emulator();
    let endpoint = outbound_endpoint(&router, &format!("endpoint-disassociate-{row_index}")).await;

    let mut rule_ids = Vec::new();
    for rule in 0..rules {
        rule_ids.push(
            forward_rule(
                &router,
                &format!("rule-disassociate-{row_index}-{rule}"),
                &format!("rule{rule}.disassociate-{row_index}.example.com"),
                &endpoint,
            )
            .await,
        );
    }
    let vpc_ids = fixture_vpcs(row_index, vpcs);

    let mut made = Vec::new();
    for pair in pairs {
        made.push(associate_pair(&router, &rule_ids[pair.rule], &vpc_ids[pair.vpc], pair.name).await);
    }

    Fixture {
        router,
        rules: rule_ids,
        vpcs: vpc_ids,
        made,
    }
}

/// A disassociate takes out exactly the association its pair was made from: the
/// answer is that record with the status the request leaves it in, the record is
/// gone from both of its readers, and every other association — and every rule —
/// is untouched.
#[tokio::test]
async fn a_disassociate_takes_out_exactly_the_association_its_pair_was_made_from() {
    for (index, row) in DISASSOCIATE_CASES.iter().enumerate() {
        let case = row.case;
        let Fixture {
            router,
            rules,
            vpcs,
            made,
        } = fixture(index, row.rules, row.vpcs, row.pairs).await;

        let target = &row.pairs[row.target];
        let target_id = string_member(&made[row.target], "Id");
        let rule_id = rules[target.rule].clone();
        let vpc_id = vpcs[target.vpc].clone();

        let answered = ok(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": rule_id, "VPCId": vpc_id }),
        )
        .await;
        let answered = answered
            .get("ResolverRuleAssociation")
            .cloned()
            .unwrap_or(Value::Null);

        assert_eq!(
            string_member(&answered, "Id"),
            target_id,
            "case {case:?}: the answer must be the association the pair was made from, got \
             {answered}"
        );
        assert_eq!(
            (
                string_member(&answered, "ResolverRuleId"),
                string_member(&answered, "VPCId"),
            ),
            (rule_id.clone(), vpc_id.clone()),
            "case {case:?}: the answer must carry the pair the request named, got {answered}"
        );
        assert_eq!(
            answered.get("Name").and_then(Value::as_str),
            target.name,
            "case {case:?}: the answer must carry the Name the association was made with, got \
             {answered}"
        );
        assert_eq!(
            string_member(&answered, "Status"),
            "DELETING",
            "case {case:?}: the association on its way out reports DELETING, got {answered}"
        );

        // Gone from the reader that addresses it by its own id, and from a
        // second disassociate of the pair it was made from.
        expect_error(
            &router,
            "GetResolverRuleAssociation",
            json!({ "ResolverRuleAssociationId": target_id.clone() }),
            case,
            "ResourceNotFoundException",
        )
        .await;
        expect_error(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": rule_id, "VPCId": vpc_id }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // Every other association is exactly as it was, still in force, and
        // still listed in creation order.
        let mut survivors = Vec::new();
        for (position, made_association) in made.iter().enumerate() {
            if position == row.target {
                continue;
            }
            let id = string_member(made_association, "Id");
            let read = ok(
                &router,
                "GetResolverRuleAssociation",
                json!({ "ResolverRuleAssociationId": id.clone() }),
            )
            .await;
            let read = read
                .get("ResolverRuleAssociation")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                without_status(&read),
                without_status(made_association),
                "case {case:?}: association {id:?} must be untouched by the disassociate, got \
                 {read}"
            );
            let status = string_member(&read, "Status");
            assert!(
                IN_FORCE_STATUSES.contains(&status.as_str()),
                "case {case:?}: association {id:?} must still be in force, got {status:?}"
            );
            survivors.push(id);
        }
        let listed = ok(&router, "ListResolverRuleAssociations", json!({})).await;
        assert_eq!(
            listed_ids(&listed),
            survivors,
            "case {case:?}: only the disassociated record may leave the listing, got {listed}"
        );

        // The disassociate takes out the association, not the rule: every
        // fixture rule is still readable and still settled.
        for rule in &rules {
            let read = ok(
                &router,
                "GetResolverRule",
                json!({ "ResolverRuleId": rule.clone() }),
            )
            .await;
            let read = read.get("ResolverRule").cloned().unwrap_or(Value::Null);
            assert_eq!(
                (string_member(&read, "Id"), string_member(&read, "Status")),
                (rule.clone(), "COMPLETE".to_string()),
                "case {case:?}: rule {rule:?} must be untouched by the disassociate, got {read}"
            );
        }
    }
}

/// The pair is ORDERED: `ResolverRuleId` names the rule and `VPCId` the VPC, so
/// a request that swaps them addresses no association the store holds, is
/// refused, and leaves the association in place.
///
/// The rows are every association graph of the table above, so the swap is
/// exercised both where neither half of the pair appears in another association
/// and where the target shares its rule or its VPC with another record.
#[tokio::test]
async fn a_swapped_pair_addresses_no_association() {
    for (index, row) in DISASSOCIATE_CASES.iter().enumerate() {
        let case = row.case;
        let Fixture {
            router,
            rules,
            vpcs,
            made,
        } = fixture(100 + index, row.rules, row.vpcs, row.pairs).await;

        let target = &row.pairs[row.target];
        let rule_id = rules[target.rule].clone();
        let vpc_id = vpcs[target.vpc].clone();

        expect_error(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": vpc_id, "VPCId": rule_id }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // Nothing was taken out: every association the row made still reads
        // back, and the pair in the documented order still disassociates.
        let listed = ok(&router, "ListResolverRuleAssociations", json!({})).await;
        assert_eq!(
            listed_ids(&listed),
            made.iter()
                .map(|association| string_member(association, "Id"))
                .collect::<Vec<String>>(),
            "case {case:?}: a refused disassociate must leave every association in place, got \
             {listed}"
        );
        let answered = ok(
            &router,
            "DisassociateResolverRule",
            json!({ "ResolverRuleId": rules[target.rule], "VPCId": vpcs[target.vpc] }),
        )
        .await;
        assert_eq!(
            string_member(
                answered
                    .get("ResolverRuleAssociation")
                    .unwrap_or(&Value::Null),
                "Id"
            ),
            string_member(&made[row.target], "Id"),
            "case {case:?}: the pair in the documented order must disassociate its association, \
             got {answered}"
        );
    }
}

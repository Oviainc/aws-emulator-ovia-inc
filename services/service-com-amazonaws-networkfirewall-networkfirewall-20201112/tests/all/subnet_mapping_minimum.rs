// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one mapping a firewall must keep: a
//! `DisassociateSubnets` that would leave it with no subnet mapping at all.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.
//!
//! Every subnet id below satisfies the member pattern the model puts on the ids
//! these operations take (`smithy_query(query="shape",shape="AzSubnets")`:
//! `^subnet-[0-9a-f]+$`), so a row exercises the rule rather than that
//! constraint.

use serde_json::{json, Value};

use crate::harness::{error_code, error_message, string_at, Harness};

/// Create a firewall carrying `subnets`.
async fn create_firewall(harness: &Harness, name: &str, policy_arn: &str, subnets: &[&str]) {
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": subnets
                    .iter()
                    .map(|id| json!({ "SubnetId": id }))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
}

/// The subnet ids the firewall `name` currently reports, read back through
/// `DescribeFirewall`.
async fn described_subnet_ids(harness: &Harness, name: &str) -> Vec<String> {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    described["Firewall"]["SubnetMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Firewall.SubnetMappings in {described:?}"))
        .iter()
        .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
        .collect()
}

/// One row: the subnets the firewall carries, and the subnets one
/// `DisassociateSubnets` names.
#[derive(Debug)]
struct Case {
    /// The row's label, which is also the firewall's name.
    label: &'static str,
    initial: &'static [&'static str],
    requested: &'static [&'static str],
    /// The subnets the refusal must name, in the order the request gave them.
    named: &'static [&'static str],
}

/// The rows: a firewall's whole mapping set named by one request, over one, two
/// and three mappings, with the ids named in the stored order and in the reverse
/// of it, and with a request that names the whole set PLUS subnets the firewall
/// does not carry (which remove nothing, so they do not save the call).
fn emptying_cases() -> Vec<Case> {
    vec![
        Case {
            label: "min-one-mapping",
            initial: &["subnet-a0"],
            requested: &["subnet-a0"],
            named: &["subnet-a0"],
        },
        Case {
            label: "min-two-mappings",
            initial: &["subnet-b0", "subnet-b1"],
            requested: &["subnet-b0", "subnet-b1"],
            named: &["subnet-b0", "subnet-b1"],
        },
        Case {
            label: "min-two-mappings-reversed",
            initial: &["subnet-c0", "subnet-c1"],
            requested: &["subnet-c1", "subnet-c0"],
            named: &["subnet-c1", "subnet-c0"],
        },
        Case {
            label: "min-three-mappings",
            initial: &["subnet-d0", "subnet-d1", "subnet-d2"],
            requested: &["subnet-d2", "subnet-d0", "subnet-d1"],
            named: &["subnet-d2", "subnet-d0", "subnet-d1"],
        },
        Case {
            label: "min-whole-set-and-a-stranger",
            initial: &["subnet-e0"],
            requested: &["subnet-e0", "subnet-ef"],
            named: &["subnet-e0"],
        },
    ]
}

/// RULE: a firewall filters through the endpoints its subnet mappings are, so
/// `DisassociateSubnets` never leaves it with none: a request that names every
/// mapping the firewall still has is refused with HTTP 400 /
/// `InvalidRequestException`, nothing is removed, and the firewall keeps
/// reporting the whole set.
#[tokio::test]
async fn disassociating_every_remaining_mapping_is_refused() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("min-mapping-policy").await;

    for case in emptying_cases() {
        create_firewall(&harness, case.label, &policy_arn, case.initial).await;
        let before = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": case.label }))
            .await;
        let token_before = string_at(&before, &["UpdateToken"]).to_string();

        let (status, body) = harness
            .call(
                "DisassociateSubnets",
                &json!({ "FirewallName": case.label, "SubnetIds": case.requested }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {body:?}"
        );
        let expected_message = format!(
            "availability zone subnet(s) cannot be removed as firewall must have at least one \
             subnet mapping, parameter: [{}]",
            case.named.join(", ")
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected the at-least-one-mapping refusal naming the subnets, got \
             {body:?}"
        );

        // Nothing is written on the refusal: the mapping set and the token the
        // firewall carried are exactly as they were.
        let after = described_subnet_ids(&harness, case.label).await;
        assert_eq!(
            after, case.initial,
            "case {case:?}: expected the refused call to leave the mapping set alone, got \
             {after:?}"
        );
        let described = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": case.label }))
            .await;
        assert_eq!(
            string_at(&described, &["UpdateToken"]),
            token_before,
            "case {case:?}: expected the refused call to leave the UpdateToken alone, got \
             {described:?}"
        );
    }
}

/// RULE (the other side of the same invariant): a request that leaves the
/// firewall at least one mapping is served, however many it removes — so the
/// refusal is conditioned on EMPTYING the set and on nothing else, and a
/// firewall shrunk to its last mapping can be grown again and then shrunk the
/// other way.
#[tokio::test]
async fn disassociating_all_but_one_mapping_is_served() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("shrink-policy").await;

    // Rows: the mappings the firewall carries, the ids one call removes, and the
    // single mapping it must be left with.
    let rows: [(&str, &[&str], &[&str], &str); 3] = [
        ("shrink-from-two", &["subnet-10", "subnet-11"], &["subnet-10"], "subnet-11"),
        (
            "shrink-from-three",
            &["subnet-20", "subnet-21", "subnet-22"],
            &["subnet-22", "subnet-20"],
            "subnet-21",
        ),
        (
            "shrink-from-four",
            &["subnet-30", "subnet-31", "subnet-32", "subnet-33"],
            &["subnet-31", "subnet-33", "subnet-30"],
            "subnet-32",
        ),
    ];

    for (label, initial, requested, remaining) in rows {
        create_firewall(&harness, label, &policy_arn, initial).await;
        let disassociated = harness
            .ok(
                "DisassociateSubnets",
                &json!({ "FirewallName": label, "SubnetIds": requested }),
            )
            .await;
        let reported: Vec<String> = disassociated["SubnetMappings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: expected SubnetMappings in {disassociated:?}"))
            .iter()
            .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
            .collect();
        assert_eq!(
            reported,
            vec![remaining.to_string()],
            "case {label}: expected only {remaining} to remain, got {reported:?}"
        );

        // The last mapping cannot go, but the firewall can be grown and then
        // shrunk again — the invariant is about the surviving set, not about the
        // subnet.
        let (status, body) = harness
            .call(
                "DisassociateSubnets",
                &json!({ "FirewallName": label, "SubnetIds": [remaining] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected the last mapping's removal to be refused, got {status} with \
             {body:?}"
        );
        let regrown: &[&str] = &[requested[0]];
        harness
            .ok(
                "AssociateSubnets",
                &json!({
                    "FirewallName": label,
                    "SubnetMappings": regrown
                        .iter()
                        .map(|id| json!({ "SubnetId": id }))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
        let served = harness
            .ok(
                "DisassociateSubnets",
                &json!({ "FirewallName": label, "SubnetIds": [remaining] }),
            )
            .await;
        let left: Vec<String> = served["SubnetMappings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label}: expected SubnetMappings in {served:?}"))
            .iter()
            .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
            .collect();
        assert_eq!(
            left,
            vec![requested[0].to_string()],
            "case {label}: expected the re-associated subnet to be the one left, got {left:?}"
        );
    }
}

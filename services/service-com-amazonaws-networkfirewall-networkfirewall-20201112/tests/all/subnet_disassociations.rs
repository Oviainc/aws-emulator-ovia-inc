// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisassociateSubnets`.
//!
//! Every test drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: NetworkFirewall_20201112.<OperationName>` through the same
//! router the emulator boots, so what is asserted is the WIRE behaviour and not a
//! handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.
//!
//! Every subnet id below satisfies the member pattern the model puts on the ids
//! this operation takes (`smithy_query(query="shape",shape="AzSubnets")`:
//! `^subnet-[0-9a-f]+$`), so a row exercises the rule rather than that
//! constraint.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// Create a firewall carrying `subnets` through `CreateFirewall`.
async fn create_firewall(
    harness: &Harness,
    name: &str,
    policy_arn: &str,
    subnets: &[&str],
    subnet_change_protection: bool,
) -> Value {
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
                "SubnetChangeProtection": subnet_change_protection,
            }),
        )
        .await
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

/// The `(Availability Zone, SubnetId)` pairs `DescribeFirewall` reports a
/// firewall endpoint for.
async fn described_endpoints(harness: &Harness, name: &str) -> Vec<(String, String)> {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    described["FirewallStatus"]["SyncStates"]
        .as_object()
        .unwrap_or_else(|| panic!("expected FirewallStatus.SyncStates in {described:?}"))
        .iter()
        .map(|(zone, sync_state)| {
            (
                zone.clone(),
                string_at(sync_state, &["Attachment", "SubnetId"]).to_string(),
            )
        })
        .collect()
}

/// The subnet ids reported in a `SubnetMappings` response member.
fn reported_subnet_ids(body: &Value, label: &str) -> Vec<String> {
    body["SubnetMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: expected SubnetMappings in {body:?}"))
        .iter()
        .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
        .collect()
}

/// One row of the disassociation table.
#[derive(Debug)]
struct DisassociateCase {
    firewall_name: &'static str,
    /// The subnets the firewall is created with.
    initial: &'static [&'static str],
    /// The subnet ids the `DisassociateSubnets` call names.
    requested: &'static [&'static str],
    /// The subnet ids the firewall must carry afterwards, in order.
    expected: &'static [&'static str],
}

/// RULE: `DisassociateSubnets` takes the named subnets OUT of the firewall's
/// subnet-mapping set — each one it carries loses its mapping and the firewall
/// endpoint in its Availability Zone, each one it does not carry is already not
/// associated and loses nothing — and the firewall then reports exactly the
/// remaining set to any later reader.
///
/// The rows vary what the rule generalises over: how many subnets are named, how
/// many of them the firewall carries, whether the request names a strict subset,
/// every mapping but one, a superset, or nothing the firewall carries at all, and
/// in which order the subnets are named. A request naming the WHOLE set would
/// leave the firewall with no endpoint, which is refused rather than served
/// (`subnet_mapping_minimum`).
#[tokio::test]
async fn disassociate_subnets_removes_the_named_subnets_from_the_mapping_set() {
    let cases = [
        DisassociateCase {
            firewall_name: "disassoc-one-of-two",
            initial: &["subnet-0a1", "subnet-0b2"],
            requested: &["subnet-0a1"],
            expected: &["subnet-0b2"],
        },
        DisassociateCase {
            firewall_name: "disassoc-several-reverse-order",
            initial: &["subnet-1a", "subnet-1b", "subnet-1c", "subnet-1d"],
            requested: &["subnet-1c", "subnet-1a"],
            expected: &["subnet-1b", "subnet-1d"],
        },
        DisassociateCase {
            firewall_name: "disassoc-every-subnet-but-one",
            initial: &["subnet-2a", "subnet-2b", "subnet-2c"],
            requested: &["subnet-2c", "subnet-2b"],
            expected: &["subnet-2a"],
        },
        DisassociateCase {
            firewall_name: "disassoc-partial-overlap",
            initial: &["subnet-3a", "subnet-3b"],
            requested: &["subnet-3b", "subnet-3f"],
            expected: &["subnet-3a"],
        },
        DisassociateCase {
            firewall_name: "disassoc-nothing-associated",
            initial: &["subnet-4a"],
            requested: &["subnet-4e", "subnet-4f"],
            expected: &["subnet-4a"],
        },
    ];

    let harness = Harness::new();
    let policy_arn = harness.shared_policy("disassoc-policy").await;

    for case in &cases {
        create_firewall(
            &harness,
            case.firewall_name,
            &policy_arn,
            case.initial,
            false,
        )
        .await;
        let described = harness
            .ok(
                "DescribeFirewall",
                &json!({ "FirewallName": case.firewall_name }),
            )
            .await;
        let token_before = string_at(&described, &["UpdateToken"]).to_string();

        let disassociated = harness
            .ok(
                "DisassociateSubnets",
                &json!({
                    "FirewallName": case.firewall_name,
                    "SubnetIds": case.requested,
                }),
            )
            .await;

        let expected_arn = format!(
            "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{}",
            case.firewall_name
        );
        assert_eq!(
            string_at(&disassociated, &["FirewallArn"]),
            expected_arn,
            "case {case:?}: expected the response to identify the firewall by ARN, got \
             {disassociated:?}"
        );
        assert_eq!(
            string_at(&disassociated, &["FirewallName"]),
            case.firewall_name,
            "case {case:?}: expected the response to identify the firewall by name, got \
             {disassociated:?}"
        );

        let reported = reported_subnet_ids(&disassociated, case.firewall_name);
        assert_eq!(
            reported, case.expected,
            "case {case:?}: expected the response to report the remaining subnets, got \
             {reported:?}"
        );

        // The response is not a per-call projection: a later reader sees the
        // same mapping set.
        let read_back = described_subnet_ids(&harness, case.firewall_name).await;
        assert_eq!(
            read_back, case.expected,
            "case {case:?}: expected DescribeFirewall to report the remaining subnets, got \
             {read_back:?}"
        );

        // The endpoint each removed subnet carried is gone with it: one endpoint
        // per remaining subnet, each still in its own Availability Zone.
        let endpoints = described_endpoints(&harness, case.firewall_name).await;
        let endpoint_subnets: HashSet<&str> =
            endpoints.iter().map(|(_, subnet)| subnet.as_str()).collect();
        let expected_subnets: HashSet<&str> = case.expected.iter().copied().collect();
        assert_eq!(
            endpoint_subnets, expected_subnets,
            "case {case:?}: expected one endpoint per remaining subnet, got {endpoints:?}"
        );
        let zones: HashSet<&str> = endpoints.iter().map(|(zone, _)| zone.as_str()).collect();
        assert_eq!(
            zones.len(),
            case.expected.len(),
            "case {case:?}: expected every remaining endpoint in its own Availability Zone, got \
             {endpoints:?}"
        );

        // The token marks the state of the resource: it moves when the call
        // changed the mapping set, and stays when the firewall carried none of
        // the named subnets.
        let token_after = string_at(&disassociated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&token_after),
            "case {case:?}: expected UpdateToken in the modelled UUID form, got {token_after:?}"
        );
        let changed = case.expected.len() < case.initial.len();
        assert_eq!(
            token_after != token_before,
            changed,
            "case {case:?}: expected the UpdateToken to change exactly when the mapping set did \
             (before {token_before:?}, after {token_after:?})"
        );
    }
}

/// RULE: the mapping set is a SET, so associating and disassociating the same
/// subnets compose: a subnet removed can be associated again, and the
/// Availability Zone its endpoint occupied is free for whatever is associated
/// next.
#[tokio::test]
async fn a_disassociated_subnet_can_be_associated_again() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("roundtrip-policy").await;
    create_firewall(
        &harness,
        "roundtrip-fw",
        &policy_arn,
        &["subnet-e0", "subnet-e1"],
        false,
    )
    .await;

    let zones_before: HashSet<String> = described_endpoints(&harness, "roundtrip-fw")
        .await
        .into_iter()
        .map(|(zone, _)| zone)
        .collect();

    harness
        .ok(
            "DisassociateSubnets",
            &json!({ "FirewallName": "roundtrip-fw", "SubnetIds": ["subnet-e0"] }),
        )
        .await;
    assert_eq!(
        described_subnet_ids(&harness, "roundtrip-fw").await,
        vec!["subnet-e1".to_string()],
        "case after the disassociation: expected only the subnet that was kept"
    );

    // The freed zone is taken by the next association, so the firewall still
    // spans one subnet per zone and no zone is lost to the removed endpoint.
    let reassociated = harness
        .ok(
            "AssociateSubnets",
            &json!({
                "FirewallName": "roundtrip-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-e0" }, { "SubnetId": "subnet-e2" }],
            }),
        )
        .await;
    let reported = reported_subnet_ids(&reassociated, "re-association");
    let reported_set: HashSet<&str> = reported.iter().map(String::as_str).collect();
    let expected_set: HashSet<&str> = ["subnet-e0", "subnet-e1", "subnet-e2"].into_iter().collect();
    assert_eq!(
        reported_set, expected_set,
        "case after the re-association: expected the subnet to be associable again, got \
         {reported:?}"
    );
    let endpoints = described_endpoints(&harness, "roundtrip-fw").await;
    let zones_after: HashSet<String> = endpoints.iter().map(|(zone, _)| zone.clone()).collect();
    assert_eq!(
        zones_after.len(),
        3,
        "case after the re-association: expected one Availability Zone per subnet, got \
         {endpoints:?}"
    );
    assert!(
        zones_before.is_subset(&zones_after),
        "case after the re-association: expected the freed zone to be reused, got {zones_after:?} \
         against {zones_before:?}"
    );
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    label: &'static str,
    request: Value,
    code: &'static str,
    /// The message, where this service fixes it; `None` where the text is not a
    /// requirement and only the status and code are.
    message: Option<&'static str>,
}

/// RULE: a subnet-disassociation request the service cannot carry out is refused
/// with the modelled client error for that defect, and every firewall's mapping
/// set is left exactly as it was.
#[tokio::test]
async fn refused_disassociations_change_nothing() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("disassoc-refusal-policy").await;
    create_firewall(
        &harness,
        "open-dfw",
        &policy_arn,
        &["subnet-d00", "subnet-d01"],
        false,
    )
    .await;
    create_firewall(&harness, "other-dfw", &policy_arn, &["subnet-d02"], false).await;
    create_firewall(
        &harness,
        "protected-dfw",
        &policy_arn,
        &["subnet-d03"],
        true,
    )
    .await;
    let open_arn = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/open-dfw");

    let cases = [
        RefusalCase {
            label: "the firewall is protected against subnet changes",
            request: json!({ "FirewallName": "protected-dfw", "SubnetIds": ["subnet-d03"] }),
            code: "InvalidOperationException",
            message: Some("Cannot DisassociateSubnets because SubnetChangeProtection is on"),
        },
        RefusalCase {
            label: "a stale update token",
            request: json!({
                "FirewallName": "open-dfw",
                "UpdateToken": "00000000-0000-0000-0000-000000000000",
                "SubnetIds": ["subnet-d00"],
            }),
            code: "InvalidTokenException",
            message: Some("Update token isn't valid."),
        },
        RefusalCase {
            label: "no firewall of that name",
            request: json!({ "FirewallName": "absent-dfw", "SubnetIds": ["subnet-d00"] }),
            code: "ResourceNotFoundException",
            message: Some(NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            label: "no firewall identified at all",
            request: json!({ "SubnetIds": ["subnet-d00"] }),
            code: "InvalidRequestException",
            message: Some("At least one of FirewallName or FirewallArn must be set"),
        },
        RefusalCase {
            label: "name and ARN identify different firewalls",
            request: json!({
                "FirewallName": "other-dfw",
                "FirewallArn": open_arn,
                "SubnetIds": ["subnet-d00"],
            }),
            code: "InvalidRequestException",
            message: Some("Name and Arn(Name in Arn) do not match"),
        },
        RefusalCase {
            label: "no subnet named",
            request: json!({ "FirewallName": "open-dfw", "SubnetIds": [] }),
            code: "InvalidRequestException",
            message: Some("Specify at least one subnet."),
        },
        RefusalCase {
            label: "one subnet named twice",
            request: json!({
                "FirewallName": "open-dfw",
                "SubnetIds": ["subnet-d00", "subnet-d00"],
            }),
            code: "InvalidRequestException",
            message: Some("Subnet is specified more than once, parameter: [subnet-d00]"),
        },
    ];

    for case in &cases {
        let before: Vec<Vec<String>> = vec![
            described_subnet_ids(&harness, "open-dfw").await,
            described_subnet_ids(&harness, "other-dfw").await,
            described_subnet_ids(&harness, "protected-dfw").await,
        ];

        let (status, body) = harness.call("DisassociateSubnets", &case.request).await;
        assert_eq!(
            status, 400,
            "case {} ({case:?}): expected HTTP 400, got {status} with body {body:?}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {case:?}: expected error code {}, got {body:?}",
            case.code
        );
        if let Some(message) = case.message {
            assert_eq!(
                error_message(&body),
                message,
                "case {case:?}: expected message {message:?}, got {body:?}"
            );
        }

        let after: Vec<Vec<String>> = vec![
            described_subnet_ids(&harness, "open-dfw").await,
            described_subnet_ids(&harness, "other-dfw").await,
            described_subnet_ids(&harness, "protected-dfw").await,
        ];
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave every firewall's SubnetMappings \
             unchanged, got {after:?}"
        );
    }
}

/// RULE: the update token a read of the firewall hands out is accepted by the
/// next disassociation, and that disassociation's own token by the one after it —
/// the optimistic lock advances with the resource, whichever call moved it.
#[tokio::test]
async fn the_current_update_token_is_accepted() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("disassoc-token-policy").await;
    let subnets = ["subnet-b0", "subnet-b1", "subnet-b2", "subnet-b3"];
    create_firewall(&harness, "disassoc-token-fw", &policy_arn, &subnets, false).await;

    let described = harness
        .ok(
            "DescribeFirewall",
            &json!({ "FirewallName": "disassoc-token-fw" }),
        )
        .await;
    let mut token = string_at(&described, &["UpdateToken"]).to_string();

    for subnet in &subnets[1..] {
        let disassociated = harness
            .ok(
                "DisassociateSubnets",
                &json!({
                    "FirewallName": "disassoc-token-fw",
                    "UpdateToken": token,
                    "SubnetIds": [subnet],
                }),
            )
            .await;
        let next = string_at(&disassociated, &["UpdateToken"]).to_string();
        assert_ne!(
            next, token,
            "case {subnet}: expected the disassociation to re-mint the UpdateToken, got {next:?}"
        );
        token = next;
    }

    assert_eq!(
        described_subnet_ids(&harness, "disassoc-token-fw").await,
        vec!["subnet-b0".to_string()],
        "expected each accepted disassociation to persist"
    );
}

/// RULE: a firewall takes one subnet per Availability Zone it spans, and a
/// disassociation frees the zone its endpoint held — so a firewall filled to the
/// bound takes a new subnet exactly after one is removed.
///
/// The bound is `state::MAX_AVAILABILITY_ZONES`, the number of zones
/// `availability_zone_of` names in a region.
#[tokio::test]
async fn disassociating_frees_a_zone_at_the_bound() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("disassoc-zone-policy").await;
    let bound = state::MAX_AVAILABILITY_ZONES;
    let subnets: Vec<String> = (0..=bound)
        .map(|index| format!("subnet-c{index:03x}"))
        .collect();
    let filled: Vec<&str> = subnets[..bound].iter().map(String::as_str).collect();
    create_firewall(&harness, "disassoc-zone-fw", &policy_arn, &filled, false).await;

    // At the bound the next subnet does not fit.
    let (status, body) = harness
        .call(
            "AssociateSubnets",
            &json!({
                "FirewallName": "disassoc-zone-fw",
                "SubnetMappings": [{ "SubnetId": subnets[bound].as_str() }],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case at the bound ({bound}): expected HTTP 400, got {status} with body {body:?}"
    );

    // One subnet removed frees exactly one zone, so the same association is
    // then accepted and the firewall still spans the bound.
    harness
        .ok(
            "DisassociateSubnets",
            &json!({
                "FirewallName": "disassoc-zone-fw",
                "SubnetIds": [subnets[0].as_str()],
            }),
        )
        .await;
    let associated = harness
        .ok(
            "AssociateSubnets",
            &json!({
                "FirewallName": "disassoc-zone-fw",
                "SubnetMappings": [{ "SubnetId": subnets[bound].as_str() }],
            }),
        )
        .await;
    let reported = reported_subnet_ids(&associated, "after the disassociation");
    assert_eq!(
        reported.len(),
        bound,
        "case after the disassociation ({bound}): expected the firewall to span the bound again, \
         got {reported:?}"
    );
    assert!(
        reported.contains(&subnets[bound]) && !reported.contains(&subnets[0]),
        "case after the disassociation ({bound}): expected the removed subnet replaced by the new \
         one, got {reported:?}"
    );
    let endpoints = described_endpoints(&harness, "disassoc-zone-fw").await;
    let zones: HashSet<&str> = endpoints.iter().map(|(zone, _)| zone.as_str()).collect();
    assert_eq!(
        zones.len(),
        bound,
        "case after the disassociation ({bound}): expected one endpoint per zone, got \
         {endpoints:?}"
    );
}

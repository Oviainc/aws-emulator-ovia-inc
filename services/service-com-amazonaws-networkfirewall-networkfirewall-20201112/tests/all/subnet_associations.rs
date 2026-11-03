// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AssociateSubnets`.
//!
//! Every test drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: NetworkFirewall_20201112.<OperationName>` through the same
//! router the emulator boots, so what is asserted is the WIRE behaviour and not a
//! handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT, NOT_FOUND_MESSAGE, REGION,
};

/// Create a firewall with `subnets` and return its create response.
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

/// The `SubnetMappings` the firewall `name` currently reports, read back through
/// `DescribeFirewall`.
async fn described_mappings(harness: &Harness, name: &str) -> Vec<Value> {
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": name }))
        .await;
    described["Firewall"]["SubnetMappings"]
        .as_array()
        .unwrap_or_else(|| panic!("expected Firewall.SubnetMappings in {described:?}"))
        .clone()
}

/// The subnet ids `DescribeFirewall` reports an endpoint for, one per
/// Availability Zone key of `FirewallStatus.SyncStates`.
async fn described_endpoint_subnets(harness: &Harness, name: &str) -> Vec<(String, String, String)> {
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
                string_at(sync_state, &["Attachment", "EndpointId"]).to_string(),
            )
        })
        .collect()
}

/// One row of the association table.
#[derive(Debug)]
struct AssociateCase {
    firewall_name: &'static str,
    /// The subnets the firewall is created with.
    initial: &'static [&'static str],
    /// `(SubnetId, IPAddressType)` pairs the `AssociateSubnets` call names.
    requested: &'static [(&'static str, Option<&'static str>)],
    /// The subnet ids the firewall must carry afterwards, in order.
    expected: &'static [&'static str],
}

/// RULE: `AssociateSubnets` makes the named subnets members of the firewall's
/// subnet-mapping SET — each one it did not already carry gains its own
/// Availability Zone and firewall endpoint, each one it already carries is left
/// as it is — and the firewall then reports exactly that set to any later reader.
///
/// The rows vary what the rule generalises over: how many subnets are named, how
/// many of them are new, whether the request overlaps the existing set entirely
/// or partly, and which `IPAddressType` each mapping carries.
#[tokio::test]
async fn associate_subnets_unions_the_named_subnets_into_the_mapping_set() {
    // The `IPAddressType` values below are the members of that enum in this
    // service's model (`smithy_query(query="shape",shape="IPAddressType")`:
    // DUALSTACK, IPV4, IPV6).
    let cases = [
        AssociateCase {
            firewall_name: "assoc-one-new",
            initial: &["subnet-0a1"],
            requested: &[("subnet-0b2", None)],
            expected: &["subnet-0a1", "subnet-0b2"],
        },
        AssociateCase {
            firewall_name: "assoc-three-new-typed",
            initial: &["subnet-1a"],
            requested: &[
                ("subnet-1b", Some("IPV4")),
                ("subnet-1c", Some("IPV6")),
                ("subnet-1d", Some("DUALSTACK")),
            ],
            expected: &["subnet-1a", "subnet-1b", "subnet-1c", "subnet-1d"],
        },
        AssociateCase {
            firewall_name: "assoc-partial-overlap",
            initial: &["subnet-2a", "subnet-2b"],
            requested: &[("subnet-2b", None), ("subnet-2c", Some("IPV4"))],
            expected: &["subnet-2a", "subnet-2b", "subnet-2c"],
        },
        AssociateCase {
            firewall_name: "assoc-already-associated",
            initial: &["subnet-3a", "subnet-3b"],
            requested: &[("subnet-3a", None), ("subnet-3b", None)],
            expected: &["subnet-3a", "subnet-3b"],
        },
    ];

    let harness = Harness::new();
    let policy = harness.create_policy("assoc-policy").await;
    let policy_arn =
        string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

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

        let request = json!({
            "FirewallName": case.firewall_name,
            "SubnetMappings": case
                .requested
                .iter()
                .map(|(id, ip_address_type)| match ip_address_type {
                    Some(kind) => json!({ "SubnetId": id, "IPAddressType": kind }),
                    None => json!({ "SubnetId": id }),
                })
                .collect::<Vec<_>>(),
        });
        let associated = harness.ok("AssociateSubnets", &request).await;

        let expected_arn = format!(
            "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{}",
            case.firewall_name
        );
        assert_eq!(
            string_at(&associated, &["FirewallArn"]),
            expected_arn,
            "case {case:?}: expected the response to identify the firewall by ARN, got \
             {associated:?}"
        );
        assert_eq!(
            string_at(&associated, &["FirewallName"]),
            case.firewall_name,
            "case {case:?}: expected the response to identify the firewall by name, got \
             {associated:?}"
        );

        let reported: Vec<String> = associated["SubnetMappings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected SubnetMappings in {associated:?}"))
            .iter()
            .map(|mapping| string_at(mapping, &["SubnetId"]).to_string())
            .collect();
        assert_eq!(
            reported, case.expected,
            "case {case:?}: expected the response to report the union of the firewall's subnets, \
             got {reported:?}"
        );

        // The response is not a per-call projection: a later reader sees the
        // same mapping set, members and all.
        let read_back = described_mappings(&harness, case.firewall_name).await;
        assert_eq!(
            read_back,
            associated["SubnetMappings"].as_array().unwrap().clone(),
            "case {case:?}: expected DescribeFirewall to report the associated mappings, got \
             {read_back:?}"
        );

        // Every requested mapping's `IPAddressType` is the one it was associated
        // with, and a subnet that was already associated keeps its own.
        for (subnet_id, ip_address_type) in case.requested {
            let stored = read_back
                .iter()
                .find(|mapping| string_at(mapping, &["SubnetId"]) == *subnet_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected subnet {subnet_id} in {read_back:?}")
                });
            let already_associated = case.initial.contains(subnet_id);
            let expected_type = if already_associated {
                None
            } else {
                *ip_address_type
            };
            assert_eq!(
                stored.get("IPAddressType").and_then(Value::as_str),
                expected_type,
                "case {case:?}: expected subnet {subnet_id} to carry IPAddressType \
                 {expected_type:?}, got {stored:?}"
            );
        }

        // One firewall endpoint per subnet, each in its own Availability Zone.
        let endpoints = described_endpoint_subnets(&harness, case.firewall_name).await;
        let endpoint_subnets: HashSet<&str> =
            endpoints.iter().map(|(_, subnet, _)| subnet.as_str()).collect();
        let expected_subnets: HashSet<&str> = case.expected.iter().copied().collect();
        assert_eq!(
            endpoint_subnets, expected_subnets,
            "case {case:?}: expected one endpoint per associated subnet, got {endpoints:?}"
        );
        let zones: HashSet<&str> = endpoints.iter().map(|(zone, ..)| zone.as_str()).collect();
        assert_eq!(
            zones.len(),
            case.expected.len(),
            "case {case:?}: expected every endpoint in its own Availability Zone, got \
             {endpoints:?}"
        );
        let endpoint_ids: HashSet<&str> = endpoints
            .iter()
            .map(|(_, _, endpoint)| endpoint.as_str())
            .collect();
        assert_eq!(
            endpoint_ids.len(),
            case.expected.len(),
            "case {case:?}: expected a distinct endpoint id per subnet, got {endpoints:?}"
        );

        // The token marks the state of the resource: it moves when the call
        // changed the mapping set, and stays when every named subnet was already
        // associated.
        let token_after = string_at(&associated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&token_after),
            "case {case:?}: expected UpdateToken in the modelled UUID form, got {token_after:?}"
        );
        let changed = case.expected.len() > case.initial.len();
        assert_eq!(
            token_after != token_before,
            changed,
            "case {case:?}: expected the UpdateToken to change exactly when the mapping set did \
             (before {token_before:?}, after {token_after:?})"
        );
    }
}

/// RULE: a firewall endpoint this call creates is not ready at once — the
/// attachment starts `CREATING` and the firewall is `PROVISIONING` until the span
/// the clock owes it elapses, after which every endpoint reports `READY`.
#[tokio::test]
async fn a_newly_associated_endpoint_provisions_then_becomes_ready() {
    let harness = Harness::new();
    let policy = harness.create_policy("provisioning-policy").await;
    let policy_arn =
        string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    create_firewall(&harness, "grow-fw", &policy_arn, &["subnet-aaa"], false).await;

    // Wait the firewall's own provisioning out, so what the association does to
    // the status cannot be confused with the create's.
    tokio::time::sleep(state::FIREWALL_PROVISIONING_SPAN + Duration::from_millis(200)).await;
    let ready = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "grow-fw" }))
        .await;
    assert_eq!(
        ready["FirewallStatus"]["Status"],
        json!("READY"),
        "expected the created firewall to be READY once its span elapsed, got {ready:?}"
    );

    harness
        .ok(
            "AssociateSubnets",
            &json!({
                "FirewallName": "grow-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-bbb" }],
            }),
        )
        .await;

    // A caller acting straight after the association still sees the transient
    // state: the span is never zero.
    let immediate = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "grow-fw" }))
        .await;
    assert_eq!(
        immediate["FirewallStatus"]["Status"],
        json!("PROVISIONING"),
        "expected the firewall to be PROVISIONING while the new endpoint comes up, got \
         {immediate:?}"
    );
    let creating: Vec<&str> = immediate["FirewallStatus"]["SyncStates"]
        .as_object()
        .expect("SyncStates")
        .values()
        .filter(|sync_state| string_at(sync_state, &["Attachment", "Status"]) == "CREATING")
        .map(|sync_state| string_at(sync_state, &["Attachment", "SubnetId"]))
        .collect();
    assert_eq!(
        creating,
        vec!["subnet-bbb"],
        "expected exactly the newly associated subnet's endpoint to be CREATING, got \
         {immediate:?}"
    );

    tokio::time::sleep(state::FIREWALL_PROVISIONING_SPAN + Duration::from_millis(200)).await;
    let settled = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "grow-fw" }))
        .await;
    assert_eq!(
        settled["FirewallStatus"]["Status"],
        json!("READY"),
        "expected the firewall to be READY once the new endpoint's span elapsed, got {settled:?}"
    );
    for (zone, sync_state) in settled["FirewallStatus"]["SyncStates"]
        .as_object()
        .expect("SyncStates")
    {
        assert_eq!(
            sync_state["Attachment"]["Status"],
            json!("READY"),
            "case zone {zone}: expected the endpoint to be READY, got {sync_state:?}"
        );
    }
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

/// RULE: a subnet-association request the service cannot carry out is refused
/// with the modelled client error for that defect, and the firewall's mapping set
/// is left exactly as it was.
#[tokio::test]
async fn refused_associations_change_nothing() {
    let harness = Harness::new();
    let policy = harness.create_policy("refusal-policy").await;
    let policy_arn =
        string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    create_firewall(&harness, "open-fw", &policy_arn, &["subnet-f00"], false).await;
    create_firewall(&harness, "other-fw", &policy_arn, &["subnet-f01"], false).await;
    create_firewall(
        &harness,
        "protected-fw",
        &policy_arn,
        &["subnet-f02"],
        true,
    )
    .await;
    let open_arn = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/open-fw");

    let cases = [
        RefusalCase {
            label: "the firewall is protected against subnet changes",
            request: json!({
                "FirewallName": "protected-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-f99" }],
            }),
            code: "InvalidOperationException",
            message: None,
        },
        RefusalCase {
            label: "a stale update token",
            request: json!({
                "FirewallName": "open-fw",
                "UpdateToken": "00000000-0000-0000-0000-000000000000",
                "SubnetMappings": [{ "SubnetId": "subnet-f98" }],
            }),
            code: "InvalidTokenException",
            message: Some("Update token isn't valid."),
        },
        RefusalCase {
            label: "no firewall of that name",
            request: json!({
                "FirewallName": "absent-fw",
                "SubnetMappings": [{ "SubnetId": "subnet-f97" }],
            }),
            code: "ResourceNotFoundException",
            message: Some(NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            label: "no firewall identified at all",
            request: json!({ "SubnetMappings": [{ "SubnetId": "subnet-f96" }] }),
            code: "InvalidRequestException",
            message: Some("At least one of FirewallName or FirewallArn must be set"),
        },
        RefusalCase {
            label: "name and ARN identify different firewalls",
            request: json!({
                "FirewallName": "other-fw",
                "FirewallArn": open_arn,
                "SubnetMappings": [{ "SubnetId": "subnet-f95" }],
            }),
            code: "InvalidRequestException",
            message: Some("Name and Arn(Name in Arn) do not match"),
        },
        RefusalCase {
            label: "no subnet named",
            request: json!({ "FirewallName": "open-fw", "SubnetMappings": [] }),
            code: "InvalidRequestException",
            message: Some("Specify at least one subnet."),
        },
        RefusalCase {
            label: "one subnet named twice",
            request: json!({
                "FirewallName": "open-fw",
                "SubnetMappings": [
                    { "SubnetId": "subnet-f94" },
                    { "SubnetId": "subnet-f94" },
                ],
            }),
            code: "InvalidRequestException",
            message: Some("Subnet is specified more than once, parameter: [subnet-f94]"),
        },
    ];

    for case in &cases {
        let before: Vec<Vec<Value>> = vec![
            described_mappings(&harness, "open-fw").await,
            described_mappings(&harness, "other-fw").await,
            described_mappings(&harness, "protected-fw").await,
        ];

        let (status, body) = harness.call("AssociateSubnets", &case.request).await;
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

        let after: Vec<Vec<Value>> = vec![
            described_mappings(&harness, "open-fw").await,
            described_mappings(&harness, "other-fw").await,
            described_mappings(&harness, "protected-fw").await,
        ];
        assert_eq!(
            after, before,
            "case {case:?}: expected the refusal to leave every firewall's SubnetMappings \
             unchanged, got {after:?}"
        );
    }
}

/// RULE: the update token a read of the firewall hands out is accepted by the
/// next association, and that association's own token by the one after it — the
/// optimistic lock advances with the resource.
#[tokio::test]
async fn the_current_update_token_is_accepted() {
    let harness = Harness::new();
    let policy = harness.create_policy("token-policy").await;
    let policy_arn =
        string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    create_firewall(&harness, "token-fw", &policy_arn, &["subnet-a0"], false).await;

    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "token-fw" }))
        .await;
    let mut token = string_at(&described, &["UpdateToken"]).to_string();

    for subnet in ["subnet-a1", "subnet-a2", "subnet-a3"] {
        let associated = harness
            .ok(
                "AssociateSubnets",
                &json!({
                    "FirewallName": "token-fw",
                    "UpdateToken": token,
                    "SubnetMappings": [{ "SubnetId": subnet }],
                }),
            )
            .await;
        let next = string_at(&associated, &["UpdateToken"]).to_string();
        assert_ne!(
            next, token,
            "case {subnet}: expected the association to re-mint the UpdateToken, got {next:?}"
        );
        token = next;
    }

    let mappings = described_mappings(&harness, "token-fw").await;
    let ids: Vec<&str> = mappings
        .iter()
        .map(|mapping| string_at(mapping, &["SubnetId"]))
        .collect();
    assert_eq!(
        ids,
        vec!["subnet-a0", "subnet-a1", "subnet-a2", "subnet-a3"],
        "expected each accepted association to persist, got {mappings:?}"
    );
}

/// RULE: a firewall takes one subnet per Availability Zone it spans, so the
/// mapping set is accepted up to that many subnets and a request that would push
/// it past the bound is refused with the mapping set untouched.
///
/// The bound is `state::MAX_AVAILABILITY_ZONES`, the number of zones
/// `availability_zone_of` names in a region.
#[tokio::test]
async fn a_firewall_spans_at_most_one_subnet_per_availability_zone() {
    let harness = Harness::new();
    let policy = harness.create_policy("zone-policy").await;
    let policy_arn =
        string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    let bound = state::MAX_AVAILABILITY_ZONES;
    let subnets: Vec<String> = (0..=bound).map(|index| format!("subnet-{index:04x}")).collect();

    create_firewall(
        &harness,
        "zone-fw",
        &policy_arn,
        &[subnets[0].as_str()],
        false,
    )
    .await;

    // Up to the bound: accepted, one endpoint per zone.
    let to_the_bound: Vec<Value> = subnets[1..bound]
        .iter()
        .map(|id| json!({ "SubnetId": id }))
        .collect();
    let associated = harness
        .ok(
            "AssociateSubnets",
            &json!({ "FirewallName": "zone-fw", "SubnetMappings": to_the_bound }),
        )
        .await;
    assert_eq!(
        associated["SubnetMappings"].as_array().map(Vec::len),
        Some(bound),
        "case at the bound ({bound}): expected every subnet associated, got {associated:?}"
    );
    let endpoints = described_endpoint_subnets(&harness, "zone-fw").await;
    assert_eq!(
        endpoints.len(),
        bound,
        "case at the bound ({bound}): expected one endpoint per subnet, got {endpoints:?}"
    );

    // One past the bound: refused, and the firewall keeps the subnets it had.
    let (status, body) = harness
        .call(
            "AssociateSubnets",
            &json!({
                "FirewallName": "zone-fw",
                "SubnetMappings": [{ "SubnetId": subnets[bound].as_str() }],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one past the bound ({}): expected HTTP 400, got {status} with body {body:?}",
        bound + 1
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "case one past the bound ({}): expected InvalidRequestException, got {body:?}",
        bound + 1
    );
    let mappings = described_mappings(&harness, "zone-fw").await;
    assert_eq!(
        mappings.len(),
        bound,
        "case one past the bound ({}): expected the refusal to change nothing, got {mappings:?}",
        bound + 1
    );
}

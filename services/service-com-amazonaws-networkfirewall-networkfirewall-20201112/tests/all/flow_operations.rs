// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the flow-operation slice: `StartFlowFlush` and the
//! `DescribeFlowOperation` that reads what it started back.
//!
//! The rule under test is the asynchronous start/describe contract: a start
//! command persists ONE flow operation per call — a freshly minted
//! `FlowOperationId`, the scope and the filters the call defined it with, and the
//! `IN_PROGRESS` status it begins in — and the describe command reports exactly
//! that record back, moving it to `COMPLETED` once the operation's span has
//! elapsed.
//!
//! Each test is a TABLE whose rows span the class the rule covers: how many
//! filters, which filter members, which scope member, whether a minimum flow age
//! is given. The recorded request is one row among them.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, error_message, is_uuid_form, string_at, Harness, ACCOUNT,
    NOT_FOUND_MESSAGE as NOT_FOUND, REGION,
};

/// Create a firewall on a fresh policy and return its ARN.
async fn firewall_arn(harness: &Harness, name: &str) -> String {
    let policy_arn = harness.shared_policy(&format!("{name}-policy")).await;
    let created = harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": name,
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": format!("subnet-{name}") }],
            }),
        )
        .await;
    string_at(&created, &["Firewall", "FirewallArn"]).to_string()
}

/// `n` distinct flow filters, each narrowing a different source address.
fn distinct_filters(n: usize) -> Vec<Value> {
    (1..=n)
        .map(|index| json!({ "SourceAddress": { "AddressDefinition": format!("10.0.0.{index}/32") } }))
        .collect()
}

/// One row of the start -> describe round-trip table.
#[derive(Debug)]
struct StartCase {
    /// What the row varies, for the assertion messages.
    label: &'static str,
    /// The `FlowFilters` the start command defines the operation with.
    filters: Vec<Value>,
    /// `MinimumFlowAgeInSeconds`, when the row gives one.
    minimum_flow_age: Option<i64>,
    /// The scope members the row adds to the request, as `(member, value)`.
    scope: Vec<(&'static str, &'static str)>,
}

/// RULE: a flow start command persists one flow operation per call — a fresh
/// server-generated id, the scope and the filters the call defined it with, and
/// the status the asynchronous operation begins in — and a later describe of that
/// `FirewallArn` + `FlowOperationId` pair reports exactly what was stored.
///
/// The rows vary what the rule generalises over: the number of filters (one, two,
/// the documented ceiling of twenty), which filter members each carries
/// (addresses, ports, port ranges, protocol numbers, `ANY`), whether a minimum
/// flow age is given, and which scope member narrows the operation
/// (`AvailabilityZone`, `AvailabilityZoneId`, `VpcEndpointId`,
/// `VpcEndpointAssociationArn`, or none at all).
///
/// The filter catalogue comes from the model: `FlowFilter` members
/// `SourceAddress`/`DestinationAddress` (`Address$AddressDefinition`, "an IP
/// address or a block of IP addresses in CIDR notation"),
/// `SourcePort`/`DestinationPort` ("an individual port, for example 1994 ... a
/// port range, for example 1990:1994 ... to match with any port, specify ANY")
/// and `Protocols` ("the assigned internet protocol number (IANA) for each
/// protocol") — `smithy_query(query="shape",shape="FlowFilter")`.
#[tokio::test]
async fn a_started_flow_flush_reads_back_identically() {
    let cases = vec![
        StartCase {
            label: "one source-address filter, no scope",
            filters: json!([{ "SourceAddress": { "AddressDefinition": "192.0.2.0/24" } }])
                .as_array()
                .expect("filter array")
                .clone(),
            minimum_flow_age: None,
            scope: vec![],
        },
        StartCase {
            label: "address pair with single ports, zone-scoped",
            filters: json!([{
                "SourceAddress": { "AddressDefinition": "192.0.2.44/32" },
                "DestinationAddress": { "AddressDefinition": "198.51.100.0/24" },
                "SourcePort": "1994",
                "DestinationPort": "443",
            }])
            .as_array()
            .expect("filter array")
            .clone(),
            minimum_flow_age: Some(60),
            scope: vec![("AvailabilityZone", "eu-west-1a")],
        },
        StartCase {
            label: "port ranges, ANY and protocol numbers, zone-id-scoped",
            filters: json!([
                {
                    "SourcePort": "1990:1994",
                    "DestinationPort": "ANY",
                    "Protocols": ["6", "17"],
                },
                {
                    "DestinationAddress": {
                        "AddressDefinition": "1111:0000:0000:0000:0000:0000:0000:0111/128",
                    },
                    "Protocols": ["1"],
                },
            ])
            .as_array()
            .expect("filter array")
            .clone(),
            minimum_flow_age: Some(0),
            scope: vec![("AvailabilityZoneId", "euw1-az1")],
        },
        StartCase {
            label: "endpoint-scoped, minimum age given",
            filters: json!([{ "SourceAddress": { "AddressDefinition": "10.1.2.3/32" } }])
                .as_array()
                .expect("filter array")
                .clone(),
            minimum_flow_age: Some(3_600),
            scope: vec![("VpcEndpointId", "vpce-0123456789abcdef0")],
        },
        StartCase {
            label: "endpoint-association-scoped",
            filters: json!([{ "DestinationPort": "ANY" }])
                .as_array()
                .expect("filter array")
                .clone(),
            minimum_flow_age: None,
            scope: vec![(
                "VpcEndpointAssociationArn",
                "arn:aws:network-firewall:eu-west-1:000000000000:vpc-endpoint-association/assoc",
            )],
        },
        StartCase {
            label: "the documented ceiling of twenty filters",
            filters: distinct_filters(state::MAX_FLOW_FILTERS),
            minimum_flow_age: Some(1),
            scope: vec![],
        },
    ];

    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-round-trip").await;

    for case in &cases {
        let mut request = json!({ "FirewallArn": arn, "FlowFilters": case.filters });
        if let Some(age) = case.minimum_flow_age {
            request["MinimumFlowAgeInSeconds"] = json!(age);
        }
        for (member, value) in &case.scope {
            request[*member] = json!(value);
        }

        let started = harness.ok("StartFlowFlush", &request).await;
        assert_eq!(
            started.get("FirewallArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: expected the start to report the stored firewall ARN, got {started:?}",
            case.label
        );
        let operation_id = string_at(&started, &["FlowOperationId"]).to_string();
        assert!(
            is_uuid_form(&operation_id),
            "case {}: expected FlowOperationId in the modelled UUID form, got {operation_id:?}",
            case.label
        );
        assert_eq!(
            started.get("FlowOperationStatus"),
            Some(&json!(state::FLOW_OPERATION_IN_PROGRESS)),
            "case {}: expected the asynchronous operation to start IN_PROGRESS, got {started:?}",
            case.label
        );

        let described = harness
            .ok(
                "DescribeFlowOperation",
                &json!({ "FirewallArn": arn, "FlowOperationId": operation_id }),
            )
            .await;
        assert_eq!(
            described.get("FirewallArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {}: expected the describe to report the stored firewall ARN, got {described:?}",
            case.label
        );
        assert_eq!(
            described.get("FlowOperationId").and_then(Value::as_str),
            Some(operation_id.as_str()),
            "case {}: expected the describe to report the id it was asked for, got {described:?}",
            case.label
        );
        assert_eq!(
            described.get("FlowOperationType"),
            Some(&json!(state::FLOW_OPERATION_FLUSH)),
            "case {}: expected a flush to be typed FLOW_FLUSH, got {described:?}",
            case.label
        );
        // The span is never zero, so a caller describing straight after the
        // start still observes the transient status.
        assert_eq!(
            described.get("FlowOperationStatus"),
            Some(&json!(state::FLOW_OPERATION_IN_PROGRESS)),
            "case {}: expected the operation to still be IN_PROGRESS, got {described:?}",
            case.label
        );
        assert_eq!(
            described.get("StatusMessage"),
            None,
            "case {}: StatusMessage reports why an operation FAILED, and this one did not: \
             got {described:?}",
            case.label
        );
        assert!(
            described.get("FlowRequestTimestamp").is_some(),
            "case {}: expected a FlowRequestTimestamp, got {described:?}",
            case.label
        );
        assert_eq!(
            described
                .get("FlowOperation")
                .and_then(|operation| operation.get("FlowFilters")),
            Some(&json!(case.filters)),
            "case {}: expected the stored filters to round-trip, got {described:?}",
            case.label
        );
        assert_eq!(
            described
                .get("FlowOperation")
                .and_then(|operation| operation.get("MinimumFlowAgeInSeconds"))
                .and_then(Value::as_i64),
            case.minimum_flow_age,
            "case {}: expected MinimumFlowAgeInSeconds to round-trip, got {described:?}",
            case.label
        );
        // The scope reported is the one the START command defined, not whatever
        // the describe request carried: this read named none of these members.
        for (member, value) in &case.scope {
            assert_eq!(
                described.get(*member).and_then(Value::as_str),
                Some(*value),
                "case {}: expected the describe to report the stored {member}, got {described:?}",
                case.label
            );
        }
        for absent in [
            "AvailabilityZone",
            "AvailabilityZoneId",
            "VpcEndpointId",
            "VpcEndpointAssociationArn",
        ] {
            if case.scope.iter().all(|(member, _)| *member != absent) {
                assert_eq!(
                    described.get(absent),
                    None,
                    "case {}: {absent} was never given, so nothing may report it: got {described:?}",
                    case.label
                );
            }
        }
    }
}

/// RULE: the start command is asynchronous — the operation it begins moves from
/// `IN_PROGRESS` to `COMPLETED` once its span has elapsed, whether or not anybody
/// polls it, and a read never moves it back.
#[tokio::test]
async fn a_flow_operation_completes_once_its_span_elapses() {
    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-settles").await;

    let started = harness
        .ok(
            "StartFlowFlush",
            &json!({
                "FirewallArn": arn,
                "FlowFilters": [{ "SourceAddress": { "AddressDefinition": "10.0.0.0/8" } }],
            }),
        )
        .await;
    let operation_id = string_at(&started, &["FlowOperationId"]).to_string();
    let describe = json!({ "FirewallArn": arn, "FlowOperationId": operation_id });

    let immediate = harness.ok("DescribeFlowOperation", &describe).await;
    assert_eq!(
        immediate.get("FlowOperationStatus"),
        Some(&json!(state::FLOW_OPERATION_IN_PROGRESS)),
        "the flow operation span is never zero, got {immediate:?}"
    );

    tokio::time::sleep(state::FLOW_OPERATION_SPAN + Duration::from_millis(200)).await;

    let settled = harness.ok("DescribeFlowOperation", &describe).await;
    assert_eq!(
        settled.get("FlowOperationStatus"),
        Some(&json!(state::FLOW_OPERATION_COMPLETED)),
        "expected the operation to be COMPLETED once the span elapsed, got {settled:?}"
    );
    let again = harness.ok("DescribeFlowOperation", &describe).await;
    assert_eq!(
        again, settled,
        "a read must not change what a later reader sees, got {again:?} after {settled:?}"
    );
    // The timestamp is the instant the operation was requested, so it does not
    // move as the operation progresses.
    assert_eq!(
        settled.get("FlowRequestTimestamp"),
        immediate.get("FlowRequestTimestamp"),
        "expected FlowRequestTimestamp to stay the instant of the start command, \
         got {settled:?} after {immediate:?}"
    );
}

/// RULE: every start command mints its OWN flow operation: the ids are distinct
/// and each id reads back the filters its own call defined, so two operations on
/// one firewall never alias.
///
/// The rows differ in filter count and in the scope each call names, which is
/// what a shared record would collapse.
#[tokio::test]
async fn each_start_mints_its_own_flow_operation() {
    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-distinct").await;

    let rows = [
        distinct_filters(1),
        distinct_filters(2),
        distinct_filters(3),
        json!([{ "DestinationPort": "ANY", "Protocols": ["6"] }])
            .as_array()
            .expect("filter array")
            .clone(),
    ];

    let mut ids = HashSet::new();
    let mut started = Vec::new();
    for filters in &rows {
        let response = harness
            .ok(
                "StartFlowFlush",
                &json!({ "FirewallArn": arn, "FlowFilters": filters }),
            )
            .await;
        let id = string_at(&response, &["FlowOperationId"]).to_string();
        assert!(
            ids.insert(id.clone()),
            "case {filters:?}: expected a fresh FlowOperationId, but {id:?} was minted twice"
        );
        started.push((id, filters.clone()));
    }

    for (id, filters) in &started {
        let described = harness
            .ok(
                "DescribeFlowOperation",
                &json!({ "FirewallArn": arn, "FlowOperationId": id }),
            )
            .await;
        assert_eq!(
            described
                .get("FlowOperation")
                .and_then(|operation| operation.get("FlowFilters")),
            Some(&json!(filters)),
            "case {id}: expected each operation to keep its own filters, got {described:?}"
        );
    }
}

/// RULE: a flow operation belongs to the firewall it was started on, so it is
/// located only by that `FirewallArn` + `FlowOperationId` pair, and it is dropped
/// when the firewall is.
#[tokio::test]
async fn a_flow_operation_is_located_only_on_its_own_firewall() {
    let harness = Harness::new();
    let first = firewall_arn(&harness, "flush-owner").await;
    let second = firewall_arn(&harness, "flush-other").await;

    let started = harness
        .ok(
            "StartFlowFlush",
            &json!({
                "FirewallArn": first,
                "FlowFilters": [{ "SourceAddress": { "AddressDefinition": "203.0.113.0/24" } }],
            }),
        )
        .await;
    let operation_id = string_at(&started, &["FlowOperationId"]).to_string();

    // The same id on the OTHER firewall locates nothing.
    let (status, body) = harness
        .call(
            "DescribeFlowOperation",
            &json!({ "FirewallArn": second, "FlowOperationId": operation_id }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "expected the pair to identify the operation, got {status} {body:?}"
    );

    // Deleting the firewall drops its flow operations with it.
    harness
        .ok("DeleteFirewall", &json!({ "FirewallArn": first }))
        .await;
    let (status, body) = harness
        .call(
            "DescribeFlowOperation",
            &json!({ "FirewallArn": first, "FlowOperationId": operation_id }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "expected a deleted firewall's flow operations to be gone, got {status} {body:?}"
    );
}

/// RULE: a flow operation request is refused where the documented rule it breaks
/// says so — the two Availability Zone members name one scope two ways so naming
/// both fails, `FlowFilters` is capped at twenty entries, and an identifier that
/// locates no stored resource is a not-found — and a refusal never answers 501 or
/// 200.
///
/// The rows span both operations, both refusal classes, each identifier that can
/// miss, and the filter ceiling at the published bound (accepted, above) and one
/// past it.
#[tokio::test]
async fn a_flow_operation_request_is_refused_by_its_documented_rule() {
    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-refusals").await;
    // A firewall ARN in this emulator's form, for a firewall never created.
    let absent_arn = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/never-created");
    let one_filter = json!([{ "SourceAddress": { "AddressDefinition": "10.0.0.1/32" } }]);

    let started = harness
        .ok(
            "StartFlowFlush",
            &json!({ "FirewallArn": arn, "FlowFilters": one_filter }),
        )
        .await;
    let operation_id = string_at(&started, &["FlowOperationId"]).to_string();

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "StartFlowFlush",
            json!({
                "FirewallArn": arn,
                "FlowFilters": one_filter,
                "AvailabilityZone": "eu-west-1a",
                "AvailabilityZoneId": "euw1-az1",
            }),
            "InvalidRequestException",
        ),
        (
            "DescribeFlowOperation",
            json!({
                "FirewallArn": arn,
                "FlowOperationId": operation_id,
                "AvailabilityZone": "eu-west-1a",
                "AvailabilityZoneId": "euw1-az1",
            }),
            "InvalidRequestException",
        ),
        (
            "StartFlowFlush",
            json!({
                "FirewallArn": arn,
                "FlowFilters": distinct_filters(state::MAX_FLOW_FILTERS + 1),
            }),
            "InvalidRequestException",
        ),
        (
            "StartFlowFlush",
            json!({ "FirewallArn": absent_arn, "FlowFilters": one_filter }),
            "ResourceNotFoundException",
        ),
        (
            "DescribeFlowOperation",
            json!({ "FirewallArn": absent_arn, "FlowOperationId": operation_id }),
            "ResourceNotFoundException",
        ),
        (
            // A well-formed id nothing ever minted.
            "DescribeFlowOperation",
            json!({ "FirewallArn": arn, "FlowOperationId": state::new_uuid() }),
            "ResourceNotFoundException",
        ),
    ];

    for (target, request, expected) in &rows {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {target} {request:?}: expected 400 {expected}, got {status} {body:?}"
        );
        if *expected == "ResourceNotFoundException" {
            let message = error_message(&body);
            assert_eq!(
                message, NOT_FOUND,
                "case {target} {request:?}: expected the service's constant not-found message, \
                 got {message:?}"
            );
        }
    }

    // The published bound itself is ACCEPTED: twenty filters is "up to 20".
    let at_bound = harness
        .ok(
            "StartFlowFlush",
            &json!({
                "FirewallArn": arn,
                "FlowFilters": distinct_filters(state::MAX_FLOW_FILTERS),
            }),
        )
        .await;
    assert!(
        at_bound.get("FlowOperationId").is_some(),
        "case at the bound: expected {} filters to be accepted, got {at_bound:?}",
        state::MAX_FLOW_FILTERS
    );

    // A listing of the same firewall breaks the same zone rule.
    let (status, body) = harness
        .call(
            "ListFlowOperations",
            &json!({
                "FirewallArn": arn,
                "AvailabilityZone": "eu-west-1a",
                "AvailabilityZoneId": "euw1-az1",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case ListFlowOperations with both zone members: expected 400 InvalidRequestException, \
         got {status} {body:?}"
    );
    let (status, body) = harness
        .call(
            "ListFlowOperations",
            &json!({ "FirewallArn": absent_arn }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case ListFlowOperations on an absent firewall: expected 400 \
         ResourceNotFoundException, got {status} {body:?}"
    );
}

/// The `FlowOperationId`s a `ListFlowOperations` body reports, in order.
fn listed_ids(body: &Value) -> Vec<String> {
    body.get("FlowOperations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected FlowOperations in {body:?}"))
        .iter()
        .map(|entry| string_at(entry, &["FlowOperationId"]).to_string())
        .collect()
}

/// RULE: the listing reports every flow operation stored on the firewall it
/// names, in the order they were started, narrowed to the type and scope the
/// request asks for — where an operation started without a scope covers the
/// whole firewall and so is never narrowed out — and each entry carries that
/// operation's current status.
///
/// The rows span the narrowing members: no filter at all, each
/// `FlowOperationType` member (one of which nothing has started), the zone by
/// name and by id, and an endpoint id, including values nothing was started with.
#[tokio::test]
async fn the_listing_reports_the_firewall_s_flow_operations_narrowed_as_asked() {
    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-listing").await;

    // Each row is (label, the scope the start command names).
    let starts: Vec<(&str, Value)> = vec![
        ("unscoped", json!({})),
        ("zone-a", json!({ "AvailabilityZone": "eu-west-1a" })),
        ("zone-b", json!({ "AvailabilityZone": "eu-west-1b" })),
        ("zone-id", json!({ "AvailabilityZoneId": "euw1-az1" })),
        (
            "endpoint",
            json!({ "VpcEndpointId": "vpce-0123456789abcdef0" }),
        ),
    ];

    let mut ids = Vec::new();
    for (label, scope) in &starts {
        let mut request = json!({ "FirewallArn": arn, "FlowFilters": distinct_filters(1) });
        for (member, value) in scope.as_object().expect("scope object") {
            request[member.as_str()] = value.clone();
        }
        let started = harness.ok("StartFlowFlush", &request).await;
        ids.push((*label, string_at(&started, &["FlowOperationId"]).to_string()));
    }

    // (label, request narrowing, the labels expected back)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no narrowing",
            json!({}),
            vec!["unscoped", "zone-a", "zone-b", "zone-id", "endpoint"],
        ),
        (
            "type FLOW_FLUSH",
            json!({ "FlowOperationType": "FLOW_FLUSH" }),
            vec!["unscoped", "zone-a", "zone-b", "zone-id", "endpoint"],
        ),
        (
            "type FLOW_CAPTURE",
            json!({ "FlowOperationType": "FLOW_CAPTURE" }),
            vec![],
        ),
        (
            "zone eu-west-1a",
            json!({ "AvailabilityZone": "eu-west-1a" }),
            vec!["unscoped", "zone-a", "zone-id", "endpoint"],
        ),
        (
            "zone eu-west-1c, which nothing was started in",
            json!({ "AvailabilityZone": "eu-west-1c" }),
            vec!["unscoped", "zone-id", "endpoint"],
        ),
        (
            "zone id euw1-az1",
            json!({ "AvailabilityZoneId": "euw1-az1" }),
            vec!["unscoped", "zone-a", "zone-b", "zone-id", "endpoint"],
        ),
        (
            "endpoint vpce-0123456789abcdef0",
            json!({ "VpcEndpointId": "vpce-0123456789abcdef0" }),
            vec!["unscoped", "zone-a", "zone-b", "zone-id", "endpoint"],
        ),
        (
            "endpoint vpce-ffff, which nothing was started with",
            json!({ "VpcEndpointId": "vpce-ffff" }),
            vec!["unscoped", "zone-a", "zone-b", "zone-id"],
        ),
    ];

    for (label, narrowing, expected_labels) in &rows {
        let mut request = json!({ "FirewallArn": arn });
        for (member, value) in narrowing.as_object().expect("narrowing object") {
            request[member.as_str()] = value.clone();
        }
        let listed = harness.ok("ListFlowOperations", &request).await;
        let expected: Vec<String> = ids
            .iter()
            .filter(|(row_label, _)| expected_labels.contains(row_label))
            .map(|(_, id)| id.clone())
            .collect();
        assert_eq!(
            listed_ids(&listed),
            expected,
            "case {label}: expected exactly {expected_labels:?} in start order, got {listed:?}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {label}: one page holds them all, so no cursor is owed: got {listed:?}"
        );
        for entry in listed
            .get("FlowOperations")
            .and_then(Value::as_array)
            .expect("FlowOperations")
        {
            assert_eq!(
                entry.get("FlowOperationType"),
                Some(&json!(state::FLOW_OPERATION_FLUSH)),
                "case {label}: every started operation is a flush, got {entry:?}"
            );
            // This table spans many calls, so an operation's span may well have
            // elapsed by now: what the rule fixes here is that the status is one
            // the model declares, with the clock-owed move asserted on its own
            // below and the transient value asserted on a read taken right after
            // a single start in the round-trip test above.
            let status = entry.get("FlowOperationStatus").cloned();
            assert!(
                status == Some(json!(state::FLOW_OPERATION_IN_PROGRESS))
                    || status == Some(json!(state::FLOW_OPERATION_COMPLETED)),
                "case {label}: expected a started flush to be IN_PROGRESS or COMPLETED, \
                 got {entry:?}"
            );
            assert!(
                entry.get("FlowRequestTimestamp").is_some(),
                "case {label}: expected each entry to carry its request timestamp, got {entry:?}"
            );
        }
    }

    // The listing materialises the same clock-owed move the describe does.
    tokio::time::sleep(state::FLOW_OPERATION_SPAN + Duration::from_millis(200)).await;
    let settled = harness
        .ok("ListFlowOperations", &json!({ "FirewallArn": arn }))
        .await;
    for entry in settled
        .get("FlowOperations")
        .and_then(Value::as_array)
        .expect("FlowOperations")
    {
        assert_eq!(
            entry.get("FlowOperationStatus"),
            Some(&json!(state::FLOW_OPERATION_COMPLETED)),
            "expected every operation to be COMPLETED once the span elapsed, got {entry:?}"
        );
    }
}

/// RULE: the listing is paged — `MaxResults` caps a page, the `NextToken` it
/// returns resumes after the last entry of that page, and a token this listing
/// did not issue is refused — so the pages concatenate to the full listing with
/// nothing repeated and nothing dropped.
///
/// The rows span the page sizes that divide the listing evenly, that leave a
/// remainder, that take it one at a time, and that exceed it.
#[tokio::test]
async fn the_listing_pages_through_every_flow_operation_once() {
    let harness = Harness::new();
    let arn = firewall_arn(&harness, "flush-paging").await;

    let mut started = Vec::new();
    for count in 1..=5 {
        let response = harness
            .ok(
                "StartFlowFlush",
                &json!({ "FirewallArn": arn, "FlowFilters": distinct_filters(count) }),
            )
            .await;
        started.push(string_at(&response, &["FlowOperationId"]).to_string());
    }

    for max_results in [1, 2, 3, 5, 7] {
        let mut seen = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "FirewallArn": arn, "MaxResults": max_results });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let listed = harness.ok("ListFlowOperations", &request).await;
            let page = listed_ids(&listed);
            assert!(
                page.len() <= max_results,
                "case MaxResults={max_results}: expected at most {max_results} entries, \
                 got {listed:?}"
            );
            seen.extend(page);
            pages += 1;
            next_token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= started.len(),
                "case MaxResults={max_results}: expected the paging to terminate, \
                 but it ran for {pages} pages"
            );
        }
        assert_eq!(
            seen, started,
            "case MaxResults={max_results}: expected the pages to concatenate to the full \
             listing in start order, got {seen:?}"
        );
    }

    let (status, body) = harness
        .call(
            "ListFlowOperations",
            &json!({ "FirewallArn": arn, "NextToken": "bm90LWEtdG9rZW4=" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case a foreign NextToken: expected 400 InvalidRequestException, got {status} {body:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! AddFlowVpcInterfaces, end to end over HTTP.
//!
//! The rule under test: adding VPC interfaces to a flow materialises EVERY requested
//! `VpcInterfaceRequest` into the flow's stored record — keeping each member the request
//! names, defaulting `NetworkInterfaceType` to the `ena` adapter the service applies when
//! the request names none, and creating one elastic network interface per VPC interface
//! (`VpcInterface$NetworkInterfaceIds`) — appends them after the ones the flow already
//! holds, and reports those stored records, which is what DescribeFlow then reports too. A
//! request the flow cannot accept (an unknown flow, no VPC interface at all, a name the
//! flow already holds or a name repeated within the request, a flow already at its bound)
//! is refused with the modelled error and stores nothing.
//!
//! The same uniqueness rule is asserted on CreateFlow's `VpcInterfaces`, which adds VPC
//! interfaces to a flow the same way, so neither operation can leave a flow in a state the
//! other would never permit.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_outputs::unknown_flow_arn;
// The two readers this suite needs over a flow's VPC interfaces already exist beside
// RemoveFlowVpcInterface's cases, so both suites read a flow back through the same ones.
use crate::remove_flow_vpc_interface::{
    described_flow_without_vpc_interfaces, described_vpc_interfaces,
};

/// The ceiling on how many VPC interfaces one flow holds.
const LIMIT: usize = mc::MAX_FLOW_VPC_INTERFACES;

/// A role the VPC interface requests below name. `VpcInterfaceRequest$RoleArn` is "the ARN
/// of the role that you created when you set up MediaConnect as a trusted service".
///
/// `pub(crate)` because `tests/all/remove_flow_vpc_interface.rs` builds its VPC interface
/// requests through this suite's builder.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/MediaConnect";

/// One VPC interface request, in the shape `VpcInterfaceRequest` takes
/// (`smithy_query(query="shape",shape="VpcInterfaceRequest")`: `Name`, `RoleArn`,
/// `SecurityGroupIds` and `SubnetId` required; `NetworkInterfaceType` and
/// `VpcInterfaceTags` optional).
///
/// `pub(crate)` because this is the crate's ONE builder of the shape:
/// `tests/all/remove_flow_vpc_interface.rs` fills its flows through it as well, fixing the
/// members its own cases do not vary.
pub(crate) fn vpc_interface_request(
    name: &str,
    network_interface_type: Option<&str>,
    security_group_ids: &[&str],
    subnet_id: &str,
    vpc_interface_tags: Option<Value>,
) -> Value {
    let mut request = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "securityGroupIds": security_group_ids,
        "subnetId": subnet_id,
    });
    if let Some(network_interface_type) = network_interface_type {
        request["networkInterfaceType"] = json!(network_interface_type);
    }
    if let Some(tags) = vpc_interface_tags {
        request["vpcInterfaceTags"] = tags;
    }
    request
}

/// The plainest VPC interface request: named `name`, every optional member omitted.
fn plain_request(name: &str) -> Value {
    vpc_interface_request(name, None, &["sg-0123456789abcdef0"], SUBNET_ID, None)
}

/// The subnet most rows name. `VpcInterfaceRequest$SubnetId` is "the subnet IDs that you
/// want to use for your VPC interface".
///
/// `pub(crate)` for the same reason as [`vpc_interface_request`].
pub(crate) const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// `count` plain VPC interface requests named `prefix-<n>` from 0, so a row can build or
/// fill a flow with a known number of distinguishable VPC interfaces.
fn numbered_requests(prefix: &str, count: usize) -> Value {
    (0..count)
        .map(|index| plain_request(&format!("{prefix}-{index}")))
        .collect()
}

/// `POST /v1/flows/{FlowArn}/vpcInterfaces` — the AddFlowVpcInterfaces binding
/// (`smithy_query(query="operation",operation="AddFlowVpcInterfaces")`).
///
/// `pub(crate)` because `tests/all/flow_name_references.rs` gives a flow the VPC interface
/// one of its references names through the same binding.
pub(crate) async fn add_vpc_interfaces(
    emulator: &Emulator,
    flow_arn: &str,
    vpc_interfaces: Value,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "POST",
            &format!("/v1/flows/{flow_arn}/vpcInterfaces"),
            Some(json!({ "vpcInterfaces": vpc_interfaces })),
        )
        .await
}

/// Create a flow named `name` carrying `vpc_interfaces`, and return `(status, error code,
/// body)` as the wire gives it, so a row can assert a refused create as well as a good one.
async fn create_flow(
    emulator: &Emulator,
    name: &str,
    vpc_interfaces: Value,
) -> (u16, Option<String>, Value) {
    let mut body = json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
    });
    if !vpc_interfaces.is_null() {
        body["vpcInterfaces"] = vpc_interfaces;
    }
    emulator.create_flow(body).await
}

/// Create a flow named `name` carrying `vpc_interfaces` and return its ARN.
async fn created_flow_arn(
    emulator: &Emulator,
    case: &str,
    name: &str,
    vpc_interfaces: Value,
) -> String {
    let (status, code, created) = create_flow(emulator, name, vpc_interfaces).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: the flow under test must be created, got {status} {code:?}: {created}"
    );
    created["flow"]["flowArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert `id` is an elastic network interface id the service minted: "IDs of the network
/// interfaces created in customer's account by MediaConnect"
/// (`smithy_query(query="shape",shape="VpcInterface")`), which AWS writes as `eni-`
/// followed by 17 lowercase hexadecimal digits.
fn assert_eni_id(case: &str, id: &str) {
    let hex = id.strip_prefix("eni-").unwrap_or_default();
    assert!(
        hex.len() == 17
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: a network interface id is \"eni-\" and 17 lowercase hex digits, got \
         {id:?}"
    );
}

/// The VPC interface the service must report for one `VpcInterfaceRequest`, given the
/// network interface ids it created for it: every member the request named, and the
/// documented adapter fallback. `VpcInterfaceTags` is deliberately absent — the reported
/// `VpcInterface` shape models no tag member.
fn expected_interface(request: &Value, network_interface_ids: Value) -> Value {
    json!({
        "name": request["name"].clone(),
        "networkInterfaceIds": network_interface_ids,
        "networkInterfaceType": request
            .get("networkInterfaceType")
            .cloned()
            .unwrap_or_else(|| json!(mc::NETWORK_INTERFACE_TYPE_ENA)),
        "roleArn": request["roleArn"].clone(),
        "securityGroupIds": request["securityGroupIds"].clone(),
        "subnetId": request["subnetId"].clone(),
    })
}

/// Add `requested` to `flow_arn` and assert the whole rule for that one call: the 201
/// answer names the flow and carries exactly the materialised records of this batch, the
/// flow then holds what it held plus that batch IN ORDER, each new VPC interface carries
/// one freshly minted network interface id unique within the flow, and nothing else about
/// the flow moved.
async fn add_and_assert(emulator: &Emulator, case: &str, flow_arn: &str, requested: &Value) {
    let before = described_vpc_interfaces(emulator, case, flow_arn).await;
    let rest_before = described_flow_without_vpc_interfaces(emulator, flow_arn).await;
    let requests = requested.as_array().cloned().unwrap_or_default();

    let (status, code, body) = add_vpc_interfaces(emulator, flow_arn, requested.clone()).await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "case {case:?}: adding {} VPC interfaces expected 201 with no error, got {status} \
         {code:?}: {body}",
        requests.len()
    );
    assert_eq!(
        body["flowArn"].as_str(),
        Some(flow_arn),
        "case {case:?}: the response names the flow the interfaces were added to, got {body}"
    );
    let added = body["vpcInterfaces"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        added.len(),
        requests.len(),
        "case {case:?}: the response carries the newly added VPC interfaces, got {body}"
    );

    for (request, reported) in requests.iter().zip(&added) {
        let ids = reported["networkInterfaceIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: the service creates one network interface per VPC interface, got \
             {reported}"
        );
        assert_eq!(
            reported,
            &expected_interface(request, json!(ids)),
            "case {case:?}: the stored VPC interface keeps every member the request named, \
             expected {}, got {reported}",
            expected_interface(request, json!(ids))
        );
        assert_eq!(
            reported.get("vpcInterfaceTags"),
            None,
            "case {case:?}: the reported VpcInterface shape models no tag member, got \
             {reported}"
        );
        assert_eni_id(case, ids[0].as_str().unwrap_or_default());
    }

    let after = described_vpc_interfaces(emulator, case, flow_arn).await;
    let expected: Vec<Value> = before.iter().chain(added.iter()).cloned().collect();
    assert_eq!(
        after, expected,
        "case {case:?}: the flow holds the VPC interfaces it held plus the added ones, in \
         order, expected {expected:?}, got {after:?}"
    );

    let mut eni_ids: Vec<String> = after
        .iter()
        .flat_map(|vpc_interface| {
            vpc_interface["networkInterfaceIds"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    let total = eni_ids.len();
    eni_ids.sort();
    eni_ids.dedup();
    assert_eq!(
        eni_ids.len(),
        total,
        "case {case:?}: no two network interfaces of one flow share an id, got {after:?}"
    );

    let rest_after = described_flow_without_vpc_interfaces(emulator, flow_arn).await;
    assert_eq!(
        rest_after, rest_before,
        "case {case:?}: adding VPC interfaces moves nothing else about the flow, expected \
         {rest_before}, got {rest_after}"
    );
}

/// One row of the add table: the VPC interfaces the flow is created with, and the batches
/// added to it afterwards, one call per batch.
struct AddCase {
    case: &'static str,
    flow: &'static str,
    created_with: Value,
    batches: Vec<Value>,
}

/// The rows span the class the rule generalises over: a flow created with and without VPC
/// interfaces, batches of one, two and three, each `NetworkInterfaceType` the model declares
/// and the type omitted, one and several security groups, a subnet per interface, the
/// optional tag map present, names differing only in case, and two adds in sequence on one
/// flow.
fn add_cases() -> Vec<AddCase> {
    vec![
        AddCase {
            case: "one interface, every optional member omitted, onto a flow with none",
            flow: "vpc-add-plain",
            created_with: Value::Null,
            batches: vec![json!([plain_request("vpc-a")])],
        },
        AddCase {
            // `NetworkInterfaceType` is `ena | efa`
            // (`smithy_query(query="shape",shape="NetworkInterfaceType")`), so a row names
            // each value and omits it.
            case: "three interfaces: the type omitted, ena named, efa named",
            flow: "vpc-add-types",
            created_with: Value::Null,
            batches: vec![json!([
                plain_request("vpc-default"),
                vpc_interface_request(
                    "vpc-ena",
                    Some("ena"),
                    &["sg-0123456789abcdef0"],
                    SUBNET_ID,
                    None
                ),
                vpc_interface_request(
                    "vpc-efa",
                    Some("efa"),
                    &["sg-00000000000000001", "sg-00000000000000002"],
                    "subnet-00000000000000002",
                    None
                ),
            ])],
        },
        AddCase {
            case: "onto a flow created with two interfaces, in two further calls",
            flow: "vpc-add-onto-held",
            created_with: numbered_requests("held", 2),
            batches: vec![
                json!([plain_request("added-0")]),
                json!([plain_request("added-1"), plain_request("added-2")]),
            ],
        },
        AddCase {
            case: "an interface carrying VPC interface tags, which the flow does not report",
            flow: "vpc-add-tagged",
            created_with: Value::Null,
            batches: vec![json!([vpc_interface_request(
                "vpc-tagged",
                None,
                &["sg-0123456789abcdef0", "sg-00000000000000003"],
                "subnet-00000000000000003",
                Some(json!({ "team": "video", "env": "test" }))
            )])],
        },
        AddCase {
            // "Immutable and has to be a unique against other VpcInterfaces in this Flow"
            // (`VpcInterface$Name`): uniqueness is of the name as written, so two names
            // differing in case are two VPC interfaces.
            case: "names differing only in case, and names with digits and underscores",
            flow: "vpc-add-names",
            created_with: json!([plain_request("vpc-case")]),
            batches: vec![json!([
                plain_request("VPC-CASE"),
                plain_request("vpc_iface-42"),
            ])],
        },
    ]
}

/// An add materialises every requested VPC interface into the flow it addresses, whatever
/// form the request has and whatever the flow already holds.
#[tokio::test]
async fn add_flow_vpc_interfaces_materialises_every_requested_interface() {
    let emulator = Emulator::new();

    for row in add_cases() {
        let AddCase {
            case,
            flow,
            created_with,
            batches,
        } = row;
        let flow_arn = created_flow_arn(&emulator, case, flow, created_with).await;
        for batch in &batches {
            add_and_assert(&emulator, case, &flow_arn, batch).await;
        }
    }
}

/// One row of the refusal table: a request the service must refuse, the flow it addresses,
/// and the modelled error it must be refused with.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    requested: Value,
    status: u16,
    code: &'static str,
}

/// A request the flow cannot accept adds nothing: it is refused with the modelled error and
/// the flow still holds exactly the VPC interfaces it held.
#[tokio::test]
async fn add_flow_vpc_interfaces_refuses_what_it_cannot_add() {
    let case = "refusals";
    let emulator = Emulator::new();

    let flow_arn = created_flow_arn(
        &emulator,
        case,
        "vpc-add-refusals",
        json!([plain_request("vpc-held")]),
    )
    .await;
    let before = described_vpc_interfaces(&emulator, case, &flow_arn).await;

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted",
            flow_arn: unknown_flow_arn(),
            requested: json!([plain_request("vpc-new")]),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            // A label that is not an ARN at all names no stored flow either, so it resolves
            // to no flow and is answered as a not-found.
            case: "a flow label that is no flow ARN at all",
            flow_arn: "not-a-flow-arn".to_string(),
            requested: json!([plain_request("vpc-new")]),
            status: 404,
            code: "NotFoundException",
        },
        RefusalCase {
            case: "no VPC interface at all",
            flow_arn: flow_arn.clone(),
            requested: json!([]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a name the flow already holds",
            flow_arn: flow_arn.clone(),
            requested: json!([plain_request("vpc-held")]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "a name repeated within the request",
            flow_arn: flow_arn.clone(),
            requested: json!([plain_request("vpc-twice"), plain_request("vpc-twice")]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            case: "an empty name, which addresses no VPC interface",
            flow_arn: flow_arn.clone(),
            requested: json!([plain_request("")]),
            status: 400,
            code: "BadRequestException",
        },
        RefusalCase {
            // The whole request is checked before anything is stored, so one bad entry
            // beside good ones adds none of them.
            case: "one repeated name beside two acceptable interfaces",
            flow_arn: flow_arn.clone(),
            requested: json!([
                plain_request("vpc-good-0"),
                plain_request("vpc-held"),
                plain_request("vpc-good-1"),
            ]),
            status: 400,
            code: "BadRequestException",
        },
    ];

    for row in rows {
        let RefusalCase {
            case,
            flow_arn: target,
            requested,
            status: expected_status,
            code: expected_code,
        } = row;
        let (status, code, body) = add_vpc_interfaces(&emulator, &target, requested).await;
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, Some(expected_code)),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code:?}: \
             {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        if expected_status == 404 {
            assert_eq!(
                message,
                mc::flow_not_found_message(&target),
                "case {case:?}: the refusal names what resolved nothing: the flow ARN, or \
                 the generic reason for a label that is no flow ARN, got {body}"
            );
        } else {
            assert!(
                message.starts_with(mc::VALIDATION_FAILED_PREFIX),
                "case {case:?}: the refusal keeps this service's validation envelope, got \
                 {body}"
            );
        }

        let after = described_vpc_interfaces(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused add stores nothing, got {after:?}"
        );
    }
}

/// How many VPC interfaces one FLOW holds is bounded, so what it already holds counts
/// towards the bound: the call that fills the last slots succeeds and the next one is
/// refused, storing nothing.
#[tokio::test]
async fn add_flow_vpc_interfaces_bounds_what_one_flow_holds() {
    let case = "fill a flow to its bound";
    let emulator = Emulator::new();

    let flow_arn = created_flow_arn(
        &emulator,
        case,
        "vpc-add-bound",
        numbered_requests("held", LIMIT - 2),
    )
    .await;

    // The call that would take the flow past the bound is refused before the one that
    // fills it, so the refusal is about the count and not about what the flow holds.
    let (status, code, body) = add_vpc_interfaces(
        &emulator,
        &flow_arn,
        numbered_requests("overflow", 3),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: a request for one more than the free slots expected 400 \
         BadRequestException, got {status} {code:?}: {body}"
    );

    add_and_assert(
        &emulator,
        case,
        &flow_arn,
        &numbered_requests("last", 2),
    )
    .await;
    assert_eq!(
        described_vpc_interfaces(&emulator, case, &flow_arn)
            .await
            .len(),
        LIMIT,
        "case {case:?}: the flow is at its bound of {LIMIT} VPC interfaces"
    );

    let (status, code, body) =
        add_vpc_interfaces(&emulator, &flow_arn, json!([plain_request("one-too-many")])).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: one more interface than the bound expected 400 BadRequestException, \
         got {status} {code:?}: {body}"
    );
    assert_eq!(
        described_vpc_interfaces(&emulator, case, &flow_arn)
            .await
            .len(),
        LIMIT,
        "case {case:?}: the refused add stores nothing"
    );
}

/// A VPC interface this operation added is the same kind of record CreateFlow stores: the
/// flow's own operations address it by name, so RemoveFlowVpcInterface drops it and the
/// name is then free to add again.
#[tokio::test]
async fn add_flow_vpc_interfaces_stores_a_removable_interface() {
    let case = "add, remove by name, add the same name again";
    let emulator = Emulator::new();

    let flow_arn = created_flow_arn(&emulator, case, "vpc-add-removable", Value::Null).await;
    add_and_assert(&emulator, case, &flow_arn, &json!([plain_request("vpc-x")])).await;

    let (status, code, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/vpcInterfaces/vpc-x"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: RemoveFlowVpcInterface must drop the added interface, got {status} \
         {code:?}: {body}"
    );
    assert!(
        described_vpc_interfaces(&emulator, case, &flow_arn)
            .await
            .is_empty(),
        "case {case:?}: the flow holds no VPC interface after the removal"
    );

    // The name is free again, and the interface added under it carries a NEW network
    // interface, not the one that went with the removed record.
    add_and_assert(&emulator, case, &flow_arn, &json!([plain_request("vpc-x")])).await;
}

/// The uniqueness rule belongs to the flow, not to one operation: CreateFlow's
/// `VpcInterfaces` adds VPC interfaces to a flow too, so it refuses a repeated name with
/// the same `BadRequestException` — and accepts the names AddFlowVpcInterfaces accepts.
#[tokio::test]
async fn create_flow_applies_the_same_vpc_interface_name_rule() {
    let case = "CreateFlow with a repeated VPC interface name";
    let emulator = Emulator::new();

    for (row, requested) in [
        (
            "the same name twice",
            json!([plain_request("vpc-dup"), plain_request("vpc-dup")]),
        ),
        ("an empty name", json!([plain_request("")])),
    ] {
        let (status, code, body) = create_flow(&emulator, "vpc-create-dup", requested).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?} / {row:?}: expected 400 BadRequestException, got {status} {code:?}: \
             {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?} / {row:?}: the refusal keeps this service's validation envelope, \
             got {body}"
        );
    }

    // Two names that differ only in case are two VPC interfaces here as well.
    let flow_arn = created_flow_arn(
        &emulator,
        case,
        "vpc-create-case",
        json!([plain_request("vpc-case"), plain_request("VPC-CASE")]),
    )
    .await;
    assert_eq!(
        described_vpc_interfaces(&emulator, case, &flow_arn)
            .await
            .len(),
        2,
        "case {case:?}: a flow created with two names differing only in case holds both"
    );
}

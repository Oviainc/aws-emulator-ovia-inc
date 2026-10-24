// SPDX-License-Identifier: CC0-1.0

//! RemoveFlowVpcInterface, end to end over HTTP: one call drops exactly the VPC interface
//! whose name it addresses from the flow whose ARN it addresses.
//!
//! The rule these cases assert, over a table rather than over one request: a remove
//! answers 200 with the stored flow's ARN, the removed VPC interface's name and the
//! network interfaces it could not delete (none, so an empty list), the flow then holds
//! the VPC interfaces it held minus that one IN THE SAME ORDER and with nothing else about
//! it moved, the name is free again, a request naming a flow the store does not hold or a
//! VPC interface that flow does not hold is refused `NotFoundException` (404), and — the
//! operation's documented precondition — a VPC interface any of the flow's sources or
//! outputs references is refused `BadRequestException` (400) until that source or output
//! is deleted or updated to no longer reference it. Every refusal removes nothing.
//!
//! Every case drives the real request path (routing, restJson1 deserialization,
//! constraint validation, handler, serialization) through the harness
//! [`crate::flow_lifecycle::Emulator`], so what they assert is what a MediaConnect client
//! sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::flow_outputs::unknown_flow_arn;

/// A role the VPC interface requests below name. `VpcInterfaceRequest$RoleArn` is "the
/// ARN of the role that you created when you set up MediaConnect as a trusted service".
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/MediaConnect";

/// `DELETE /v1/flows/{FlowArn}/vpcInterfaces/{VpcInterfaceName}` — the
/// RemoveFlowVpcInterface binding
/// (`smithy_query(query="operation",operation="RemoveFlowVpcInterface")`).
async fn remove_vpc_interface(
    emulator: &Emulator,
    flow_arn: &str,
    vpc_interface_name: &str,
) -> (u16, Option<String>, Value) {
    emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/vpcInterfaces/{vpc_interface_name}"),
            None,
        )
        .await
}

/// One VPC interface request, in the shape `CreateFlowRequest$VpcInterfaces` takes
/// (`smithy_query(query="shape",shape="VpcInterfaceRequest")`: `Name`, `RoleArn`,
/// `SecurityGroupIds` and `SubnetId` required, `NetworkInterfaceType` optional).
fn vpc_interface_request(
    name: &str,
    network_interface_type: Option<&str>,
    security_group_ids: &[&str],
) -> Value {
    let mut request = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "securityGroupIds": security_group_ids,
        "subnetId": "subnet-0123456789abcdef0"
    });
    if let Some(network_interface_type) = network_interface_type {
        request["networkInterfaceType"] = json!(network_interface_type);
    }
    request
}

/// `count` VPC interface requests named `vpc-<n>` from 0, each with the default network
/// interface type, so a row can build a flow with a known number of distinguishable VPC
/// interfaces.
fn numbered_interfaces(count: usize) -> Value {
    (0..count)
        .map(|index| {
            vpc_interface_request(&format!("vpc-{index}"), None, &["sg-0123456789abcdef0"])
        })
        .collect()
}

/// Create a flow carrying `vpc_interfaces`, with `extra` merged into the request body (the
/// sources and outputs a reference case needs), and return its ARN.
async fn create_flow_with_vpc_interfaces(
    emulator: &Emulator,
    case: &str,
    name: &str,
    vpc_interfaces: Value,
    extra: Value,
) -> String {
    let mut body = json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" },
        "vpcInterfaces": vpc_interfaces,
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let (status, code, created) = emulator.create_flow(body).await;
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

/// The flow's stored VPC interfaces, as DescribeFlow reports them.
async fn described_vpc_interfaces(emulator: &Emulator, case: &str, flow_arn: &str) -> Vec<Value> {
    let (status, code, described) = emulator.describe_flow(flow_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: DescribeFlow must answer 200, got {status} {code:?}: {described}"
    );
    described["flow"]["vpcInterfaces"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The flow as DescribeFlow reports it, with its VPC interfaces removed — what a removal
/// must leave untouched.
async fn described_flow_without_vpc_interfaces(emulator: &Emulator, flow_arn: &str) -> Value {
    let (_, _, described) = emulator.describe_flow(flow_arn).await;
    let mut flow = described["flow"].clone();
    if let Some(object) = flow.as_object_mut() {
        object.remove("vpcInterfaces");
    }
    flow
}

/// Remove the VPC interface at `position` of the flow's CURRENT list and assert the whole
/// rule for that one call: the 200 answer names the flow, that VPC interface and an empty
/// list of network interfaces it could not delete, the flow then holds the other VPC
/// interfaces unchanged and in order, and nothing else about the flow moved.
async fn remove_at(emulator: &Emulator, case: &str, flow_arn: &str, position: usize) {
    let before = described_vpc_interfaces(emulator, case, flow_arn).await;
    let rest_before = described_flow_without_vpc_interfaces(emulator, flow_arn).await;
    let target = before.get(position).cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the flow must hold a VPC interface at {position}")
    });
    let name = target["name"].as_str().unwrap_or_default().to_string();

    let (status, code, body) = remove_vpc_interface(emulator, flow_arn, &name).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: removing VPC interface {position} ({name:?}) expected 200 with no \
         error, got {status} {code:?}: {body}"
    );
    assert_eq!(
        (body["flowArn"].as_str(), body["vpcInterfaceName"].as_str()),
        (Some(flow_arn), Some(name.as_str())),
        "case {case:?}: the response names the flow and the removed VPC interface, got {body}"
    );
    assert_eq!(
        body["nonDeletedNetworkInterfaceIds"].as_array(),
        Some(&Vec::new()),
        "case {case:?}: every network interface of the removed VPC interface goes with it, \
         so none is reported undeleted, got {body}"
    );

    let after = described_vpc_interfaces(emulator, case, flow_arn).await;
    let expected: Vec<Value> = before
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != position)
        .map(|(_, vpc_interface)| vpc_interface.clone())
        .collect();
    assert_eq!(
        after, expected,
        "case {case:?}: the flow holds the other VPC interfaces, in order, expected \
         {expected:?}, got {after:?}"
    );
    let rest_after = described_flow_without_vpc_interfaces(emulator, flow_arn).await;
    assert_eq!(
        rest_after, rest_before,
        "case {case:?}: removing a VPC interface moves nothing else about the flow, expected \
         {rest_before}, got {rest_after}"
    );
}

/// One row of the removal table: the VPC interfaces a flow is created with, and which
/// position the call removes.
struct RemoveCase {
    case: &'static str,
    vpc_interfaces: Value,
    position: usize,
}

fn remove_cases() -> Vec<RemoveCase> {
    vec![
        RemoveCase {
            case: "the only VPC interface of a flow",
            vpc_interfaces: numbered_interfaces(1),
            position: 0,
        },
        RemoveCase {
            case: "the first of three",
            vpc_interfaces: numbered_interfaces(3),
            position: 0,
        },
        RemoveCase {
            case: "the middle of three",
            vpc_interfaces: numbered_interfaces(3),
            position: 1,
        },
        RemoveCase {
            case: "the last of five",
            vpc_interfaces: numbered_interfaces(5),
            position: 4,
        },
        RemoveCase {
            // `NetworkInterfaceType` is `ena | efa`
            // (`smithy_query(query="shape",shape="NetworkInterfaceType")`), and
            // `SecurityGroupIds` is a list, so a row removes an interface of each form.
            case: "an efa interface with three security groups, among interfaces of each form",
            vpc_interfaces: json!([
                vpc_interface_request("vpc-ena-default", None, &["sg-0123456789abcdef0"]),
                vpc_interface_request(
                    "vpc-efa",
                    Some("efa"),
                    &["sg-0123456789abcdef0", "sg-00000000000000001", "sg-00000000000000002"]
                ),
                vpc_interface_request("vpc-ena-explicit", Some("ena"), &["sg-0123456789abcdef0"]),
            ]),
            position: 1,
        },
        RemoveCase {
            case: "a name carrying digits, hyphens and underscores",
            vpc_interfaces: json!([
                vpc_interface_request("vpc-0", None, &["sg-0123456789abcdef0"]),
                vpc_interface_request("VPC_iface-42", None, &["sg-0123456789abcdef0"]),
            ]),
            position: 1,
        },
    ]
}

/// A remove drops exactly the VPC interface it addresses, from exactly the flow it
/// addresses, whatever form that interface has and wherever in the flow's list it sits.
#[tokio::test]
async fn remove_flow_vpc_interface_drops_exactly_the_named_interface() {
    let emulator = Emulator::new();

    for (index, row) in remove_cases().into_iter().enumerate() {
        let RemoveCase {
            case,
            vpc_interfaces,
            position,
        } = row;
        let flow_arn = create_flow_with_vpc_interfaces(
            &emulator,
            case,
            &format!("flow-vpc-remove-{index}"),
            vpc_interfaces,
            json!({}),
        )
        .await;
        remove_at(&emulator, case, &flow_arn, position).await;
    }
}

/// Removing every VPC interface of a flow in turn leaves the flow with none, and
/// DescribeFlow then omits the member rather than reporting an empty list — the same way a
/// flow created without any reports none.
#[tokio::test]
async fn remove_flow_vpc_interface_drains_a_flow_to_none() {
    let case = "drain four interfaces, newest first";
    let emulator = Emulator::new();
    let flow_arn = create_flow_with_vpc_interfaces(
        &emulator,
        case,
        "flow-vpc-drain",
        numbered_interfaces(4),
        json!({}),
    )
    .await;

    for remaining in (1..=4usize).rev() {
        assert_eq!(
            described_vpc_interfaces(&emulator, case, &flow_arn)
                .await
                .len(),
            remaining,
            "case {case:?}: the flow holds {remaining} VPC interfaces before this removal"
        );
        remove_at(&emulator, case, &flow_arn, remaining - 1).await;
    }

    let (_, _, described) = emulator.describe_flow(&flow_arn).await;
    assert!(
        described["flow"]["vpcInterfaces"].is_null(),
        "case {case:?}: a flow with no VPC interfaces left reports no vpcInterfaces member, \
         got {}",
        described["flow"]
    );
    let bare = create_flow_with_vpc_interfaces(
        &emulator,
        case,
        "flow-vpc-none",
        json!([]),
        json!({}),
    )
    .await;
    let (_, _, described_bare) = emulator.describe_flow(&bare).await;
    assert!(
        described_bare["flow"]["vpcInterfaces"].is_null(),
        "case {case:?}: a flow created with no VPC interfaces reports the member the same \
         way, got {}",
        described_bare["flow"]
    );
}

/// One row of the refusal table: a RemoveFlowVpcInterface request the service must refuse,
/// what it addresses, and the identifier the `NotFoundException` envelope must name.
struct RefusalCase {
    case: &'static str,
    flow_arn: String,
    vpc_interface_name: String,
}

/// A RemoveFlowVpcInterface request that resolves no VPC interface removes nothing: it is
/// refused `NotFoundException` (404) naming what resolved nothing, and every flow still
/// holds exactly the VPC interfaces it held.
#[tokio::test]
async fn remove_flow_vpc_interface_refuses_what_it_cannot_remove() {
    let case = "refusals";
    let emulator = Emulator::new();

    let flow_arn = create_flow_with_vpc_interfaces(
        &emulator,
        case,
        "flow-vpc-refusals",
        numbered_interfaces(3),
        json!({}),
    )
    .await;
    let other_flow = create_flow_with_vpc_interfaces(
        &emulator,
        case,
        "flow-vpc-other",
        json!([vpc_interface_request(
            "vpc-only-here",
            None,
            &["sg-0123456789abcdef0"]
        )]),
        json!({}),
    )
    .await;

    // One interface is removed up front, so a row can address a name this flow HELD and
    // no longer holds.
    let (status, _, body) = remove_vpc_interface(&emulator, &flow_arn, "vpc-0").await;
    assert_eq!(
        status, 200,
        "the fixture removal must succeed, got {status}: {body}"
    );

    let before = described_vpc_interfaces(&emulator, case, &flow_arn).await;
    let other_before = described_vpc_interfaces(&emulator, case, &other_flow).await;

    let rows = vec![
        RefusalCase {
            case: "a flow ARN this service never minted, with an interface name it holds",
            flow_arn: unknown_flow_arn(),
            vpc_interface_name: "vpc-1".to_string(),
        },
        RefusalCase {
            case: "a VPC interface name no flow holds",
            flow_arn: flow_arn.clone(),
            vpc_interface_name: "never-added".to_string(),
        },
        RefusalCase {
            case: "a VPC interface of another flow, addressed through this one",
            flow_arn: flow_arn.clone(),
            vpc_interface_name: "vpc-only-here".to_string(),
        },
        RefusalCase {
            case: "a VPC interface already removed",
            flow_arn: flow_arn.clone(),
            vpc_interface_name: "vpc-0".to_string(),
        },
        RefusalCase {
            case: "a name that differs from a held one only in case",
            flow_arn: flow_arn.clone(),
            vpc_interface_name: "VPC-1".to_string(),
        },
        RefusalCase {
            // A label that is not an ARN at all names no stored flow either, so it
            // resolves to no flow and is answered as a not-found — the same answer
            // DescribeFlow and RemoveFlowMediaStream give such a label.
            case: "a flow label that is no flow ARN at all",
            flow_arn: "not-a-flow-arn".to_string(),
            vpc_interface_name: "vpc-1".to_string(),
        },
    ];

    for RefusalCase {
        case,
        flow_arn: target_flow,
        vpc_interface_name,
    } in rows
    {
        let (status, code, body) =
            remove_vpc_interface(&emulator, &target_flow, &vpc_interface_name).await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        let identifier = if target_flow == flow_arn {
            vpc_interface_name.clone()
        } else {
            target_flow.clone()
        };
        assert_eq!(
            body["message"].as_str(),
            Some(mc::not_found_message(&identifier).as_str()),
            "case {case:?}: the refusal names what resolved nothing, got {body}"
        );

        let after = described_vpc_interfaces(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused remove leaves every VPC interface in place, got \
             {after:?}"
        );
        let other_after = described_vpc_interfaces(&emulator, case, &other_flow).await;
        assert_eq!(
            other_after, other_before,
            "case {case:?}: and touches no other flow, got {other_after:?}"
        );
    }
}

/// One row of the reference table: the sources and outputs a flow is created with, and the
/// name of the VPC interface they reference.
struct ReferenceCase {
    case: &'static str,
    extra: Value,
    referenced: &'static str,
    free: &'static str,
}

fn reference_cases() -> Vec<ReferenceCase> {
    vec![
        ReferenceCase {
            // `Source$VpcInterfaceName`.
            case: "a source that ingests through the VPC interface",
            extra: json!({
                "source": {
                    "name": "vpc-src",
                    "protocol": "rtp",
                    "whitelistCidr": "10.0.0.0/16",
                    "vpcInterfaceName": "vpc-ref"
                }
            }),
            referenced: "vpc-ref",
            free: "vpc-free",
        },
        ReferenceCase {
            // `Output$VpcInterfaceAttachment`.
            case: "an output attached to the VPC interface",
            extra: json!({
                "outputs": [{
                    "name": "vpc-out",
                    "protocol": "rtp-fec",
                    "destination": "203.0.113.10",
                    "port": 5000,
                    "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-ref" }
                }]
            }),
            referenced: "vpc-ref",
            free: "vpc-free",
        },
        ReferenceCase {
            // `GatewayBridgeSource$VpcInterfaceAttachment`.
            case: "a source fed by a gateway bridge over the VPC interface",
            extra: json!({
                "source": {
                    "name": "bridge-src",
                    "gatewayBridgeSource": {
                        "bridgeArn": "arn:aws:mediaconnect:us-east-1:111122223333:bridge:1-0000cafe:br-1",
                        "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-ref" }
                    }
                }
            }),
            referenced: "vpc-ref",
            free: "vpc-free",
        },
    ]
}

/// "This request can be made only on a VPC interface that does not have a Source or Output
/// associated with it": a VPC interface any of the flow's sources or outputs references is
/// refused `BadRequestException` and stays on the flow, while one of the same flow that
/// nothing references is removed.
#[tokio::test]
async fn remove_flow_vpc_interface_refuses_a_referenced_interface() {
    let emulator = Emulator::new();

    for (index, row) in reference_cases().into_iter().enumerate() {
        let ReferenceCase {
            case,
            extra,
            referenced,
            free,
        } = row;
        let flow_arn = create_flow_with_vpc_interfaces(
            &emulator,
            case,
            &format!("flow-vpc-ref-{index}"),
            json!([
                vpc_interface_request(referenced, None, &["sg-0123456789abcdef0"]),
                vpc_interface_request(free, None, &["sg-0123456789abcdef0"]),
            ]),
            extra,
        )
        .await;
        let before = described_vpc_interfaces(&emulator, case, &flow_arn).await;

        let (status, code, body) = remove_vpc_interface(&emulator, &flow_arn, referenced).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: removing the referenced VPC interface {referenced:?} expected 400 \
             BadRequestException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .starts_with(mc::VALIDATION_FAILED_PREFIX),
            "case {case:?}: the refusal keeps this service's validation envelope, got {body}"
        );
        let after = described_vpc_interfaces(&emulator, case, &flow_arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused remove leaves every VPC interface in place, got \
             {after:?}"
        );

        // The one nothing references is removable, so the refusal is about the reference
        // and not about the flow.
        remove_at(&emulator, case, &flow_arn, 1).await;
    }
}

/// "If the VPC interface is referenced by a Source or Output, you must first delete or
/// update the Source or Output to no longer reference the VPC interface": once the
/// referencing output is gone, the same request that was refused removes the interface.
#[tokio::test]
async fn remove_flow_vpc_interface_succeeds_once_the_reference_is_lifted() {
    let case = "lift an output's attachment by removing the output";
    let emulator = Emulator::new();

    let flow_arn = create_flow_with_vpc_interfaces(
        &emulator,
        case,
        "flow-vpc-lift",
        json!([vpc_interface_request(
            "vpc-attached",
            None,
            &["sg-0123456789abcdef0"]
        )]),
        json!({
            "outputs": [{
                "name": "attached-out",
                "protocol": "rtp-fec",
                "destination": "203.0.113.11",
                "port": 5002,
                "vpcInterfaceAttachment": { "vpcInterfaceName": "vpc-attached" }
            }]
        }),
    )
    .await;

    let (status, code, body) = remove_vpc_interface(&emulator, &flow_arn, "vpc-attached").await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "case {case:?}: while the output is attached the remove is refused, got {status} \
         {code:?}: {body}"
    );

    let (_, _, described) = emulator.describe_flow(&flow_arn).await;
    let output_arn = described["flow"]["outputs"][0]["outputArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, code, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: RemoveFlowOutput expected 202, got {status} {code:?}: {body}"
    );

    remove_at(&emulator, case, &flow_arn, 0).await;
}

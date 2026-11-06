// SPDX-License-Identifier: CC0-1.0

//! The gateways and the Elastic IP addresses a stack declares, read back through AMAZON
//! EC2's own wire.
//!
//! RULE: a stack that reports `CREATE_COMPLETE` has created each declared resource in the
//! service that OWNS it, and the physical resource id it publishes for one is the identifier
//! that service assigned — so every id a `Ref`, an `Fn::GetAtt` or an output of such a stack
//! carries is one the owning service answers a read for, with the configuration the template
//! declared, and one the stack's deletion takes away again.
//!
//! Amazon EC2 is the owning service here, and these are the types that give a VPC its way
//! out: an internet gateway (`CreateInternetGateway`, which assigns an `igw-…` id), a virtual
//! private gateway (`CreateVpnGateway`, `vgw-…`), the attachment that puts one of those on a
//! VPC (`AttachInternetGateway` / `AttachVpnGateway`, which EC2 gives no id of its own), and
//! an Elastic IP address (`AllocateAddress`, whose identity is the ADDRESS, with the
//! allocation id answered beside it). `DescribeInternetGateways`, `DescribeVpnGateways` and
//! `DescribeAddresses` are what say whether the resources a completed stack claims really
//! exist; an id nothing was created for is refused with that resource's own
//! `Invalid…NotFound`, which is also what a deleted one is refused with — the other side of
//! the same rule.
//!
//! The rows differ along what the rule generalises over: which KIND of gateway the attachment
//! puts on the VPC, how many addresses the stack allocates (none, one, two), whether a
//! gateway is attached at all, the template spelling (YAML with short-form intrinsics, JSON),
//! and whether the ids leave the stack through `Ref` or through an `Fn::GetAtt` of the
//! allocation.

use service_com_amazonaws_ec2_amazonec2::state::{
    INVALID_ALLOCATION_ID_NOT_FOUND, INVALID_INTERNET_GATEWAY_ID_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, deploy, ec2, is_service_assigned_id, member, router, wait_for_the_transition, xml_text,
    xml_texts, SERVICE_ID_HEX_DIGITS,
};

/// Which gateway a row's attachment puts on its VPC, and so which call the stack has to have
/// made and which read reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gateway {
    /// `AWS::EC2::InternetGateway`, read back through `DescribeInternetGateways`.
    Internet,
    /// `AWS::EC2::VPNGateway`, read back through `DescribeVpnGateways`.
    VirtualPrivate,
}

/// One deployment of a VPC's way out: the gateway it declares, whether an attachment puts
/// that gateway on the VPC, and the Elastic IP addresses it allocates.
struct GatewayCase {
    case: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The kind of gateway, and the logical id the template gives it.
    gateway: (Gateway, &'static str),
    /// The logical id of the `AWS::EC2::VPC` the attachment is for, or `None` for a row that
    /// declares a gateway attached to nothing.
    attached_to: Option<&'static str>,
    /// The logical ids of the `AWS::EC2::EIP` resources, in declaration order.
    addresses: &'static [&'static str],
    /// The output each address's `Fn::GetAtt AllocationId` leaves the stack through, in the
    /// same order as [`Self::addresses`].
    allocation_outputs: &'static [&'static str],
}

const GATEWAY_CASES: &[GatewayCase] = &[
    GatewayCase {
        case: "an internet gateway attached to a VPC beside two Elastic IP addresses, in \
               YAML",
        stack: "ec2-gateways-internet",
        template: concat!(
            "Resources:\n",
            "  Net:\n",
            "    Type: AWS::EC2::VPC\n",
            "    Properties:\n",
            "      CidrBlock: 10.70.0.0/16\n",
            "  Igw:\n",
            "    Type: AWS::EC2::InternetGateway\n",
            "    Properties:\n",
            "      Tags:\n",
            "        - Key: Name\n",
            "          Value: the way out\n",
            "  IgwAttachment:\n",
            "    Type: AWS::EC2::VPCGatewayAttachment\n",
            "    Properties:\n",
            "      VpcId: !Ref Net\n",
            "      InternetGatewayId: !Ref Igw\n",
            "  NatEipA:\n",
            "    Type: AWS::EC2::EIP\n",
            "    Properties:\n",
            "      Domain: vpc\n",
            "      Tags:\n",
            "        - Key: Name\n",
            "          Value: nat a\n",
            "  NatEipB:\n",
            "    Type: AWS::EC2::EIP\n",
            "    Properties:\n",
            "      Domain: vpc\n",
            "Outputs:\n",
            "  GatewayId:\n",
            "    Value: !Ref Igw\n",
            "  AllocationA:\n",
            "    Value: !GetAtt NatEipA.AllocationId\n",
            "  AllocationB:\n",
            "    Value: !GetAtt NatEipB.AllocationId\n",
        ),
        gateway: (Gateway::Internet, "Igw"),
        attached_to: Some("Net"),
        addresses: &["NatEipA", "NatEipB"],
        allocation_outputs: &["AllocationA", "AllocationB"],
    },
    GatewayCase {
        case: "a virtual private gateway attached to a VPC with one address, in JSON",
        stack: "ec2-gateways-virtual-private",
        template: r#"{"Resources":{
            "Net":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"172.20.0.0/20"}},
            "Vgw":{"Type":"AWS::EC2::VPNGateway","Properties":{
              "Type":"ipsec.1","AmazonSideAsn":64512,
              "Tags":[{"Key":"Tier","Value":"onprem"}]}},
            "VgwAttachment":{"Type":"AWS::EC2::VPCGatewayAttachment","Properties":{
              "VpcId":{"Ref":"Net"},"VpnGatewayId":{"Ref":"Vgw"}}},
            "Eip":{"Type":"AWS::EC2::EIP","Properties":{"Domain":"vpc"}}},
          "Outputs":{
            "GatewayId":{"Value":{"Ref":"Vgw"}},
            "AllocationA":{"Value":{"Fn::GetAtt":["Eip","AllocationId"]}}}}"#,
        gateway: (Gateway::VirtualPrivate, "Vgw"),
        attached_to: Some("Net"),
        addresses: &["Eip"],
        allocation_outputs: &["AllocationA"],
    },
    GatewayCase {
        case: "an internet gateway attached to nothing, with no address at all, in JSON",
        stack: "ec2-gateways-unattached",
        template: r#"{"Resources":{
            "Spare":{"Type":"AWS::EC2::InternetGateway"}},
          "Outputs":{"GatewayId":{"Value":{"Ref":"Spare"}}}}"#,
        gateway: (Gateway::Internet, "Spare"),
        attached_to: None,
        addresses: &[],
        allocation_outputs: &[],
    },
];

/// The `PhysicalResourceId` the stack reports for one of its resources.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}, {logical_id}) must answer 200, got {status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// One output's value, or the empty string where the stack published no such output.
async fn output(router: &MultiServiceRouter, stack: &str, key: &str) -> String {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack}) must answer 200, got {status} {body}"
    );
    xml_texts(&body, "OutputKey")
        .into_iter()
        .zip(xml_texts(&body, "OutputValue"))
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// The tag value a described EC2 resource carries under `key`, read off the `tagSet` the
/// answer reports.
fn tag_value(body: &str, key: &str) -> Option<String> {
    xml_texts(body, "key")
        .into_iter()
        .zip(xml_texts(body, "value"))
        .find(|(held, _)| held == key)
        .map(|(_, value)| value)
}

/// The read that reports one gateway, as the status and the body EC2 answered.
async fn describe_gateway(
    router: &MultiServiceRouter,
    gateway: Gateway,
    gateway_id: &str,
) -> (u16, String) {
    match gateway {
        Gateway::Internet => {
            ec2(
                router,
                "DescribeInternetGateways",
                &[("InternetGatewayId.1", gateway_id)],
            )
            .await
        }
        Gateway::VirtualPrivate => {
            ec2(
                router,
                "DescribeVpnGateways",
                &[("VpnGatewayId.1", gateway_id)],
            )
            .await
        }
    }
}

/// What Amazon EC2 reports for a gateway a stack's deletion has taken away.
///
/// Which it is follows from the gateway's own API: an internet gateway `DeleteInternetGateway`
/// has taken away is GONE, so the id is refused; a virtual private gateway's state enum
/// carries `deleting` and `deleted` beside `available`, so a deleted one is still reported —
/// in that terminal state, attached to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gone {
    /// The id names no such resource any more, refused with this code.
    Refused(&'static str),
    /// The resource is still reported, in this state.
    Reported(&'static str),
}

/// The id prefix, the element the describe reports the id under and what EC2 answers for the
/// gateway once the stack that created it is gone, for one kind of gateway.
fn gateway_wire(gateway: Gateway) -> (&'static str, &'static str, Gone) {
    match gateway {
        Gateway::Internet => (
            "igw",
            "internetGatewayId",
            Gone::Refused(INVALID_INTERNET_GATEWAY_ID_NOT_FOUND),
        ),
        Gateway::VirtualPrivate => {
            ("vgw", "vpnGatewayId", Gone::Reported("deleted"))
        }
    }
}

#[tokio::test]
async fn the_gateways_and_addresses_a_stack_declares_are_created_in_ec2() {
    for row in GATEWAY_CASES {
        let case = row.case;
        let router = router();
        deploy(&router, row.stack, row.template).await;

        let (gateway, gateway_logical_id) = row.gateway;
        let (prefix, id_element, gone) = gateway_wire(gateway);
        let gateway_id = physical_id(&router, row.stack, gateway_logical_id).await;
        assert!(
            is_service_assigned_id(&gateway_id, prefix),
            "case {case:?}: expected the gateway's physical id to carry {prefix}- plus \
             {SERVICE_ID_HEX_DIGITS} hexadecimal characters, got {gateway_id:?}"
        );
        assert_eq!(
            output(&router, row.stack, "GatewayId").await,
            gateway_id,
            "case {case:?}: expected the GatewayId output to carry the id EC2 assigned"
        );

        // Amazon EC2 holds that gateway: a stack reports a resource created only once its
        // owning service does.
        let (status, described) = describe_gateway(&router, gateway, &gateway_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: EC2 must report the gateway {gateway_id:?} the stack created, got \
             {status} {described}"
        );
        assert_eq!(
            xml_text(&described, id_element).as_deref(),
            Some(gateway_id.as_str()),
            "case {case:?}: expected the read to report that gateway, got {described}"
        );
        // The provenance CloudFormation records on every resource it creates reached EC2,
        // which only a real create could carry.
        assert_eq!(
            tag_value(&described, "aws:cloudformation:logical-id"),
            Some(gateway_logical_id.to_string()),
            "case {case:?}: expected the gateway to carry the stack's logical-id tag, got \
             {described}"
        );
        assert_eq!(
            tag_value(&described, "aws:cloudformation:stack-name"),
            Some(row.stack.to_string()),
            "case {case:?}: expected the gateway to carry the stack-name tag, got {described}"
        );

        // The attachment the stack declared is in force in EC2: the gateway reports the VPC
        // the stack created, read off the describe's own attachment element. A row that
        // declares no attachment has a gateway attached to nothing, which is what EC2
        // reports for a gateway nothing attached.
        let attached_vpc = xml_text(&described, "vpcId");
        match row.attached_to {
            Some(vpc_logical_id) => {
                let vpc_id = physical_id(&router, row.stack, vpc_logical_id).await;
                assert_eq!(
                    attached_vpc.as_deref(),
                    Some(vpc_id.as_str()),
                    "case {case:?}: expected the gateway to be attached to the stack's VPC \
                     {vpc_id:?}, got {described}"
                );
            }
            None => assert_eq!(
                attached_vpc, None,
                "case {case:?}: expected the gateway to be attached to nothing, got \
                 {described}"
            ),
        }

        // Each Elastic IP address the stack declared: "Ref returns the Elastic IP address",
        // so the physical id is the ADDRESS, and the allocation id the stack publishes
        // through `Fn::GetAtt` is the one EC2 answered the allocation with — the value every
        // VPC use of the address is made with.
        let mut allocations = Vec::new();
        for (address_logical_id, output_key) in
            row.addresses.iter().zip(row.allocation_outputs)
        {
            let case = format!("{case} / {address_logical_id}");
            let public_ip = physical_id(&router, row.stack, address_logical_id).await;
            assert!(
                public_ip.split('.').count() == 4
                    && public_ip
                        .split('.')
                        .all(|octet| octet.parse::<u8>().is_ok()),
                "case {case:?}: expected the address's physical id to be an IPv4 address, got \
                 {public_ip:?}"
            );
            let allocation_id = output(&router, row.stack, output_key).await;
            assert!(
                is_service_assigned_id(&allocation_id, "eipalloc"),
                "case {case:?}: expected the {output_key} output to carry eipalloc- plus \
                 {SERVICE_ID_HEX_DIGITS} hexadecimal characters, got {allocation_id:?}"
            );
            // EC2 holds that allocation, and the address it reports for it is the one the
            // stack published as the resource's own identity.
            let (status, described) = ec2(
                &router,
                "DescribeAddresses",
                &[("AllocationId.1", &allocation_id)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: EC2 must report the address {allocation_id:?} the stack \
                 allocated, got {status} {described}"
            );
            assert_eq!(
                xml_text(&described, "publicIp").as_deref(),
                Some(public_ip.as_str()),
                "case {case:?}: expected the allocation to carry the address the stack \
                 published, got {described}"
            );
            assert_eq!(
                tag_value(&described, "aws:cloudformation:logical-id"),
                Some((*address_logical_id).to_string()),
                "case {case:?}: expected the address to carry the stack's logical-id tag, got \
                 {described}"
            );
            allocations.push(allocation_id);
        }
        // Every address is one of its own: a stack declaring two allocates two.
        let mut distinct = allocations.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            row.addresses.len(),
            "case {case:?}: expected {} distinct allocations, got {allocations:?}",
            row.addresses.len()
        );

        // ...and deleting the stack takes them away in EC2, which is what the stack having
        // created them means.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStack must answer 200, got {status} {body}"
        );
        wait_for_the_transition().await;
        let (status, answered) = describe_gateway(&router, gateway, &gateway_id).await;
        match gone {
            Gone::Refused(code) => assert_eq!(
                (status, xml_text(&answered, "Code")),
                (400, Some(code.to_string())),
                "case {case:?}: expected EC2 to refuse the deleted gateway {gateway_id:?}, got \
                 {status} {answered}"
            ),
            Gone::Reported(state) => assert_eq!(
                (status, xml_text(&answered, "state")),
                (200, Some(state.to_string())),
                "case {case:?}: expected EC2 to report the deleted gateway {gateway_id:?} as \
                 {state}, got {status} {answered}"
            ),
        }
        for allocation_id in &allocations {
            let (status, refused) = ec2(
                &router,
                "DescribeAddresses",
                &[("AllocationId.1", allocation_id)],
            )
            .await;
            assert_eq!(
                (status, xml_text(&refused, "Code")),
                (400, Some(INVALID_ALLOCATION_ID_NOT_FOUND.to_string())),
                "case {case:?}: expected EC2 to refuse the released allocation \
                 {allocation_id:?}, got {status} {refused}"
            );
        }
    }
}

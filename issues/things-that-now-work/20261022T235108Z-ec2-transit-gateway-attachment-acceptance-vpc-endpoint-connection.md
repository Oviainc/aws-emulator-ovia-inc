Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-acceptTransitGatewayClientVpnAttachment ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations-b ec2.op-contract-acceptTransitGatewayPeeringAttachment ec2.op-contract-acceptTransitGatewayPeeringAttachment-b ec2.op-contract-acceptTransitGatewayVpcAttachment-b ec2.op-contract-reject-transit-gateway-vpc-attachment-rejected ec2.op-contract-deleteTransitGatewayRouteTableAnnouncement ec2.op-contract-acceptVpcEndpointConnections ec2.op-contract-acceptVpcEndpointConnections-b ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive ec2.op-contract-advertiseByoipCidr ec2.op-contract-advertiseByoipCidr-b ec2.op-contract-move-byoip-cidr-to-ipam-rejected ec2.transit-gateway-family-error-semantics
Issue-Key: ec2.op-contract-acceptTransitGatewayClientVpnAttachment|run#4|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations|run#5|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations-b|run#5|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptTransitGatewayPeeringAttachment|run#4|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptTransitGatewayPeeringAttachment-b|run#4|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptTransitGatewayVpcAttachment-b|run#4|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-reject-transit-gateway-vpc-attachment-rejected|run#4|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-deleteTransitGatewayRouteTableAnnouncement|run#3|AssertionError|assert absent == "InvalidTransitGatewayRouteTableAnnouncementId.NotFound", (
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections|run#6|AssertionError|assert info["code"] == "InvalidVpcEndpointServiceId.NotFound", (
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections-b|run#5|AssertionError|assert info["code"] == "InvalidVpcEndpointServiceId.NotFound", (
Issue-Key: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive|run#8|AssertionError|assert code == "InvalidVpcEndpointId.NotFound", (
Issue-Key: ec2.op-contract-advertiseByoipCidr|run#3|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-advertiseByoipCidr-b|run#3|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-move-byoip-cidr-to-ipam-rejected|run#4|AssertionError|assert code == "InvalidIpamPoolId.NotFound", f"expected InvalidIpamPoolId.NotFound, got {code}"
Issue-Key: ec2.transit-gateway-family-error-semantics|run#6|AssertionError|assert not problems, "unexpected transit-gateway error semantics:\n" + "\n".join(problems)
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 transit-gateway attachment acceptance, VPC-endpoint connection and BYOIP operations answer 501 NotImplemented instead of their modeled responses (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptTransitGatewayVpcAttachment

## Description
Ten unimplemented EC2 operations, each its own cause. All ten must be closed before submitting. Common shape: every one of these operations models a DryRun member and no error shapes (Smithy com-amazonaws-ec2-amazonec2: e.g. AcceptVpcEndpointConnections input DryRun: Boolean, ServiceId/VpcEndpointIds required, errors: [] — EC2's error codes are ec2Query wire codes, documented rather than modeled). AWS therefore answers an authorized DryRun=true call with 400 DryRunOperation 'Request would have succeeded, but DryRun flag is set.' and an unknown-id call with that operation's own NotFound/Malformed code. The emulator answers all of them with NotImplemented ('operation <Op> is scaffolded but not implemented in this emulator'), so neither the permission contract nor the not-found boundary is observable. Why, for all ten: a client cannot probe permissions with DryRun and cannot distinguish 'no such resource' from 'service broken' — every negative path on these surfaces is unreachable, and transit-gateway/VPC-endpoint/BYOIP workflows cannot be modelled at all. Fix, for all ten: implement the named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Cause 1 of 10: AcceptTransitGatewayClientVpnAttachment — expected DryRunOperation, got NotImplemented; the unknown-attachment arm expects InvalidTransitGatewayAttachmentID.NotFound. Trace: ec2.op-contract-acceptTransitGatewayClientVpnAttachment. Handler: accept_transit_gateway_client_vpn_attachment.rs.
Cause 2 of 10: AcceptTransitGatewayMulticastDomainAssociations — expected DryRunOperation, got NotImplemented. Traces: ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations, ec2.op-contract-acceptTransitGatewayMulticastDomainAssociations-b.
Cause 3 of 10: AcceptTransitGatewayPeeringAttachment — expected DryRunOperation, got NotImplemented. Traces: ec2.op-contract-acceptTransitGatewayPeeringAttachment, -b.
Cause 4 of 10: AcceptTransitGatewayVpcAttachment — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-acceptTransitGatewayVpcAttachment-b.
Cause 5 of 10: RejectTransitGatewayVpcAttachment — expected DryRunOperation, got NotImplemented. Trace: ec2.op-contract-reject-transit-gateway-vpc-attachment-rejected.
Cause 6 of 10: DeleteTransitGatewayRouteTableAnnouncement — expected a NotFound for an absent announcement id, got NotImplemented. Trace: ec2.op-contract-deleteTransitGatewayRouteTableAnnouncement.
Cause 7 of 10: AcceptVpcEndpointConnections — expected InvalidVpcEndpointServiceId.NotFound for an unknown ServiceId, got NotImplemented. Traces: ec2.op-contract-acceptVpcEndpointConnections, -b.
Cause 8 of 10: DescribeVpcEndpoints — AWS refuses an unknown endpoint id with InvalidVpcEndpointId.NotFound; the emulator answers NotImplemented, so a dangling vpce reference cannot be detected. Trace: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.
Cause 9 of 10: AdvertiseByoipCidr — expected DryRunOperation, got NotImplemented. Traces: ec2.op-contract-advertiseByoipCidr, -b.
Cause 10 of 10: MoveByoipCidrToIpam — expected InvalidIpamPoolId.NotFound, got NotImplemented. Trace: ec2.op-contract-move-byoip-cidr-to-ipam-rejected.

The aggregate family check ec2.transit-gateway-family-error-semantics fails over this same surface and is listed in trace_ids so it is re-replayed with the fix.

## Evidence
Representative replay output: AssertionError: expected DryRunOperation, got 'NotImplemented' (causes 1-5, 9); AssertionError: expected a NotFound for an absent announcement id, got 'NotImplemented' (cause 6); AssertionError: expected InvalidVpcEndpointServiceId.NotFound, got 'NotImplemented' (cause 7); AssertionError: DescribeVpcEndpoints for 'vpce-00000000000000000' raised 'NotImplemented', expected InvalidVpcEndpointId.NotFound (cause 8); AssertionError: expected InvalidIpamPoolId.NotFound, got NotImplemented (cause 10). Every NotImplemented body reads 'operation <Op> is scaffolded but not implemented in this emulator'. Expected status/codes: HTTP 400 DryRunOperation with message 'Request would have succeeded, but DryRun flag is set.' for the DryRun arms, and the operation's own NotFound code for the unknown-id arms.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repro per cause: call the named EC2 operation twice with a well-formed but nonexistent resource id — once with DryRun=True (expect DryRunOperation) and once without (expect the operation's NotFound/Malformed code) — and observe NotImplemented both times.

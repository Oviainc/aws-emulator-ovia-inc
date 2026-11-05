Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.transit-gateway-family-error-semantics ec2.op-contract-create-transit-gateway-peering-attachment ec2.op-contract-create-transit-gateway-policy-table ec2.op-contract-create-transit-gateway-route ec2.op-contract-create-transit-gateway-vpc-attachment ec2.local-gateway-family-error-semantics ec2.create-coip-pool-invalid-route-table-rejected ec2.op-contract-search-local-gateway-routes ec2.client-vpn-and-coip-error-semantics ec2.vpn-and-vpc-config-error-semantics ec2.op-contract-attachVpnGateway ec2.op-contract-associateVpcCidrBlock ec2.op-contract-modify-vpc-tenancy-notfound-rejected ec2.op-contract-modify-vpc-block-public-access-exclusion ec2.image-task-and-fpga-error-semantics ec2.instance-task-error-semantics ec2.verified-access-and-insights-error-semantics ec2.dhcp-options-associate-delete-dependency ec2.disassociate-route-server-unassociated-vpc-rejected ec2.route-server-delete-blocked-by-association-rejected ec2.concurrent-modify-route-server-optimistic-lock-conflict ec2.concurrent-associate-route-server-same-vpc-race ec2.concurrent-capacity-reservation-split-oversubscription-rejected ec2.op-contract-cancel-reserved-instances-listing ec2.op-contract-purchase-capacity-block-extension-rejected ec2.op-contract-purchase-host-reservation-rejected ec2.op-contract-disassociate-enclave-certificate-iam-role ec2.op-contract-modify-ipam-routing-policy-registration
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.transit-gateway-family-error-semantics|run#6|AssertionError|assert not problems, "unexpected transit-gateway error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.op-contract-create-transit-gateway-peering-attachment|run#8|AssertionError|assert code == "InvalidTransitGatewayID.NotFound", f"unexpected code: {code}"
Issue-Key: ec2.op-contract-create-transit-gateway-policy-table|run#6|AssertionError|assert code == "DryRunOperation", f"unexpected code: {code}"
Issue-Key: ec2.op-contract-create-transit-gateway-route|run#7|AssertionError|assert code == "DryRunOperation", f"unexpected code: {code}"
Issue-Key: ec2.op-contract-create-transit-gateway-vpc-attachment|run#6|AssertionError|assert code == "InvalidSubnetID.NotFound", f"unexpected code: {code}"
Issue-Key: ec2.local-gateway-family-error-semantics|run#7|AssertionError|assert not problems, "unexpected local-gateway error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.create-coip-pool-invalid-route-table-rejected|run#7|AssertionError|assert code == "InvalidLocalGatewayRouteTableID.NotFound", (
Issue-Key: ec2|DescribeLocalGatewayRouteTables|InvalidLocalGatewayRouteTableID.NotFound
Issue-Key: ec2.client-vpn-and-coip-error-semantics|run#5|AssertionError|assert not problems, "unexpected client-VPN/COIP error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.vpn-and-vpc-config-error-semantics|run#4|AssertionError|assert not problems, "unexpected VPN/VPC-config error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.op-contract-attachVpnGateway|run#8|AssertionError|assert code == "InvalidVpnGatewayID.NotFound", f"expected InvalidVpnGatewayID.NotFound, got {code}"
Issue-Key: ec2.op-contract-associateVpcCidrBlock|run#6|AssertionError|assert code == "InvalidVpcID.NotFound", f"expected InvalidVpcID.NotFound, got {code!r}"
Issue-Key: ec2.op-contract-modify-vpc-tenancy-notfound-rejected|run#9|AssertionError|assert err == "InvalidVpcID.NotFound", err
Issue-Key: ec2.op-contract-modify-vpc-block-public-access-exclusion|run#14|AssertionError|assert err == "DryRunOperation", err
Issue-Key: ec2.image-task-and-fpga-error-semantics|run#4|AssertionError|assert not problems, "unexpected image/FPGA error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.instance-task-error-semantics|run#9|AssertionError|assert not problems, "unexpected instance-task error semantics:\n" + "\n".join(problems)
Issue-Key: ec2.verified-access-and-insights-error-semantics|run#4|AssertionError|assert not problems, (
Issue-Key: ec2.dhcp-options-associate-delete-dependency|run#15|AssertionError|assert code == "DependencyViolation", (
Issue-Key: ec2.disassociate-route-server-unassociated-vpc-rejected|run#11|AssertionError|assert code == "InvalidRouteServerId.NotAssociated", f"unexpected error code for unassociated pairing: {code}"
Issue-Key: ec2.route-server-delete-blocked-by-association-rejected|run#13|AssertionError|assert code == "IncorrectState", f"unexpected error code for delete-with-live-association: {code!r}"
Issue-Key: ec2.concurrent-modify-route-server-optimistic-lock-conflict|run#12|AssertionError|assert len(successes) >= 1, f"expected at least one ModifyRouteServer to succeed, got {results}"
Issue-Key: ec2.concurrent-associate-route-server-same-vpc-race|run#15|AssertionError|assert code in ("IncorrectState", "InvalidRouteServerAssociation.NotFound", "RouteServerAssociationAlreadyExists"), (
Issue-Key: ec2.concurrent-capacity-reservation-split-oversubscription-rejected|run#14|AssertionError|assert len(successes) == 1, f"expected exactly one split to succeed, got {results}"
Issue-Key: ec2.op-contract-cancel-reserved-instances-listing|run#5|AssertionError|assert code == "InvalidInput", f"expected InvalidInput, got {code}"
Issue-Key: ec2.op-contract-purchase-capacity-block-extension-rejected|run#4|AssertionError|assert err == "InvalidCapacityReservationId.NotFound", (
Issue-Key: ec2.op-contract-purchase-host-reservation-rejected|run#6|AssertionError|assert err == "InvalidHostId.NotFound", (
Issue-Key: ec2.op-contract-disassociate-enclave-certificate-iam-role|run#15|AssertionError|assert code == "DryRunOperation", code
Issue-Key: ec2.op-contract-modify-ipam-routing-policy-registration|run#5|AssertionError|assert code == "InvalidIpamInternetRegistryAssociationId.Malformed", (
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2 refusal paths answer NotImplemented instead of the modelled client error, so every negative contract in nine operation families is unreachable (9 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransitGatewayVpcAttachment

## Description
Nine root causes, one per operation family, in which a request that AWS refuses with a specific modelled client error is instead refused with the emulator's scaffold code `NotImplemented` and the message `operation <OperationName> is scaffolded but not implemented in this emulator`. These are NOT the checks the suite marked ALREADY FILED; they are separate families whose negative contract no filed issue carries.

AWS, for every cause below: the refusal is a 400-class ec2Query error whose Code is the resource-specific one named per cause, with the message AWS words for that class (fixed sentence plus the id/value from the request). Emulator, for every cause: code `NotImplemented`, message `operation <Op> is scaffolded but not implemented in this emulator`, which no client catches and which cannot be distinguished from a server fault.
Why, for every cause: the only way a client learns its input is wrong is the specific code; NotImplemented turns a permanent input error into an opaque failure, and dry-run/preflight and conflict-retry paths built on these codes never fire.
Fix, for every cause: the named operations' handlers (services/service-com-amazonaws-ec2-amazonec2/src/ops/) must implement the validation and mint the modelled error before any scaffold short-circuit.

Cause 1 of 9 — transit gateway. Required codes: CreateTransitGatewayMulticastDomain and CreateTransitGatewayPolicyTable -> `InvalidTransitGatewayID.NotFound`; CreateTransitGatewayVpcAttachment -> one of `InvalidSubnetID.NotFound` / `InvalidTransitGatewayID.NotFound` / `InvalidVpcID.NotFound`; CreateTransitGatewayPeeringAttachment and CreateTransitGatewayConnect -> `InvalidParameterValue`; CreateTransitGatewayRoute -> `InvalidRouteTableID.NotFound`. Traces: ec2.transit-gateway-family-error-semantics, ec2.op-contract-create-transit-gateway-peering-attachment, ec2.op-contract-create-transit-gateway-policy-table, ec2.op-contract-create-transit-gateway-route, ec2.op-contract-create-transit-gateway-vpc-attachment.

Cause 2 of 9 — local gateway / COIP. Required code for CreateCoipPool naming a well-formed but nonexistent local-gateway route table: `InvalidLocalGatewayRouteTableID.NotFound`. Traces: ec2.local-gateway-family-error-semantics, ec2.create-coip-pool-invalid-route-table-rejected, ec2.op-contract-search-local-gateway-routes.

Cause 3 of 9 — client VPN and COIP error semantics as a family. Traces: ec2.client-vpn-and-coip-error-semantics.

Cause 4 of 9 — VPN gateway / VPC CIDR / VPC tenancy / VPC block-public-access. Required codes: AttachVpnGateway -> `InvalidVpnGatewayID.NotFound`; AssociateVpcCidrBlock -> `InvalidVpcID.NotFound`; ModifyVpcTenancy and ModifyVpcBlockPublicAccessExclusion -> their not-found codes. Traces: ec2.vpn-and-vpc-config-error-semantics, ec2.op-contract-attachVpnGateway, ec2.op-contract-associateVpcCidrBlock, ec2.op-contract-modify-vpc-tenancy-notfound-rejected, ec2.op-contract-modify-vpc-block-public-access-exclusion.

Cause 5 of 9 — image/FPGA task error semantics as a family. Traces: ec2.image-task-and-fpga-error-semantics.

Cause 6 of 9 — instance-task error semantics as a family. Traces: ec2.instance-task-error-semantics.

Cause 7 of 9 — Verified Access and Network Insights error semantics as a family. Traces: ec2.verified-access-and-insights-error-semantics.

Cause 8 of 9 — dependency and state guards in the DHCP-options and route-server planes. Required: deleting a DHCP option set still associated with a VPC -> `DependencyViolation`; DisassociateRouteServer for a VPC that is not associated -> its not-found/invalid-state code; DeleteRouteServer with a live association -> `DependencyViolation`; ModifyRouteServer under concurrency -> at least one success plus a well-known conflict code; AssociateRouteServer racing on one VPC -> a well-known conflict code (the emulator answers `Resource.AlreadyAssociated`, which is not one); CreateCapacityReservationBySplitting under concurrency -> exactly one success. Traces: ec2.dhcp-options-associate-delete-dependency, ec2.disassociate-route-server-unassociated-vpc-rejected, ec2.route-server-delete-blocked-by-association-rejected, ec2.concurrent-modify-route-server-optimistic-lock-conflict, ec2.concurrent-associate-route-server-same-vpc-race, ec2.concurrent-capacity-reservation-split-oversubscription-rejected.

Cause 9 of 9 — purchase/listing/association refusals. Required codes: CancelReservedInstancesListing -> `InvalidInput`; PurchaseCapacityBlockExtension -> `InvalidCapacityReservationId.NotFound`; PurchaseHostReservation with a well-formed but nonexistent host id against a real offering -> `InvalidHostId.NotFound`; DisassociateEnclaveCertificateIamRole and ModifyIpamRoutingPolicyRegistration -> `InvalidIpamInternetRegistryAssociationId.Malformed` / their modelled codes. Traces: ec2.op-contract-cancel-reserved-instances-listing, ec2.op-contract-purchase-capacity-block-extension-rejected, ec2.op-contract-purchase-host-reservation-rejected, ec2.op-contract-disassociate-enclave-certificate-iam-role, ec2.op-contract-modify-ipam-routing-policy-registration.

## Evidence
Verbatim replay diagnosis lines.
Cause 1: `AssertionError: unexpected transit-gateway error semantics:\ncreate_transit_gateway_multicast_domain: expected ['InvalidTransitGatewayID.NotFound'], got NotImplemented\ncreate_transit_gateway_policy_table: expected ['InvalidTransitGatewayID.NotFound'], got NotImplemented\ncreate_transit_gateway_vpc_attachment: expected ['InvalidSubnetID.NotFound', 'InvalidTransitGatewayID.NotFound', 'InvalidVpcID.NotFound'], got NotImplemented\ncreate_transit_gateway_peering_attachment: expected ['InvalidParameterValue'], got NotImplemented\ncreate_transit_gateway_connect: expected ['InvalidParameterValue'], got NotImplemented\ncreate_transit_gateway_route: expected ['InvalidRouteTableID.NotFound'], got NotImplemented`; plus four `AssertionError: unexpected code: NotImplemented`.
Cause 2: `expected InvalidLocalGatewayRouteTableID.NotFound for a well-formed but nonexistent local gateway route table id, got 'NotImplemented'`.
Cause 3: `AssertionError: unexpected client-VPN/COIP error semantics:`.
Cause 4: `expected InvalidVpnGatewayID.NotFound, got NotImplemented`; `expected InvalidVpcID.NotFound, got 'NotImplemented'`; `AssertionError: NotImplemented` (modify-vpc-tenancy-notfound-rejected and modify-vpc-block-public-access-exclusion); `AssertionError: unexpected VPN/VPC-config error semantics:`.
Cause 5/6/7: `unexpected image/FPGA error semantics:`; `unexpected instance-task error semantics:`; `unexpected verified-access/insights error semantics:`.
Cause 8: `expected DependencyViolation deleting an associated DHCP option set, got NotImplemented`; `unexpected error code for unassociated pairing: NotImplemented`; `unexpected error code for delete-with-live-association: 'NotImplemented'`; `expected at least one ModifyRouteServer to succeed, got [('error', 'NotImplemented'), ('error', 'NotImplemented')]`; `expected a racing AssociateRouteServer failure to be a well-known conflict code, got Resource.AlreadyAssociated`; `expected exactly one split to succeed, got [('error', 'NotImplemented'), ('error', 'NotImplemented')]`.
Cause 9: `expected InvalidInput, got NotImplemented`; `expected InvalidCapacityReservationId.NotFound, got NotImplemented`; `a well-formed but nonexistent host ID against a REAL offering: expected InvalidHostId.NotFound, got 'NotImplemented'`; `AssertionError: NotImplemented`; `expected InvalidIpamInternetRegistryAssociationId.Malformed, got NotImplemented`.

## Steps to Reproduce
1. Boot the emulator; point an EC2 client at it.
2. Cause 1: CreateTransitGatewayVpcAttachment naming a nonexistent transit gateway, VPC and subnet; observe NotImplemented instead of InvalidTransitGatewayID.NotFound / InvalidVpcID.NotFound / InvalidSubnetID.NotFound. Repeat for CreateTransitGatewayRoute, CreateTransitGatewayPolicyTable, CreateTransitGatewayMulticastDomain, CreateTransitGatewayConnect, CreateTransitGatewayPeeringAttachment.
3. Cause 2: CreateCoipPool with LocalGatewayRouteTableId='lgw-rtb-0123456789abcdef0'.
4. Cause 4: AttachVpnGateway with an unknown vgw id; AssociateVpcCidrBlock with an unknown vpc id; ModifyVpcTenancy with an unknown vpc id.
5. Cause 8: CreateDhcpOptions, AssociateDhcpOptions to a VPC, then DeleteDhcpOptions — expect DependencyViolation. DisassociateRouteServer for an unassociated VPC. DeleteRouteServer with a live association. Two concurrent ModifyRouteServer calls.
6. Cause 9: CancelReservedInstancesListing with a bad input; PurchaseHostReservation with host id 'h-0123456789abcdef0'.
7. Re-replay the check ids in trace_ids.

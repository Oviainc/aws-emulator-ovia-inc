Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.eip-reassociation-without-allow-reassociation-rejected ec2.op-contract-disassociate-address ec2.op-contract-modify-public-ip-dns-name-options ec2.op-release-associated-eip-fails ec2.op-contract-associate-address-already-associated-rejected ec2.ecsdeploy-s3-gateway-endpoint-adds-prefix-list-route-not-default-route ec2.op-contract-createVpcEndpoint ec2.op-contract-modify-vpc-endpoint-notfound-rejected ec2.create-nat-gateway ec2.create-network-interface-permission ec2.op-contract-delete-network-interface-permission ec2.trunk-interface-association-lifecycle ec2.op-contract-describeTrunkInterfaceAssociations ec2.delete-route-server-with-active-peer-blocked ec2.enable-route-server-propagation-without-association-blocked ec2.ec2mr-pub-replace-route-rejects-detached-internet-gateway-leaves-the-active-route-untouched ec2.op-contract-delete-internet-gateway-dependency-rejected ec2.modify-instance-attribute-wrong-state-fails ec2.op-contract-modify-instance-placement ec2.op-contract-modify-instance-network-performance-options ec2.op-contract-modify-instance-event-start-time ec2.op-contract-modify-instance-capacity-reservation-attributes ec2.enable-application-status-check-suppression-contract ec2.op-contract-disassociate-application-status-check ec2.op-contract-modify-fpga-image-attribute ec2.op-contract-modify-local-gateway-route ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AssociateAddress|NotImplemented
Issue-Key: ec2.op-contract-associate-address-already-associated-rejected|run#19|AssertionError|assert code == "Resource.AlreadyAssociated", f"unexpected error code: {code}"
Issue-Key: ec2|CreateVpcEndpoint|NotImplemented
Issue-Key: ec2.create-nat-gateway|run#14|AssertionError|assert code.startswith("InvalidSubnet"), (
Issue-Key: ec2.create-network-interface-permission|run#9|AssertionError|assert code == "OperationNotPermitted", f"expected OperationNotPermitted, got {code!r}"
Issue-Key: ec2.op-contract-delete-network-interface-permission|run#7|AssertionError|assert grant_code == "OperationNotPermitted", f"expected OperationNotPermitted, got {grant_code!r}"
Issue-Key: ec2.trunk-interface-association-lifecycle|run#18|AssertionError|assert dis_code == "OperationNotPermitted", f"unexpected code: {dis_code!r}"
Issue-Key: ec2.op-contract-describeTrunkInterfaceAssociations|run#6|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation for DryRun=True, got {dry_code!r}"
Issue-Key: ec2.delete-route-server-with-active-peer-blocked|run#24|AssertionError|assert code == "IncorrectState", f"expected IncorrectState, got {code!r}"
Issue-Key: ec2.enable-route-server-propagation-without-association-blocked|run#12|AssertionError|assert code == "InvalidRouteServerId.NotAssociated", f"expected InvalidRouteServerId.NotAssociated, got {code!r}"
Issue-Key: ec2.ec2mr-pub-replace-route-rejects-detached-internet-gateway-leaves-the-active-route-untouched|run#18|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-delete-internet-gateway-dependency-rejected|run#12|AssertionError|assert code == "DependencyViolation", f"expected DependencyViolation while attached, got {code!r}"
Issue-Key: ec2.modify-instance-attribute-wrong-state-fails|run#7|AssertionError|assert code == "IncorrectInstanceState", f"expected IncorrectInstanceState, got {code}"
Issue-Key: ec2.op-contract-modify-instance-placement|run#9|AssertionError|assert code == "InvalidInstanceState", f"expected InvalidInstanceState, got {code}"
Issue-Key: ec2.op-contract-modify-instance-network-performance-options|run#4|AssertionError|assert running_code == "InvalidState", f"expected InvalidState while running, got {running_code}"
Issue-Key: ec2.op-contract-modify-instance-event-start-time|run#8|AssertionError|assert code_bad_instance == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.op-contract-modify-instance-capacity-reservation-attributes|run#8|AssertionError|assert err is None, f"unexpected error setting preference to none: {err}"
Issue-Key: ec2.enable-application-status-check-suppression-contract|run#20|AssertionError|assert code == "InvalidInstanceID.Malformed", f"unexpected error code for mixed batch: {code}"
Issue-Key: ec2.op-contract-disassociate-application-status-check|run#11|AssertionError|assert code == "InvalidParameterCombination", code
Issue-Key: ec2.op-contract-modify-fpga-image-attribute|run#5|AssertionError|assert code_not_found == "InvalidFpgaImageID.NotFound", (
Issue-Key: ec2.op-contract-modify-local-gateway-route|run#14|AssertionError|assert code == "InvalidLocalGatewayRouteTableID.NotFound", (
Issue-Key: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork|run#8|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b|run#10|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# Ten EC2 request shapes answer 501 NotImplemented instead of the operation's real answer or refusal

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAddress

## Description
This issue bundles 10 net-new root causes, all in the EC2 emulator. Each is a request shape whose handler answers NotImplemented ("... is scaffolded but not implemented in this emulator" or a bare NotImplemented code) where AWS answers normally or with a specific refusal. Smithy: the ec2 model binds awsQuery and models no operation errors (errors: [] on every operation inspected), so every code below is recorded behavior; NotImplemented is not an EC2 error code at all. Related open issues 20261029T175114Z-ec2-refusal-paths-answer-notimplemented-instead-of-the.md and 20261104T044826Z-ec2-lambda-s3-createvpcendpoint-with-route-table-associations-is-refused-notimplemented.md carry other replay keys and do not cover these checks.

Cause 1 of 10: AssociateAddress with NetworkInterfaceId.
AWS: AssociateAddress(AllocationId, NetworkInterfaceId[, PrivateIpAddress, AllowReassociation]) associates the address and returns AssociationId (Smithy: AssociateAddress input carries NetworkInterfaceId/PrivateIpAddress/AllowReassociation; output AssociationId). Re-associating an already-associated address without AllowReassociation is refused Resource.AlreadyAssociated.
Emulator: 501 "operation AssociateAddress with a networkInterfaceId is scaffolded but not implemented in this emulator", so the dependent refusals (Resource.AlreadyAssociated, the release-while-associated refusal and DisassociateAddress) are unreachable.
Why: no caller can attach an EIP to an ENI, the normal path for NAT/appliance instances.
Fix: the AssociateAddress handler — implement the networkInterfaceId branch and its reassociation check.
Traces: ec2.eip-reassociation-without-allow-reassociation-rejected, ec2.op-contract-disassociate-address, ec2.op-contract-modify-public-ip-dns-name-options, ec2.op-release-associated-eip-fails, ec2.op-contract-associate-address-already-associated-rejected.

Cause 2 of 10: CreateVpcEndpoint with RouteTableIds.
AWS: a Gateway endpoint created with RouteTableIds succeeds and installs a prefix-list route into each named table; ModifyVpcEndpoint can add/remove them. Recorded behavior only.
Emulator: 501 "operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator".
Why: S3/DynamoDB gateway-endpoint topologies cannot be built, and the route the endpoint installs is never observable.
Fix: the CreateVpcEndpoint handler (and ModifyVpcEndpoint) — accept RouteTableIds and install the prefix-list route.
Traces: ec2.ecsdeploy-s3-gateway-endpoint-adds-prefix-list-route-not-default-route, ec2.op-contract-createVpcEndpoint, ec2.op-contract-modify-vpc-endpoint-notfound-rejected.

Cause 3 of 10: CreateNatGateway.
AWS: CreateNatGateway naming a subnet that does not exist is refused with an InvalidSubnet* code (InvalidSubnetID.NotFound) naming the subnet id from the request. Recorded behavior only.
Emulator: NotImplemented.
Why: a client validating its subnet wiring gets a transport-level failure instead of the modeled refusal, and NAT topologies cannot be created at all.
Fix: the CreateNatGateway handler and its subnet validation.
Traces: ec2.create-nat-gateway.

Cause 4 of 10: Create/DeleteNetworkInterfacePermission.
AWS: granting an ENI permission for an interface the caller may not share is refused OperationNotPermitted; the same code governs the delete path.
Emulator: NotImplemented for both.
Why: cross-account ENI sharing is untestable and the refusal class is lost.
Fix: the create_network_interface_permission / delete_network_interface_permission handlers.
Traces: ec2.create-network-interface-permission, ec2.op-contract-delete-network-interface-permission.

Cause 5 of 10: trunk-interface association paths.
AWS: AssociateTrunkInterface/DisassociateTrunkInterface refuse an ineligible interface with OperationNotPermitted, and DescribeTrunkInterfaceAssociations with DryRun=true answers DryRunOperation ("Request would have succeeded, but DryRun flag is set.").
Emulator: NotImplemented in both paths.
Why: a caller pre-validating with DryRun cannot distinguish "authorized" from "unsupported".
Fix: the trunk-interface association handlers and their DryRun branch.
Traces: ec2.trunk-interface-association-lifecycle, ec2.op-contract-describeTrunkInterfaceAssociations.

Cause 6 of 10: route-server delete and propagation refusals.
AWS: DeleteRouteServer while a peer is still active is refused IncorrectState; EnableRouteServerPropagation for a route server not associated with the table is refused InvalidRouteServerId.NotAssociated.
Emulator: NotImplemented for both.
Why: the ordering rules of route-server teardown are invisible, so a client cannot learn what it must delete first.
Fix: the route-server delete/propagation handlers and their state checks.
Traces: ec2.delete-route-server-with-active-peer-blocked, ec2.enable-route-server-propagation-without-association-blocked.

Cause 7 of 10: ReplaceRoute onto a detached internet gateway.
AWS: ReplaceRoute naming an internet gateway that is not attached to the route table's VPC is refused InvalidParameterValue and the existing route keeps its live target.
Emulator: NotImplemented.
Why: a failed route swap cannot be distinguished from an unsupported API, and the active route's fate is unknown.
Fix: the ReplaceRoute handler and its gateway-attachment validation.
Traces: ec2.ec2mr-pub-replace-route-rejects-detached-internet-gateway-leaves-the-active-route-untouched.

Cause 8 of 10: DeleteInternetGateway while attached.
AWS: deleting an internet gateway still attached to a VPC is refused DependencyViolation.
Emulator: NotImplemented.
Why: teardown order cannot be learned from the API; a client retries forever.
Fix: the DeleteInternetGateway handler's dependency check.
Traces: ec2.op-contract-delete-internet-gateway-dependency-rejected.

Cause 9 of 10: the ModifyInstance* family.
AWS: ModifyInstanceAttribute on an attribute that requires a stopped instance is refused IncorrectInstanceState; ModifyInstancePlacement on a running instance is refused InvalidInstanceState; ModifyInstanceNetworkPerformanceOptions while running is refused InvalidState; ModifyInstanceEventStartTime with a malformed instance id is refused InvalidInstanceID.Malformed; ModifyInstanceCapacityReservationAttributes setting the preference to none SUCCEEDS.
Emulator: NotImplemented for all five.
Why: a client cannot tell "wrong state, stop the instance first" from "operation unavailable", and a legitimate success is reported as a server error.
Fix: the modify_instance_* handlers, their state validation and id parsing.
Traces: ec2.modify-instance-attribute-wrong-state-fails, ec2.op-contract-modify-instance-placement, ec2.op-contract-modify-instance-network-performance-options, ec2.op-contract-modify-instance-event-start-time, ec2.op-contract-modify-instance-capacity-reservation-attributes.

Cause 10 of 10: application-status-check, FPGA, local-gateway-route and Client VPN DryRun paths.
AWS: EnableApplicationStatusCheckSuppression with a mixed/malformed instance batch is refused InvalidInstanceID.Malformed; DisassociateApplicationStatusCheck with a conflicting parameter pair is refused InvalidParameterCombination; ModifyFpgaImageAttribute on an unknown image is refused InvalidFpgaImageID.NotFound; ModifyLocalGatewayRoute on an unknown route table is refused InvalidLocalGatewayRouteTableID.NotFound; ApplySecurityGroupsToClientVpnTargetNetwork with DryRun=true answers DryRunOperation.
Emulator: NotImplemented in all of them.
Why: each of these is a distinct, documented client-error class a caller branches on; a 501 collapses them into one unusable outcome and makes DryRun pre-validation useless.
Fix: the respective handlers and their parameter/id validation (and the DryRun branch for the Client VPN op).
Traces: ec2.enable-application-status-check-suppression-contract, ec2.op-contract-disassociate-application-status-check, ec2.op-contract-modify-fpga-image-attribute, ec2.op-contract-modify-local-gateway-route, ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork, ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b.

## Evidence
Representative replay output per cause — expected vs actual:
C1: ClientError "An error occurred (NotImplemented) when calling the AssociateAddress operation: operation AssociateAddress with a networkInterfaceId is scaffolded but not implemented in this emulator"; expected a 200 with AssociationId, and Resource.AlreadyAssociated on reassociation without AllowReassociation.
C2: "An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator"; expected a created Gateway endpoint plus the prefix-list route.
C3: "expected an InvalidSubnet* rejection, got 'NotImplemented'".
C4: "expected OperationNotPermitted, got 'NotImplemented'" (both create and delete).
C5: "unexpected code: 'NotImplemented'" (expected OperationNotPermitted) and "expected DryRunOperation for DryRun=True, got 'NotImplemented'".
C6: "expected IncorrectState, got 'NotImplemented'"; "expected InvalidRouteServerId.NotAssociated, got 'NotImplemented'".
C7: "ReplaceRoute onto the unattached gateway 'igw-...' returned 'NotImplemented', expected 'InvalidParameterValue'".
C8: "expected DependencyViolation while attached, got 'NotImplemented'".
C9: "expected IncorrectInstanceState, got NotImplemented"; "expected InvalidInstanceState, got NotImplemented"; "expected InvalidState while running, got NotImplemented"; "expected InvalidInstanceID.Malformed, got NotImplemented"; "unexpected error setting preference to none: NotImplemented".
C10: "unexpected error code for mixed batch: NotImplemented"; "AssertionError: NotImplemented" (expected InvalidParameterCombination); "expected InvalidFpgaImageID.NotFound, got NotImplemented"; "expected InvalidLocalGatewayRouteTableID.NotFound, got NotImplemented"; "expected DryRunOperation, got 'NotImplemented'" (x2).

## Steps to Reproduce
Replay the checks in trace_ids. Minimal reproductions: (1) AllocateAddress + CreateNetworkInterface, then AssociateAddress(AllocationId=..., NetworkInterfaceId=...); (2) CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[rtb-...]); (3) CreateNatGateway(SubnetId='subnet-0123456789abcdef0'); (4) CreateNetworkInterfacePermission on an interface the caller cannot share; (5) DescribeTrunkInterfaceAssociations(DryRun=True); (6) DeleteRouteServer with a live peer / EnableRouteServerPropagation without an association; (7) ReplaceRoute(GatewayId=<detached igw>); (8) DeleteInternetGateway while attached; (9) ModifyInstanceAttribute/ModifyInstancePlacement on a running instance, ModifyInstanceEventStartTime with a malformed id, ModifyInstanceCapacityReservationAttributes(CapacityReservationSpecification={'CapacityReservationPreference':'none'}); (10) EnableApplicationStatusCheckSuppression with a mixed id batch, ModifyFpgaImageAttribute on an unknown afi-, ModifyLocalGatewayRoute on an unknown lgw-rtb-, ApplySecurityGroupsToClientVpnTargetNetwork(DryRun=True).

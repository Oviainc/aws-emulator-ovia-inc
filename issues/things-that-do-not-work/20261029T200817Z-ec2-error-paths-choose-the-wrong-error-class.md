Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-elastic-ip-association
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-elastic-ip-association ec2-conflict-volume-attachment ec2-route-server-delete-lifecycle ec2-networking-association-error-paths-vpc-and-network ec2-spot-fleet-request-lifecycle-monitoring ec2-t12-iam-profile-replace-and-stop-echo ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-instance-event-notification-attributes-registration ec2-byoip-cidr-advertise-withdraw-lifecycle ec2-capacity-reservation-family-error-semantics
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AssociateAddress|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AssociateAddress|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DetachVolume|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DetachVolume|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssociateRouteServer|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AssociateRouteServer|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssociateSecurityGroupVpc|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AssociateSecurityGroupVpc|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|RequestSpotFleet|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|RequestSpotFleet|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ReplaceIamInstanceProfileAssociation|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ReplaceIamInstanceProfileAssociation|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeprovisionIpamByoasn|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeprovisionIpamByoasn|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|RegisterInstanceEventNotificationAttributes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|RegisterInstanceEventNotificationAttributes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|ProvisionByoipCidr|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|ProvisionByoipCidr|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCapacityReservationFleet|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCapacityReservationFleet|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 error paths choose the wrong error class and validate parameters out of AWS's order

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAddress

## Description
Ten net-new causes sharing one defect: on a refusal path the emulator reports a DIFFERENT error class than AWS - either because it checks the request's parameters in the wrong order (so the first thing AWS complains about is never reached) or because it maps a conflict/state condition onto an unrelated code. ec2 models no per-operation error shapes in Smithy (awsQuery/ec2Query, errors: [] on every operation queried), so each code/message pair below is recorded behavior only, taken from the recorded 400 response. Fixed text is the requirement; ids, names and values come from the request or from emulator state.

Cause 1 of 10 - AssociateAddress on an already-associated allocation (ec2-conflict-elastic-ip-association, task#10). AWS: `Resource.AlreadyAssociated`, "resource <eipalloc-id> is already associated with associate-id <eipassoc-id>" (both ids from state). Emulator: `InvalidAllocationID.NotFound`, "The allocation ID '<eipalloc-id>' does not exist" - it loses track of an allocation it just handed out and reports it missing instead of in conflict. Why: a client reconciling an EIP sees 'deleted' where AWS says 'busy', and re-allocates. Fix: AssociateAddress's conflict check and its address state lookup.

Cause 2 of 10 - DetachVolume for a device the volume is not attached at (ec2-conflict-volume-attachment#task#12). AWS: `InvalidAttachment.NotFound`, "The volume '<vol-id>' is not attached to instance '<i-id>' as device '<device>'." Emulator: `IncorrectState` with its own wording ("Volume '...' is not attached to instance '...' at device '...'"). Fix: DetachVolume's attachment lookup must raise the attachment-not-found class, not a state class.

Cause 3 of 10 - AssociateRouteServer against a route server being deleted (ec2-route-server-delete-lifecycle#task#8). AWS: `IncorrectState`, "Route Server <rs-id> is in an invalid state." Emulator: `Resource.AlreadyAssociated` with "The vpc '<vpc-id>' is already associated with route server '<rs-id>'" - the state precondition is never evaluated because the association check runs first.

Cause 4 of 10 - AssociateSecurityGroupVpc with both a malformed group id and an unknown vpc id (ec2-networking-association-error-paths-vpc-and-network#task#4). AWS validates the group id first: `InvalidGroupId.Malformed`, "The security-group ID '<sg-id>' is malformed". Emulator resolves the VPC first and answers `InvalidVpcID.NotFound`.

Cause 5 of 10 - RequestSpotFleet with an invalid IamFleetRole (ec2-spot-fleet-request-lifecycle-monitoring#task#2). AWS: `InvalidSpotFleetRequestConfig`, "Parameter: SpotFleetRequestConfig.IamFleetRole is invalid." Emulator validates the launch specification's image first and answers `InvalidAMIID.NotFound`.

Cause 6 of 10 - ReplaceIamInstanceProfileAssociation with an unknown association id (ec2-t12-iam-profile-replace-and-stop-echo#task#15). AWS: `InvalidAssociationID.NotFound`, "An invalid association-id of '<iip-assoc-id>' was given". Emulator: `InvalidParameterValue`, 'Invalid Id: <iip-assoc-id>'.

Cause 7 of 10 - DeprovisionIpamByoasn for an ASN that is not provisioned (ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#34). AWS: `InvalidAsn.NotFound`, 'The requested ASN could not be found.' - the message carries NO value from the request. Emulator: `InvalidByoasn.NotFound`, "The ASN '<asn>' does not exist".

Cause 8 of 10 - RegisterInstanceEventNotificationAttributes with an empty tag key (ec2-instance-event-notification-attributes-registration#task#5). AWS reports a framework constraint violation: `InvalidRequest`, "1 validation error detected: Value '[]' at 'resourceTagAttribute.resourceTagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1]" - fixed text apart from the member path and the offending value. Emulator: `InvalidParameterValue`, 'A tag key cannot be empty'.

Cause 9 of 10 - ProvisionByoipCidr without CidrAuthorizationContext (ec2-byoip-cidr-advertise-withdraw-lifecycle#task#1). AWS reports the missing required parameter first: `MissingParameter`, 'You must provide a CidrAuthorizationContext parameter.' Emulator skips that check and rejects the CIDR instead: `InvalidParameterValue`, 'Value (<cidr>) for parameter cidr is invalid. Only a public address range can be provisioned.'

Cause 10 of 10 - CreateCapacityReservationFleet missing InstanceMatchCriteria (ec2-capacity-reservation-family-error-semantics#task#3). AWS: `MissingParameter`, 'The following parameter is missing: instanceMatchCriteria'. Emulator validates AllocationStrategy first: `InvalidParameterValue`, "Invalid value '<strategy>' for allocationStrategy. Valid values: prioritized". Smithy: CreateCapacityReservationFleet models InstanceMatchCriteria as an enum member and TotalTargetCapacity as required, so required-ness/presence must be screened before value-domain checks on other members.

Why (all causes): SDK retry and reconciliation logic branches on the error code, and contract tests assert the first refusal AWS would emit. Reordering validation changes which error a client sees for the same request, and mapping a conflict to NotFound (cause 1) actively corrupts client state.

Fix: per-operation validation ordering plus the conflict/state classification in each named handler under services/service-com-amazonaws-ec2-amazonec2/src/ops/. Related but non-covering open issue: 20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md (different operations, no replay key for these steps).

Traces: the representative step plus the nine fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim per cause:
1. AssociateAddress: expected 'Resource.AlreadyAssociated' / 'resource eipalloc-0000000000000002d is already associated with associate-id {{association_id}}'; actual 'InvalidAllocationID.NotFound' / "The allocation ID 'eipalloc-0000000000000002d' does not exist".
2. DetachVolume: expected 'InvalidAttachment.NotFound' / "The volume 'vol-0000000000000002c' is not attached to instance 'i-00000000000000001' as device '/dev/sdz'."; actual 'IncorrectState' / "Volume 'vol-0000000000000002c' is not attached to instance 'i-00000000000000001' at device '/dev/sdz'".
3. AssociateRouteServer: expected 'IncorrectState' / 'Route Server rs-0000000000000002b is in an invalid state.'; actual 'Resource.AlreadyAssociated' / "The vpc 'vpc-00000000000000022' is already associated with route server 'rs-0000000000000002b'".
4. AssociateSecurityGroupVpc: expected 'InvalidGroupId.Malformed' / "The security-group ID 'sg-0123456789abcdef0' is malformed"; actual 'InvalidVpcID.NotFound' / "The vpc ID 'vpc-0123456789abcdef0' does not exist".
5. RequestSpotFleet: expected 'InvalidSpotFleetRequestConfig' / 'Parameter: SpotFleetRequestConfig.IamFleetRole is invalid.'; actual 'InvalidAMIID.NotFound' / "The image id '[ami-0d1bc08881a62440e]' does not exist".
6. ReplaceIamInstanceProfileAssociation: expected 'InvalidAssociationID.NotFound' / "An invalid association-id of 'iip-assoc-0000000000000002c' was given"; actual 'InvalidParameterValue' / 'Invalid Id: iip-assoc-0000000000000002c'.
7. DeprovisionIpamByoasn: expected 'InvalidAsn.NotFound' / 'The requested ASN could not be found.'; actual 'InvalidByoasn.NotFound' / "The ASN '64512' does not exist".
8. RegisterInstanceEventNotificationAttributes: expected 'InvalidRequest' / "1 validation error detected: Value '[]' at 'resourceTagAttribute.resourceTagKeys' failed to satisfy constraint: ..."; actual 'InvalidParameterValue' / 'A tag key cannot be empty'.
9. ProvisionByoipCidr: expected 'MissingParameter' / 'You must provide a CidrAuthorizationContext parameter.'; actual 'InvalidParameterValue' / 'Value (203.0.113.0/24) for parameter cidr is invalid. Only a public address range can be provisioned.'
10. CreateCapacityReservationFleet: expected 'MissingParameter' / 'The following parameter is missing: instanceMatchCriteria'; actual 'InvalidParameterValue' / "Invalid value 'not-a-strategy' for allocationStrategy. Valid values: prioritized".
All ten clusters: HTTP 400 expected and 400 returned, verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it.
2. Run replay_wire_corpus on simparity-wire-aws-v1.
3. For fixture ec2-conflict-elastic-ip-association step task#10 and each fixture#step in trace_ids, compare Response.Errors.Error.Code / .Message in the emulator's 400 with the recorded 400.

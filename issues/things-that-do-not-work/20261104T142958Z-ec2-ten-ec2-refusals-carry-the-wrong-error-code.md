Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.capres-launch-of-a-mismatched-instance-type-into-a-targeted-reservation-rejected ec2.create-delegate-mac-volume-ownership-task-invalid-instance-rejected ec2.ecsdeploy-default-route-to-absent-nat-gateway-rejected ec2.nfwr-second-igw-attach-to-vpc-rejected ec2.op-contract-attachInternetGateway ec2.op-contract-monitor-instances ec2.op-contract-send-diagnostic-interrupt ec2.op-contract-start-network-insights-analysis ec2.op-contract-detach-network-interface ec2.op-contract-disassociate-iam-instance-profile ec2.iam-instance-profile-associate-replace-disassociate ec2.ec2-delete-vpc-removes-own-and-default-resource-tags
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.capres-launch-of-a-mismatched-instance-type-into-a-targeted-reservation-rejected|run#10|AssertionError|assert got == expected, (
Issue-Key: ec2.create-delegate-mac-volume-ownership-task-invalid-instance-rejected|run#11|AssertionError|assert real_code == "InvalidInstanceType", (
Issue-Key: ec2.ecsdeploy-default-route-to-absent-nat-gateway-rejected|run#8|AssertionError|assert code == "InvalidNatGatewayID.NotFound", (
Issue-Key: ec2.nfwr-second-igw-attach-to-vpc-rejected|run#10|AssertionError|assert err["Code"] == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-attachInternetGateway|run#26|AssertionError|assert got == want, f"attaching a second gateway to {vpc1} answered {got}, expected {want}"
Issue-Key: ec2.op-contract-monitor-instances|run#16|AssertionError|assert err == "InvalidParameterValue", f"expected InvalidParameterValue, got {err}"
Issue-Key: ec2.op-contract-send-diagnostic-interrupt|run#10|AssertionError|assert code == "InvalidInstanceID.Malformed", code
Issue-Key: ec2.op-contract-start-network-insights-analysis|run#21|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-detach-network-interface|run#15|AssertionError|assert code == "InvalidAttachmentID.NotFound", (
Issue-Key: ec2|DescribeIamInstanceProfileAssociations|InvalidParameterValue
Issue-Key: ec2|DisassociateIamInstanceProfile|InvalidParameterValue
Issue-Key: ec2|CreateTags|InvalidID
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# Ten EC2 refusals carry the wrong error code, so clients branch on a code AWS never returns

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
This issue bundles 10 net-new root causes in the EC2 emulator. In each, the call is correctly refused but classified under a code AWS does not use for that condition. Smithy: the ec2 model binds awsQuery and models no operation errors (errors: [] on every operation inspected), so the codes AWS uses are recorded behavior only; each cause states the recorded code.

Cause 1 of 10: RunInstances into a targeted capacity reservation with a mismatched instance type.
AWS: refuses InvalidParameterValue (the reservation's type and the requested type must match).
Emulator: answers ParameterNotFound for a CapacityReservationTarget that exists.
Why: ParameterNotFound reads as "the reservation id is unknown", so a client retries with the same id instead of fixing the instance type.
Fix: the RunInstances handler's capacity-reservation matching/validation.
Traces: ec2.capres-launch-of-a-mismatched-instance-type-into-a-targeted-reservation-rejected.

Cause 2 of 10: CreateDelegateMacVolumeOwnershipTask against a real, non-Mac instance.
AWS: refuses InvalidInstanceType — the instance exists, but its type is ineligible.
Emulator: refuses InvalidParameterValue with an instanceId-shaped message.
Why: the caller cannot tell "this id is wrong" from "this instance family cannot do it".
Fix: the create_delegate_mac_volume_ownership_task handler's instance-type classification.
Traces: ec2.create-delegate-mac-volume-ownership-task-invalid-instance-rejected.

Cause 3 of 10: CreateRoute naming an absent NAT gateway.
AWS: refuses InvalidNatGatewayID.NotFound.
Emulator: refuses NatGatewayNotFound, a code that is not in EC2's vocabulary.
Why: error-code matching in client retry/branch logic never fires.
Fix: the CreateRoute handler's NAT-gateway target resolution.
Traces: ec2.ecsdeploy-default-route-to-absent-nat-gateway-rejected.

Cause 4 of 10: AttachInternetGateway when the VPC already has a gateway.
AWS: refuses InvalidParameterValue with the message "Network <vpc-id> already has an internet gateway attached" (fixed text; the vpc-id comes from the request).
Emulator: refuses Resource.AlreadyAssociated with "resource <igw-id> is already attached to network <vpc-id>".
Why: both the code and the message are wrong, so a client that distinguishes "already has one" from an association conflict mis-handles it.
Fix: the AttachInternetGateway handler's duplicate-attachment refusal.
Traces: ec2.nfwr-second-igw-attach-to-vpc-rejected, ec2.op-contract-attachInternetGateway.

Cause 5 of 10: MonitorInstances with an unusable instance id.
AWS: refuses InvalidParameterValue for this input class.
Emulator: refuses InvalidInstanceID.Malformed.
Why: a client that treats *.Malformed as a permanent client bug and InvalidParameterValue as a per-item input error takes the wrong branch.
Fix: the MonitorInstances handler's id validation ordering.
Traces: ec2.op-contract-monitor-instances.

Cause 6 of 10: SendDiagnosticInterrupt with a malformed instance id.
AWS: refuses InvalidInstanceID.Malformed — malformed is decided before existence.
Emulator: refuses InvalidInstanceID.NotFound.
Why: the caller is told the id does not exist when the id can never exist, inverting malformed-vs-absent diagnosis.
Fix: the SendDiagnosticInterrupt handler — validate id syntax before lookup.
Traces: ec2.op-contract-send-diagnostic-interrupt.

Cause 7 of 10: StartNetworkInsightsAnalysis with a bogus path id.
AWS: refuses InvalidParameterValue for this input.
Emulator: refuses InvalidNetworkInsightsPathId.NotFound.
Why: same inversion — a parameter-shape error is reported as a missing resource.
Fix: the StartNetworkInsightsAnalysis handler's path-id validation.
Traces: ec2.op-contract-start-network-insights-analysis.

Cause 8 of 10: DetachNetworkInterface with a well-formed but nonexistent AttachmentId.
AWS: refuses InvalidAttachmentID.NotFound.
Emulator: refuses InvalidNetworkInterfaceAttachmentId.Malformed, calling a well-formed id malformed.
Why: a client cleaning up attachments cannot treat the absent attachment as already-detached.
Fix: the DetachNetworkInterface handler's attachment-id parsing and not-found path.
Traces: ec2.op-contract-detach-network-interface.

Cause 9 of 10: IAM-instance-profile association ids are rejected as invalid.
AWS: DescribeIamInstanceProfileAssociations accepts a well-formed iip-assoc-* id (returning an empty set when it is gone), and DisassociateIamInstanceProfile for an association already removed is refused InvalidAssociationID.NotFound.
Emulator: DescribeIamInstanceProfileAssociations refuses InvalidParameterValue "Invalid Id: iip-assoc-..." for an id it minted itself, and the re-disassociate answers InvalidParameterValue instead of InvalidAssociationID.NotFound.
Why: the whole associate/replace/disassociate lifecycle is unusable — the id the emulator returns is rejected by its own reader.
Fix: iip-assoc id parsing in the IAM-instance-profile association handlers and their not-found classification.
Traces: ec2.op-contract-disassociate-iam-instance-profile, ec2.iam-instance-profile-associate-replace-disassociate.

Cause 10 of 10: CreateTags refuses a network-ACL id.
AWS: CreateTags accepts any taggable EC2 resource id, including acl-*; the request succeeds with no body.
Emulator: refuses InvalidID with "The ID 'acl-...' is not valid" for an acl id the emulator itself created.
Why: tagging the default/created network ACLs is impossible, and any tag-sweeping teardown breaks on it.
Fix: the CreateTags handler's resource-id type table (accept acl-*).
Traces: ec2.ec2-delete-vpc-removes-own-and-default-resource-tags.

## Evidence
Replay output, expected vs actual:
C1: "a launch naming a different instance type must be refused with exactly 'InvalidParameterValue'; RunInstances answered 'ParameterNotFound'".
C2: "expected InvalidInstanceType for a real but non-Mac instance, got 'InvalidParameterValue' (Invalid value 'i-...' for instanceId. The instance is a 't3.micro' instance, which is not an Amazon EC2 Mac instance)".
C3: "a route to an absent NAT gateway must be refused; got 'NatGatewayNotFound'" (expected InvalidNatGatewayID.NotFound).
C4: "attaching a second gateway to vpc-... answered ('Resource.AlreadyAssociated', 'resource igw-... is already attached to network vpc-...'), expected ('InvalidParameterValue', 'Network vpc-... already has an internet gateway attached')".
C5: "expected InvalidParameterValue, got InvalidInstanceID.Malformed".
C6: expected InvalidInstanceID.Malformed, got "InvalidInstanceID.NotFound".
C7: "expected InvalidParameterValue for a bogus path id, got 'InvalidNetworkInsightsPathId.NotFound'".
C8: "expected InvalidAttachmentID.NotFound for a well-formed but nonexistent AttachmentId, got 'InvalidNetworkInterfaceAttachmentId.Malformed'".
C9: "An error occurred (InvalidParameterValue) when calling the DescribeIamInstanceProfileAssociations operation: Invalid Id: iip-assoc-..."; "expected InvalidAssociationID.NotFound re-disassociating iip-assoc-..., got InvalidParameterValue".
C10: "An error occurred (InvalidID) when calling the CreateTags operation: The ID 'acl-...' is not valid" (AWS returns 200).

## Steps to Reproduce
Replay the checks in trace_ids. Minimal reproductions: CreateCapacityReservation(t3.micro) then RunInstances(t3.small, CapacityReservationTarget=<that id>); CreateDelegateMacVolumeOwnershipTask against a t3.micro; CreateRoute(NatGatewayId='nat-0123456789abcdef0'); AttachInternetGateway twice on one VPC; MonitorInstances with an unusable id; SendDiagnosticInterrupt(InstanceId='i-ffffffffffffffffff'); StartNetworkInsightsAnalysis with a bogus nip-; DetachNetworkInterface(AttachmentId='eni-attach-0123456789abcdef0'); AssociateIamInstanceProfile then DescribeIamInstanceProfileAssociations(AssociationIds=[<returned id>]) and DisassociateIamInstanceProfile twice; CreateTags(Resources=['acl-<existing>'], Tags=[...]).

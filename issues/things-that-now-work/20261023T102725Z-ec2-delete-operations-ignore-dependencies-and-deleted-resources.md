Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-carrier-gateway-vpc-attachment
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-carrier-gateway-vpc-attachment ec2-conflict-security-group-dependency ec2-conflict-subnet-eni-dependency ec2-ebs-volume-delete-and-snapshot-not-found ec2-elastic-ip-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteVpc|4xx->2xx|$.Response
Issue-Key: ec2|DeleteVpc|4xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteVpc|4xx->2xx|status
Issue-Key: ec2|DeleteVpc|2xx->4xx|status
Issue-Key: ec2|DeleteSecurityGroup|4xx->2xx|$.Response
Issue-Key: ec2|DeleteSecurityGroup|4xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteSecurityGroup|4xx->2xx|status
Issue-Key: ec2|DeleteSecurityGroup|2xx->4xx|status
Issue-Key: ec2|DeleteSubnet|4xx->2xx|$.Response
Issue-Key: ec2|DeleteSubnet|4xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteSubnet|4xx->2xx|status
Issue-Key: ec2|DeleteSubnet|2xx->4xx|status
Issue-Key: ec2|DescribeVolumes|4xx->2xx|$
Issue-Key: ec2|DescribeVolumes|4xx->2xx|$.Response
Issue-Key: ec2|DescribeVolumes|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeVolumes|4xx->2xx|status
Issue-Key: ec2|DescribeAddresses|4xx->2xx|$.Response
Issue-Key: ec2|DescribeAddresses|4xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeAddresses|4xx->2xx|status
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 delete operations ignore dependencies and deleted resources stay readable

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpc

## Description
Eight independent defects in EC2's delete path and in the state that backs it. All eight must be closed before submitting. Background for all of them: EC2 models no error shapes (Smithy com-amazonaws-ec2-amazonec2: DeleteVpc/CreateVolume/DescribeAvailabilityZones all report errors: []), so the ec2Query HTTP status plus the error Code on the wire is the entire contract a client can branch on.

Cause 1 of 8: DeleteVpc on a VPC that still has an attached resource succeeds.
AWS: refuses the delete with HTTP 400 and Code DependencyViolation while any dependent object (here a carrier gateway) is still attached; recorded message example "The vpc 'vpc-...' has dependencies and cannot be deleted." (recorded behavior only for the message text; the status and code are the requirement).
Emulator: HTTP 200 and no Response/Errors element at all - the VPC is destroyed.
Why: a client that deletes bottom-up by reacting to DependencyViolation never sees it, and the VPC disappears while its dependents still reference it, so state is left inconsistent.
Fix: dependency checking in the DeleteVpc handler before it mutates EC2 state.
Traces: ec2-carrier-gateway-vpc-attachment#task#5.

Cause 2 of 8: the same fixture's teardown DeleteVpc then fails.
AWS: HTTP 200 once the dependent object has been removed.
Emulator: HTTP 400 - the VPC was already destroyed by the step in cause 1, so the legitimate delete has nothing to delete.
Why: a teardown that AWS completes cleanly errors out, so scripted lifecycles cannot finish.
Fix: same handler/state as cause 1; this step passes once the premature delete is refused.
Traces: ec2-carrier-gateway-vpc-attachment#teardown#8.

Cause 3 of 8: DeleteSecurityGroup on a group still referenced by a dependent object succeeds.
AWS: HTTP 400 with Code DependencyViolation (recorded message example 'resource sg-... has a dependent object').
Emulator: HTTP 200 with no error body - the group is deleted.
Why: same misbranching as cause 1 on the security-group surface, and ENIs are left pointing at a group that no longer exists.
Fix: dependency checking in the DeleteSecurityGroup handler.
Traces: ec2-conflict-security-group-dependency#task#5.

Cause 4 of 8: the matching teardown DeleteSecurityGroup then fails.
AWS: HTTP 200.
Emulator: HTTP 400, because cause 3 already removed the group.
Fix: same handler/state as cause 3.
Traces: ec2-conflict-security-group-dependency#teardown#9.

Cause 5 of 8: DeleteSubnet on a subnet that still holds a network interface succeeds.
AWS: HTTP 400 with Code DependencyViolation (recorded message example "The subnet 'subnet-...' has dependencies and cannot be deleted.").
Emulator: HTTP 200 with no error body.
Why: same as cause 1 for subnets; ENIs survive their subnet.
Fix: dependency checking in the DeleteSubnet handler.
Traces: ec2-conflict-subnet-eni-dependency#task#5.

Cause 6 of 8: the matching teardown DeleteSubnet then fails.
AWS: HTTP 200.
Emulator: HTTP 400, because cause 5 already removed the subnet.
Fix: same handler/state as cause 5.
Traces: ec2-conflict-subnet-eni-dependency#teardown#7.

Cause 7 of 8: a deleted EBS volume stays visible to DescribeVolumes forever.
AWS: after DeleteVolume, DescribeVolumes on that id answers HTTP 400 with Code InvalidVolume.NotFound.
Emulator: HTTP 200 with the full volumeSet item for the deleted volume (status 'available'); the replay poller gave up after 31 attempts / 60s with no error code ever appearing.
Why: every delete-then-wait-for-not-found waiter hangs until its timeout, and a client cannot tell a live volume from a deleted one.
Fix: deletion must remove the volume from EC2 state (DeleteVolume handler / volume state), and DescribeVolumes must raise InvalidVolume.NotFound for unknown ids.
Traces: ec2-ebs-volume-delete-and-snapshot-not-found#task#4.

Cause 8 of 8: a released Elastic IP allocation stays visible to DescribeAddresses.
AWS: after ReleaseAddress, DescribeAddresses on the allocation id answers HTTP 400 with Code InvalidAllocationID.NotFound.
Emulator: HTTP 200 with no Response/Errors element.
Why: same as cause 7 for Elastic IPs - release appears to have no effect from the read side.
Fix: release must remove the allocation from EC2 address state, and DescribeAddresses must raise InvalidAllocationID.NotFound.
Traces: ec2-elastic-ip-lifecycle#teardown#10. Note: ReleaseAddress is itself an unimplemented stub (services/service-com-amazonaws-ec2-amazonec2/src/ops/release_address.rs) already covered by its own filed issue; this cause is about the read side after release.

## Evidence
Cause 1 - expected 400 / Response.Errors.Error.Code=DependencyViolation, got 200 and $.Response missing. Cause 2 - expected 200, got 400. Cause 3 - expected 400 / DependencyViolation, got 200 and $.Response missing. Cause 4 - expected 200, got 400. Cause 5 - expected 400 / DependencyViolation, got 200 and $.Response missing. Cause 6 - expected 200, got 400. Cause 7 - expected 400 / InvalidVolume.NotFound, got 200 with the volume still present; 'poll: error code None not in [InvalidVolume.NotFound] after 31 attempt(s)', poll capped at 60s. Cause 8 - expected 400 / InvalidAllocationID.NotFound, got 200 with $.Response missing.

## Steps to Reproduce
Replay the fixture steps named under Traces. Minimal repros: (1) ec2:CreateVpc, attach a carrier gateway, then ec2:DeleteVpc on that VPC. (3) ec2:CreateSecurityGroup, attach an ENI that references it, then ec2:DeleteSecurityGroup. (5) ec2:CreateSubnet, ec2:CreateNetworkInterface in it, then ec2:DeleteSubnet. (7) ec2:CreateVolume, ec2:DeleteVolume, then ec2:DescribeVolumes VolumeIds=[that id]. (8) ec2:AllocateAddress, ec2:ReleaseAddress, then ec2:DescribeAddresses AllocationIds=[that id].

Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dvpc-traj-vpc-id-is-region-scoped
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dvpc-traj-vpc-id-is-region-scoped ec2-xrg-traj-instance-id-is-region-scoped ec2-ibp-traj-launch-template-from-failed-ami-poll
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|4xx->2xx|$.Response
Issue-Key: ec2|DescribeVpcs|4xx->2xx|status
Issue-Key: ec2|DescribeInstances|4xx->2xx|$.Response
Issue-Key: ec2|DescribeInstances|4xx->2xx|status
Issue-Key: ec2|StopInstances|4xx->2xx|$.Response
Issue-Key: ec2|StopInstances|4xx->2xx|status
Issue-Key: ec2|DescribeLaunchTemplates|4xx->2xx|$.Response
Issue-Key: ec2|DescribeLaunchTemplates|4xx->2xx|status
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 reads are not region-scoped and a deleted launch template stays readable

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
Resource ids are resolved out of one global store with no region (and, for launch templates, no deletion) predicate, so four calls that AWS refuses with a NotFound error answer HTTP 200 with the resource present. An absent error is the most damaging class of divergence: client code that uses a describe as an existence probe concludes the resource exists in a region where it does not.

Cause 1 of 4: DescribeVpcs with a VPC id created in another region.
AWS: recorded behavior only — the id does not resolve in the signed region, so 400 Code 'InvalidVpcID.NotFound', Message "The vpc ID 'vpc-00000000000000024' does not exist" (fixed text 'The vpc ID ' + the quoted request id + ' does not exist').
Emulator: 200 with the VPC in vpcSet.
Why: a caller that probes a VPC in the wrong region is told it is there, so cross-region mistakes are never surfaced and any subsequent create in that region is built on a resource that is not local.
Fix: region scoping in the VPC lookup used by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpcs.rs (the state store must key resources by region and the handler must refuse ids from other regions).
Traces: ec2-dvpc-traj-vpc-id-is-region-scoped#task#2 (cluster also covers ec2-xrg-traj-instance-id-is-region-scoped#task#7).

Cause 2 of 4: DescribeInstances with an instance id from another region.
AWS: recorded behavior only — 400 Code 'InvalidInstanceID.NotFound', Message "The instance ID 'i-00000000000000001' does not exist".
Emulator: 200 with the reservation present.
Why/Fix: as cause 1, in describe_instances.rs / the instance state store.
Traces: ec2-xrg-traj-instance-id-is-region-scoped#task#4.

Cause 3 of 4: StopInstances against an instance id from another region.
AWS: recorded behavior only — 400 Code 'InvalidInstanceID.NotFound', Message "The instance ID 'i-00000000000000001' does not exist".
Emulator: 200 — the mutation is accepted and applied to a resource that does not exist in the signed region.
Why: this is a cross-region WRITE: an operator acting on the wrong region stops an instance AWS would have protected by refusing.
Fix: region scoping in stop_instances.rs before the state transition.
Traces: ec2-xrg-traj-instance-id-is-region-scoped#task#6.

Cause 4 of 4: DescribeLaunchTemplates naming a deleted template.
AWS: recorded behavior only — 400 Code 'InvalidLaunchTemplateName.NotFoundException', Message 'At least one of the launch templates specified in the request does not exist.' (fixed text, no variable part).
Emulator: 200 with the deleted template still described.
Why: teardown verification passes against a template that should be gone, and recreation logic keyed on the NotFound refusal never runs.
Fix: deletion predicate in the launch-template lookup behind describe_launch_templates.rs.
Traces: ec2-ibp-traj-launch-template-from-failed-ami-poll#teardown#12.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-dvpc-traj-vpc-id-is-region-scoped task#2 — expected status 400 with Response.Errors.Error.Code 'InvalidVpcID.NotFound' and Message "The vpc ID 'vpc-00000000000000024' does not exist"; actual status 200 and no Response error element. Same 400→200 inversion for DescribeInstances and StopInstances with a foreign-region instance id (expected InvalidInstanceID.NotFound) and for DescribeLaunchTemplates after delete (expected InvalidLaunchTemplateName.NotFoundException).

## Steps to Reproduce
1. Create a VPC in us-west-2, then call DescribeVpcs for that id against another region's endpoint (fixture ec2-dvpc-traj-vpc-id-is-region-scoped, step task#2): expect 400 InvalidVpcID.NotFound, observe 200.
2. Repeat with DescribeInstances and StopInstances using ec2-xrg-traj-instance-id-is-region-scoped task#4 and task#6.
3. Delete a launch template, then DescribeLaunchTemplates by name (ec2-ibp-traj-launch-template-from-failed-ami-poll, teardown#12): expect 400 InvalidLaunchTemplateName.NotFoundException, observe 200.

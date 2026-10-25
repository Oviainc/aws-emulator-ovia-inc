Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-security-group-dependency-rejected ec2.asgeni-launch-template-ssm-alias-resolves-to-an-amazon-ami
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-delete-security-group-dependency-rejected|_client_error#18|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: ec2.asgeni-launch-template-ssm-alias-resolves-to-an-amazon-ami|run#6|AssertionError|assert resolved.startswith("ami-") and resolved != _AL2023, (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# EC2: DeleteSecurityGroup ignores an attached ENI (no DependencyViolation) and DescribeLaunchTemplateVersions ResolveAlias=true never resolves a resolve:ssm ImageId (2 causes)

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecurityGroup

## Description
Two net-new root causes in the ec2 service. Both must be closed before submitting.

=== Cause 1 of 2: DeleteSecurityGroup deletes a security group that is still attached to a network interface ===
AWS: Smithy (com.amazonaws.ec2#DeleteSecurityGroup) binds no per-operation errors — the refusal is one of the ec2Query common client errors — and the operation's output is {Return, GroupId}. Recorded behavior: deleting a security group that is still referenced by a network interface is refused with HTTP 400 and error code DependencyViolation (observed message, an example only: "resource sg-... has a dependent object"), and the rejected delete has NO effect — DescribeSecurityGroups still returns the group. Once the ENI is deleted the same DeleteSecurityGroup succeeds and the group then really is gone (DescribeSecurityGroups answers InvalidGroup.NotFound). The dependency guard, not the message, is the contract.
Emulator: the delete of the ENI-attached group SUCCEEDS — HTTP 200 with {'Return': True, 'GroupId': 'sg-...'} — so the group is destroyed while a dependent network interface still references it.
Why: a client relying on AWS's behavior uses DependencyViolation to discover teardown ordering (delete ENIs/instances before their groups) and to protect an in-use group from a mistaken delete. Against the emulator a wrong teardown order silently "succeeds", so ordering defects never surface, and the surviving ENI is left pointing at a group that no longer exists — a state AWS cannot produce.
Fix: the delete_security_group handler — before removing the group, scan the network-interface state for references to it and return DependencyViolation when any exist, leaving the group in place.
Related open issue (does NOT cover this): 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md owns check ec2.security-group-delete-while-in-use-fails, which is a refusal-message-template divergence; here the refusal does not happen at all.
Traces: ec2.op-contract-delete-security-group-dependency-rejected

=== Cause 2 of 2: DescribeLaunchTemplateVersions with ResolveAlias=true returns the resolve:ssm alias instead of an AMI id ===
AWS: Smithy (DescribeLaunchTemplateVersionsRequest.ResolveAlias) documents it exactly: "If true, and if a Systems Manager parameter is specified for ImageId, the AMI ID is displayed in the response for imageId. If false, and if a Systems Manager parameter is specified for ImageId, the parameter is displayed in the response for imageId. Default: false". Recorded behavior: a launch template created with ImageId = resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 stores that alias verbatim (ResolveAlias absent/false returns the alias), and the SAME call with ResolveAlias=true returns an ami- id that describes as an available x86_64 Amazon-owned image. Stored form and resolved form are two different answers to one call shape.
Emulator: ResolveAlias=true returns the alias string unchanged — ImageId 'resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64' — so the resolved form is indistinguishable from the stored form.
Why: a client relying on AWS's behavior uses ResolveAlias=true to learn which concrete AMI a launch template (and therefore an ASG or a RunInstances from that template) will actually use. It receives a string that is not an AMI id, so every downstream DescribeImages/RunInstances/consistency check built on it fails or compares the wrong value.
Fix: the describe_launch_template_versions handler — honour the ResolveAlias input by resolving a resolve:ssm: ImageId through the SSM public-parameter catalog to the AMI id it names, leaving the stored template data untouched.
Related open issue (does NOT cover this): 20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md owns ec2.asgeni-launch-template-with-unknown-ssm-alias-rejected (rejection of an UNKNOWN alias), not the resolution of a valid one.
Traces: ec2.asgeni-launch-template-ssm-alias-resolves-to-an-amazon-ami

## Evidence
Cause 1 — check ec2.op-contract-delete-security-group-dependency-rejected, verdict mismatch.
Expected (recorded AWS): ec2.delete_security_group(GroupId=<sg attached to an ENI>) raises ClientError with Error.Code == "DependencyViolation" (HTTP 400) and the group still describes afterwards.
Actual (emulator): AssertionError: expected a ClientError, but the call succeeded and returned {'Return': True, 'GroupId': 'sg-0000000000000002f', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Divergence: expected HTTP 400 / DependencyViolation, got HTTP 200 success.

Cause 2 — check ec2.asgeni-launch-template-ssm-alias-resolves-to-an-amazon-ami, verdict mismatch.
Expected (model + recorded AWS): describe_launch_template_versions(..., ResolveAlias=True)["LaunchTemplateVersions"][0]["LaunchTemplateData"]["ImageId"] starts with "ami-" and differs from the stored alias.
Actual (emulator): AssertionError: ResolveAlias=True returned ImageId 'resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64', expected an ami- id
Divergence: HTTP 200 in both cases, but the resolved ImageId is the unresolved alias.

## Steps to Reproduce
Cause 1:
1. ec2.create_vpc(CidrBlock="10.94.0.0/16"); ec2.create_subnet(VpcId=..., CidrBlock="10.94.1.0/24").
2. ec2.create_security_group(GroupName=<unique>, Description="attached to eni", VpcId=...).
3. ec2.create_network_interface(SubnetId=..., Groups=[<that group>]).
4. ec2.delete_security_group(GroupId=<that group>) -> emulator answers 200 Return=true; AWS answers 400 DependencyViolation and keeps the group.

Cause 2:
1. ec2.create_launch_template with LaunchTemplateData {"ImageId": "resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64", "InstanceType": "t3.micro"}.
2. ec2.describe_launch_template_versions(LaunchTemplateId=..., Versions=["$Latest"]) -> ImageId is the alias (correct).
3. ec2.describe_launch_template_versions(LaunchTemplateId=..., Versions=["$Latest"], ResolveAlias=True) -> emulator still returns the alias; AWS returns an ami- id.

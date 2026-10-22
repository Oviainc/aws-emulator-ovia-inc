Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.multiattach-flag-on-a-gp3-volume-is-refused-while-io2-accepts-the-same-request ec2.op-contract-describe-images ec2.op-contract-terminate-instances ec2.op-vpc-cidr-out-of-range-rejected ec2.create-mac-sip-modification-task-rejected-unknown-instance ec2.op-contract-modify-reserved-instances-required-params-rejected ec2.op-contract-describe-volumes ec2.asgeni-launch-template-with-unknown-ssm-alias-rejected ec2.create-default-subnet-contract ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.multiattach-flag-on-a-gp3-volume-is-refused-while-io2-accepts-the-same-request|run#7|AssertionError|assert code == ERR_BAD_COMBINATION, (
Issue-Key: ec2.op-contract-describe-images|run#13|AssertionError|assert code == "InvalidAMIID.Malformed", f"expected InvalidAMIID.Malformed, got {code}"
Issue-Key: ec2.op-contract-terminate-instances|run#9|AssertionError|assert code == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.op-vpc-cidr-out-of-range-rejected|run#6|AssertionError|assert err.get("Code") == "InvalidVpc.Range", (
Issue-Key: ec2.create-mac-sip-modification-task-rejected-unknown-instance|run#3|AssertionError|assert code_all_zero == "InvalidInstanceID.Malformed", (
Issue-Key: ec2.op-contract-modify-reserved-instances-required-params-rejected|run#4|AssertionError|assert code == "MissingParameter", f"expected MissingParameter, got {code}"
Issue-Key: ec2|DescribeVolumes|InvalidVolume.NotFound
Issue-Key: ec2.asgeni-launch-template-with-unknown-ssm-alias-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2.create-default-subnet-contract|run#6|AssertionError|assert len(existing) == 1, (
Issue-Key: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently|run#20|AssertionError|assert len(custom_subnets) >= 4, f"expected >=4 subnets in {vpc_id}, got {len(custom_subnets)}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# EC2 misclassifies rejections and skips validation: malformed ids answered as NotFound, wrong parameter-error codes, an unresolvable AMI alias accepted, and no default subnets in the account baseline (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Ten independent fidelity defects in EC2 operations that DO answer (no stub involved). All ten must be closed before submitting. Background for all of them: EC2 models no error shapes (Smithy com-amazonaws-ec2-amazonec2: every operation queried reports errors: []), so the ec2Query error CODE on the wire is the entire contract a client can branch on — a wrong code is a wrong response, not a cosmetic difference.

Cause 1 of 10: CreateVolume with VolumeType=gp3 and MultiAttachEnabled=true is refused with the wrong code.
AWS: InvalidParameterCombination (multi-attach is an io1/io2-only combination). Smithy: CreateVolume input carries VolumeType (enum) and MultiAttachEnabled (boolean).
Emulator: InvalidParameterValue with message 'multiAttachEnabled is supported with io1 and io2 volumes only, not with a gp3 volume'.
Why: clients that branch on InvalidParameterCombination to retry with io2 never see it.
Fix: validation in the CreateVolume handler.
Trace: ec2.multiattach-flag-on-a-gp3-volume-is-refused-while-io2-accepts-the-same-request.

Cause 2 of 10: DescribeImages answers a MALFORMED AMI id with InvalidAMIID.NotFound.
AWS: InvalidAMIID.Malformed — id syntax is validated before lookup.
Emulator: InvalidAMIID.NotFound.
Why: a client cannot tell a typo from a missing image, so retry/repair logic takes the wrong branch.
Fix: id parsing/validation before lookup in the DescribeImages handler.
Trace: ec2.op-contract-describe-images.

Cause 3 of 10: TerminateInstances answers a checksum-broken instance id with InvalidInstanceID.NotFound.
AWS: InvalidInstanceID.Malformed. Smithy: TerminateInstances.InstanceIds is required (InstanceIdStringList).
Emulator: InvalidInstanceID.NotFound.
Why: same misbranching as cause 2 on a destructive operation.
Fix: instance-id validation in the TerminateInstances handler.
Trace: ec2.op-contract-terminate-instances.

Cause 4 of 10: CreateVpc with an out-of-range CIDR is refused with the wrong code.
AWS: InvalidVpc.Range for 10.0.0.0/8 (outside the permitted /16-/28 VPC range).
Emulator: InvalidParameterValue.
Why: a client that distinguishes a range violation from a syntax error cannot.
Fix: CIDR range validation in the CreateVpc handler.
Trace: ec2.op-vpc-cidr-out-of-range-rejected.

Cause 5 of 10: CreateMacSystemIntegrityProtectionModificationTask answers an all-zero instance id with a code EC2 does not use.
AWS: InvalidInstanceID.Malformed.
Emulator: ValidationError.
Why: ValidationError is not part of EC2's ec2Query vocabulary for this surface, so no client error handler matches it.
Fix: instance-id validation in that operation's handler.
Trace: ec2.create-mac-sip-modification-task-rejected-unknown-instance.

Cause 6 of 10: ModifyReservedInstances with required parameters omitted is refused with the wrong code.
AWS: MissingParameter.
Emulator: MalformedInput.
Why: a client cannot tell 'you forgot a parameter' from 'your request body is corrupt'.
Fix: required-member validation in the ModifyReservedInstances handler.
Trace: ec2.op-contract-modify-reserved-instances-required-params-rejected.

Cause 7 of 10: DescribeVolumes classifies the check's probe as InvalidVolume.NotFound where AWS answers InvalidParameterValue.
AWS: InvalidParameterValue for this request form (recorded behavior only).
Emulator: InvalidVolume.NotFound.
Why: parameter-level rejections are reported as resource-level ones, so a client retries a lookup that can never succeed.
Fix: request validation ordering in the DescribeVolumes handler.
Trace: ec2.op-contract-describe-volumes.

Cause 8 of 10: CreateLaunchTemplate accepts an ImageId SSM alias that cannot resolve.
AWS: refuses the request (recorded behavior only for the exact code); the alias must resolve to an AMI at create time.
Emulator: the call succeeds, so no error is produced at all.
Why: the failure surfaces much later, at launch, instead of at template creation where the client can fix it.
Fix: resolve:ssm alias resolution/validation in the CreateLaunchTemplate handler.
Trace: ec2.asgeni-launch-template-with-unknown-ssm-alias-rejected.

Cause 9 of 10: the account baseline holds no default subnet.
AWS: a default VPC has exactly one default subnet per Availability Zone, discoverable by DescribeSubnets with the default-for-az filter; the check expects exactly one in us-west-2a.
Emulator: DescribeSubnets returns [] — no default subnet exists anywhere in the baseline.
Why: every tutorial-shaped client that launches into the default VPC/subnet without creating network state cannot run.
Fix: default-VPC/default-subnet seeding in EC2 state (and the CreateDefaultSubnet path).
Trace: ec2.create-default-subnet-contract.

Cause 10 of 10: a CDK/CloudFormation-deployed VPC reports zero subnets.
AWS: the deployed stack's VPC carries the 4 subnets the template declares, visible via DescribeSubnets filtered by vpc-id.
Emulator: 'expected >=4 subnets in ...Vpc8378EB38..., got 0' — the VPC exists but holds no subnet.
Why: public-vs-isolated routing behaviour cannot be observed, and any IaC-deployed network is empty from EC2's point of view.
Fix: subnet provisioning/state linkage for template-created subnets. Related open issues: 20261022T192234Z-ec2-createsubnet-stub.md and 20261022T193451Z-ec2-createsubnet-stub.md name the CreateSubnet stub; neither carries this check.
Trace: ec2.deploy-cdk-vpc-public-and-isolated-subnets-route-differently.

## Evidence
Cause 1 — AssertionError: CreateVolume(VolumeType='gp3', MultiAttachEnabled=True) in us-west-2a must be refused with exactly 'InvalidParameterCombination', got 'InvalidParameterValue': 'multiAttachEnabled is supported with io1 and io2 volumes only, not with a gp3 volume'.
Cause 2 — AssertionError: expected InvalidAMIID.Malformed, got InvalidAMIID.NotFound.
Cause 3 — AssertionError: expected InvalidInstanceID.Malformed for a checksum-broken instance id, got InvalidInstanceID.NotFound.
Cause 4 — AssertionError: expected InvalidVpc.Range for VPC CIDR 10.0.0.0/8, got 'InvalidParameterValue'.
Cause 5 — AssertionError: expected InvalidInstanceID.Malformed for the all-zero placeholder, got 'ValidationError'.
Cause 6 — AssertionError: expected MissingParameter, got MalformedInput.
Cause 7 — AssertionError: expected a volume-not-found-shaped rejection, got InvalidVolume.NotFound (assert code == 'InvalidParameterValue').
Cause 8 — AssertionError: the call succeeded; real AWS rejects it, so there is no error.
Cause 9 — AssertionError: the account baseline holds exactly one default subnet in 'us-west-2a', found [].
Cause 10 — AssertionError: expected >=4 subnets in sim2real-behav-cdkvpc-3d61d3e36d53-Vpc8378EB38-C349A3289518, got 0.

## Steps to Reproduce
Replay the check ids in trace_ids. Minimal repros: (1) ec2:CreateVolume AvailabilityZone=us-west-2a Size=4 VolumeType=gp3 MultiAttachEnabled=True. (2) ec2:DescribeImages ImageIds=['ami-zzzz']. (3) ec2:TerminateInstances with a checksum-broken i-... id. (4) ec2:CreateVpc CidrBlock=10.0.0.0/8. (5) ec2:CreateMacSystemIntegrityProtectionModificationTask InstanceId='i-00000000000000000'. (6) ec2:ModifyReservedInstances with no ReservedInstancesIds. (7) ec2:DescribeVolumes with the check's probe id. (8) ec2:CreateLaunchTemplate with ImageId='resolve:ssm:/does/not/exist'. (9) ec2:DescribeSubnets Filters=[{'Name':'default-for-az','Values':['true']}]. (10) deploy the CDK VPC app and ec2:DescribeSubnets filtered by its vpc-id.

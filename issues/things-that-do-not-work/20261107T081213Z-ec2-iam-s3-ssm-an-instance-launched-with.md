Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-put-object-outside-role-policy-denied ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-withdrawn-after-disassociate-iam-instance-profile
Issue-Key: ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-put-object-outside-role-policy-denied|_poll#16|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-withdrawn-after-disassociate-iam-instance-profile|_poll#14|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# ec2__iam__s3__ssm: an instance launched with an instance profile never becomes an Online SSM managed node, so the IMDS-credentials scenarios never reach a verdict

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
One net-new root cause grounded in failing checks the suite lists under '### ec2__iam__s3__ssm'. 1 CAUSE.

Cause 1 of 1: an EC2 instance launched with an IAM instance profile (AmazonSSMManagedInstanceCore plus an S3 policy) never registers as a Systems Manager managed node, so the only observation point these scenarios have — Run Command inside the guest — never opens and both checks are killed mid-poll.
AWS: such an instance registers with Systems Manager shortly after boot and DescribeInstanceInformation reports one entry for it with PingStatus 'Online' (Smithy, services/service-com-amazonaws-ssm-amazonssm: InstanceInformation$PingStatus -> enum PingStatus Online | ConnectionLost | Inactive, plus LastPingDateTime, AgentVersion, IamRole). Only then can SendCommand/GetCommandInvocation run in the guest, which is how these checks read IMDS role credentials and use them to call S3 — observing that a PutObject outside the role policy is denied (AccessDenied) and that the credentials stop being served once DisassociateIamInstanceProfile removes the profile.
Emulator: neither check reaches an assertion. Both die in their agent-online wait with 'TimeoutError: the SSM agent on i-00000000000000001 coming Online: the run budget ended first; not graded', raised from `_poll(ctx, _agent, lambda i: i is not None and i.get("PingStatus") == "Online", 600, 2, ...)`. Hypothesis: nothing in the emulator registers an EC2-launched instance as an SSM managed node, so DescribeInstanceInformation never reports it.
Why: the failure mode is a hang rather than a refusal — a client waiting for PingStatus=='Online' has no error code to branch on and no service-side timeout — and the whole instance-credentials story (IMDS role credentials, their policy boundary, their withdrawal when the profile is disassociated) cannot be exercised. The open issue 20261027T034339Z-ec2-ssm-a-launched-ec2-instance-never-becomes.md diagnoses the same gate for ec2__ssm checks but carries neither of these check ids.
Fix: SSM managed-node registration in the ssm crate's state (populate the managed-node inventory for an instance whose profile can reach Systems Manager and project it from describe_instance_information with PingStatus Online/LastPingDateTime/IamRole), plus in-guest execution for SendCommand/GetCommandInvocation and IMDS role-credential serving tied to the associated instance profile.
Traces: ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-put-object-outside-role-policy-denied, ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-withdrawn-after-disassociate-iam-instance-profile.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; both checks are listed under '### ec2__iam__s3__ssm'.

ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-put-object-outside-role-policy-denied: 'File check_ec2__iam__s3__ssm_xsvc_ec2_3_imds_credentials_put_object_outside_role_policy_denied.py, line 321, in _await_agent -> _poll(ctx, _agent, lambda i: i is not None and i.get("PingStatus") == "Online", 600, 2, ...) -> TimeoutError: the SSM agent on i-00000000000000001 coming Online: the run budget ended first; not graded'.
ec2__iam__s3__ssm.xsvc-ec2-3-imds-credentials-withdrawn-after-disassociate-iam-instance-profile: the same TimeoutError from its own _await_agent at line 292.
Expected (AWS): DescribeInstanceInformation lists the launched instance with PingStatus 'Online' within registration latency, after which SendCommand returns the guest's output. Actual (emulator): InstanceInformationList never contains the instance and the poll runs to the caller's deadline. (The instance id is this run's value.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids.
3. iam CreateRole trusted by ec2.amazonaws.com, AttachRolePolicy arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore plus an inline S3 policy, CreateInstanceProfile/AddRoleToInstanceProfile; ec2 RunInstances with that profile in a subnet; wait for 'running'; poll ssm DescribeInstanceInformation(Filters=[{'Key':'InstanceIds','Values':[<id>]}]). Expected: one entry with PingStatus 'Online', then SendCommand('AWS-RunShellScript') reading IMDS credentials and calling S3 with them. Actual: the list stays empty and the agent-online poll never converges.

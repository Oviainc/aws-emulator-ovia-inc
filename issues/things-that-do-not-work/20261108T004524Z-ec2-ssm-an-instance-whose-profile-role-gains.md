Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached
Issue-Key: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached|_info#15|Timeout|return rows[0] if rows else None
Issue-Commit: cae7c7114b6b8b21c16a9367bb3638c681e12be0

# ec2__ssm: an instance whose profile role gains AmazonSSMManagedInstanceCore never reports PingStatus Online in DescribeInstanceInformation, so the inventory-omission check never reaches a verdict

> _Note: this issue was identified against the code base at commit `cae7c7114b6b8b21c16a9367bb3638c681e12be0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: a running EC2 instance whose instance-profile role carries (or is given) arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore never shows up as an Online managed node, so the check's registration poll never settles and the check is killed by the harness budget with no verdict. Failing check: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached.

AWS: a running instance reachable from the SSM endpoints whose instance profile grants the SSM core permissions registers with Systems Manager, and DescribeInstanceInformation (filtered Key=InstanceIds, Values=[<id>]) returns exactly one InstanceInformation row for it with PingStatus='Online'; a host whose role lacks those permissions is absent from that list, and it appears once the policy is attached. Smithy: SSM DescribeInstanceInformation -> InstanceInformationList[].PingStatus (PingStatusType: Online | ConnectionLost | Inactive) and InstanceId; the InstanceInformationStringFilter key 'InstanceIds' is the server-side filter the check uses. Registration latency is not modelled — recorded behavior only: the recording has the host Online well inside the check's 600 s deadline.

Emulator: the poll for PingStatus=='Online' never succeeds; the whole check is terminated with "Error: Check timed out after 180.0s" (the harness budget), i.e. no InstanceInformation row (or no Online row) is ever served for the instance. Hypothesis: the emulator never links a RunInstances instance that carries an IamInstanceProfile to SSM's managed-node registry, so DescribeInstanceInformation stays empty for it regardless of the attached policy. Sibling checks of the same family are already filed under 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md and 20261103T200742Z-ec2-ssm-cloudformation-deployed-ec2-ssm-estates-are-unusable-instance.md, and the CloudFormation-deployed variant under 20261107T133755Z-cloudformation-ec2-ssm-an-ec2-instance-launched-with.md / 20261107T201046Z-cloudformation-ec2-ssm-an-ec2-instance-launched-with-355dae.md; none of them carries this check's replay key (the suite did not mark it owned), and this is the plain RunInstances + IAM-attach path, not a CloudFormation or Windows one.

Why: a client cannot observe the central Systems Manager invariant that SSM reachability follows the instance profile's permissions — the negative half (host absent while the role lacks the core policy) is indistinguishable from the emulator simply never registering any host, and the positive half (host appears after AttachRolePolicy) never happens, so SendCommand / inventory / patch flows keyed on DescribeInstanceInformation are unusable and callers that wait for Online hang instead of proceeding.

Fix: the SSM managed-node registration path and the DescribeInstanceInformation handler/state — an instance in state 'running' with an instance profile whose role grants the SSM core actions must be projected into the InstanceInformation store with PingStatus 'Online' (and withheld while the role lacks them), with the IAM role/policy attachment re-evaluated so the transition is observable.

Traces: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached. Note: the failure surfaces as the harness's 180 s budget expiring inside _wait_online()'s poll (the check itself allows 600 s for registration), so the replay shows no response body — the divergence is the absent/never-Online InstanceInformation row.

## Evidence
Expected (AWS): ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[<instance-id>]}])['InstanceInformationList'] is empty while the instance profile's role lacks AmazonSSMManagedInstanceCore, and after iam.attach_role_policy(...AmazonSSMManagedInstanceCore) it returns one row for that InstanceId with PingStatus 'Online'.

Actual (emulator): the condition 'the SSM agent on <instance-id> coming Online' is never met; the check is terminated by the harness:
  Error: Check timed out after 180.0s
No Online InstanceInformation row is ever served for the launched instance.

## Steps to Reproduce
1. Replay the behavioral suite and re-run ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached.
2. Or manually: create a role trusted by ec2.amazonaws.com with NO SSM policy, put it in an instance profile, RunInstances a t3.micro into a public subnet with that profile.
3. Poll ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[<id>]}]) — it is (correctly) empty.
4. iam.attach_role_policy(RoleName=<role>, PolicyArn=arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore) and keep polling: AWS serves one row with PingStatus 'Online'; the emulator serves none, so the poll runs until the check's budget expires.

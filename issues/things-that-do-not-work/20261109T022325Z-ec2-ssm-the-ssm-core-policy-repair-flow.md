Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached
Issue-Key: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached|_poll#15|Timeout|last = probe()
Issue-Commit: 28a8a61701b6d4c7fe3b061a21d38872ad20994f

# ec2__ssm: the SSM-core policy repair flow never converges — no host reports PingStatus Online, so the negative-then-repair inventory check reaches no verdict

> _Note: this issue was identified against the code base at commit `28a8a61701b6d4c7fe3b061a21d38872ad20994f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
1 root cause, grounded in 1 failing check.

Cause 1 of 1: neither the fully privileged host nor the repaired host ever becomes a Systems Manager managed node, so the check can prove neither the absence it asserts nor the repair that follows it.

AWS: with Default Host Management Configuration off, an EC2 instance whose instance-profile role carries AmazonSSMManagedInstanceCore registers with Systems Manager and appears in DescribeInstanceInformation (Smithy: DescribeInstanceInformation -> InstanceInformationList, MaxResults range 5-50) with PingStatus 'Online'; a sibling instance whose profile role lacks that policy never appears at all, and attaching the policy to that role and rebooting the host makes the same InstanceId appear Online within minutes. The check needs the privileged sibling Online first (600 s cap) in order to establish a 60 s absence window for the under-privileged twin and then a 900 s repair deadline.

Emulator: the check reports 'Check timed out after 180.0s' and reaches no assertion. The only wait that can consume the budget before the first comparison is the poll for the privileged sibling's PingStatus Online, so on the evidence no instance ever becomes a managed node — the absence the check is designed to observe is indistinguishable from the emulator never registering anything, and the repair half never runs. Hypothesis: SSM managed-node registration is not driven from an instance's profile at all, so the policy is neither honoured nor enforced. Open issues 20261108T004524Z-ec2-ssm-an-instance-whose-profile-role-gains.md and 20261107T225143Z-ec2-ssm-2-net-new-causes-a-launch.md describe the same registration gap but do not carry this check's replay key.

Why: a client cannot tell a correctly refused host from a broken emulator. The IAM-driven gate that decides which hosts are manageable is unobservable: a fleet operator sees an empty DescribeInstanceInformation for hosts that really do carry AmazonSSMManagedInstanceCore, and gets no signal when they attach the policy to fix a host.

Fix: the SSM managed-node registration state keyed off the instance profile's role policies, and the DescribeInstanceInformation read that reports PingStatus for it.

Traces: ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached.

## Evidence
Expected (real AWS): GetServiceSetting('/ssm/managed-instance/default-ec2-instance-management-role') is unset; the host whose profile role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore appears in DescribeInstanceInformation with PingStatus 'Online' (<=600 s); the sibling whose role lacks the policy is absent from that listing for at least a 60 s window after it; after attaching the policy to the weak role and rebooting, that same InstanceId appears Online within 900 s.

Actual (emulator): ec2__ssm.vscan-inventory-omits-a-host-whose-profile-role-lacks-the-ssm-core-policy-until-it-is-attached ->
  Error: Check timed out after 180.0s
No host ever reports PingStatus 'Online', so the absence assertion is vacuous and the repair assertion is never evaluated.

## Steps to Reproduce
1. Confirm the Default Host Management Configuration service setting is unset.
2. Build two instance profiles for the same scenario: one whose role carries arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, one whose role does not.
3. RunInstances one t3.micro AL2 host with each profile into your own VPC/subnet.
4. Poll ssm:DescribeInstanceInformation for the privileged host's InstanceId with PingStatus 'Online'.
Observed: it never appears, and the check exhausts its budget before comparing the two hosts.
5. (unreachable) Attach AmazonSSMManagedInstanceCore to the weak role, reboot that host, and poll for it to appear Online.

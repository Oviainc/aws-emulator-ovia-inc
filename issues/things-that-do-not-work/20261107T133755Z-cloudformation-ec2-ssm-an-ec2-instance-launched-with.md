Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative
Issue-Key: cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative|_wait_ssm_online#5|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: d9e6cac456a6ddf38a11f2ddf4f109e13159427a

# cloudformation__ec2__ssm: an EC2 instance launched with an AmazonSSMManagedInstanceCore instance profile never becomes a managed node, so DescribeInstanceInformation never reports it Online and the check hangs to the harness cap

> _Note: this issue was identified against the code base at commit `d9e6cac456a6ddf38a11f2ddf4f109e13159427a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: nothing in the emulator registers an instance that RunInstances created as a Systems Manager managed node, so ssm DescribeInstanceInformation answers 200 with an InstanceInformationList that never contains the instance. A caller that waits for the agent to come up — which is the documented way to know an instance is manageable — waits forever: the check's positive-control poll (an instance launched WITH the SSM-capable instance profile must appear with PingStatus "Online") never converges and the check is killed by the harness instead of ever reaching its negative assertion about the profile-less sibling.

AWS: an instance launched from the AL2023 AMI (resolved from the public SSM parameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64) with an instance profile whose role carries arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore registers with Systems Manager shortly after boot. DescribeInstanceInformation (Smithy: service-com-amazonaws-ssm-amazonssm) then returns one InstanceInformation for it, with PingStatus from enum PingStatus {Online|ConnectionLost|Inactive}, plus LastPingDateTime, PlatformType, IamRole and ResourceType; an instance launched with NO SSM-capable profile has no agent credentials and never appears, so a Filters=[{Key: InstanceIds, Values: [<that id>]}] query returns InstanceInformationList == []. The model says nothing about registration latency — recorded behavior only: the recording shows the with-profile instance reaching PingStatus "Online" and the profile-less instance staying absent while it is running.
Emulator: the with-profile instance never appears at all, so the positive control cannot be established; the whole check makes no progress and ends as "Error: Check timed out after 60.0s" with no response divergence printed, because the poll never gets an answer to assert on. Hypothesis: the stall is the managed-node gate rather than the launch — the check's only other unbounded wait is the ec2 instance_running waiter before it — but either way the hang is produced by the emulator's own state, not by the runner: an emulator provisions instantly, so a faithful one settles this scenario in seconds.
Why: the failure mode is a hang, which is strictly worse for a client than a refusal — there is no error code to branch on and no timeout of the service's own. Every SSM-over-EC2 workflow (Run Command, Session Manager, inventory, patch scan, `aws ssm wait`, any CI gate that blocks until a node is Online) never starts, and the negative contract this check exists to prove — that an instance WITHOUT an SSM-capable role must not appear in the managed-node inventory — is unobservable, because absence is indistinguishable from the emulator's blanket absence.
Fix: managed-node registration in the SSM service's state — an instance EC2 minted whose instance profile can reach Systems Manager must enter the managed-node inventory and be projected with PingStatus Online, a LastPingDateTime, ResourceType EC2Instance, IamRole naming the profile and the platform members, on the same owed-transition arrangement the rest of the emulator uses; DescribeInstanceInformation's filtering/paging needs no change. NOTE: the open issue 20261027T034339Z-ec2-ssm-a-launched-ec2-instance-never-becomes.md describes this same root cause but carries only the two ec2__ssm replay keys; it does not carry this check's key, so this check must be re-replayed to confirm the fix and both issues close against the same registration work.
Traces: cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative (mismatch, "Check timed out after 60.0s").

## Evidence
Expected (AWS): after RunInstances of a t3.micro from the AL2023 AMI with an instance profile carrying AmazonSSMManagedInstanceCore, ssm DescribeInstanceInformation(Filters=[{Key: "InstanceIds", Values: [<with-role id>]}]) returns one InstanceInformation whose PingStatus is "Online" (and LastPingDateTime/PlatformType/IamRole/ResourceType populated); the same query for the sibling launched with no instance profile returns InstanceInformationList == [] while that instance is running.
Actual (emulator): the with-role instance never appears in DescribeInstanceInformation, so the positive-control poll (_wait_ssm_online, PingStatus == "Online", 300 s budget at 10 s interval) never converges; the check never reaches its negative assertion and the suite records verdict mismatch with detail "Error: Check timed out after 60.0s". No divergent response body is printed because the poll never receives a populated answer.

## Steps to Reproduce
1. ssm GetParameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 to resolve the AMI id.
2. Create an IAM role trusted by ec2.amazonaws.com, attach arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, create an instance profile and add the role to it.
3. ec2 RunInstances a t3.micro from that AMI into a subnet WITH IamInstanceProfile={Name: <profile>}, and a second identical instance WITHOUT any instance profile; wait for both via the instance_running waiter.
4. Poll ssm DescribeInstanceInformation(Filters=[{Key: "InstanceIds", Values: [<with-profile id>]}]) for PingStatus == "Online".
   Expected: the entry appears Online within registration latency.
   Actual: InstanceInformationList stays empty indefinitely; the caller polls until its deadline (the suite kills the check at 60 s).
5. Only then is the contract under test reachable: DescribeInstanceInformation for the profile-less instance must return InstanceInformationList == [].

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch cloudwatch__ec2__ssm.obs-ec2-2-ebs-consumed-iops-published-for-io2-and-withheld-from-a-gp3-written-in-the-same-instant cloudwatch__ec2__ssm.obs-ec2-2-ebs-reads-and-writes-land-on-separate-series-under-exactly-the-volume-id-dimension cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-series-follows-the-volume-across-a-move-to-a-second-instance cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached
Issue-Key: cloudwatch__ec2__ssm.dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch|_poll#16|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-2-ebs-consumed-iops-published-for-io2-and-withheld-from-a-gp3-written-in-the-same-instant|_poll#3|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-2-ebs-reads-and-writes-land-on-separate-series-under-exactly-the-volume-id-dimension|_poll#3|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-series-follows-the-volume-across-a-move-to-a-second-instance|_poll#3|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-2-ebs-write-telemetry-published-only-while-attached|_poll#3|AssertionError|raise AssertionError(
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# An EC2 instance launched with the AmazonSSMManagedInstanceCore profile never becomes an Online SSM managed node, so no EBS/agent telemetry check can start

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an instance launched with an instance profile carrying AmazonSSMManagedInstanceCore never registers with Systems Manager — DescribeInstanceInformation returns no row for it, so PingStatus never reaches Online.

AWS: an instance whose attached instance profile grants AmazonSSMManagedInstanceCore and which can reach the SSM endpoints registers with Systems Manager shortly after boot. DescribeInstanceInformation then returns an InstanceInformation entry for that instance id whose PingStatus is `Online`. Smithy (ssm): DescribeInstanceInformation output InstanceInformationList -> InstanceInformation with members InstanceId, PingStatus (enum Online|ConnectionLost|Inactive), PlatformType, IPAddress, ComputerName; the operation has no modeled error for "not yet registered" — an unregistered instance is simply absent from the list, which is what the recording shows forever.

Emulator: DescribeInstanceInformation never returns a row for the launched instance (`last observed: None`) for the whole 420–600 s wait, so PingStatus is never Online. Every check under this heading that needs the guest — a StatsD/guest-memory metric via the CloudWatch agent, and the four EBS-telemetry scenarios that write inside the guest and then read VolumeWriteBytes/VolumeReadBytes/VolumeConsumedReadWriteOps — stops at that gate and reaches no verdict about the telemetry it exists to check.

Why: a client that follows AWS's documented path to a managed node (attach the SSM-core instance profile, wait for Online) never gets there, so nothing that depends on SSM reaching into the guest works: Run Command, Session Manager, inventory, patching, and any agent that is installed or driven through SSM. The absence is also silent — the API returns an empty list, not an error, so a client cannot tell "not yet" from "never".

Fix: the SSM managed-node registration path (the component that registers an instance whose profile grants the SSM core policy and maintains its InstanceInformation/PingStatus), and its coupling to EC2 instance lifecycle.

Traces: dcwagent-custom-statsd-value-and-guest-only-memory-metric-reach-cloudwatch, obs-ec2-2-ebs-consumed-iops-published-for-io2-and-withheld-from-a-gp3-written-in-the-same-instant, obs-ec2-2-ebs-reads-and-writes-land-on-separate-series-under-exactly-the-volume-id-dimension, obs-ec2-2-ebs-write-series-follows-the-volume-across-a-move-to-a-second-instance, obs-ec2-2-ebs-write-telemetry-published-only-while-attached.

Related but not covering: open issue 20261107T064927Z-cloudwatch-ec2-ssm-the-ssm-installed-cloudwatch-agent.md owns two other agent checks under this heading; the five checks listed here are not carried by it.

## Evidence
Expected: after RunInstances with an IamInstanceProfile whose role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, ssm DescribeInstanceInformation(Filters=[InstanceIds=<id>]) returns one InstanceInformation with InstanceId=<id> and PingStatus='Online' within a few minutes.

Actual, identically in all five checks:
- `AssertionError: the SSM agent on i-<id> coming Online not met within 600s (last observed: None)`
- `AssertionError: the SSM agent on i-<id> coming Online not met within 420s (check-wide deadline 600s, 425s used; last observed: None)` (x4)
`last observed: None` means DescribeInstanceInformation returned no entry for the instance at all (empty list, 200), never a non-Online PingStatus. The instance id is a per-run value.

## Steps to Reproduce
1. Create a VPC/subnet and an IAM role+instance profile with AmazonSSMManagedInstanceCore attached.
2. ec2 RunInstances an AL2023 t3.micro with that IamInstanceProfile; wait for instance state `running`.
3. Poll ssm DescribeInstanceInformation(Filters=[{Key:'InstanceIds',Values:[<id>]}]) — the list stays empty indefinitely; no row with PingStatus='Online' ever appears.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2
Issue-Key: ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2|_poll#10|Timeout|last = probe()
Issue-Commit: 582ed21b383f102b650a2b551c0e79525419c00c

# ec2__ssm: a profile-bearing EC2 instance never becomes an SSM managed node, so DescribeInstanceInformation never serves the row whose IPAddress/ComputerName/SourceId and platform must agree with EC2

> _Note: this issue was identified against the code base at commit `582ed21b383f102b650a2b551c0e79525419c00c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an EC2 instance launched from the Amazon Linux 2 AMI with an instance profile whose role carries arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore never appears as an Online managed node, so the check's registration poll never settles and the identity/platform join against EC2 is never evaluated. Failing check: ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2.

AWS: a running instance reachable from the Systems Manager endpoints whose instance profile grants the SSM core permissions registers with Systems Manager, and DescribeInstanceInformation filtered with InstanceInformationStringFilter Key='InstanceIds', Values=[<instance-id>] returns exactly one InstanceInformation row for it with PingStatus='Online'. That row names the SAME machine EC2 describes: Smithy (com.amazonaws.ssm) InstanceInformation$IPAddress (@sensitive, @length 1..46) = the instance's PrivateIpAddress, InstanceInformation$ComputerName (@length 1..255, documented 'the fully qualified host name of the managed node') = the instance's PrivateDnsName, InstanceInformation$SourceId (@length 0..128, pattern ^[a-zA-Z0-9:_-]*$) = the instance id, and for an AL2 EC2 host InstanceInformation$PlatformType='Linux' (enum PlatformType), PlatformName='Amazon Linux', PlatformVersion='2', ResourceType='EC2Instance' (enum ResourceType) and SourceType='AWS::EC2::Instance' (enum SourceType). Registration latency is not modelled — recorded behavior only: the recording has the host Online well inside the check's 600 s registration deadline.

Emulator: the poll for an InstanceInformation row with PingStatus=='Online' never succeeds; no row (or no Online row) is ever served for the launched instance, and the check is terminated by the harness budget with 'Error: Check timed out after 180.0s' before any of the identity or platform comparisons run. No status/error code is produced — the divergence is the permanently absent (or never-Online) InstanceInformation row, not a refusal. Hypothesis: the emulator never projects a RunInstances instance that carries an IamInstanceProfile into SSM's managed-node registry, so DescribeInstanceInformation stays empty for it. Related open issues exist for sibling checks of this family — 20261108T004524Z-ec2-ssm-an-instance-whose-profile-role-gains.md, 20261107T225143Z-ec2-ssm-2-net-new-causes-a-launch.md, 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md, 20261107T173527Z-ec2-ssm-ssm-inventory-agent-version-check-never-completes-the.md and the CloudFormation-deployed variants 20261107T133755Z / 20261107T201046Z — but none of them carries this check's replay key (the suite did not mark it owned), and this one is specifically the identity/platform join (IPAddress, ComputerName, SourceId, PlatformType/Name/Version, ResourceType, SourceType) against EC2's DescribeInstances view of the same instance, which a canned inventory row would also fail.

Why: a client cannot observe the central Systems Manager invariant that an EC2 host with the SSM core instance profile becomes a managed node whose InstanceInformation describes that exact machine. Every flow keyed on DescribeInstanceInformation (SendCommand targeting, inventory, patch compliance, Session Manager) is unusable, and callers that poll for PingStatus 'Online' before proceeding hang until their own deadline instead of making progress; tools that join SSM rows to EC2 instances by IPAddress/ComputerName/SourceId get no data at all.

Fix: the SSM managed-node registration path and the DescribeInstanceInformation handler/state — a running EC2 instance whose instance-profile role grants the SSM core actions must be projected into the InstanceInformation store with PingStatus 'Online', and that row's IPAddress/ComputerName/SourceId must be derived from the instance's PrivateIpAddress/PrivateDnsName/instance id rather than canned, with PlatformType/PlatformName/PlatformVersion derived from the launched AMI and ResourceType/SourceType set to EC2Instance / AWS::EC2::Instance.

Traces: ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2 (the failure surfaces as the harness's 180 s budget expiring inside the check's _wait_online() poll, whose own deadline is 600 s, so the replay shows no response body).

## Evidence
Expected (AWS): after RunInstances of a t3.micro from /aws/service/ami-amazon-linux-latest/amzn2-ami-hvm-x86_64-gp2 into a public subnet with an instance profile whose role has AmazonSSMManagedInstanceCore,
  ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[<instance-id>]}])['InstanceInformationList']
returns one row with PingStatus='Online' and, for that row, IPAddress == ec2.describe_instances(...)['PrivateIpAddress'], ComputerName == PrivateDnsName, SourceId == <instance-id>, and (PlatformType, PlatformName, PlatformVersion, ResourceType, SourceType) == ('Linux', 'Amazon Linux', '2', 'EC2Instance', 'AWS::EC2::Instance').

Actual (emulator): the condition 'the SSM agent on <instance-id> coming Online' is never met — no Online InstanceInformation row is ever returned for the instance — and the check is killed by the harness:
  Error: Check timed out after 180.0s
None of the IPAddress/ComputerName/SourceId or platform assertions is ever reached.

## Steps to Reproduce
1. Replay the behavioral suite and re-run ec2__ssm.vscan-inventory-platform-and-address-agree-with-ec2.
2. Or manually: create a VPC + public subnet (IGW, 0.0.0.0/0 route, MapPublicIpOnLaunch), a role trusted by ec2.amazonaws.com with arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore attached, and an instance profile holding it.
3. Resolve the AL2 AMI via ssm.get_parameter(Name='/aws/service/ami-amazon-linux-latest/amzn2-ami-hvm-x86_64-gp2') and ec2.run_instances(ImageId=<that>, InstanceType='t3.micro', SubnetId=<subnet>, SecurityGroupIds=[<sg>], IamInstanceProfile={'Name': <profile>}).
4. Poll ssm.describe_instance_information(Filters=[{'Key':'InstanceIds','Values':[<instance-id>]}]): AWS serves one row with PingStatus 'Online'; the emulator serves none, so the poll never settles.
5. On AWS, compare that row's IPAddress/ComputerName/SourceId with ec2.describe_instances(InstanceIds=[<id>]) PrivateIpAddress/PrivateDnsName/InstanceId and its PlatformType/PlatformName/PlatformVersion/ResourceType/SourceType with ('Linux','Amazon Linux','2','EC2Instance','AWS::EC2::Instance') — the emulator never reaches this step.

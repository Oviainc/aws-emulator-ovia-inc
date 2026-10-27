Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer
Issue-Key: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer|_poll#5|Timeout|time.sleep(interval_s)
Issue-Commit: 547f40f49be2649465cead789446113d922f667f

# ec2__ssm: a launched EC2 instance never becomes a Systems Manager managed node, so DescribeInstanceInformation never reports PingStatus Online and both SSM-observed VPC data-plane checks hang until the harness kills them

> _Note: this issue was identified against the code base at commit `547f40f49be2649465cead789446113d922f667f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an EC2 instance that RunInstances created is never registered as a Systems Manager managed node, so `DescribeInstanceInformation` reports an empty `InstanceInformationList` forever and any caller that waits for the agent to come up waits for ever.

AWS: an instance launched from an AL2023 AMI with an instance profile carrying `AmazonSSMManagedInstanceCore`, in a VPC that can reach the `ssm`, `ssmmessages` and `ec2messages` interface endpoints, registers itself with Systems Manager shortly after boot. `DescribeInstanceInformation` then returns one `InstanceInformation` for that instance. Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model): `InstanceInformation$InstanceId` (pattern `^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$`), `InstanceInformation$PingStatus` -> enum `PingStatus` with values `Online | ConnectionLost | Inactive` ("Connection status of SSM Agent"), plus `LastPingDateTime`, `AgentVersion`, `PlatformType`, `IamRole` ("the role assigned to an Amazon EC2 instance") and `ResourceType` ("Instances are either EC2 instances or managed instances"). Registration is precisely what makes an instance a managed node, and once it is one, `SendCommand` on `AWS-RunShellScript` runs in the guest and `GetCommandInvocation` reports a terminal `Status` with the guest's `StandardOutputContent`. The checks poll that view with a 600 s budget at a 10 s interval, i.e. they allow for AWS's own registration latency.

Emulator: `DescribeInstanceInformation` is implemented, but it only reports nodes held in `ComAmazonawsSsmAmazonssmState::managed_nodes`, which — per its own handler contract, services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs lines 8-14 — "a node enters only by registering: an EC2 instance whose SSM Agent registers, or a hybrid node installed with an activation code. Nothing here invents a node". No path in the emulator performs that registration for an instance EC2's `RunInstances` minted, so the inventory stays empty for the life of the process: `PingStatus` is never `Online`, in fact no entry for the instance id exists at all. Both grounding checks launch two instances and immediately poll `describe_instance_information` (paginated) for `PingStatus == "Online"`; the predicate never becomes true, nothing after the poll is ever executed, and the suite's verdict for both is `mismatch` with `Error: Check timed out after 60.0s` — no response divergence is printed because the check never gets an answer to assert on. Hypothesis: the stall is at this managed-node gate rather than earlier in the fixture, because the only other long poll ahead of it is the interface endpoint reaching `available`, and that one does settle — `CreateVpcEndpoint` is implemented and records the owed `Pending` -> `Available` move in `vpc_endpoint_moves` (services/service-com-amazonaws-ec2-amazonec2/src/state.rs lines 3078-3084) at `TRANSITION_DELAY`, which is 500 ms (state.rs line 1567).

Why: a client asking "is this instance under Systems Manager management yet?" is told "no" permanently. Every SSM-driven workflow against an EC2 instance — Run Command, Session Manager, patch scan, inventory, and any CI that blocks on `aws ssm wait`/a PingStatus poll before continuing — never starts. The failure mode is a hang rather than an error, which is strictly worse for a client than a refusal: there is no error code to branch on and no timeout of the service's own.

Fix: instance registration belongs in the SSM crate's state (services/service-com-amazonaws-ssm-amazonssm/src/state.rs, the `managed_nodes` inventory) — a launched instance whose instance profile can reach Systems Manager must enter that inventory and be projected with `PingStatus` `Online`, a `LastPingDateTime`, `ResourceType` EC2Instance, `IamRole` naming the instance profile and the platform members, on the same clock-owed-transition arrangement the rest of the emulator uses. The describe handler's filtering/paging needs no change. Note for the fixer, not separate causes to close: once registration lands, these two checks additionally need `SendCommand`/`GetCommandInvocation` to execute in the guest and the VPC to actually filter packets (a NACL is stateless, a security group stateful), and the second check also reaches `StartNetworkInsightsAnalysis`, which is still a scaffold stub answering 501 (services/service-com-amazonaws-ec2-amazonec2/src/ops/start_network_insights_analysis.rs) alongside the already-open `DescribeNetworkInsightsAnalyses`/`DescribeNetworkInsightsPaths` stub issues.

Traces: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful (mismatch, "Check timed out after 60.0s"); ec2__ssm.sg-resolution-prefix-list-and-reachability-analyzer (mismatch, "Check timed out after 60.0s"). Both poll `_agent_view` -> `describe_instance_information` for `PingStatus == "Online"` right after launching their client and server instances.

## Evidence
Expected (AWS): after RunInstances of an AL2023 instance with an AmazonSSMManagedInstanceCore instance profile and available ssm/ssmmessages/ec2messages interface endpoints, `DescribeInstanceInformation` returns an InstanceInformation whose InstanceId is the launched instance and whose PingStatus is "Online" (enum PingStatus: Online|ConnectionLost|Inactive), with LastPingDateTime, PlatformType, IamRole and ResourceType populated; SendCommand/GetCommandInvocation then run shell commands in that guest.

Actual (emulator): `DescribeInstanceInformation` answers 200 with an InstanceInformationList that never contains the instance — the handler reports only nodes in `managed_nodes`, which nothing populates for an EC2-launched instance (services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs:8-14, "Nothing here invents a node"). The two checks' poll (predicate `i is not None and i.get("PingStatus") == "Online"`, timeout_s=600, interval_s=10) therefore never converges and each check is killed by the harness: verdict `mismatch`, detail `Error: Check timed out after 60.0s`.

Supporting: the preceding fixture poll (interface endpoint -> "available") does settle, since CreateVpcEndpoint records the Pending->Available move in vpc_endpoint_moves (ec2 state.rs:3078-3084) at TRANSITION_DELAY = 500 ms (ec2 state.rs:1567), so the fixture reaches the managed-node gate and stops there.

## Steps to Reproduce
1. Create a VPC (10.71.0.0/16) with EnableDnsSupport and EnableDnsHostnames true, three subnets (client, server, endpoint) and security groups allowing tcp/443 from the VPC CIDR to the endpoint SG.
2. CreateVpcEndpoint (Interface) for com.amazonaws.<region>.ssm, .ssmmessages and .ec2messages into the endpoint subnet with PrivateDnsEnabled=true; poll DescribeVpcEndpoints until each is "available" (this succeeds).
3. Create an IAM role/instance profile with arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore.
4. RunInstances two t3.micro instances from the AMI in /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, attaching that instance profile, one per subnet.
5. Poll ssm DescribeInstanceInformation (paginated) for an entry whose InstanceId is either launched instance.
   Expected: within AWS's registration latency each instance appears with PingStatus "Online".
   Actual: InstanceInformationList never contains the instances; the poll runs until the caller's deadline. Replaying either grounding check reproduces it as "Check timed out after 60.0s".

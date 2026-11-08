Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive|_wait_ssm_online#4|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: 903d830d252bdd68b0796b6265be694baf73f62a

# An EC2 instance with an SSM-core instance profile never becomes a managed node, so Run Command output is never retrievable (check hangs to its 180s budget)

> _Note: this issue was identified against the code base at commit `903d830d252bdd68b0796b6265be694baf73f62a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
One net-new root cause, for the one cloudformation__ec2__ssm check that the open issue 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md does NOT carry a current replay key for. That issue describes a similar registration gap and still owns the sibling sdk-ssm-* checks, but it does not cover this one, so it is filed here; a fixer should close both together.

Cause 1 of 1: a running EC2 instance launched with an IAM instance profile carrying AmazonSSMManagedInstanceCore never registers with Systems Manager, so it never appears in DescribeInstanceInformation and no Run Command invocation on it ever reaches a terminal status.

AWS: recorded behavior only (the Smithy model states no lifecycle). An EC2 instance in `running` state whose instance profile grants the SSM core policy registers with Systems Manager shortly after boot: DescribeInstanceInformation(Filters=[{Key:"InstanceIds",Values:[<instance id>]}]) returns one InstanceInformationList entry whose PingStatus is `Online`; SendCommand(DocumentName="AWS-RunShellScript", Parameters={"commands":["echo <token>"]}) is accepted; and GetCommandInvocation(CommandId, InstanceId) reaches Status `Success` with StandardOutputContent carrying the command's stdout verbatim (the echoed token plus a trailing newline) and StandardErrorContent empty. A well-formed instance id that names no managed node is not an error for DescribeInstanceInformation — it simply matches nothing — so an unregistered instance is indistinguishable from an absent one and a caller can only wait.

Emulator: the node never enters the inventory. `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs` answers purely from `ComAmazonawsSsmAmazonssmState::managed_nodes`, "which a node enters only by registering: an EC2 instance whose SSM Agent registers, or a hybrid node installed with an activation code", and the EC2 RunInstances lifecycle performs no such registration. DescribeInstanceInformation therefore returns an empty InstanceInformationList forever, the check's registration poll never sees PingStatus `Online`, and the check is killed by its budget: `Error: Check timed out after 180.0s`. Because the instance never registers, the SendCommand / GetCommandInvocation half of the operation is never reached either.

Why: every Run Command scenario against a CloudFormation/EC2-provisioned instance is unusable — there is no status, no error code and no refusal, only an unbounded wait. A client cannot distinguish "the agent has not registered yet" (retry) from "this instance will never be managed" (fail), so SDK waiters and CI pipelines hang instead of failing, and the stdout-capture contract (StandardOutputContent / StandardErrorContent) can never be exercised.

Fix: the SSM managed-node registration the EC2 instance lifecycle owes — an instance reaching `running` with an SSM-core instance profile must insert a node into the ssm service's `managed_nodes` state with PingStatus `Online` — and the SendCommand/GetCommandInvocation state machine that depends on that node, so an invocation progresses to `Success` and carries the document's stdout.

Traces: cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive.

## Evidence
From the behavioral replay, check cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive:

  Error: Check timed out after 180.0s

The check creates an IAM role trusting ec2.amazonaws.com, attaches arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, wraps it in an instance profile, runs a t3.micro from the AL2023 AMI SSM parameter, waits for `instance_running` (which succeeds), then polls DescribeInstanceInformation for PingStatus `Online`. No entry is ever returned, so the 300s registration poll is still spinning when the check's 180s budget expires; the assertions on Status == `Success`, StandardOutputContent == the echoed token and StandardErrorContent == "" are never reached.

Expected (AWS): DescribeInstanceInformation lists the instance with PingStatus `Online`; GetCommandInvocation reaches Status `Success`, StandardOutputContent is the command's stdout, StandardErrorContent is empty.
Actual (emulator): DescribeInstanceInformation returns an empty InstanceInformationList indefinitely — no response, no error code, no refusal.

Source: `services/service-com-amazonaws-ssm-amazonssm/src/ops/describe_instance_information.rs` (header, lines 8-14) states the inventory is only ever populated by registration; nothing in the EC2 instance lifecycle registers a node.

Related open issue (does not cover this check): 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md.

## Steps to Reproduce
1. Create an IAM role trusting ec2.amazonaws.com, attach arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, create an instance profile and add the role to it.
2. Resolve the AL2023 AMI via GetParameter(Name="/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64") and RunInstances(InstanceType="t3.micro", IamInstanceProfile={Name:<profile>}) into a shared-network subnet; wait for the instance_running waiter.
3. Poll DescribeInstanceInformation(Filters=[{Key:"InstanceIds",Values:[<instance id>]}]) — the list stays empty, never reporting PingStatus `Online`.
4. (Unreachable in the emulator) SendCommand(DocumentName="AWS-RunShellScript", Parameters={"commands":["echo <token>"]}, TimeoutSeconds=120), then GetCommandInvocation(CommandId, InstanceId) until terminal, and compare StandardOutputContent with the echoed token.

Re-replay: cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive.

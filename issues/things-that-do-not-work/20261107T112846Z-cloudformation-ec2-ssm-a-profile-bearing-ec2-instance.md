Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive|_wait_ssm_online#5|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: 46dd75ad5ad4e002f82bb21fdacb370faf08a983

# cloudformation__ec2__ssm: a profile-bearing EC2 instance never becomes an SSM managed node, so SendCommand fan-out and ListCommandInvocations scoped by InstanceId can never be exercised (check hangs to timeout)

> _Note: this issue was identified against the code base at commit `46dd75ad5ad4e002f82bb21fdacb370faf08a983`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommandInvocations

## Description
Cause 1 of 1: an EC2 instance launched with an AmazonSSMManagedInstanceCore instance profile never registers as an SSM managed node, so a SendCommand fanned out to two instances never produces per-instance invocations and ListCommandInvocations can never be observed scoping by InstanceId. The check makes no further progress and ends in the harness budget ("Check timed out after 60.0s").

AWS: an instance whose instance profile carries AmazonSSMManagedInstanceCore registers with Systems Manager within minutes — DescribeInstanceInformation (Smithy: DescribeInstanceInformation -> InstanceInformationList, filter key InstanceIds) returns one InstanceInformation for it with PingStatus=Online. SendCommand (Smithy: SendCommand, InstanceIds up to 50, DocumentName required, returns Command with a CommandId) accepted for two InstanceIds creates ONE CommandId with one invocation per target instance; GetCommandInvocation reaches a terminal Status (Success for `echo` on AWS-RunShellScript) for each, and ListCommandInvocations(CommandId, InstanceId) (Smithy: ListCommandInvocations, InstanceId pattern ^(^i-(\w{8}|\w{17})$)|(^mi-\w{17}$)$, output CommandInvocations) returns exactly the one invocation of the named instance and none of the sibling's. Modeled refusals for a target that is not a managed node are InvalidInstanceId (400); nothing in the model permits an indefinite non-answer.

Emulator: the instance is launched and reaches `running`, but DescribeInstanceInformation keeps returning an empty InstanceInformationList (or never PingStatus=Online) for it, so the check's registration poll never satisfies its predicate and the check is killed at the 60s budget before any SendCommand/ListCommandInvocations assertion runs. No SSM invocation state is ever created for the instance.

Why: a client that launches an SSM-managed instance and drives Run Command gets neither success nor a refusal — it hangs. Fleet code cannot distinguish "node not yet registered" from "permission denied" or "bad instance id", and the per-instance scoping of ListCommandInvocations (the guarantee that a sibling target's invocation does not leak into another instance's filtered list) is untestable and unusable.

Fix: the SSM managed-node registration that the EC2 instance lifecycle should produce (the DescribeInstanceInformation state fed by an instance profile granting ssm:*), plus the SendCommand/command-invocation state machine and the InstanceId filter in the ListCommandInvocations handler that depend on it.

Note: open issue 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md describes a similar managed-node-registration gap for neighbouring checks, but the replay did not report this check as owned by it, so it is filed here; if a fixer closes both, this check must be re-replayed to confirm.

Traces: cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive.

## Evidence
Behavioral replay, check cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive [mismatch]:
  Error: Check timed out after 60.0s

The check's own expectations (from its source in the replay report):
- `_wait_ssm_online` polls ssm.describe_instance_information(Filters=[{"Key":"InstanceIds","Values":[<id>]}]) and requires items[0]["PingStatus"] == "Online"; it asserts `instance <id> never registered Online with SSM` otherwise.
- then ssm.send_command(InstanceIds=[a,b], DocumentName="AWS-RunShellScript", Parameters={"commands":["echo fanout-ok"]}) -> one Command.CommandId, and get_command_invocation must reach Status == "Success" for both a and b.
- then ssm.list_command_invocations(CommandId=<id>, InstanceId=a) must return exactly 1 CommandInvocation with InstanceId == a, and none with InstanceId == b (and symmetrically for b).

Expected: all of the above observable within the registration/ invocation budgets. Actual: the emulator never lists the instance as an Online managed node, so execution never leaves the registration poll and the check is terminated at the harness budget — no SendCommand response, no invocation, no ListCommandInvocations answer.

## Steps to Reproduce
1. Build and boot the emulator and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Re-run check cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive.
3. Manually: create an IAM role trusted by ec2.amazonaws.com with arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore attached, put it in an instance profile, RunInstances two t3.micro instances with that profile, then poll ssm DescribeInstanceInformation with Filters=[{Key:InstanceIds,Values:[<instance-id>]}] — observe the list stays empty / never reaches PingStatus=Online.
4. Then SendCommand to both instance ids with AWS-RunShellScript and call ListCommandInvocations(CommandId, InstanceId=<one of them>) — observe no per-instance invocation is ever produced.

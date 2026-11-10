Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|_wait_all#13|Timeout|time.sleep(2)
Issue-Commit: 22d8facbc3b601c7c60aed2bc6871542e8162809

# autoscaling__cloudformation__ec2: an AutoScalingReplacingUpdate (WillReplace) stack update with a CreationPolicy ResourceSignal never settles, so the replacement-group path cannot complete

> _Note: this issue was identified against the code base at commit `22d8facbc3b601c7c60aed2bc6871542e8162809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
1 net-new root cause on the Auto Scaling replacing-update path.

Cause 1 of 1: a stack whose AWS::AutoScaling::AutoScalingGroup carries UpdatePolicy AutoScalingReplacingUpdate WillReplace: true and a CreationPolicy ResourceSignal never reaches a terminal stack status; the check is cut off at its 180 s budget with no response to diagnose, so the replacement group is never built and the FAILURE-signal rejection it should produce is never observable.

AWS: a replacing update rewrites the group's LaunchTemplate property, so CloudFormation builds a whole new Auto Scaling group alongside the old one, waits for the new group's instances to signal their CreationPolicy ResourceSignal, and then either cuts over (UPDATE_COMPLETE, old group deleted) or — when the new instances signal FAILURE — fails the update and rolls back to the original group (UPDATE_ROLLBACK_COMPLETE, the replacement group removed and the original group's instances still InService). Either way the stack leaves UPDATE_IN_PROGRESS and lands in a terminal status, and DescribeStackEvents names both the original and the replacement group PhysicalResourceId for the group's logical id. Smithy: cloudformation#UpdateStack declares InsufficientCapabilitiesException, TokenAlreadyExistsException (and the common ValidationError); none of them applies here — AWS does not refuse the call, it completes the operation. The timing and the signal semantics are recorded behavior only.

Emulator: the check is reported as `Error: Check timed out after 180.0s`. No status, error code or message is returned at all: the stack is still non-terminal when the budget expires, so nothing downstream of the replacing update (the replacement group's PhysicalResourceId in the events, the FAILURE-signal reason, the rollback to the original group) can be read.

Why: a client that uses AutoScalingReplacingUpdate with instance signals — the standard blue/green pattern for an ASG, and what CDK emits for a replacing ASG update — gets a stack that hangs in UPDATE_IN_PROGRESS instead of either completing or rolling back, so deploys never finish and a failed replacement is never reported as failed.

Fix: the AWS::AutoScaling::AutoScalingGroup update handler and the stack operation's resource-signal wait — honour AutoScalingReplacingUpdate/WillReplace by provisioning a replacement group, account the CreationPolicy ResourceSignal for the replacement group's instances, and drive the stack to UPDATE_COMPLETE or UPDATE_ROLLBACK_COMPLETE (deleting the discarded group) rather than leaving the operation non-terminal.

The open issue 20261030T154944Z-autoscaling-cloudformation-ec2-a-cloudformation-stack-whose-aws.md covers the sibling rolling-update check but does not carry this check's replay key.

Traces: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected.

## Evidence
Expected (AWS): after bumping the Generation parameter (which rewrites the launch template UserData and so the group's LaunchTemplate property) with SignalExitCode=1, the stack leaves UPDATE_IN_PROGRESS and reaches UPDATE_ROLLBACK_COMPLETE; DescribeStackEvents shows a replacement group PhysicalResourceId for the AutoScalingGroup logical id with a FAILED status and a signal-failure reason, DescribeAutoScalingGroups still returns the original group with its instances InService, and the replacement group name is gone.

Actual (emulator): no terminal status is ever reached — the replay reports only
  Error: Check timed out after 180.0s
with no HTTP status, error code or message from the emulator to diagnose.

## Steps to Reproduce
1. CreateStack a template with an AWS::EC2::LaunchTemplate (UserData calling `aws cloudformation signal-resource`) and an AWS::AutoScaling::AutoScalingGroup with no AutoScalingGroupName, DesiredCapacity 2, a CreationPolicy ResourceSignal and UpdatePolicy AutoScalingReplacingUpdate WillReplace: true; wait for CREATE_COMPLETE.
2. UpdateStack with the Generation parameter bumped and SignalExitCode=1, so the launch template UserData changes and the new group's instances signal FAILURE.
3. Poll DescribeStacks: the stack never leaves its in-progress status, so the operation never settles.
4. Re-replay autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected to confirm the fix.

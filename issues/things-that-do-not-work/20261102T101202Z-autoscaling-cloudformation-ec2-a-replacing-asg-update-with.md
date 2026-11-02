Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|_wait_all#13|Timeout|statuses = {n: _stack_status(cfn, n) for n in names}
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# autoscaling/cloudformation/ec2: a replacing ASG update with a failed signal never settles — the check exhausts its 60s budget

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a stack update that replaces an Auto Scaling group and must then refuse the replacement on a failed creation signal never reaches a terminal state.
AWS: recorded behavior only — the replacing update completes or fails and rolls back well inside the check's 60s budget, leaving the stack in a terminal status (UPDATE_COMPLETE or UPDATE_ROLLBACK_COMPLETE) and the original group serving.
Emulator: the check ends with 'Check timed out after 60.0s' — the stack operation never leaves *_IN_PROGRESS. Hypothesis: the CreationPolicy/signal wait for the replacement group never resolves, so the update neither completes nor fails.
Why: a client waiting on the update hangs forever and can neither proceed nor roll back.
Fix: the ASG replacement path of the UpdateStack handler — the creation-signal wait must reach a terminal state (and time out as AWS does).
Traces: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected.

## Evidence
Error: Check timed out after 60.0s — no terminal stack status was ever observed for the replacing update. Neighbouring checks in the same family are already filed under 20261030T154944Z-autoscaling-cloudformation-ec2-a-cloudformation-stack-whose-aws.md, which does not carry this replay key.

## Steps to Reproduce
Replay autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected: deploy a stack with an AWS::AutoScaling::AutoScalingGroup carrying a CreationPolicy, then update it so the group is replaced and the replacement's creation signal fails; poll DescribeStacks for a terminal status.

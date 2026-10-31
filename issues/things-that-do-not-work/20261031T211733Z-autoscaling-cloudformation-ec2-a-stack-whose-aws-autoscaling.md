Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_deploy_cfn#9|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|run#14|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Commit: fc759f6a3a0a0e1d24e16f577daa39145fc82623

# autoscaling__cloudformation__ec2: a stack whose AWS::AutoScaling::AutoScalingGroup carries a CreationPolicy ResourceSignal always ends CREATE_FAILED — the group's instances never deliver a resource signal

> _Note: this issue was identified against the code base at commit `fc759f6a3a0a0e1d24e16f577daa39145fc82623`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, grounded in the two autoscaling__cloudformation__ec2 checks below — the only failures in this suite run that the report marks neither ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: a CreateStack whose AWS::AutoScaling::AutoScalingGroup declares a CreationPolicy.ResourceSignal (Count = the group's desired capacity, Timeout PT12M/PT15M) can never satisfy that policy. The emulator launches the group's instances but nothing ever arrives at CloudFormation as a resource signal for the group's LogicalResourceId, so the CreationPolicy always expires and the stack converges to CREATE_FAILED with the status reason 'Failed to receive <n> resource signal(s) within the specified duration' instead of CREATE_COMPLETE. Both fixtures carry a signal-based UpdatePolicy as well (AutoScalingRollingUpdate with MaxBatchSize/MinInstancesInService/WaitOnResourceSignals in one, AutoScalingReplacingUpdate/WillReplace in the other), and because the create leg never completes neither update path is reachable at all.

AWS: a CreationPolicy.ResourceSignal on an Auto Scaling group holds the group resource in CREATE_IN_PROGRESS until Count signals with Status=SUCCESS have been delivered for that LogicalResourceId, and then the resource and stack reach CREATE_COMPLETE. The delivery channel is cloudformation:SignalResource (Smithy, cloudformation: SignalResource input StackName required, StackNameOrId, @length min 1, pattern '^([a-zA-Z][-a-zA-Z0-9]*)|(arn:...)$'; LogicalResourceId required; UniqueId required, @length 1..64; Status required, enum ResourceSignalStatus SUCCESS/FAILURE; no modelled errors), which both fixtures invoke exactly as AWS documents the pattern — `aws cloudformation signal-resource --stack-name <stack> --logical-resource-id AutoScalingGroup --unique-id <instance-id> --status SUCCESS|FAILURE` from the launch template's UserData under an instance profile scoped to the stack. On real AWS the instances the group launches run that UserData, the signals arrive, and the stack reaches CREATE_COMPLETE; only when the signals do NOT arrive inside the CreationPolicy Timeout (or a FAILURE signal arrives) does the resource fail with 'Failed to receive N resource signal(s) within the specified duration' and the stack go CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE. The two fixtures are marked DRAFT/unblessed, so for the exact stack-event wording this is recorded behavior only; the requirement asserted here is the documented one — a group whose instances DO signal success must reach CREATE_COMPLETE, and the timeout text is the failure path, not the normal one.

Emulator: the success path is unreachable. Check 2 reports the terminal status and reason verbatim: stack CREATE_FAILED, ['<stack>:CREATE_FAILED:Failed to receive 1 resource signal(s) within the specified duration'] — i.e. the CreationPolicy timed out even though the group's launch template UserData issues the signal call in a 30-attempt retry loop. Check 1 fails the same way one step earlier in its helper, 'Stack <name> ended in CREATE_FAILED'. So instances the emulator's Auto Scaling group launches never execute (or never have their UserData interpreted as) the signal-resource call, and the emulator mints no signal on their behalf; the only outcome the signal-gated ASG stack can have is the timeout failure. Hypothesis (neither the model nor the recording states it): check 2's CreationPolicy Count is `!Ref DesiredCapacity` = 2 while the reason names 1 outstanding signal, so either the !Ref-resolved Count or the launched-instance count feeding the accounting is 1 rather than 2 — worth confirming while fixing the accounting, but the divergence filed here is the unconditional failure, not the number in the sentence.

Why: the standard, documented way to deploy an Auto Scaling group through CloudFormation — a CreationPolicy ResourceSignal fed from instance UserData, plus AutoScalingRollingUpdate or AutoScalingReplacingUpdate for subsequent changes — cannot succeed on this emulator. `aws cloudformation deploy`, `cdk deploy`, a stack waiter or any CI gate on stack-create-complete always fails the create, so a client cannot bring up a signal-gated ASG stack at all, and the behaviours layered on top of it (the rolling-update batch cap and minimum-in-service floor, the FAILURE-signal cancel-and-roll-back, the restored launch-template version after a replacing update) are untestable because the stack never gets past create.

Fix: the CloudFormation stack-progress engine's CreationPolicy/ResourceSignal accounting for AWS::AutoScaling::AutoScalingGroup together with the EC2/Auto Scaling instance-launch path that is supposed to feed it — instances the group launches must produce the SignalResource(Status=SUCCESS) deliveries their launch-template UserData asks for (one per instance, keyed by UniqueId), the Count must be the !Ref-resolved CreationPolicy value, and the group resource must reach CREATE_COMPLETE once Count SUCCESS signals are counted, reserving the 'Failed to receive N resource signal(s) within the specified duration' reason and the CREATE_FAILED/ROLLBACK path for the genuine timeout and FAILURE cases. The UpdatePolicy legs (WaitOnResourceSignals/PauseTime, WillReplace) hang off the same accounting.

Related open issue that does NOT cover this: 20261030T154944Z-autoscaling-cloudformation-ec2-a-cloudformation-stack-whose-aws.md named these same two checks when the stacks never reached ANY terminal status (its replay keys are harness timeouts in the poll loops). The emulator's behaviour has since moved — the stacks now DO settle, on CREATE_FAILED with the resource-signal timeout reason — so neither check's current replay key is carried by that issue and the report does not mark them OWNED. Also not covering it: 20261029T172317Z-cloudformation-a-stack-deploying-an-auto-scaling-group.md (ASG stack Outputs / CREATE_COMPLETE reporting, no CreationPolicy signal path) and 20261028T172843Z-autoscaling-cloudformation-cloudwatch-ec...-the-cdk-l2-asg.md (CDK L2 target-tracking drift scenario).

Traces: the two failing check ids in trace_ids.

## Evidence
Failing checks (both [mismatch]; the report marks neither ALREADY FILED nor OWNED):

1) autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal
   Error: File "...cfn_rolling_update_honours_max_batch_size_then_rolls_back_on_a_failed_signal.py", line 144, in run
     result = _deploy_cfn(ctx, TEMPLATE, parameters=parameters, timeout_s=1500.0)
   File ..., line 612, in _deploy_cfn
     raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
   AssertionError: Stack <stack> ended in CREATE_FAILED
   Template deployments/asg-rolling-update-signals.yaml: AutoScalingGroup with CreationPolicy.ResourceSignal {Count: !Ref DesiredCapacity = 3, Timeout: PT15M} and UpdatePolicy.AutoScalingRollingUpdate {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}. Leg A (create) is the positive control: it can only pass if DESIRED instances really signalled.

2) autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
   Error: File "...deploy_autoscaling_1_replacing_update_swaps_group_failed_signal_replacement_rejected.py", line 291, in run
     assert status == "CREATE_COMPLETE", ...
   AssertionError: stack <stack> must reach CREATE_COMPLETE once 2 instances signal SUCCESS, got 'CREATE_FAILED': ['<stack>:CREATE_FAILED:Failed to receive 1 resource signal(s) within the specified duration']
   Inlined template: AutoScalingGroup, no AutoScalingGroupName, CreationPolicy {AutoScalingCreationPolicy: {MinSuccessfulInstancesPercent: 100}, ResourceSignal: {Count: !Ref DesiredCapacity = 2, Timeout: PT12M}}, UpdatePolicy.AutoScalingReplacingUpdate {WillReplace: true}; UserData runs `aws cloudformation signal-resource --region ${AWS::Region} --stack-name ${AWS::StackName} --logical-resource-id AutoScalingGroup --unique-id <...> --status SUCCESS|FAILURE` inside a 30-attempt retry loop, with SignalExitCode selecting SUCCESS (0) or FAILURE (1).

Expected (AWS): with SignalExitCode=0 the group's instances each deliver SignalResource(Status=SUCCESS) for LogicalResourceId AutoScalingGroup, the CreationPolicy's Count is met inside its Timeout, and DescribeStacks reports StackStatus CREATE_COMPLETE. 'Failed to receive N resource signal(s) within the specified duration' with CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE is reserved for the case where the signals genuinely do not arrive in time or one of them is FAILURE; N is the number of signals still outstanding, derived from the resolved CreationPolicy Count minus the SUCCESS signals counted.

Actual (emulator): every signal-gated ASG stack takes the failure path. StackStatus CREATE_FAILED; the status reason is the timeout sentence 'Failed to receive 1 resource signal(s) within the specified duration' even where the resolved Count is 2 and every launched instance's UserData asks for the signal. No SUCCESS signal is ever accounted for, so CREATE_COMPLETE is unreachable and the rolling-update / replacing-update legs after it never run.

Fixed-text vs variable parts of the diverging message: the fixed text is 'Failed to receive ' + ' resource signal(s) within the specified duration'; the only variable is the outstanding-signal count, which must come from the CreationPolicy's resolved ResourceSignal Count and the SUCCESS signals received (the recorded '1' and the stack name in the event prefix are this run's values, not part of the requirement).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a stack from a template whose AWS::AutoScaling::AutoScalingGroup declares, over a VPC/subnet/launch-template/instance-profile estate:
     CreationPolicy:
       AutoScalingCreationPolicy: {MinSuccessfulInstancesPercent: 100}
       ResourceSignal: {Count: !Ref DesiredCapacity, Timeout: PT12M}
     UpdatePolicy:
       AutoScalingRollingUpdate: {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}
   (or AutoScalingReplacingUpdate: {WillReplace: true}), with DesiredCapacity 2-3 and a launch template whose UserData runs
     aws cloudformation signal-resource --region <r> --stack-name <stack> --logical-resource-id AutoScalingGroup --unique-id <instance-id> --status SUCCESS
   e.g. deployments/asg-rolling-update-signals.yaml from the first check:
     aws cloudformation create-stack --stack-name s --template-body file://asg-rolling-update-signals.yaml --parameters ... --capabilities CAPABILITY_IAM
3. Poll `aws cloudformation describe-stacks --stack-name s --query 'Stacks[0].StackStatus'` and read `aws cloudformation describe-stack-events --stack-name s`.
   Observed: CREATE_FAILED with reason 'Failed to receive 1 resource signal(s) within the specified duration' — no SUCCESS signal is ever counted, so the create always fails and `aws cloudformation wait stack-create-complete` always errors.
   Expected: CREATE_COMPLETE once the group's instances have delivered Count SUCCESS signals inside the Timeout.
4. Cross-check the delivery channel directly: while the create is in progress, send `aws cloudformation signal-resource --stack-name s --logical-resource-id AutoScalingGroup --unique-id <per-instance-id> --status SUCCESS` once per expected instance, and confirm whether the stack then reaches CREATE_COMPLETE — this separates 'the emulator's launched instances never run their UserData / never signal' from 'SignalResource is not counted against the ASG CreationPolicy at all'.
5. Only after the create converges, exercise the update legs: bump the parameter that rewrites the launch template UserData and confirm UPDATE_COMPLETE on the SUCCESS path, and UPDATE_ROLLBACK_COMPLETE (update cancelled, group rolled back to the starting launch-template version) when the replacement instances signal FAILURE.
6. Replay the two checks in trace_ids to confirm the fix.

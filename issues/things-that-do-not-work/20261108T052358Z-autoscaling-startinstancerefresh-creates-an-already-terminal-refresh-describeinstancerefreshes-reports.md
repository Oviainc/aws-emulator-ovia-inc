Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.op-contract-cancelInstanceRefresh
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.op-contract-cancelInstanceRefresh|_poll_until#7|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# StartInstanceRefresh creates an already-terminal refresh: DescribeInstanceRefreshes reports Status=Successful immediately, so no refresh is ever InProgress and CancelInstanceRefresh is unreachable

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceRefresh

## Description
Cause 1 of 1: an instance refresh is born terminal — it is reported Successful at the first poll after StartInstanceRefresh, so the Pending/InProgress/Baking phases never exist and nothing can be observed, monitored or cancelled while a refresh is running.

AWS: Smithy (com.amazonaws.autoscaling#AutoScaling_2011_01_01): StartInstanceRefresh takes AutoScalingGroupName (required), Strategy, DesiredConfiguration, Preferences and returns StartInstanceRefreshAnswer.InstanceRefreshId; DescribeInstanceRefreshes returns InstanceRefresh with Status of enum InstanceRefreshStatus = [Pending, InProgress, Successful, Failed, Cancelling, Cancelled, RollbackInProgress, RollbackFailed, RollbackSuccessful, Baking], plus PercentageComplete (0-100) and InstancesToUpdate (min 0). The model documents the states: 'Pending - The request was created, but the instance refresh has not started', 'InProgress - An instance refresh is in progress', 'Baking - Waiting the specified bake time after an instance refresh has finished updating instances', 'Successful - An instance refresh completed successfully'. CancelInstanceRefresh returns CancelInstanceRefreshAnswer.InstanceRefreshId (the refresh it cancelled) and models ActiveInstanceRefreshNotFoundFault (HTTP 400, awsQueryError code ActiveInstanceRefreshNotFound); StartInstanceRefresh models InstanceRefreshInProgressFault (HTTP 400, code InstanceRefreshInProgress). The ordering in time is recorded behavior only: in the recording, a refresh started on a group with one InService instance (Preferences MinHealthyPercentage=90, InstanceWarmup=120) is reported Status=InProgress while it replaces instances, CancelInstanceRefresh then returns that same InstanceRefreshId and the refresh moves to Cancelled; a second StartInstanceRefresh while one is still running is refused HTTP 400 InstanceRefreshInProgress, and CancelInstanceRefresh once nothing is active is refused HTTP 400 ActiveInstanceRefreshNotFound with the message 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.' (fixed text; the only variable part is the group name taken from the request).

Emulator: StartInstanceRefresh returns an InstanceRefreshId, but the very first DescribeInstanceRefreshes(AutoScalingGroupName=<group>, InstanceRefreshIds=[<id>]) already answers Status='Successful', and it stays Successful — the check polls for Status in ('InProgress',) every 3s for 60s and every observation is 'Successful' (AssertionError: condition not met within 60.0s; last='Successful'). No Pending, no InProgress, no Baking, and no instance is actually replaced in between. As a direct consequence CancelInstanceRefresh on a group whose refresh was just started hits the no-active-refresh branch, and the modeled InstanceRefreshInProgress boundary can never be reached.

Why: a client's standard instance-refresh flow is start -> poll DescribeInstanceRefreshes until the Status leaves InProgress (reading PercentageComplete/InstancesToUpdate), with CancelInstanceRefresh as the abort path. Against this emulator the refresh is complete before the caller can see it: a progress waiter never observes a running refresh, an abort path can never cancel one (it is refused ActiveInstanceRefreshNotFound instead of returning the refresh id and driving it to Cancelled), and a client that guards against concurrent refreshes never sees InstanceRefreshInProgress. The refresh is also reported as having succeeded without having replaced anything, so a deploy gate keyed on Successful passes on work that never happened.

Fix: the Auto Scaling instance-refresh state machine in the autoscaling crate — the per-group refresh state written by the StartInstanceRefresh handler and advanced by the reconciliation that replaces instances, as read by DescribeInstanceRefreshes and mutated by CancelInstanceRefresh. Not the response serializers: the Status member is present and well-formed, its value is wrong.

Related open issue, which carries this check id but for a different (now-passed) symptom and so does not cover this cause: 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md — its replay key for this check is a _poll_until Timeout inside the helper that waits for an InService instance. In this replay the group does produce an InService instance and the check gets past that helper; the failure has moved to the refresh status progression described here. 20261107T081710Z-autoscaling-auto-scaling-not-found-deletion-protection-and-instance.md covers the wording of the CancelInstanceRefresh/StartInstanceRefresh refusal sentences, not the refresh lifecycle.

Traces: autoscaling.op-contract-cancelInstanceRefresh.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; the emulator is up and answering).

[mismatch] autoscaling.op-contract-cancelInstanceRefresh:
  File "check_autoscaling_op_contract_cancelInstanceRefresh.py", line 39, in run
    _poll_until(_probe, lambda s: s in ("InProgress",), timeout_s=60.0, interval_s=3.0)
  AssertionError: condition not met within 60.0s; last='Successful'

Expected (recording): after StartInstanceRefresh(AutoScalingGroupName=<group>, Preferences={MinHealthyPercentage:90, InstanceWarmup:120}), DescribeInstanceRefreshes(AutoScalingGroupName=<group>, InstanceRefreshIds=[<id>])['InstanceRefreshes'][0]['Status'] == 'InProgress'; CancelInstanceRefresh(AutoScalingGroupName=<group>) then returns InstanceRefreshId == <id> and the Status reaches 'Cancelled'; a further CancelInstanceRefresh is refused HTTP 400 / ActiveInstanceRefreshNotFound with 'No in progress, baking, pending, or cancelling Instance Refresh found for Auto Scaling group <group>.'

Actual: Status is 'Successful' on every poll from the first one onward, so none of the assertions after the wait is reached. (The refresh id, group name and the 60s poll budget are per-run values; the requirement is that a started refresh is observable in a non-terminal state and is cancellable.)

## Steps to Reproduce
1. Boot the emulator. In EC2, create a launch template with a resolvable ImageId and InstanceType=t3.micro.
2. CreateAutoScalingGroup with that LaunchTemplate, MinSize=0/MaxSize=2/DesiredCapacity=1 and a VPCZoneIdentifier subnet; wait until DescribeAutoScalingGroups reports one instance with LifecycleState='InService'.
3. StartInstanceRefresh(AutoScalingGroupName=<group>, Preferences={'MinHealthyPercentage': 90, 'InstanceWarmup': 120}) and note the returned InstanceRefreshId.
4. Immediately call DescribeInstanceRefreshes(AutoScalingGroupName=<group>, InstanceRefreshIds=[<id>]) and read InstanceRefreshes[0].Status. Observe 'Successful' on the first call (AWS reports Pending/InProgress until the replacement finishes).
5. Call CancelInstanceRefresh(AutoScalingGroupName=<group>): expect it to return the active InstanceRefreshId and the refresh to reach 'Cancelled'; observe instead the no-active-refresh refusal because the refresh is already terminal.
6. Call StartInstanceRefresh twice in a row: expect the second to be refused HTTP 400 InstanceRefreshInProgress; observe it accepted.
7. Re-replay autoscaling.op-contract-cancelInstanceRefresh to confirm a fix.

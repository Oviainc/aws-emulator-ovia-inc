Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asgeni-asg-min-above-max-rejected
Issue-Key: autoscaling__ec2.asgeni-asg-min-above-max-rejected|run#12|AssertionError|assert (code, message) == ("ValidationError",
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# UpdateAutoScalingGroup refuses an out-of-range DesiredCapacity with an emulator-authored message instead of AWS's "Desired capacity:N must be between the specified min size:N and max size:N"

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoScalingGroup

## Description
ONE net-new root cause on the Auto Scaling capacity-bound validation path (awsQuery, API version 2011-01-01), reached through the autoscaling__ec2 suite.

Cause 1 of 1: UpdateAutoScalingGroup rejects a DesiredCapacity outside the effective [MinSize, MaxSize] window with the right status and the right error code but a sentence AWS never sends.

AWS: HTTP 400, awsQuery ErrorResponse, Error.Type Sender, Error.Code ValidationError, Error.Message 'Desired capacity:<DesiredCapacity> must be between the specified min size:<MinSize> and max size:<MaxSize>'. The fixed text is 'Desired capacity:' + ' must be between the specified min size:' + ' and max size:' — note there is NO space after either colon; the only variable parts are the three integers, taken from the request (and from the stored group for any bound the request does not supply). The window used is the one the SAME request establishes, so raising MaxSize in the request counts, and the refusal applies no field at all. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model): UpdateAutoScalingGroupType.MinSize/MaxSize/DesiredCapacity target AutoScalingGroupMinSize / AutoScalingGroupMaxSize / AutoScalingGroupDesiredCapacity — plain integers with no modeled relational constraint — and UpdateAutoScalingGroup models only ResourceContentionFault, ScalingActivityInProgressFault and ServiceLinkedRoleFailure, so a bound-relation refusal is the service-common ValidationError and its sentence is recorded behavior only.

Emulator: status 400 and Error.Code ValidationError are correct, but Error.Message is an emulator-authored sentence of the form 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is <v> and the range is [<min>, <max>]' — member names instead of AWS's 'Desired capacity:' / 'min size:' / 'max size:' wording, and a bracketed-range form AWS never emits. (In the replayed check the values were 4, 0 and 3; those are that run's request values, not part of the requirement.) The sibling MinSize>MaxSize refusal in the same check answered AWS's exact sentence, so only the DesiredCapacity branch is at fault here.

Why: ValidationError is the single generic code for every Auto Scaling argument refusal, so the message is the only machine-visible statement of WHICH rule failed. A client, retry wrapper or log/audit scraper written against AWS — matching the fixed sentence, or parsing the three integers positionally out of it — never recognises this rejection, and cannot tell a desired-capacity violation from any other ValidationError.

Fix: the DesiredCapacity range check in the UpdateAutoScalingGroup handler's validation (shared with CreateAutoScalingGroup) — the message it raises; status and code already match.

Related: open issue 20261107T081710Z-autoscaling-auto-scaling-capacity-bound-refusals-6-net-new.md carries the same wording defect as its Cause 5, but grounded only in autoscaling.update-asg-capacity-bounds; it does not carry this check id, so this failure is not covered by it.

Traces: autoscaling__ec2.asgeni-asg-min-above-max-rejected [mismatch].

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (62/414 checks passed; the emulator is up and answering).

autoscaling__ec2.asgeni-asg-min-above-max-rejected [mismatch], check_autoscaling__ec2_asgeni_asg_min_above_max_rejected.py line 42:

  code, message = _error(lambda: asg.update_auto_scaling_group(
      AutoScalingGroupName=name, MinSize=0, MaxSize=3, DesiredCapacity=4))
  assert (code, message) == ("ValidationError",
      "Desired capacity:4 must be between the specified min size:0 and max size:3")

  AssertionError: DesiredCapacity=4 over a new MaxSize=3 answered ('ValidationError', 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 4 and the range is [0, 3]')

Expected (AWS): Error.Code 'ValidationError', Error.Message 'Desired capacity:4 must be between the specified min size:0 and max size:3'.
Actual (emulator): Error.Code 'ValidationError', Error.Message 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 4 and the range is [0, 3]'.
Fixed text of the required message: 'Desired capacity:', ' must be between the specified min size:', ' and max size:'. Variable parts: the requested DesiredCapacity (4 here) and the effective min/max of that request (0 and 3 here).
The earlier step of the same check — UpdateAutoScalingGroup MinSize=2 on a MaxSize=1 group expecting 'Max bound, 1, must be greater than or equal to min bound, 2' — passed, so the group's MinSize/MaxSize relational message is already right; only the DesiredCapacity window message diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a launch template and an Auto Scaling group with MinSize=0, MaxSize=1, DesiredCapacity=0 in a subnet of the run's network.
3. POST Action=UpdateAutoScalingGroup with AutoScalingGroupName=<group>, MinSize=0, MaxSize=3, DesiredCapacity=4.
   Expect HTTP 400, awsQuery Error.Code 'ValidationError', Error.Message 'Desired capacity:4 must be between the specified min size:0 and max size:3'.
   Observe the emulator's 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 4 and the range is [0, 3]'.
4. DescribeAutoScalingGroups must still report (MinSize, MaxSize, DesiredCapacity) = (0, 1, 0) — the refused combined update applies neither field.
5. Then UpdateAutoScalingGroup with MinSize=2, MaxSize=3, DesiredCapacity=2 must succeed and read back (2, 3, 2).
6. Re-replay autoscaling__ec2.asgeni-asg-min-above-max-rejected to confirm.

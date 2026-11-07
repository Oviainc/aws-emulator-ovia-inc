Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.asg-min-size-exceeds-max-size-rejected autoscaling.asg-minsize-greater-than-maxsize-rejected autoscaling.capacity-min-exceeds-max-rejected autoscaling.create-auto-scaling-group-min-exceeds-max-rejected autoscaling.op-contract-update-auto-scaling-group autoscaling.update-asg-min-above-max-fails autoscaling.create-asg-negative-min-size-rejected autoscaling.set-desired-capacity-above-max-size-rejected-and-no-instance-is-launched autoscaling.set-desired-capacity-exceeds-max-fails autoscaling.set-desired-capacity-below-min-rejected autoscaling.set-desired-capacity-negative-value-rejected autoscaling.update-asg-capacity-bounds autoscaling.asg-capacity-large-numeric-precision-preserved autoscaling.capacity-numeric-boundary-values-preserved
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.asg-min-size-exceeds-max-size-rejected|run#11|AssertionError|assert message == "Max bound, 2, must be greater than or equal to min bound, 5", f"expected the exact refusal message, got {message!r}"
Issue-Key: autoscaling.asg-minsize-greater-than-maxsize-rejected|run#11|AssertionError|assert message == "Max bound, 2, must be greater than or equal to min bound, 5", f"expected the exact refusal message, got {message!r}"
Issue-Key: autoscaling.capacity-min-exceeds-max-rejected|run#12|AssertionError|assert message == "Max bound, 1, must be greater than or equal to min bound, 5", f"expected the exact refusal message, got {message!r}"
Issue-Key: autoscaling.create-auto-scaling-group-min-exceeds-max-rejected|run#15|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: autoscaling.op-contract-update-auto-scaling-group|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: autoscaling.update-asg-min-above-max-fails|run#15|AssertionError|assert message == "Max bound, 2, must be greater than or equal to min bound, 5", (
Issue-Key: autoscaling.create-asg-negative-min-size-rejected|run#11|AssertionError|assert message == "Min bound, -1, must be non-negative", (
Issue-Key: autoscaling.set-desired-capacity-above-max-size-rejected-and-no-instance-is-launched|run#10|AssertionError|assert message == (
Issue-Key: autoscaling.set-desired-capacity-exceeds-max-fails|run#15|AssertionError|assert message == (
Issue-Key: autoscaling.set-desired-capacity-below-min-rejected|run#16|AssertionError|assert message == (
Issue-Key: autoscaling.set-desired-capacity-negative-value-rejected|run#13|AssertionError|assert message == "New SetDesiredCapacity value -1 is negative.", (
Issue-Key: autoscaling.update-asg-capacity-bounds|run#17|AssertionError|assert message == "Desired capacity:10 must be between the specified min size:1 and max size:5", (
Issue-Key: auto-scaling|CreateAutoScalingGroup|LimitExceeded
Issue-Commit: 38b869ac761e6c1ceaf9cdd20433d5027a2449ee

# Auto Scaling capacity-bound refusals: 6 net-new causes — emulator-authored MinSize/MaxSize, MinSize<0, SetDesiredCapacity and DesiredCapacity messages, and a valid large MaxSize refused LimitExceeded

> _Note: this issue was identified against the code base at commit `38b869ac761e6c1ceaf9cdd20433d5027a2449ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
SIX net-new root causes on the Auto Scaling capacity-bound validation path (awsQuery, 2011-01-01). Each must be closed separately. Related open issue 20261107T064116Z-autoscaling-cloudtrail-updateautoscalinggroup-minsize-maxsize-refusal-uses-an-emulator-authored.md carries the SAME wording defect for UpdateAutoScalingGroup only and none of the check ids below; the remaining five causes are not in it or in 20261029T211352Z-autoscaling-auto-scaling-client-error-refusals-diverge-emulator-authored.md / 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md.

Cause 1 of 6: MinSize > MaxSize is refused with an emulator-authored sentence on BOTH CreateAutoScalingGroup and UpdateAutoScalingGroup.
AWS: HTTP 400, awsQuery ErrorResponse, Error.Type Sender, Error.Code ValidationError, Error.Message 'Max bound, <MaxSize>, must be greater than or equal to min bound, <MinSize>'. Fixed text is 'Max bound, ' + ', must be greater than or equal to min bound, '; the only variable parts are the two integers from the request, with the MAX named first. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model): CreateAutoScalingGroupType.MinSize/@MaxSize target AutoScalingGroupMinSize/AutoScalingGroupMaxSize (plain integers, no modeled relational constraint); modeled errors are AlreadyExistsFault, LimitExceededFault, ResourceContentionFault, ServiceLinkedRoleFailure — so a bound-relation refusal is the service-common ValidationError and its sentence is recorded behavior only.
Emulator: correct status and code, but Error.Message 'The MinSize must be less than or equal to the MaxSize, but MinSize is <MinSize> and MaxSize is <MaxSize>' — different sentence, bounds named in the opposite order, member names instead of AWS's 'max bound'/'min bound' wording.
Why: Error.Code is the generic ValidationError for every Auto Scaling argument refusal, so the message is the only machine-visible statement of WHICH rule failed; a client or log scraper matching AWS's sentence (or parsing the bounds positionally out of it) never recognises the rejection.
Fix: the shared capacity-bound validation used by the CreateAutoScalingGroup and UpdateAutoScalingGroup handlers — the message it raises for the MinSize/MaxSize relational check.
Traces: autoscaling.asg-min-size-exceeds-max-size-rejected, autoscaling.asg-minsize-greater-than-maxsize-rejected, autoscaling.capacity-min-exceeds-max-rejected, autoscaling.create-auto-scaling-group-min-exceeds-max-rejected, autoscaling.op-contract-update-auto-scaling-group, autoscaling.update-asg-min-above-max-fails.

Cause 2 of 6: a negative MinSize is refused with the wrong sentence.
AWS: HTTP 400 / ValidationError with Error.Message 'Min bound, <MinSize>, must be non-negative' — fixed text 'Min bound, ' + ', must be non-negative', the single variable part being the negative integer from the request. Recorded behavior only (no modeled range constraint on AutoScalingGroupMinSize).
Emulator: 'The MinSize must not be negative, but it is <MinSize>'.
Why: same as cause 1 — the only discriminating field is the text, and it is text AWS never sends.
Fix: the negative-bound branch of CreateAutoScalingGroup's capacity validation.
Traces: autoscaling.create-asg-negative-min-size-rejected.

Cause 3 of 6: SetDesiredCapacity's out-of-range refusals append the group name to AWS's sentence.
AWS: HTTP 400 / ValidationError with Error.Message 'New SetDesiredCapacity value <value> is above max value <MaxSize> for the AutoScalingGroup.' and, below the minimum, 'New SetDesiredCapacity value <value> is below min value <MinSize> for the AutoScalingGroup.' The sentence ENDS with 'for the AutoScalingGroup.' — the group name is NOT part of it; only the requested value and the group's stored bound are variable. Smithy: SetDesiredCapacity models only ScalingActivityInProgressFault and ResourceContentionFault, so this is the common ValidationError; sentence is recorded behavior only.
Emulator: '... for the AutoScalingGroup: <group-name>' — AWS's terminating period is replaced by ': ' plus the group name.
Why: byte-exact message comparison is how callers and audit pipelines identify this refusal; appending a per-run group name makes every occurrence unmatchable.
Fix: the message builder in the SetDesiredCapacity handler's bound check (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/set_desired_capacity.rs).
Traces: autoscaling.set-desired-capacity-above-max-size-rejected-and-no-instance-is-launched, autoscaling.set-desired-capacity-exceeds-max-fails, autoscaling.set-desired-capacity-below-min-rejected.

Cause 4 of 6: a negative SetDesiredCapacity is reported as a below-minimum violation instead of as a negative value.
AWS: HTTP 400 / ValidationError with Error.Message 'New SetDesiredCapacity value <value> is negative.' — a distinct sentence for the negative case (fixed text 'New SetDesiredCapacity value ' + ' is negative.'), not the min-bound sentence.
Emulator: 'New SetDesiredCapacity value -1 is below min value 0 for the AutoScalingGroup: <group-name>' — it routes the negative input through the min-bound branch (and also carries cause 3's suffix).
Why: the caller is told the group's minimum is the obstacle, so a client that would correct a sign error instead tries to raise MinSize.
Fix: a negative-value branch ahead of the min-bound comparison in the SetDesiredCapacity handler's validation.
Traces: autoscaling.set-desired-capacity-negative-value-rejected.

Cause 5 of 6: a DesiredCapacity outside [MinSize, MaxSize] on Create/Update is refused with an emulator-authored sentence.
AWS: HTTP 400 / ValidationError with Error.Message 'Desired capacity:<DesiredCapacity> must be between the specified min size:<MinSize> and max size:<MaxSize>' — fixed text 'Desired capacity:' + ' must be between the specified min size:' + ' and max size:' with no spaces after the colons; the three integers come from the request/stored group. Recorded behavior only.
Emulator: 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is <v> and the range is [<min>, <max>]'.
Why: same as cause 1; additionally the emulator's bracketed-range form cannot be parsed by anything written against AWS.
Fix: the DesiredCapacity range check shared by the CreateAutoScalingGroup and UpdateAutoScalingGroup handlers.
Traces: autoscaling.update-asg-capacity-bounds.

Cause 6 of 6: a large but legal MaxSize is refused LimitExceeded, so the group is never created.
AWS: CreateAutoScalingGroup with MinSize=0, DesiredCapacity=0 and a very large MaxSize is ACCEPTED (HTTP 200) and DescribeAutoScalingGroups round-trips MaxSize unchanged as a plain integer; the per-group instance quota is evaluated against the capacity actually asked for, not against MaxSize at create time. Recorded behavior only (Smithy puts no @range on AutoScalingGroupMaxSize; LimitExceededFault is modeled for exceeding account resource limits, which a zero-desired group does not do).
Emulator: HTTP 400 LimitExceeded, Error.Message 'The group size <MaxSize> exceeds the limit of 1000 instances per Auto Scaling group', so the create fails outright.
Why: a legitimate request is refused — any group whose MaxSize is a large headroom value cannot be created at all, and the representation/round-trip property (no overflow, int not float) can never be observed.
Fix: the group-size limit check in the CreateAutoScalingGroup handler's validation (bound it to the requested/desired capacity, not MaxSize, and keep MaxSize stored verbatim).
Traces: autoscaling.asg-capacity-large-numeric-precision-preserved, autoscaling.capacity-numeric-boundary-values-preserved.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 checks passed; the emulator is up and answering).

Cause 1 — autoscaling.asg-min-size-exceeds-max-size-rejected [mismatch]: assert message == 'Max bound, 2, must be greater than or equal to min bound, 5' -> AssertionError: expected the exact refusal message, got 'The MinSize must be less than or equal to the MaxSize, but MinSize is 5 and MaxSize is 2'. Same text from autoscaling.asg-minsize-greater-than-maxsize-rejected; autoscaling.capacity-min-exceeds-max-rejected (expected 'Max bound, 1, must be greater than or equal to min bound, 5'); autoscaling.create-auto-scaling-group-min-exceeds-max-rejected (expected 'Max bound, 1, must be greater than or equal to min bound, 2', HTTP 400 asserted and correct); autoscaling.op-contract-update-auto-scaling-group (expected 'Max bound, 4, must be greater than or equal to min bound, 10'); autoscaling.update-asg-min-above-max-fails. The integers in these quotes are each run's request values; the fixed text and the max-first ordering are the requirement.

Cause 2 — autoscaling.create-asg-negative-min-size-rejected [mismatch]: assert message == 'Min bound, -1, must be non-negative' -> got 'The MinSize must not be negative, but it is -1'. Code ValidationError asserted and correct.

Cause 3 — autoscaling.set-desired-capacity-above-max-size-rejected-and-no-instance-is-launched: expected 'New SetDesiredCapacity value 5 is above max value 2 for the AutoScalingGroup.', got '... for the AutoScalingGroup: sim2real-behav-as-grp-3f8f181672dc'. autoscaling.set-desired-capacity-exceeds-max-fails: identical shape. autoscaling.set-desired-capacity-below-min-rejected: expected 'New SetDesiredCapacity value 0 is below min value 1 for the AutoScalingGroup.', got '... for the AutoScalingGroup: sim2real-behav-as-grp-8c1cd1d59d36'. The group names are per-run values; the terminating 'for the AutoScalingGroup.' is the fixed text.

Cause 4 — autoscaling.set-desired-capacity-negative-value-rejected: assert message == 'New SetDesiredCapacity value -1 is negative.' -> got 'New SetDesiredCapacity value -1 is below min value 0 for the AutoScalingGroup: sim2real-behav-asg-00fc6fc9be02'.

Cause 5 — autoscaling.update-asg-capacity-bounds: assert message == 'Desired capacity:10 must be between the specified min size:1 and max size:5' -> got 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 10 and the range is [1, 5]'.

Cause 6 — autoscaling.asg-capacity-large-numeric-precision-preserved [error]: CreateAutoScalingGroup(MinSize=0, MaxSize=999999999, DesiredCapacity=0, LaunchTemplate=<valid>) raised botocore.errorfactory.LimitExceededFault: An error occurred (LimitExceeded) ... 'The group size 999999999 exceeds the limit of 1000 instances per Auto Scaling group'. The check then expects DescribeAutoScalingGroups to echo MaxSize=999999999 as an int. autoscaling.capacity-numeric-boundary-values-preserved is the same refusal with MaxSize=2147483647. In both the recording the check was written from, the create SUCCEEDS.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral checks listed in trace_ids.
2. By hand, with a valid launch template and the run's subnets:
   a. POST Action=CreateAutoScalingGroup with MinSize=5, MaxSize=2 -> expect 400/ValidationError 'Max bound, 2, must be greater than or equal to min bound, 5'; same for Action=UpdateAutoScalingGroup with MinSize=10 on a MaxSize=4 group ('Max bound, 4, must be greater than or equal to min bound, 10').
   b. POST Action=CreateAutoScalingGroup with MinSize=-1, MaxSize=3 -> expect 'Min bound, -1, must be non-negative'.
   c. Create a group with MinSize=1, MaxSize=2, DesiredCapacity=1, then Action=SetDesiredCapacity DesiredCapacity=5 -> expect 'New SetDesiredCapacity value 5 is above max value 2 for the AutoScalingGroup.'; DesiredCapacity=0 -> '... is below min value 1 for the AutoScalingGroup.'; DesiredCapacity=-1 -> 'New SetDesiredCapacity value -1 is negative.'
   d. Action=UpdateAutoScalingGroup DesiredCapacity=10 on a group with MinSize=1/MaxSize=5 -> expect 'Desired capacity:10 must be between the specified min size:1 and max size:5'.
   e. POST Action=CreateAutoScalingGroup with MinSize=0, DesiredCapacity=0, MaxSize=999999999 -> expect HTTP 200, then DescribeAutoScalingGroups to report MaxSize 999999999.
3. Confirm every refused call left the group unchanged / uncreated (it does today), so only the status/code/message are at fault in causes 1-5 and only the acceptance in cause 6.

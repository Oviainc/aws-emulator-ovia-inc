Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters
Issue-Key: autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters|run#32|AssertionError|assert bounds_message == _BOUNDS_SDK_MESSAGE, (
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# UpdateAutoScalingGroup MinSize>MaxSize refusal uses an emulator-authored sentence instead of AWS's "Max bound, <MaxSize>, must be greater than or equal to min bound, <MinSize>"

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAutoScalingGroup

## Description
ONE net-new root cause on the Auto Scaling (awsQuery, 2011-01-01) write path. The behavioral check was listed under the suite heading `autoscaling__cloudtrail`, but the divergence is entirely in the Auto Scaling UpdateAutoScalingGroup refusal it asserts on; the CloudTrail half of the check is never reached because the Auto Scaling assertion fails first.

Cause 1 of 1: UpdateAutoScalingGroup with inverted capacity bounds (MinSize greater than MaxSize) is refused with the correct status and error code but with a message the emulator authored itself.

AWS: HTTP 400, awsQuery ErrorResponse with Error.Type Sender and Error.Code ValidationError, and Error.Message of the form 'Max bound, <MaxSize>, must be greater than or equal to min bound, <MinSize>'. The fixed text is 'Max bound, ' + ', must be greater than or equal to min bound, '; the ONLY variable parts are the two integers taken straight from the request (the MaxSize value, then the MinSize value, in that order — the max is named FIRST). Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): UpdateAutoScalingGroupType.MinSize targets AutoScalingGroupMinSize and .MaxSize targets AutoScalingGroupMaxSize (both plain integers, no modeled relational constraint), and the operation's only modeled errors are ScalingActivityInProgressFault (400), ResourceContentionFault (500) and ServiceLinkedRoleFailure (500) — so the bound-relation refusal is the service-common ValidationError (awsQuery code ValidationError, Type Sender) and its sentence is recorded behavior only.

Emulator: same HTTP status and the same Error.Code ValidationError (the check's code assertion passes), but Error.Message is 'The MinSize must be less than or equal to the MaxSize, but MinSize is <MinSize> and MaxSize is <MaxSize>' — a completely different sentence: it reverses the order the two bounds are named, uses member names rather than AWS's 'max bound'/'min bound' wording, and restates the rule as a clause instead of AWS's single assertion.

Why: Error.Code here is the generic ValidationError that Auto Scaling uses for every argument refusal, so the message is the only machine-visible explanation of WHICH rule was violated. A client, test or operator log scraper that matches AWS's sentence (or parses the two bounds out of it positionally) sees a sentence AWS never sends and treats a well-understood capacity-bound rejection as an unknown failure. Nothing in AWS's sentence is per-run except the two request integers, so the mismatch is permanent. The same divergence also costs the check its downstream assertion: the audited errorMessage CloudTrail records for this rejected call is required to be byte-identical to the SDK's errorMessage, so an emulator-authored sentence propagates into the audit record too.

Fix: the UpdateAutoScalingGroup handler's capacity-bound validation in the Auto Scaling service crate — the message text it raises for the MinSize/MaxSize relational check (and the shared bound-check message builder, if CreateAutoScalingGroup is made to share it). Status and code are already right; only the sentence must be reproduced as AWS words it.

Related but not covering: open issue 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md Cause 2 owns the fact that CreateAutoScalingGroup never performs the MinSize<=MaxSize check at all (it answers with the launch-source complaint instead) — a different operation and a different failure mode (missing check vs. wrong wording), and this behavioral check is not among its replay keys. Open issues 20261029T211352Z-autoscaling-auto-scaling-client-error-refusals-diverge-emulator-authored.md and 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md enumerate other Auto Scaling refusal wordings (launch-source, deletion-protection, filter, health-check-type, group-not-found, launch-template identity); neither lists the MinSize/MaxSize bound message for UpdateAutoScalingGroup.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator up: 92/145 checks passed).

Check autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters, verdict mismatch, failing at check_autoscaling__cloudtrail_obs_autoscaling_3_... .py line 114:

  AssertionError: MinSize 3 > MaxSize 1 raised message 'The MinSize must be less than or equal to the MaxSize, but MinSize is 3 and MaxSize is 1', expected 'Max bound, 1, must be greater than or equal to min bound, 3'

The asserted call is `asg.update_auto_scaling_group(AutoScalingGroupName=<group>, MinSize=3, MaxSize=1)` on a group previously created with MinSize=0, MaxSize=1, DesiredCapacity=0. The preceding assertions in the same check PASS: the rejected update's Error.Code is 'ValidationError' as AWS serves it, and the duplicate-name CreateAutoScalingGroup refusal already matches AWS ('AlreadyExists' / 'AutoScalingGroup by this name already exists - A group with the name <name> already exists'). So status and code are correct and only the bound message's text diverges.

Requirement vs per-run values: 'Max bound, ' and ', must be greater than or equal to min bound, ' are the fixed text and the ordering (max first, min second) is part of it; the integers 1 and 3 in the quoted strings are this run's MaxSize and MinSize request values and are NOT part of the requirement. The group name, the launch-template name and the RequestId in this check are per-run values.

Smithy grounding: UpdateAutoScalingGroup — input UpdateAutoScalingGroupType (MinSize -> AutoScalingGroupMinSize, MaxSize -> AutoScalingGroupMaxSize, both optional integers), output Unit, modeled errors ScalingActivityInProgressFault (awsQueryError ScalingActivityInProgress, 400), ResourceContentionFault (500), ServiceLinkedRoleFailure (500); no modeled error covers inverted bounds, hence the service-common ValidationError.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite check autoscaling__cloudtrail.obs-autoscaling-3-rejected-asg-calls-are-audited-with-their-sdk-error-and-request-parameters.
2. Or by hand: create an Auto Scaling group with a valid launch template and MinSize=0, MaxSize=1, DesiredCapacity=0.
3. POST Action=UpdateAutoScalingGroup with AutoScalingGroupName=<that group>, MinSize=3, MaxSize=1.
4. Observe HTTP 400 with Error.Code ValidationError (correct) and Error.Message 'The MinSize must be less than or equal to the MaxSize, but MinSize is 3 and MaxSize is 1'. Expected AWS message: 'Max bound, 1, must be greater than or equal to min bound, 3' — i.e. 'Max bound, <MaxSize>, must be greater than or equal to min bound, <MinSize>' with the two integers from the request.
5. Confirm the rejected update left the group unchanged (MinSize 0 / MaxSize 1 / DesiredCapacity 0) — it does, so only the message is at fault.
6. Re-replay the check above after the fix; note the check then goes on to assert that CloudTrail's recorded errorMessage for this call is byte-identical to the SDK's, so the corrected sentence must also reach the audit record.

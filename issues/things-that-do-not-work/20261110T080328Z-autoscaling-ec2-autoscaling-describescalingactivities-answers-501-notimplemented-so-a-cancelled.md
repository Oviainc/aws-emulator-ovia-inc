Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed
Issue-Key: autoscaling__ec2|DescribeScalingActivities|NotImplemented
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# autoscaling DescribeScalingActivities answers 501 NotImplemented, so a cancelled scale-out is never observable

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
One net-new root cause. Cause 1 of 1: DescribeScalingActivities is refused instead of answered.

AWS: Smithy com.amazonaws.autoscaling#DescribeScalingActivities takes DescribeScalingActivitiesType (ActivityIds, AutoScalingGroupName, IncludeDeletedGroups, MaxRecords, NextToken, Filters) and answers HTTP 200 with ActivitiesType{Activities (required), NextToken}; the only modeled errors are InvalidNextToken (awsQueryError InvalidNextToken, 400) and ResourceContentionFault (ResourceContention, 500). A scale-out attempt that could not launch — for example because the group's launch template was deleted — is still reported as an Activity with its StatusCode and Cause, which is the only way a client learns the launch was cancelled.

Emulator: HTTP 501, error code NotImplemented, message "operation DescribeScalingActivities is scaffolded but not implemented in this emulator". That message is entirely emulator-authored fixed text with no part taken from the request; neither the code nor the message is in the Smithy error set, and 501 is not a status this operation can produce.

Why: a client cannot tell a cancelled or failed scaling activity from a successful one, cannot page activity history, and cannot drive the standard "poll DescribeScalingActivities until the activity settles" loop at all — the call fails for every input, including the empty/no-filter read.

Fix: implement the operation handler in services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_scaling_activities.rs, serving activities from the group's scaling state and honouring ActivityIds/AutoScalingGroupName/MaxRecords/NextToken.

Related: the open issue 20261107T081710Z-autoscaling-describescalingactivities-stub.md names the same operation under the service token "autoscaling"; the failing check here is listed under the "autoscaling__ec2" heading and was not reported as owned by that issue, so it is filed separately and should be closed together with it.

Traces: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed.

## Evidence
Replay output for autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeScalingActivities operation: operation DescribeScalingActivities is scaffolded but not implemented in this emulator

Expected: HTTP 200 with ActivitiesType.Activities listing the group's scaling activities (ActivityId, AutoScalingGroupName, Cause, StatusCode), per Smithy com.amazonaws.autoscaling#DescribeScalingActivities whose only errors are InvalidNextToken and ResourceContentionFault.

## Steps to Reproduce
1. CreateLaunchTemplate and CreateAutoScalingGroup referencing it.
2. Delete the launch template, then raise DesiredCapacity so the group attempts a launch.
3. Call autoscaling:DescribeScalingActivities(AutoScalingGroupName=<group>). Expected 200 with Activities[]; the emulator answers 501 NotImplemented.
4. Re-replay autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed to confirm.

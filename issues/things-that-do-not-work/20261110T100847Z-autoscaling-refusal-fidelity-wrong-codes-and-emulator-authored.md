Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-capacity-bounds
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-capacity-bounds asg-err-launch-source-conflict asglf-subnet-placement-refusals asg-err-zone-placement asglf-lt-contents-refusals asglf-lt-identity-refusals asglf-lt-version-refusals asglf-mixed-instances-policy-refusals autoscaling-asgeni-traj-create-group-validates-template-network-interfaces autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals autoscaling-asghc-traj-describe-groups-and-instances-lookup-mistakes autoscaling-group-configuration-and-topology-negative autoscaling-group-dependent-operations-on-nonexistent-group
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|CreateAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|RollbackInstanceRefresh|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: autoscaling|RollbackInstanceRefresh|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|SetDesiredCapacity|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeAutoScalingGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DetachLoadBalancerTargetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md
Issue-Commit: 8f521f25a30f45aed4bf452c3a7727cefd2e1709

# autoscaling refusal fidelity: wrong codes and emulator-authored messages on six operations

> _Note: this issue was identified against the code base at commit `8f521f25a30f45aed4bf452c3a7727cefd2e1709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Six net-new root causes in the Auto Scaling client-error path. In each one the emulator does refuse the request with HTTP 400, but it either picks a different modeled error code than AWS or serves a sentence the service never emits, so a client that branches on the error code or parses the message gets a different answer than AWS gives. This bundle carries 6 causes and all 6 must be closed.

Cause 1 of 6: CreateAutoScalingGroup desired-capacity-out-of-bounds message is emulator-authored.
AWS: Smithy (com.amazonaws.autoscaling#CreateAutoScalingGroup, input members MinSize/MaxSize/DesiredCapacity on AutoScalingGroupMinSize/MaxSize/DesiredCapacity) models no validation fault for this; the awsQuery service-common ValidationError is what the recording shows: HTTP 400, Code 'ValidationError', Type 'Sender', Message 'Desired capacity:2 must be between the specified min size:0 and max size:1'. Fixed text of that template: 'Desired capacity:' + ' must be between the specified min size:' + ' and max size:'; the three numbers are the request's DesiredCapacity, MinSize and MaxSize. No group name appears.
Emulator: HTTP 400 with Message 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 2 and the range is [0, 1]' — a sentence of the emulator's own invention, naming the members instead of the values in AWS's phrasing.
Why: a client (or a test suite) that matches on AWS's sentence, or a human reading the refusal, sees a message AWS never produces; the member-name phrasing also hides which bound was violated in AWS's terms.
Fix: the CreateAutoScalingGroup handler's capacity-bounds validation, in the message it formats for the ValidationError it raises.
Traces: asg-err-capacity-bounds#task#9 (cluster also covers asg-err-launch-source-conflict#task#9, asglf-subnet-placement-refusals#task#6). Related but not covering: open issue 20261107T081710Z-autoscaling-auto-scaling-capacity-bound-refusals-6-net-new.md and 20261108T081736Z-autoscaling-ec2-updateautoscalinggroup-refuses-an-out-of-range-desiredcapacity-with.md — neither carries this cluster's replay key.

Cause 2 of 6: CreateAutoScalingGroup never checks that the subnets in VPCZoneIdentifier and the group's AvailabilityZones agree, and answers AlreadyExists instead.
AWS: VPCZoneIdentifier and AvailabilityZones must describe the same zones; when they do not, CreateAutoScalingGroup is refused HTTP 400 Code 'ValidationError' with the fixed sentence 'The availability zones of the specified subnets and the Auto Scaling group do not match' (no variable parts). Smithy: CreateAutoScalingGroup models AlreadyExistsFault (awsQueryError code 'AlreadyExists') only for a name that is already taken, and AlreadyExistsFault is documented 'You already have an Auto Scaling group or launch configuration with this name.'
Emulator: HTTP 400 Code 'AlreadyExists', Message 'AutoScalingGroup by this name already exists - A group with the name <group-name> already exists'. The zone/subnet consistency check is never run; the name collides because an earlier create in the same fixture that AWS refuses was accepted by the emulator, so every later attempt under that name reports a name conflict rather than the request defect.
Why: the client is told the wrong thing twice over — the request's real defect (mismatched zones) is never reported, and the code it does get, AlreadyExists, means a different, non-retryable condition. Retry-and-rename logic keyed on AlreadyExists loops forever against a request AWS would simply reject.
Fix: CreateAutoScalingGroup's validation (zone/subnet agreement must be checked and must run before the name-collision check), plus the handler's error selection so the request defect wins over the collision.
Traces: asg-err-zone-placement#task#8 (cluster of 15, including asglf-mixed-instances-policy-refusals#task#7..#14, asglf-lt-identity-refusals#task#7..#9, asglf-lt-contents-refusals#task#17/#19, autoscaling-asgeni-traj-create-group-validates-template-network-interfaces#task#8). Related but not covering: 20261107T081710Z-autoscaling-auto-scaling-accepts-requests-aws-refuses-8-net.md.

Cause 3 of 6: RollbackInstanceRefresh picks ActiveInstanceRefreshNotFound where AWS raises IrreversibleInstanceRefresh.
AWS: Smithy models both on RollbackInstanceRefresh — IrreversibleInstanceRefreshFault (awsQueryError 'IrreversibleInstanceRefresh', 400), documented 'The request failed because a desired configuration was not found or an incompatible launch template (uses a Systems Manager parameter instead of an AMI ID) or launch template version ($Latest or $Default) is present on the Auto Scaling group', and ActiveInstanceRefreshNotFoundFault ('ActiveInstanceRefreshNotFound', 400), documented 'The request failed because an active instance refresh or rollback for the specified Auto Scaling group was not found'. A group referencing its launch template by '$Latest' or '$Default' is the first case: HTTP 400, Code 'IrreversibleInstanceRefresh', Message "This Auto Scaling group's launch template version isn't compatible with a rollback. Rollbacks are not supported when the Auto Scaling group uses the launch template's '$Latest' or '$Default' version." — that sentence is entirely fixed text, with no value taken from the request.
Emulator: HTTP 400, Code 'ActiveInstanceRefreshNotFound', Message 'No in progress or baking Instance Refresh found for Auto Scaling group <group-name>.' — it decides the refusal purely from refresh presence and never evaluates rollback compatibility.
Why: the two codes mean opposite things to a caller. IrreversibleInstanceRefresh says 'this group can never be rolled back, fix the launch template reference'; ActiveInstanceRefreshNotFound says 'nothing is running right now, start one and retry'. A client is steered into a retry loop that cannot succeed.
Fix: the RollbackInstanceRefresh handler — the launch-template-version compatibility gate ('$Latest'/'$Default', SSM-parameter ImageId, missing desired configuration) must be evaluated and must raise IrreversibleInstanceRefreshFault with AWS's sentence.
Traces: autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#8. Related but not covering: 20261107T081710Z-autoscaling-auto-scaling-not-found-deletion-protection-and-instance.md and 20261109T172356Z-autoscaling-auto-scaling-3-net-new-causes-a-desiredcapacitytype.md.

Cause 4 of 6: SetDesiredCapacity's below-min refusal appends the group name and loses AWS's terminating period.
AWS: recorded behavior for the service-common ValidationError on SetDesiredCapacity: HTTP 400, Message 'New SetDesiredCapacity value 0 is below min value 1 for the AutoScalingGroup.' Fixed text: 'New SetDesiredCapacity value ' + ' is below min value ' + ' for the AutoScalingGroup.' — the two numbers are the request's DesiredCapacity and the group's stored MinSize; the sentence ends at 'AutoScalingGroup.' and the group name is NOT part of it. Smithy: SetDesiredCapacity models only ScalingActivityInProgressFault and ResourceContentionFault, so the bound check is the common ValidationError.
Emulator: Message 'New SetDesiredCapacity value 0 is below min value 1 for the AutoScalingGroup: sim2real-probe-asgdx-1674bf44' — it replaces the period with ': ' and the group name.
Why: the message is part of the API contract; appending state AWS omits breaks exact-match assertions and makes the refusal non-deterministic across groups.
Fix: the SetDesiredCapacity handler's capacity validation, in the message template it formats.
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#12. Related but not covering: 20261107T075601Z-autoscaling-ec2-events-sqs-asg-lifecycle-events-never-reach-eventbridge-sqs-and.md.

Cause 5 of 6: DescribeAutoScalingGroups' unsupported-filter refusal enumerates valid filter types instead of naming the unsupported criterion.
AWS: Smithy documents Filter.Name for DescribeAutoScalingGroups as accepting 'tag-key', 'tag-value' and 'tag:<key>'. When the filter asks to filter by group name, the recorded refusal is HTTP 400 ValidationError with "The filter criteria isn't valid. Filtering Auto Scaling groups by group name isn't supported." — a two-sentence message whose second sentence names the criterion that is not supported; the criterion is the variable part taken from the request's Filter.Name.
Emulator: "The filter criteria isn't valid. Valid filter types for the Name attribute of filters are: tag-key, tag-value and tag:<key>." — it keeps AWS's first sentence and substitutes an enumeration of the accepted names for AWS's statement about the rejected one.
Why: the caller is not told which filter was refused, only what is allowed, so a client that surfaces the message (or asserts on it) diverges from AWS for every unsupported filter name.
Fix: DescribeAutoScalingGroups' filter validation, in the message it formats for the rejected Filter.Name.
Traces: autoscaling-asghc-traj-describe-groups-and-instances-lookup-mistakes#task#8.

Cause 6 of 6: DetachLoadBalancerTargetGroups' group-not-found refusal interpolates the group name where AWS emits the literal 'null'.
AWS: Smithy models only ResourceContentionFault on DetachLoadBalancerTargetGroups, so an unknown AutoScalingGroupName is refused with the service-common ValidationError. Recorded behavior only: HTTP 400, Message 'AutoScalingGroup name not found - null'. For this operation the whole sentence is fixed text — AWS does not substitute the requested name into the clause after the dash, it emits the literal token 'null'.
Emulator: Message 'AutoScalingGroup name not found - AutoScalingGroup sim2real-probe-autoscaling-missing-asg not found' — it fills the second clause with its own 'AutoScalingGroup <name> not found' sentence.
Why: an exact-match assertion on AWS's refusal fails, and a client cannot distinguish this emulator's group-scoped not-found messages from AWS's, which are deliberately name-free here.
Fix: the DetachLoadBalancerTargetGroups handler's group-lookup refusal, in the ValidationError message it formats.
Traces: autoscaling-group-configuration-and-topology-negative#task#12 (cluster also covers autoscaling-group-dependent-operations-on-nonexistent-group#task#18).

## Evidence
Replay of trace set simparity-wire-aws-v1 (89 fixtures, 1044 steps, 647 passed, score 62.0%). Six unowned mismatch clusters, all HTTP 400 vs HTTP 400:

1) asg-err-capacity-bounds#task#9 — CreateAutoScalingGroup, 400 vs 400: $.ErrorResponse.Error.Message expected 'Desired capacity:2 must be between the specified min size:0 and max size:1', got 'The DesiredCapacity must be between the MinSize and the MaxSize, but DesiredCapacity is 2 and the range is [0, 1]' (3 steps affected).

2) asg-err-zone-placement#task#8 — CreateAutoScalingGroup, 400 vs 400: $.ErrorResponse.Error.Code expected 'ValidationError', got 'AlreadyExists'; $.ErrorResponse.Error.Message expected 'The availability zones of the specified subnets and the Auto Scaling group do not match', got 'AutoScalingGroup by this name already exists - A group with the name sim2real-probe-asgerr-3766b1ac-54ef-4735-baa6-ea9d80bd2163 already exists' (15 steps affected).

3) autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#8 — RollbackInstanceRefresh, 400 vs 400: Code expected 'IrreversibleInstanceRefresh', got 'ActiveInstanceRefreshNotFound'; Message expected "This Auto Scaling group's launch template version isn't compatible with a rollback. Rollbacks are not supported when the Auto Scaling group uses the launch template's '$Latest' or '$Default' version.", got 'No in progress or baking Instance Refresh found for Auto Scaling group sim2real-probe-asgdx-4795aad9.'

4) autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#12 — SetDesiredCapacity, 400 vs 400: Message expected 'New SetDesiredCapacity value 0 is below min value 1 for the AutoScalingGroup.', got 'New SetDesiredCapacity value 0 is below min value 1 for the AutoScalingGroup: sim2real-probe-asgdx-1674bf44'.

5) autoscaling-asghc-traj-describe-groups-and-instances-lookup-mistakes#task#8 — DescribeAutoScalingGroups, 400 vs 400: Message expected "The filter criteria isn't valid. Filtering Auto Scaling groups by group name isn't supported.", got "The filter criteria isn't valid. Valid filter types for the Name attribute of filters are: tag-key, tag-value and tag:<key>."

6) autoscaling-group-configuration-and-topology-negative#task#12 — DetachLoadBalancerTargetGroups, 400 vs 400: Message expected 'AutoScalingGroup name not found - null', got 'AutoScalingGroup name not found - AutoScalingGroup sim2real-probe-autoscaling-missing-asg not found' (2 steps affected).

Model evidence: Smithy com.amazonaws.autoscaling (2011-01-01) — CreateAutoScalingGroup errors AlreadyExistsFault/LimitExceededFault/ResourceContentionFault/ServiceLinkedRoleFailure (AlreadyExistsFault doc: 'You already have an Auto Scaling group or launch configuration with this name.'); RollbackInstanceRefresh errors IrreversibleInstanceRefreshFault (doc: incompatible launch template or '$Latest'/'$Default' version present) and ActiveInstanceRefreshNotFoundFault (doc: no active instance refresh or rollback found); SetDesiredCapacity errors ScalingActivityInProgressFault/ResourceContentionFault; DetachLoadBalancerTargetGroups errors ResourceContentionFault only; Filter.Name doc for DescribeAutoScalingGroups lists tag-key, tag-value, tag:<key>.

## Steps to Reproduce
Replay the bound wire corpus against the emulator (replay_wire_corpus) and inspect these steps:
1. asg-err-capacity-bounds step task#9 — CreateAutoScalingGroup with MinSize=0, MaxSize=1, DesiredCapacity=2; compare $.ErrorResponse.Error.Message.
2. asg-err-zone-placement step task#8 — CreateAutoScalingGroup whose VPCZoneIdentifier subnets are in zones other than the requested AvailabilityZones (the group name was already consumed by the preceding create the emulator wrongly accepted); compare $.ErrorResponse.Error.Code and .Message.
3. autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block step task#8 — RollbackInstanceRefresh on a group whose LaunchTemplate version is '$Latest'/'$Default'; compare Code and Message.
4. autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals step task#12 — SetDesiredCapacity 0 on a group with MinSize 1; compare Message.
5. autoscaling-asghc-traj-describe-groups-and-instances-lookup-mistakes step task#8 — DescribeAutoScalingGroups with a Filter naming the group name; compare Message.
6. autoscaling-group-configuration-and-topology-negative step task#12 — DetachLoadBalancerTargetGroups against a nonexistent AutoScalingGroupName; compare Message.

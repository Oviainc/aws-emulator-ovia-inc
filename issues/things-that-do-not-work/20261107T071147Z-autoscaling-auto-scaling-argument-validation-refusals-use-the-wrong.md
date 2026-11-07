Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-target-tracking-metric-type
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-target-tracking-metric-type autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals autoscaling-asgdx-traj-standby-park-and-return
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|PutScalingPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|StartInstanceRefresh|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|StartInstanceRefresh|4xx->2xx|status
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: autoscaling|EnterStandby|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 5e1f0b507a128238d75b745ce1d1dcae4dc5a977

# Auto Scaling argument-validation refusals use the wrong error code and wording, or are skipped entirely

> _Note: this issue was identified against the code base at commit `5e1f0b507a128238d75b745ce1d1dcae4dc5a977`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Four net-new root causes in the autoscaling request-validation path: one refusal message lists the wrong enum value set, one refusal is not raised at all, and two refusals are classified under an error code AWS never returns for an argument problem. Each cause must be closed separately.

Cause 1 of 4 - PutScalingPolicy's predefined-metric-type refusal lists an incomplete enum value set.
AWS: Smithy: PutScalingPolicy input TargetTrackingConfiguration.PredefinedMetricSpecification.PredefinedMetricType targets MetricType, whose modelled values are [ASGAverageCPUUtilization, ASGAverageNetworkIn, ASGAverageNetworkOut, ALBRequestCountPerTarget]; recorded behavior only for the live value set, which additionally accepts ASGAverageCPUUtilizationHighResolution. AWS refuses an unknown value with HTTP 400 / Error.Code ValidationError and the standard constraint sentence: fixed text "1 validation error detected: Value '" + the rejected value from the request + "' at 'targetTrackingConfiguration.predefinedMetricSpecification.predefinedMetricType' failed to satisfy constraint: Member must satisfy enum value set: [" + the service's full value list, in the service's order + "]".
Emulator: same status and code, but the bracketed list is the emulator's own four-value list in its own order ([ASGAverageCPUUtilization, ASGAverageNetworkIn, ASGAverageNetworkOut, ALBRequestCountPerTarget]), so ASGAverageCPUUtilizationHighResolution is missing and the order differs.
Why: the message is the only place AWS tells a caller which metric types exist; a client (or a human reading CLI output) is told that the high-resolution CPU metric is not a legal value, and will not retry with it.
Fix: the PutScalingPolicy validation's enum value list for predefined metric types (the catalog must include ASGAverageCPUUtilizationHighResolution and emit AWS's ordering). Related open issue 20261029T172317Z-autoscaling-auto-scaling-6-fidelity-divergences-putscalingpolicy-rejects-none.md covers other PutScalingPolicy validation rules, not this value set.
Traces: asg-err-target-tracking-metric-type#task#8.

Cause 2 of 4 - StartInstanceRefresh accepts AutoRollback=true with no DesiredConfiguration.
AWS: Smithy: StartInstanceRefresh input Preferences (RefreshPreferences) and DesiredConfiguration; RefreshPreferences.AutoRollback is documented as unsupported when "There is no desired configuration specified for the instance refresh". AWS refuses the combination with HTTP 400, Error.Type Sender, Error.Code ValidationError and the fixed message "The request isn't valid. The AutoRollback parameter cannot be set to true when the DesiredConfiguration parameter is empty. Set AutoRollback to false or specify a desired configuration and try again." (note the typographic apostrophe in "isn't" as AWS words it; the sentence has no variable parts).
Emulator: the request is accepted with HTTP 200 and a refresh is started, with no ErrorResponse at all.
Why: the caller believes a rollback-protected refresh is running; when it fails there is no rollback, and the refresh that AWS would never have started is now consuming the group's one in-progress refresh slot.
Fix: the StartInstanceRefresh handler's request validation (AutoRollback/DesiredConfiguration cross-check) before the refresh is created.
Traces: autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#12.

Cause 3 of 4 - TerminateInstanceInAutoScalingGroup's argument refusal is coded MalformedInput.
AWS: Smithy: TerminateInstanceInAutoScalingGroup input InstanceId (XmlStringMaxLen19), InstanceIds, AutoScalingGroupName, ShouldDecrementDesiredCapacity @required; its modelled errors are only ScalingActivityInProgress and ResourceContention, so an argument problem is reported through the awsQuery common client error: HTTP 400 with Error.Type Sender and Error.Code 'ValidationError'.
Emulator: HTTP 400 with Error.Code 'MalformedInput'.
Why: 'MalformedInput' is not a code AWS emits for this operation, so a client that branches on ValidationError (or on the SDK's modelled error set) treats an ordinary bad-argument refusal as an unknown/unretryable protocol failure.
Fix: the error classification used by TerminateInstanceInAutoScalingGroup's input validation (map argument failures onto the awsQuery ValidationError envelope).
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#10.

Cause 4 of 4 - EnterStandby's argument refusal is coded MalformedInput.
AWS: Smithy: EnterStandby input AutoScalingGroupName @required, InstanceIds, ShouldDecrementDesiredCapacity @required; modelled errors are only ResourceContention, so an argument problem comes back as HTTP 400, Error.Type Sender, Error.Code 'ValidationError'.
Emulator: HTTP 400 with Error.Code 'MalformedInput'.
Why: same as cause 3 - the standby workflow's error branch never matches, so a recoverable client mistake surfaces as an unrecognised failure.
Fix: the error classification in EnterStandby's input validation. Note the open stub issue 20261104T042502Z-autoscaling-enterstandby-stub.md concerns an unimplemented EnterStandby; here the operation does answer, with the wrong code.
Traces: autoscaling-asgdx-traj-standby-park-and-return#task#7.

## Evidence
Cause 1 (asg-err-target-tracking-metric-type#task#8, PutScalingPolicy, 400 vs 400): Error.Message expected "1 validation error detected: Value 'ASGBogusMetric' at 'targetTrackingConfiguration.predefinedMetricSpecification.predefinedMetricType' failed to satisfy constraint: Member must satisfy enum value set: [ASGAverageCPUUtilization, ASGAverageNetworkOut, ASGAverageNetworkIn, ASGAverageCPUUtilizationHighResolution, ALBRequestCountPerTarget]", got the same sentence with "[ASGAverageCPUUtilization, ASGAverageNetworkIn, ASGAverageNetworkOut, ALBRequestCountPerTarget]".
Cause 2 (autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#12, StartInstanceRefresh): expected 400 with Error {'Type': 'Sender', 'Code': 'ValidationError', 'Message': "The request isn't valid. The AutoRollback parameter cannot be set to true when the DesiredConfiguration parameter is empty. Set AutoRollback to false or specify a desired configuration and try again."}, got 200 and no ErrorResponse.
Cause 3 (autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#10, TerminateInstanceInAutoScalingGroup, 400 vs 400): Error.Code expected 'ValidationError', got 'MalformedInput'.
Cause 4 (autoscaling-asgdx-traj-standby-park-and-return#task#7, EnterStandby, 400 vs 400): Error.Code expected 'ValidationError', got 'MalformedInput'.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus and inspect: asg-err-target-tracking-metric-type#task#8 (PutScalingPolicy with PredefinedMetricType='ASGBogusMetric'), autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#12 (StartInstanceRefresh with Preferences.AutoRollback=true and no DesiredConfiguration), autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#10 (TerminateInstanceInAutoScalingGroup with an invalid argument), autoscaling-asgdx-traj-standby-park-and-return#task#7 (EnterStandby with an invalid argument).

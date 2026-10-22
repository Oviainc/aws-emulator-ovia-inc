Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-scaling-plan-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-scaling-plan-lifecycle
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].LastMutatingRequestTime
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].StatusMessage
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->2xx|$.ScalingPlanResources[*].ScalingPolicies
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->2xx|$.ScalingPlanResources[*].ScalingStatusMessage
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].ScalingInstructions[*].TargetTrackingConfigurations[*].TargetValue
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling-plans describe envelopes drop status and policy fields and report unapplied update state

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPlans

## Description
Across the scaling-plan lifecycle the emulator's read paths return 200 but under-populate the described plan: the status narrative on the plan, the generated scaling policies and status narrative on the resource, and the post-update instruction state are all wrong. This bundle has 3 causes; all three must be closed.

Cause 1 of 3: DescribeScalingPlans omits ScalingPlan.StatusMessage and LastMutatingRequestTime for an active plan.
AWS: ScalingPlan models StatusMessage (XmlString, doc "A simple message about the current status of the scaling plan.") alongside the required StatusCode, plus StatusStartTime and CreationTime (Smithy: shape ScalingPlan). LastMutatingRequestTime is not declared in the on-disk model, so it is recorded behavior only: the recording shows AWS returning it as an epoch-seconds number on each described plan. The recording shows an active plan carrying both a StatusMessage (example text: "Scaling plan has been created and applied to all resources.") and a LastMutatingRequestTime.
Emulator: HTTP 200, but $.ScalingPlans[0].StatusMessage and $.ScalingPlans[0].LastMutatingRequestTime are absent from the serialized plan entirely.
Why: StatusMessage is the documented place a client reads WHY a plan is in its current StatusCode - it is the field the model's own ScalingStatusCode/StatusCode docs tell callers to check. A console or script that renders plan status gets a blank narrative, and one that uses LastMutatingRequestTime to order or de-duplicate concurrent mutations has no timestamp to compare, so it cannot tell a stale read from a fresh one.
Fix: the DescribeScalingPlans response serializer / the plan record in the service's state: persist a status message and a last-mutating-request timestamp when a plan is created or mutated and emit both members on every described plan.
Traces: autoscalingplans-scaling-plan-lifecycle#task#4.

Cause 2 of 3: DescribeScalingPlanResources returns resources with no ScalingPolicies and no ScalingStatusMessage, so the plan's target-tracking policies are never materialized.
AWS: ScalingPlanResource models ScalingPolicies (list, doc "The scaling policies.") and ScalingStatusMessage (doc "A simple message about the current scaling status of the resource.") next to the required ScalingStatusCode (Smithy: shape ScalingPlanResource). ScalingInstruction's doc states AWS Auto Scaling CREATES target tracking scaling policies from the scaling instructions, and the recording bears that out: the described resource carries one policy whose PolicyType is TargetTrackingScaling and whose TargetTrackingConfiguration echoes the instruction's PredefinedScalingMetricSpecification and TargetValue, plus a ScalingStatusMessage (example text: "Target tracking scaling policies have been applied to the resource."). The generated PolicyName is an example only - it embeds a generated uuid and must not be asserted verbatim; the requirement is that one TargetTrackingScaling policy per target tracking configuration is present with the configuration echoed.
Emulator: HTTP 200, but $.ScalingPlanResources[0].ScalingPolicies and $.ScalingPlanResources[0].ScalingStatusMessage are both missing. The emulator records the plan's resource but never derives the policies from its scaling instructions.
Why: the whole point of a scaling plan is the policies it mints on the scalable resource. A client that creates a plan and then reads back its resources to confirm the policy exists - or to discover the policy name so it can be inspected or replaced - finds nothing, and cannot distinguish "policies applied" from "plan created but inert" because the status narrative is missing too.
Fix: the DescribeScalingPlanResources handler plus the scaling-plan state: on create/update, derive one TargetTrackingScaling policy per TargetTrackingConfiguration of each scaling instruction (echoing the predefined/customized metric specification and TargetValue) and store it on the resource record, then serialize ScalingPolicies and ScalingStatusMessage on every described resource.
Traces: autoscalingplans-scaling-plan-lifecycle#task#5.

Cause 3 of 3: after the lifecycle's UpdateScalingPlan step, DescribeScalingPlans reports a target-tracking TargetValue that does not match the recorded plan state, and again omits the status fields.
AWS: UpdateScalingPlan takes ScalingPlanName and ScalingPlanVersion as required and ScalingInstructions as optional, and models ObjectNotFoundException(400) and ValidationException(400) (Smithy: UpdateScalingPlan). The recording shows that after the fixture's update, the plan read back reports ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue = 50.0, and still reports LastMutatingRequestTime and StatusMessage. Which mutations an update does and does not apply to an existing plan's instructions is recorded behavior only.
Emulator: HTTP 200, but reports TargetValue 75.0 where the recording reports 50.0, and once more omits $.ScalingPlans[0].StatusMessage and $.ScalingPlans[0].LastMutatingRequestTime. Hypothesis: the emulator's update path overwrites the stored target tracking configuration with the request's value where AWS's recorded plan state retains the configuration established earlier in the lifecycle - i.e. the update is applied with different semantics than AWS applies it.
Why: a client that updates a plan and reads it back to confirm the applied configuration sees a different target value than AWS would report, so any convergence check or drift detector built on that read reaches the opposite conclusion against the emulator than against AWS.
Fix: the UpdateScalingPlan handler's state merge for ScalingInstructions / TargetTrackingConfigurations, so the post-update plan state matches the recorded behavior; the missing StatusMessage/LastMutatingRequestTime on this read share Cause 1's serializer fix.
Traces: autoscalingplans-scaling-plan-lifecycle#task#7.

## Evidence
Cause 1 - fixture autoscalingplans-scaling-plan-lifecycle, step task#4, DescribeScalingPlans. Expected vs actual (status matched at 200): $.ScalingPlans[0].LastMutatingRequestTime missing (expected a number, recorded example 1792338984.965, got None); $.ScalingPlans[0].StatusMessage missing (recorded example 'Scaling plan has been created and applied to all resources.', got None).

Cause 2 - fixture autoscalingplans-scaling-plan-lifecycle, step task#5, DescribeScalingPlanResources. Expected vs actual (status matched at 200): $.ScalingPlanResources[0].ScalingPolicies missing - expected one policy with PolicyType 'TargetTrackingScaling' and TargetTrackingConfiguration {PredefinedScalingMetricSpecification: {PredefinedScalingMetricType: 'DynamoDBReadCapacityUtilization'}, TargetValue: 50.0} (PolicyName in the recording embeds a generated uuid and is an example only), got None; $.ScalingPlanResources[0].ScalingStatusMessage missing (recorded example 'Target tracking scaling policies have been applied to the resource.'), got None.

Cause 3 - fixture autoscalingplans-scaling-plan-lifecycle, step task#7, DescribeScalingPlans. Expected vs actual (status matched at 200): $.ScalingPlans[0].ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue value_mismatch (expected 50.0, got 75.0); $.ScalingPlans[0].LastMutatingRequestTime missing; $.ScalingPlans[0].StatusMessage missing.

Model evidence: Smithy shape ScalingPlan declares StatusMessage, StatusStartTime, CreationTime (LastMutatingRequestTime is not in the on-disk model - recorded behavior only); shape ScalingPlanResource declares ScalingPolicies and ScalingStatusMessage; ScalingInstruction doc states AWS Auto Scaling creates target tracking scaling policies from the scaling instructions.

## Steps to Reproduce
1. Replay fixture autoscalingplans-scaling-plan-lifecycle against the emulator. It creates a DynamoDB-backed scaling plan with a target tracking configuration, then reads it back.
2. At step task#4 (DescribeScalingPlans) observe the described plan has no StatusMessage and no LastMutatingRequestTime.
3. At step task#5 (DescribeScalingPlanResources) observe the described resource has no ScalingPolicies list and no ScalingStatusMessage, i.e. no TargetTrackingScaling policy was derived from the plan's scaling instruction.
4. Let the fixture's update step run and stop at step task#7 (DescribeScalingPlans): observe ScalingInstructions[0].TargetTrackingConfigurations[0].TargetValue is 75.0 where the recording reports 50.0, and StatusMessage / LastMutatingRequestTime are missing again.

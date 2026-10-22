Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-create-scaling-plan-validation-envelopes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-create-scaling-plan-validation-envelopes autoscalingplans-absent-plan-read-and-write-envelopes
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|$.Message
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|$.__type
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|status
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->4xx|$.ScalingPlanResources
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->4xx|status
Issue-Commit: 6f223cb07d8f19a9e48b2b4f7aac2665ac919e98

# autoscaling-plans wrong status: CreateScalingPlan skips instruction validation and absent-plan resource reads reject

> _Note: this issue was identified against the code base at commit `6f223cb07d8f19a9e48b2b4f7aac2665ac919e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScalingPlan

## Description
The autoscaling-plans emulator returns the wrong HTTP status on two distinct paths: it accepts CreateScalingPlan input that AWS rejects, and it rejects a DescribeScalingPlanResources read that AWS answers successfully. This bundle has 2 causes; both must be closed.

Cause 1 of 2: CreateScalingPlan performs no semantic validation of ScalingInstructions and answers 200 for input AWS rejects.
AWS: CreateScalingPlan models ValidationException (client error, httpError 400) and the request shape requires ScalingInstructions, each ScalingInstruction requiring MinCapacity, MaxCapacity and TargetTrackingConfigurations (Smithy: CreateScalingPlan input ScalingPlanName/ApplicationSource/ScalingInstructions all @required; errors ValidationException(400), LimitExceededException(400); shape ScalingInstruction members MinCapacity, MaxCapacity (ResourceCapacity, integer, @required), TargetTrackingConfigurations @required; ValidationException doc: "An exception was thrown for a validation issue. Review the parameters provided."). The recorded responses for this fixture's six validation probes are HTTP 400 with __type ValidationException; the representative probe sends a scaling instruction whose MaxCapacity is below its MinCapacity and the recorded message is, as an example, "Maximum capacity cannot be less than minimum capacity". Smithy constrains member types but not the cross-member capacity relation, so the specific relations are recorded behavior only; the requirement taken from the recording is the 400/ValidationException envelope for semantically invalid scaling instructions.
Emulator: every one of the six probes returns HTTP 200 with a CreateScalingPlanResponse and no error body at all ($.__type missing, $.Message missing). The plan is created from input AWS would have refused.
Why: a client that relies on AWS validating the plan at creation time gets a success for a plan that cannot exist. Nothing surfaces the bad capacity bounds / missing or contradictory instruction fields, so SDK error handling and retry/repair logic is never exercised, and the emulator's state then contains a plan whose shape AWS can never produce — every later read of it is wrong too.
Fix: CreateScalingPlan's input validation, ahead of the handler's state write: validate each ScalingInstruction's semantics (at minimum MaxCapacity >= MinCapacity, plus the other relations the six probes exercise) and raise ValidationException serialized as HTTP 400 with __type ValidationException and a message naming the offending member/constraint, instead of falling through to the create path.
Traces: autoscalingplans-create-scaling-plan-validation-envelopes#task#3 (representative) and the same cluster's task#4, task#5, task#6, task#7, task#8 — six probes, one per invalid-input case.

Cause 2 of 2: DescribeScalingPlanResources on a plan that does not exist answers 400 instead of 200 with an empty resource list.
AWS: DescribeScalingPlanResources does NOT model a not-found error. Its modeled errors are ConcurrentUpdateException(500), InternalServiceException(500), InvalidNextTokenException(400) and ValidationException(400) — ObjectNotFoundException is modeled on DeleteScalingPlan and UpdateScalingPlan but deliberately not here (Smithy: DescribeScalingPlanResources errors; output ScalingPlanResources is a non-required list). The recording confirms the read of an absent plan name/version is HTTP 200 whose ScalingPlanResources is an empty list.
Emulator: HTTP 400, and the response carries no ScalingPlanResources member at all (expected [], got None).
Why: a client polling a plan's resources - the normal way to wait for a freshly created or just-deleted plan to settle - sees a terminal client error where AWS returns an empty page. Because no not-found error is modeled for this operation, SDK clients have no typed branch for it and the 400 surfaces as an unexpected failure rather than "no resources yet", so polling and teardown loops break.
Fix: the DescribeScalingPlanResources handler's lookup path: a miss on (ScalingPlanName, ScalingPlanVersion) must serialize a 200 response containing an empty ScalingPlanResources list rather than mapping the miss onto a 400 error.
Traces: autoscalingplans-absent-plan-read-and-write-envelopes#task#2.

## Evidence
Cause 1 - fixture autoscalingplans-create-scaling-plan-validation-envelopes, step task#3, CreateScalingPlan. Expected: HTTP 400, $.__type = 'ValidationException', $.Message present (recorded example: 'Maximum capacity cannot be less than minimum capacity'). Actual: HTTP 200, $.__type missing, $.Message missing. Same verdict for the cluster's 5 other members (task#4 .. task#8), affected_count 6.

Cause 2 - fixture autoscalingplans-absent-plan-read-and-write-envelopes, step task#2, DescribeScalingPlanResources. Expected: HTTP 200 with $.ScalingPlanResources = []. Actual: HTTP 400 with $.ScalingPlanResources missing (None).

Model evidence: Smithy DescribeScalingPlanResources errors = [ConcurrentUpdateException 500, InternalServiceException 500, InvalidNextTokenException 400, ValidationException 400] - no not-found error; CreateScalingPlan errors include ValidationException (client, httpError 400).

## Steps to Reproduce
1. Replay fixture autoscalingplans-create-scaling-plan-validation-envelopes against the emulator and stop at step task#3: CreateScalingPlan with a ScalingInstruction whose MaxCapacity is less than its MinCapacity. Observe HTTP 200 instead of HTTP 400 / ValidationException. Steps task#4 through task#8 are the other five invalid-input probes and fail identically.
2. Replay fixture autoscalingplans-absent-plan-read-and-write-envelopes and stop at step task#2: DescribeScalingPlanResources for a scaling plan name that was never created, version 1. Observe HTTP 400 with no ScalingPlanResources member instead of HTTP 200 with ScalingPlanResources = [].

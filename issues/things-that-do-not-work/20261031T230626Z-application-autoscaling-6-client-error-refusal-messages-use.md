Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected application-autoscaling.ecs-step-adjustment-without-any-bound-rejected application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected application-autoscaling.target-tracking-with-both-metric-specifications-rejected application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression application-autoscaling.scheduled-action-name-with-a-slash-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected|run#14|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-adjustment-without-any-bound-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.target-tracking-with-both-metric-specifications-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression|run#13|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.scheduled-action-name-with-a-slash-rejected|run#14|AssertionError|assert error.get('Message') == (
Issue-Commit: 025abd355378a9dd6de39ac3439e25eecad5e6c9

# application-autoscaling: 6 client-error refusal messages use emulator wording instead of AWS's (DeleteScheduledAction, PutScheduledAction, PutScalingPolicy)

> _Note: this issue was identified against the code base at commit `025abd355378a9dd6de39ac3439e25eecad5e6c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Six net-new root causes, all in the Application Auto Scaling frontend service: every one of them returns the correct HTTP status and error CODE but an emulator-authored MESSAGE, so clients (and CLI users) that match on AWS's sentence see a different string. The related open issue 20261029T175650Z-application-autoscaling-ecs-5-net-new-root-causes.md covers the ECS-namespace variants of two message defects; none of the six below is covered by it or by any other open issue.

Cause 1 of 6: DeleteScheduledAction for a scheduled action that does not exist.
AWS: ObjectNotFoundException (the modeled error for "the specified object could not be found") whose message names the SERVICE NAMESPACE, the SCALABLE TARGET ID and the SCHEDULED ACTION NAME: fixed text `No scheduled action found for service namespace: `, `, scalable target ID: `, `, scheduled action name: `; the variable parts are the request's ServiceNamespace, the target's id (the 32 hex digits that end the ScalableTargetARN returned by RegisterScalableTarget, hyphenated 8-4-4-4-12) and the request's ScheduledActionName. Recorded behavior only for the exact template (the Smithy model documents only the error class).
Emulator: same code, but the message is `No scheduled action registered for service namespace: <ns>, resource ID: <resourceId>, scalable dimension: <dimension>, scheduled action name: <name>` — "registered" instead of "found", and it identifies the target by ResourceId + ScalableDimension instead of by the scalable-target id.
Why: a client that parses the message for the target id, or asserts AWS's sentence, mis-handles the refusal.
Fix: the ObjectNotFoundException message construction in the DeleteScheduledAction handler of the application-autoscaling service.
Traces: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected

Cause 2 of 6: PutScalingPolicy with a StepScaling configuration whose single StepAdjustment has neither MetricIntervalLowerBound nor MetricIntervalUpperBound.
AWS: ValidationException with the fixed sentence `Both lower and upper bounds of a step adjustment cannot be left unspecified` (no variable parts). Smithy: StepAdjustment documents the rule "The upper and lower bound can't be null in the same step adjustment" as prose; the wire message is the sentence above (recorded behavior).
Emulator: ValidationException whose message is the documentation prose `The upper and lower bound can't be null in the same step adjustment`.
Why: the documentation sentence is not the API's message; message-matching clients diverge.
Fix: the step-adjustment validation of the PutScalingPolicy handler.
Traces: application-autoscaling.ecs-step-adjustment-without-any-bound-rejected

Cause 3 of 6: PutScalingPolicy with PolicyType=StepScaling and a StepScalingPolicyConfiguration carrying no StepAdjustments at all.
AWS: ValidationException with the fixed sentence `There must be at least one step adjustment` (no variable parts). Smithy: StepScalingPolicyConfiguration.StepAdjustments documents "At least one step adjustment is required if you are adding a new step scaling policy configuration" as prose; the wire message is the shorter sentence (recorded behavior).
Emulator: ValidationException whose message is the documentation prose `At least one step adjustment is required if you are adding a new step scaling policy configuration`.
Why: same as Cause 2 — the doc prose is emitted where AWS emits its own sentence.
Fix: the step-scaling validation of the PutScalingPolicy handler.
Traces: application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected

Cause 4 of 6: PutScalingPolicy with PolicyType=TargetTrackingScaling and BOTH PredefinedMetricSpecification and CustomizedMetricSpecification set.
AWS: ValidationException with the fixed sentence `Only one of PredefinedMetricSpecification or CustomizedMetricSpecification can be set for target tracking scaling.` (no variable parts; note the trailing period and the member names as modeled). Smithy: TargetTrackingScalingPolicyConfiguration members both document "You can specify either a predefined metric or a customized metric".
Emulator: ValidationException whose message is `You can specify either a predefined metric or a customized metric, but not both`.
Why: the message names neither modeled member, so a client cannot tell which field to drop, and exact-match assertions fail.
Fix: the target-tracking validation of the PutScalingPolicy handler.
Traces: application-autoscaling.target-tracking-with-both-metric-specifications-rejected

Cause 5 of 6: PutScheduledAction with an unparseable cron, e.g. Schedule='cron(not a cron)'.
AWS: ValidationException whose message is `Invalid schedule expression. Details: Invalid CRON expression: error at position <POSITION>, details: Illegal characters for this position: '<TOKEN>'` — the fixed text is `Invalid schedule expression. Details: Invalid CRON expression: error at position `, `, details: Illegal characters for this position: '`, `'`; the variable parts are the 0-based position in the cron body where parsing failed and the offending token as AWS reports it (upper-cased). Recorded behavior only for the template.
Emulator: ValidationException whose message is `Invalid schedule expression. Details: parse error at position 6: unexpected token 'not' in the Minutes field.` — a different detail grammar, a different position convention and a lower-cased token.
Why: a client that surfaces or parses the CRON detail shows text real AWS never emits, and the reported position points elsewhere.
Fix: the schedule-expression validation / its error-detail formatting in the PutScheduledAction handler.
Traces: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression

Cause 6 of 6: PutScheduledAction with a ScheduledActionName containing '/' (e.g. 'scale-up/weekday').
AWS: ValidationException with the standard constraint-violation sentence `1 validation error detected: Value '<VALUE>' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: <PATTERN>`, where <VALUE> is the rejected request value, the member path is the lower-camel wire name, and <PATTERN> is the modeled pattern WITHOUT its leading '^' and trailing '$' anchors, i.e. `(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+`. Smithy: ScheduledActionName has @length min 1 max 256 and @pattern `^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$`.
Emulator: same code and same sentence, but it quotes the modeled pattern verbatim INCLUDING the anchors (`^(?!...).+$`), so the reported pattern differs from AWS's by the leading '^' and trailing '$'.
Why: clients and test suites that compare the reported pattern (or re-compile it) against AWS's see a mismatch; this is the generic constraint-message formatter, so every @pattern member of this service reports the anchored form.
Fix: the pattern rendering in the shared constraint-violation message formatter used by this service's input validation (strip the anchors AWS strips), not the PutScheduledAction handler alone.
Traces: application-autoscaling.scheduled-action-name-with-a-slash-rejected

## Evidence
All six checks are [mismatch] in the behavioral suite; each asserts Code first (which matched) and then Message.

1) delete-scheduled-action-that-does-not-exist-rejected
   expected: "No scheduled action found for service namespace: dynamodb, scalable target ID: <8-4-4-4-12 uuid from ScalableTargetARN>, scheduled action name: <name>"
   actual:   "No scheduled action registered for service namespace: dynamodb, resource ID: table/<table>, scalable dimension: dynamodb:table:ReadCapacityUnits, scheduled action name: <name>"

2) ecs-step-adjustment-without-any-bound-rejected
   expected: "Both lower and upper bounds of a step adjustment cannot be left unspecified"
   actual:   "The upper and lower bound can't be null in the same step adjustment"

3) ecs-step-scaling-policy-without-step-adjustments-rejected
   expected: "There must be at least one step adjustment"
   actual:   "At least one step adjustment is required if you are adding a new step scaling policy configuration"

4) target-tracking-with-both-metric-specifications-rejected
   expected: "Only one of PredefinedMetricSpecification or CustomizedMetricSpecification can be set for target tracking scaling."
   actual:   "You can specify either a predefined metric or a customized metric, but not both"

5) put-scheduled-action-rejects-invalid-cron-expression (Schedule='cron(not a cron)')
   expected: "Invalid schedule expression. Details: Invalid CRON expression: error at position 0, details: Illegal characters for this position: 'NOT'"
   actual:   "Invalid schedule expression. Details: parse error at position 6: unexpected token 'not' in the Minutes field."

6) scheduled-action-name-with-a-slash-rejected (ScheduledActionName='scale-up/weekday')
   expected: "1 validation error detected: Value 'scale-up/weekday' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+"
   actual:   "1 validation error detected: Value 'scale-up/weekday' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$"

In every case the refusal itself was correct (right status, right error code) and the post-conditions the checks assert afterwards (nothing stored, bounds unchanged) were not reached because the message assertion fired first.

## Steps to Reproduce
Cause 1: create a PROVISIONED DynamoDB table, RegisterScalableTarget(ServiceNamespace=dynamodb, ResourceId=table/<t>, ScalableDimension=dynamodb:table:ReadCapacityUnits, Min=1, Max=10), PutScheduledAction for a real name, then DeleteScheduledAction with a name that was never put; compare Error.Message.
Cause 2: on a registered ecs:service:DesiredCount target, PutScalingPolicy(PolicyType=StepScaling, StepScalingPolicyConfiguration={AdjustmentType: ChangeInCapacity, StepAdjustments: [{ScalingAdjustment: 1}]}).
Cause 3: same target, PutScalingPolicy(PolicyType=StepScaling, StepScalingPolicyConfiguration={AdjustmentType: ChangeInCapacity}) with no StepAdjustments.
Cause 4: same target, PutScalingPolicy(PolicyType=TargetTrackingScaling, TargetTrackingScalingPolicyConfiguration={TargetValue: 60.0, PredefinedMetricSpecification: {...}, CustomizedMetricSpecification: {...}}).
Cause 5: on a registered dynamodb:table:ReadCapacityUnits target, PutScheduledAction(Schedule='cron(not a cron)').
Cause 6: on the same target, PutScheduledAction(ScheduledActionName='scale-up/weekday', Schedule='cron(0 6 ? * MON-FRI *)').
Each call must be refused with the AWS message quoted in Evidence.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.register-scalable-target-rejects-nonexistent-table application-autoscaling.register-missing-global-secondary-index-rejected application-autoscaling.register-on-demand-table-rejected application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected application-autoscaling.register-scalable-target-rejects-min-above-max application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression application-autoscaling.scheduled-action-name-with-a-slash-rejected application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected application-autoscaling.ecs-step-adjustment-without-any-bound-rejected application-autoscaling.target-tracking-with-both-metric-specifications-rejected application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.register-scalable-target-rejects-nonexistent-table|run#11|AssertionError|assert error.get('Message') == f'DynamoDB table does not exist: {absent_id}', (
Issue-Key: application-autoscaling.register-missing-global-secondary-index-rejected|run#11|AssertionError|assert error.get('Message') == f'DynamoDB index does not exist: {missing}', (
Issue-Key: application-autoscaling.register-on-demand-table-rejected|run#10|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected|run#10|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.register-scalable-target-rejects-min-above-max|run#18|AssertionError|assert error.get('Message') == expected_message, f"expected the message {expected_message!r}, got {error.get('Message')!r}"
Issue-Key: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression|run#13|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.scheduled-action-name-with-a-slash-rejected|run#14|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-adjustment-without-any-bound-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.target-tracking-with-both-metric-specifications-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected|run#14|AssertionError|assert (error.get('Code'), error.get('Message')) == (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Application Auto Scaling (application-autoscaling checks): refusal messages use a generic constraint template instead of the service's texts (6 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
The verdict and error code are right on these refusals; the Message is synthesized from a generic "Value 'X' at 'member' failed to satisfy constraint: Member must ..." template instead of Application Auto Scaling's own text. Each recorded message is built from the request (it names the table, index, bound or member), so the TEXT RULE is the requirement, not the recorded literal. Six independent families.

Cause 1 of 6: DynamoDB resource-id refusals on RegisterScalableTarget.
AWS: ValidationException (Smithy: RegisterScalableTarget errors include ValidationException, 400) with 'DynamoDB table does not exist: <resourceId>' for a missing table, and the service's own texts for a missing global secondary index, for an on-demand (PAY_PER_REQUEST) table, and for a table resource id paired with an index dimension.
Emulator: constraint-template variants, e.g. "Value 'table/<tbl>' at 'resourceId' failed to satisfy constraint: no DynamoDB table named '<tbl>' exists in us-west-2", "... has no global secondary index named 'by-mail'", "... is in PAY_PER_REQUEST billing mode and has no provisioned capacity to scale", "... Member must identify a index resource of the 'dynamodb' service namespace, of the form 'table/{table name}/index/{index name}'".
Why: clients that branch on or surface the refusal text see invented phrasing (and a leaked region) AWS never emits.
Fix: RegisterScalableTarget validation, DynamoDB resource-resolution error texts.
Traces: application-autoscaling.register-scalable-target-rejects-nonexistent-table, application-autoscaling.register-missing-global-secondary-index-rejected, application-autoscaling.register-on-demand-table-rejected, application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected

Cause 2 of 6: inverted capacity bounds.
AWS: ValidationException 'Maximum capacity cannot be less than minimum capacity'.
Emulator: 'MinCapacity (9) must be less than or equal to MaxCapacity (2)'.
Fix: RegisterScalableTarget bounds-validation error text.
Why: a text-sensitive client mis-classifies the most common registration error.
Traces: application-autoscaling.register-scalable-target-rejects-min-above-max

Cause 3 of 6: invalid cron schedule expression.
AWS: ValidationException whose message reports the parse position and the offending token of the cron expression.
Emulator: "Value 'cron(not a cron)' at 'schedule' failed to satisfy constraint: Member must be a cron expression of 6 fields ([Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]), got 3. The supported formats are at(...), rate(value unit) and cron(fields)".
Why: schedule editors use the reported position/token to point the user at the broken field.
Fix: PutScheduledAction schedule-parser error text.
Traces: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression

Cause 4 of 6: illegal ScheduledActionName.
AWS: ValidationException with the service's own text for a name containing '/' .
Emulator: dumps the raw Smithy pattern - "Value 'scale-up/weekday' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!((^[ ]+.*)|(.*([\\u0000-\\u001f]|[\\u007f-\\u009f]|[:/|])+.*)|(.*[ ]+$))).+$".
Why: the raw regex is not actionable for a caller.
Fix: PutScheduledAction name-validation error text.
Traces: application-autoscaling.scheduled-action-name-with-a-slash-rejected

Cause 5 of 6: PutScalingPolicy configuration refusals.
AWS: ValidationException with the service's own text for an ALBRequestCountPerTarget predefined metric missing ResourceLabel, for a StepScaling policy with no StepAdjustments, for a StepAdjustment with neither bound, and for a target-tracking configuration carrying both a predefined and a customized metric specification.
Emulator: 'A ALBRequestCountPerTarget policy requires a ResourceLabel identifying the target group the request count is measured per', 'A StepScaling policy requires at least one StepAdjustment', 'A StepAdjustment must specify MetricIntervalLowerBound or MetricIntervalUpperBound: ...', 'A TargetTrackingScaling policy takes either a PredefinedMetricSpecification or a CustomizedMetricSpecification, not both' (note the 'A ALB...' article bug as a symptom of hand-written text).
Why: these four are the common policy-authoring errors whose text reaches operators and CI logs.
Fix: PutScalingPolicy validation error texts.
Traces: application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected, application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected, application-autoscaling.ecs-step-adjustment-without-any-bound-rejected, application-autoscaling.target-tracking-with-both-metric-specifications-rejected

Cause 6 of 6: tag operations on a deregistered target ARN.
AWS: ResourceNotFoundException (Smithy: TagResource/ListTagsForResource declare ResourceNotFoundException, httpError 404) with message 'Scalable Target not found'.
Emulator: ResourceNotFoundException with 'No scalable target found for resource arn:aws:application-autoscaling:...:scalable-target/<id>'.
Why: the code matches, but the message differs from AWS's fixed phrase that tag tooling matches on.
Fix: ListTagsForResource / TagResource / UntagResource not-found error text.
Traces: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected

## Evidence
Replay report expected-vs-actual:
- register-scalable-target-rejects-nonexistent-table: expected 'DynamoDB table does not exist: table/<tbl>'; got "Value 'table/<tbl>' at 'resourceId' failed to satisfy constraint: no DynamoDB table named '<tbl>' exists in us-west-2".
- register-scalable-target-rejects-min-above-max: "expected the message 'Maximum capacity cannot be less than minimum capacity', got 'MinCapacity (9) must be less than or equal to MaxCapacity (2)'".
- put-scheduled-action-rejects-invalid-cron-expression: 'the rejection message was "Value \'cron(not a cron)\' at \'schedule\' ... got 3 ..."; real AWS reports the parse position and the offending token'.
- scheduled-action-name-with-a-slash-rejected: emulator returned the raw regex pattern message.
- the four PutScalingPolicy checks: 'the refusal message was ...' with the hand-written texts quoted above.
- tag-operations-on-a-deregistered-target-arn-rejected: expected ('ResourceNotFoundException', 'Scalable Target not found'); got ResourceNotFoundException 'No scalable target found for resource arn:...:scalable-target/<id>'.
- register-missing-global-secondary-index-rejected / register-on-demand-table-rejected / register-table-resource-id-with-an-index-dimension-rejected: 'the refusal message was ...' with the constraint-template texts quoted above.
Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (register_scalable_target.rs, put_scheduled_action.rs, put_scaling_policy.rs, list_tags_for_resource.rs) and the shared validation message builder.

## Steps to Reproduce
Boot the emulator and compare response['Error']['Message'] for:
1. register_scalable_target on a missing table / a missing GSI / an on-demand table / a table resource id with dynamodb:index:* dimension.
2. register_scalable_target with MinCapacity 9, MaxCapacity 2.
3. put_scheduled_action with Schedule='cron(not a cron)' and with ScheduledActionName='scale-up/weekday'.
4. put_scaling_policy: ALBRequestCountPerTarget without ResourceLabel; StepScaling with no StepAdjustments; a StepAdjustment with neither bound; target tracking with both metric specifications.
5. register then deregister a target, then list_tags_for_resource on its ARN.

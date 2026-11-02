Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.register-scalable-target-nonexistent-resource-rejected application-autoscaling.table-that-exists-only-in-another-region-register-rejected application-autoscaling.deleted-table-leaves-its-scalable-target-registered application-autoscaling.register-scalable-target-min-greater-than-max-rejected application-autoscaling.min-only-register-above-the-stored-max-rejected application-autoscaling.register-below-namespace-minimum-rejected application-autoscaling.put-scheduled-action-unknown-rate-unit-rejected application-autoscaling.put-scheduled-action-non-canonical-timezone-rejected application-autoscaling.put-scheduled-action-without-schedule-rejected-on-create application-autoscaling.put-scheduled-action-start-after-end-rejected application-autoscaling.put-scheduled-action-inverted-bounds-rejected application-autoscaling.dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected application-autoscaling.predefined-metric-of-the-other-dimension-rejected application-autoscaling.customized-metric-rejected-on-a-dynamodb-dimension application-autoscaling.scheduled-action-name-is-scoped-to-its-dimension application-autoscaling.describe-scalable-targets-invalid-next-token-rejected application-autoscaling.deregister-nonexistent-target-rejected application-autoscaling.tag-resource-nonexistent-resource-rejected application-autoscaling.tag-resource-too-many-tags-rejected application-autoscaling.aws-prefixed-tag-key-rejected-on-scalable-target application-autoscaling.describe-filter-lists-above-fifty-entries-rejected application-autoscaling.put-scaling-policy-missing-configuration-rejected application-autoscaling.second-target-tracking-policy-on-the-same-metric-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.register-scalable-target-nonexistent-resource-rejected|run#7|AssertionError|assert error.get("Message") == f"DynamoDB table does not exist: {ghost_resource_id}", (
Issue-Key: application-autoscaling.table-that-exists-only-in-another-region-register-rejected|run#12|AssertionError|assert message == expected, f'refusal message {message!r}, expected {expected!r}'
Issue-Key: application-autoscaling.deleted-table-leaves-its-scalable-target-registered|run#17|AssertionError|assert (code, message) == ("ValidationException",
Issue-Key: application-autoscaling.register-scalable-target-min-greater-than-max-rejected|run#7|AssertionError|assert error.get("Message") == "Maximum capacity cannot be less than minimum capacity", (
Issue-Key: application-autoscaling.min-only-register-above-the-stored-max-rejected|run#7|AssertionError|assert msg == "Maximum capacity cannot be less than minimum capacity", f"rejection message was {msg!r}"
Issue-Key: application-autoscaling.register-below-namespace-minimum-rejected|run#7|AssertionError|assert message == "Minimum capacity cannot be less than 1", (
Issue-Key: application-autoscaling.put-scheduled-action-unknown-rate-unit-rejected|run#14|AssertionError|assert error['Message'] == _SYNTAX_MESSAGE, (
Issue-Key: application-autoscaling.put-scheduled-action-non-canonical-timezone-rejected|run#13|AssertionError|assert error['Message'] == 'Invalid schedule timezone: europe/london', (
Issue-Key: application-autoscaling.put-scheduled-action-without-schedule-rejected-on-create|run#14|AssertionError|assert error['Message'] == 'Schedule must be set', (
Issue-Key: application-autoscaling.put-scheduled-action-start-after-end-rejected|run#15|AssertionError|assert error['Message'] == 'StartTime must be before EndTime', (
Issue-Key: application-autoscaling.put-scheduled-action-inverted-bounds-rejected|run#11|AssertionError|assert message == "Maximum capacity cannot be less than minimum capacity", (
Issue-Key: application-autoscaling.dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected|run#11|AssertionError|assert message == expected, f'refusal message {message!r}, expected {expected!r}'
Issue-Key: application-autoscaling.predefined-metric-of-the-other-dimension-rejected|run#8|AssertionError|assert msg == (f"Scalable dimension {_DIM} only supports the following predefined metric types: "
Issue-Key: application-autoscaling.customized-metric-rejected-on-a-dynamodb-dimension|run#8|AssertionError|assert msg == f"Scalable dimension {_DIM} does not support CustomizedMetricSpecification.", (
Issue-Key: application-autoscaling.scheduled-action-name-is-scoped-to-its-dimension|run#19|AssertionError|assert (error['Code'], error['Message']) == (
Issue-Key: application-autoscaling.describe-scalable-targets-invalid-next-token-rejected|run#10|AssertionError|assert error.get("Message") == "The NextToken value is not valid", (
Issue-Key: application-autoscaling.deregister-nonexistent-target-rejected|run#8|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: application-autoscaling.tag-resource-nonexistent-resource-rejected|run#5|AssertionError|assert error.get("Message") == "Access Denied", (
Issue-Key: application-autoscaling.tag-resource-too-many-tags-rejected|run#10|AssertionError|assert error.get("Message") == "The resultant tag set must not have more than 50 user tags.", (
Issue-Key: application-autoscaling.aws-prefixed-tag-key-rejected-on-scalable-target|run#12|AssertionError|assert error['Message'] == _SYSTEM, (
Issue-Key: application-autoscaling.describe-filter-lists-above-fifty-entries-rejected|run#20|AssertionError|assert got == expected[member], (
Issue-Key: application-autoscaling.put-scaling-policy-missing-configuration-rejected|run#11|AssertionError|assert error.get("Message") == "TargetTrackingScaling policy configuration must be specified", (
Issue-Key: application-autoscaling.second-target-tracking-policy-on-the-same-metric-rejected|run#10|AssertionError|assert msg == "Only one TargetTrackingScaling policy for a given metric specification is allowed.", (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Application Auto Scaling refusal messages are synthesized from a generic constraint template instead of the service's own texts (10 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
The emulator reaches the right verdict and (except where noted) the right error code on these refusals, but builds the Message from a generic "Value 'X' at 'member' failed to satisfy constraint: Member must ..." template. Application Auto Scaling has its own message for each of these rules, and each recorded message below is built from the request (it names the offending resource, bound, metric or member), so the TEXT RULE - not the recorded literal - is the requirement. Ten independent message families; each needs its own text.

Cause 1 of 10: nonexistent / wrong-region DynamoDB resource id on RegisterScalableTarget.
AWS: ValidationException (Smithy: RegisterScalableTarget errors include ValidationException, 400) with 'DynamoDB table does not exist: <resourceId>' - the message is the fixed phrase plus the request's resourceId.
Emulator: "Value 'table/<name>' at 'resourceId' failed to satisfy constraint: no DynamoDB table named '<name>' exists in us-west-2".
Why: clients and tests that branch on the refusal text (or surface it to users) see an invented phrasing and a region leak that AWS never emits.
Fix: RegisterScalableTarget validation, DynamoDB resource-resolution error text.
Traces: application-auto-scaling.register-scalable-target-nonexistent-resource-rejected, application-auto-scaling.table-that-exists-only-in-another-region-register-rejected, application-auto-scaling.deleted-table-leaves-its-scalable-target-registered

Cause 2 of 10: capacity-bound refusals.
AWS: ValidationException with 'Maximum capacity cannot be less than minimum capacity' for an inverted pair on RegisterScalableTarget; for a min-only upsert above the stored max, and for a MinCapacity below the namespace/dimension floor, the recorded message names the inverted pair / the enforced floor (the floor is not in the Smithy model - ResourceCapacity is an unconstrained integer - so the message is the only way a caller learns it).
Emulator: 'MinCapacity (10) must be less than or equal to MaxCapacity (5)' and "Value '0' at 'minCapacity' failed to satisfy constraint: Member must have value greater than or equal to 1 for scalable dimension dynamodb:table:ReadCapacityUnits".
Why: text-sensitive clients mis-handle a bound error class that AWS words uniformly.
Fix: RegisterScalableTarget validation (bounds error text).
Traces: application-auto-scaling.register-scalable-target-min-greater-than-max-rejected, application-auto-scaling.min-only-register-above-the-stored-max-rejected, application-auto-scaling.register-below-namespace-minimum-rejected

Cause 3 of 10: PutScheduledAction schedule/timezone/window refusals.
AWS: ValidationException whose recorded texts are the service's own, e.g. 'Invalid schedule expression. Details: Schedule expressions must have the following syntax: rate(<number>\\s?(minutes?|hours?|days?)), cron(<cron_expression>) or at(yyyy-MM-dd\'T\'HH:mm:ss).' for a bad rate unit, 'Invalid schedule timezone: <value>' for a non-canonical timezone, 'Schedule must be set' when Schedule is missing on create, and 'StartTime must be before EndTime' for an inverted window.
Emulator: constraint-template variants, e.g. "Value 'rate(5 fortnights)' at 'schedule' failed to satisfy constraint: Member's rate unit must be one of minute | minutes | ...", "Value 'europe/london' at 'timezone' failed to satisfy constraint: Member must be the canonical name of an IANA time zone, ...", 'Schedule is required when creating a scheduled action', 'EndTime must be at or after StartTime for a scheduled action', 'MinCapacity (8) must be less than or equal to MaxCapacity (3)' for inverted scalable-target bounds on the action.
Why: scheduling tools parse these messages to tell the user which part of the expression is wrong.
Fix: PutScheduledAction validation (schedule parser + window + bounds error texts).
Traces: application-auto-scaling.put-scheduled-action-unknown-rate-unit-rejected, application-auto-scaling.put-scheduled-action-non-canonical-timezone-rejected, application-auto-scaling.put-scheduled-action-without-schedule-rejected-on-create, application-auto-scaling.put-scheduled-action-start-after-end-rejected, application-auto-scaling.put-scheduled-action-inverted-bounds-rejected

Cause 4 of 10: metric-not-supported-for-dimension refusals on PutScalingPolicy.
AWS: ValidationException naming the dimension and enumerating the metric types it supports, e.g. 'Scalable dimension cassandra:table:ReadCapacityUnits only supports the following predefined metric types: CassandraReadCapacityUtilization'; a customized metric on a DynamoDB dimension is refused with the service's own text.
Emulator: "Value 'DynamoDBReadCapacityUtilization' at '...predefinedMetricType' failed to satisfy constraint: Member must be a metric published for scalable dimension ..." / "Value at '...customizedMetricSpecification' failed to satisfy constraint: a customized metric is not supported for scalable dimension dynamodb:table:WriteCapacityUnits".
Why: the AWS message tells the caller WHICH metric types are allowed; the emulator's does not, so a client cannot recover from it.
Fix: PutScalingPolicy validation (predefined/customized metric support table + error text).
Traces: application-auto-scaling.dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected, application-auto-scaling.predefined-metric-of-the-other-dimension-rejected, application-auto-scaling.customized-metric-rejected-on-a-dynamodb-dimension

Cause 5 of 10: dimension-filter-without-resource-id refusal on the Describe* reads that do refuse it.
AWS: ValidationException 'Scalable dimension cannot be provided without a resource ID.' (DescribeScalableTargets, whose filter is a list, says resource IDs).
Emulator: "Value 'dynamodb:table:WriteCapacityUnits' at 'scalableDimension' failed to satisfy constraint: A resource ID must be specified when a scalable dimension is specified".
Why: same error class worded differently on every read path.
Fix: shared Describe* input validation error text.
Traces: application-auto-scaling.scheduled-action-name-is-scoped-to-its-dimension

Cause 6 of 10: invalid NextToken.
AWS: InvalidNextTokenException is modeled (Smithy: DescribeScalableTargets errors include InvalidNextTokenException, 400); recorded message 'The NextToken value is not valid'.
Emulator: 'The next token supplied was invalid.'
Why: pagination wrappers that match on the message treat a bad token as an unknown failure.
Fix: pagination-token validation error text.
Traces: application-auto-scaling.describe-scalable-targets-invalid-next-token-rejected

Cause 7 of 10: DeregisterScalableTarget for a target that is not registered.
AWS: recorded behavior only - the refusal carries the service's own not-found text (the check pins it exactly).
Emulator: 'No scalable target registered for service namespace: dynamodb, resource ID: table/<name>, scalable dimension: dynamodb:table:WriteCapacityUnits'.
Why: callers that distinguish an already-deregistered target by message mis-classify it.
Fix: DeregisterScalableTarget handler not-found error text.
Traces: application-auto-scaling.deregister-nonexistent-target-rejected

Cause 8 of 10: tagging refusals.
AWS: TagResource on a well-formed but nonexistent scalable-target ARN is ValidationException with message 'Access Denied'; 51 tags is ValidationException 'The resultant tag set must not have more than 50 user tags.' (notably NOT the modeled TooManyTagsException, which the model also declares); an 'aws:'-prefixed key is refused with 'Caller is an end user and not allowed to mutate system tags'.
Emulator: "Value 'arn:...:scalable-target/00000000-...' at 'resourceARN' failed to satisfy constraint: Member must identify a registered scalable target", "Value at 'tags' failed to satisfy constraint: Member must have at most 50 entries, and 51 were supplied", and "Value 'aws:cost-center' at 'tags' failed to satisfy constraint: Member must not begin with the reserved prefix 'aws:'".
Why: tag-management tooling keys off these three distinct texts to tell a permission problem from a limit problem from a reserved-prefix problem.
Fix: TagResource validation error texts.
Traces: application-auto-scaling.tag-resource-nonexistent-resource-rejected, application-auto-scaling.tag-resource-too-many-tags-rejected, application-auto-scaling.aws-prefixed-tag-key-rejected-on-scalable-target

Cause 9 of 10: list-length validation message does not embed the offending value.
AWS: the framework message embeds the member's actual value, i.e. '1 validation error detected: Value \'[<all 51 resource ids>]\' at \'resourceIds\' failed to satisfy constraint: Member must have length less than or equal to 50'. (Smithy: DescribeScalableTargets.ResourceIds @length max 50.)
Emulator: substitutes a summary - '1 validation error detected: Value \'51 items\' at \'resourceIds\' failed to satisfy constraint: Member must have length less than or equal to 50'.
Why: the rule is "echo the member value"; a client that parses the echoed list out of the message gets nothing.
Fix: shared constraint-violation message builder (value rendering for list members).
Traces: application-auto-scaling.describe-filter-lists-above-fifty-entries-rejected

Cause 10 of 10: PutScalingPolicy configuration refusals.
AWS: a TargetTrackingScaling policy with no TargetTrackingScalingPolicyConfiguration, and a second target-tracking policy on a metric already tracked for that target, are each refused with ValidationException carrying the service's own text (the checks pin both).
Emulator: 'A TargetTrackingScaling policy requires TargetTrackingScalingPolicyConfiguration' and 'Only one TargetTrackingScaling policy for a given metric specification is allowed: the policy <name> already tracks this metric for this scalable target'.
Why: both are common client-side failures whose text is surfaced to operators.
Fix: PutScalingPolicy validation error texts.
Traces: application-auto-scaling.put-scaling-policy-missing-configuration-rejected, application-auto-scaling.second-target-tracking-policy-on-the-same-metric-rejected

## Evidence
Each failing check asserted the error code (matched) and then the Message (diverged). Representative expected-vs-actual from the replay report:
- register-scalable-target-min-greater-than-max-rejected: expected 'Maximum capacity cannot be less than minimum capacity'; got 'MinCapacity (10) must be less than or equal to MaxCapacity (5)'.
- deleted-table-leaves-its-scalable-target-registered: expected ('ValidationException', 'DynamoDB table does not exist: table/<name>'); got ValidationException "Value 'table/<name>' at 'resourceId' failed to satisfy constraint: no DynamoDB table named '<name>' exists in us-west-2".
- scheduled-action-name-is-scoped-to-its-dimension: expected ('ValidationException', 'Scalable dimension cannot be provided without a resource ID.'); got "...at 'scalableDimension' failed to satisfy constraint: A resource ID must be specified when a scalable dimension is specified".
- describe-scalable-targets-invalid-next-token-rejected: expected 'The NextToken value is not valid'; got 'The next token supplied was invalid.'
- tag-resource-nonexistent-resource-rejected: expected ValidationException 'Access Denied'; got the resourceARN constraint template.
- tag-resource-too-many-tags-rejected: expected 'The resultant tag set must not have more than 50 user tags.'; got "Value at 'tags' failed to satisfy constraint: Member must have at most 50 entries, and 51 were supplied".
- aws-prefixed-tag-key-rejected-on-scalable-target: expected 'Caller is an end user and not allowed to mutate system tags'.
- put-scheduled-action-unknown-rate-unit-rejected: expected 'Invalid schedule expression. Details: Schedule expressions must have the following syntax: rate(<number>\\s?(minutes?|hours?|days?)), cron(<cron_expression>) or at(yyyy-MM-dd\'T\'HH:mm:ss).'
- put-scheduled-action-non-canonical-timezone-rejected: expected 'Invalid schedule timezone: europe/london'.
- put-scheduled-action-without-schedule-rejected-on-create: expected 'Schedule must be set'. put-scheduled-action-start-after-end-rejected: expected 'StartTime must be before EndTime'.
- dynamodb-predefined-metric-on-a-keyspaces-dimension-rejected: expected 'Scalable dimension cassandra:table:ReadCapacityUnits only supports the following predefined metric types: CassandraReadCapacityUtilization'.
- describe-filter-lists-above-fifty-entries-rejected: expected the full resourceIds list echoed in the message; got "Value '51 items'".
Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ plus the shared validation/message builder used by these handlers.

## Steps to Reproduce
Boot the emulator and, for each family, issue the refused call with botocore and compare response['Error']['Message']:
1. register_scalable_target for a nonexistent / deleted / other-region DynamoDB table.
2. register_scalable_target with MinCapacity>MaxCapacity, with a min-only upsert above the stored max, and with MinCapacity 0.
3. put_scheduled_action with Schedule='rate(5 fortnights)', Timezone='europe/london', no Schedule, StartTime>EndTime, and MinCapacity>MaxCapacity.
4. put_scaling_policy with a predefined metric of another dimension, a DynamoDB customized metric, no TargetTrackingScalingPolicyConfiguration, and a duplicate target-tracking metric.
5. describe_scheduled_actions with ScalableDimension and no ResourceId; describe_scalable_targets with NextToken='bogus'; describe_scalable_targets with 51 ResourceIds.
6. deregister_scalable_target for an unregistered target.
7. tag_resource with a bogus but well-formed target ARN, with 51 tags, and with key 'aws:cost-center'.

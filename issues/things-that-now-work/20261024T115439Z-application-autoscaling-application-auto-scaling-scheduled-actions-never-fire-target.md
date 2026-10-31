Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.at-expression-actions-fire-in-schedule-order-not-creation-order application-autoscaling.ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed application-autoscaling.scheduled-action-cron-fires-and-moves-capacity application-autoscaling.scheduled-action-cron-is-read-in-its-declared-timezone application-autoscaling.scheduled-scaling-suspended-blocks-scheduled-action application-autoscaling.target-tracking-policy-scales-out-table-capacity application-autoscaling.scaling-activity-status-progresses-through-a-nonterminal-state application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected application-autoscaling.service-linked-role-is-the-role-the-scalable-target-uses application-autoscaling.scheduled-action-name-with-a-slash-rejected application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected application-autoscaling.ecs-step-adjustment-without-any-bound-rejected application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression application-autoscaling.register-on-demand-table-rejected application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected application-autoscaling.target-tracking-with-both-metric-specifications-rejected application-autoscaling.put-scaling-policy-rejects-unregistered-target application-autoscaling.put-scaling-policy-unregistered-dimension-rejected application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.at-expression-actions-fire-in-schedule-order-not-creation-order|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scheduled-action-cron-fires-and-moves-capacity|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scheduled-action-cron-is-read-in-its-declared-timezone|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scheduled-scaling-suspended-blocks-scheduled-action|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.target-tracking-policy-scales-out-table-capacity|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scaling-activity-status-progresses-through-a-nonterminal-state|_poll_until#44|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected|run#14|AssertionError|assert (error.get('Code'), error.get('Message')) == (
Issue-Key: iam|ListAttachedRolePolicies|NoSuchEntity
Issue-Key: application-autoscaling.scheduled-action-name-with-a-slash-rejected|run#14|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-adjustment-without-any-bound-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression|run#13|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.register-on-demand-table-rejected|run#10|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected|run#10|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.target-tracking-with-both-metric-specifications-rejected|run#9|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.put-scaling-policy-rejects-unregistered-target|run#13|AssertionError|assert error.get('Message') == (
Issue-Key: application-autoscaling.put-scaling-policy-unregistered-dimension-rejected|run#17|AssertionError|assert error.get("Message") == (
Issue-Key: application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected|run#14|AssertionError|assert error.get('Message') == (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Application Auto Scaling: scheduled actions never fire, target tracking never scales, tag calls on a stale target ARN answer the wrong error, no service-linked role is minted, and refusal messages are emulator-authored (6 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScheduledAction

## Description
Six independent root causes, each grounded in failing behavioral checks whose ids carry the service token `application-autoscaling`. The ten checks owned by 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md are skipped and not re-filed here; causes 1–2 below are adjacent to that issue's cause 5/6 but are grounded in check ids it does not carry and in behaviors (scheduled dispatch, activity status progression, deregister cascade) it does not describe.

Cause 1 of 6: scheduled actions are stored and listed but never dispatched.
AWS: Smithy PutScheduledAction accepts Schedule, Timezone, StartTime, EndTime and ScalableTargetAction; recorded behavior is that the action fires at its instant, moves the target resource's capacity (a DynamoDB table's provisioned capacity, an ECS service's desiredCount), remains listed by DescribeScheduledActions afterwards, is read in its declared Timezone rather than UTC, fires in schedule order rather than creation order when several at() actions are queued, and is withheld while SuspendedState.ScheduledScalingSuspended is true.
Emulator: no firing is ever observed — each check exhausts the replay deadline (`Check timed out after 60.0s`) inside its bounded poll of the target resource or of DescribeScalingActivities.
Why: a client's scheduled capacity plan simply does not execute, and nothing in the activity history says so.
Fix: a scheduled-action dispatcher (at/rate/cron evaluation, Timezone, StartTime/EndTime window, schedule-order queueing, ScheduledScalingSuspended gate) wired to the capacity-enforcement path and the activity recorder.
Traces: application-autoscaling.at-expression-actions-fire-in-schedule-order-not-creation-order, application-autoscaling.ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed, application-autoscaling.scheduled-action-cron-fires-and-moves-capacity, application-autoscaling.scheduled-action-cron-is-read-in-its-declared-timezone, application-autoscaling.scheduled-scaling-suspended-blocks-scheduled-action

Cause 2 of 6: the scaling-activity lifecycle never advances and a target-tracking policy never scales the resource.
AWS: recorded behavior — a target-tracking policy whose alarm breaches scales the DynamoDB table out; an activity passes through a non-terminal status (Pending/InProgress) before reaching Successful; and DeregisterScalableTarget cascades to the target's policies and scheduled actions while its existing DescribeScalingActivities entries are retained.
Emulator: all three checks exhaust the deadline polling for the capacity change, for the non-terminal status, or for the post-deregister activity listing (`Check timed out after 60.0s`).
Why: the activity history is the only observable record of what auto scaling did; without progression and without the post-deregister retention a client cannot audit scaling at all, and target tracking never acts.
Fix: the activity state machine in this service (status progression + retention across deregister) and the target-tracking execution path; DeregisterScalableTarget handler for the cascade.
Traces: application-autoscaling.target-tracking-policy-scales-out-table-capacity, application-autoscaling.scaling-activity-status-progresses-through-a-nonterminal-state, application-autoscaling.deregister-scalable-target-cascades-but-keeps-activities

Cause 3 of 6: TagResource on the ARN of a deregistered scalable target answers the wrong status and error code.
AWS: Smithy TagResource declares ResourceNotFoundException with @httpError 404 (alongside TooManyTagsException and ValidationException, both 400), and its ResourceARN member is pattern-constrained to `^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$` — so a well-formed ARN whose target no longer exists is the 404 ResourceNotFoundException case, not a validation failure. Recorded message for it: 'Scalable Target not found'.
Emulator: HTTP 400 with `{'Code': 'ValidationException', 'Message': 'Access Denied'}`.
Why: the status (400 vs 404) and the error code are both wrong, so a client cannot distinguish a vanished target from a malformed request or from an authorization failure, and retry/branching logic keyed on ResourceNotFoundException never triggers.
Fix: the TagResource (and matching UntagResource/ListTagsForResource) handler — resolve the scalable-target ARN and raise ResourceNotFoundException (404) when it is absent.
Traces: application-autoscaling.tag-operations-on-a-deregistered-target-arn-rejected

Cause 4 of 6: registering a DynamoDB scalable target does not create the Application Auto Scaling service-linked role.
AWS: documented behavior — the first RegisterScalableTarget in a service-linked namespace creates the account's service-linked role; for DynamoDB that is `AWSServiceRoleForApplicationAutoScaling_DynamoDBTable`, trusted by `dynamodb.application-autoscaling.amazonaws.com`, at arn:aws:iam::<acct>:role/aws-service-role/dynamodb.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_DynamoDBTable, and the role carries the managed policy `AWSApplicationAutoscalingDynamoDBTablePolicy`. The role name and trust principal are fixed API constants, not account-varying values.
Emulator: the role is never created — IAM answers `NoSuchEntity: The role with name AWSServiceRoleForApplicationAutoScaling_DynamoDBTable cannot be found.` on ListAttachedRolePolicies after the registration, which the check surfaces as an error verdict.
Why: the RoleARN a scalable target reports points at a role that does not exist, so a client cannot inspect or validate the permissions auto scaling uses. NOTE for the fixer: this check is self-labelled DRAFT/unblessed (no real-AWS replay oracle); the role name, principal, ARN shape and policy name above are the documented constants it asserts, and the attachment shape (managed vs inline) should be confirmed before pinning it.
Fix: RegisterScalableTarget handler — mint the per-namespace service-linked role in IAM state on first registration and report its ARN as the target's RoleARN.
Traces: application-autoscaling.service-linked-role-is-the-role-the-scalable-target-uses

Cause 5 of 6: a ScheduledActionName that violates the modeled @pattern is refused with bespoke prose instead of AWS's constraint-violation message.
AWS: Smithy PutScheduledAction.ScheduledActionName targets ScheduledActionName with @length 1..256 and @pattern `^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$`, which forbids '/'. A modeled pattern violation is reported in AWS's standard constraint form, which is built from the request and the model: `1 validation error detected: Value '<value>' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. HTTP 400 / ValidationException already match.
Emulator: `Invalid scheduled action name: scale-up/weekday. A scheduled action name must not contain '/'`.
Why: the request-derived constraint message is the rule, and it is what SDK/CLI users and validators parse to learn which member and which constraint failed; a hand-written sentence names neither the member path nor the pattern.
Fix: PutScheduledAction input validation — emit modeled @pattern/@length violations through the shared constraint-violation message builder.
Traces: application-autoscaling.scheduled-action-name-with-a-slash-rejected

Cause 6 of 6: refusal messages across the validation and not-found paths are emulator-authored paraphrases of AWS's request-derived templates. HTTP status and error code match in every case below; only the message diverges, and in each case the check's expected text is the one recorded from real AWS.
AWS (recorded, per operation): PutScalingPolicy with PredefinedMetricType ALBRequestCountPerTarget and no ResourceLabel -> 'Resource label should be specified for predefined metric type ALBRequestCountPerTarget'; a StepScaling policy with no StepAdjustments, and a step adjustment with neither bound, have their own fixed texts; PutScheduledAction with a malformed cron -> a message naming the parse position and the offending token; RegisterScalableTarget on a PAY_PER_REQUEST DynamoDB table -> 'Validation failed for scalable target. Reason: PAY_PER_REQUEST table mode is not scalable.'; a table ResourceId with the dynamodb:index:ReadCapacityUnits dimension -> 'Unsupported service namespace, resource type or scalable dimension'; a target-tracking configuration carrying both a predefined and a customized metric -> its fixed text; and the ObjectNotFoundException template `No scalable target registered for service namespace: <ns>, resource ID: <rid>, scalable dimension: <dim>` (and the scheduled-action equivalent) for the unregistered-target, unregistered-dimension and missing-scheduled-action cases.
Emulator: substitutes its own prose, e.g. 'A resource label is required when the metric type is ALBRequestCountPerTarget'; 'DynamoDB table table/<t> is in PAY_PER_REQUEST billing mode and has no provisioned throughput to scale'; 'ResourceId not valid for ScalableDimension dynamodb:index:ReadCapacityUnits: table/<t> must name a resource of type index ... of the form table/{table name}/index/{index name}'; 'No scalable target found for service namespace: dynamodb, resource ID: ..., scalable dimension: ...' ('found' where AWS says 'registered').
Why: these messages are the only place the service says which member, which metric type or which namespace/resource/dimension triple was at fault, and they are built from the request, so the template is the contract; clients and test suites matching the AWS text do not match.
Fix: the shared refusal-message builders for PutScalingPolicy, PutScheduledAction, DeleteScheduledAction and RegisterScalableTarget validation in this service's ops module — align each template with AWS's wording rather than paraphrasing it.
Traces: application-autoscaling.ecs-alb-request-count-policy-without-resource-label-rejected, application-autoscaling.ecs-step-adjustment-without-any-bound-rejected, application-autoscaling.ecs-step-scaling-policy-without-step-adjustments-rejected, application-autoscaling.put-scheduled-action-rejects-invalid-cron-expression, application-autoscaling.register-on-demand-table-rejected, application-autoscaling.register-table-resource-id-with-an-index-dimension-rejected, application-autoscaling.target-tracking-with-both-metric-specifications-rejected, application-autoscaling.put-scaling-policy-rejects-unregistered-target, application-autoscaling.put-scaling-policy-unregistered-dimension-rejected, application-autoscaling.delete-scheduled-action-that-does-not-exist-rejected

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), service pass rate application-autoscaling 9/40 (22%), 31 failing checks of which 10 are owned by 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md and skipped. The 21 remaining:

Cause 1: at-expression-actions-fire-in-schedule-order-not-creation-order, ecs-at-scheduled-action-sets-the-desired-count-and-stays-listed, scheduled-action-cron-fires-and-moves-capacity, scheduled-action-cron-is-read-in-its-declared-timezone, scheduled-scaling-suspended-blocks-scheduled-action — all `Error: Check timed out after 60.0s`, each inside a bounded poll of the target resource / DescribeScalingActivities waiting for the action to fire.

Cause 2: target-tracking-policy-scales-out-table-capacity, scaling-activity-status-progresses-through-a-nonterminal-state, deregister-scalable-target-cascades-but-keeps-activities — all `Error: Check timed out after 60.0s`.

Cause 3: tag-operations-on-a-deregistered-target-arn-rejected — `assert (error.get('Code'), error.get('Message')) == ('ResourceNotFoundException', 'Scalable Target not found')` failed: `TagResource on the deregistered target ARN was refused with {'Message': 'Access Denied', 'Code': 'ValidationException'}`.

Cause 4: service-linked-role-is-the-role-the-scalable-target-uses — verdict `error`: `botocore.errorfactory.NoSuchEntityException: An error occurred (NoSuchEntity) when calling the ListAttachedRolePolicies operation: The role with name AWSServiceRoleForApplicationAutoScaling_DynamoDBTable cannot be found.`

Cause 5: scheduled-action-name-with-a-slash-rejected — expected `f"1 validation error detected: Value '{bad}' at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: {pattern}"`; got `Invalid scheduled action name: scale-up/weekday. A scheduled action name must not contain '/'`.

Cause 6 (expected -> actual):
- ecs-alb-request-count-policy-without-resource-label-rejected: 'Resource label should be specified for predefined metric type ALBRequestCountPerTarget' -> 'A resource label is required when the metric type is ALBRequestCountPerTarget'.
- register-on-demand-table-rejected: 'Validation failed for scalable target. Reason: PAY_PER_REQUEST table mode is not scalable.' -> 'DynamoDB table table/<t> is in PAY_PER_REQUEST billing mode and has no provisioned throughput to scale'.
- register-table-resource-id-with-an-index-dimension-rejected: 'Unsupported service namespace, resource type or scalable dimension' -> 'ResourceId not valid for ScalableDimension dynamodb:index:ReadCapacityUnits: table/<t> must name a resource of type index in the dynamodb service namespace, of the form table/{table name}/index/{index name}'.
- put-scheduled-action-rejects-invalid-cron-expression: got "Invalid schedule expression. Details: parse error at position 6: unexpected token 'not' in the Minutes field."; the check notes real AWS reports the parse position and offending token in its own form.
- ecs-step-adjustment-without-any-bound-rejected: got "The upper and lower bound can't be null in the same step adjustment".
- ecs-step-scaling-policy-without-step-adjustments-rejected: got 'At least one step adjustment is required if you are adding a new step scaling policy configuration'.
- target-tracking-with-both-metric-specifications-rejected: got 'You can specify either a predefined metric or a customized metric, but not both'.
- put-scaling-policy-rejects-unregistered-target / put-scaling-policy-unregistered-dimension-rejected / delete-scheduled-action-that-does-not-exist-rejected: got 'No scalable target found for service namespace: dynamodb, resource ID: ..., scalable dimension: ...' and 'No scheduled action found for service namespace: ..., scheduled action name: ...' where AWS's template says 'registered'.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (put_scheduled_action.rs, delete_scheduled_action.rs, register_scalable_target.rs, deregister_scalable_target.rs, put_scaling_policy.rs, tag_resource.rs, describe_scaling_activities.rs) and this service's state module.

## Steps to Reproduce
Boot the emulator and drive these with botocore.

Cause 1: create a PROVISIONED DynamoDB table and an ECS service; register_scalable_target; put_scheduled_action with (a) two at() expressions created in reverse schedule order, (b) a cron expression plus Timezone, (c) an at() action on ecs:service:DesiredCount; poll describe_table / describe_services and describe_scaling_activities past each instant — AWS applies them (and in schedule order, in the declared timezone) and keeps them listed; the emulator never fires. Repeat with SuspendedState.ScheduledScalingSuspended=true to see the gate.

Cause 2: put_scaling_policy TargetTrackingScaling on dynamodb:table:ReadCapacityUnits and breach the minted AlarmHigh via cloudwatch.set_alarm_state; poll describe_table for the scale-out and describe_scaling_activities for a non-terminal StatusCode before Successful. Then deregister_scalable_target and re-read describe_scaling_activities — the prior activities must still be listed while policies and scheduled actions are gone.

Cause 3: register_scalable_target, capture ScalableTargetARN, deregister_scalable_target, then tag_resource(ResourceARN=<that ARN>, Tags={'k':'v'}) — expect HTTP 404 ResourceNotFoundException ('Scalable Target not found'), observe HTTP 400 ValidationException 'Access Denied'. Same for untag_resource / list_tags_for_resource.

Cause 4: in a fresh account, register_scalable_target for dynamodb:table:ReadCapacityUnits, then iam.get_role / iam.list_attached_role_policies for 'AWSServiceRoleForApplicationAutoScaling_DynamoDBTable' — the role must exist at arn:aws:iam::<acct>:role/aws-service-role/dynamodb.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_DynamoDBTable; observe NoSuchEntity.

Cause 5: put_scheduled_action with ScheduledActionName='scale-up/weekday' — the ValidationException message must be the standard '1 validation error detected: Value ... at 'scheduledActionName' failed to satisfy constraint: Member must satisfy regular expression pattern: ...'.

Cause 6: drive each refusal and compare only the Message (status and code already match): put_scaling_policy ALBRequestCountPerTarget without ResourceLabel; StepScaling with no StepAdjustments; a step adjustment with neither MetricIntervalLowerBound nor MetricIntervalUpperBound; put_scheduled_action with cron(0 not * * ? *); register_scalable_target on a PAY_PER_REQUEST table; register_scalable_target with ResourceId 'table/<t>' and ScalableDimension dynamodb:index:ReadCapacityUnits; put_scaling_policy with both PredefinedMetricSpecification and CustomizedMetricSpecification; put_scaling_policy / delete_scheduled_action against an unregistered target or dimension.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__logs.cfn-logs-loggroup-getatt-unknown-attribute-rejected-rolls-back cloudformation__logs.cfn-logs-loggroup-readonly-arn-ignored-and-ref-getatt-match-logs cloudformation__logs.cfn-logs-metricfilter-dimension-unsupported-property-rejected cloudformation__logs.cfn-logs-metricfilter-schema-structure-rejected cloudformation__logs.cfn-metric-filter-name-string-constraints-rejected cloudformation__logs.deploy-logs-log-group-name-change-replaces-retention-change-updates-in-place cloudformation__logs.deploy-logs-log-group-rename-collision-blocks-replacement-rolls-back cloudformation__logs.deploy-logs-resourcepolicy-round-trip-then-stack-delete cloudformation__logs.metric-filter-tags-property-rejected-rolls-back-stack cloudformation__logs.stack-tags-propagate-with-system-tags-and-non-taggable-filter-deploys
Issue-Key: cloudformation__logs.cfn-logs-loggroup-getatt-unknown-attribute-rejected-rolls-back|run#15|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__logs.cfn-logs-loggroup-readonly-arn-ignored-and-ref-getatt-match-logs|run#10|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__logs.cfn-logs-metricfilter-dimension-unsupported-property-rejected|run#22|AssertionError|assert status == "ROLLBACK_COMPLETE", f"unsupported Dimension member: expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__logs.cfn-logs-metricfilter-schema-structure-rejected|run#17|AssertionError|assert status == "ROLLBACK_COMPLETE", f"{label}: expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__logs.cfn-metric-filter-name-string-constraints-rejected|run#21|AssertionError|assert _masked(reason) == _PATTERN_REASON, f"pattern violation reason: {reason!r}"
Issue-Key: cloudformation__logs.deploy-logs-log-group-name-change-replaces-retention-change-updates-in-place|run#35|AssertionError|assert _in_order(replaced_rows, [
Issue-Key: cloudformation__logs.deploy-logs-log-group-rename-collision-blocks-replacement-rolls-back|run#19|AssertionError|assert _in_order(rows, [
Issue-Key: cloudformation__logs.deploy-logs-resourcepolicy-round-trip-then-stack-delete|run#12|AssertionError|assert len(matches) == 1, f"describe_resource_policies found {matches!r} for {policy_name!r}"
Issue-Key: cloudformation__logs.metric-filter-tags-property-rejected-rolls-back-stack|_stack_failure_reason#12|AssertionError|raise AssertionError(f"no stack-level CREATE_FAILED event found for {stack_name!r}")
Issue-Key: cloudformation__logs.stack-tags-propagate-with-system-tags-and-non-taggable-filter-deploys|run#17|AssertionError|assert tag_map == want_tags, (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# cloudformation__logs: 10 net-new causes in template validation, replacement, rollback, tagging and the AWS::Logs::ResourcePolicy handler

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Ten distinct causes, all reached through CloudFormation stacks over AWS::Logs::* resources.

Cause 1 of 10: an unknown GetAtt attribute does not fail the stack.
AWS: a template whose Fn::GetAtt names an attribute the resource type does not define is refused and the stack ends ROLLBACK_COMPLETE.
Emulator: the stack reaches 'CREATE_COMPLETE'.
Why: a typo in a GetAtt silently yields an empty/odd output instead of a failed deploy.
Fix: template validation of Fn::GetAtt attribute names in the CloudFormation crate.
Traces: cloudformation__logs.cfn-logs-loggroup-getatt-unknown-attribute-rejected-rolls-back.

Cause 2 of 10: a readOnlyProperty value present in the template blocks the create.
AWS: a readOnly property (AWS::Logs::LogGroup Arn) supplied in the template is ignored and the create succeeds (CREATE_COMPLETE).
Emulator: the stack ends 'ROLLBACK_COMPLETE'.
Why: templates that round-trip a described resource (which carries Arn) cannot be deployed.
Fix: resource-schema property handling in the AWS::Logs::LogGroup handler (ignore readOnly properties).
Traces: cloudformation__logs.cfn-logs-loggroup-readonly-arn-ignored-and-ref-getatt-match-logs.

Cause 3 of 10: an unsupported AWS::Logs::MetricFilter Dimension member is accepted.
AWS: a MetricTransformation Dimensions entry naming an unsupported member fails the resource with a validation reason and the stack ends ROLLBACK_COMPLETE.
Emulator: the stack reaches 'CREATE_COMPLETE'.
Why: an invalid metric filter is created and silently emits nothing.
Fix: property validation in the AWS::Logs::MetricFilter handler.
Traces: cloudformation__logs.cfn-logs-metricfilter-dimension-unsupported-property-rejected.

Cause 4 of 10: a MetricFilter with no FilterPattern is accepted.
AWS: FilterPattern is required; its absence fails the resource and rolls the stack back to ROLLBACK_COMPLETE.
Emulator: the stack reaches 'CREATE_COMPLETE'.
Why: a filter with no pattern is created, so a client's structure error is not reported.
Fix: required-property validation in the AWS::Logs::MetricFilter handler.
Traces: cloudformation__logs.cfn-logs-metricfilter-schema-structure-rejected.

Cause 5 of 10: the filterName pattern-violation reason is not AWS's text.
AWS: a FilterName violating the name pattern fails with a resource-handler reason of the form 'Resource handler returned message: "1 validation error detected: Value '<name>' at 'filterName' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:*]*" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the offending value is part of the message.
Emulator: the message omits the value entirely: 'Value at \'filterName\' failed to satisfy constraint: ...'.
Why: a client parsing or displaying the failure reason cannot tell which value was rejected.
Fix: the validation-error message formatting in the AWS::Logs::MetricFilter handler.
Traces: cloudformation__logs.cfn-metric-filter-name-string-constraints-rejected.

Cause 6 of 10: a replacement update does not report the replacement rows in AWS's order.
AWS: a LogGroupName change replaces the resource and the stack-event rows read CREATE_IN_PROGRESS → 'Resource creation Initiated' → CREATE_COMPLETE for the new physical id, then DELETE_IN_PROGRESS → DELETE_COMPLETE for the old one, each row carrying the physical id it concerns.
Emulator: the rows carry empty/None physical ids on the first CREATE_IN_PROGRESS and on DELETE_IN_PROGRESS, so the required ordering over (status, reason, physical id) is not satisfied.
Why: a client that watches stack events to learn the replacement's new and old physical ids cannot do so.
Fix: stack-event emission for replacement updates in the CloudFormation crate.
Traces: cloudformation__logs.deploy-logs-log-group-name-change-replaces-retention-change-updates-in-place.

Cause 7 of 10: a rename collision fails the resource but never rolls the stack back.
AWS: when the replacement name already exists, the resource row is CREATE_FAILED with 'Resource handler returned message: "The specified log group already exists" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)' and the stack then rolls back to the original physical id (UPDATE_ROLLBACK_COMPLETE), leaving the original group untouched.
Emulator: only the CREATE_IN_PROGRESS and CREATE_FAILED rows appear; no rollback-to-original rows follow.
Why: a failed rename leaves the stack without the documented rollback, so the original resource's fate is unknown to the client.
Fix: the update-rollback path for a replaced AWS::Logs::LogGroup in the CloudFormation crate.
Traces: cloudformation__logs.deploy-logs-log-group-rename-collision-blocks-replacement-rolls-back.

Cause 8 of 10: an AWS::Logs::ResourcePolicy the stack creates does not exist in CloudWatch Logs.
AWS: after CREATE_COMPLETE, logs:DescribeResourcePolicies lists exactly one policy with the declared policyName, the template's document and policyScope 'ACCOUNT'.
Emulator: 'describe_resource_policies found [] for <policy name>'.
Why: a resource policy a client deploys by template silently does not exist, so cross-service log delivery it is meant to authorize is denied.
Fix: the AWS::Logs::ResourcePolicy handler in the CloudFormation crate (call through to PutResourcePolicy).
Traces: cloudformation__logs.deploy-logs-resourcepolicy-round-trip-then-stack-delete.

Cause 9 of 10: a rejected metric-filter Tags property emits no stack-level CREATE_FAILED event.
AWS: when a resource property is refused, the stack carries a stack-level CREATE_FAILED event ('The following resource(s) failed to create: [<logical id>].') alongside the resource row.
Emulator: 'no stack-level CREATE_FAILED event found for <stack>'.
Why: a client that reads stack events to find why a deploy failed sees no stack-level failure at all.
Fix: stack-level failure event emission in the CloudFormation crate.
Traces: cloudformation__logs.metric-filter-tags-property-rejected-rolls-back-stack.

Cause 10 of 10: stack tags and the aws:cloudformation:* system tags are not propagated to the created resource.
AWS: a resource created by a stack carries the stack's own tags plus aws:cloudformation:stack-name, aws:cloudformation:stack-id and aws:cloudformation:logical-id.
Emulator: the log group carries only the user tag ({'sim2real-env': 'v1'}); the three system tags are absent.
Why: tag-based cost allocation, policies and inventory that depend on the system tags do not see CloudFormation-managed resources.
Fix: tag propagation in the CloudFormation resource-create path.
Traces: cloudformation__logs.stack-tags-propagate-with-system-tags-and-non-taggable-filter-deploys.

## Evidence
Cause 1 — 'AssertionError: an unknown GetAtt attribute must fail the stack and roll it back; got 'CREATE_COMPLETE''.
Cause 2 — 'AssertionError: a readOnlyProperty value in the template must not block the create; got 'ROLLBACK_COMPLETE''.
Cause 3 — 'AssertionError: unsupported Dimension member: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE''.
Cause 4 — 'AssertionError: missing FilterPattern: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE''.
Cause 5 — 'AssertionError: pattern violation reason: 'Resource handler returned message: "1 validation error detected: Value at \'filterName\' failed to satisfy constraint: Member must satisfy regular expression pattern: [^:*]*" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)''.
Cause 6 — 'AssertionError: Replaced must report the replacement, complete as '<new name>' and delete '<old name>', in that order: [('CREATE_IN_PROGRESS', None, ''), ...]'.
Cause 7 — 'AssertionError: Group must report the replacement, fail it with the AlreadyExists reason and roll back to the original id, in that order: [('CREATE_IN_PROGRESS', '', ''), ('CREATE_FAILED', 'Resource handler returned message: "The specified log group already exists" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)', None)]'.
Cause 8 — 'AssertionError: describe_resource_policies found [] for 'sim2real-cfnlogs-rp-pol-<id>''.
Cause 9 — 'AssertionError: no stack-level CREATE_FAILED event found for '<stack>''.
Cause 10 — 'AssertionError: expected the stack tag and the three aws:cloudformation:* system tags {...} on the log group, got {'sim2real-env': 'v1'}'.

## Steps to Reproduce
Replay the ten cloudformation__logs checks listed in trace_ids; each deploys (or updates) a stack over AWS::Logs::* resources and then inspects the stack status, its event rows, the resource in CloudWatch Logs, or the resource's tags.

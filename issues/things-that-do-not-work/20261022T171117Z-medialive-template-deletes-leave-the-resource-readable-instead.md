Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Step: teardown#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cloudwatch-alarm-template-group-lifecycle medialive-eventbridge-rule-template-group-lifecycle medialive-eventbridge-rule-template-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$.code
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$.requestId
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$.retryable
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$.statusCode
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|$.time
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|4xx->2xx|status
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$.code
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$.requestId
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$.retryable
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$.statusCode
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|$.time
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|4xx->2xx|status
Issue-Key: medialive|GetEventBridgeRuleTemplate|4xx->2xx|$
Issue-Key: medialive|GetEventBridgeRuleTemplate|4xx->2xx|status
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive template deletes leave the resource readable instead of 404

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCloudWatchAlarmTemplateGroup

## Description
After a successful Delete of a medialive template group / rule template, the resource is still fully readable: the Get keeps answering 200 with the whole resource instead of 404 NotFoundException, indefinitely (the fixtures polled 15-16 times). This bundle has FOUR causes, one per deleted resource type; all must be closed.

Cause 1 of 4: GetCloudWatchAlarmTemplateGroup after DeleteCloudWatchAlarmTemplateGroup (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step teardown#8).
AWS: Smithy: GetCloudWatchAlarmTemplateGroup is GET /prod/cloudwatch-alarm-template-groups/{Identifier} with code 200 and models NotFoundException (httpError 404) among its errors; the recording shows the post-delete read settling on HTTP 404 with error code 'NotFoundException'.
Emulator: answers 200 with the live group body (arn/id/name/description/createdAt/modifiedAt) on every one of 15 poll attempts; no 404 and no NotFoundException code is ever produced.
Why: a client that deletes and then waits for the resource to disappear never observes deletion — teardown loops hang, and a caller that treats a 200 as 'still exists' concludes the delete failed. Worse, the stale resource keeps occupying its name, so a recreate collides.
Fix: the DeleteCloudWatchAlarmTemplateGroup handler's state mutation (remove/mark-deleted the record) and the Get handler's miss path (return NotFoundException with HTTP 404).
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#teardown#8.

Cause 2 of 4: GetEventBridgeRuleTemplateGroup after its Delete (fixture medialive-eventbridge-rule-template-group-lifecycle, step teardown#7).
AWS: the rule-template-group Get models NotFoundException/404; recording shows 404 with code 'NotFoundException' after the delete.
Emulator: 200 with the live group body for 15 attempts.
Why: as cause 1.
Fix: DeleteEventBridgeRuleTemplateGroup state mutation + the Get miss path.
Traces: medialive-eventbridge-rule-template-group-lifecycle#teardown#7.

Cause 3 of 4: GetEventBridgeRuleTemplate after its Delete (fixture medialive-eventbridge-rule-template-lifecycle, step teardown#8).
AWS: Get of a deleted rule template answers 404 with error code 'NotFoundException'.
Emulator: 200 with the full template body (including eventTargets and groupId) for 16 attempts; error code None.
Why: as cause 1.
Fix: DeleteEventBridgeRuleTemplate state mutation + the Get miss path.
Traces: medialive-eventbridge-rule-template-lifecycle#teardown#8.

Cause 4 of 4: GetEventBridgeRuleTemplateGroup after deleting the second group in the rule-template fixture (fixture medialive-eventbridge-rule-template-lifecycle, step teardown#10).
AWS: 404 with error code 'NotFoundException'.
Emulator: 200 with the live group body for 16 attempts.
Why: as cause 1; this one additionally shows the group delete is not blocked or cascaded by its member templates, so neither the group nor its templates ever leave state.
Fix: DeleteEventBridgeRuleTemplateGroup state mutation + the Get miss path.
Traces: medialive-eventbridge-rule-template-lifecycle#teardown#10.

## Evidence
Replay of simparity-wire-aws-v1 (4 clusters, verdict=mismatch, expected 404 / actual 200):
- GetCloudWatchAlarmTemplateGroup: 'status: expected 404, got 200'; $.code missing (expected 'NotFoundException'); $.statusCode missing (expected 404); poll: status 200 not in [404] after 15 attempt(s), got {'arn': 'arn:aws:medialive:...:cloudwatch-alarm-template-group:1000000', 'id': '1000000', 'description': 'initial description', ...}.
- GetEventBridgeRuleTemplateGroup (group fixture): same, poll 15 attempts, got the live group body.
- GetEventBridgeRuleTemplate: 'status: expected 404, got 200'; poll: error code None not in ['NotFoundException'] after 16 attempt(s), got the full template incl. eventTargets.
- GetEventBridgeRuleTemplateGroup (template fixture teardown#10): same, 16 attempts.
Smithy: GetCloudWatchAlarmTemplateGroup GET /prod/cloudwatch-alarm-template-groups/{Identifier} code 200, errors include NotFoundException httpError 404.

## Steps to Reproduce
1. POST /prod/cloudwatch-alarm-template-groups to create a group.
2. DELETE /prod/cloudwatch-alarm-template-groups/<id> (answers success).
3. GET /prod/cloudwatch-alarm-template-groups/<id> — returns 200 with the group body instead of 404 NotFoundException, on every retry.
4. Repeat for the EventBridge rule template group and the EventBridge rule template.
5. Equivalent replay: fixture medialive-cloudwatch-alarm-template-group-lifecycle step teardown#8, plus the steps in trace_ids.

Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cloudwatch-alarm-template-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cloudwatch-alarm-template-lifecycle medialive-template-error-paths
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateCloudWatchAlarmTemplate|2xx->2xx|$.datapointsToAlarm
Issue-Key: medialive|GetCloudWatchAlarmTemplate|2xx->2xx|$.datapointsToAlarm
Issue-Key: medialive|GetCloudWatchAlarmTemplate|2xx->2xx|$.description
Issue-Key: medialive|GetCloudWatchAlarmTemplate|2xx->2xx|$.threshold
Issue-Key: medialive|DeleteCloudWatchAlarmTemplateGroup|2xx->4xx|status
Issue-Commit: 982a07ada62dfbeb289a85399a86f8ade68b0ada

# medialive CloudWatch alarm template APIs lose datapointsToAlarm and reject empty-group delete with 409

> _Note: this issue was identified against the code base at commit `982a07ada62dfbeb289a85399a86f8ade68b0ada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCloudWatchAlarmTemplate

## Description
The emulator's CloudWatch-alarm-template resource is unfaithful in two independent ways: the optional DatapointsToAlarm member is never carried on the wire, and deleting an alarm template group answers with a conflict instead of succeeding. This bundle has THREE causes; all must be closed.

Cause 1 of 3: CreateCloudWatchAlarmTemplate drops datapointsToAlarm from its response (fixture medialive-cloudwatch-alarm-template-lifecycle, step task#2; same cluster also medialive-template-error-paths#setup#2).
AWS: Smithy: CreateCloudWatchAlarmTemplate is POST /prod/cloudwatch-alarm-templates with HTTP code 201; input member DatapointsToAlarm targets __integerMin1 (jsonName 'datapointsToAlarm', optional), and CreateCloudWatchAlarmTemplateResponse declares the same DatapointsToAlarm member. AWS therefore echoes the value the caller supplied. The recording shows 201 with $.datapointsToAlarm = 1.
Emulator: answers 201 but $.datapointsToAlarm is absent from the body (all other members match).
Why: a client that creates an alarm template with datapointsToAlarm and reads the creation response back cannot confirm the M-of-N datapoint setting it just configured; SDK models the member as optional, so it deserializes as None and configuration-management code concludes the setting was not accepted and re-issues or reports drift.
Fix: the CreateCloudWatchAlarmTemplate handler/state write plus its response serializer — accept the request's DatapointsToAlarm, store it on the template, and emit it when present.
Traces: medialive-cloudwatch-alarm-template-lifecycle#task#2.

Cause 2 of 3: GetCloudWatchAlarmTemplate drops datapointsToAlarm from its response (same fixture, steps task#3 and task#6).
AWS: Smithy: GetCloudWatchAlarmTemplate is GET /prod/cloudwatch-alarm-templates/{Identifier}, HTTP 200, and GetCloudWatchAlarmTemplateResponse declares DatapointsToAlarm (__integerMin1, jsonName 'datapointsToAlarm'). A read of a template created with the member returns it. The recording shows 200 with $.datapointsToAlarm = 1 on both reads.
Emulator: 200 with $.datapointsToAlarm missing on the read right after create (task#3) and on the later read (task#6).
Why: the member is unreadable for the whole lifetime of the resource, not just at create time, so no client can ever observe the stored alarm sensitivity. Hypothesis: the member is not modelled in the emulator's stored template at all, which is why neither the create echo nor any later read carries it.
Fix: the alarm-template state record and the shared GetCloudWatchAlarmTemplate/Create serializer. Note: step task#6 ALSO shows $.threshold (expected 5, got 1.0) and $.description (expected 'raised threshold', got 'alert on active alerts') still holding their pre-update values; that part is downstream of the unimplemented UpdateCloudWatchAlarmTemplate already filed as 20261023T031236Z-medialive-updatecloudwatchalarmtemplate-stub.md and is NOT part of this issue — only the missing datapointsToAlarm is.
Traces: medialive-cloudwatch-alarm-template-lifecycle#task#3, medialive-cloudwatch-alarm-template-lifecycle#task#6.

Cause 3 of 3: DeleteCloudWatchAlarmTemplateGroup returns 409 instead of 204 when the group is deletable (same fixture, step teardown#9; same cluster also medialive-template-error-paths#teardown#6).
AWS: Smithy: DeleteCloudWatchAlarmTemplateGroup is DELETE /prod/cloudwatch-alarm-template-groups/{Identifier} with HTTP code 204 and an empty output; ConflictException (409) is modelled but is the non-success path. The recording shows 204 with no body for a group whose templates were already deleted earlier in the same teardown.
Emulator: 409 (ConflictException) for the same request, so the resource is never removed.
Why: the status itself diverges, so a client's teardown fails: an SDK raises ConflictException, CloudFormation/Terraform-style deletes abort, and test suites leak alarm template groups that cannot be removed at all. Hypothesis: the emulator's non-empty-group guard counts template membership that is not decremented when a member template is deleted, so an empty group still looks occupied.
Fix: the DeleteCloudWatchAlarmTemplateGroup handler's conflict precondition and the group-membership bookkeeping in the alarm-template state (membership must be released on DeleteCloudWatchAlarmTemplate), returning 204 with an empty body on success.
Traces: medialive-cloudwatch-alarm-template-lifecycle#teardown#9, medialive-template-error-paths#teardown#6.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (service medialive, verdict=mismatch in every cluster):
- CreateCloudWatchAlarmTemplate, expected 201 / actual 201: '$.datapointsToAlarm: missing (expected 1, got None)' — fixture medialive-cloudwatch-alarm-template-lifecycle step task#2, cluster also covers medialive-template-error-paths#setup#2.
- GetCloudWatchAlarmTemplate, expected 200 / actual 200: '$.datapointsToAlarm: missing (expected 1, got None)' — step task#3.
- GetCloudWatchAlarmTemplate, expected 200 / actual 200: '$.threshold value_mismatch (expected 5, got 1.0); $.datapointsToAlarm missing (expected 1, got None); $.description value_mismatch (expected "raised threshold", got "alert on active alerts")' — step task#6 (only the datapointsToAlarm key belongs to this issue; see Cause 2).
- DeleteCloudWatchAlarmTemplateGroup: 'status: expected 204, got 409' — step teardown#9, cluster also covers medialive-template-error-paths#teardown#6.
Smithy (services/service-com-amazonaws-medialive-medialive): CreateCloudWatchAlarmTemplate POST /prod/cloudwatch-alarm-templates code 201, input+output member DatapointsToAlarm -> __integerMin1 jsonName 'datapointsToAlarm'; GetCloudWatchAlarmTemplate GET /prod/cloudwatch-alarm-templates/{Identifier} code 200, output member DatapointsToAlarm; DeleteCloudWatchAlarmTemplateGroup DELETE /prod/cloudwatch-alarm-template-groups/{Identifier} code 204, output Unit, errors include ConflictException(409)/NotFoundException(404).

## Steps to Reproduce
1. POST /prod/cloudwatch-alarm-template-groups to create a group.
2. POST /prod/cloudwatch-alarm-templates with groupIdentifier=<group>, a required threshold/period/statistic/etc. and datapointsToAlarm=1 -> response is 201 but has no 'datapointsToAlarm' key.
3. GET /prod/cloudwatch-alarm-templates/<templateId> -> 200, still no 'datapointsToAlarm'.
4. DELETE /prod/cloudwatch-alarm-templates/<templateId> (succeeds), then DELETE /prod/cloudwatch-alarm-template-groups/<groupId> -> 409 ConflictException instead of 204 with an empty body.
5. Equivalent replay: fixture medialive-cloudwatch-alarm-template-lifecycle steps task#2 (representative), task#3, task#6 and teardown#9, plus fixture medialive-template-error-paths steps setup#2 and teardown#6.

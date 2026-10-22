Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cloudwatch-alarm-template-group-lifecycle medialive-cloudwatch-alarm-template-lifecycle medialive-template-error-paths medialive-eventbridge-rule-template-group-lifecycle medialive-eventbridge-rule-template-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|2xx->2xx|$.createdAt
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|2xx->2xx|$.modifiedAt
Issue-Key: medialive|CreateCloudWatchAlarmTemplateGroup|2xx->2xx|$.tags
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|2xx->2xx|$.createdAt
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|2xx->2xx|$.modifiedAt
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|2xx->2xx|$.tags
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|2xx->2xx|$.createdAt
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|2xx->2xx|$.modifiedAt
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|2xx->2xx|$.tags
Issue-Key: medialive|CreateEventBridgeRuleTemplateGroup|2xx->2xx|$.createdAt
Issue-Key: medialive|CreateEventBridgeRuleTemplateGroup|2xx->2xx|$.modifiedAt
Issue-Key: medialive|CreateEventBridgeRuleTemplateGroup|2xx->2xx|$.tags
Issue-Key: medialive|CreateEventBridgeRuleTemplate|2xx->2xx|$.createdAt
Issue-Key: medialive|CreateEventBridgeRuleTemplate|2xx->2xx|$.modifiedAt
Issue-Key: medialive|CreateEventBridgeRuleTemplate|2xx->2xx|$.tags
Issue-Key: medialive|GetEventBridgeRuleTemplate|2xx->2xx|$.createdAt
Issue-Key: medialive|GetEventBridgeRuleTemplate|2xx->2xx|$.modifiedAt
Issue-Key: medialive|GetEventBridgeRuleTemplate|2xx->2xx|$.tags
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive template APIs serialize timestamps as epoch numbers and omit tags

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCloudWatchAlarmTemplateGroup

## Description
Every medialive template/template-group response serializes the `createdAt` and `modifiedAt` members as a bare epoch-seconds number and leaves `tags` absent instead of emitting the empty map. This bundle has SIX causes, one per diverging operation; all share the same serializer defect and all must be closed.

Cause 1 of 6: CreateCloudWatchAlarmTemplateGroup (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step setup#1).
AWS: Smithy: CreateCloudWatchAlarmTemplateGroupResponse.CreatedAt and .ModifiedAt both target `__timestampIso8601` with `timestampFormat: date-time`, so the wire value is an RFC3339/ISO-8601 string (the recording shows e.g. '2026-09-06T17:08:00.204Z'); .Tags targets `TagMap` and the recording shows it present as `{}` on an untagged group. HTTP 201, which the emulator does match.
Emulator: returns 201 but emits `createdAt`/`modifiedAt` as the JSON number 1792688709.7830105 and omits `tags` entirely (null).
Why: a generated SDK deserializes a `date-time` member from a string; a number either fails to parse or yields a wrong instant, and `tags` being null instead of an empty map makes a caller that iterates tags NPE/panic rather than see zero tags.
Fix: the medialive CloudWatch-alarm-template-group response serializer — emit the timestamp members in `date-time` form and always project `tags` as a map.
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#setup#1 (also medialive-cloudwatch-alarm-template-lifecycle#task#1, medialive-template-error-paths#setup#1).

Cause 2 of 6: GetCloudWatchAlarmTemplateGroup (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#2).
AWS: Smithy: GetCloudWatchAlarmTemplateGroupResponse.CreatedAt/.ModifiedAt -> `__timestampIso8601`, `timestampFormat: date-time`; .Tags -> `TagMap`, recorded as `{}`. HTTP 200.
Emulator: 200 with the same two timestamps as epoch numbers and `tags` missing.
Why: as cause 1 — a read of an existing group is the common path, so every client sees the malformed timestamp.
Fix: the GetCloudWatchAlarmTemplateGroup response serializer (same projection helper as cause 1).
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#2.

Cause 3 of 6: GetEventBridgeRuleTemplateGroup (fixture medialive-eventbridge-rule-template-group-lifecycle, step task#2).
AWS: same modeled shape family — the EventBridge-rule-template-group response's CreatedAt/ModifiedAt are ISO-8601 `date-time` timestamps and `tags` is present as `{}`. HTTP 200.
Emulator: 200 with epoch-number timestamps and no `tags`.
Why: as cause 1.
Fix: the EventBridge-rule-template-group response serializer.
Traces: medialive-eventbridge-rule-template-group-lifecycle#task#2.

Cause 4 of 6: CreateEventBridgeRuleTemplateGroup (fixture medialive-eventbridge-rule-template-lifecycle, step setup#1).
AWS: ISO-8601 `date-time` createdAt/modifiedAt (recording: '2026-09-06T17:08:00.204Z') and `tags: {}`; HTTP 201.
Emulator: 201 with epoch numbers and no `tags`.
Why: as cause 1.
Fix: the CreateEventBridgeRuleTemplateGroup response serializer.
Traces: medialive-eventbridge-rule-template-lifecycle#setup#1.

Cause 5 of 6: CreateEventBridgeRuleTemplate (fixture medialive-eventbridge-rule-template-lifecycle, step setup#2).
AWS: ISO-8601 `date-time` createdAt/modifiedAt (recording: '2026-09-06T17:08:00.316Z') and `tags: {}`; HTTP 201.
Emulator: 201 with epoch numbers and no `tags`.
Why: as cause 1.
Fix: the CreateEventBridgeRuleTemplate response serializer.
Traces: medialive-eventbridge-rule-template-lifecycle#setup#2.

Cause 6 of 6: GetEventBridgeRuleTemplate (fixture medialive-eventbridge-rule-template-lifecycle, step task#3).
AWS: ISO-8601 `date-time` createdAt/modifiedAt and `tags: {}`; HTTP 200.
Emulator: 200 with epoch numbers and no `tags`.
Why: as cause 1.
Fix: the GetEventBridgeRuleTemplate response serializer.
Traces: medialive-eventbridge-rule-template-lifecycle#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (6 divergence clusters, all verdict=mismatch, status matched in every one):
- CreateCloudWatchAlarmTemplateGroup 201->201: $.createdAt value_mismatch (expected shaped ISO-8601, got 1792688709.7830105); $.modifiedAt same; $.tags missing (expected {}, got None).
- GetCloudWatchAlarmTemplateGroup 200->200: identical three deltas.
- GetEventBridgeRuleTemplateGroup 200->200: $.createdAt/$.modifiedAt got 1792688740.1119943; $.tags missing.
- CreateEventBridgeRuleTemplateGroup 201->201: expected '2026-09-06T17:08:00.204Z', got 1792688768.9553368; $.tags missing.
- CreateEventBridgeRuleTemplate 201->201: expected '2026-09-06T17:08:00.316Z', got 1792688768.9947217; $.tags missing.
- GetEventBridgeRuleTemplate 200->200: expected '2026-09-06T17:08:00.316Z', got 1792688768.9947217; $.tags missing.
Smithy (services/service-com-amazonaws-medialive-medialive): CreatedAt/ModifiedAt -> __timestampIso8601 with timestampFormat date-time; Tags -> TagMap.

## Steps to Reproduce
1. Boot the emulator and POST /prod/cloudwatch-alarm-template-groups with {"name": "<name>", "description": "initial description"}.
2. Observe the 201 body: createdAt/modifiedAt are JSON numbers, not ISO-8601 date-time strings, and tags is absent.
3. GET /prod/cloudwatch-alarm-template-groups/<id> and observe the same.
4. Repeat with POST /prod/eventbridge-rule-template-groups, POST /prod/eventbridge-rule-templates and their GETs.
5. Equivalent replay: fixture medialive-cloudwatch-alarm-template-group-lifecycle step setup#1, plus the steps listed in trace_ids.

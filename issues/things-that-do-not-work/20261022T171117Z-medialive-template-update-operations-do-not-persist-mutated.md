Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cloudwatch-alarm-template-group-lifecycle medialive-eventbridge-rule-template-group-lifecycle medialive-eventbridge-rule-template-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|GetCloudWatchAlarmTemplateGroup|2xx->2xx|$.description
Issue-Key: medialive|GetEventBridgeRuleTemplateGroup|2xx->2xx|$.description
Issue-Key: medialive|GetEventBridgeRuleTemplate|2xx->2xx|$.description
Issue-Key: medialive|GetEventBridgeRuleTemplate|2xx->2xx|$.eventType
Issue-Related: 20261022T171117Z-medialive-template-apis-serialize-timestamps-as-epoch-numbers.md
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# medialive template update operations do not persist mutated fields

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCloudWatchAlarmTemplateGroup

## Description
An Update on a medialive template group / rule template returns success but the mutated members are not written to state: the following Get still returns the pre-update values. This bundle has THREE causes, one per resource type; all must be closed.

Cause 1 of 3: UpdateCloudWatchAlarmTemplateGroup not persisted, observed on GetCloudWatchAlarmTemplateGroup (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#5).
AWS: Smithy: UpdateCloudWatchAlarmTemplateGroup is PATCH /prod/cloudwatch-alarm-template-groups/{Identifier}, HTTP 200, input member Description (`__stringMin0Max1024`, jsonName 'description'); its response and the subsequent GetCloudWatchAlarmTemplateGroup response both carry Description. The recording shows the group's description equal to the updated value after the PATCH.
Emulator: the GET after the PATCH returns 200 with $.description still 'initial description' instead of 'updated description'.
Why: a client that patches a description and re-reads it (or an idempotent configure-then-verify loop) sees its write silently lost, and reconciling tools loop forever or report drift.
Fix: the UpdateCloudWatchAlarmTemplateGroup handler/state write — apply the request's Description onto the stored group (and bump ModifiedAt).
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#5. (The same step also shows the epoch-timestamp/missing-tags serializer defect filed separately.)

Cause 2 of 3: UpdateEventBridgeRuleTemplateGroup not persisted, observed on GetEventBridgeRuleTemplateGroup (fixture medialive-eventbridge-rule-template-group-lifecycle, step task#5).
AWS: the rule-template-group update accepts `description` and the following Get reflects it; recording shows 'eb group updated'. HTTP 200.
Emulator: 200 with $.description still 'eb group initial'.
Why: as cause 1.
Fix: the UpdateEventBridgeRuleTemplateGroup handler/state write.
Traces: medialive-eventbridge-rule-template-group-lifecycle#task#5.

Cause 3 of 3: UpdateEventBridgeRuleTemplate not persisted, observed on GetEventBridgeRuleTemplate (fixture medialive-eventbridge-rule-template-lifecycle, step task#6).
AWS: the rule-template update accepts `description` and `eventType`, and the following Get reflects both; recording shows description 'updated channel alert rule' and eventType 'MEDIALIVE_CHANNEL_STATE_CHANGE'. HTTP 200.
Emulator: 200 with $.description still 'alert on channel alerts' and $.eventType still 'MEDIALIVE_CHANNEL_ALERT' — so an enum-valued member is dropped too, not only free text.
Why: as cause 1, and an unchanged eventType means the emulated rule template routes the wrong event class, so a caller's test of its own event wiring passes against AWS and fails here.
Fix: the UpdateEventBridgeRuleTemplate handler/state write — apply every present input member (description, eventType, event targets) and bump ModifiedAt.
Traces: medialive-eventbridge-rule-template-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (3 clusters, verdict=mismatch, expected and actual status both 200):
- GetCloudWatchAlarmTemplateGroup: $.description value_mismatch (expected 'updated description', got 'initial description').
- GetEventBridgeRuleTemplateGroup: $.description value_mismatch (expected 'eb group updated', got 'eb group initial').
- GetEventBridgeRuleTemplate: $.description value_mismatch (expected 'updated channel alert rule', got 'alert on channel alerts') and $.eventType value_mismatch (expected 'MEDIALIVE_CHANNEL_STATE_CHANGE', got 'MEDIALIVE_CHANNEL_ALERT').
In all three the preceding Update step itself answered successfully, so the loss is in the state write, not the request parse.
Smithy: UpdateCloudWatchAlarmTemplateGroup PATCH /prod/cloudwatch-alarm-template-groups/{Identifier} code 200, input {Description, Identifier}; response carries Description.

## Steps to Reproduce
1. POST /prod/cloudwatch-alarm-template-groups with description 'initial description'.
2. PATCH /prod/cloudwatch-alarm-template-groups/<id> with {"description": "updated description"}.
3. GET /prod/cloudwatch-alarm-template-groups/<id> — description is still 'initial description'.
4. Repeat for the EventBridge rule template group (description) and the EventBridge rule template (description + eventType).
5. Equivalent replay: fixture medialive-cloudwatch-alarm-template-group-lifecycle step task#5, plus the steps in trace_ids.

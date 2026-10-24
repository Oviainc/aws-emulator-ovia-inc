Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# ListTagsForResource answers 404 for CloudWatch-alarm-template and EventBridge-rule-template ARNs, so tags written at template create are unreadable

_Filed: 2026-10-24T01:07:38Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaLive's monitor TEMPLATES (CloudWatchAlarmTemplate, EventBridgeRuleTemplate) are taggable resources: each has its own ARN member and its own `Tags` map in the model, and the emulator already stores the tags a create supplied (services/service-com-amazonaws-medialive-medialive/src/state/alarm_templates.rs:117,324 and state/event_bridge_rule_templates.rs:193,533 keep and report `tags`).

Expected: GET /prod/tags/<template-arn> answers 200 with the template's tag map (empty map if it carries none), exactly as it does for a channel, input, input security group, SdiSource, signal map, multiplex or template GROUP.

Actual: the handler resolves the ARN through `ComAmazonawsMedialiveMedialiveState::resource_tags`, which dispatches on the private `tagged_resource` resolver (state.rs:2387-2427). That resolver enumerates channel, input, input security group, SdiSource, signal map, multiplex, alarm template GROUP and eventbridge rule template GROUP — the two TEMPLATE stores are missing. So a live template's ARN resolves to `None` and ListTagsForResource answers 404 NotFoundException (ops/list_tags_for_resource.rs:166-176) for a resource that exists and demonstrably carries tags, which are therefore observable only in the template's own Get/Create response and never through the tagging API.

## Evidence

Spec: `CloudWatchAlarmTemplateSummary` (smithy_query shape) declares `Arn -> __stringPatternArnMedialiveCloudwatchAlarmTemplate` (required) and `Tags -> TagMap`; the EventBridgeRuleTemplate shapes carry the same pair, and CreateCloudWatchAlarmTemplate / CreateEventBridgeRuleTemplate both accept `Tags`. ListTagsForResource is the service's only generic tag reader (GET /prod/tags/{ResourceArn}, 200, output `Tags`), so a taggable MediaLive resource must be resolvable through it. Handler: src/ops/list_tags_for_resource.rs lines 163-181 (404 when `resource_tags` misses). Root cause: src/state.rs `tagged_resource` lines 2387-2427 lists eight resource kinds and omits `alarm_templates` and `event_bridge_rule_templates`. No test asserts this (the crate has no handler tests; only src/service/error_envelope.rs has `#[test]`s). Same defect class as the already-filed healthlake/dsql tag-resolver issues.

## Steps to Reproduce

1. POST /prod/cloudwatch-alarm-template-groups to create a group (note its id).
2. POST /prod/cloudwatch-alarm-templates with groupIdentifier=<group>, the required alarm members and `tags: {"env": "prod"}` — the 201 response carries both `arn` and `tags`.
3. GET /prod/tags/<that arn> — expected 200 with {"tags": {"env": "prod"}}; the emulator answers 404 NotFoundException ("Resource not found: <arn>").
4. Repeat with an eventbridge rule template ARN (POST /prod/eventbridge-rule-templates, then GET /prod/tags/<arn>) — same 404, while the ARN of the template's GROUP resolves fine.

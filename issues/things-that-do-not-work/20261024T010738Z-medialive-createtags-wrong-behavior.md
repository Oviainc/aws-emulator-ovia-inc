Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateTags (and DeleteTags) cannot tag a CloudWatch-alarm-template or EventBridge-rule-template ARN: 404 for a live taggable resource

_Filed: 2026-10-24T01:07:38Z_
_Operation: CreateTags | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two write halves of the tagging API share the same ARN resolver as the read half and therefore have the same blind spot: a CloudWatchAlarmTemplate or EventBridgeRuleTemplate ARN names no resource they can reach.

Expected: POST /prod/tags/<template-arn> with a tag map answers 204 and the tag is then visible on the template (its Get response's `tags`) and through ListTagsForResource; DELETE /prod/tags/<template-arn>?tagKeys=... answers 204 and removes the named keys.

Actual: both handlers resolve the ARN through the private `resource_tags_mut` (src/state.rs:2471-2510), whose match arms cover channel, input, input security group, SdiSource, signal map, multiplex and the two template GROUPS but not the template stores themselves. `insert_resource_tags` therefore returns `TagRefusal::NoSuchResource` and CreateTags answers 404 NotFoundException (ops/create_tags.rs:195-205); DeleteTags answers the same 404 (ops/delete_tags.rs:183-192). A template's tags can consequently only ever be the ones its create request carried — they can never be added to or removed afterwards, even though the stored record has a live, mutable tag map.

## Evidence

Spec: CreateTags POST /prod/tags/{ResourceArn} (code 204, errors BadRequest/Forbidden/InternalServerError/NotFound) is the service's generic tag writer; the template shapes declare both `Arn` (pattern `ArnMedialiveCloudwatchAlarmTemplate` / the eventbridge equivalent) and `Tags`, and the create operations accept `Tags`, so templates are taggable resources. Handlers: src/ops/create_tags.rs lines 184-231, src/ops/delete_tags.rs lines 179-192. Root cause: `resource_tags_mut` in src/state.rs lines 2471-2510 (eight arms, neither template store among them) — the same omission as in `tagged_resource`. The template records DO hold writable tag maps (state/alarm_templates.rs:117, state/event_bridge_rule_templates.rs:193). No test covers it.

## Steps to Reproduce

1. Create an eventbridge rule template group, then POST /prod/eventbridge-rule-templates to create a template in it; keep the returned `arn`.
2. POST /prod/tags/<template arn> with body {"tags": {"owner": "team-a"}} — expected 204 and the tag visible on the next GET /prod/eventbridge-rule-templates/<id>; the emulator answers 404 NotFoundException.
3. DELETE /prod/tags/<template arn>?tagKeys=owner — expected 204; the emulator answers 404 NotFoundException.
4. Repeat with a cloudwatch-alarm-template ARN for the same result; the same calls against that template's GROUP ARN succeed, which shows the resolver, not the ARN syntax, is the cause.

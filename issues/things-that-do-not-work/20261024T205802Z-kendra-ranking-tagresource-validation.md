Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 438e57f04f6559b0a435f22a577d0c7dbfc3b209

# TagResource accepts tag keys and values outside the documented character set (including the reserved aws: prefix)

_Filed: 2026-10-24T20:58:02Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `438e57f04f6559b0a435f22a577d0c7dbfc3b209`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service documents the character set a tag key or value may use: "A tag key and value can consist of Unicode letters, digits, white space, and any of the following symbols: _ . : / = + - @." (`Tag` doc in the Smithy model, confirmed with smithy_query query=shape shape=Tag). The real service refuses a key or value containing anything else (e.g. `bad!key`, `100%`, `a,b`) with HTTP 400 `ValidationException`, and likewise refuses the AWS-reserved `aws:` key prefix.

Expected: `TagResource` with `Tags=[{Key:"bad!key",Value:"v"}]` is refused with `ValidationException` and the plan's tag set is unchanged; a key such as `aws:createdBy` is refused as reserved.

Actual: the emulator stores any key/value that satisfies only the modeled length bounds. `TagKey` (1..128) and `TagValue` (0..256) carry NO `@pattern` in the model, so the boundary validator cannot refuse them, and the handler adds no character-set check — it goes straight from the ARN lookup and the per-plan tag ceiling to `plan.put_tag(tag)`. The illegal tag is then persisted and reported verbatim by `ListTagsForResource`.

## Evidence

Spec: `Tag` documentation in smithy-model (`com.amazonaws.kendraranking#Tag`) names the permitted character set; `TagKey`/`TagValue` carry only `@length`, no `@pattern`, so nothing in the generated validation path enforces it. Handler: services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/ops/tag_resource.rs — after the `plan_by_arn_mut` lookup and the `MAX_TAGS_PER_PLAN` check it writes every supplied tag with `plan.put_tag(tag)`; the only refusals it can produce are `ResourceUnavailableException` and the tag-ceiling `ValidationException`. State: `RescoreExecutionPlan::put_tag` / `normalized_tag_key` in src/state.rs perform no character validation. Tests: tests/all/resource_tags.rs covers only LEGAL content (lengths at the bounds, Unicode letters, the documented symbols) and the length/count refusals — no row submits a character outside the documented set, so nothing asserts today's permissive behavior.

## Steps to Reproduce

1. CreateRescoreExecutionPlan with Name="tag-target" and note the returned Arn
2. Call TagResource with that ResourceARN and Tags=[{"Key":"bad!key","Value":"v$v"}]
3. Observe HTTP 200 instead of HTTP 400 ValidationException
4. Call ListTagsForResource for the same ARN and observe the illegal tag reported back
5. Repeat with Tags=[{"Key":"aws:createdBy","Value":"me"}] and observe the reserved-prefix key is accepted as well

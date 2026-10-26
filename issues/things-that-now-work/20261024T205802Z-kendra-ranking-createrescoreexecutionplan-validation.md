Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 438e57f04f6559b0a435f22a577d0c7dbfc3b209

# CreateRescoreExecutionPlan accepts Tags whose keys/values are outside the documented tag character set

_Filed: 2026-10-24T20:58:02Z_
_Operation: CreateRescoreExecutionPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `438e57f04f6559b0a435f22a577d0c7dbfc3b209`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateRescoreExecutionPlan` takes the same `TagList` as `TagResource`, and the same documented rule applies: "A tag key and value can consist of Unicode letters, digits, white space, and any of the following symbols: _ . : / = + - @." (`Tag` doc in the model). The real service refuses a create carrying an illegal tag key/value with HTTP 400 `ValidationException` and creates NO plan.

Expected: `CreateRescoreExecutionPlan` with `Tags=[{Key:"bad!key",Value:"v"}]` is refused with `ValidationException`; no plan is created and no id/ARN is minted.

Actual: the handler validates nothing about tag content — it mints the id/ARN, inserts the plan, then upserts every supplied tag — so the create succeeds (HTTP 200) and `ListTagsForResource` reports the illegal tag. Only the modeled `@length` bounds on `TagKey`/`TagValue`/`TagList` are enforced, at the boundary.

## Evidence

Spec: `com.amazonaws.kendraranking#Tag` documents the permitted character set; `TagKey`/`TagValue` have `@length` only (no `@pattern`), verified with smithy_query query=shape shape=Tag,TagKey,TagValue. Handler: services/.../src/ops/create_rescore_execution_plan.rs — after the quota check it inserts the plan and loops `for tag in tags { plan.put_tag(tag) }` with no content check (its own comment says the tags "go in through the same upsert the tagging operations use"). Tests: tests/all/resource_tags.rs and tests/all/control_plane_fidelity.rs assert only the length/pattern constraints the model carries (Name pattern/length, tag lengths and counts); no test submits a tag key outside the documented character set.

## Steps to Reproduce

1. Call CreateRescoreExecutionPlan with Name="plan" and Tags=[{"Key":"bad!key","Value":"v"}]
2. Observe HTTP 200 with an Id/Arn instead of HTTP 400 ValidationException
3. Call ListTagsForResource with the returned Arn and observe the illegal tag persisted
4. Call ListRescoreExecutionPlans and observe the plan was created even though the request should have been refused whole

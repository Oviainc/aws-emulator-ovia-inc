Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# UpdateLifecyclePolicy accepts interval values outside the documented ranges

_Filed: 2026-10-23T23:28:46Z_
_Operation: UpdateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateLifecyclePolicy` carries the same `CreateInterval`/`RetainInterval` members (and the same `CreateRule` shape) as the create, with the same documented ranges, and models `InvalidRequestException` (400) for a refused parameter. Expected: a PATCH that sets `CreateInterval` above 7, `RetainInterval` to 1 or above 14, a `RetainInterval` not greater than the effective `CreateInterval`, or a schedule `CreateRule$Interval` outside {1,2,3,4,6,8,12,24} is refused with 400 and the stored policy is left untouched. Actual: the handler validates only the create-rule cron/interval mutual exclusion; every other value is written straight onto the record, so the emulator holds — and subsequently reports — a configuration AWS would have rejected.

## Evidence

Spec: `com.amazonaws.dlm#UpdateLifecyclePolicyRequest$RetainInterval` documentation (smithy_query doc) — "The retention period can range from 2 to 14 days, but it must be greater than the creation frequency"; `$CreateInterval` — "The creation frequency can range from 1 to 7 days"; `CreateRule$Interval` — "The supported values are 1, 2, 3, 4, 6, 8, 12, and 24". `UpdateLifecyclePolicy` declares `InvalidRequestException` (HTTP 400).
Handler: `src/ops/update_lifecycle_policy.rs` lines ~186-243 — after the single `schedule_with_conflicting_create_rule` check, `input.create_interval` / `input.retain_interval` are assigned unconditionally (`policy.create_interval = Some(create_interval)` etc.).
Tests: `tests/all/error_paths.rs` only exercises "UpdateLifecyclePolicy: a RetainInterval below its minimum" (the model `@range(min:1)`) and the create-rule conflict; nothing asserts the documented upper bounds or the retain>create relation.

## Steps to Reproduce

1. POST /policies to create a default VOLUME policy with CreateInterval 3 and RetainInterval 14
2. PATCH /policies/{PolicyId} with {"CreateInterval": 30, "RetainInterval": 1}
3. Observe HTTP 200 instead of 400 InvalidRequestException (Code InvalidParameter)
4. GET /policies/{PolicyId} and observe PolicyDetails.CreateInterval=30 / RetainInterval=1 persisted

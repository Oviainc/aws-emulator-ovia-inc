Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# CreateLifecyclePolicy accepts interval values outside the documented ranges

_Filed: 2026-10-23T23:28:46Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The documented numeric rules for a default policy's creation/retention cadence, and for a custom schedule's creation interval, are not enforced. Expected: AWS DLM answers HTTP 400 `InvalidRequestException` (Code `InvalidParameter`) for a create whose
- `CreateInterval` is greater than 7 ("The creation frequency can range from 1 to 7 days"),
- `RetainInterval` is 1 or greater than 14 ("The retention period can range from 2 to 14 days"),
- `RetainInterval` is not greater than `CreateInterval` ("it must be greater than the creation frequency to ensure that the policy retains at least 1 snapshot or AMI at any given time"),
- custom schedule `CreateRule$Interval` is a value outside the supported set ("The supported values are 1, 2, 3, 4, 6, 8, 12, and 24").

Actual: the emulator refuses only what the Smithy `@range` traits express (`CreateInterval`/`RetainInterval` min 1, `CreateRule$Interval` min 1), so e.g. `DefaultPolicy=VOLUME, CreateInterval=30, RetainInterval=1` is accepted with HTTP 200, a policy AWS would never have created is persisted, and every later `GetLifecyclePolicy`/`GetLifecyclePolicies` reports it.

## Evidence

Spec: `com.amazonaws.dlm#CreateLifecyclePolicyRequest$CreateInterval` documentation (via smithy_query doc) — "The creation frequency can range from 1 to 7 days"; `$RetainInterval` — "The retention period can range from 2 to 14 days, but it must be greater than the creation frequency"; `com.amazonaws.dlm#CreateRule$Interval` — "The supported values are 1, 2, 3, 4, 6, 8, 12, and 24". The operation models `InvalidRequestException` (httpError 400) with `Code`/`Message` for exactly this class of refusal.
Handler: `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs` — `handle` validates only the default-policy body-vs-PolicyDetails duplication, the `CreateRule` cron/interval mutual exclusion and reserved `aws:` tag keys; no interval magnitude or cross-member interval comparison is ever made before `store.policies.insert`.
Tests: `tests/all/error_paths.rs` covers only the `@range(min:1)` floor ("a CreateInterval below its minimum") and the create-rule mutual exclusion; no test asserts the documented upper bounds, the 2-day retention floor, the retain>create rule, or the `CreateRule$Interval` value set (grep for `CreateInterval|RetainInterval` in `tests/`).

## Steps to Reproduce

1. POST /policies with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "too frequent", "State": "ENABLED", "DefaultPolicy": "VOLUME", "CreateInterval": 30, "RetainInterval": 1}
2. Observe HTTP 200 with a minted PolicyId instead of 400 InvalidRequestException (Code InvalidParameter)
3. GET /policies/{PolicyId} and observe the out-of-range PolicyDetails.CreateInterval=30 / RetainInterval=1 reported back
4. Also POST /policies with a custom PolicyDetails whose Schedules[0].CreateRule = {"Interval": 5, "IntervalUnit": "HOURS"} and observe HTTP 200 for an unsupported interval value

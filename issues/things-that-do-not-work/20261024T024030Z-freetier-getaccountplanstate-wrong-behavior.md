Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# GetAccountPlanState emits accountPlanExpirationDate as an epoch-seconds number instead of a date-time string

_Filed: 2026-10-24T02:40:30Z_
_Operation: GetAccountPlanState | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetAccountPlanStateResponse$accountPlanExpirationDate` is a `Timestamp` member the model marks `@timestampFormat("date-time")`, so on the awsJson wire the value is an RFC 3339 / ISO-8601 STRING (e.g. `"2026-04-21T00:00:00Z"`). The emulator serializes it as a bare JSON NUMBER of epoch seconds (e.g. `1776729600`).

Expected: `{"accountId":"...","accountPlanType":"FREE","accountPlanStatus":"ACTIVE","accountPlanRemainingCredits":{...},"accountPlanExpirationDate":"<RFC 3339 string>"}`.

Actual: `"accountPlanExpirationDate": <number>`. A client that deserializes this member per the model reads a value of the wrong JSON type — an SDK parsing a `date-time` member given a number fails or mis-parses, so the whole response is unusable even though the status is 200.

Root cause is local to this crate: `src/ops/types/get_account_plan_state_types.rs` builds `GetAccountPlanStateOutput` with the plain `#[smithy_macros::smithy(shape = ...)]` derive, and the vendored codegen never emits `.with_timestamp_format(..)` onto a member schema, so the JSON codec falls back to the protocol default (epoch seconds). Other services in this workspace compensate by hand (e.g. `service-com-amazonaws-bedrockagentruntime-.../src/ops/types/date_time_wire.rs`, `service-com-amazonaws-bcmdataexports-.../src/ops/types/shared_types.rs`); this crate has no such compensation.

## Evidence

Spec: `smithy_query(query="operation",operation="GetAccountPlanState")` reports `accountPlanExpirationDate: {"target": "Timestamp", "timestampFormat": "date-time"}` — the member-level `@timestampFormat("date-time")` makes the string form part of the contract. Handler/types: services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_plan_state.rs lines 182-188 returns `plan.expiration_date` (an `aws_smithy_types::DateTime`), and src/ops/types/get_account_plan_state_types.rs lines 19-27 declares the output with the plain derive macro, which drops `@timestampFormat` (grep: `vendor/smithy-macros/src` contains no `timestampFormat`; `vendor/smithy-schema-server/src/json.rs` renders epoch seconds when the member schema reports no format). Workspace-level generator defect already recorded in issues/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md, with per-service fixes filed and fixed for other services (e.g. notificationscontacts, verifiedpermissions). Tests: tests/all/free_tier.rs lines 233 and 300 assert `first["accountPlanExpirationDate"].as_f64().is_some()` — i.e. the existing tests pin the epoch-NUMBER form, so nothing in this crate asserts the modelled date-time rendering.

## Steps to Reproduce

1. Boot the emulator with the freetier service mounted.
2. POST / with `X-Amz-Target: AWSFreeTierService.UpgradeAccountPlan` and body {"accountPlanType": "FREE"} to establish the plan.
3. POST / with `X-Amz-Target: AWSFreeTierService.GetAccountPlanState` and body {}.
4. Inspect the JSON body: `accountPlanExpirationDate` is a number (epoch seconds) where the model's `@timestampFormat("date-time")` requires an RFC 3339 string.
5. Deserialize the response with an AWS SDK freetier client and observe the timestamp member fails to parse / is not the modelled wire form.

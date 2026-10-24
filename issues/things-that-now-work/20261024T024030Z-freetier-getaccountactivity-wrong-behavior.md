Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# GetAccountActivity emits expiresAt/startedAt/completedAt as epoch-seconds numbers instead of date-time strings

_Filed: 2026-10-24T02:40:30Z_
_Operation: GetAccountActivity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three timestamp members of `GetAccountActivityResponse` — `expiresAt`, `startedAt`, `completedAt` — are marked `@timestampFormat("date-time")` in the model, so each must travel as an RFC 3339 / ISO-8601 STRING. The emulator serializes them as bare JSON NUMBERS of epoch seconds.

Expected (for a COMPLETED activity): `{"activityId":"...","status":"COMPLETED","startedAt":"<RFC 3339 string>","completedAt":"<RFC 3339 string>", ...}`.

Actual: `"startedAt": <number>, "completedAt": <number>` (and `"expiresAt": <number>` for the EXPIRING activity). The status code and the member selection logic are right; the JSON type of the values is not, so a client deserializing per the model reads the wrong type for these members and an SDK rejects or mis-parses the response.

Root cause is local to this crate: `src/ops/types/get_account_activity_types.rs` declares `GetAccountActivityOutput` with the plain `#[smithy_macros::smithy(shape = ...)]` derive, and the vendored codegen never attaches `with_timestamp_format(..)` to the member schemas, so the JSON codec falls back to the protocol default of epoch seconds. Other services in this workspace already compensate by hand-writing the member schemas (see `date_time_wire.rs` in the bedrock-agent-runtime / codeguru-profiler / appconfig crates); this crate does not.

## Evidence

Spec: `smithy_query(query="operation",operation="GetAccountActivity")` reports `expiresAt`, `startedAt` and `completedAt` each as `{"target": "Timestamp", "timestampFormat": "date-time"}`. Handler/types: services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_activity.rs lines 223-234 returns the stored `DateTime`s, and src/ops/types/get_account_activity_types.rs lines 21-34 uses the plain derive macro; `vendor/smithy-macros/src` contains no `timestampFormat` handling at all, and `vendor/smithy-schema-server/src/json.rs` falls back to epoch seconds when a member schema carries no format. The generator-level defect is recorded in issues/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md and has been fixed per-service elsewhere in this workspace. Tests: tests/all/free_tier.rs line 667 asserts `detail["completedAt"].as_f64().is_some() && detail["startedAt"].as_f64().is_some()` — the existing test pins the epoch-NUMBER form, so no test asserts the modelled date-time rendering.

## Steps to Reproduce

1. Boot the emulator with the freetier service mounted.
2. POST / with `X-Amz-Target: AWSFreeTierService.ListAccountActivities` and body {"maxResults": 1000}; pick the activityId whose status is COMPLETED.
3. POST / with `X-Amz-Target: AWSFreeTierService.GetAccountActivity` and body {"activityId": "<that id>"}.
4. Inspect the JSON body: `startedAt` and `completedAt` are numbers (epoch seconds) where the model's `@timestampFormat("date-time")` requires RFC 3339 strings; repeat with the EXPIRING activity to see the same for `expiresAt`.
5. Deserialize the response with an AWS SDK freetier client and observe the timestamp members are not the modelled wire form.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# GetServiceSettings omits the ServiceSettings structure entirely for a Region no update has configured

_Filed: 2026-10-24T21:30:38Z_
_Operation: GetServiceSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Quick Setup's service settings are not a creatable resource: `GetServiceSettings` is modeled with NO `ResourceNotFoundException` and NO `ValidationException` (only AccessDenied/Conflict/InternalServer/Throttling), so every account and Region always has a settings record — one whose `ExplorerEnablingRoleArn` is simply unset until an `UpdateServiceSettings` names one. The expected response for a Region that was never configured is therefore a 200 carrying the `ServiceSettings` structure with no `ExplorerEnablingRoleArn` member (`{"ServiceSettings":{}}`), which is what a client's `response.service_settings().explorer_enabling_role_arn()` chain is written against.

The emulator instead drops the whole `ServiceSettings` member from the response when the store holds no record for the signed Region, so an SDK caller gets `None` for `service_settings()` itself and a raw-HTTP caller gets `{}` as the entire body. A client that reads `ServiceSettings` unconditionally (the common shape, since the member can never be 'not found') sees a null/absent struct instead of an empty one, and cannot distinguish a never-configured Region from a malformed response.

Expected: 200 with `ServiceSettings` present and `ExplorerEnablingRoleArn` absent for a Region no update has configured.
Actual: 200 with the `ServiceSettings` member absent altogether.

## Evidence

Spec grounding: `smithy_query(query="operation", operation="GetServiceSettings")` — the operation binds only AccessDeniedException/ConflictException/InternalServerException/ThrottlingException, i.e. there is no modeled way for settings to be 'absent'; `ServiceSettings` (shape `com.amazonaws.ssmquicksetup#ServiceSettings`) has exactly one optional member, `ExplorerEnablingRoleArn`, so the unconfigured state is expressible inside the structure and needs no omission of the structure itself.

Handler evidence: `src/ops/get_service_settings.rs:155-166` — `let stored = state.lock().service_settings.get(&region).map(ServiceSettingsRecord::to_service_settings);` then `Ok(GetServiceSettingsOutput { service_settings: stored })`, with the in-code comment stating the omission is deliberate ('Absent, not an empty structure, when this region has never been configured'). The store (`src/state.rs`, `service_settings: BTreeMap<String /* region */, ServiceSettingsRecord>`) only gains an entry on a successful `UpdateServiceSettings` that names a role ARN (`src/ops/update_service_settings.rs:179-186` returns early and writes nothing for an input naming no member).

Test coverage: `tests/all/service_settings_and_types.rs:404` (`an_unconfigured_region_reports_no_service_settings`) PINS the current behavior — it asserts an unconfigured Region must report no `ServiceSettings` at all — so that assertion would have to be updated along with the handler. No test asserts the empty-structure form.

## Steps to Reproduce

1. Boot the emulator with an empty store.
2. GET /serviceSettings signed for us-west-2 (`ssm-quicksetup.us-west-2.amazonaws.com`, SigV4 credential scope us-west-2).
3. Observe the 200 body is `{}` — the `ServiceSettings` member is absent, so `GetServiceSettingsOutput.service_settings` is None for an SDK caller.
4. PUT /serviceSettings with {"ExplorerEnablingRoleArn":"arn:aws:iam::000000000000:role/explorer"} signed for us-west-2, then GET /serviceSettings signed for eu-west-1: that Region still reports no `ServiceSettings` member at all rather than an empty settings structure.

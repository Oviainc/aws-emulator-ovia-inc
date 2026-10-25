Issue-Severity: HIGH
Issue-Gap-Class: missing
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Implement GetGlobalSettings: the operation has no handler at all

_Filed: 2026-10-24T01:59:23Z_
_Operation: GetGlobalSettings | Class: missing_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetGlobalSettings` (GET /global-settings) is modelled by this service and is on the operation checklist, but the crate contains NO handler for it: there is no `src/ops/get_global_settings.rs`, the operation is not in `src/ops/mod.rs`, and it is not registered in `src/service.rs`. The scaffold generator declined to emit even a stub for it because its input is the Smithy `Unit` shape.

Expected: `GetGlobalSettings` answers HTTP 200 with a `GetGlobalSettingsOutput` computed from the account-level settings this emulator holds — `OrganizationSharingStatus` (ENABLED|DISABLED), `DiscoveryIntegrationStatus` (ENABLED|DISABLED) and `JiraConfiguration` (`AccountJiraConfigurationOutput`) — with sane defaults before anything has been configured, and reflecting whatever `UpdateGlobalSettings` later records (that operation is a separate, already-filed stub).

Actual: the request names no operation this service can route, so it is answered by the framework's unroutable-operation path (a routing error whose shape is not even this service's modelled error envelope), not by a modelled response and not by the honest scaffold 501 every other unimplemented operation of this service returns. A client reading global settings — the first call the Well-Architected console makes — cannot work at all.

## Evidence

Smithy model: `smithy_query(query="operation", operation="GetGlobalSettings")` shows `http {method: GET, uri: /global-settings}`, `input_shape: Unit`, output members `OrganizationSharingStatus`, `DiscoveryIntegrationStatus`, `JiraConfiguration`, errors AccessDenied/InternalServer/Throttling/Validation. Emulator: `glob services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/*.rs` lists no `get_global_settings.rs`; `src/ops/SKIPPED_STUBS.md` line 21 records `// SKIPPED: GetGlobalSettings — unsupported shape construct: operation 'GetGlobalSettings' has a Unit/absent input, which the generator does not emit yet`; `src/service.rs` line 214 registers only `update_global_settings::UpdateGlobalSettings`. Test coverage: no test under `tests/all/` mentions GlobalSettings (grep for `GlobalSettings|global_settings` matches only OPERATIONS.md, service.rs and the UpdateGlobalSettings stub).

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. Call GetGlobalSettings (GET /global-settings)
3. Observe the framework's unroutable-operation error instead of a 200 `GetGlobalSettingsOutput` (and instead of the scaffold 501 other unimplemented operations give)
4. Hand-author `src/ops/get_global_settings.rs` using a rendered sibling as the pattern, add it to `src/ops/mod.rs` and register it in `src/service.rs`

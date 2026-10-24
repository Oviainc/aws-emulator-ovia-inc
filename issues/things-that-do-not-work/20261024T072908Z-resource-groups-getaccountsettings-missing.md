Issue-Severity: HIGH
Issue-Gap-Class: missing
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# GetAccountSettings has no handler at all — the operation is modeled but unwired

_Filed: 2026-10-24T07:29:08Z_
_Operation: GetAccountSettings | Class: missing_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetAccountSettings is bound to the Ardi (Resource Groups) service in the model (POST /get-account-settings -> 200, output AccountSettings), but the crate contains NO handler file for it: `src/ops/` has 22 operation files and `get_account_settings.rs` is not among them, and `src/ops/SKIPPED_STUBS.md` records why ('SKIPPED: GetAccountSettings — unsupported shape construct: operation GetAccountSettings has a Unit/absent input'). Because the operation is not in the service's generated routing table, a request to POST /get-account-settings is not even answered by a scaffold: it falls into the framework's unroutable-operation path. Expected: 200 with `AccountSettings` computed from persisted per-account settings (`GroupLifecycleEventsDesiredStatus`, `GroupLifecycleEventsStatus`, optional `GroupLifecycleEventsStatusMessage`), and a value previously set through UpdateAccountSettings must be visible here. Actual: the operation does not exist in the emulator. Note the service state (`src/state.rs` `ComAmazonawsResourcegroupsArdiState`) holds only `groups` — there is no account-settings field at all, so implementing this also requires adding that state (shared with UpdateAccountSettings).

## Evidence

Spec: model binds `com.amazonaws.resourcegroups#GetAccountSettings` with output member `AccountSettings` (AccountSettings: GroupLifecycleEventsDesiredStatus, GroupLifecycleEventsStatus one of ACTIVE|INACTIVE|IN_PROGRESS|ERROR, GroupLifecycleEventsStatusMessage); OPERATIONS.md lines 47-52. Handler: no `services/service-com-amazonaws-resourcegroups-ardi/src/ops/get_account_settings.rs` exists; `src/ops/SKIPPED_STUBS.md` line 21 names it as declined by the stub generator (it is NOT a SCAFFOLDED stub, so it is not covered by the census stub issues). State: `src/state.rs` has no account-settings record. Tests: no test in `tests/all/` references account settings (grep for 'account_settings'/'AccountSettings' matches only OPERATIONS.md, update_account_settings.rs and the model).

## Steps to Reproduce

1. Start the emulator with the resource-groups service mounted.
2. POST /get-account-settings with an empty JSON body and a resource-groups SigV4 scope.
3. Observe the request is not routed to any handler (framework unroutable-operation answer) instead of a 200 carrying AccountSettings.
4. Also observe there is no state field for account settings, so no value could be reported even if routed.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# An Automatic Management opt-in type of NotifyAndAdjust is accepted and stored although the service documents only NotifyOnly as available

_Filed: 2026-10-24T18:39:22Z_
_Operation: UpdateAutoManagement | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAutoManagement` (and `StartAutoManagement`, which shares the code path) stores whatever `OptInType` the request carries, so `OptInType=NotifyAndAdjust` is accepted with HTTP 200 and `GetAutoManagementConfiguration` then reports the account as opted in to the auto-adjust mode.

Expected (spec): `OptInType` is documented as "There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available." — so AWS refuses `NotifyAndAdjust` with one of the operation's modeled client errors (`IllegalArgumentException`, 400) and the stored configuration can only ever report `NotifyOnly`.

Actual: `src/ops/update_auto_management.rs` lines 147-149 assigns `config.opt_in_type = opt_in_type` unconditionally, and `src/ops/start_auto_management.rs` lines 146-154 stores `opt_in_type: input.opt_in_type` unconditionally; the only filter applied anywhere is the enum membership check the schema validator already did. The emulator therefore reports a mode AWS does not serve, and nothing in it ever auto-adjusts a quota (no `RequestedServiceQuotaChange` with `RequestType=AutomaticManagement` is ever minted), so a client that branches on the reported `OptInType` believes auto-adjustment is enabled when neither AWS nor the emulator would do it.

## Evidence

Model: `smithy_query(query="doc", shape="UpdateAutoManagementRequest", member="OptInType")` returns "Information on the opt-in type for your Automatic Management configuration. There are two modes: Notify only and Notify and Auto-Adjust. Currently, only NotifyOnly is available." (identical text on `StartAutoManagementRequest$OptInType`). Both operations model `IllegalArgumentException` (400) "Invalid input was provided" (OPERATIONS.md lines 153-157 and 183-187).

Handlers: `src/ops/update_auto_management.rs` lines 114-158 (no check on `input.opt_in_type`); `src/ops/start_auto_management.rs` lines 133-157 (same). `src/state.rs` `AutoManagementConfig.opt_in_type` is a plain `String` copied through `set_auto_management`.

Test coverage: the current lenient behaviour is pinned — `tests/all/auto_management.rs` rows at lines 192, 219, 312, 336, 432 and 515-524 submit `OptInType=NotifyAndAdjust` through `StartAutoManagement`/`UpdateAutoManagement` and assert it reads back, and `tests/all/auto_management_configuration.rs` line 368 does the same for the stored configuration; those rows must move with the fix.

Not a duplicate: the open issue `20261024T142232Z-service-quotas-startautomanagement-validation` is scoped to the unresolved `ExclusionList` entries on `StartAutoManagement` and says nothing about `OptInType`; the fix for this one belongs on the shared opt-in write path used by both operations.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.StartAutoManagement` with `{"OptInLevel":"ACCOUNT","OptInType":"NotifyOnly"}` -> HTTP 200.
2. POST `UpdateAutoManagement` with `{"OptInType":"NotifyAndAdjust"}` -> HTTP 200, nothing refused (AWS: 400 `IllegalArgumentException`, since only `NotifyOnly` is available).
3. POST `GetAutoManagementConfiguration` `{}` -> the body reports `"OptInType":"NotifyAndAdjust"` and `"OptInStatus":"ENABLED"`, a configuration AWS cannot currently hold; no quota is ever auto-adjusted as a result.
4. The same acceptance happens on the first call: POST `StartAutoManagement` with `{"OptInLevel":"ACCOUNT","OptInType":"NotifyAndAdjust"}` -> HTTP 200.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3302ef7557ecef1ce1978c32967329b865cc9c8a

# UpdateAccountSettings records the audit outcome "settings-updated" for a request its own comment says changes nothing

_Filed: 2026-10-28T23:48:24Z_
_Operation: UpdateAccountSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3302ef7557ecef1ce1978c32967329b865cc9c8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAccountSettings` is one of the few handlers in this crate that participates in the state module's audit facility, and it is the ONLY writer of the account-settings resource, so its audit line is the only record that the account's OCU capacity limits were changed.

`UpdateAccountSettingsRequest$capacityLimits` is optional (it is the request's only member), and the handler deliberately treats an absent `capacityLimits` as a no-op — its own comment says so in as many words. But the `audit(...)` call two lines below the conditional write is UNCONDITIONAL, so a request that names no `capacityLimits` (or one that names an empty `capacityLimits` with both members absent, which `AccountSettingsRecord::apply` likewise leaves untouched) still emits `outcome=settings-updated`.

That token is defined in this crate's own state module as meaning the settings "were written". So the audit log reports a write of the account's capacity limits for a call during which `store.account_settings` was provably not touched, and an operator reading the log cannot tell a real limit change from a probe. The crate's other audited exits each name the outcome that actually happened (`IdempotentReplay` for a replay that writes nothing, `ValidationRefused`, `NotFound`, `VersionMismatch`, `AlreadyExists`, `QuotaExceeded`, `StoreUnreachable`), so the handler is not short of a vocabulary for "nothing was written" — the unconditional call simply does not consult whether the write happened.

The defect is confined to the audit record: the 200 body (the stored settings, correctly reported) and the stored state are unchanged either way, so no response and no existing test moves.

## Evidence

The handler states the no-op rule and then audits a write regardless — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/update_account_settings.rs:167-181:
```
        let mut store = state.lock();
        // Both `CapacityLimits` members carry `@range(min: 2)`, so the boundary validator
        // has already refused anything below that floor; the model states no ceiling and
        // no increment, so this handler adds neither and stores the value as given. A
        // request that names no `capacityLimits` at all changes nothing and reports the
        // settings as they stand.
        if let Some(limits) = &input.capacity_limits {
            store.account_settings.apply(limits);
        }
        crate::state::audit(
            Self::NAME,
            ctx,
            AuditOutcome::SettingsUpdated,
            Some(resource),
        );
```
Note that `audit(..., AuditOutcome::SettingsUpdated, ...)` sits OUTSIDE the `if let Some(limits)` that performs the only write.

What that outcome token is defined to mean, in this crate's own state module — src/state.rs:2976-2977:
```
    /// The account's own settings were written (`UpdateAccountSettings`).
    SettingsUpdated,
```
and the token it is logged as — src/state.rs:3000: `Self::SettingsUpdated => "settings-updated",`.

That even a non-absent `capacityLimits` can write nothing — src/state.rs:1068-1078:
```
    /// Apply the limits an `UpdateAccountSettings` request named, leaving a limit the
    /// request did NOT name at its stored value: both `CapacityLimits` members are
    /// optional, so a request may raise or lower one limit without restating the other.
    pub fn apply(&mut self, limits: &CapacityLimits) {
        if let Some(value) = limits.max_indexing_capacity_in_ocu {
            self.max_indexing_capacity_in_ocu = value;
        }
        if let Some(value) = limits.max_search_capacity_in_ocu {
            self.max_search_capacity_in_ocu = value;
        }
    }
```
so `capacityLimits: {}` reaches `apply` and changes neither field, yet is logged as `settings-updated` too.

That `capacityLimits` really is optional (so this request shape is reachable) — `smithy_query(query="shape", shape="UpdateAccountSettingsRequest")`: `{"type": "structure", "members": {"capacityLimits": {"target": "CapacityLimits"}}}` — no `required`.

The crate does have outcomes for exits that write nothing, and other handlers use them — src/state.rs:2971-2973 (`/// A replayed `clientToken` answered the record its first call created; nothing was written.` `IdempotentReplay,`), used e.g. at src/ops/update_access_policy.rs:262-267.

Test coverage: tests/all/account_settings.rs covers `get_account_settings_reports_the_stored_limits_and_changes_nothing` (line 94), `update_account_settings_stores_each_named_limit_exactly` (line 167) and `update_account_settings_refuses_a_capacity_below_the_published_minimum` (line 279) — all assert response bodies and stored limits; no test in tests/all/ asserts anything about audit records (the facility is `tracing`-only). So correcting the outcome leaves every response and every existing assertion unchanged.

## Steps to Reproduce

1. Run the emulator with the `emulator::audit` tracing target enabled.
2. Call UpdateAccountSettings with body {"capacityLimits":{"maxIndexingCapacityInOCU":20,"maxSearchCapacityInOCU":20}} -> 200, and observe one audit line: operation=UpdateAccountSettings resource=account-settings outcome=settings-updated. This one is correct: src/state.rs:1072-1077 wrote both fields.
3. Call UpdateAccountSettings with body {} (no capacityLimits) -> 200, reporting the limits as they stand.
4. Observe a SECOND audit line with the identical outcome=settings-updated, although src/ops/update_account_settings.rs:170-172 states "A request that names no `capacityLimits` at all changes nothing" and the `if let Some(limits)` at src/ops/update_account_settings.rs:173 was not entered — nothing was written, yet the log says the account's settings were.
5. Call UpdateAccountSettings with body {"capacityLimits":{}} -> 200: a third identical settings-updated line, although `AccountSettingsRecord::apply` (src/state.rs:1071-1078) assigned neither field.
6. Confirm nothing changed: GetAccountSettings -> accountSettingsDetail.capacityLimits is still {20,20} from step 2, so two of the three logged "settings-updated" records describe writes that never happened.

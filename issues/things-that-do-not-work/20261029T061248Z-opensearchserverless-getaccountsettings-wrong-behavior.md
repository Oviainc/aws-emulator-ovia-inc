Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a30e9f7a31e09aa2b907716a7c9b6be0706cfea

# GetAccountSettings discards ctx and emits no audit record, although this crate carries an audit resource name for exactly the resource it reads

_Filed: 2026-10-29T06:12:48Z_
_Operation: GetAccountSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a30e9f7a31e09aa2b907716a7c9b6be0706cfea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has an audit facility in its own state module (`crate::state::audit`, `AuditOutcome`, and one `resource` name per audited resource), and the account-settings resource is one of the resources it names: `crate::state::AUDIT_ACCOUNT_SETTINGS_RESOURCE` exists solely for it, and `UpdateAccountSettings` — the writer of that same resource — uses it, stating the discipline in its own body ("Every exit leaves exactly one record, so this state-changing call is observable whatever it answered").

`GetAccountSettings`, the READ of the very same resource, throws the request context away on its first line (`let _ = (&input, ctx);`) — `ctx` being the one value `crate::state::audit` builds a record from — so the read leaves no record at all.

That is an inconsistency inside this crate, not a judgement about audit policy, because the crate's other no-input, account-wide READ does audit: `GetPoliciesStats` discards only `input` (`let _ = input;`), deliberately keeping `ctx`, and emits `AuditOutcome::PolicyRead` on success and `AuditOutcome::StoreUnreachable` on the store-unreachable exit. `GetAccessPolicy` and `BatchGetLifecyclePolicy` likewise audit `PolicyRead` for a read. `AuditOutcome::PolicyRead` is documented as "A stored policy was reported to a reader" and is the outcome every audited reader in this crate already uses, so nothing new has to be invented: `GetAccountSettings` needs the two calls its own sibling writer and its own sibling reader already make (`StoreUnreachable` on the unreachable-store exit, the reader outcome on success, with `Some(crate::state::AUDIT_ACCOUNT_SETTINGS_RESOURCE)`).

No response body and no status changes either way — the audit facility is `tracing`-only.

This is NOT the already-open issue `20261028T203217Z-opensearchserverless-createsecuritypolicy-wrong-behavior`: that one is scoped to the ten security-policy and security-configuration handlers and explicitly lists `UpdateAccountSettings` and `GetPoliciesStats` as handlers that DO follow the discipline. It never mentions `GetAccountSettings`, which is a different operation on a different resource.

## Evidence

The resource name this crate carries for exactly this resource — services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs:3162-3165:

    /// The `resource` field naming the account's own settings, which `UpdateAccountSettings`
    /// writes. A single resource per account, so it is a constant rather than a function:
    /// nothing request-derived identifies it.
    pub const AUDIT_ACCOUNT_SETTINGS_RESOURCE: &str = "account-settings";

The sibling WRITER of the same resource, which uses it and states the rule — src/ops/update_account_settings.rs:147-151:

        // The settings are account-wide, so nothing in the request's metadata selects what
        // is written; `ctx` is what the audit record below identifies the caller from.
        // Every exit leaves exactly one record, so this state-changing call is observable
        // whatever it answered.
        let resource = crate::state::AUDIT_ACCOUNT_SETTINGS_RESOURCE;

…and src/ops/update_account_settings.rs:176-181:

        crate::state::audit(
            Self::NAME,
            ctx,
            AuditOutcome::SettingsUpdated,
            Some(resource),
        );

The READ of the same resource, discarding the value the record is built from — src/ops/get_account_settings.rs:134-136:

        // `GetAccountSettingsRequest` models no members, and the read is account-wide, so
        // neither the input nor the request's metadata selects anything.
        let _ = (&input, ctx);

…and its only other exit, src/ops/get_account_settings.rs:146-150:

        // A REPORTING operation: the guard is taken for reading only, so a read can never
        // change what a later caller sees.
        let settings = state.lock().account_settings;
        Ok(GetAccountSettingsOutput {
            account_settings_detail: Some(settings.detail()),

The crate's OTHER no-input, account-wide read, which keeps `ctx` on purpose and audits — src/ops/get_policies_stats.rs:143-145:

        // `GetPoliciesStatsRequest` models no members: the statistics are account-wide, so
        // there is nothing in the request to read.
        let _ = input;

…src/ops/get_policies_stats.rs:148: `crate::state::audit(Self::NAME, ctx, AuditOutcome::StoreUnreachable, None);` and src/ops/get_policies_stats.rs:195: `crate::state::audit(Self::NAME, ctx, AuditOutcome::PolicyRead, None);`

The outcome a read is recorded under — src/state.rs:3082-3083:

    /// A stored policy was reported to a reader.
    PolicyRead,

The record is built from `ctx` — src/state.rs:3130-3136:

    pub fn audit(
        operation: &str,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        outcome: AuditOutcome,
        resource: Option<&str>,
    ) {
        let parts = ctx.parts();

Test coverage: no test in tests/all/ asserts anything about audit records, and the two tests that pin this area assert only state and response bodies — tests/all/account_settings.rs:94 `get_account_settings_reports_the_stored_limits_and_changes_nothing` and tests/all/policies_stats.rs:491 `reading_the_statistics_changes_nothing_a_later_caller_sees` (the latter passing today for an operation that DOES audit, which shows a `tracing` line is not a state change). Adding the missing calls therefore leaves every response and every existing assertion unchanged.

## Steps to Reproduce

1. Run the emulator with the `emulator::audit` tracing target enabled.
2. Call UpdateAccountSettings with {"capacityLimits":{"maxIndexingCapacityInOCU":12}} -> 200, and observe one audit line: operation=UpdateAccountSettings resource=account-settings outcome=settings-updated (src/ops/update_account_settings.rs:176-181).
3. Call GetPoliciesStats (empty body) -> 200, and observe one audit line: operation=GetPoliciesStats resource=- outcome=policy-read (src/ops/get_policies_stats.rs:195).
4. Call GetAccountSettings (empty body) -> 200, reporting the limits step 2 stored, and observe NO audit line at all — `ctx` was discarded at src/ops/get_account_settings.rs:136, so neither exit of the handler can build a record.
5. Grep the crate: `crate::state::AUDIT_ACCOUNT_SETTINGS_RESOURCE` has exactly one reader, `src/ops/update_account_settings.rs`, although the constant's own doc names it as "the account's own settings" rather than as a write-only marker.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# Security policy and security configuration calls emit no audit record, breaking the audit discipline every other policy-plane handler in this crate states

_Filed: 2026-10-28T20:32:17Z_
_Operation: CreateSecurityPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has an audit facility in its own state module (`crate::state::audit`, `AuditOutcome`, `audit_access_policy_resource`, `audit_lifecycle_policy_resource`, `AUDIT_ACCOUNT_SETTINGS_RESOURCE`) and every data-access-policy and lifecycle-policy handler states the discipline in its own body: "Every exit below leaves exactly one audit record naming this `(type, name)` pair, so a state-changing call is observable whatever it answered." `GetPoliciesStats` and `UpdateAccountSettings` follow it too.

The security-policy and security-configuration handlers — the OTHER half of the same policy plane, the one `GetPoliciesStats` counts as "security policies and configurations" — emit nothing at all. All ten of them open their handler body by DISCARDING the request context (`let _ = ctx;`), which is the very value `audit` reads the caller identity and request target from, so not one of these state changes is observable in the audit log:

* CreateSecurityPolicy, UpdateSecurityPolicy, DeleteSecurityPolicy (writes), GetSecurityPolicy, ListSecurityPolicies (reads)
* CreateSecurityConfig, UpdateSecurityConfig, DeleteSecurityConfig (writes), GetSecurityConfig, ListSecurityConfigs (reads)

So creating or deleting an `encryption`/`network` security policy — the policy `CreateCollection` reads to decide how a collection is encrypted — leaves no record, while creating or deleting a `data` access policy beside it leaves one. The same split hits the SAML/IAM-Identity-Center configurations. The audit outcomes the enum already models (`PolicyCreated`, `PolicyUpdated`, `PolicyDeleted`, `VersionMismatch`, `IdempotentReplay`, `PolicyRead`, `AlreadyExists`, `QuotaExceeded`, `NotFound`, `StoreUnreachable`) are exactly the exits these handlers take, so nothing about them is special — they were simply left out. No response body or status changes either way.

## Evidence

The discipline, stated in the sibling handlers of the same plane — src/ops/create_access_policy.rs:160-162: "// Every exit below leaves exactly one audit record naming this `(type, name)` pair,\n        // so a state-changing call is observable whatever it answered.\n        let resource = crate::state::audit_access_policy_resource(&input.r#type, &input.name);" — identically at src/ops/update_access_policy.rs:162-164, src/ops/delete_access_policy.rs:147-149, src/ops/create_lifecycle_policy.rs:161-163, src/ops/update_lifecycle_policy.rs:168-170, src/ops/delete_lifecycle_policy.rs:149-151, and src/ops/update_account_settings.rs:147-151 ("Every exit leaves exactly one record, so this state-changing call is observable\n        // whatever it answered.").

The facility itself — src/state.rs:2964-2967: "/// One structured audit record for a call: which operation, the identity the request\n/// presented, what it was addressed to, the resource it resolved and the outcome. The\n/// `tracing` subscriber supplies the timestamp." and src/state.rs:2976-2982: "pub fn audit(\n    operation: &str,\n    ctx: &smithy_server_runtime::OperationContext<'_>,\n    outcome: AuditOutcome,\n    resource: Option<&str>,\n) {\n    let parts = ctx.parts();" — the record is built from `ctx`.

The handlers that discard `ctx` instead, one line into the body: src/ops/create_security_policy.rs:160 "let _ = ctx;", src/ops/update_security_policy.rs:166 "let _ = ctx;", src/ops/delete_security_policy.rs:143 "let _ = ctx;", src/ops/get_security_policy.rs:144 "let _ = ctx;", src/ops/list_security_policies.rs:151 "let _ = ctx;", src/ops/create_security_config.rs:161 "let _ = ctx;", src/ops/update_security_config.rs:162 "let _ = ctx;", src/ops/delete_security_config.rs:143 "let _ = ctx;", src/ops/get_security_config.rs:141 "let _ = ctx;", src/ops/list_security_configs.rs:148 "let _ = ctx;". A repo-wide grep for `crate::state::audit` matches only create/update/delete/get_access_policy, create/update/delete_lifecycle_policy, batch_get_lifecycle_policy, get_policies_stats and update_account_settings — never a security-policy or security-config handler.

That these are the same plane, by this crate's own accounting — src/ops/get_policies_stats.rs:170-172: "counter(store.security_policy_count(crate::state::SECURITY_POLICY_ENCRYPTION)),\n                counter(store.security_policy_count(crate::state::SECURITY_POLICY_NETWORK)),\n                counter(store.security_config_count(crate::state::SECURITY_CONFIG_SAML))," — counted beside the access and lifecycle policies, under a total the handler documents at src/ops/get_policies_stats.rs:177-179 as "the total number of OpenSearch Serverless security policies and configurations in your account". `GetPoliciesStats` itself audits (src/ops/get_policies_stats.rs:186) while the writes that change the numbers it reports do not.

Test coverage: no test in tests/all/ asserts anything about audit records (the audit facility is `tracing`-only), so adding the missing calls leaves every response and every existing assertion unchanged.

## Steps to Reproduce

1. Run the emulator with the `emulator::audit` tracing target enabled.
2. CreateAccessPolicy(type="data", name="readers", policy='[]') -> 200, and observe one audit line: operation=CreateAccessPolicy resource=access-policy/data/readers outcome=policy-created (src/ops/create_access_policy.rs:322).
3. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200, and observe NO audit line at all (src/ops/create_security_policy.rs:160 discards `ctx`).
4. DeleteSecurityPolicy(type="encryption", name="enc") -> 200: again no audit record, while DeleteAccessPolicy(type="data", name="readers") logs outcome=policy-deleted (src/ops/delete_access_policy.rs:204).
5. Repeat with CreateSecurityConfig/UpdateSecurityConfig/DeleteSecurityConfig and with GetSecurityPolicy/GetSecurityConfig: none of the ten security-policy or security-config operations emits a record, although GetAccessPolicy and BatchGetLifecyclePolicy log outcome=policy-read for the equivalent read.

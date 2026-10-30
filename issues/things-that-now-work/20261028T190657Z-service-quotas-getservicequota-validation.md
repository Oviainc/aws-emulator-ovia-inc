Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 3da2fe3b5486a725ab91e82448a29b4c21c390bb

# GetServiceQuota guards only half of the ContextId/plane rule its sibling RequestServiceQuotaIncrease guards in full

_Filed: 2026-10-28T19:06:57Z_
_Operation: GetServiceQuota | Class: validation_

> _Note: this issue was identified against the code base at commit `3da2fe3b5486a725ab91e82448a29b4c21c390bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DefaultQuota` models the two halves of one rule through two predicates: `resource_adjustable()` (the quota has a per-resource plane) and `account_adjustable()` (the quota has an account-wide plane). `RequestServiceQuotaIncrease` applies BOTH halves — a `ContextId` on a quota with no resource plane is refused, and NO `ContextId` on a quota with no account plane is refused — and its own comment states that the second arm exists deliberately to guard the RULE rather than whichever catalog rows happen to be seeded.

`GetServiceQuota`, which reads the same two planes through the same `DefaultQuota`, applies only the first half. `account_adjustable()` is never called outside `request_service_quota_increase.rs`. So for a catalog row published as `PER_RESOURCE` (a quota whose value can only ever be applied to an individual resource, and for which `AdjustableAtLevelEnum` declares a member), `GetServiceQuota` with no `ContextId` answers HTTP 200 with a record labelled `QuotaAppliedAtLevel: ACCOUNT` and a `QuotaContext` whose `ContextScope` is `ACCOUNT` — an account-wide plane the crate itself says such a quota does not have, and the exact mirror of the refusal the handler already makes in the other direction.

The two handlers cannot both be right about whether the account plane of a `PER_RESOURCE` quota is readable: the write path refuses to land on it, the read path reports it. Today the seeded catalog holds no `PER_RESOURCE` row (only one `LEVEL_ALL` row, `state.rs:4122`), so the divergence is latent — but that is precisely the condition the sibling's comment says the guard must not depend on.

## Evidence

`DefaultQuota` declares both halves — services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/state.rs:413-424:

    /// Whether this quota's value can be applied to an individual resource: its
    /// `AdjustableAtLevel` is `PER_RESOURCE` or `ALL` ...
    pub fn resource_adjustable(&self) -> bool {
        self.adjustable_at_level == LEVEL_PER_RESOURCE || self.adjustable_at_level == LEVEL_ALL
    }

    /// Whether this quota's value can be applied account-wide: its
    /// `AdjustableAtLevel` is `ACCOUNT` or `ALL`.
    pub fn account_adjustable(&self) -> bool {
        self.adjustable_at_level == LEVEL_ACCOUNT || self.adjustable_at_level == LEVEL_ALL
    }

The SIBLING applies both, and says why the second arm is there — src/ops/request_service_quota_increase.rs:193-212:

            // The other half of the same rule: a quota with no account-wide plane can
            // only be increased for one resource, so a request that names none has no
            // plane to land on. It holds for any row the catalog publishes as
            // `PER_RESOURCE`; today every row it publishes is either account-only or
            // adjustable at both levels, so this arm guards the rule rather than a
            // seeded quota, and keeps the refusal from depending on which rows exist.
            None if !account_adjustable => {
                return Err(
                    RequestServiceQuotaIncreaseErrorModeled::IllegalArgumentException(
                        IllegalArgumentException {
                            message: Some(format!(
                                "QuotaCode '{}' is adjustable for an individual resource only, so \
                                 a ContextId is required",
                                input.quota_code
                            )),

THIS handler applies only the first half — src/ops/get_service_quota.rs:127-142:

        let resource_adjustable = default.resource_adjustable();

        if let Some(context_id) = input.context_id.as_deref() {
            if !resource_adjustable {
                return Err(GetServiceQuotaErrorModeled::IllegalArgumentException(
                    IllegalArgumentException {
                        message: Some(format!(
                            "ContextId '{context_id}' was provided for QuotaCode '{}', which is \
                             adjustable at the ACCOUNT level only and has no quota context",
                            input.quota_code
                        )),

...and there is no `None`/`account_adjustable` arm: `grep -n "account_adjustable" src/` matches only state.rs:422 (the definition) and request_service_quota_increase.rs:176,199 (the one caller).

The record it would then build labels the plane ACCOUNT unconditionally — src/state.rs:922-931:

        // The record answers for the plane it was read on: a value read for a
        // resource is applied at the RESOURCE level, the account-wide one at ACCOUNT.
        quota.quota_applied_at_level = Some(
            if context.is_some() {
                LEVEL_RESOURCE
            } else {
                LEVEL_ACCOUNT
            }

Model wording for the level the two predicates come from (`smithy_query query="shape" shape="QuotaContextInfo"`, member `AdjustableAtLevel`): "`PER_RESOURCE` – You can request an increase only for an individual resource."

Test coverage: tests/all/resource_quota_context.rs exercises only the one `LEVEL_ALL` row (`es` / "Instances per domain", src/state.rs:4122) and tests/all/quota_workflow.rs:1093-1111 pins the ContextId-on-account-only refusal; no test exercises a `PER_RESOURCE`-only row through either handler, and no comment in get_service_quota.rs states a reason for omitting the second half.

## Steps to Reproduce

1. Add (or observe, once the catalog carries one) a catalog row published as adjustable per resource ONLY, e.g. `quota("Instances per domain", 80.0).adjustable_at(LEVEL_PER_RESOURCE, "AWS::Elasticsearch::Domain")` in `es_quotas()` (src/state.rs:4113-4124).
2. Boot the emulator and POST `/` with `X-Amz-Target: ServiceQuotasV20190624.RequestServiceQuotaIncrease` and body `{"ServiceCode":"es","QuotaCode":"<that code>","DesiredValue":100}` (no ContextId) — refused `IllegalArgumentException`: "... is adjustable for an individual resource only, so a ContextId is required".
3. POST `/` with `X-Amz-Target: ServiceQuotasV20190624.GetServiceQuota` and body `{"ServiceCode":"es","QuotaCode":"<that code>"}` (no ContextId) — HTTP 200 with `Quota.QuotaAppliedAtLevel = "ACCOUNT"` and `Quota.QuotaContext.ContextScope = "ACCOUNT"`.
4. Compare: the write path says the quota has no account plane, the read path reports a record on it.

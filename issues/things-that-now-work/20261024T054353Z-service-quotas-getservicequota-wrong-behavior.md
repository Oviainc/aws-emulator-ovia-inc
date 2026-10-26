Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7

# An approved increase on a GlobalQuota is invisible to readers in other regions until someone calls in the requesting region

_Filed: 2026-10-24T05:43:53Z_
_Operation: GetServiceQuota | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a72a5132d94bd5cd1bb063d76c12b2ff9f8a44a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A quota increase is decided by the clock (`decided_at = now + REQUEST_DECISION_DELAY`) and materialised lazily by `settle_due_requests`. That settlement is scoped to the CALLER's region only, so a `GlobalQuota` increase requested in region A and already past its decision instant is not put in force when the first read comes from region B.

Expected: a `GlobalQuota` is counted account-wide (its ARN deliberately carries no region), so once the decision is due its applied value is in force at every regional endpoint — which is what this crate's own state module documents (`global_applied_quotas`: "an approved increase on it is in force at every regional endpoint, not just the one it was requested at") and what AWS's account-wide quota semantics require.

Actual: `src/ops/get_service_quota.rs` line 108 calls `quotas_state.settle_due_requests(&region, now)` for the caller's region; `src/state.rs` lines 725-739 return early for any other region's request store, so the pending request in region A is never decided and `global_applied_quotas` is never written. `GetServiceQuota` in region B keeps reporting the AWS default value indefinitely — and then starts reporting the increased value as soon as any call is made in region A, so the same read answers two different values depending on traffic to an unrelated endpoint. `ListServiceQuotas`, `GetQuotaUtilizationReport` and `RequestServiceQuotaIncrease` (whose "is this an increase?" check reads `applied_value`) inherit the same stale view.

## Evidence

Spec/semantics: `ServiceQuota.GlobalQuota` — "Indicates whether the quota is global" (Smithy) — and AWS's account-wide quota semantics: one value for the whole account. The crate states the same invariant in `src/state.rs` lines 487-491 and encodes it in `settle_due_requests` lines 771-780, which writes `global_applied_quotas` for a global quota.

Handler/state: settlement is per-region only (`src/state.rs` lines 725-739: `let Some(state) = self.regions.get(region) else { return; }` and the `request_order` walk of that one region); every handler passes `crate::state::request_region(&ctx.parts().headers)`.

Test coverage: `tests/all/quota_workflow.rs::an_approved_increase_on_a_global_quota_is_in_force_in_every_region` (lines 930-991) reads in the REQUESTING region first (lines 954-961), which settles the request, before reading elsewhere — so the test passes and the gap is untested. Reordering it (read only from `ap-southeast-2`) exposes the stale value.

## Steps to Reproduce

1. Boot the emulator. With SigV4 credential scope region `us-east-1`, POST `RequestServiceQuotaIncrease` `{"ServiceCode":"s3","QuotaCode":"L-DC2B2D3D","DesiredValue":20000}` (a seeded GlobalQuota) and observe `Status: PENDING`.
2. Wait past `state::REQUEST_DECISION_DELAY` (500 ms). Make NO further call against us-east-1.
3. With credential scope region `ap-southeast-2`, POST `GetServiceQuota` `{"ServiceCode":"s3","QuotaCode":"L-DC2B2D3D"}` -> `Quota.Value` is still the 10000 default (expected: 20000, the account-wide value in force).
4. Now POST `GetServiceQuota` once against us-east-1, then repeat the ap-southeast-2 read -> it now reports 20000, i.e. the answer changed because of a call to another endpoint.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# UpgradeAccountPlan to FREE reactivates an expired free plan and reports a status the next read contradicts

_Filed: 2026-10-24T00:03:52Z_
_Operation: UpgradeAccountPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the stored plan is a FREE plan whose `accountPlanExpirationDate` has already passed, `UpgradeAccountPlan` with `accountPlanType=FREE` is accepted: the handler settles the plan to EXPIRED, then unconditionally sets `plan_status = ACTIVE` and (because the target is not PAID) leaves the already-past `expiration_date` in place. The 200 response therefore claims `accountPlanStatus: ACTIVE` for a plan whose expiry is in the past, and the very next `GetAccountPlanState` re-settles the same record and reports `accountPlanStatus: EXPIRED` with `accountPlanExpirationDate` in the past. Two responses describe the same stored plan at the same instant and disagree, so one of them is necessarily wrong.

Expected: the free plan is a one-off, time-boxed window (the same rule the handler already enforces for PAID -> FREE, which it refuses with ValidationException). An expired free plan must not be moved back to ACTIVE by `UpgradeAccountPlan(FREE)` — the call should either be refused with the modelled `ValidationException` or leave the plan EXPIRED, and whatever status it returns must be the status the following `GetAccountPlanState` reports.

Actual: HTTP 200 with `accountPlanType: FREE`, `accountPlanStatus: ACTIVE`, immediately followed by a read reporting `EXPIRED`.

## Evidence

Spec/model: `UpgradeAccountPlanResponse$accountPlanStatus` is @required and is the status of the plan the account now holds; `AccountPlanStatus` declares EXPIRED, and `GetAccountPlanStateResponse$accountPlanExpirationDate` is documented as "the timestamp for when the current account plan expires", so a plan past that instant is EXPIRED (the emulator itself encodes this in `AccountPlan::settle`). Handler: services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/upgrade_account_plan.rs lines 183-209 — `plan.settle(now)` then `plan.upgrade_to(target)`; state.rs `upgrade_to` (lines 141-147) sets `plan_status = PLAN_STATUS_ACTIVE` for any target and keeps `expiration_date` when the target is FREE. The one-way rule is already asserted for PAID -> FREE (same handler, lines 195-201). Tests: tests/all/free_tier.rs `upgrade_account_plan_persists_the_target_plan` only exercises fresh accounts, and `a_free_plan_past_its_expiration_instant_reports_expired` only reads the aged plan — no test combines an aged/expired free plan with an `UpgradeAccountPlan(FREE)` call, so this path is uncovered.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. Call UpgradeAccountPlan with {"accountPlanType": "FREE"} to establish the free plan.
3. Age the stored plan past its expiration instant (as tests/all/free_tier.rs does by setting `expiration_date` to a past instant), or let the free window elapse.
4. Call GetAccountPlanState — observe accountPlanStatus: EXPIRED.
5. Call UpgradeAccountPlan with {"accountPlanType": "FREE"} again — observe HTTP 200 with accountPlanStatus: ACTIVE.
6. Call GetAccountPlanState again — observe accountPlanStatus: EXPIRED with accountPlanExpirationDate still in the past, contradicting the upgrade response.

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# A bundled pricing plan reports no pricingTier and a plan change is never reported as pending

_Filed: 2026-10-24T13:56:40Z_
_Operation: UpdatePricingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real IoT TwinMaker): the pricing plan is a billing object with a tier and an effective date.
1. For a bundled mode (`pricingMode` other than BASIC, e.g. TIERED_BUNDLE) the reported `currentPricingPlan.bundleInformation` carries a `pricingTier` — the tier the account's billable entity count places it in (TIER_1 for the smallest band) — alongside `bundleNames`.
2. A change that does not take effect immediately (notably leaving a bundled mode for pay-as-you-go, which AWS applies at the start of the next billing cycle) is reported as `pendingPricingPlan` with its own later `effectiveDateTime`, while `currentPricingPlan` keeps reporting the plan still in force; a subsequent UpdatePricingPlan supersedes that pending plan, which is what the modeled `UpdateReason` value `OVERWRITTEN` describes.

Actual (emulator): `crate::state::set_pricing_plan()` applies every change AT ONCE (new `effectiveDateTime` == the instant of the call), and `PricingPlanRecord::pricing_tier` is never assigned by any writer, so `bundleInformation` is always serialized without `pricingTier`. `state.pending_pricing_plan` is only ever `take()`n (cleared) — nothing ever stores a pending plan, so both UpdatePricingPlan and GetPricingPlan always report `pendingPricingPlan` absent, the `OVERWRITTEN` reason is unreachable in practice (there is never a pending plan to supersede), and the `ENTITY_COUNT_UPDATE` reason — the tier move the service makes on its own when the billable entity count crosses a threshold — is never produced either. A client reading `bundleInformation.pricingTier` to show the account's tier, or polling `pendingPricingPlan` to learn when a downgrade lands, gets nothing it can act on.

## Evidence

Spec: the model defines `BundleInformation.pricingTier` (enum `PricingTier`: TIER_1..TIER_4), `PricingPlan.effectiveDateTime`, `UpdatePricingPlanResponse.pendingPricingPlan` / `GetPricingPlanResponse.pendingPricingPlan`, and `UpdateReason` values OVERWRITTEN and ENTITY_COUNT_UPDATE. A `pendingPricingPlan` member plus an `OVERWRITTEN` update reason only have meaning if an accepted change can be pending and later superseded, and `pricingTier` only has meaning if a bundled plan is reported with its tier — which is also how AWS publishes IoT TwinMaker tiered-bundle pricing (the tier follows the billable entity count, and a switch away from a bundle takes effect at the start of the next billing cycle).

Handler/state: src/ops/update_pricing_plan.rs calls `state.set_pricing_plan(mode, bundles, now)` then reports `pending_pricing_plan` straight from the store; src/state.rs `set_pricing_plan()` documents "The change takes effect AT ONCE", does `self.pending_pricing_plan.take()`, and documents "`pricingTier` is left unset"; `PricingPlanRecord::as_pricing_plan()` builds `BundleInformation { bundle_names, pricing_tier: self.pricing_tier.clone() }` from that permanently-`None` field.

Tests: tests/all/pricing_plan.rs:143-148 asserts the reported `pricingTier` equals whatever the STORE holds (always None), and tests/all/pricing_plan.rs:204 / pricing_plan_update.rs:201-221,371-385 assert `pendingPricingPlan` is null — i.e. the suite pins the emulator's choice rather than covering the AWS behavior; no test exercises a tier or a deferred change.

## Steps to Reproduce

1. GetPricingPlan() on a fresh account -> currentPricingPlan pricingMode BASIC, updateReason DEFAULT (correct today).
2. UpdatePricingPlan(pricingMode="TIERED_BUNDLE", bundleNames=["TWINMAKER"]).
3. Expected (AWS): currentPricingPlan.bundleInformation = {bundleNames: ["TWINMAKER"], pricingTier: "TIER_1"}. Actual: bundleInformation carries bundleNames only — no pricingTier member at all.
4. UpdatePricingPlan(pricingMode="BASIC") to leave the bundle.
5. Expected (AWS): the downgrade does not take effect immediately — currentPricingPlan still reports TIERED_BUNDLE and pendingPricingPlan reports BASIC with a future effectiveDateTime; a following GetPricingPlan reports the same pair until that instant passes. Actual: currentPricingPlan is BASIC effective now and pendingPricingPlan is absent in both the update response and GetPricingPlan.
6. Issue a third UpdatePricingPlan while a change should still be pending. Expected (AWS): updateReason OVERWRITTEN on the superseding plan. Actual: OVERWRITTEN is unreachable because nothing was pending.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# GetRenewalPricing always answers UNABLE_TO_PRICE with an empty PricingOptions list for every Outpost

_Filed: 2026-10-24T07:16:24Z_
_Operation: GetRenewalPricing | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRenewalPricing` reports `OutpostRecord::renewal_pricing`, and nothing in the service ever writes that vector: `CreateOutpost` initialises it to `Vec::new()` and no other handler touches it (the only writers in the repo are the tests, which poke the store directly). So over the wire the operation is a canned response — for every Outpost that exists it answers HTTP 200 with `PricingResult: "UNABLE_TO_PRICE"` and `PricingOptions: []`, no matter what the Outpost is or what was ordered for it.

Expected: an Outpost with a subscription/contract behind it is priced — `PricingResult: "PRICED"` and one or more `PricingOption` entries (payment option, payment term, and the monthly/upfront `PricingDetails`) up to the modelled maximum of 9 — and `UNABLE_TO_PRICE` only for an Outpost that genuinely cannot be priced. Actual: the `PRICED` branch of the handler is dead code from an API client's point of view, so a renewal-pricing workflow can never see a priced option.

## Evidence

Spec: `GetRenewalPricing` is modelled with output `PricingResult` (enum `PRICED | UNABLE_TO_PRICE`) and `PricingOptions` (list, length 0..9) and is documented "Gets all available renewal pricing options for the specified Outpost" — an operation whose whole purpose is to return pricing options.

Handler/state: `src/ops/get_renewal_pricing.rs` lines ~171-201 derives the result solely from `record.renewal_pricing.is_empty()`; `src/state.rs` line ~228 declares the field and `src/ops/create_outpost.rs` line ~287 is the only initialisation (`Vec::new()`). `grep renewal_pricing` across `src/` finds no other write.

Tests: `tests/all/renewal_pricing.rs::renewal_pricing_reports_every_option_recorded_for_the_outpost` has to seed `record.renewal_pricing` directly through the store (line ~208) because no request can produce an option — confirming no HTTP path ever yields `PRICED`.

## Steps to Reproduce

1. CreateSite, then CreateOutpost against it; keep the OutpostId.
2. GET /outpost/{OutpostId}/renewal-pricing — observe HTTP 200 with {"PricingResult":"UNABLE_TO_PRICE","PricingOptions":[]}.
3. Repeat for any other Outpost, in any region, with any tags or hardware type — the response is identical, so the field is not computed from anything about the Outpost.

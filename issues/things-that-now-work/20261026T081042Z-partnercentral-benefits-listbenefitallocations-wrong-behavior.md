Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 23e63f00b11bb74d67049d5cb37769202febdd46

# BenefitAllocationSummary$FulfillmentTypes reports (and the FulfillmentTypes filter selects on) the types the BENEFIT offers, not the ones the allocation is fulfilled with

_Filed: 2026-10-26T08:10:42Z_
_Operation: ListBenefitAllocations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `23e63f00b11bb74d67049d5cb37769202febdd46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBenefitAllocations` fills the summary member `FulfillmentTypes` from the stored field `BenefitAllocation::fulfillment_types`, which this crate's own state module documents as "the ones the benefit this allocation is based on offers" — NOT the fulfillment the allocation actually used, which the store keeps in a separate field `fulfillment_type` ("The one `FulfillmentType` this allocation is fulfilled with") and which `GetBenefitAllocation` reports. The same wrong field drives the request filter: `matches_any(input.fulfillment_types, &allocation.fulfillment_types)`.

Consequence, visible in the default store: the seeded allocation "Migration Acceleration Funding allocation" is based on a benefit offering CASH and CREDITS but is fulfilled with CREDITS. `ListBenefitAllocations` reports `FulfillmentTypes: ["CASH","CREDITS"]` for it, while `GetBenefitAllocation` on the very same id reports `FulfillmentType: "CREDITS"` and a `FulfillmentDetail` of the CREDITS shape — the two reads of one record contradict each other. A caller filtering `FulfillmentTypes: ["CASH"]` gets that allocation back even though nothing about it was ever fulfilled as CASH.

The fix is to report and filter on the allocation's own fulfillment (the stored `fulfillment_type`), which is the fact both the model member and the sibling read describe.

## Evidence

Model (smithy_query doc, BenefitAllocationSummary$FulfillmentTypes): "The fulfillment types used for this benefit allocation." And ListBenefitAllocationsInput$FulfillmentTypes: "Filter benefit allocations by specific fulfillment types."

The crate's own distinction — src/state.rs:876-881:
```
    /// `FulfillmentType` values: the ones the benefit this allocation is based on offers.
    pub fulfillment_types: Vec<String>,
    /// The one `FulfillmentType` this allocation is fulfilled with, which is always one of
    /// [`Self::fulfillment_types`]: a benefit may offer two, and the allocation is fulfilled
    /// with one of them.
    pub fulfillment_type: String,
```

What the list reports — src/ops/list_benefit_allocations.rs:320-322:
```
                        fulfillment_types: crate::state::optional_list(
                            &allocation.fulfillment_types,
                        ),
```
and what it filters on — src/ops/list_benefit_allocations.rs:278-281:
```
                crate::state::matches_any(
                    input.fulfillment_types.as_ref(),
                    &allocation.fulfillment_types,
                )
```

The sibling read of the same record — src/ops/get_benefit_allocation.rs:307:
```
            fulfillment_type: Some(allocation.fulfillment_type.clone()),
```

A record in the default store where the two disagree — src/state.rs:616-630:
```
    SeededAllocation {
        benefit_name: "Migration Acceleration Funding",
        name: "Migration Acceleration Funding allocation",
        status: ALLOCATION_STATUS_ACTIVE,
        status_reason: None,
        // The benefit offers `CASH` and `CREDITS`; this allocation is fulfilled with the
        // second of them, so the type is the allocation's own choice rather than the
        // benefit's first.
        fulfillment_type: FULFILLMENT_TYPE_CREDITS,
```
with the seeding that copies the benefit's whole offer list onto the allocation — src/state.rs:1238: `fulfillment_types: benefit.fulfillment_types.clone(),`.

Test coverage today only mirrors the store field rather than the modelled meaning: tests/all/benefit_allocations.rs:328-332 asserts `members(summary, "FulfillmentTypes") == allocation.fulfillment_types` ("expected the stored FulfillmentTypes"), and tests/all/benefit_allocations.rs:442-451 / 530-539 build the filter expectation from the same field; tests/all/get_benefit_allocation.rs:374-377 only checks that the single reported `FulfillmentType` is CONTAINED in that list, so the superset is never challenged.

## Steps to Reproduce

1. Grant the emulator account the partner entitlement (crate::state::grant_partner_entitlement, as the test harness does) so the entitlement gate admits requests.
2. Call ListBenefitAllocations with {"Catalog": "AWS"} and find the summary named "Migration Acceleration Funding allocation": its FulfillmentTypes is ["CASH","CREDITS"].
3. Call GetBenefitAllocation with that summary's Id: FulfillmentType is "CREDITS" and FulfillmentDetail is the CreditDetails member — the allocation was never fulfilled as CASH.
4. Call ListBenefitAllocations with {"Catalog": "AWS", "FulfillmentTypes": ["CASH"]}: the same allocation is still returned, so the filter selects an allocation whose own fulfillment type is CREDITS.

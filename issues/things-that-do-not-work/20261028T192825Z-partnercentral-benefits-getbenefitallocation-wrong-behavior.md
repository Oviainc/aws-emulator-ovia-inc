Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# GetBenefitAllocation reports the raw stored fulfillment type, skipping the guard its sibling report and the store's own invariant rely on

_Filed: 2026-10-28T19:28:25Z_
_Operation: GetBenefitAllocation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An allocation's fulfillment type is stored as a plain `String` on `BenefitAllocation`, and the crate states twice that an allocation must never REPORT an empty one, because the empty string is not a value of the modelled `FulfillmentType` enum (`ACCESS | CASH | CREDITS`).

* The store has one guard for exactly that case, `BenefitAllocation::fulfillment_types()`, whose doc says an allocation with nothing recorded "reports no fulfillment type rather than an unmodelled empty one".
* `ListBenefitAllocations` reports the member THROUGH that guard, and its comment says the value it reports is the same fact `GetBenefitAllocation` reports singly as `FulfillmentType`.
* `GetBenefitAllocation` does NOT go through the guard: it clones the stored string and wraps it in `Some(..)` unconditionally, so a record whose `fulfillment_type` is empty is reported as `"FulfillmentType": ""` — a value the modelled enum does not declare — while `ListBenefitAllocations` reports the member as absent for the very same record. The two reports of one record disagree.

The empty value is not hypothetical to the store: `grant_allocation` writes `fulfillment_type: fulfillment_type.unwrap_or_default()`, i.e. the empty string, for a benefit that offers no fulfillment type — which also contradicts the field's own doc ("The one `FulfillmentType` this allocation is fulfilled with, which is always one of the types its benefit offers"). Only the in-process grant path can produce such a record today (every seeded benefit offers at least one type), so this is a latent inconsistency rather than a wire-reachable one; the defect is that the read path omits the normalisation the crate wrote for it and that the grant path can write a value the field doc forbids.

## Evidence

The guard, with the rule it exists to enforce — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/state.rs:950-958:
    /// Empty for an allocation no fulfillment type was recorded for, so such an allocation
    /// reports no fulfillment type rather than an unmodelled empty one.
    pub fn fulfillment_types(&self) -> &[String] {
        if self.fulfillment_type.is_empty() {
            &[]
        } else {
            std::slice::from_ref(&self.fulfillment_type)
        }
    }

The sibling report, which goes through it and says it reports the same fact as this operation — src/ops/list_benefit_allocations.rs:303-308:
                        // The types USED for this allocation, which is what the summary
                        // member reports and what `GetBenefitAllocation` reports singly as
                        // `FulfillmentType` — never its benefit's offer list.
                        fulfillment_types: crate::state::optional_list(
                            allocation.fulfillment_types(),
                        ),

The read that skips it — src/ops/get_benefit_allocation.rs:307:
            fulfillment_type: Some(allocation.fulfillment_type.clone()),

The field doc the store contradicts — src/state.rs:923-928:
    /// The one `FulfillmentType` this allocation is fulfilled with, which is always one of
    /// the types its benefit offers: a benefit may offer two, and the allocation is
    /// fulfilled with one of them. ...
    pub fulfillment_type: String,

and the write that can leave it empty — src/state.rs:1709 and :1730:
        let fulfillment_type = benefit.fulfillment_types.first().cloned();
...
            fulfillment_type: fulfillment_type.unwrap_or_default(),

The model, for why an empty string is not reportable there: `smithy_query(query="shape",shape="GetBenefitAllocationOutput")` types `FulfillmentType` as the enum `FulfillmentType`, whose values are `ACCESS | CASH | CREDITS` (OPERATIONS.md:74 — "`FulfillmentType` (FulfillmentType, one of: ACCESS | CASH | CREDITS)").

Test coverage: tests/all/get_benefit_allocation.rs:370-378 asserts of the reported member both that `FULFILLMENT_TYPE_VALUES.contains(&fulfillment_type.as_str())` ("FulfillmentType {fulfillment_type:?} is not a modelled FulfillmentType") and that it equals `allocation.fulfillment_types()` as a one-element slice — the two properties the handler cannot guarantee for a record with no recorded type; tests/all/allocation_fulfillment_types.rs:103-117 pins the same equality across the summary and the single report. No test exercises an allocation with no fulfillment type, so neither assertion fires today.

## Steps to Reproduce

1. Boot the emulator with the in-memory engine and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as tests/all/benefit_allocations.rs:85-98 does for the out-of-API grant path.
2. In the shared store, add a benefit to a seeded catalog whose `fulfillment_types` is empty (the shape `state::Benefit` allows it; no API operation creates a benefit, so the catalog is reached in-process exactly as the grant path is).
3. Call `state::grant_allocation(catalog, <that benefit's id>, None, "name", now)`. src/state.rs:1709/:1730 store the allocation with `fulfillment_type: ""`.
4. Call `GetBenefitAllocation` with that catalog and the returned `benalloc-…` id. Observe `"FulfillmentType": ""` in the 200 body — not a value the modelled `FulfillmentType` enum declares, and the case src/state.rs:950-951 says must report no fulfillment type at all.
5. Call `ListBenefitAllocations` for the same catalog and find the same allocation's summary. Observe that `FulfillmentTypes` is ABSENT there (src/ops/list_benefit_allocations.rs:306-308 goes through `fulfillment_types()`), so the two reports of one record disagree about the fulfillment it was granted with.

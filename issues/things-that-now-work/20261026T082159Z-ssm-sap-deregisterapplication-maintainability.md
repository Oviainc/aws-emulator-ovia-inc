Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# Region-partition reclamation is implemented twice: settle's retain and the never-called forget_region_if_empty

_Filed: 2026-10-26T08:21:59Z_
_Operation: DeregisterApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

"A Region partition that holds nothing is dropped" is written twice in `src/state.rs`: once inside `ComAmazonawsSsmsapSsmsapState::settle`, as a `retain` over every partition, and once as the public `forget_region_if_empty`, which does the same thing for one Region. No handler calls `forget_region_if_empty` — the only other mentions of it in the whole crate are the two doc comments that point at it — so the rule has one live copy and one dead one that can drift from it (for instance if `RegionState::is_empty` grows a field only one of them is meant to consider). Removing the dead copy (or routing the live one through it) changes no response and no test: the reclamation callers observe is the one `settle` performs on every request.

## Evidence

Copy 1 — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2828-2832: "Settling a Region can empty it (the last deregistration it held completed), and a partition that holds nothing is not one this store has to keep ... this is the same reclamation [`Self::forget_region_if_empty`] makes, for every Region at once." `self.regions.retain(|_, region| !region.is_empty());`

Copy 2 — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2865-2873: "Drop `region`'s partition once nothing is left in it, so a Region that held an application and no longer does stops counting against [`MAX_REGIONS`]." `pub fn forget_region_if_empty(&mut self, region: &str) { if self.regions.get(region).is_some_and(RegionState::is_empty) { self.regions.remove(region); } }`

Dead: `grep -rn 'forget_region_if_empty' services/service-com-amazonaws-ssmsap-ssmsap` matches only src/state.rs:303 (doc), src/state.rs:2831 (doc) and src/state.rs:2869 (the definition) — no handler and no test calls it.

Tests that pin the surviving behaviour: tests/all/resource_permissions.rs:859-907 (the `MAX_REGIONS` rule, which registers in `MAX_REGIONS` Regions, deregisters and re-registers) and tests/all/resource_permission_reads.rs:365-395 ("MAX_REGIONS Regions just read at is accepted, which it could not be had the reads" minted partitions). Both exercise the `retain` in `settle`; neither names `forget_region_if_empty`.

## Steps to Reproduce

1. grep -rn 'forget_region_if_empty' services/service-com-amazonaws-ssmsap-ssmsap — observe the only hits are its own definition at src/state.rs:2869 and two doc comments; no caller exists.
2. Read src/state.rs:2824-2833 and src/state.rs:2865-2873 side by side: both implement "drop a partition for which `RegionState::is_empty` holds".
3. Run `cargo test -p service-com-amazonaws-ssmsap-ssmsap` after deleting the unused helper (or after making `settle` call it per Region): the MAX_REGIONS tests in tests/all/resource_permissions.rs and tests/all/resource_permission_reads.rs still pass, confirming only one copy is observable.

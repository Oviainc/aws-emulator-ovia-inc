Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# The Filters-size refusal rule is implemented three times, once per router listing

_Filed: 2026-10-28T16:05:10Z_
_Operation: ListRouterInputs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "at most 10 `Filters` entries, at most 100 values per entry, refused with BadRequestException before the store lock is taken" is written out three times in this crate — in ListRouterInputs, ListRouterOutputs and ListRouterNetworkInterfaces — each with its own copy of the two constants, the two branches and the same refusal wording. The three copies can drift (one ceiling raised, one branch reordered, one refusal reworded) while all three claim in their own doc comments to apply "the same" bound as the others. A single shared checker (fed the per-operation value-list lengths, which is all `filter_values` produces) would state the rule once and leave every response and every existing test byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/list_router_inputs.rs:114-118 and :261-277:
```
const MAX_FILTERS: usize = 10;
const MAX_VALUES_PER_FILTER: usize = 100;
...
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "A list router inputs request may apply at most {MAX_FILTERS} Filters \
                 entries; this one has {}.",
                filters.len()
            )));
        }
        for filter in &filters {
            let values = filter_values(filter);
            if values.len() > MAX_VALUES_PER_FILTER {
```
Copy 2 — src/ops/list_router_outputs.rs:266-282:
```
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "A list router outputs request may apply at most {MAX_FILTERS} Filters \
                 entries; this one has {}.",
...
            if values.len() > MAX_VALUES_PER_FILTER {
```
Copy 3 — src/ops/list_router_network_interfaces.rs:115-118 and :247-258:
```
const MAX_FILTERS: usize = 10;
const MAX_VALUES_PER_FILTER: usize = 100;
...
        if filters.len() > MAX_FILTERS {
            return Err(bad_request(format!(
                "A list router network interfaces request may apply at most {MAX_FILTERS} \
...
            if values.len() > MAX_VALUES_PER_FILTER {
```
The copies even cross-reference each other as one rule — list_router_inputs.rs:112-113: "The router OUTPUT listing bounds its own filters the same way (`list_router_outputs.rs`)."

Tests that pin the behaviour and must stay unchanged: tests/all/router_input_listing.rs:1107-1118, tests/all/router_output_listing.rs:1111-1118, tests/all/router_network_interface_listing.rs:951-958 (each asserting 200 at the ceiling and 400 one past it, for both ceilings).

## Steps to Reproduce

1. grep -n "MAX_VALUES_PER_FILTER" services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/list_router_inputs.rs src/ops/list_router_outputs.rs src/ops/list_router_network_interfaces.rs — three independent definitions and three independent enforcement blocks.
2. Change the ceiling in one file only and observe the three listings disagree while their doc comments still claim one rule.

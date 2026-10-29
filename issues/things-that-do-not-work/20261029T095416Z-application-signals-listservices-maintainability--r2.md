Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d1b341c944550181b7ac630538402367c666ba11

# Route the five inlined page-bound clamps through the crate's defaulted_page_bound helper

_Filed: 2026-10-29T09:54:16Z_
_Operation: ListServices | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d1b341c944550181b7ac630538402367c666ba11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` owns three page-bound rules (`page_bound`, `defaulted_page_bound`, `optional_page_bound`) and documents that every read of this service holds to ONE copy of them. Two handlers do: `ListInstrumentationConfigurations` and `GetInstrumentationConfigurationStatus` call `defaulted_page_bound` for a `MaxResults` that carries both a modelled range and a `@default`.

Five other handlers re-implement the clamp inline — `input.max_results.max(0) as usize` — for a `MaxResults` of exactly the same modelled shape (range plus `@default`): `ListServices`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents` and `ListEntityEvents`. The logic is the same rule written twice, so the copies can drift from the helper (and from each other) the moment the rule changes — e.g. if the floor for a bound outside the modelled range were revisited, five call sites would keep the old reading.

The replacement is behaviour-preserving: each of these members is modelled with `range {min: 1, ...}` (verified via smithy_query), so the framework refuses a non-positive bound before the handler runs and the value reaching the handler is always >= 1, where `max(0) as usize` and `defaulted_page_bound(..., <that operation's documented default>)` return the same number. Responses and existing tests stay unchanged.

## Evidence

The helper and its stated single-home rule:

- `src/state.rs:1468-1474` — `pub fn defaulted_page_bound(max_results: i32, default: i32) -> usize { if max_results < MIN_PAGE_BOUND { default as usize } else { max_results as usize } }`
- `src/state.rs:1486-1490` (its doc) — "[`defaulted_page_bound`] is the same rule for a member whose model carries that default itself (so the handler receives an `i32` already defaulted by the walk) ... All three live here so every read of this service that holds to one of them holds to ONE copy of it, and the page sizes they default to are the constants beside them."

Two handlers hold to it:

- `src/ops/list_instrumentation_configurations.rs:197-200` — `let page_size = crate::state::defaulted_page_bound(input.max_results, crate::state::DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE);`
- `src/ops/get_instrumentation_configuration_status.rs:234-237` — `let page_size = crate::state::defaulted_page_bound(input.max_results, crate::state::DEFAULT_STATUS_EVENT_PAGE);`

Five inline the same clamp instead:

- `src/ops/list_services.rs:218` — `input.max_results.max(0) as usize,`
- `src/ops/list_service_operations.rs:217` — `input.max_results.max(0) as usize,`
- `src/ops/list_service_dependencies.rs:220` — `input.max_results.max(0) as usize,`
- `src/ops/list_service_dependents.rs:218` — `input.max_results.max(0) as usize,`
- `src/ops/list_entity_events.rs:230` — `input.max_results.max(0) as usize,`

That the inline form is equivalent for these members: `smithy_query(query="operation", operation="ListServices")` gives `"MaxResults": {"target": "ListServicesMaxResults", "httpQuery": "MaxResults", "default": 50, "range": {"min": 1, "max": 100}}` — a stated minimum of 1, which the centralized walk enforces before the handler runs (the same reason `src/ops/list_services.rs:204-205` gives: "its `range 1..100` is refused before this handler runs, so the bound is read straight off the input").

Test coverage for the page bounds of these reads: `tests/all/defaulted_page_bound.rs`, `tests/all/page_bounds.rs`, `tests/all/max_results_range.rs` and `tests/all/optional_page_bound.rs`.

## Steps to Reproduce

1. Open `src/state.rs:1486-1490` and read the stated rule that all three page-bound helpers live in one place so every read holds to one copy.
2. Grep the crate for `max_results`: `src/ops/list_services.rs:218`, `src/ops/list_service_operations.rs:217`, `src/ops/list_service_dependencies.rs:220`, `src/ops/list_service_dependents.rs:218` and `src/ops/list_entity_events.rs:230` each re-implement the clamp, while `src/ops/list_instrumentation_configurations.rs:197` and `src/ops/get_instrumentation_configuration_status.rs:234` call the helper for an identically modelled member.
3. Confirm the equivalence with `smithy_query(query="operation", operation="ListServices")`: `MaxResults` declares `range {min: 1, max: 100}` and `default: 50`, so a bound reaching the handler is always >= 1 and the two forms agree.
4. Run `tests/all/page_bounds.rs`, `tests/all/defaulted_page_bound.rs` and `tests/all/max_results_range.rs` before and after routing the five call sites through the helper — responses are byte-identical.

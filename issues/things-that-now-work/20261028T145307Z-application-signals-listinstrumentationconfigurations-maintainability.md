Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: af767dd3eb1ad507e674c736ee4881cbe95a30bf

# The "a non-positive MaxResults can only mean the member was absent" branch is written out twice, once per instrumentation read

_Filed: 2026-10-28T14:53:07Z_
_Operation: ListInstrumentationConfigurations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `af767dd3eb1ad507e674c736ee4881cbe95a30bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both instrumentation reads whose `MaxResults` carries a modelled `@default` plus a `range 1..100` decide their page size with the same four-line branch and the same five-line justification, copied verbatim and differing only in which default constant is named:

* `ListInstrumentationConfigurations` -> `crate::state::DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE`
* `GetInstrumentationConfigurationStatus` -> `crate::state::DEFAULT_STATUS_EVENT_PAGE`

The rule being implemented is one rule — "the boundary validator has already refused anything outside the range and filled the default, so a non-positive value can only mean the member was absent and is read as this operation's documented default" — and this crate already centralizes the OTHER page-bound rule it owns (`crate::state::page_bound`, for a member whose model states a maximum only) in `state.rs`, beside the page-size constants both copies reach for. With two copies, a later change to the reasoning (say, refusing a non-positive bound instead of defaulting it, as `page_bound` does) can be applied to one read and missed on the other, leaving the two instrumentation reads disagreeing about one rule.

The fix is a single helper in `crate::state` taking the member's documented default (`fn defaulted_page_bound(max_results: i32, default: i32) -> usize`), called from both handlers. Every response and every existing assertion stays byte-identical: the helper computes exactly what both branches compute today.

## Evidence

COPY 1 — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_instrumentation_configurations.rs:195-204:
```
        // `MaxResults` is modelled with a `@default` of 50 and `range 1..100`, so the
        // boundary validator has already refused anything outside the range and filled the
        // default for an absent member. A non-positive value can therefore only mean the
        // member was absent, and is read as the documented default rather than as a page of
        // nothing.
        let page_size = if input.max_results < 1 {
            crate::state::DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE as usize
        } else {
            input.max_results as usize
        };
```

COPY 2 — src/ops/get_instrumentation_configuration_status.rs:230-239, the same comment and the same branch:
```
        // `MaxResults` is modelled with a `@default` of 60 and `range 1..100`, so the
        // boundary validator has already refused anything outside the range and filled the
        // default for an absent member. A non-positive value can therefore only mean the
        // member was absent, and is read as the documented default rather than as a page of
        // nothing.
        let page_size = if input.max_results < 1 {
            crate::state::DEFAULT_STATUS_EVENT_PAGE as usize
        } else {
            input.max_results as usize
        };
```

THE CRATE ALREADY CENTRALIZES THE SIBLING PAGE-BOUND RULE — src/state.rs:1362-1391:
```
/// The page size a list read serves for the `MaxResults` a request states, or the refusal
/// for a bound outside the range that member admits: at least [`MIN_PAGE_BOUND`] and at
/// most `maximum`.
///
/// Where a `MaxResults` member's own model declares both bounds, the centralized walk
/// enforces them before any handler runs, and the handler reads the value straight off the
/// input. This is the same rule for a member whose model declares a maximum ONLY ...
pub fn page_bound(member_path: &str, max_results: i32, maximum: i32) -> Result<usize, String> {
```
and both defaults the copies name live in the same module: src/state.rs:3250 `pub const DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE: i32 = 50;` and src/state.rs:3270 `pub const DEFAULT_STATUS_EVENT_PAGE: i32 = 60;`.

MODEL confirming both members carry a `@default` and a `range 1..100` (so the two copies really are one rule) — `smithy_query(query="shape", shape="ListInstrumentationConfigurationsRequest,GetInstrumentationConfigurationStatusRequest")`: `"MaxResults": {"target": "Integer", "range": {"min": 1, "max": 100}, "default": 50}` and `"MaxResults": {"target": "Integer", "range": {"min": 1, "max": 100}, "default": 60}`.

TESTS THAT COVER BOTH COPIES AND MUST STAY UNCHANGED: tests/all/instrumentation_configurations.rs:634-652 walks every page size "plus the absent member that takes its `@default`" (`for max_results in [Some(1), Some(2), Some(stored.len() as i64), Some(100), None]`), and tests/all/instrumentation_configuration_status.rs:909-911 walks "Page sizes spanning one, several, exactly the event count and more than it, plus the modelled `MaxResults` bounds (`range 1..100`)" (`for page_size in [1_i64, 2, 3, 7, 8, 100]`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/list_instrumentation_configurations.rs at lines 195-204 and src/ops/get_instrumentation_configuration_status.rs at lines 230-239, and compare: the comment is word for word identical apart from the default it quotes, and the branch is identical apart from the constant it names.
2. Note that the crate's other page-bound rule lives in one place (src/state.rs:1378 `page_bound`), reached by ListServiceStates, and that both default constants these copies name are already in src/state.rs (lines 3250 and 3270).
3. Replace both copies with one `crate::state` helper taking the member's documented default, then run the suites that cover them: tests/all/instrumentation_configurations.rs (`every_page_size_reports_each_configuration_once`, which includes the absent-MaxResults row) and tests/all/instrumentation_configuration_status.rs (the page-size walk at line 911) — both must pass unchanged, since the helper computes exactly what the two branches compute.

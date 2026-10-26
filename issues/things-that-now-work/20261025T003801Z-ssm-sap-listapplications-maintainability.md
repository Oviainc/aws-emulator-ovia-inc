Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# The filter name/operator validation rule is implemented twice, once per filtered listing

_Filed: 2026-10-25T00:38:01Z_
_Operation: ListApplications | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both filtered listings in this crate validate their `Filters` with the same hand-copied block: `crate::state::filters_within_bounds(&filters)`, then a loop that refuses an operator outside `FILTER_OPERATOR_VALUES` and a name outside the listing's name catalogue, each rendering the same two message templates ("Filter Operator must be one of {}, but was {}" / "Filter Name must be one of {}, but was {}"). The two copies differ only in the name catalogue (`state::FILTER_NAMES` vs `state::CONFIGURATION_CHECK_FILTER_NAMES`) and the per-operation error variant, so they can drift: a change to the loop in one listing (an added check, a reordered precedence between operator and name, a reworded refusal) silently leaves the other listing behaving differently for the same input.

`src/state.rs` already owns every other half of this rule in one place — `filters_within_bounds` for the modelled bounds and `filter_matches_attribute` for what an operator MEANS ("so one rule decides what a `FilterOperator` means whatever is being listed", state.rs:1783-1786) — so the per-handler validation block is the only part of the rule that exists twice. Lifting it into one `state.rs` helper that takes the allowed names and returns `Result<(), ValidationException>` leaves every response byte-identical (same order of checks, same messages) and every existing test unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_applications.rs:159-182`:
```
crate::state::filters_within_bounds(&filters)
    .map_err(ListApplicationsErrorModeled::ValidationException)?;
for filter in &filters {
    if !FILTER_OPERATOR_VALUES.contains(&filter.operator.as_str()) {
        return Err(ListApplicationsErrorModeled::ValidationException(
            crate::state::validation(format!(
                "Filter Operator must be one of {}, but was {}",
                FILTER_OPERATOR_VALUES.join(", "),
                filter.operator
            )),
        )
        .into());
    }
    if !crate::state::FILTER_NAMES.contains(&filter.name.as_str()) {
        return Err(ListApplicationsErrorModeled::ValidationException(
            crate::state::validation(format!(
                "Filter Name must be one of {}, but was {}",
                crate::state::FILTER_NAMES.join(", "),
                filter.name
            )),
        )
        .into());
    }
}
```
Copy 2 — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_configuration_check_operations.rs:190-217`:
```
crate::state::filters_within_bounds(&filters)
    .map_err(ListConfigurationCheckOperationsErrorModeled::ValidationException)?;
for filter in &filters {
    if !FILTER_OPERATOR_VALUES.contains(&filter.operator.as_str()) {
        return Err(
            ListConfigurationCheckOperationsErrorModeled::ValidationException(
                crate::state::validation(format!(
                    "Filter Operator must be one of {}, but was {}",
                    FILTER_OPERATOR_VALUES.join(", "),
                    filter.operator
                )),
            )
            .into(),
        );
    }
    if !crate::state::CONFIGURATION_CHECK_FILTER_NAMES.contains(&filter.name.as_str()) {
        return Err(
            ListConfigurationCheckOperationsErrorModeled::ValidationException(
                crate::state::validation(format!(
                    "Filter Name must be one of {}, but was {}",
                    crate::state::CONFIGURATION_CHECK_FILTER_NAMES.join(", "),
                    filter.name
                )),
            )
            .into(),
        );
    }
}
```
The shared half already lives in one place — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1734` (`pub fn filters_within_bounds(filters: &[Filter]) -> Result<(), ValidationException>`) and `state.rs:1787` (`pub fn filter_matches_attribute(...)`, documented as "This is the comparison every filtered listing of this service shares ... so one rule decides what a `FilterOperator` means whatever is being listed").

Existing tests that pin both copies and must keep passing unchanged: `tests/all/application_lifecycle.rs:1540` (unknown filter name), `:1559` (each operator), `:1571` (operator outside the enum), `:1587-1599` (the modelled bounds) for ListApplications; `tests/all/list_configuration_check_operations.rs:483` (filter names) and `:508` (operators) for ListConfigurationCheckOperations.

## Steps to Reproduce

1. Read services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_applications.rs:159-182.
2. Read services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_configuration_check_operations.rs:190-217.
3. Observe the two blocks are the same rule character-for-character apart from the name catalogue constant and the operation's error variant.
4. Run `cargo test -p service-com-amazonaws-ssmsap-ssmsap` before and after factoring the block into one state.rs helper: every test (including application_lifecycle.rs and list_configuration_check_operations.rs filter cases) must pass unchanged and every response stay identical.

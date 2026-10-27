Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c15dfa091c78a47417078a336c74dcf21189cb0f

# The FilterList validation rule is implemented twice, once in each list handler

_Filed: 2026-10-27T09:08:03Z_
_Operation: ListChangeSets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c15dfa091c78a47417078a336c74dcf21189cb0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "every `Filter` must name an attribute, the name must be one this operation filters on, and its `ValueList` must carry at least one value" is written out twice in this crate — once in `list_change_sets.rs` and once in `list_entities.rs` — as the same three branches in the same order, producing the same two violation paths (`FilterList.Name`, `FilterList.ValueList`) and two byte-identical messages ("every filter names an attribute", "filter {name} carries no value to match"). The only per-operation differences are the allowed-name constant (`CHANGE_SET_FILTER_NAMES` vs `ENTITY_FILTER_NAMES`) and the operation name interpolated into the unsupported-name message; `ListChangeSets` then adds its own time-filter branch on top. Because the rule lives in two places, a later correction to any of the three branches (for instance treating a `ValueList` present-but-empty differently, or reporting the member path another way) can be applied to one list and missed on the other, leaving the two list operations judging the same `Filter` shape differently. The crate already centralises comparable per-operation decisions in `state.rs` (`sorts_descending`, `invalid_sort_by`, `check_catalog`, `decode_page_token`), so one shared helper taking the operation name and the allowed-name slice would leave every response and every existing test byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_change_sets.rs:173-203:
```
        let filters = input.filter_list.unwrap_or_default();
        let mut predicates: Vec<(String, Vec<String>)> = Vec::with_capacity(filters.len());
        for filter in filters {
            let Some(name) = filter.name else {
                return Err(ListChangeSetsErrorModeled::ValidationException(
                    crate::state::validation("FilterList.Name", "every filter names an attribute"),
                )
                .into());
            };
            if !crate::state::CHANGE_SET_FILTER_NAMES.contains(&name.as_str()) { ... }
            let values = filter.value_list.unwrap_or_default();
            if values.is_empty() {
                return Err(ListChangeSetsErrorModeled::ValidationException(
                    crate::state::validation(
                        "FilterList.ValueList",
                        format!("filter {name} carries no value to match"),
```

Copy 2 — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_entities.rs:262-293:
```
        let filters = input.filter_list.unwrap_or_default();
        let mut predicates: Vec<(String, Vec<String>)> = Vec::with_capacity(filters.len());
        for filter in filters {
            let Some(name) = filter.name else {
                return Err(ListEntitiesErrorModeled::ValidationException(
                    crate::state::validation("FilterList.Name", "every filter names an attribute"),
                )
                .into());
            };
            if !crate::state::ENTITY_FILTER_NAMES.contains(&name.as_str()) { ... }
            let values = filter.value_list.unwrap_or_default();
            if values.is_empty() {
                return Err(ListEntitiesErrorModeled::ValidationException(
                    crate::state::validation(
                        "FilterList.ValueList",
                        format!("filter {name} carries no value to match"),
```

Existing tests that must stay unchanged: tests/all/change_set_lifecycle.rs:1379,1418 (ListChangeSets Status filters) and its unsupported-filter rows, tests/all/entity_reads.rs:535-545 (ListEntities EntityId filter rows), tests/all/request_validation.rs (the per-member refusal rules).

## Steps to Reproduce

1. Read src/ops/list_change_sets.rs:173-203 and src/ops/list_entities.rs:262-293 side by side.
2. Observe the identical three-branch structure, the identical violation paths `FilterList.Name` / `FilterList.ValueList`, and the identical literal messages "every filter names an attribute" and "filter {name} carries no value to match".
3. Note the only variation is the allowed-name constant and the operation name in the unsupported-name message, so a change to any branch has to be made twice to keep the two lists in step.

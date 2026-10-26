Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# The "is this SortOrder descending" rule is implemented twice, once as a bare literal and once by comparing against DEFAULT_SORT_ORDER

_Filed: 2026-10-26T05:31:52Z_
_Operation: ListChangeSets | Class: maintainability_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both list operations have to turn a `Sort$SortOrder` value into a sort direction, and each one does it its own way. `ListChangeSets` compares the value against the bare literal `"DESCENDING"`. `ListEntities` instead compares it against `crate::state::DEFAULT_SORT_ORDER`, the constant that exists to hold the DEFAULT order ("The `SortOrder` a list uses when the request names none"), and does so in two places. The two copies can drift, and the second conflates two different facts: "this is the order used when none was given" and "this order means descending". If `DEFAULT_SORT_ORDER` were ever changed, `ListEntities` would silently invert the direction of every page — including for a request that explicitly asked for `DESCENDING` — while `ListChangeSets` kept behaving correctly. One helper (or a `SORT_ORDER_DESCENDING` constant beside the existing `SORT_ORDER_VALUES`) would express the rule once; no response and no existing test changes.

## Evidence

Copy 1 — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_change_sets.rs:318-325`:
```
            if sort_order == "DESCENDING" {
                ordering.reverse()
            } else {
                ordering
            }
```
Copy 2 — `src/ops/list_entities.rs:338-348`:
```
            orders.push(PageOrder {
                by: type_sort_by
                    .unwrap_or(crate::state::ENTITY_SORT_BY[0])
                    .to_string(),
                descending: type_sort_order.unwrap_or(crate::state::DEFAULT_SORT_ORDER)
                    == crate::state::DEFAULT_SORT_ORDER,
            });
        }
        orders.push(PageOrder {
            descending: sort_order == crate::state::DEFAULT_SORT_ORDER,
```
The constant being used as if it meant "descending" is declared as the DEFAULT — `src/state.rs:119-121`:
```
/// The `SortOrder` a list uses when the request names none: `Sort$SortOrder`
/// documents "The default value is `DESCENDING`".
pub const DEFAULT_SORT_ORDER: &str = "DESCENDING";
```
while the enum's values already live in one place — `src/ops/types/shared_types.rs:239`: `pub const SORT_ORDER_VALUES: &[&str] = &["ASCENDING", "DESCENDING"];`

Existing coverage that must stay green and unchanged: `tests/all/change_set_lifecycle.rs:1203` and `:1288` (ListChangeSets rows that set `{"SortBy": sort_by, "SortOrder": order}`), `tests/all/entity_reads.rs:670` (`"Sort": {"SortBy": sort_by, "SortOrder": sort_order}`), and `tests/all/entity_type_projections.rs:470-512` (per-type `*Sort` rows with `ASCENDING`/`DESCENDING`).

## Steps to Reproduce

1. Read `src/ops/list_change_sets.rs:321` — the direction is decided by comparing the order against the string literal "DESCENDING"
2. Read `src/ops/list_entities.rs:342-347` — the same decision is made by comparing the order against `crate::state::DEFAULT_SORT_ORDER`, the constant documented at `src/state.rs:119-121` as the order used when the request names NONE
3. Observe that the one rule now lives in two spellings; changing the default constant would flip ListEntities' explicit `DESCENDING`/`ASCENDING` handling while leaving ListChangeSets correct

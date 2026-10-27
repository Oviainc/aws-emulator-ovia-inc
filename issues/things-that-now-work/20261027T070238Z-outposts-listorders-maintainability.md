Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# The "a resolved OutpostIdentifierFilter narrows to that Outpost, and one of another endpoint matches nothing" rule is written twice, in ListOrders and ListCapacityTasks

_Filed: 2026-10-27T07:02:38Z_
_Operation: ListOrders | Class: maintainability_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two list handlers take an `OutpostIdentifierFilter` modelled as an `OutpostIdentifier`, resolve it once with `state::resolve_outpost_id`, and then each re-states the same selection rule over the resolved value: an absent filter narrows nothing, and a present one matches a record only when the resolved identifier belongs to THIS endpoint (`ResolvedIdentifier::here()`) and equals the record's Outpost id — so a filter naming an Outpost of another partition, Region or account reports an empty list rather than failing. The two copies are three lines of identical structure under near-identical doc comments, and they can drift apart on exactly the two decisions they share (what an absent filter means, and whether an ARN that is not this endpoint's matches nothing or everything).

This crate already keeps such cross-list rules in one place for that reason — `state::filter_accepts` is "Shared by the five list operations that take filters so they cannot drift apart on that rule", and `state::page` is "Shared by both list operations so they cannot drift apart" — and `list_capacity_tasks` itself routes its OTHER filter through `state::filter_accepts` while hand-writing this one. Lifting the resolved-identifier match into one helper beside `filter_accepts` changes no response and no test.

## Evidence

Copy 1 — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/list_orders.rs:227-237:
```
fn matches_outpost(
    record: &crate::state::OrderRecord,
    outpost_filter: Option<&crate::state::ResolvedIdentifier>,
) -> bool {
    match outpost_filter {
        None => true,
        Some(filter) => filter
            .here()
            .is_some_and(|outpost_id| record.order.outpost_id.as_deref() == Some(outpost_id)),
    }
}
```
under the doc at :218-226: "It is a filter and not an identifier the request is addressed to: it narrows the list the caller is reported, so one naming an Outpost that holds no order — including one this endpoint does not hold at all, which is what an ARN of another partition, Region or account names — reports no order rather than failing".

Copy 2 — src/ops/list_capacity_tasks.rs:244-258:
```
fn matches_filters(
    task: &crate::state::CapacityTaskRecord,
    outpost_filter: Option<&crate::state::ResolvedIdentifier>,
    status_filter: Option<&[String]>,
) -> bool {
    let outpost_matches = match outpost_filter {
        None => true,
        Some(filter) => filter
            .here()
            .is_some_and(|outpost_id| task.outpost_id == outpost_id),
    };
    // The status filter is a list of accepted values, so it selects by the rule every
    // list of this service selects its own list filters by.
    let status_matches = crate::state::filter_accepts(status_filter, Some(task.status.as_str()));
```
under the doc at :240-243, the same sentence with "task" for "order": "A filter is a filter and not an identifier the request is addressed to: it narrows the list the caller is reported, so one naming an Outpost that holds no task — including one this endpoint does not hold at all, which is what an ARN of another partition, Region or account names — reports no task rather than failing."

The shared home this crate already uses for the sibling rule — src/state.rs:5385-5387: "Shared by the five list operations that take filters so they cannot drift apart on that rule." with `pub fn filter_accepts(filter: Option<&[String]>, value: Option<&str>) -> bool`, and src/state.rs:5399-5401: "Shared by both list operations so they cannot drift apart" (`pub fn page<T>`). Note that copy 2 already calls `filter_accepts` for its status filter on the very next line, so one of this function's two filters goes through the shared rule and the other does not.

Tests that pin the behaviour a refactor must leave untouched: tests/all/order_listing.rs::the_outpost_filter_narrows_the_list_to_that_outposts_orders (:530-645, rows for the bare id, the ARN, an Outpost with no orders and ABSENT_OUTPOST_ID, plus the paged `?MaxResults=1&OutpostIdentifierFilter=…` rows) and tests/all/capacity_task_listing.rs::the_filters_narrow_the_list (:506-600, the same rows plus the combined `OutpostIdentifierFilter` + `CapacityTaskStatusFilter` cases) and ::a_foreign_page_token_or_an_unmodelled_filter_value_is_refused (:401-465, `OutpostIdentifierFilter=op-not-an-identifier`).

## Steps to Reproduce

1. Open src/ops/list_orders.rs at line 227 and src/ops/list_capacity_tasks.rs at line 244 — the `match outpost_filter { None => true, Some(filter) => filter.here().is_some_and(...) }` body is the same rule written twice.
2. Note src/state.rs:5385-5387 and :5399-5401, where this crate already keeps the filter-selection and page-cut rules in one place precisely so the lists "cannot drift apart".
3. Note that list_capacity_tasks.rs:257 routes its status filter through `state::filter_accepts` while its Outpost filter is hand-written two lines above.
4. Change what an absent filter means in one copy only, and observe that the other list keeps the old meaning while both docs still claim the rule is shared.

Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f9882e4630ff46bd32c7db4f9b7db9452b91004b

# ListCatalogItems' class filter is a sixth copy of the filter-selection rule state::filter_accepts already owns

_Filed: 2026-10-27T17:50:01Z_
_Operation: ListCatalogItems | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f9882e4630ff46bd32c7db4f9b7db9452b91004b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::filter_accepts` was factored out precisely so the list operations that take `*Filter` members cannot drift apart on the selection rule (absent-or-empty filter narrows nothing; otherwise the record's value must be one of the accepted values). `ListCatalogItems` is a sixth list that takes filters and does not use it: `CatalogItemRecord::is_class` re-states the same single-valued rule in state.rs, and the shared helper's own doc comment still claims it is shared by "the five list operations that take filters" — so the count in the doc is already wrong and the sixth copy can drift on what an empty filter list means. Replacing the body of `is_class` with a call to `filter_accepts` (its sibling methods `supports_storage` / `offers_family` select on MULTI-valued members, which `filter_accepts` does not cover, so they stay as they are) changes no response and no test.

## Evidence

The shared rule and the claim that it is the one home for it — `src/state.rs:5521-5537`:

    /// Whether a record's value for ONE filtered member is accepted by the `*Filter`
    /// the caller sent for that member.
    ...
    /// Shared by the five list operations that take filters so they cannot drift apart on
    /// that rule.
    pub fn filter_accepts(filter: Option<&[String]>, value: Option<&str>) -> bool {
        match filter {
            Some(accepted) if !accepted.is_empty() => {
                value.is_some_and(|value| accepted.iter().any(|candidate| candidate.as_str() == value))
            }
            _ => true,
        }
    }

The sixth copy of that same rule — `src/state.rs:1911-1920`:

    impl CatalogItemRecord {
        /// Whether this item is of ANY of the classes a filter names.
        ...
        /// so a filter carrying no value at all narrows
        /// nothing, exactly as an absent one does.
        pub fn is_class(&self, classes: &[String]) -> bool {
            classes.is_empty() || classes.contains(&self.item_class)
        }

and its only caller, which collapses absent and empty exactly as `filter_accepts` does — `src/ops/list_catalog_items.rs:186` and `:196-200`:

        let classes = input.item_class_filter.unwrap_or_default();
        ...
                .filter(|record| {
                    record.is_class(&classes)
                        && record.supports_storage(&storage)
                        && record.offers_family(&families)
                })

Tests that pin the behaviour the change must leave untouched: `tests/all/catalog_items.rs:383` (`the_class_filter_partitions_the_catalog`), `:471` (`storage_and_family_filters_select_items_matching_any_value`), `:565` (`filters_given_together_narrow_to_their_intersection`) and `:655` (`paging_the_catalog_covers_every_item_exactly_once`).

## Steps to Reproduce

1. Open src/state.rs at line 5537 and read the doc above `filter_accepts`: it states the selection rule and says it is shared so the lists "cannot drift apart on that rule".
2. Open src/state.rs at line 1918: `CatalogItemRecord::is_class` states the same rule again (`classes.is_empty() || classes.contains(&self.item_class)`).
3. Open src/ops/list_catalog_items.rs:186-200: this sixth list operation selects through `is_class` rather than through the shared helper, so the helper's doc ("the five list operations that take filters") no longer counts them all.
4. Change what an empty filter list means in `filter_accepts` only, and observe that `ItemClassFilter` keeps the old meaning while the five other lists change.

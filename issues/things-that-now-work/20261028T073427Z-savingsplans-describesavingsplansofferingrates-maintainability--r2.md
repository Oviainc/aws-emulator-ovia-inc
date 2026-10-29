Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b056d947c24faab0377cbe866250e0547cccc526

# DescribeSavingsPlansOfferingRates re-implements RateLineItem::matches_filter's property-filter rule instead of calling it

_Filed: 2026-10-28T07:34:27Z_
_Operation: DescribeSavingsPlansOfferingRates | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b056d947c24faab0377cbe866250e0547cccc526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a filter with no values constrains nothing; a line item that is not dimensioned by the property a filter names cannot satisfy it; otherwise the filter admits the values it names" is implemented twice in this crate over the SAME type (`crate::state::RateLineItem`).

`RateLineItem::matches_filter` in `src/state.rs` is the crate's home for that rule, and `DescribeSavingsPlanRates` calls it. `DescribeSavingsPlansOfferingRates`'s `item_admits` walks `input.filters` over the very same `RateLineItem` values but hand-rolls the rule again from `item.property(name)` + `filter_admits` + an inline `None` arm, including a second copy of the "unless that filter names no value at all" comment. The two copies are behaviourally identical for every `SavingsPlanRateFilterAttribute` the model declares (`region`, `instanceFamily`, `instanceType`, `productDescription`, `tenancy`, `productId` — none of which collide with the four dimension names `matches_filter` special-cases), so swapping the loop body for `item.matches_filter(name, filter.values.as_ref())` changes no response and no existing test; but while both copies exist, a change to the rule in one is invisible to the other.

The same rule appears a third time in `DescribeSavingsPlansOfferings`'s `admits`, over `Offering` (which also exposes a `property(&str)` accessor), so the crate carries three spellings of one admission rule.

The fix is mechanical and must leave every response and every existing test unchanged.

## Evidence

The helper, src/state.rs:1435-1457:
```
    /// Whether a `SavingsPlanRateFilter` admits this line item. `name` is a
    /// `SavingsPlanRateFilterName`; a filter with no values constrains nothing.
    ...
    /// item that is not dimensioned by the property a filter names cannot
    /// satisfy it.
    pub fn matches_filter(&self, name: &str, values: Option<&Vec<String>>) -> bool {
        let field = match name {
            "productType" => Some(self.product_type.as_str()),
            ...
            property => self.property(property),
        };
        match (values, field) {
            (Some(values), _) if values.is_empty() => true,
            (None, _) => true,
            (Some(_), None) => false,
            (Some(values), Some(field)) => values.iter().any(|value| value == field),
        }
    }
```

One path CALLS it — src/ops/describe_savings_plan_rates.rs:229-236:
```
fn admits(input: &DescribeSavingsPlanRatesInput, item: &crate::state::RateLineItem) -> bool {
    input.filters.iter().flatten().all(|filter| {
        filter
            .name
            .as_deref()
            .is_none_or(|name| item.matches_filter(name, filter.values.as_ref()))
    })
}
```

The other path re-implements it for the same type — src/ops/describe_savings_plans_offering_rates.rs:288-301:
```
    for filter in input.filters.iter().flatten() {
        let Some(name) = filter.name.as_deref() else {
            continue;
        };
        let admitted = match item.property(name) {
            Some(value) => filter_admits(filter.values.as_ref(), value),
            // A line item that is not priced along the property cannot satisfy a
            // filter on it, unless that filter names no value at all.
            None => filter.values.as_ref().is_none_or(Vec::is_empty),
        };
        if !admitted {
            return false;
        }
    }
```

Third copy, over `Offering` — src/ops/describe_savings_plans_offerings.rs:250-263:
```
    for filter in input.filters.iter().flatten() {
        let Some(name) = filter.name.as_deref() else {
            continue;
        };
        let admitted = match offering.property(name) {
            Some(value) => crate::state::filter_admits(filter.values.as_ref(), value),
            // An offering that does not carry the property cannot satisfy a
            // filter on it, unless that filter names no value at all.
            None => filter.values.as_ref().is_none_or(Vec::is_empty),
        };
```

Test coverage that must stay green and unchanged: tests/all/offering_rate_catalog.rs:566-595 (`filters region us-west-2`, `filters instanceFamily r6i`, `filters productDescription Linux/UNIX`, `filters tenancy shared`) and tests/all/offering_rate_catalog.rs:645-659 (`"filters tenancy dedicated"`, `"filters productId, a dimension no rate is priced along"` — the two rows that pin the `None`/no-match arms), plus tests/all/plan_rate_card.rs:507 and :543, which drive the helper's own path through `DescribeSavingsPlanRates`.

## Steps to Reproduce

1. Open src/state.rs and read `RateLineItem::matches_filter` (lines 1435-1457): the whole admission rule for a rate filter, including the "not dimensioned by the property" arm.
2. Open src/ops/describe_savings_plan_rates.rs:229-236 and observe `admits` delegating to that helper.
3. Open src/ops/describe_savings_plans_offering_rates.rs:275-303 and observe `item_admits` re-deriving the same rule for the same `RateLineItem` type with `item.property(name)`, `filter_admits` and an inline `None` arm.
4. Open src/ops/describe_savings_plans_offerings.rs:250-263 for the third spelling of the rule over `Offering`.
5. Note that a change to the rule (say, case-insensitive value matching, or treating a filter naming an unknown attribute as unconstraining) would have to be made in three places, and that nothing in the crate fails if only one is changed.

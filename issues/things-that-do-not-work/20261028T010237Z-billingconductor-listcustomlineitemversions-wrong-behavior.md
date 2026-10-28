Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 21d333ac35778a98ed9861b96af3d5cd9f44ed91

# ListCustomLineItemVersions reports a generation a bounded delete left applying in no billing period, contrary to the rule the crate's own write path enforces

_Filed: 2026-10-28T01:02:37Z_
_Operation: ListCustomLineItemVersions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `21d333ac35778a98ed9861b96af3d5cd9f44ed91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A custom line item's generations are each held with the billing-period stretches they apply in. Two write paths in this crate empty those stretches, and only ONE of them prunes a generation that is left with none:

- `ComAmazonawsBillingconductorAwsbillingconductorState::put_custom_line_item` (the `UpdateCustomLineItem` path) removes the periods a write takes over from every kept generation and then DROPS the generations that keep none, with a comment stating the rule: "A generation this write left no billing period to is not a version of the item in any period, so there is nothing for a read to report it for."
- `CustomLineItemRecord::remove_periods` (the `DeleteCustomLineItem` narrowing path) removes the periods from the live spans AND from every superseded generation's spans — its doc says "EVERY generation loses them, because a period the item was deleted from reports no version of it at all" — but it writes the emptied generations back without pruning them.

`ListCustomLineItemVersions` then reports those emptied generations: it pages over `item.versions()` (live + every superseded generation) and the only skip is the billing-period FILTER, which passes everything when the request names no filter (`in_filter` returns `true` for `filter == None`). So after an update that opened a new generation and a bounded delete that removed exactly the earlier generation's periods, the versions listing reports a version that applies in no billing period at all — and reports it with `StartBillingPeriod` equal to its `EndBillingPeriod`, i.e. a half-open range covering nothing, because `CustomLineItemVersion::start_billing_period()` falls back to the instant the generation took effect when its span list is empty and `end_billing_period()` falls back to the successor's start.

That contradicts every other read of the same item: `ListCustomLineItems` for the deleted period reports nothing (`CustomLineItemRecord::effective_in` finds no generation covering it) and `GetBillingGroupCostReport` charges nothing for it (`covers` is false), while the version history still shows a generation for it. The fix is the branch the crate already has on the other write path: prune generations left with no spans when `remove_periods` writes them back (or skip them in the listing), which leaves every other response unchanged.

## Evidence

The rule, stated and enforced on the UPDATE write path — `src/state.rs:2742-2744` (in `put_custom_line_item`):
```
            // A generation this write left no billing period to is not a version of the
            // item in any period, so there is nothing for a read to report it for.
            kept.retain(|version| !version.spans.is_empty());
```

The DELETE write path, which empties generations and writes them back unpruned — `src/state.rs:1795-1797` (doc) and `:1812-1816` (code):
```
    /// EVERY generation loses them, because a period the item was deleted from reports
    /// no version of it at all. Nothing is written unless every generation's removal
    /// stays inside [`MAX_BILLING_PERIOD_SPANS`], so a refused removal leaves the whole
...
        if outcome == PeriodsRemoval::Changed {
            self.spans = spans;
            self.superseded_versions = superseded;
        }
```
called directly on the stored record by `src/ops/delete_custom_line_item.rs:217-228`:
```
                match item.remove_periods(&removed) {
                    crate::state::PeriodsRemoval::Changed => {
                        // The record changed, so the instant a read reports as its most
                        // recent modification advances with it.
                        item.last_modified_time_millis = crate::state::now_millis();
                        // An item left applying in no billing period at all has nothing
                        // for a read to report, so its record goes.
                        if item.is_empty() {
```
(`CustomLineItemRecord::is_empty`, `src/state.rs:1780-1786`, asks whether ALL generations are empty, so an item whose live generation still applies survives with the emptied one attached.)

The read that reports it — `src/ops/list_custom_line_item_versions.rs:327-337`:
```
        let versions = item.versions();
        let mut selected: Vec<(String, CustomLineItemVersionListElement)> = Vec::new();
        for (index, version) in versions.iter().enumerate() {
            let end = end_billing_period(version, versions.get(index + 1));
            if !in_filter(&version.start_billing_period(), end.as_deref(), filter) {
                continue;
            }
```
with the only skip being the filter, which admits everything when absent — `src/ops/list_custom_line_item_versions.rs:163-165`:
```
    let Some(filter) = filter else {
        return true;
    };
```
and the fallbacks that make an emptied generation render a range covering nothing — `src/state.rs:1932-1937` and `:1946-1950`:
```
    pub fn start_billing_period(&self) -> String {
        self.spans
            .first()
            .and_then(|span| span.inclusive_start.clone())
            .unwrap_or_else(|| billing_period_at(self.start_time_millis))
    }
...
    pub fn exclusive_end_billing_period(&self) -> Option<String> {
        self.spans
            .last()
            .and_then(|span| span.exclusive_end.clone())
    }
```

The contradicting sibling read — `src/state.rs:1766-1776` (`CustomLineItemRecord::effective_in`) reports NO version for a period no generation's spans cover, which is what `ListCustomLineItems` (`src/ops/list_custom_line_items.rs:291-293`) and `proforma_custom_line_item_charges` (`src/state.rs:3160`, via `item.covers`) answer with.

Test coverage: no suite pins the current answer for this shape. `tests/all/delete_custom_line_item.rs:1110-1161` ("the history of a bisected item") exercises a bounded delete against an item with ONE generation and asserts `elements.len() == 1`; `tests/all/list_custom_line_item_versions.rs` and `tests/all/custom_line_item_versions_by_billing_period.rs` build their histories with `UpdateCustomLineItem` only — the path that prunes — so none of them has a generation emptied by a delete.

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable, then POST /create-pricing-plan and POST /create-billing-group.
2. POST /create-custom-line-item with a Flat ChargeDetails and BillingPeriodRange {"InclusiveStartBillingPeriod": "<previous billing period>"}; keep the returned Arn. The item now applies from the previous period onward in one generation.
3. POST /update-custom-line-item with {"Arn": "<item>", "Description": "second", "BillingPeriodRange": {"InclusiveStartBillingPeriod": "<current billing period>"}}. The create's generation is frozen holding exactly [previous, current) and a new generation holds [current, open).
4. POST /delete-custom-line-item with {"Arn": "<item>", "BillingPeriodRange": {"InclusiveStartBillingPeriod": "<previous billing period>", "ExclusiveEndBillingPeriod": "<current billing period>"}} — 200, and the first generation is left with no billing-period span at all.
5. POST /list-custom-line-items with {"BillingPeriod": "<previous billing period>"} — the item is correctly absent.
6. POST /list-custom-line-item-versions with {"Arn": "<item>"} and no Filters — TWO CustomLineItemVersions come back, and the older element reports StartBillingPeriod == EndBillingPeriod (an exclusive end equal to the inclusive start, so it covers no billing period), although the delete removed that generation from every period it applied in.
7. Compare the update-only path: make the same history with two /update-custom-line-item calls whose ranges supersede each other completely and observe that put_custom_line_item drops the generation left with no periods, so the listing reports only the generations that apply somewhere.

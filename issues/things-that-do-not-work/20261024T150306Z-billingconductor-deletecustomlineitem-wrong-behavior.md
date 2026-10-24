Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# A bounded DeleteCustomLineItem range deletes the item from every later billing period too, not just the periods it named

_Filed: 2026-10-24T15:03:06Z_
_Operation: DeleteCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `DeleteCustomLineItem.BillingPeriodRange` names the billing periods the item is deleted FROM — `InclusiveStartBillingPeriod` is in the range and `ExclusiveEndBillingPeriod` is not — so a delete whose range lies strictly inside the item's effective span removes the item only for those periods and the item keeps applying from the range's exclusive end onwards. After creating an item with no range (so it applies to every period) and deleting it with {InclusiveStartBillingPeriod: 2026-06, ExclusiveEndBillingPeriod: 2026-08}, a `ListCustomLineItems` for 2026-06 and 2026-07 must report nothing while 2026-08, 2026-09 and later must still report the item.

Actual: the handler can only store ONE half-open span per item, so when the deleted range is interior to the item's span it keeps the part BEFORE the delete and silently discards everything from the delete's exclusive end onwards (`remaining_span` returns `Narrowed { inclusive_start: item_start, exclusive_end: delete_start }`). The item therefore disappears from billing periods the request never named: the delete removes more than it was asked to. The same path also makes the outcome asymmetric — a delete that overlaps the item's head is honoured exactly, a delete that bisects the span is not.

## Evidence

Smithy model: `CustomLineItemBillingPeriodRange` members are named `InclusiveStartBillingPeriod` / `ExclusiveEndBillingPeriod` and `DeleteCustomLineItemInput$BillingPeriodRange` is documented as the range the deletion is applied in; `CustomLineItemVersionListElement` (StartBillingPeriod/EndBillingPeriod per version) establishes that one item spans several per-period generations. Handler: `src/ops/delete_custom_line_item.rs` lines 278-316 (`remaining_span`), whose own comment concedes "A request whose range ends inside the item's span would leave both parts, which one stored span cannot hold ... so the part before the request's start is what remains and the item ends where the delete begins". State: `CustomLineItemRecord` holds exactly one `inclusive_start_billing_period`/`exclusive_end_billing_period` pair (`src/state.rs` lines 517-541). Test coverage: `tests/all/delete_custom_line_item.rs` lines 397-403 PIN the divergence — for an item created with no range and deleted with 2026-06..2026-08 the test asserts 2026-09 is NOT reported — and no case exercises a delete strictly inside a bounded span. Distinct from the filed UpdateCustomLineItem/wrong-behavior issue, which is about the update path rewriting the span.

## Steps to Reproduce

1. Authorize as the payer account; POST /create-pricing-plan then POST /create-billing-group to get a billing group ARN
2. POST /create-custom-line-item with no BillingPeriodRange, Name="item1", ChargeDetails={"Type":"FEE","Flat":{"ChargeValue":10}} and note the Arn
3. POST /delete-custom-line-item with {"Arn":"<item>","BillingPeriodRange":{"InclusiveStartBillingPeriod":"2026-06","ExclusiveEndBillingPeriod":"2026-08"}}
4. POST /list-custom-line-items with {"BillingPeriod":"2026-07"} — correctly reports no item
5. POST /list-custom-line-items with {"BillingPeriod":"2026-09"} (and "2026-08") — the item is absent, where the real service still reports it because the delete named only 2026-06 and 2026-07

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# An activeTimeRange filter hides the account's PRIMARY billing view, although the model (and the handler's own doc) say PRIMARY is always listed

_Filed: 2026-11-01T12:33:34Z_
_Operation: ListBillingViews | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`matches_filters` applies the `activeTimeRange` predicate uniformly to every stored view, including the account's service-managed `PRIMARY` view: a view whose `created_at` is later than `activeBeforeInclusive` is dropped from the page. Because the emulator seeds the `PRIMARY` view when the state is first constructed (its `created_at` is boot time), ANY `activeTimeRange` whose `activeBeforeInclusive` is earlier than the process start — i.e. every historical window a caller asks about — returns a page with no `PRIMARY` entry at all.

The model documents the opposite for this very member: the `PRIMARY` view is listed whatever the time range is, and only `BILLING_GROUP` views are time-range-sensitive. The handler's own module doc repeats that sentence verbatim and the code under it breaks it. A client that lists views for last month to discover the default view it may query cost data against sees an empty list instead of the one view every account always owns.

## Evidence

Smithy (`smithy_query(query="doc", shape="ListBillingViewsRequest", member="activeTimeRange")`): "The time range for the billing views listed. PRIMARY billing view is always listed. BILLING_GROUP billing views are listed for time ranges when the associated billing group resource in Billing Conductor is active. The time range must be within one calendar month."

The same sentence in the handler's own module doc — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:57-60:
```
//! * `activeTimeRange` (ActiveTimeRange) — The time range for the billing views listed.
//!   PRIMARY billing view is always listed. BILLING_GROUP billing views are listed for time
//!   ranges when the associated billing group resource in Billing Conductor is active. The
//!   time range must be within one calendar month.
```
The code under it — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:262-270, inside `matches_filters`, which is called for EVERY view at line 353 with no exemption for the `PRIMARY` type:
```
    if let Some(range) = &input.active_time_range {
        // A view is active over a period once it exists: one created after the
        // period ended was not available during it.
        if crate::state::epoch_secs(view.created_at)
            > crate::state::date_time_secs(&range.active_before_inclusive)
        {
            return Ok(false);
        }
    }
```
Existing test coverage pins the contradicting answer, as a bare table row with no stated reason — services/service-com-amazonaws-billing-awsbilling/tests/all/billing_views.rs:486-490:
```
        (
            "activeTimeRange ending before every view was created",
            json!({ "activeTimeRange": { "activeAfterInclusive": 0, "activeBeforeInclusive": 1 } }),
            vec![],
        ),
```
(the expected list is empty, i.e. `PRIMARY_ARN` is absent), while the neighbouring rows at lines 456-460 show the same suite expects `PRIMARY_ARN` to be listed for other filters.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call ListBillingViews with no filters — observe the account's PRIMARY view `arn:aws:billing::<account>:billingview/primary` in `billingViews`.
3. Call ListBillingViews with `activeTimeRange = { activeAfterInclusive: 0, activeBeforeInclusive: 1 }` (or any window that ends before the emulator process started).
4. Observe `billingViews` is empty: the PRIMARY view was filtered out by `created_at > activeBeforeInclusive`, although the modeled member doc states "PRIMARY billing view is always listed".

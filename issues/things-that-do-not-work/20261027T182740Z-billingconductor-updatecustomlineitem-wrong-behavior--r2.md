Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2ead5306d42c2c14ef865f57d355d9f40696e5a

# UpdateCustomLineItem advances LastModifiedTime on a call that modified nothing, unlike every sibling writer of the same resource

_Filed: 2026-10-27T18:27:40Z_
_Operation: UpdateCustomLineItem | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2ead5306d42c2c14ef865f57d355d9f40696e5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateCustomLineItem`'s merge sets `last_modified_time_millis = now_millis()` unconditionally, on every request the handler does not refuse — including a request that carries nothing but `Arn`, which replaces no stored member at all. The handler's own doc comment justifies the move with "`LastModifiedTime` advances because the record changed", but the code reaches that line when the record did NOT change.

The crate already implements the opposite rule twice, once for the very same resource:
- `DeleteCustomLineItem` (same custom line item record) moves the instant only in the `PeriodsRemoval::Changed` arm and leaves it alone in the `Unchanged` arm.
- `UpdateBillingGroup` computes a `changed` flag and moves `last_modified_time_millis` only inside `if changed`, with a comment spelling the rule out.

So one crate answers two opposite things for "the most recent time when the resource was modified" after a no-op write: a custom line item's instant jumps forward, a billing group's and a narrowing delete's do not. The fix is the branch the crate already has: compare the merged record against the stored one and advance the instant only when something actually differs (the refusal paths are unchanged).

## Evidence

Handler, unconditional move — `src/ops/update_custom_line_item.rs:236-239` (merge's doc) and `:332`:
```
/// The stored item as this request leaves it: a member the request carries replaces
/// the stored one, a member it omits keeps what state holds, and `LastModifiedTime`
/// advances because the record changed ("The most recent time when the custom line
/// item was modified").
...
    updated.last_modified_time_millis = crate::state::now_millis();
```

Sibling on the SAME resource — `src/ops/delete_custom_line_item.rs:217-233`:
```
                match item.remove_periods(&removed) {
                    crate::state::PeriodsRemoval::Changed => {
                        // The record changed, so the instant a read reports as its most
                        // recent modification advances with it.
                        item.last_modified_time_millis = crate::state::now_millis();
...
                    // The request named only billing periods the item never applied in,
                    // so there is nothing of it to delete from them and no stored value
                    // changes.
                    crate::state::PeriodsRemoval::Unchanged => ("unchanged", false),
```

Sibling update — `src/ops/update_billing_group.rs:360-371`:
```
        // `LastModifiedTime` is "the most recent time when the billing group was
        // modified", so it moves only when this call actually modified the group: a
        // request that names no member, or names the values the group already holds,
        // leaves it where it was.
        let changed = updated.name != stored.name
            || updated.description != stored.description
...
        if changed {
            updated.last_modified_time_millis = crate::state::now_millis();
```

That a member-less update is served (so the line is reachable) is pinned by `tests/all/update_custom_line_item.rs:294`:
```
        ("nothing but the Arn", json!({}), "original", "the original custom line item", 1.0, 0),
```
and the only `LastModifiedTime` assertions in that suite are weak — `:420-425` ("at or after the creation time") and `:979-988` ("never goes backwards") — so neither pins the advance. The sibling rule IS pinned, by `tests/all/update_billing_group.rs:1120` `last_modified_time_moves_only_when_the_group_changed` (":1184" message: "a call that modified nothing must leave LastModifiedTime where it ...") and by `tests/all/delete_custom_line_item.rs:581` ("while a delete that changes nothing leaves the ...").

## Steps to Reproduce

1. Designate the emulated account as the payer so the control plane is reachable.
2. POST /create-pricing-plan, POST /create-billing-group, then POST /create-custom-line-item with a Flat ChargeDetails; keep the item Arn.
3. POST /list-custom-line-items and note the item's LastModifiedTime (call it T0).
4. Wait a millisecond, then POST /update-custom-line-item with {"Arn":"<item arn>"} only — no Name, no Description, no ChargeDetails, no BillingPeriodRange.
5. Observe the 200 response's LastModifiedTime > T0 and a following /list-custom-line-items reporting the moved value, although nothing about the item changed.
6. Compare: POST /update-billing-group with only {"Arn":"<group arn>"} leaves the group's LastModifiedTime at its previous value, and a DeleteCustomLineItem naming only periods the item never applied in leaves the item's LastModifiedTime alone.

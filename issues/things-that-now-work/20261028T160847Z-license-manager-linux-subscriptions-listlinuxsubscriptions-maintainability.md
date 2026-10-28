Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0b62ce506db227f8753aee084e9b25daeb773a55

# Deduplicate the discovery-pass gate and the filter-conjunction rule shared by the two discovery lists

_Filed: 2026-10-28T16:08:47Z_
_Operation: ListLinuxSubscriptions | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0b62ce506db227f8753aee084e9b25daeb773a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two discovery lists (`ListLinuxSubscriptions`, `ListLinuxSubscriptionInstances`) each carry their OWN copy of two rules that must hold identically for both, because both report from the single discovered-instance store:

1. WHEN a report materializes a discovery pass — "a request continuing a walk runs no pass" — written as the branch `if input.next_token.is_none() { refresh_discovered_instances(..).await }` in both handlers.
2. How several `Filters` combine — "several filters select what satisfies all of them" — written as `filters.iter().all(|filter| crate::state::filter_selects(..))` in both handlers.

Neither rule is per-operation: `crate::state::discovery` already owns the pass itself and `crate::state::filter_selects`/`validate_filters` already own the per-filter comparison, so the two decisions that are left are the only part of the rule living in two places. The second copy's comment even DEFERS to the first ("The same discovery pass the sibling list materializes … As there, a request continuing a walk runs no pass"), which is exactly the drift hazard: changing the gate in one handler silently makes the two lists disagree about which inventory they report from, while the crate's own state docs assert the two "can never disagree about what was discovered".

Fix shape (no response and no test changes): let `crate::state::discovery` own the gate (e.g. take the request's `next_token`/`is_continuation` and return early), and put the filter conjunction in one helper beside `filter_selects`, so each handler states only what is its own (the filter-name set and the member projection).

## Evidence

Copy 1 of the pass gate — services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/list_linux_subscriptions.rs:172-179:
```
        // A request CONTINUING a walk carries the cursor the previous page minted, which is
        // a position in the list that page reported, so no pass runs for it: a refresh
        // between two pages of one walk could move a record across the cursor and make the
        // walk skip or repeat it. A walk therefore reports the inventory of the pass its
        // first page ran.
        if input.next_token.is_none() {
            crate::state::discovery::refresh_discovered_instances(&*self.engine, ctx).await;
        }
```
Copy 2, deferring to copy 1 — src/ops/list_linux_subscription_instances.rs:172-178:
```
        // The same discovery pass the sibling list materializes: both report from the ONE
        // store of discovered instances, so neither can be shown an inventory the other was
        // not. As there, a request continuing a walk runs no pass: its cursor is a position
        // in the list the previous page reported.
        if input.next_token.is_none() {
            crate::state::discovery::refresh_discovered_instances(&*self.engine, ctx).await;
        }
```
Copy 1 of the filter conjunction — src/ops/list_linux_subscriptions.rs:187-195:
```
            .filter(|subscription| {
                // Several filters select what satisfies all of them; `Subscription` is
                // the only name this list accepts, and it addresses the subscription's
                // own name.
                filters.iter().all(|filter| {
                    crate::state::filter_selects(filter, std::slice::from_ref(&subscription.name))
                })
            })
```
Copy 2 — src/ops/list_linux_subscription_instances.rs:186-192:
```
            .filter(|record| {
                // Several filters select what satisfies all of them.
                filters.iter().all(|filter| {
                    let name = filter.name.as_deref().unwrap_or_default();
                    crate::state::filter_selects(filter, &filtered_member(record, name))
                })
            })
```
The invariant the copies are supposed to preserve, stated once in the state module — src/state.rs:225-229:
```
/// discovery pass ([`discovery::refresh_discovered_instances`]). Both
/// `ListLinuxSubscriptionInstances` (which reports these records) and
/// `ListLinuxSubscriptions` (which aggregates them into subscriptions) project it, so
/// the two lists can never disagree about what was discovered.
```
Existing coverage that must stay green and unchanged: tests/all/linux_subscriptions.rs ("`MaxResults`/`NextToken` page each list exactly once over every element, with no token on the last page"; "a `Filter` selects on the member its `Name` addresses … and several filters select what satisfies all of them") and tests/all/discovery_from_ec2.rs (the pass a first-page report materializes, driven against EC2 mounted on the same engine).

## Steps to Reproduce

1. Open src/ops/list_linux_subscriptions.rs and src/ops/list_linux_subscription_instances.rs side by side.
2. Compare lines 172-179 of the first with lines 172-178 of the second: the same `if input.next_token.is_none() { refresh_discovered_instances(..) }` gate is written twice, the second copy's comment deferring to the first ("As there, …").
3. Compare lines 187-195 of the first with lines 186-192 of the second: the `filters.iter().all(..)` conjunction is written twice.
4. Change the gate in one file only (for example drop the `next_token` check) and observe that nothing in the crate forces the sibling list to follow, even though src/state.rs:225-229 asserts the two lists can never disagree about what was discovered.

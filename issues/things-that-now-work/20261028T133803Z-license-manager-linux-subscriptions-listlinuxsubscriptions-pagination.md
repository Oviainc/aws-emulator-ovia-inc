Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# A paginated walk of either discovery list is not isolated from the sibling list's discovery refresh, breaking the invariant the handler states

_Filed: 2026-10-28T13:38:03Z_
_Operation: ListLinuxSubscriptions | Class: pagination_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both discovery lists project the ONE shared `discovered_instances` store, and each refreshes it from EC2 on any request that carries no `NextToken`. `ListLinuxSubscriptions` states the resulting invariant explicitly: a continuation page runs no pass, "so a refresh between two pages of one walk could move a record across the cursor and make the walk skip or repeat it. A walk therefore reports the inventory of the pass its first page ran."

That invariant does not hold, because suppressing the refresh on *this* list's continuation pages does not suppress it on the SIBLING list. `ListLinuxSubscriptionInstances` refreshes the same store on every first-page call, and the refresh writes through `record_discovery_pass`, which does `self.discovered_instances.retain(|_, record| record.region != region)` before re-inserting — it can REMOVE records. Since the cursor is a plain offset into the list rebuilt from that store on each page (`crate::state::paginate` over `discovered_subscriptions()` / `discovered_instances_in_order()`), a first-page call on the other list in between two pages of a walk shifts every later element across the cursor, making the walk skip or repeat exactly the way the comment says it must not. The same holds symmetrically for a walk of `ListLinuxSubscriptionInstances` interleaved with a first-page `ListLinuxSubscriptions`.

So the code implements the no-refresh-on-continuation rule only against itself, while the comment claims a property of the whole walk.

## Evidence

The invariant the handler states, and the refresh guarded only on its OWN token — `src/ops/list_linux_subscriptions.rs:171-179`:
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
The sibling handler refreshing the SAME store, and admitting the store is shared — `src/ops/list_linux_subscription_instances.rs:172-178`:
```
        // The same discovery pass the sibling list materializes: both report from the ONE
        // store of discovered instances, so neither can be shown an inventory the other was
        // not. As there, a request continuing a walk runs no pass: its cursor is a position
        // in the list the previous page reported.
        if input.next_token.is_none() {
            crate::state::discovery::refresh_discovered_instances(&*self.engine, ctx).await;
        }
```
The refresh can REMOVE records, which is what moves elements across an offset cursor — `src/state.rs:443-456`:
```
    pub fn record_discovery_pass(
        &mut self,
        region: &str,
        found: Vec<DiscoveredInstanceRecord>,
        now: SystemTime,
    ) {
        let timestamped = self.timestamped(found, now);
        self.discovered_instances
            .retain(|_, record| record.region != region);
```
The cursor is a plain offset re-resolved against the rebuilt list on every page — `src/state.rs:840-859`:
```
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
...
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
```
Test coverage: `tests/all/linux_subscriptions.rs:707-795` (`max_results_pages_each_list_exactly_once`) walks each list with every page size and asserts "the paged walk must visit every entry exactly once, in order", and `tests/all/discovery_from_ec2.rs:674` (`a_walk_of_the_discovered_list_covers_every_record_once`) walks the EC2-driven list — neither interleaves a first-page call to the OTHER list inside a walk, so the gap is untested.

## Steps to Reproduce

1. Mount this service beside EC2 on one engine as `tests/all/discovery_from_ec2.rs` does, and POST /subscription/UpdateServiceSettings with {"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["us-west-2"]}}.
2. Launch several EC2 instances in us-west-2 from AMIs that bill Linux subscriptions, so both lists report more than one page.
3. POST /subscription/ListLinuxSubscriptions with {"MaxResults": 1} and keep the returned NextToken (page 1 of a walk).
4. Before sending page 2, terminate one of the instances through EC2 and POST /subscription/ListLinuxSubscriptionInstances with an empty body — a first-page call, so it runs a discovery pass that calls `record_discovery_pass` for us-west-2 and drops the terminated record from the shared store.
5. POST /subscription/ListLinuxSubscriptions with {"MaxResults": 1, "NextToken": <token from step 3>}: the offset now points into a shorter list, so the walk skips an entry (or, when the pass adds a record, repeats one) — the exact outcome the handler comment at src/ops/list_linux_subscriptions.rs:171-176 says cannot happen because "A walk therefore reports the inventory of the pass its first page ran".

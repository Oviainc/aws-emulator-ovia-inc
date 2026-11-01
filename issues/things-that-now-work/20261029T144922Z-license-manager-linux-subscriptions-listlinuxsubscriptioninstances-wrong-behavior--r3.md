Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a8bb77f04de11c29da4c1926ec69265ce5cd3881

# The discovery pass never resumes, so source Regions past the shared page budget are never discovered at all

_Filed: 2026-10-29T14:49:22Z_
_Operation: ListLinuxSubscriptionInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a8bb77f04de11c29da4c1926ec69265ce5cd3881`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`materialize_for_report` -> `refresh_discovered_instances` walks the account's source Regions with ONE shared `Budget` (`pages: MAX_INVENTORY_PAGES = 8`, `records: MAX_DISCOVERED_INSTANCES = 1_000`) and breaks out of the Region loop as soon as the budget is spent. The plan it walks is `state.lock().discovered_regions()`, a `BTreeSet<String>` — i.e. the SAME Regions in the SAME sorted order on every pass — and the budget is re-initialised from the two constants at the top of every pass. Nothing anywhere records where the previous pass stopped.

The consequence is that the Regions after the one that exhausts the budget get no EC2 hop on ANY call, not just on the call that ran out: every later pass re-reads the identical alphabetical prefix, spends the identical budget on it, and breaks at the identical point. `retain_discovered_regions()` then keeps those Regions (they are still in `SourceRegions`), so they simply hold nothing, permanently. An account that configures `SourceRegions` with more than 8 Regions that each hold instances (the modelled `SourceRegions` list admits up to 100 entries) therefore gets instances in its alphabetically-later Regions reported by NEITHER `ListLinuxSubscriptionInstances` NOR `ListLinuxSubscriptions` — ever — and no caller can tell, because the pages the client does get answer 200.

This is not a complaint about the value of either constant: it is that two doc comments in this same module state that the remainder is deferred to a later pass and that every configured Region gets a hop, and the code under them implements neither. `RegionPass::Truncated` exists precisely so a partial read does not erase what an earlier pass found, which shows the module intends the remainder to be picked up later; nothing picks it up.

## Evidence

The module's own promise that the remainder is deferred (src/state/discovery.rs:110-115):
```
/// The most pages of the EC2 inventory ONE refresh reads, over all of its Regions.
///
/// `DescribeInstances` answers a page of reservations and a token for the next, so without
/// a cap one report could walk an unbounded inventory. A pass that spends the budget
/// records what it read and leaves the rest to the next one.
const MAX_INVENTORY_PAGES: usize = 8;
```
and that the budget is not a ceiling on what the store holds (src/state/discovery.rs:117-124):
```
/// The most instances of the account's inventory ONE refresh reads, over all of its
/// Regions.
///
/// A budget on the work one report does, not a ceiling on the store: what the store holds
/// is what the owner's inventory in the discovered Regions holds, which is why a Region a
/// pass read to its end REPLACES what was discovered there and a Region the settings no
/// longer name is dropped entirely.
const MAX_DISCOVERED_INSTANCES: usize = 1_000;
```
and that every configured Region is asked (src/state/discovery.rs:188-190):
```
/// One hop per configured source Region (the modelled `SourceRegions` list bounds that at
/// 100 entries), plus the AMI hops the instances found there need, all bounded by the
/// constants above — and the store's lock is never held across any of them.
```
The code those three comments sit on (src/state/discovery.rs:208-217) — the plan is the same sorted set every time, the budget is fresh every time, and a spent budget abandons the remaining Regions with no record of where it stopped:
```
    let planned: BTreeSet<String> = state.lock().discovered_regions();

    let mut budget = Budget {
        pages: MAX_INVENTORY_PAGES,
        records: MAX_DISCOVERED_INSTANCES,
    };
    for region in &planned {
        if budget.spent() {
            break;
        }
```
with (src/state/discovery.rs:266-271):
```
impl Budget {
    /// Whether there is nothing left to spend.
    fn spent(&self) -> bool {
        self.pages == 0 || self.records == 0
    }
}
```
and one page of one Region costing one unit of the shared page budget (src/state/discovery.rs:325):
```
        budget.pages -= 1;
```
The model admits the input that reaches it (`smithy_query(query="shape",shape="LinuxSubscriptionsDiscoverySettings")`):
```
"SourceRegions": {"target": "StringList", "required": true, "length": {"min": 1, "max": 100}, "uniqueItems": true}
```
Test coverage: nothing pins this. `tests/all/discovery_from_ec2.rs:645-661` (`discovery_covers_exactly_the_configured_source_regions`) asserts the rule over at most TWO Regions (`(&[REGION, OTHER_REGION], ...)`), and `tests/all/discovery_pass_settings_change.rs` / `discovery_switch_off.rs` likewise use a handful of Regions, so no test configures more Regions than the shared page budget can serve and no comment or test states a reason for the abandoned remainder.

## Steps to Reproduce

1. Mount this service beside the EC2 emulator on one engine (as tests/all/discovery_from_ec2.rs::Fleet does) and launch one instance from a RHEL-billing AMI in each of 10 distinct Regions, e.g. ap-northeast-1 … us-west-2.
2. POST /subscription/UpdateServiceSettings with {"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"SourceRegions":[all 10 Regions],"OrganizationIntegration":"Disabled"}} — 10 entries is inside the modelled SourceRegions length 1..100.
3. POST /subscription/ListLinuxSubscriptionInstances with {} and collect the reported Instance$Region values.
4. Observe that only the first 8 Regions of the sorted plan appear: the shared Budget's `pages` reaches 0 after the 8th Region's DescribeInstances page, so `for region in &planned { if budget.spent() { break; } }` abandons the last two.
5. Call POST /subscription/ListLinuxSubscriptionInstances with {} again, any number of times, and POST /subscription/ListLinuxSubscriptions with {} too.
6. Observe the same 8 Regions every time and never the last two: each pass rebuilds `planned` from the same BTreeSet and the budget from the same constants, so the 'rest' the comment at src/state/discovery.rs:113-114 leaves 'to the next one' is never read, and the instances in those Regions are reported by neither discovery list.

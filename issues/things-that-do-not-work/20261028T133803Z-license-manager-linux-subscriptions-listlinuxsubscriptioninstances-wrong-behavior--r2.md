Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# The discovery pass acts on a settings snapshot it never re-reads, so it writes for Regions (and for an account) the settings no longer name

_Filed: 2026-10-28T13:38:03Z_
_Operation: ListLinuxSubscriptionInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`refresh_discovered_instances` reads the account's `LinuxSubscriptionsDiscovery` and `SourceRegions` ONCE, before the first EC2 hop, and then uses that snapshot for everything it does afterwards — including the pruning step that decides which Regions the store may keep data for. The pass awaits repeatedly between the snapshot and the pruning (one or more `DescribeInstances` hops plus the `DescribeImages` hops per Region), and the settings singleton can be rewritten by `UpdateServiceSettings` during that window.

Two stated rules are broken by the stale snapshot:

1. The pruning comment claims the pass drops a Region the settings no longer name "including one dropped while this pass was reading another". It cannot: `retained` is built from `discovered_regions`, which is built from the `source_regions` snapshot taken before the hops, so a Region removed from `SourceRegions` mid-pass is still in `retained` and its records are kept rather than dropped.
2. The module rule for a non-enabled account — "Nothing happens for an account whose `LinuxSubscriptionsDiscovery` is not enabled: there is no discovery to run, so the store is left exactly as it is" — is broken when discovery is disabled mid-pass: the in-flight pass still calls `record_discovery_pass` / `merge_discovery_pass` / `retain_discovered_regions`, so the store is both filled and pruned for an account that, by the time those writes land, has asked for no discovery at all.

This is distinct from the already-filed `UpdateServiceSettings` issue about a disabled account keeping its old inventory: that one is about the steady state after a disable, this one is about an in-flight pass writing against settings that no longer exist.

## Evidence

The snapshot, taken once before any hop — `src/state/discovery.rs:163-179`:
```
    // The lock is taken and released HERE, before the first hop: what the pass needs from
    // the store is the account's settings, and the store is never held across an await.
    let settings = {
        let store = state.lock();
        store.service_settings.as_ref().map(|settings| {
            (
                settings.linux_subscriptions_discovery.clone(),
                settings.discovery_settings.source_regions.clone(),
            )
        })
    };
    let Some((discovery, source_regions)) = settings else {
        return;
    };
    if discovery != DISCOVERY_ENABLED {
        return;
    }
```
The pruning claim the snapshot cannot satisfy — `src/state/discovery.rs:186-218`:
```
    let discovered_regions: BTreeSet<&String> = source_regions.iter().collect();
    for region in &discovered_regions {
...
    // Discovery holds data for the Regions it discovers and no others, so a Region the
    // settings no longer name is dropped — including one dropped while this pass was
    // reading another.
    let retained: Vec<&str> = discovered_regions
        .iter()
        .map(|region| region.as_str())
        .collect();
    state.lock().retain_discovered_regions(&retained);
```
The awaits that make the window real — `src/state/discovery.rs:192` (`discovered_in_region(...).await`), `src/state/discovery.rs:270-274` (`client.describe_instances()...send().await`) and `src/state/discovery.rs:470` (`client.describe_images().filters(by_id).send().await`).

The non-enabled rule the in-flight pass breaks — `src/state/discovery.rs:149-151`:
```
/// Nothing happens for an account whose `LinuxSubscriptionsDiscovery` is not enabled: there
/// is no discovery to run, so the store is left exactly as it is rather than being filled
/// from an inventory the account did not ask to have discovered.
```
The writer `UpdateServiceSettings`, which replaces the whole singleton under its own short lock — `src/ops/update_service_settings.rs:211-221`:
```
        state.service_settings = Some(crate::state::ServiceSettingsRecord {
            linux_subscriptions_discovery: input.linux_subscriptions_discovery.clone(),
            discovery_settings: input.linux_subscriptions_discovery_settings.clone(),
```
Test coverage: `tests/all/discovery_from_ec2.rs:613` (`discovery_covers_exactly_the_configured_source_regions`) pins pruning when a Region leaves `SourceRegions` BETWEEN passes, and `tests/all/discovery_from_ec2.rs:719` (`nothing_is_discovered_until_the_account_enables_discovery`) pins the non-enabled rule before any pass runs; no test changes the settings while a pass is in flight, so neither stale-snapshot path is covered.

## Steps to Reproduce

1. Mount this service beside EC2 on one engine as `tests/all/discovery_from_ec2.rs` does, and POST /subscription/UpdateServiceSettings with {"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["us-west-2","us-east-1"]}}.
2. Launch instances from AMIs that bill Linux subscriptions in BOTH us-west-2 and us-east-1, then POST /subscription/ListLinuxSubscriptionInstances once so both Regions are in the store.
3. Issue a first-page POST /subscription/ListLinuxSubscriptionInstances (empty body) so a pass starts and snapshots the settings at src/state/discovery.rs:165-173.
4. While that pass is between its per-Region hops, POST /subscription/UpdateServiceSettings with SourceRegions reduced to ["us-west-2"] (AllowUpdate true).
5. The in-flight pass finishes and calls `retain_discovered_regions` with the stale snapshot ["us-east-1","us-west-2"], so the us-east-1 records are KEPT — contradicting the comment at src/state/discovery.rs:211-213 that a Region "dropped while this pass was reading another" is dropped.
6. Repeat the experiment but have step 4 send {"LinuxSubscriptionsDiscovery":"Disabled"} instead: the in-flight pass still writes through `record_discovery_pass` / `retain_discovered_regions`, so the store is filled and pruned for an account whose discovery is disabled — contradicting src/state/discovery.rs:149-151.

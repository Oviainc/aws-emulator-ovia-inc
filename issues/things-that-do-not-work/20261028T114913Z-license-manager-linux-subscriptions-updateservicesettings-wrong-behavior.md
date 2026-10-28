Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# Disabling LinuxSubscriptionsDiscovery leaves the discovered-instance store intact, so both discovery lists keep reporting an inventory nothing refreshes

_Filed: 2026-10-28T11:49:13Z_
_Operation: UpdateServiceSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateServiceSettings` persists `LinuxSubscriptionsDiscovery: Disabled` without touching `discovered_instances`, and the discovery pass the two list operations materialize returns EARLY for a non-enabled account. The combination means the records of the last pass made while discovery was enabled stay in the store forever and keep being reported by `ListLinuxSubscriptionInstances` and `ListLinuxSubscriptions` — including instances that have since been stopped or terminated, which no later pass can ever correct because no pass runs.

That contradicts the crate's own rule for discovered data nothing refreshes any more: `retain_discovered_regions` drops what was found in a Region the settings no longer name precisely so the data does not stay 'in the store as an inventory nothing refreshes', and that pruning call sits AFTER the early return, so it is unreachable once discovery is disabled. It also makes the answer depend on history rather than on the setting: the crate's own end-to-end test asserts that an account whose discovery is not enabled reports nothing from both lists, while an account that enabled and then disabled discovery — whose `LinuxSubscriptionsDiscovery` reads exactly the same `Disabled` — reports the full stale fleet.

Nothing in the crate states a reason for keeping the records: the only comment on the early return justifies not FILLING the store ('rather than being filled from an inventory the account did not ask to have discovered'), not keeping what is already there.

## Evidence

The early return, which also skips the pruning call below it (src/state/discovery.rs:174-179, 211-218):
```
    let Some((discovery, source_regions)) = settings else {
        return;
    };
    if discovery != DISCOVERY_ENABLED {
        return;
    }
...
    // Discovery holds data for the Regions it discovers and no others, so a Region the
    // settings no longer name is dropped — including one dropped while this pass was
    // reading another.
    let retained: Vec<&str> = ...
    state.lock().retain_discovered_regions(&retained);
```
The crate's own rule for data nothing refreshes (src/state.rs:495-504):
```
    /// Drop what was discovered in a Region this account no longer discovers.
    ///
    /// `discovered_regions` are the source Regions the settings name, so discovery holds
    /// data for exactly those: a Region taken out of `SourceRegions` is one nothing reads
    /// any more, and the instances a previous pass found there stop being reported rather
    /// than staying in the store as an inventory nothing refreshes.
    pub fn retain_discovered_regions(&mut self, discovered_regions: &[&str]) {
```
The handler that stores the disable without touching the store (src/ops/update_service_settings.rs:211-221):
```
        state.service_settings = Some(crate::state::ServiceSettingsRecord {
            linux_subscriptions_discovery: input.linux_subscriptions_discovery.clone(),
            discovery_settings: input.linux_subscriptions_discovery_settings.clone(),
```
Test coverage: `tests/all/discovery_from_ec2.rs:718-733` pins the OTHER half of the rule — 'an account that did not enable discovery discovers nothing' — and `tests/all/discovery_from_ec2.rs:613-650` pins pruning when a Region leaves `SourceRegions`. No test covers enable → discover → disable → list, so the stale report is untested; `tests/all/error_taxonomy.rs:87-89` and `tests/all/service_settings_status.rs:148` walk enable/disable sequences but assert only on `Status`.

## Steps to Reproduce

1. POST /subscription/UpdateServiceSettings with {"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["us-west-2"]}} the way tests/all/discovery_from_ec2.rs::enable_discovery does.
2. Launch an EC2 instance from an AMI that bills a Linux subscription in us-west-2 and POST /subscription/ListLinuxSubscriptionInstances: the instance is reported.
3. POST /subscription/UpdateServiceSettings again with the same settings but {"LinuxSubscriptionsDiscovery":"Disabled"}.
4. POST /subscription/ListLinuxSubscriptionInstances and /subscription/ListLinuxSubscriptions again: both still report the instance and its subscription, although discovery is disabled and no pass runs (src/state/discovery.rs:177-179).
5. Terminate the instance through EC2 and repeat the two list calls: the terminated instance is still reported, and no later call can ever correct it.
6. Compare with a fresh account that never enabled discovery (tests/all/discovery_from_ec2.rs:718-733): both lists are empty for the same `LinuxSubscriptionsDiscovery: Disabled` value.

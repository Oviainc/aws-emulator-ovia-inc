Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# Disabling LinuxSubscriptionsDiscovery leaves both discovery lists reporting a stale inventory nothing refreshes

_Filed: 2026-10-28T15:05:56Z_
_Operation: ListLinuxSubscriptions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::discovery::refresh_discovered_instances` is the only writer of the discovered-instance store, and both `ListLinuxSubscriptions` (list_linux_subscriptions.rs:178) and `ListLinuxSubscriptionInstances` (list_linux_subscription_instances.rs:177) materialize it. When the account's `LinuxSubscriptionsDiscovery` is NOT `Enabled`, the pass returns immediately — before the pruning step `retain_discovered_regions` — so every record a previous pass wrote while discovery was on stays in the store and keeps being reported by both lists forever. Nothing can ever refresh or remove those records while discovery stays disabled: terminating or stopping the discovered instances in EC2 changes nothing, and `GetServiceSettings` meanwhile truthfully answers `LinuxSubscriptionsDiscovery: Disabled`. That is exactly the situation the crate's own state module says must not arise — it prunes a region taken out of `SourceRegions` precisely so records do not sit in the store "as an inventory nothing refreshes" — so the same rule is applied when the region set shrinks but not when discovery is switched off entirely (which removes every region from discovery). The handler doc for the list also claims "Nothing is recorded for an account that did not enable discovery", which holds only for an account that NEVER enabled it, not for one that turned it off.

## Evidence

`src/state/discovery.rs:174-179` (the disabled path returns before any pruning):
```
    let Some((discovery, source_regions)) = settings else {
        return;
    };
    if discovery != DISCOVERY_ENABLED {
        return;
    }
```
versus the rule the same crate states for data discovery no longer covers, `src/state.rs:495-503`:
```
    /// Drop what was discovered in a Region this account no longer discovers.
    ///
    /// `discovered_regions` are the source Regions the settings name, so discovery holds
    /// data for exactly those: a Region taken out of `SourceRegions` is one nothing reads
    /// any more, and the instances a previous pass found there stop being reported rather
    /// than staying in the store as an inventory nothing refreshes.
    pub fn retain_discovered_regions(&mut self, discovered_regions: &[&str]) {
```
`retain_discovered_regions` is reached only at `src/state/discovery.rs:218`, after the `DISCOVERY_ENABLED` guard above, so a disable never reaches it.

The list handler's own doc, `src/ops/list_linux_subscriptions.rs:165-170`: "Discovery is what fills the store this list aggregates ... Nothing is recorded for an account that did not enable discovery, and a pass over an unchanged inventory records what is already there."

Test coverage: `tests/all/discovery_from_ec2.rs:719` (`nothing_is_discovered_until_the_account_enables_discovery`) covers only an account that NEVER enabled discovery; `tests/all/discovery_from_ec2.rs:613` (`discovery_covers_exactly_the_configured_source_regions`) covers shrinking `SourceRegions` while discovery stays `Enabled`. No suite drives `UpdateServiceSettings` back to `Disabled` after instances were discovered.

## Steps to Reproduce

1. Mount this service beside EC2 on one engine (as `tests/all/discovery_from_ec2.rs` does) and `POST /subscription/UpdateServiceSettings` with `LinuxSubscriptionsDiscovery: "Enabled"` and `SourceRegions: ["us-west-2"]`.
2. Launch an instance from an AMI whose `PlatformDetails` is `Red Hat Enterprise Linux`, then `POST /subscription/ListLinuxSubscriptions` and `POST /subscription/ListLinuxSubscriptionInstances` — the subscription and the instance are reported.
3. `POST /subscription/UpdateServiceSettings` with `LinuxSubscriptionsDiscovery: "Disabled"` (same discovery settings). `POST /subscription/GetServiceSettings` now reports `Disabled`.
4. Terminate the instance in EC2, then call `ListLinuxSubscriptions` / `ListLinuxSubscriptionInstances` again: both still report the subscription and the instance from the last pass, and will keep doing so indefinitely — the store holds an inventory nothing refreshes, which `src/state.rs:499-500` states must not happen.

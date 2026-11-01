Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Linux subscription discovery never reads the EC2 inventory, so both discovery lists are permanently empty

_Filed: 2026-10-24T01:03:15Z_
_Operation: ListLinuxSubscriptionInstances | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListLinuxSubscriptionInstances` (and `ListLinuxSubscriptions`, which aggregates the same records) answer from `state.discovered_instances`, a store that NO request path ever writes. There is no discovery pass that reads the account's EC2 instance inventory in the configured `LinuxSubscriptionsDiscoverySettings.SourceRegions`, so for every wire client the two lists return `{"Instances": []}` / `{"Subscriptions": []}` forever, even after `UpdateServiceSettings` enabled discovery and RHEL/SUSE instances are running in the co-hosted EC2 emulator.

Expected: once `LinuxSubscriptionsDiscovery` is `Enabled` for the account, discovery populates one `Instance` per running EC2 instance in the source Regions (AmiId, InstanceID, InstanceType, AccountID, Status, Region, UsageOperation, ProductCode, LastUpdatedTime, SubscriptionName), and `ListLinuxSubscriptions` reports one `Subscription` per distinct subscription name/type with `InstanceCount` counting the running instances. Filters and pagination then operate over that real inventory.

Actual: the records exist only as a test fixture — `tests/all/linux_subscriptions.rs::discover` inserts them straight into the store through `engine.state::<…>()`, and the state doc comment states "Discovery is the only writer … nothing in THIS service's API registers an instance" while no discovery is implemented. Secondary symptom of the same gap: neither list consults the persisted settings at all, so seeded records are reported even when `LinuxSubscriptionsDiscovery` is `Disabled` and regardless of whether the instance's Region is one of the configured `SourceRegions`.

## Evidence

Smithy model: `Instance` members are EC2 inventory facts (`AmiId`, `InstanceID`, `InstanceType`, `AccountID`, `Status`, `Region`, `UsageOperation`, `ProductCode`), documented as "Details discovered information about a running instance using Linux subscriptions"; `LinuxSubscriptionsDiscoverySettings.SourceRegions` is "The Regions in which to discover data for Linux subscriptions" and `LinuxSubscriptionsDiscovery` is the Enabled/Disabled switch — i.e. the service's documented job is to discover EC2 instances and report their Linux subscriptions. `Subscription$InstanceCount` is "The total amount of running instances using this subscription".

Handlers: `src/ops/list_linux_subscription_instances.rs` reads `state.discovered_instances_in_order()`; `src/ops/list_linux_subscriptions.rs` reads `state.discovered_subscriptions()`. `src/state.rs` (`discovered_instances`, `DiscoveredInstanceRecord`) is written by nothing in `src/` — grep shows no writer outside the test fixture.

Test coverage: `tests/all/linux_subscriptions.rs` asserts the projection/filter/pagination behaviour over fixtures it inserts directly (`fn discover`), and `an_account_that_discovered_nothing_reports_nothing` pins the empty result; no test covers discovery of EC2 state, and nothing asserts the discovery switch or SourceRegions gate the lists. Cross-service state access is already an established pattern in this repo (e.g. the `evs` service crate depends on the `ec2` service crate), so the wiring is available.

## Steps to Reproduce

1. Launch an EC2 instance in the co-hosted EC2 emulator (e.g. a RHEL AMI) in us-east-1.
2. Call POST /subscription/UpdateServiceSettings with {"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"OrganizationIntegration":"Disabled","SourceRegions":["us-east-1"]}} and poll GetServiceSettings until Status is Completed.
3. Call POST /subscription/ListLinuxSubscriptionInstances with an empty body — observe {"Instances":[]} instead of the discovered instance.
4. Call POST /subscription/ListLinuxSubscriptions — observe {"Subscriptions":[]} instead of a Subscription row with InstanceCount 1.
5. Then set LinuxSubscriptionsDiscovery back to "Disabled" and note that any records present in the store are still listed: the lists never consult the discovery setting or the configured SourceRegions.

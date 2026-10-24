Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# ListLinuxSubscriptions always reports no subscriptions because nothing feeds the discovered-instance store

_Filed: 2026-10-24T03:43:12Z_
_Operation: ListLinuxSubscriptions | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: with discovery enabled, `ListLinuxSubscriptions` aggregates the discovered instances into one `Subscription` per (Name, Type) pair and reports `InstanceCount` — "The total amount of running instances using this subscription" — so a client that has RHEL/SUSE/Ubuntu instances in a configured source Region sees at least one subscription with a non-zero count.

Actual: the handler aggregates `state.discovered_instances`, which no production code path ever populates (only test code writes it directly). Every call over HTTP answers `{"Subscriptions": []}` no matter what the account holds or what `UpdateServiceSettings` enabled, so the service's headline read is permanently empty and `InstanceCount` can never be exercised by a client.

## Evidence

Spec: `ListLinuxSubscriptions` returns `Subscriptions` (list<Subscription>) with `Subscription$InstanceCount` documented as "The total amount of running instances using this subscription". Handler: `src/ops/list_linux_subscriptions.rs:165` calls `state.discovered_subscriptions()`, which derives from `discovered_instances` (`src/state.rs:412`); that map has no writer outside `tests/all/linux_subscriptions.rs:207`. Same root cause as the ListLinuxSubscriptionInstances discovery gap, but separately observable on this operation. Tests: `tests/all/linux_subscriptions.rs:375` seeds state directly, so nothing asserts an API-driven path.

## Steps to Reproduce

1. POST /subscription/UpdateServiceSettings enabling discovery for us-east-1
2. Create Linux instances through the EC2 emulator in us-east-1
3. POST /subscription/ListLinuxSubscriptions with {} — observe `Subscriptions` is empty instead of an aggregated subscription with a non-zero `InstanceCount`

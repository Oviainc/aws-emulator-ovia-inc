Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9e2d684fc826ee70e56effd17777b7bc5b4fec7

# GetServiceSettings invents a settled Status (Completed) for an account that never applied a settings change

_Filed: 2026-10-27T18:08:15Z_
_Operation: GetServiceSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9e2d684fc826ee70e56effd17777b7bc5b4fec7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`reported_service_settings` has a branch for an account with no persisted settings singleton. Its own doc says what that default is (discovery `Disabled`, no discovery settings, no home Region) and states the rule the whole mapping follows: a member the account has nothing for is `None` rather than an invented value. The code in that branch nevertheless fills in `Status: Completed` — a value the doc's enumeration of the default does not mention, and which the constant itself defines as the status of an enable/disable that HAS been applied. So `GetServiceSettings` on a brand-new account reports that a Linux-subscriptions settings change has completed when no change was ever requested: the same constant's doc says `Completed` "is the value every client's onboarding poll waits for", so a client that polls GetServiceSettings for the settled status before expecting discovery data is told onboarding is done on an account where discovery is `Disabled` and nothing has been configured. The three other members of the same branch correctly report absence (`discovery_settings`, `status_message`, `home_regions` are all `None`), which is what makes this one value inconsistent with its own neighbours and with the documented rule above them.

## Evidence

The rule the mapping states for itself — src/state.rs:631-635:
```
/// What `GetServiceSettings` and `UpdateServiceSettings` report for this account.
///
/// Both operations answer with the same five members, so the mapping from the stored
/// singleton onto them lives here once. A member the account has nothing for is `None`
/// rather than an invented value.
```
The default branch — src/state.rs:650-663:
```
/// The settings to report for this account: the persisted singleton, or the default an
/// account that never called `UpdateServiceSettings` has — discovery `Disabled`, with no
/// discovery settings and no home Region, since nothing has been configured.
pub fn reported_service_settings(…) {
    let Some(settings) = settings else {
        return ReportedServiceSettings {
            linux_subscriptions_discovery: Some(DISCOVERY_DISABLED.to_string()),
            discovery_settings: None,
            status: Some(SETTINGS_STATUS_COMPLETED.to_string()),   // <- line 660
            status_message: None,
            home_regions: None,
        };
```
What that constant means, per its own doc — src/state.rs:61-70: "The `Status` a settled change to the Linux subscriptions service settings reports … `Completed` is the value the recorded service reports once an enable or a disable of `LinuxSubscriptionsDiscovery` has been applied, and it is the value every client's onboarding poll waits for. The settings singleton is written before the call that changed it returns, so the change is already applied". No change has been applied in this branch — there is no singleton at all.

The handler that serialises it: src/ops/get_service_settings.rs:152-161 (`// A pure read of the settings singleton` … `status: reported.status`).

Test coverage: no test pins the pre-update `Status`. tests/all/subscription_providers.rs:254-267 ("settings before any update") asserts only `LinuxSubscriptionsDiscovery == "Disabled"` and that `LinuxSubscriptionsDiscoverySettings`/`HomeRegions` are absent; tests/all/error_taxonomy.rs:84-118 checks `Status == "Completed"` only after an `UpdateServiceSettings` call applied a change.

## Steps to Reproduce

1. Start the emulator with an empty store (no `UpdateServiceSettings` call ever made).
2. POST `/subscription/GetServiceSettings` with `{}`.
3. Observe the body reports `"Status": "Completed"` alongside `"LinuxSubscriptionsDiscovery": "Disabled"` and no `LinuxSubscriptionsDiscoverySettings`/`HomeRegions` — a settled-change status for an account where, per src/state.rs:650-652, "nothing has been configured" and, per src/state.rs:634, a member the account has nothing for must be absent rather than invented.

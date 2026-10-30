Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# CreateConfigurationManager skips store.settle(), so it refuses creates against managers and configurations the clock has already removed

_Filed: 2026-10-27T22:22:05Z_
_Operation: CreateConfigurationManager | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate that takes the store lock calls `store.settle(now)` as its first act, because that is the one place a teardown the clock has made due is materialised (the manager and the configurations it owns are removed). `CreateConfigurationManager` is the only handler that takes the lock and never calls it, and then immediately reads two counts out of the un-settled store:

* `store.managers.len() >= crate::state::MAX_CONFIGURATION_MANAGERS` -> 409 `ConflictException`;
* `crate::state::configuration_capacity_rejection(store.configurations.len(), planned_deployments)` -> 409 `ConflictException`.

Both counts include records whose removal the clock already owes: a manager whose `deletes_at` has passed, plus every configuration it owns, is still in `store.managers` / `store.configurations` until some operation calls `settle`. So a create is refused with `ConflictException` because of resources that are, by this crate's own clock model, already gone. Worse, the refusal is not stable: because every OTHER operation (including the read-only listings) settles on entry, issuing any unrelated `ListConfigurationManagers`/`GetConfigurationManager` call first makes the very same create succeed. The same request therefore succeeds or fails depending on whether an unrelated read happened to run in between.

This is not a complaint about the value of either bound (both are documented emulator resource guards and stay exactly as they are). The defect is that create evaluates them against state the crate's own `settle` helper would have already reclaimed, while every sibling path calls that helper for exactly this reason.

## Evidence

The path that is missing the call -- services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/create_configuration_manager.rs:327-357 (no `settle` anywhere in the file):
```
        let mut store = store_guard_elided; // line 327: let mut store = state.lock();

        // The store is finite: a create that would take it past its cap is refused
        // rather than growing it, the way the service refuses one past its quota.
        if store.managers.len() >= crate::state::MAX_CONFIGURATION_MANAGERS {
```
and, same file, lines 347-357:
```
        let planned_deployments: usize = input
            .configuration_definitions
            ...
        if let Some(why) = crate::state::configuration_capacity_rejection(
            store.configurations.len(),
            planned_deployments,
        ) {
```

Every sibling handler calls it, with a comment saying why -- services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/delete_configuration_manager.rs:182-186:
```
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // So a manager whose teardown has already come due is gone before this
        // request looks for it, rather than being deleted a second time.
        store.settle(now);
```
services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/tag_resource.rs:216-220:
```
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // So a manager whose teardown has come due is gone before this request looks
        // for it, rather than being tagged on its way out of the store.
        store.settle(now);
```
(the same three lines appear in untag_resource.rs:217-221, update_configuration_manager.rs:181-185, update_configuration_definition.rs:258-262, get_configuration.rs:178-179, get_configuration_manager.rs:172-173, list_configuration_managers.rs:195-196, list_configurations.rs:196-197, list_tags_for_resource.rs:160-163.)

What the skipped helper does -- services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1316-1330:
```
    /// Materialise every status change the clock already owes, so a reporting
    /// operation answers with what is due at `now` rather than with what the last
    /// mutating call left behind.
    pub fn settle(&mut self, now: SystemTime) {
        // A teardown the clock has made due is the removal of the manager and of the
        // configurations it owns: owed at this instant whether or not anybody called.
        let torn_down: Vec<String> = self
            .managers
            .iter()
            .filter(|(_, manager)| manager.deletes_at.is_some_and(|due_at| due_at <= now))
            ...
        for manager_arn in &torn_down {
            self.remove_manager(manager_arn);
        }
```
and state.rs:80-86: "The instant each phase comes due is stored on the summary ... -- or, for a teardown, on the manager ([`ConfigurationManagerRecord::deletes_at`]) -- and materialised by [`ComAmazonawsSsmquicksetupQuicksetupState::settle`]; nothing sleeps."

Test coverage of the bounds that must keep passing: tests/all/deployment_targets.rs:812 `a_create_whose_definitions_outgrow_the_store_is_refused` (builds a store of exactly `MAX_CONFIGURATIONS` configurations via `definitions_per_store = MAX_CONFIGURATIONS / MAX_DEPLOYMENT_TARGETS`) and tests/all/deployment_targets.rs:955 `an_update_whose_targets_outgrow_the_store_is_refused`. Neither deletes a manager, so neither covers the stale-count path.

## Steps to Reproduce

1. Boot the emulator and sign every request for one Region (the handlers read the Region from the SigV4 credential scope).
2. Fill the configuration store to its bound exactly as tests/all/deployment_targets.rs:812 does: POST /configurationManager with `MAX_CONFIGURATIONS / MAX_DEPLOYMENT_TARGETS` (= 20) configuration definitions, each whose Parameters name `MAX_DEPLOYMENT_TARGETS` (= 100) TargetRegions, so 2000 configurations are deployed.
3. DELETE /configurationManager/{ManagerArn} for that manager (200, teardown accepted).
4. Sleep longer than `crate::state::TRANSITION_SPAN`, so the teardown is due and the manager plus its 2000 configurations are owed removal. Do NOT call any other operation.
5. POST /configurationManager again with a single definition: the create is refused 409 ConflictException ("this request deploys N configurations, more than this service can hold beside the 2000 it already deploys"), because create never settled the store and still counts the deleted manager's configurations.
6. Now POST /listConfigurationManagers (or GET any manager) -- that handler calls `store.settle(now)` and reclaims the torn-down records -- and repeat the identical create: it now answers 200. The same request's outcome depends on whether an unrelated read ran first.
7. Same shape at the manager bound: fill to MAX_CONFIGURATION_MANAGERS, delete one, wait out TRANSITION_SPAN, and a create is refused until some other operation settles the store.

## Resolution

`store.settle(created_at)` is now the first act `CreateConfigurationManager` takes after the store lock, using the same instant the create stamps its records with, exactly as the eleven sibling handlers that reach managers and configurations do. Both bounds and both refusal messages are unchanged; only the state they are measured against is now the state the clock owes, so the same request no longer succeeds or fails depending on whether an unrelated read ran first. Covered end-to-end by `tests/all/store_bounds_after_teardown.rs`, a table over both bounds (manager count, configuration count) x what became of one manager (nothing deleted, teardown in flight, teardown due), each due-teardown row also run with an unrelated listing in between.

### Note on the Cargo.lock change that rides with this fix

No dependency is being added or upgraded by this fix, and the service crate's `Cargo.toml` is untouched. The lockfile at the integration tip no longer described the workspace's own vendored manifests, so every `--locked` build of the merged tree failed with "cannot update the lock file ... because --locked was passed" before any test could run. Running `cargo update --workspace` reported "Locking 0 packages to latest compatible versions": no package version moved, no `[[package]]` entry was added or removed, and no checksum changed. The whole delta is four dependency-edge lines that bring the lockfile back in line with declarations that were already in the read-only vendor tree — `vendor/emulator-telemetry/Cargo.toml` declaring `libc = "0.2"` and `windows-sys = "0.61"` per target (both already present in the lockfile as pinned, checksummed entries), and `vendor/server/Cargo.toml` declaring the two internal path crates. The authored service crate keeps `#![forbid(unsafe_code)]`.

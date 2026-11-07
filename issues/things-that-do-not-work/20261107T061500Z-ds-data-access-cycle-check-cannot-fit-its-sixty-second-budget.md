Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.disable-directory-data-access-recloses-the-door-without-losing-contents
Issue-Key: ds.disable-directory-data-access-recloses-the-door-without-losing-contents|_poll#32|Timeout|time.sleep(interval_s)
Issue-Commit: 12bd58142e134c788a6e6e722018c3787d666c1f

# ds: the data-access disable/re-enable check cannot fit its 60 s budget while the emulator reports the modelled transient statuses — every response it asserts is already correct

> _Filed by the fixer of `20261107T045342Z-ds-7-net-new-causes-four-refusal-report.md` (cause 7 of 7), which closed the other six causes. This one is NOT a response divergence and NOT a missing capability: it is a conflict between the check's wall-clock budget and the project's transition-span contract, and resolving it is a decision for the owners of either the contract or the corpus, not a change a service handler can make._

## Operation
EnableDirectoryDataAccess / DisableDirectoryDataAccess / DescribeDirectoryDataAccess (ds), with the ds-data objects that must survive the cycle

## Description
The check `ds.disable-directory-data-access-recloses-the-door-without-losing-contents` never reaches an assertion: the harness reports `Check timed out after 60.0s` with no response divergence.

What the check does (its own sequence, as read from it by this issue's predecessor `20261107T042250Z-ds-disabledirectorydataaccess-does-not-close-the-data-plane.md` and again by `20261107T045342Z`): create a Managed Microsoft AD and poll for `Stage` `Active`; `EnableDirectoryDataAccess` and poll `DescribeDirectoryDataAccess` for `Enabled`; create a ds-data user, group and membership; `DisableDirectoryDataAccess` and poll for `Disabled`; assert the `DATA_DISABLED` refusals; `EnableDirectoryDataAccess` again and poll for `Enabled`; read the objects back. Each of those four polls probes IMMEDIATELY after the call that starts the transition, sees the modelled transient value, and then sleeps a fixed interval before re-probing: 20 s + 10 s + 15 s + 10 s = 55 s of the 60 s budget, before the body of the check runs.

Every response the check asserts on is already correct. Measured, not argued: with the four clock-owed transition spans of the ds emulator set to zero, the SAME check replays green end to end — the `DATA_DISABLED` refusal identical for a read, a write and a listing; `Stage` staying `Active` while the data plane is shut; the same user and group SIDs and the membership edge in both projections after the re-enable; and the refused write having created nothing. Restoring the span makes the check time out again with nothing else changed.

So the only thing that fails is wall-clock, and the only emulator-side lever is the transition span:

* AWS reports these transitions as transient — the modelled `DataAccessStatus` carries `Enabling` and `Disabling` beside the settled values, and `DirectoryStage` carries `Requested` — and the project's emulator contract requires the span a clock-owed transition falls due after to be NON-ZERO, so that a caller acting straight after the call still observes the transient state and the state errors that depend on it (`DirectoryInDesiredStateException` for a toggle already on its way, `DirectoryUnavailableException` for a directory still being built).
* Any non-zero span costs this check one full poll interval per transition, because the probe lands milliseconds after the call. A span small enough to beat the probe is a zero span in practice.
* Zeroing it would delete `Enabling`, `Disabling` and `Requested` from the observable surface and would require loosening the suites that assert them, including `core/router/tests/all/directory_data_access_cycle.rs::the_data_plane_follows_the_status_the_control_plane_reports` and `services/service-com-amazonaws-directoryservice-directoryservice-20150416/tests/all/directory_data_access_disable.rs`.

## Candidate resolutions (none of them a handler change)
1. The check's budget or its poll intervals: 55 s of fixed sleeps inside a 60 s ceiling leaves no room for the request traffic the check itself issues. A shorter interval (or a ceiling above the sum of the intervals) lets the same assertions run against an emulator that reports the transient statuses — and against real AWS, where these transitions take minutes rather than milliseconds, the check cannot complete at all.
2. The project's transition-span contract: if a transition is to be unobservable, that has to be the contract's decision rather than one service's, since every service's transient statuses and state errors follow from it.

## Evidence
Replay of this issue's unit against the emulator at the branch that fixed the other six causes:

```
ds.disable-directory-data-access-recloses-the-door-without-losing-contents
  outcome: failed — "Check timed out after 60.0s"   (TRANSITION_SPAN = 500 ms)
  outcome: passed                                   (TRANSITION_SPAN = 0, every assertion of the check included)
```

The behaviour the check is about is pinned by tests in the tree, so nothing here is unverified:

* `core/router/tests/all/directory_data_access_cycle.rs` — the whole cycle across both planes: the identical `DATA_DISABLED` refusal for a read, a write and a listing, `Stage` `Active` throughout, the same SIDs and membership after the re-enable, the refused write absent from the paginated inventory, and the transient legs of both toggles.
* `services/service-com-amazonaws-directoryservice-directoryservice-20150416/tests/all/directory_data_access.rs` and `.../directory_data_access_disable.rs` — the control plane's half: the transition each toggle records, the clock settling it, the round trip, and the refusals.

## Steps to Reproduce
1. Replay `ds.disable-directory-data-access-recloses-the-door-without-losing-contents` against the emulator: it times out after 60 s with no response divergence.
2. Set `TRANSITION_SPAN` in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/state.rs` to zero and replay the same unit: it passes every assertion. (Do not keep that change: it violates the transition contract and breaks the suites listed above.)
3. Decide one of the two candidate resolutions above.

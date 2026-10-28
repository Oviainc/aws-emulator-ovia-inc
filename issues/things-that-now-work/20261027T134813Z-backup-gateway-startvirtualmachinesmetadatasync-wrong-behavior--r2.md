Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# A metadata sync restarted before the previous one falls due publishes the inventory while the hypervisor reports no successful sync

_Filed: 2026-10-27T13:48:13Z_
_Operation: StartVirtualMachinesMetadataSync | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`start_metadata_sync` does two things with two different clocks and they disagree when a second sync is started while the first is still in flight.

For the hypervisor record it REPLACES the instant the sync progress is derived from (`metadata_sync_started = Some(now)`) and folds forward only a sync the clock had ALREADY finished (`last_successful_metadata_sync_time_as_of(now)`), so an unfinished first sync is abandoned: its success is discarded and the status restarts at `CREATED`.

For the virtual-machine inventory it does NOT re-date anything: `record_virtual_machine` finds each machine by `(hypervisor_arn, path)` and KEEPS the `discovered_time` the first sync gave it (`METADATA_SYNC_SUCCEEDS_AFTER` after the FIRST call). So the machines of the abandoned sync still become visible at the abandoned sync's due instant.

Result: between the first sync's due instant and the second sync's due instant, `ListVirtualMachines`/`GetVirtualMachine` report the hypervisor's three machines while `GetHypervisor` reports `LatestMetadataSyncStatus` `RUNNING` and NO `LastSuccessfulMetadataSyncTime` at all — i.e. the service publishes an inventory it simultaneously claims no sync has ever successfully produced. The crate's own state module states the opposite invariant in two places: the machines a listing reports and the `SUCCEEDED` a read of the hypervisor reports "become true at the same instant".

The inconsistency is in the store helper the handler calls (`crate::state::start_metadata_sync`), reached from `start_virtual_machines_metadata_sync.rs`.

## Evidence

The documented invariant, stated twice in this crate:

- `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:106-109` (module doc): "That one sync is also what records the hypervisor's virtual-machine inventory, dated at the instant the sync falls due — so the machines a listing reports and the `SUCCEEDED` a read of the hypervisor reports become true together."
- `src/state.rs:1668-1671` (`start_metadata_sync` doc): "records every machine the hypervisor reports ([`synced_inventory`]) through [`Self::record_virtual_machine`], dated at the instant the sync falls due — so the machines a listing reports and the [`SYNC_METADATA_SUCCEEDED`] a read of the hypervisor reports become true at the same instant".

The code that breaks it, in the same function:

- `src/state.rs:1676-1681`: "// A sync the clock had already finished is MATERIALISED before the new one / // replaces the instant its progress is derived from … so it never goes / // backwards or disappears because another sync was started." then `record.last_successful_metadata_sync_time = record.last_successful_metadata_sync_time_as_of(now);` and `record.metadata_sync_started = Some(now);` — for a first sync NOT yet due, `last_successful_metadata_sync_time_as_of` falls to the `_ =>` arm (`src/state.rs:814-820`) and yields `None`, so the pending success is dropped and the status restarts.
- `src/state.rs:1691-1694`: `let discovered = now + METADATA_SYNC_SUCCEEDS_AFTER; for machine in synced_inventory(arn) { self.record_virtual_machine(&region, arn, machine, discovered); }`
- `src/state.rs:1727-1748`: the `known` lookup matches `machine.hypervisor_arn == hypervisor_arn && machine.path == inventory.path` and returns `(machine.arn.clone(), machine.discovered_time)`, and `src/state.rs:1758` writes `discovered_time` back unchanged — documented at `src/state.rs:1709-1712` as "reporting the same path again UPDATES the record under the ARN the machine already has (keeping its `discovered_time`)". So the `discovered` value computed for the SECOND sync is thrown away and the abandoned first sync's instant stands.
- Read side: `src/state.rs:922-924` `discovered_as_of` = `now >= self.discovered_time`, used by `page_of_virtual_machines` (`src/state.rs:1811-1814`) and `virtual_machine_as_of` (`src/state.rs:1779-1786`), while the hypervisor read uses `metadata_sync_started` (`src/state.rs:790-821`).
- Handler that reaches it: `src/ops/start_virtual_machines_metadata_sync.rs:199-202` `let started = store.start_metadata_sync(&input.hypervisor_arn, ::std::time::SystemTime::now());`

Test coverage: `tests/all/virtual_machine_inventory_sync.rs:384-389` pins only the re-sync of an ALREADY-established inventory — "while the new sync is in flight the inventory the previous one established still stands" — and `tests/all/metadata_sync.rs:626-628` pins "a second sync restarts it: the status goes back to `CREATED` while the time of the sync that already succeeded stands". Neither exercises a second sync started BEFORE the first fell due, so this case is untested.

## Steps to Reproduce

1. Start the emulator and point a `backup-gateway` client at it.
2. ImportHypervisorConfiguration with Name="hv-one", Host="10.0.0.11" -> note the returned HypervisorArn.
3. Call StartVirtualMachinesMetadataSync with that HypervisorArn (call this instant T0). The handler dates the three discovered machines at T0 + METADATA_SYNC_SUCCEEDS_AFTER (500 ms).
4. Within 500 ms of T0, call StartVirtualMachinesMetadataSync again on the same HypervisorArn. This resets `metadata_sync_started` to the new instant and drops the first sync's pending success, but leaves each machine's `discovered_time` at T0 + 500 ms.
5. At roughly T0 + 500 ms (after the first sync's due instant, before the second's), call ListVirtualMachines with that HypervisorArn -> three machines are reported, and each is readable through GetVirtualMachine.
6. In the same window call GetHypervisor on that ARN -> `LatestMetadataSyncStatus` is `RUNNING` and `LastSuccessfulMetadataSyncTime` is absent, so the service reports an inventory while reporting that no metadata sync has ever succeeded — contradicting the invariant quoted at src/state.rs:106-109 and src/state.rs:1668-1671.

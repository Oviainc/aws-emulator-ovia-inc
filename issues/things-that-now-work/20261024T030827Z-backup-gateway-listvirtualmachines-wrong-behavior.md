Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# ListVirtualMachines is permanently empty: no API call ever records a hypervisor's virtual-machine inventory

_Filed: 2026-10-24T03:08:27Z_
_Operation: ListVirtualMachines | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a hypervisor's inventory becomes visible after its metadata is synced — ListVirtualMachines on a hypervisor with no sync returns an empty list (the behavior the recorded unit `backup-gateway.list-virtual-machines-empty-for-hypervisor-without-sync` captures), and after a successful StartVirtualMachinesMetadataSync (LatestMetadataSyncStatus = SUCCEEDED) the same call reports the hypervisor's virtual machines (Name, HostName, Path, HypervisorId, ResourceArn, LastBackupDate).

Actual: the only way a `StoredVirtualMachine` reaches the store is `state::record_virtual_machine`, an in-process helper called by nothing but the test suite; neither ImportHypervisorConfiguration nor StartVirtualMachinesMetadataSync records any inventory. So over the wire ListVirtualMachines returns `VirtualMachines: []` forever, at every page size and for every HypervisorArn filter, even after a sync reports SUCCEEDED. The listing can never report a machine, and no ARN exists for the sibling GetVirtualMachine/tagging paths to resolve.

## Evidence

Spec: ListVirtualMachines is @paginated over `VirtualMachines` (list<VirtualMachine>) and VirtualMachine declares Name/HostName/Path/HypervisorId/ResourceArn/LastBackupDate; StartVirtualMachinesMetadataSync is documented as syncing metadata across the hypervisor's virtual machines, and HypervisorDetails carries LatestMetadataSyncStatus/LastSuccessfulMetadataSyncTime, i.e. a sync is the service event that materialises inventory. Handler: src/ops/list_virtual_machines.rs reads `page_of_virtual_machines` only; src/state.rs documents `record_virtual_machine` as the single out-of-band channel and `start_metadata_sync` records only the start instant ("The sync itself discovers nothing here"). Tests: tests/all/virtual_machine_listing.rs seeds machines through `state::record_virtual_machine` directly, so no test exercises an HTTP-only path producing a non-empty listing.

## Steps to Reproduce

1. Boot the emulator and call ImportHypervisorConfiguration to mint a hypervisor.
2. Make StartVirtualMachinesMetadataSync succeed for that hypervisor and poll GetHypervisor until LatestMetadataSyncStatus is SUCCEEDED.
3. Call ListVirtualMachines with that HypervisorArn (and again with no filter).
4. Observe an empty VirtualMachines list and no NextToken; expected the hypervisor's synced virtual machines, each with ResourceArn/HypervisorId/Name/Path.

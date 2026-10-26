Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# StartVirtualMachinesMetadataSync invents a property-mappings precondition (403) and discovers no virtual machines

_Filed: 2026-10-24T08:44:27Z_
_Operation: StartVirtualMachinesMetadataSync | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two divergences in one handler:

1. Invented precondition. The handler refuses the sync with HTTP 403 AccessDeniedException / ErrorCode `MappingsNotConfigured` unless the hypervisor already carries property mappings. The spec documents the operation simply as "This action sends a request to sync metadata across the specified virtual machines", and AccessDeniedException is the generic insufficient-permissions error; VMware-to-AWS tag mappings (PutHypervisorPropertyMappings) are an optional feature, not a prerequisite for a metadata sync. Expected: ImportHypervisorConfiguration followed by StartVirtualMachinesMetadataSync returns HTTP 200 with the HypervisorArn and moves LatestMetadataSyncStatus along. Actual: 403 for every hypervisor. Because PutHypervisorPropertyMappings is still a scaffold (and the `property_mappings` field is otherwise only writable in-process from tests), the operation can NEVER succeed over the wire today.

   The same gate also mis-answers an unknown hypervisor ARN: the permissions check runs before the existence check, so an ARN the store holds no hypervisor for gets 403 AccessDeniedException instead of the modeled ResourceNotFoundException the operation declares for that case.

2. The sync discovers nothing. Even when the gate is passed, `start_metadata_sync` only stamps an instant and derives CREATED→RUNNING→SUCCEEDED from the clock; it records no virtual machines. Expected: a successful metadata sync populates the hypervisor's inventory, so a later ListVirtualMachines (and GetVirtualMachine) reports the hypervisor's VMs. Actual: ListVirtualMachines returns an empty VirtualMachines list for every client even after the sync reports SUCCEEDED, because the only writer of the virtual-machine map (`record_virtual_machine`) is an in-process Rust helper that nothing wire-reachable calls — so the service reports a successful sync with a permanently empty inventory.

## Evidence

Smithy model: StartVirtualMachinesMetadataSync takes only HypervisorArn and declares AccessDeniedException (403), ResourceNotFoundException (404), ValidationException, ThrottlingException, InternalServerException; its doc is "This action sends a request to sync metadata across the specified virtual machines" — nothing conditions it on property mappings (smithy_query operation/doc). AccessDeniedException is documented as "The operation cannot proceed because you have insufficient permissions." ListVirtualMachines is @paginated over VirtualMachines, i.e. the inventory a sync produces.
Handler: src/ops/start_virtual_machines_metadata_sync.rs lines ~177-187 (`if !store.property_mappings_configured(&input.hypervisor_arn) { return Err(AccessDeniedException{ErrorCode: MAPPINGS_NOT_CONFIGURED, ...}) }`) placed BEFORE the hypervisor existence check at lines ~195-211; src/state.rs `start_metadata_sync` (line ~932) writes only `metadata_sync_started`/status fields and explicitly "discovers nothing"; `record_virtual_machine` (line ~963) is reachable only from tests (repo grep shows callers only in tests/all/virtual_machine_listing.rs).
Tests: tests/all/metadata_sync.rs::a_sync_is_refused_when_no_mappings_are_configured and ::the_refusal_does_not_reveal_whether_the_arn_names_a_hypervisor assert the current (invented) 403 behavior and would need updating; no test asserts a successful sync for a freshly imported hypervisor or a non-empty inventory afterwards.

## Steps to Reproduce

1. Start the emulator and point a `backup-gateway` client at it.
2. ImportHypervisorConfiguration with Name="hv-one", Host="10.0.0.11", Username/Password set -> note the returned HypervisorArn.
3. Call StartVirtualMachinesMetadataSync with that HypervisorArn -> observe HTTP 403 AccessDeniedException with ErrorCode "MappingsNotConfigured" instead of HTTP 200 echoing HypervisorArn.
4. Call StartVirtualMachinesMetadataSync with a well-formed but never-imported hypervisor ARN -> observe the same 403 AccessDeniedException instead of the modeled ResourceNotFoundException.
5. (After the gate is corrected) call GetHypervisor until LatestMetadataSyncStatus is SUCCEEDED, then ListVirtualMachines with HypervisorArn set -> observe an empty VirtualMachines list, where a real sync makes the hypervisor's virtual machines listable.

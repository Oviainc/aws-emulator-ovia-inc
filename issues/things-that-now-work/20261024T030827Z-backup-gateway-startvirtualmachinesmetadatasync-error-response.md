Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# StartVirtualMachinesMetadataSync refuses every hypervisor with 403 AccessDeniedException unless property mappings are set, and can never answer ResourceNotFoundException

_Filed: 2026-10-24T03:08:27Z_
_Operation: StartVirtualMachinesMetadataSync | Class: error-response_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler gates the sync on the hypervisor carrying property mappings: `if !store.property_mappings_configured(&input.hypervisor_arn)` returns `AccessDeniedException` (HTTP 403, ErrorCode `MappingsNotConfigured`) BEFORE any existence lookup.

Expected (real service): `StartVirtualMachinesMetadataSync` syncs the virtual-machine metadata of an imported hypervisor. VMware-to-AWS tag property mappings (`PutHypervisorPropertyMappings`) are an optional tag-mapping feature, not a precondition of a metadata sync, so a hypervisor that was just imported and has no property mappings accepts the sync and answers 200 with its `HypervisorArn`. A well-formed `HypervisorArn` that names no hypervisor is refused with the modeled `ResourceNotFoundException` (this service answers a missing hypervisor with ErrorCode `HypervisorNotFound`, as `crate::state::MissingResource::Hypervisor` already encodes for GetHypervisor/DeleteHypervisor/TagResource).

Actual (emulator): BOTH cases answer HTTP 403 `AccessDeniedException` / `MappingsNotConfigured`. The handler's own comment admits the `ResourceNotFoundException` branch below the gate is unreachable ("with one guard spanning both this refusal cannot be reached"), so a modeled error of this operation can never be produced. Because `PutHypervisorPropertyMappings` is still a scaffold answering 501, there is NO API path that can set mappings, which means the operation can never succeed over the wire at all: the documented workflow ImportHypervisorConfiguration -> StartVirtualMachinesMetadataSync -> ListVirtualMachines / GetHypervisor(LatestMetadataSyncStatus) is unreachable for any client.

## Evidence

Spec: smithy_query(operation="StartVirtualMachinesMetadataSync") declares errors AccessDeniedException (403), ResourceNotFoundException (404) plus the service-common ones; nothing in the model or in the operation/PutHypervisorPropertyMappings docs makes property mappings a precondition of a metadata sync (mappings are documented as "the relationship of entity properties available from the on-premises hypervisor to the properties available in AWS", i.e. tag mapping). Handler: services/.../src/ops/start_virtual_machines_metadata_sync.rs lines ~177-211 (mappings gate first, then an unreachable missing-hypervisor branch). State: src/state.rs `property_mappings` is only writable in-process (PutHypervisorPropertyMappings is a 501 scaffold, see its own open stub issue), so no wire client can satisfy the gate. Tests: tests/all/metadata_sync.rs asserts the emulator's invented rule (`a_sync_is_refused_when_no_mappings_are_configured`, `the_refusal_does_not_reveal_whether_the_arn_names_a_hypervisor`) and its module docs state it as a chosen rule with no AWS recording cited — so the existing coverage enshrines the divergence rather than covering it.

## Steps to Reproduce

1. Boot the emulator and call ImportHypervisorConfiguration with Name/Host to mint a hypervisor; note the returned HypervisorArn.
2. Call StartVirtualMachinesMetadataSync with that HypervisorArn.
3. Observe HTTP 403 AccessDeniedException with ErrorCode 'MappingsNotConfigured'; expected HTTP 200 with the HypervisorArn, and a following GetHypervisor reporting LatestMetadataSyncStatus.
4. Call StartVirtualMachinesMetadataSync with a well-formed hypervisor ARN that was never imported.
5. Observe the same 403 AccessDeniedException; expected the modeled ResourceNotFoundException (ErrorCode 'HypervisorNotFound'), the refusal every other operation of this service gives for a hypervisor it holds no record for.

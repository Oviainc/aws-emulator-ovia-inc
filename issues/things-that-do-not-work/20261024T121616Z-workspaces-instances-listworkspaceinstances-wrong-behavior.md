Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 57d818e145087aac91ee9b85e99c9525e7c9e7c0

# WorkSpace Instances and volumes are not scoped to the request's Region: a resource created in one Region is listed, read, tagged and deleted from every other

_Filed: 2026-10-24T12:16:16Z_
_Operation: ListWorkspaceInstances | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `57d818e145087aac91ee9b85e99c9525e7c9e7c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

WorkSpaces Instances is a REGIONAL service: an instance created through the us-east-1 endpoint exists only in us-east-1, and the same call signed for another Region sees an account with no instances. The emulator holds one global store with no Region dimension at all, so every instance (and every volume) is visible and mutable from every Region.

Expected: ListWorkspaceInstances signed for eu-west-3 after a CreateWorkspaceInstance signed for us-east-1 reports an empty WorkspaceInstances list; GetWorkspaceInstance / ListTagsForResource / TagResource / DeleteWorkspaceInstance for that id from eu-west-3 answer the not-found refusal this service uses for an unknown wsinst- id; DeleteVolume for a volume created in us-east-1 answers not-found when signed for eu-west-3; and the running-instance quota is counted per Region.

Actual: ListWorkspaceInstances reports the us-east-1 instance to a eu-west-3 caller, Get/ListTagsForResource/TagResource/Delete all succeed cross-Region, and the quota counters are account-global. The same hole affects volumes: CreateVolume is the ONLY operation that consults the credential scope (it checks AvailabilityZone against the signed Region, so the volume's Region is known at create time) yet the VolumeRecord does not store it, so DeleteVolume from any Region deletes a volume that belongs to another Region.

A secondary symptom of the same missing Region dimension: a request signed for a Region this service is not available in (ListRegions deliberately excludes the GovCloud/China partitions and other unsupported Regions) is served normally — CreateVolume happily derives Availability Zones for cn-north-1 and creates a volume there.

## Evidence

Spec / well-established semantics: resources of a regional AWS service are scoped to the Region of the endpoint/credential scope they were created through; this service's own model publishes its Region availability set via ListRegions (Region/RegionName ^[-0-9a-z]{1,31}$) and its ResourceNotFoundException is the modeled refusal for an id the Region does not hold.

Handler/state: src/state.rs `ComAmazonawsWorkspacesinstancesEucmifrontendapiserviceState` holds `instances: BTreeMap<String, WorkspaceInstanceRecord>`, `volumes: BTreeMap<String, VolumeRecord>`, `client_tokens`, `volume_client_tokens` and `id_counter` with NO Region key, and `service_state()` reaches one global `InMemoryEngine::state::<...>()`. `WorkspaceInstanceRecord` (src/state.rs ~1255) stores no Region; `VolumeRecord` (~1348) stores only `availability_zone`. `request_region()` exists (src/state.rs ~1084) and its own doc says "A regional resource belongs to the Region its create call was signed for", but the only caller in the whole crate is src/ops/create_volume.rs:326 (Availability-Zone validation) — grep for `request_region` in src/ matches nothing else. src/ops/list_workspace_instances.rs:183-227 pages `state.instances` unconditionally; src/ops/get_workspace_instance.rs:170, src/ops/list_tags_for_resource.rs:154, src/ops/tag_resource.rs:159, src/ops/delete_workspace_instance.rs:162 and src/ops/delete_volume.rs:163 all look the id up in the single global map.

Tests: the harness already has the means to drive two Regions — tests/all/workspace_instance_lifecycle.rs exposes `call_in_region(router, region, ...)` and tests/all/volume_configuration_validation.rs uses it across us-east-1 / eu-west-3 / ap-northeast-2 — but it is used only for CreateVolume's Availability-Zone rule. No test asserts that an instance or volume created in one Region is invisible in another, so the cross-Region leak is uncovered.

## Steps to Reproduce

1. Call CreateWorkspaceInstance with {"ManagedInstance":{}} signed for us-east-1 (Credential=.../us-east-1/workspaces-instances/aws4_request) and keep the returned WorkspaceInstanceId
2. Call ListWorkspaceInstances signed for eu-west-3 — the us-east-1 instance is reported, where the real service reports an empty list
3. Call GetWorkspaceInstance and ListTagsForResource for that id signed for eu-west-3 — both answer 200 with the instance instead of the not-found refusal
4. Call TagResource and then DeleteWorkspaceInstance for that id signed for ap-northeast-2 — both succeed, mutating a resource that belongs to another Region
5. Call CreateVolume signed for us-east-1 with AvailabilityZone us-east-1a and SizeInGB 100, then DeleteVolume for the returned VolumeId signed for eu-west-3 — the delete succeeds instead of answering not-found

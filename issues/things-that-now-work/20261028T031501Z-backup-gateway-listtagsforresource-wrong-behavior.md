Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# Tagging operations report a virtual machine the store really holds as ResourceNotFoundException

_Filed: 2026-10-28T03:15:01Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tagging operations (`ListTagsForResource`, `TagResource`, `UntagResource`) resolve their ARN member through `crate::state`'s resource lookups, which consult only the `gateways` and `hypervisors` maps. The `virtual_machines` map — the store's documented THIRD resource — is never consulted, so an ARN naming a virtual machine the store really holds (one a metadata sync discovered, which `GetVirtualMachine` answers 200 for on the very same ARN) is reported as `ResourceNotFoundException` ("A resource that is required for the action wasn't found").

This contradicts the lookup's own doc comment, which says `None` is returned only "for an ARN this store holds no record for" and that the tagging operations "take any of this service's resources" through this one lookup. It also makes two sibling handlers disagree about the same ARN: `GetVirtualMachine` resolves it to a stored record, `ListTagsForResource` says it does not exist.

All three tagging members target the same Smithy shape `ResourceArn` that `GetVirtualMachine$ResourceArn` and `VirtualMachine$ResourceArn` target (gateway operations use `GatewayArn`, hypervisor operations use `ServerArn`), so the member the tagging operations take is the virtual-machine-shaped ARN of this model.

The three symptoms (list, tag, untag) are one defect in the shared lookups and collapse into this issue.

## Evidence

Model — all three tagging members and the virtual-machine members target ONE shape (`smithy_query(query="operation",operation="TagResource,ListTagsForResource,UntagResource,GetVirtualMachine")`):
  TagResource input: `"ResourceARN": {"target": "ResourceArn", ...}`
  ListTagsForResource input: `"ResourceArn": {"target": "ResourceArn", ...}`
  UntagResource input: `"ResourceARN": {"target": "ResourceArn", ...}`
  GetVirtualMachine input: `"ResourceArn": {"target": "ResourceArn", ...}`
`ResourceNotFoundException` doc: "A resource that is required for the action wasn't found."

Repo side 1 — the lookup's own contract, services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/state.rs:1817-1821:
```
/// The ARN `arn` is stored under and the tags the resource it names carries, or
/// `None` for an ARN this store holds no record for.
///
/// The tagging operations take any of this service's resources in one `ResourceARN`
/// member, so they resolve it through this one lookup rather than knowing which map
/// holds which kind
```

Repo side 2 — the code under that comment, state.rs:1824-1829:
```
pub fn resource_tags(&self, arn: &str) -> Option<(String, Vec<Tag>)> {
    if let Some(gateway) = self.gateways.get(arn) {
        return Some((gateway.arn.clone(), gateway.tag_list()));
    }
    let hypervisor = self.hypervisors.get(arn)?;
```
The same two-map-only resolution in `merge_resource_tags` (state.rs:1843-1849) and `remove_resource_tags` (state.rs:1865-1868).

Repo side 3 — the store does own virtual machines, state.rs:49-51 and state.rs:1156:
```
//! The third resource this store owns is the **virtual machine**
//! ([`StoredVirtualMachine`]), keyed by its own ARN.
```
```
virtual_machines: BTreeMap<String, StoredVirtualMachine>,
```

Handler that turns the `None` into the refusal — services/.../src/ops/list_tags_for_resource.rs:160-172:
```
let Some((resource_arn, tags)) = state.resource_tags(&input.resource_arn) else {
    let missing = crate::state::MissingResource::for_arn(&input.resource_arn);
```

Test coverage: the existing tests only ever use a virtual-machine ARN that was NEVER discovered, so the live-VM case is unpinned — tests/all/negative_paths.rs:238-244 (`"a virtual machine", arn_of("vm", "vm-1111111111111111"), 404`) and tests/all/resource_untagging.rs:370-375 (`"a virtual machine", arn_of("vm", "vm-2222222222222222"), 404`).

## Steps to Reproduce

1. Issue an activation key, `CreateGateway`, then `ImportHypervisorConfiguration` to mint a hypervisor.
2. Call `StartVirtualMachinesMetadataSync` for that hypervisor and wait for the sync to fall due.
3. Call `ListVirtualMachines` and take a `ResourceArn` from the reported inventory.
4. Call `GetVirtualMachine` with that ARN — it answers 200 with the machine's details, proving the store holds the record.
5. Call `ListTagsForResource` (and `TagResource`, `UntagResource`) with the SAME ARN — each answers `ResourceNotFoundException` instead of resolving the record the store holds.

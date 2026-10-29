Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# The synced inventory is derived from the hypervisor ARN's resource id alone, so hypervisors in two regions report machines with identical Name, HostName and Path

_Filed: 2026-10-29T04:13:22Z_
_Operation: StartVirtualMachinesMetadataSync | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`synced_inventory` builds the machine set a metadata sync publishes from `arn_resource_id(hypervisor_arn)` — the `hv-<16 hex>` suffix only — and discards the region segment of the ARN. The mint counter those ids come from is keyed per (resource kind, REGION) (`next_resource_ids: BTreeMap<(MintedResource, String), u64>`, `mint_resource` does `.entry((resource, region.clone())).or_default()`), so the FIRST hypervisor imported in every region gets the very same resource id `hv-0000000000000000`, the second `hv-0000000000000001`, and so on.

Consequence: the first hypervisor of `us-east-1` and the first hypervisor of `eu-central-1` are two different records whose syncs publish three machines each with the SAME `VirtualMachine$Name`, the same `HostName` and the same `Path` (`/Datacenter/vm/hv-0000000000000000-vm-1` …). The crate itself states the opposite rule twice, in the state module doc and on `synced_inventory`, and the state module also states that the ARN — not its resource-id suffix — is a record's only identity. The inventory generator uses only part of that identity, so the stated rule is false as soon as more than one region is addressed.

The records themselves stay distinct (the VM ARNs differ, and `record_virtual_machine` dedups on `(hypervisor_arn, path)`), so nothing is overwritten; what is wrong is the published inventory data: a client that identifies a machine the way this crate does — by its path in the hypervisor inventory — cannot tell two machines of two different hypervisors apart.

## Evidence

The rule the crate states for itself — `src/state.rs:74-78` (module doc):

    //! WHICH machines an inventory holds is this emulator's own answer ([`synced_inventory`]):
    //! it stands in for the on-premises hypervisor as much as for the service, and has no
    //! VMware host to enumerate, so a sync reports a bounded set of machines derived from the
    //! identity of the hypervisor record itself. Every sync of one record reports the same
    //! machines — which is what makes a re-sync idempotent — and no two records share one.

restated on the generator itself — `src/state.rs:1239-1244`:

    /// it has no VMware host to enumerate — so the inventory is derived from the IDENTITY of
    /// the hypervisor record itself: the resource id of the ARN this store minted for it,
    /// which no request chose. Two consequences, both of them the point: every sync of one
    /// hypervisor reports exactly the machines the previous sync did, so a re-sync re-finds
    /// each machine by its [`StoredVirtualMachine::path`] and updates the record it already
    /// has rather than minting a second one; and no two hypervisors report the same machine.

and what the crate says a record's identity actually is — `src/state.rs:46-47` (module doc):

    //! The ARN minted for a record is its ONLY identity: `Host`, `Name` and the rest are
    //! metadata the record carries, so any number of records may hold the same host

The code that uses only the suffix — `src/state.rs:1256-1267`:

    let hypervisor_id = arn_resource_id(hypervisor_arn).unwrap_or(HYPERVISOR_RESOURCE_TYPE);
    (1..=SYNCED_VIRTUAL_MACHINES)
        .map(|ordinal| {
            let name = synced_machine_name(hypervisor_id, ordinal);
            VirtualMachineInventory {
                path: format!("{SYNCED_INVENTORY_FOLDER}/{name}"),
                ...
                host_name: name.clone(),
                name,

(`arn_resource_id` returns the resource-id segment only — `src/state.rs:2661-2663`: `arn_parts(arn).map(|parts| parts.resource_id)`.)

Why that suffix is NOT unique across regions — `src/state.rs:1414-1426`:

    /// The next sequence to mint for each (resource kind, region) pair — the number behind
    /// the `<kind>/<id>` ARN suffix.
    ...
    next_resource_ids: BTreeMap<(MintedResource, String), u64>,

and `src/state.rs:2327-2334` (`mint_resource`):

        let region = arn_region(region);
        let sequence = self
            .next_resource_ids
            .entry((resource, region.clone()))
            .or_default();
        let id = minted_resource_id(resource.id_prefix(), *sequence);

so the per-region sequence restarts at 0 and `minted_resource_id("hv-", 0)` is `hv-0000000000000000` in every region.

The sync path that publishes it — `src/state.rs:2020-2022` (`start_metadata_sync`):

        for machine in synced_inventory(arn) {
            self.record_virtual_machine(&region, arn, machine, discovered);
        }

reached from the handler at `src/ops/start_virtual_machines_metadata_sync.rs:199-202`:

        let started = store.start_metadata_sync(
            &input.hypervisor_arn,
            ::std::time::SystemTime::now(),
        );

Test coverage: `tests/all/virtual_machine_inventory_sync.rs` drives one region only (`use crate::gateway_lifecycle::{..., REGION};`, and the ARN assertion at `:161` is `format!("arn:aws:backup-gateway:{REGION}:{}:vm/", ...)`), and a grep of the crate for "no two hypervisors" finds the claim only in `src/state.rs:1244` — no test pins that two hypervisors report distinct machines, so the broken rule is untested.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted and SigV4 credential scopes honoured.
2. Call ImportHypervisorConfiguration signing for us-east-1 with {"Name":"hv-a","Host":"198.51.100.1"} -> HypervisorArn arn:aws:backup-gateway:us-east-1:000000000000:hypervisor/hv-0000000000000000.
3. Call ImportHypervisorConfiguration signing for eu-central-1 with {"Name":"hv-b","Host":"198.51.100.2"} -> HypervisorArn arn:aws:backup-gateway:eu-central-1:000000000000:hypervisor/hv-0000000000000000 (the per-region mint sequence restarted at 0, so the resource id is identical).
4. Call StartVirtualMachinesMetadataSync for each of the two ARNs, each signed for its own region.
5. Wait for METADATA_SYNC_SUCCEEDS_AFTER, then call ListVirtualMachines signed for us-east-1 and again signed for eu-central-1.
6. Compare the two inventories: both report three machines named hv-0000000000000000-vm-1..3 with HostName equal to the name and Path /Datacenter/vm/hv-0000000000000000-vm-1..3 — i.e. the two hypervisor records report the same machines, which src/state.rs:78 and src/state.rs:1244 both state cannot happen.

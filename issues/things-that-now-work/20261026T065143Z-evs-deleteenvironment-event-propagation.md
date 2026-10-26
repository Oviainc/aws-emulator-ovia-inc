Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# An Elastic IP associated while the environment is DELETING is never released from the account's EC2 store

_Filed: 2026-10-26T06:51:43Z_
_Operation: DeleteEnvironment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteEnvironment releases the account's EC2 Elastic IP associations exactly once — on the call that starts the deletion, from the set of associations the VLANs hold at that instant — and then the environment stays writable for the whole TRANSITION_DELAY: AssociateEipToVlan refuses only an environment that has already reached DELETED, so an associate issued while the deletion is in flight is accepted, writes a NEW association onto the allocation's EC2 `Address` record (and onto the HCX VLAN, whose EC2 subnet the same delete already removed), and nothing ever releases it. `StoredEnvironment::materialise_due_transition` settles exactly this late-arrival case for the other sub-resources — "including a record that reached the environment after its deletion had started" — but `finish_dependent_deletions` walks only `connectors` and `entitlements`; no code path releases a VLAN's Elastic IP associations a second time.

The result contradicts this crate's own two statements about a deleted environment's addresses: DeleteEnvironment says "nothing is associated with a VLAN subnet that is being deleted, so a later associate can use them again", and `eip_allocation_holder` says an environment on its way out "holds nothing: its VLAN subnets are released with it, and so are the addresses they were using" — and it deliberately skips DELETING/DELETED environments. So after the window, EC2 still reports the allocation associated, EVS's own holder check reports it free, and the two checks inside AssociateEipToVlan disagree: a later associate of that allocation on a NEW environment passes the EVS check and is then refused by EC2 with the AlreadyAssociated clause. The allocation is unusable through EVS from then on, and no EVS call can disassociate it (DisassociateEipFromVlan on the now-DELETED environment answers ResourceNotFoundException).

## Evidence

AssociateEipToVlan admits an environment whose deletion is in flight — src/ops/associate_eip_to_vlan.rs:229-236:
```
        let environment = state
            .environment_in_region(&region, &input.environment_id)
            .filter(|environment| environment.state != crate::state::STATE_DELETED)
```
The delete's EC2 release runs only on the call that starts the deletion — src/ops/delete_environment.rs:238 `if !already_going_away {`, src/ops/delete_environment.rs:265-275 (`released_eips = environment.vlans.iter().flat_map(|vlan| vlan.eip_associations.iter())…`) and src/ops/delete_environment.rs:303-306:
```
        // The same reading for the Elastic IP addresses the environment's VLANs were
        // using: the allocations remain the account's, but nothing is associated with a
        // VLAN subnet that is being deleted, so a later associate can use them again.
        crate::state::ec2_resources::release_elastic_ips(&*self.engine, &released_eips);
```
The crate handles the late-arriving sub-resource for connectors and entitlements only — src/state.rs:1208-1213:
```
            if self.state == STATE_DELETED {
                // The environment is gone as of this instant, so nothing that belongs
                // to it is live any more — including a record that reached the
                // environment after its deletion had started and so rode no transition
                // of its own ([`Self::start_dependent_deletions`]).
                self.finish_dependent_deletions(pending.due_at_ms);
            }
```
and src/state.rs:1309-1318 shows `finish_dependent_deletions` walking `self.connectors` and `self.entitlements` and nothing else — no VLAN / `eip_associations` pass.

The rule the store states for a departing environment's addresses — src/state.rs:2388-2396:
```
    /// The scope is every environment rather than one Region's… An environment on its way out
    /// (`DELETING`/`DELETED`) holds nothing: its VLAN subnets are released with it, and
    /// so are the addresses they were using.
    pub fn eip_allocation_holder(&self, allocation_id: &str) -> Option<(String, String)> {
        self.environments
            .values()
            .filter(|environment| {
                environment.state != STATE_DELETED && environment.state != STATE_DELETING
            })
```
The window is real and deterministic: `pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);` (src/state.rs:234), and the association is written straight onto EC2's record (`address.association_id = Some(association_id.clone());`, src/state/ec2_resources.rs:654) with only `release_elastic_ips` able to clear it (src/state/ec2_resources.rs:671-697).

Test coverage: tests/all/eip_ec2_propagation.rs:311 covers only "two allocations on one VLAN, released by deleting the environment" — associations that existed BEFORE the delete; no test associates during the DELETING window, and tests/all/environment_deletion_cascade.rs pins the late-arrival rule for connectors/entitlements only.

## Steps to Reproduce

1. Allocate an Elastic IP in the EC2 emulator (`AllocateAddress`) and note its `eipalloc-…` allocation id.
2. CreateEnvironment with `initialVlans.isHcxPublic = true` and an `hcxNetworkAclId`; wait past the transition so the environment is CREATED, and delete every host with DeleteEnvironmentHost.
3. Call DeleteEnvironment: 200, `environmentState` is DELETING — the handler has already released the VLAN subnets and the then-held EIP associations in EC2.
4. Within the next 500 ms (TRANSITION_DELAY) call AssociateEipToVlan with `vlanName: "hcx"` and that allocation id: it is accepted 200 and EC2's `Address` record now carries a fresh `eipassoc-…`.
5. Sleep past the transition: GetEnvironment reports DELETED, and DisassociateEipFromVlan for that environment answers ResourceNotFoundException.
6. EC2 `DescribeAddresses` still reports the allocation associated with that `eipassoc-…`; calling AssociateEipToVlan for the same allocation on a freshly created environment passes EVS's own `eip_allocation_holder` check and is then refused with the ValidationException "names an Elastic IP address that is already associated… disassociate it first".

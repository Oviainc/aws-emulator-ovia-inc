Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# Single-resource gateway/hypervisor operations ignore the request Region, so a caller can delete and read records its own listings exclude

_Filed: 2026-10-28T15:46:19Z_
_Operation: DeleteGateway | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate implements Region scoping in exactly one place — the three listings — and nowhere else. `ListGateways`, `ListHypervisors` and `ListVirtualMachines` all read `crate::state::request_region(ctx.parts())` and hand it to `page_of_in_region` / `page_of_virtual_machines`, which drop every record whose ARN segment names another Region (`crate::state::arn_is_in_region`). `CreateGateway` and `ImportHypervisorConfiguration` mint the ARN in the signed-for Region, and `CreateGateway` goes further and refuses an activation key this service issued for a *different* Region (`activation_key_is_redeemable` compares `pending.region == arn_region(region)`), on the stated grounds that "a key issued for another region was issued by another region's service".

Every operation that resolves a single ARN instead throws the request metadata away with `let _ = ctx;` and looks the ARN up in the one global map with no Region predicate: `DeleteGateway`, `GetGateway`, `UpdateGatewayInformation`, `UpdateGatewaySoftwareNow`, `PutMaintenanceStartTime`, `PutBandwidthRateLimitSchedule`, `GetBandwidthRateLimitSchedule`, `TestHypervisorConfiguration`, `AssociateGatewayToServer`, `DisassociateGatewayFromServer`, `GetHypervisor`, `UpdateHypervisor`, `DeleteHypervisor`, `StartVirtualMachinesMetadataSync`, `PutHypervisorPropertyMappings`, `GetHypervisorPropertyMappings`, `GetVirtualMachine`, `TagResource`, `UntagResource`, `ListTagsForResource`.

The result is that two reads of the same store, through the same crate, disagree about whether a record belongs to the caller's Region:

* A gateway created in `us-east-1` is NOT reported by `ListGateways` signed for `eu-central-1` (pinned by the crate's own suite), yet `GetGateway` signed for `eu-central-1` returns it in full and `DeleteGateway` signed for `eu-central-1` REMOVES it from the store. A caller can therefore destroy a resource it is not allowed to enumerate — state loss across a boundary the same crate enforces elsewhere.
* `AssociateGatewayToServer` signed for `eu-central-1` will link a `us-east-1` gateway to a `us-east-1` hypervisor, after which `GetGateway` reports a `HypervisorId` that `ListHypervisors` in that Region never lists.
* `StartVirtualMachinesMetadataSync` signed for one Region materialises an inventory whose machine ARNs are minted in the hypervisor's Region (`start_metadata_sync` reads the Region off the stored hypervisor ARN), so `ListVirtualMachines` in the calling Region reports none of the machines the call just created.

The defect is the missing use of the helpers the crate already owns (`crate::state::request_region`, `crate::state::arn_is_in_region`) on the single-ARN resolution paths, not a new feature: the helpers exist, the sibling listings call them, and the crate's own state module states the rule in general terms about "every resource of this service".

## Evidence

The rule the crate states for itself — `src/state.rs:77-82` (module doc):

    //! Every resource of this service is REGIONAL — its region is a segment of the
    //! ARN it is minted and keyed under — so every listing reports only the records of the
    //! region the listing request itself was addressed to ([`arn_is_in_region`], applied
    //! before the page is cut), whichever resource it lists: one store serves every region a
    //! client addresses, and a listing is an account-and-region listing, never a listing of
    //! every region at once.

and `src/state.rs:1350-1355` (`page_of_gateways`):

    /// `region` is the region the listing request was addressed to, and only the gateways
    /// minted in it are part of the listing ([`page_of_in_region`]): a gateway is a
    /// regional resource, so one of another region is no more part of this listing than a
    /// gateway of another account would be.

Side that APPLIES the check — `src/ops/list_gateways.rs:140-146`:

        // A gateway is REGIONAL — its ARN carries the region the create that minted it was
        // signed for — and this listing reports the gateways "owned by an Amazon Web
        // Services account in an Amazon Web Services Region", so the region the listing
        // request itself was addressed to is what decides which records are part of it.
        // One store serves every region a client addresses, so without this a listing
        // would report gateways another region's endpoint owns.
        let region = crate::state::request_region(ctx.parts());

(identical reads at `src/ops/list_hypervisors.rs:146` and `src/ops/list_virtual_machines.rs:153`; the predicate is `src/state.rs:2293-2295` `arn_is_in_region`).

Side that SKIPS it — `src/ops/delete_gateway.rs:139-140` and `:154`:

        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;
    ...
        let Some(removed) = state.lock().remove_gateway(&input.gateway_arn) else {

and `src/ops/get_gateway.rs:140-141` / `:154`:

        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;
    ...
        let Some(gateway) = state.gateway(&input.gateway_arn) else {

The same `let _ = ctx;` plus un-filtered map lookup appears in `get_hypervisor.rs:141-142/155`, `delete_hypervisor.rs:145-146/162`, `update_gateway_information.rs:145-146/160`, `update_gateway_software_now.rs:145-146/164`, `put_maintenance_start_time.rs:176-177/231`, `put_bandwidth_rate_limit_schedule.rs:157-158/188`, `get_bandwidth_rate_limit_schedule.rs:154-155/174`, `associate_gateway_to_server.rs:160-161/181`, `disassociate_gateway_from_server.rs:153-154/175`, `test_hypervisor_configuration.rs:167-168/195`, `start_virtual_machines_metadata_sync.rs:170-171/199`, `put_hypervisor_property_mappings.rs:182-183/213`, `get_hypervisor_property_mappings.rs:149-150/170`, `get_virtual_machine.rs:144-145/161`, `tag_resource.rs:146-147/166`, `untag_resource.rs:149-150/171`, `list_tags_for_resource.rs:143-144/160`. The store maps they index are global: `src/state.rs:1331-1342`

    pub fn gateway(&self, arn: &str) -> Option<&StoredGateway> {
        self.gateways.get(arn)
    }
    ...
    pub fn remove_gateway(&mut self, arn: &str) -> Option<StoredGateway> {
        self.gateways.remove(arn)
    }

That the same crate treats one Region's service as distinct from another's — `src/ops/create_gateway.rs:176-179` and `src/state.rs:1263-1267`:

        // A gateway is regional, so its ARN carries the region the caller signed for — and
        // so does the activation the create redeems, which the handshake issued for one
        // region.
        let region = crate::state::request_region(ctx.parts());

    pub fn activation_key_is_redeemable(&self, key: &str, region: &str) -> bool {
        self.pending_activations
            .get(key)
            .is_some_and(|pending| pending.region == arn_region(region))
    }

Test coverage: `tests/all/regional_listing_scope.rs:11-18` pins the listing side across five Regions —

    //! The rule under test: every resource of this service is regional (its region is a
    //! segment of the ARN minted for it), so a listing reports only the resources of the
    //! Region the listing request was addressed to ... and the records of every
    //! other region are no more part of it than another account's would be.

No test in the suite exercises a single-resource operation with a cross-Region credential scope: the only other `call_in_region` callers are `gateway_activation.rs:650` (the activation-key Region refusal) and `default_maintenance_window.rs:75/225/276`, which always sign for the same Region the record was created in. So the cross-Region read/delete path is both wrong and untested.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted and SigV4 scopes honoured.
2. Perform the activation handshake against us-east-1 (`GET /?activationRegion=us-east-1&no_redirect`) and call CreateGateway with the issued key, signing the request for us-east-1. Note the returned GatewayArn (its region segment is us-east-1).
3. Call ListGateways signing for eu-central-1 -> the response omits that gateway, as tests/all/regional_listing_scope.rs pins (page_of_in_region drops it via arn_is_in_region).
4. Call GetGateway with the same GatewayArn, signing for eu-central-1 -> HTTP 200 with the full GatewayDetails of a gateway the previous step said is not in this Region.
5. Call DeleteGateway with that GatewayArn, signing for eu-central-1 -> HTTP 200 and the record is removed from the store.
6. Call GetGateway with the same ARN signing for us-east-1 -> 404 ResourceNotFoundException: the Region that owns the gateway has lost it to a caller that could not even list it.
7. Repeat with hypervisors: ImportHypervisorConfiguration signed for us-east-1, then DeleteHypervisor / UpdateHypervisor / StartVirtualMachinesMetadataSync signed for eu-central-1 on the returned HypervisorArn — all succeed, while ListHypervisors signed for eu-central-1 reports nothing.

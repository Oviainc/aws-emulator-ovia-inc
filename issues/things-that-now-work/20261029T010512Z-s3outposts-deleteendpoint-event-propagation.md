Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: d034a3e938ef16cd07dd30892ca1ec04abab6413

# An endpoint whose Outpost leaves the live S3 inventory is orphaned forever: no listing reports it, DeleteEndpoint answers 404, and the CIDR/VPC mapping it holds is never released

_Filed: 2026-10-29T01:05:12Z_
_Operation: DeleteEndpoint | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `d034a3e938ef16cd07dd30892ca1ec04abab6413`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The S3 on Outposts inventory is resolved LIVE from the co-hosted AWS Outposts control plane on every call (`crate::state::outpost_inventory`), so an Outpost disappears from this service's inventory the moment that control plane stops reporting it as Active-with-S3-capacity — a cancelled S3 order (`outposts:CancelOrder`), a requested decommission (`outposts:StartOutpostDecommission`) or a non-Active life-cycle status all do it. The ENDPOINT records this service stored against that Outpost, however, are never touched:

* every listing filters on `home_region(endpoint) == Some(region)`, and `outpost_region` answers `None` for an id the inventory no longer holds, so `ListEndpoints` and `ListSharedEndpoints` report the endpoint to nobody;
* `DeleteEndpoint` resolves the Outpost FIRST and answers the modelled 404 before it ever looks at the endpoint, so the record can never be removed by a client;
* `ComAmazonawsS3outpostsS3outpostsState::remove_endpoint` — the crate's own "release what this record held" helper — is called from exactly one place, `settle_all`, and only for an endpoint whose accepted teardown has fallen due (`endpoint.removal_due(now)`). Nothing calls it for an endpoint whose parent Outpost has left the inventory.

The result is stored state that no operation of this service can observe or release: the record keeps its committed `/26` (so `allocate_cidr_block` will never offer that block again) and keeps its `subnet_vpcs` entry (so that side table keeps an entry for a subnet no reachable endpoint sits in). Each repetition of create-Outpost -> order S3 capacity -> CreateEndpoint -> CancelOrder strands one more such record for the life of the process.

The 404 itself is not the complaint — the Outpost genuinely no longer exists for this service, and `control_plane_inventory.rs` pins that answer. The defect is that the endpoints it carried are left behind instead of being released through the helper the crate already owns, which contradicts the crate's own "BOUNDED" invariants on `endpoints` and `subnet_vpcs`.

## Evidence

Live inventory, stated by the crate — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:33-36:
  "this service keeps no copy of them and resolves the inventory from it on every call ([`outpost_inventory`]), so an Outpost created, ordered for or decommissioned through the `outposts` API is one this service answers for at the same instant."

No listing reports the orphan — src/state.rs:1218 (inside `endpoint_page`):
  "|endpoint| home_region(endpoint) == Some(region) && include(endpoint),"
with src/state.rs:819-823:
  "pub fn outpost_region<'a>(inventory: &'a OutpostInventory, outpost_id: &str) -> Option<&'a str> { inventory.get(outpost_id).map(|outpost| outpost.region.as_str()) }"
(an id the inventory no longer holds yields `None`, which never equals `Some(region)`).

DeleteEndpoint can never reach it — src/ops/delete_endpoint.rs:185-192:
  "// The Outpost is resolved FIRST: an endpoint is named by the (Outpost, endpoint) pair, so an Outpost that resolves to nothing this caller has access to ... is the modelled 404 before any endpoint under it is looked at"
  "crate::state::resolve_outpost(&inventory, &region, &input.outpost_id).map_err(DeleteEndpointErrorModeled::ResourceNotFoundException)?;"

The only release path is teardown-due, never Outpost-gone — src/state.rs:650-663:
  "pub fn settle_all(&mut self, now: SystemTime) { let completed: Vec<(String, String)> = self.endpoints.iter().filter(|(_, endpoint)| endpoint.removal_due(now)).map(|(key, _)| key.clone()).collect(); for key in &completed { self.remove_endpoint(key); } ..."
and src/state.rs:670-673:
  "This is the END of a teardown, not its start: `DeleteEndpoint` only accepts the deletion ([`EndpointRecord::begin_deletion`]), and [`Self::settle_all`] calls this for the endpoint whose teardown the clock has since completed."
(`remove_endpoint` has no other caller in the crate; the handlers only ever call `settle_all` — src/ops/create_endpoint.rs:197, src/ops/delete_endpoint.rs:183, src/state.rs:1208.)

The invariants this breaks — src/state.rs:484-488:
  "BOUNDED: an endpoint is only stored once [`Self::allocate_cidr_block`] has committed one of the `ENDPOINT_CIDR_BLOCK_COUNT` ranges the emulator reserves"
and src/state.rs:495-500:
  "BOUNDED by [`Self::endpoints`]: an entry exists only for a subnet a stored endpoint sits in. [`Self::remove_endpoint`] drops the entry with the last endpoint in that subnet, so a create/delete sequence cannot grow this map"
(the orphan keeps both: `allocate_cidr_block` skips any candidate matched by `self.endpoints.values()`, src/state.rs:599-613, and `remove_endpoint` is what drops the `subnet_vpcs` entry, src/state.rs:678-688).

Test coverage of the surrounding rule, and the hole in it — tests/all/control_plane_inventory.rs:647-700 drives "the only S3-capable order cancelled again" and asserts "an Outpost with no ordered S3 capacity is in no inventory" plus a 404 from CreateEndpoint / ListSharedEndpoints / DeleteEndpoint — but the DeleteEndpoint row names a never-issued id (`.delete_endpoint_in(REGION, &outpost_id, "0000000000000000001")`, line 680) and no row creates an endpoint BEFORE the order is cancelled, so the orphaned-record case is untested. tests/all/endpoint_lifecycle.rs:1793-1879 (`a_create_delete_sequence_leaves_no_subnet_mapping_behind`) pins the release path only for the create/delete sequence.

## Steps to Reproduce

1. Mount both `s3-outposts` and `outposts` over one shared engine (as tests/all/control_plane_inventory.rs:104-118 does)
2. outposts:CreateSite, then outposts:CreateOutpost -> OutpostId=op-xxxxxxxxxxxxxxxxx
3. outposts:CreateOrder against a catalog item whose published S3 storage capacity is non-zero, so the Outpost enters the S3 inventory
4. s3-outposts:CreateEndpoint(OutpostId=op-xxxxxxxxxxxxxxxxx, SubnetId=subnet-0a1b2c3d4e5f60718, SecurityGroupId=sg-0a1b2c3d4e5f60718) -> 200; note the EndpointId at the tail of the returned EndpointArn and the CidrBlock reported for it by ListEndpoints
5. outposts:CancelOrder on that order (or outposts:StartOutpostDecommission on the Outpost)
6. s3-outposts:ListOutpostsWithS3 -> the Outpost is gone from the inventory (as intended)
7. s3-outposts:ListEndpoints?maxResults=100 -> the endpoint is reported by nobody
8. s3-outposts:DeleteEndpoint?outpostId=op-xxxxxxxxxxxxxxxxx&endpointId=<the id> -> 404 ResourceNotFoundException, so the record cannot be removed
9. Inspect the store (engine.state::<ComAmazonawsS3outpostsS3outpostsState>()): `endpoints` still holds the record, `subnet_vpcs` still holds the subnet entry, and the committed /26 is still excluded by allocate_cidr_block — expected: the record and everything it held are released when its Outpost leaves the inventory, via the existing remove_endpoint helper

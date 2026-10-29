Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb

# The subnet→VPC side table is the one store map that is not Region-scoped, so an endpoint's VpcId is decided by endpoints of another Region

_Filed: 2026-10-29T03:15:35Z_
_Operation: CreateEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7a484987dc823baaa4ff3cdf8b9628f66a8b0eeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states one Region rule for the whole store: "every lookup and listing resolves inside the Region the request was signed for alone", "exactly as if the store held one set of resources per Region" (src/state.rs:60-63). Every other path obeys it — `resolve_outpost` filters `outpost.region == region` (src/state.rs:802-811), `outpost_page` filters `|outpost| outpost.region == region` (src/state.rs:1126), `endpoint_page` filters `home_region(endpoint) == Some(region)` (src/state.rs:1218).

`ComAmazonawsS3outpostsS3outpostsState::subnet_vpcs` is the exception: it is keyed by the bare `SubnetId` with no Region beside it (src/state.rs:501), and `vpc_for_subnet` resolves it globally (src/state.rs:573-576). `CreateEndpoint` is the one caller (src/ops/create_endpoint.rs:249), so the `Endpoint$VpcId` an endpoint reports is not computed from this Region's state at all:

* a client that creates an endpoint on an Outpost in us-east-1 and another on an Outpost in eu-west-1 with the SAME `SubnetId` gets ONE VPC id reported for both — each Region's listing reports a VPC that was minted for an endpoint the Region's own listings (correctly) cannot see. Under the stated per-Region store the two subnets are two different subnets and would carry two different VPC ids;
* the entry's LIFETIME crosses the Region boundary the same way. `remove_endpoint` drops the mapping only when no stored endpoint anywhere is left in that subnet (src/state.rs:680-686), so after every us-east-1 endpoint in that subnet is torn down the mapping survives on the strength of the eu-west-1 one, and the next us-east-1 endpoint in that subnet is handed the old id rather than the fresh one the member's own doc promises ("a subnet that no endpoint is left in is simply one this emulator no longer knows a VPC for, which the next endpoint there mints afresh", src/state.rs:498-500).

The defect is the missing Region in the one map that lacks it, not the minting of VPC ids itself: the crate already has `request_region`/`OutpostRecord::region` and every sibling lookup uses them.

## Evidence

The rule the crate states for the whole store — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:55-63:

    //! An Outpost is installed at a site anchored to ONE Region, and the S3 on
    //! Outposts endpoints on it are reached through that Region's endpoint, so each
    //! resource here belongs to one Region and is reachable only through it:
    //! [`OutpostRecord::region`] records the Outpost's own (home) Region, an endpoint
    //! takes the Region of the Outpost it is on ([`outpost_region`]),
    //! and every lookup and listing resolves inside the Region the request was signed
    //! for alone ([`request_region`]) — so a resource of another Region is reported by
    //! no listing and resolves to the modelled 404, exactly as if the store held one
    //! set of resources per Region.

The map that breaks it — src/state.rs:489-501:

    /// The VPC the emulator has associated with a subnet. `Endpoint$VpcId` is
    /// "the ID of the VPC used for the endpoint" and a subnet belongs to exactly
    /// one VPC; this emulator models no EC2 subnets, so it mints one VPC id the
    /// first time a subnet is used and reports that same id for every later
    /// endpoint in the same subnet.
    ///
    /// BOUNDED by [`Self::endpoints`]: an entry exists only for a subnet a
    /// stored endpoint sits in. [`Self::remove_endpoint`] drops the entry with
    /// the last endpoint in that subnet, so a create/delete sequence cannot grow
    /// this map — and a subnet that no endpoint is left in is simply one this
    /// emulator no longer knows a VPC for, which the next endpoint there mints
    /// afresh.
    pub subnet_vpcs: BTreeMap<String, String>,

the global lookup — src/state.rs:571-582:

    /// The VPC id this emulator reports for `subnet_id`, minting one the first
    /// time that subnet is used.
    pub fn vpc_for_subnet(&mut self, subnet_id: &str) -> String {
        if let Some(vpc_id) = self.subnet_vpcs.get(subnet_id) {
            return vpc_id.clone();
        }

its one caller — src/ops/create_endpoint.rs:249:

    let vpc_id = store.vpc_for_subnet(&input.subnet_id);

and the Region-blind release — src/state.rs:678-686:

    pub fn remove_endpoint(&mut self, key: &(String, String)) -> Option<EndpointRecord> {
        let removed = self.endpoints.remove(key)?;
        if !self
            .endpoints
            .values()
            .any(|endpoint| endpoint.subnet_id == removed.subnet_id)
        {
            self.subnet_vpcs.remove(&removed.subnet_id);
        }

The sibling lookups that DO carry the Region, in the same module — src/state.rs:807-810 (`resolve_outpost`): "inventory.get(outpost_id).filter(|outpost| outpost.region == region)"; src/state.rs:1126 (`outpost_page`): "|outpost| outpost.region == region,"; src/state.rs:1218 (`endpoint_page`): "|endpoint| home_region(endpoint) == Some(region) && include(endpoint),".

Test coverage: the two cases that pin this side table both run in ONE Region, so the cross-Region case is untested — tests/all/endpoint_lifecycle.rs:1769-1782 ("endpoints in subnet {subnet_id} must report one VPC" / "subnet {subnet_id} must not report the VPC of {seen_subnet}") and tests/all/endpoint_lifecycle.rs:1800-1879 (`a_create_delete_sequence_leaves_no_subnet_mapping_behind`), both driven from `emulator_with_engine()` whose Outposts are all installed in `REGION` (tests/all/endpoint_lifecycle.rs:100-102, :107-129). tests/all/region_scoping.rs provisions its Outposts in a single home Region per fixture, so no existing case stores endpoints of two Regions in one engine.

## Steps to Reproduce

1. Stand up one emulator holding two Outposts in two Regions — `emulator_holding(&[("op-0a1b2c3d4e5f60718".into(), "us-east-1"), ("op-1b2c3d4e5f6071829".into(), "eu-west-1")])` (tests/all/endpoint_lifecycle.rs:71-87), or run with S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248:<account>:us-east-1,op-1b2c3d4e5f6071829:52776558133248:<account>:eu-west-1"
2. POST /S3Outposts/CreateEndpoint signed for us-east-1 with OutpostId=op-0a1b2c3d4e5f60718, SubnetId=subnet-0a1b2c3d4e5f60718, SecurityGroupId=sg-0a1b2c3d4e5f60718 -> 200
3. POST /S3Outposts/CreateEndpoint signed for eu-west-1 with OutpostId=op-1b2c3d4e5f6071829, the SAME SubnetId=subnet-0a1b2c3d4e5f60718 and SecurityGroupId=sg-0a1b2c3d4e5f60718 -> 200
4. GET /S3Outposts/ListEndpoints?maxResults=100 signed for us-east-1 and read the VpcId of the single endpoint reported; repeat signed for eu-west-1
5. Observe the SAME VpcId in both Regions: the eu-west-1 endpoint reports a VPC id minted for an endpoint that only us-east-1's listing can see. Expected per src/state.rs:60-63 (a store that behaves "exactly as if the store held one set of resources per Region"): the subnet named in eu-west-1 is a different subnet, so it carries its own VPC id
6. Lifetime symptom: DELETE /S3Outposts/DeleteEndpoint?outpostId=op-0a1b2c3d4e5f60718&endpointId=<the us-east-1 id>, wait past the teardown span, then create another endpoint in us-east-1 on subnet-0a1b2c3d4e5f60718 — it is handed the ORIGINAL VPC id, because the eu-west-1 endpoint kept the mapping alive (src/state.rs:680-686), rather than the fresh one src/state.rs:498-500 describes for a subnet no endpoint is left in

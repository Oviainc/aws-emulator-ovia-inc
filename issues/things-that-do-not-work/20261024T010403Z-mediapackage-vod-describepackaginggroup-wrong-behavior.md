Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# MediaPackage VOD state is not partitioned by Region: a group/configuration/asset created in one Region resolves and lists in every other

_Filed: 2026-10-24T01:04:03Z_
_Operation: DescribePackagingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaPackage VOD is a regional service: a PackagingGroup, PackagingConfiguration or Asset created through the us-west-2 endpoint does not exist through the eu-central-1 endpoint (a describe there answers NotFoundException/404 and the listings there do not report it).

The emulator keeps one global store keyed only by the resource's own `Id` (`ComAmazonawsMediapackagevodMediapackagevodState { packaging_groups, packaging_configurations, assets, tags_by_arn }` in `src/state.rs`), and the request's region is read ONLY to mint the ARN and the egress domain name at create time (`crate::state::request_region(ctx.parts())` in `create_packaging_group.rs`, `create_packaging_configuration.rs`, `create_asset.rs`). Every read/mutate/delete handler — `describe_packaging_group.rs`, `describe_packaging_configuration.rs`, `describe_asset.rs`, `list_*`, `configure_logs.rs`, `update_packaging_group.rs`, `delete_*` — looks the id up with no region at all (several of them do not even take `ctx`: `_ctx` is discarded).

Expected: a group created with us-west-2 credentials is invisible (404) to a describe signed for eu-central-1, and absent from ListPackagingGroups there; ids may be reused independently per region.
Actual: the cross-region describe returns 200 with the record, and the projection reports the OTHER region's `Arn` (`arn:aws:mediapackage-vod:us-west-2:...`) and `DomainName` (`https://<hex>.egress.mediapackage-vod.us-west-2.amazonaws.com`) — a response AWS can never produce, since an ARN's region always matches the endpoint that served it. ConfigureLogs / UpdatePackagingGroup / Delete* likewise mutate and destroy resources that belong to another region.

## Evidence

Spec/well-known semantics: the Smithy model's `aws.api#service` gives endpointPrefix/arnNamespace `mediapackage-vod` and sigv4 signing name `mediapackage-vod` — a regional, SigV4-signed control plane whose resources are scoped to the signing region, and whose ARNs embed that region (`arn:aws:mediapackage-vod:<region>:<account>:packaging-groups/<Id>`).
Handler: `src/state.rs` lines ~206-220 define the three resource maps keyed by `Id` alone, with no region dimension; `src/ops/describe_packaging_group.rs` `handle()` does `state.packaging_groups.get(&input.id)` and ignores `_ctx`, so the credential scope's region (available via `crate::state::request_region`, used by the creates) is never consulted on read.
Tests: no test in `tests/all/` exercises a second region — `packaging_lifecycle.rs`, `success_path_projections.rs`, `describe_packaging_configuration.rs`, `packaging_group_update.rs` and `error_responses.rs` each sign for a single `REGION` constant, so nothing asserts regional isolation. No existing open/fixed MediaPackage VOD issue covers region scoping (the open ones cover the egress data plane + CreatePackagingGroup authorization + DeleteAsset status, and DeletePackagingGroup dependency checking).

## Steps to Reproduce

1. Boot the emulator and call CreatePackagingGroup(Id="vod-group") with credentials signed for region us-west-2; note the returned Arn/DomainName carry us-west-2.
2. Call DescribePackagingGroup(Id="vod-group") with credentials signed for region eu-central-1: observe HTTP 200 with the us-west-2 Arn and DomainName, where AWS answers NotFoundException (404).
3. Call ListPackagingGroups signed for eu-central-1: observe the us-west-2 group listed.
4. Repeat with CreateAsset/DescribeAsset and CreatePackagingConfiguration/DescribePackagingConfiguration, and with UpdatePackagingGroup/ConfigureLogs/DeletePackagingGroup signed for the other region: observe that the foreign-region resource is mutated and deleted instead of being reported missing.
5. Also observe that creating the same Id in two regions is refused as a duplicate (422) instead of succeeding independently per region.

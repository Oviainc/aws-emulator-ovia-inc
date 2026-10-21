Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the MediaPackage VOD emulator

_Filed: 2026-10-21T22:08:14Z_
_Operation: service-com-amazonaws-mediapackagevod-mediapackagevod | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 17 operations in `services/service-com-amazonaws-mediapackagevod-mediapackagevod/src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state. Implement a FIRST stateful slice covering the core packaging-group -> asset lifecycle (restJson1 wire protocol, signing name `mediapackage-vod`).

### Happy path to implement (in order)

1. **CreatePackagingGroup** — `POST /packaging_groups` -> 200. Input: `Id` (required), `Authorization`, `EgressAccessLogs`, `Tags`. Must persist a new group keyed by `Id` and return a computed `Arn` (`arn:aws:mediapackage-vod:<region>:<account>:packaging-groups/<Id>`), a generated `DomainName`, `CreatedAt` (ISO-8601 of creation time), plus the stored `Authorization`, `EgressAccessLogs` and `Tags`.
2. **CreatePackagingConfiguration** — `POST /packaging_configurations` -> 200. Input: `Id` (required), `PackagingGroupId` (required), one of `HlsPackage`/`DashPackage`/`CmafPackage`/`MssPackage`, `Tags`. Must verify the referenced group exists (else `NotFoundException`, HTTP 404), persist the configuration with its package block verbatim, and return `Arn`, `CreatedAt`, `Id`, `PackagingGroupId` and the package block read back from state.
3. **CreateAsset** — `POST /assets` -> 200. Input: `Id`, `PackagingGroupId`, `SourceArn`, `SourceRoleArn` (all required), `ResourceId`, `Tags`. Must verify the group exists (404 otherwise), persist the asset, and return `Arn`, `CreatedAt`, `Id`, `PackagingGroupId`, `ResourceId` (defaulting to `Id` when absent), `SourceArn`, `SourceRoleArn`, `Tags`, and one `EgressEndpoints` entry per packaging configuration attached to that group (`PackagingConfigurationId` + a derived playback `Url` on the group's `DomainName`).
4. **DescribeAsset** — `GET /assets/{Id}` -> 200. Must return exactly the stored asset record (same `Arn`, `CreatedAt`, `EgressEndpoints`, `SourceArn`, `SourceRoleArn`, `Tags` as the create returned), and `NotFoundException` (404) for an unknown `Id`.
5. **ListAssets** — `GET /assets` -> 200. Must return `Assets` (list<AssetShallow>) from stored state, filtered by the `packagingGroupId` query param when present, honouring `maxResults` (1..1000) with an opaque `NextToken` that resumes where the previous page ended.
6. **DeleteAsset** — `DELETE /assets/{Id}` -> 202. Must remove the asset from state; a following DescribeAsset must return 404 and ListAssets must no longer include it. Unknown `Id` -> `NotFoundException` (404).

Also implement **DescribePackagingGroup** (`GET /packaging_groups/{Id}`) as the read-back for step 1, returning stored fields plus `ApproximateAssetCount` COMPUTED as the number of assets currently stored with that `PackagingGroupId`.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no generating an ARN/CreatedAt that is not stored and returned identically by the subsequent describe/list. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line in each implemented file.

## State Model

Store in `ComAmazonawsMediapackagevodMediapackagevodState` (src/state.rs), reached via `crate::state::service_state(&*self.engine)`:

- `packaging_groups: BTreeMap<String /*Id*/, PackagingGroup { id, arn, domain_name, created_at, authorization: Option<Authorization>, egress_access_logs: Option<EgressAccessLogs>, tags: BTreeMap<String,String> }>`
- `packaging_configurations: BTreeMap<String /*Id*/, PackagingConfiguration { id, arn, created_at, packaging_group_id, hls_package, dash_package, cmaf_package, mss_package (each Option), tags }>`
- `assets: BTreeMap<String /*Id*/, Asset { id, arn, created_at, packaging_group_id, resource_id, source_arn, source_role_arn, egress_endpoints: Vec<EgressEndpoint{packaging_configuration_id, url}>, tags }>`
- `tags_by_arn: BTreeMap<String /*ResourceArn*/, BTreeMap<String,String>>` (shared tag store so the later TagResource/UntagResource/ListTagsForResource slice reads the same tags the create calls stored)

`ApproximateAssetCount` and `EgressEndpoints` are DERIVED at read time from these maps, never stored as literals.

The remaining operations (ConfigureLogs, UpdatePackagingGroup, DeletePackagingGroup, DeletePackagingConfiguration, DescribePackagingConfiguration, ListPackagingConfigurations, ListPackagingGroups, TagResource, UntagResource, ListTagsForResource) stay at 501 for now and are tracked as separate gap issues.

## Evidence

OPERATIONS.md at the crate root documents all 17 operations with routes, required inputs and error sets: CreatePackagingGroup `POST /packaging_groups` (Id required; outputs Arn/DomainName/CreatedAt), CreatePackagingConfiguration `POST /packaging_configurations` (Id + PackagingGroupId required), CreateAsset `POST /assets` (Id, PackagingGroupId, SourceArn, SourceRoleArn required; outputs EgressEndpoints), DescribeAsset `GET /assets/{Id}`, ListAssets `GET /assets` (@paginated inputToken=NextToken, outputToken=NextToken, items=Assets; MaxResults range 1..1000; packagingGroupId filter), DeleteAsset `DELETE /assets/{Id}` -> 202, DescribePackagingGroup `GET /packaging_groups/{Id}` (ApproximateAssetCount). Every operation declares `NotFoundException (HTTP 404)` and `UnprocessableEntityException (HTTP 422)`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 17 handler files, so nothing is implemented and no state is held.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with the `mediapackage-vod` endpoint.
2. CreatePackagingGroup with Id="vod-group" and Tags={env:test} — expect 200 with a non-empty Arn, DomainName and CreatedAt (currently 501).
3. DescribePackagingGroup Id="vod-group" — expect the same Arn/DomainName/CreatedAt/Tags returned by the create, and ApproximateAssetCount=0.
4. CreatePackagingConfiguration with Id="hls-cfg", PackagingGroupId="vod-group" and an HlsPackage block — expect 200 echoing the package read back from state.
5. CreateAsset with Id="asset-1", PackagingGroupId="vod-group", SourceArn="arn:aws:s3:::bucket/movie.smil", SourceRoleArn="arn:aws:iam::123456789012:role/MediaPackageVod" — expect 200 with EgressEndpoints containing one entry for PackagingConfigurationId="hls-cfg".
6. DescribeAsset Id="asset-1" — expect the identical Arn, CreatedAt, SourceArn, SourceRoleArn, Tags and EgressEndpoints as the create returned.
7. ListAssets with packagingGroupId="vod-group" — expect Assets to contain asset-1; repeat with maxResults=1 after creating a second asset and expect a NextToken that returns the remaining asset on the second page.
8. DescribePackagingGroup Id="vod-group" — expect ApproximateAssetCount to reflect the number of assets created.
9. DeleteAsset Id="asset-1" — expect 202; then DescribeAsset Id="asset-1" must return NotFoundException (HTTP 404) and ListAssets must not include it.
10. DescribeAsset Id="does-not-exist" — expect NotFoundException (HTTP 404), not 501.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# A package block carrying an EMPTY manifest list is accepted and stored instead of being refused

_Filed: 2026-10-24T01:04:03Z_
_Operation: CreatePackagingConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A packaging configuration is defined by its manifests: `HlsPackage$HlsManifests`, `DashPackage$DashManifests`, `CmafPackage$HlsManifests` and `MssPackage$MssManifests` are all `@required` lists of manifest configurations, and AWS requires at least one manifest — a create whose package block carries an empty manifest list is refused with UnprocessableEntityException (HTTP 422) because the configuration would describe no output manifest at all.

Expected: `CreatePackagingConfiguration(Id=c, PackagingGroupId=g, HlsPackage={"hlsManifests": []})` is refused with UnprocessableEntityException/422 and nothing is stored (DescribePackagingConfiguration then answers NotFoundException).
Actual: the handler only counts how many of the four package blocks are present (`package_blocks != 1`) and never inspects the manifest list inside the chosen block, so the empty-list configuration is stored and answered 202 Accepted. Worse, downstream derivation silently invents a manifest that the caller never configured: `manifest_name_and_suffix()` in `src/state.rs` takes `pkg.hls_manifests.first()` and falls back to `DEFAULT_MANIFEST_NAME` ("index"), so every asset in the group derives an `EgressEndpoint` whose `Url` ends in `/index.m3u8` for a configuration that declares no manifests, and `DescribePackagingConfiguration`/`ListPackagingConfigurations` report a package block with an empty `hlsManifests` array.

## Evidence

Spec: `HlsPackage$HlsManifests` ("A list of HLS manifest configurations"), `CmafPackage$HlsManifests`, `DashPackage$DashManifests` and `MssPackage$MssManifests` are each marked `required` in the model; CreatePackagingConfiguration models `UnprocessableEntityException` (client, httpError 422, "The parameters sent in the request are not valid."). A configuration with zero manifests cannot produce the parent manifest that `EgressEndpoint$Url` is documented to point at ("The URL of the parent manifest for the repackaged Asset"), which is why the service rejects it.
Handler: `src/ops/create_packaging_configuration.rs` `handle()` lines ~189-218 validate only the id and the exactly-one-package-block rule, then store the block (with defaults applied) unchanged; `src/state.rs` `manifest_name_and_suffix()` lines ~343-363 use `.first()` with a "index" fallback, so the empty list is papered over rather than refused.
Tests: `tests/all/error_responses.rs::a_whole_request_violation_is_attributed_to_base` covers zero and multiple package blocks only; `tests/all/describe_packaging_configuration.rs` and `success_path_projections.rs` only use blocks with one or two manifests. A grep for an empty manifest array (`hlsManifests": []`) across `tests/` returns no matches, so no test asserts this rejection.

## Steps to Reproduce

1. Create a packaging group: POST /packaging_groups {"id": "vod-group"}.
2. POST /packaging_configurations with body {"id": "cfg", "packagingGroupId": "vod-group", "hlsPackage": {"hlsManifests": []}}: observe HTTP 202 and a stored configuration, where AWS refuses with UnprocessableEntityException (422).
3. GET /packaging_configurations/cfg: observe the configuration exists with `hlsPackage.hlsManifests == []`.
4. POST /assets {"id": "a1", "packagingGroupId": "vod-group", "sourceArn": "arn:aws:s3:::bucket/a1.smil", "sourceRoleArn": "arn:aws:iam::000000000000:role/MediaPackageVod"} then GET /assets/a1: observe an egressEndpoints entry for `cfg` whose url ends in `/index.m3u8` — a manifest name the request never supplied.
5. Repeat step 2 with {"dashPackage": {"dashManifests": []}} and {"mssPackage": {"mssManifests": []}}: same unvalidated acceptance.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# SourceArn and SourceRoleArn are stored as arbitrary strings with no ARN validation

_Filed: 2026-10-24T01:04:03Z_
_Operation: CreateAsset | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAsset` documents `SourceArn` as "ARN of the source object in S3" and `SourceRoleArn` as "The IAM role ARN used to access the source S3 bucket". AWS validates both as ARNs of the expected service before the asset is created and refuses a malformed value with UnprocessableEntityException (HTTP 422, "The parameters sent in the request are not valid."), creating nothing.

Expected: `CreateAsset(Id=a1, PackagingGroupId=g, SourceArn="not-an-arn", SourceRoleArn="also-not-an-arn")` is refused with UnprocessableEntityException/422 naming the offending member, and a following DescribeAsset answers NotFoundException (404).
Actual: the handler validates only that `Id` is non-empty, that `PackagingGroupId` resolves, that the asset id is free and that the capacity bound is not hit. The two ARN members are moved into the stored record unchecked (`source_arn: input.source_arn, source_role_arn: input.source_role_arn`), the create answers 200, and the junk values are echoed back by the create response and by every later DescribeAsset / ListAssets projection. Empty strings are accepted too, so an asset can be created with `sourceArn: ""`.

## Evidence

Spec: in the model, `CreateAssetRequest$SourceArn` and `$SourceRoleArn` are `@required` members documented as an S3 object ARN and an IAM role ARN respectively; CreateAsset models `UnprocessableEntityException` (errorKind client, httpError 422, "The parameters sent in the request are not valid.") — the refusal channel for invalid request parameters on this surface. Rejecting syntactically invalid ARNs for members documented as ARNs is well-established AWS control-plane behavior (the service must assume `SourceRoleArn` to read `SourceArn`, so it cannot be an arbitrary string).
Handler: `src/ops/create_asset.rs` `handle()` lines ~172-232 — the only input checks are `input.id.is_empty()`, `state.packaging_groups.contains_key(&input.packaging_group_id)`, the duplicate-id check and `MAX_ASSETS`; `source_arn`/`source_role_arn` are stored verbatim with no syntax check and no emptiness check.
Tests: every test supplies well-formed ARNs (`tests/all/packaging_lifecycle.rs::create_asset`, `tests/all/error_responses.rs::asset_body`, `success_path_projections.rs`), and `error_responses.rs` asserts only the empty-id, unresolvable-packagingGroupId and duplicate-id refusals — no test asserts an ARN-shape rejection, so this is uncovered.

## Steps to Reproduce

1. Create a packaging group: POST /packaging_groups {"id": "vod-group"}.
2. POST /assets with body {"id": "a1", "packagingGroupId": "vod-group", "sourceArn": "not-an-arn", "sourceRoleArn": "nope"}: observe HTTP 200 and a created asset, where AWS refuses with UnprocessableEntityException (422) and creates nothing.
3. GET /assets/a1: observe the asset exists and reports `sourceArn: "not-an-arn"` / `sourceRoleArn: "nope"`.
4. Repeat with {"sourceArn": "", "sourceRoleArn": ""} and with an ARN of the wrong service (e.g. sourceArn of an IAM role, sourceRoleArn of an S3 object): observe the same unvalidated acceptance.

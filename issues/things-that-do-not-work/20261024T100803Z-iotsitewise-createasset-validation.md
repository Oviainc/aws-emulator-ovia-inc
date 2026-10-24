Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# Tags supplied at create time bypass the TagKey rules TagResource enforces

_Filed: 2026-10-24T10:08:02Z_
_Operation: CreateAsset | Class: validation_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the tags member of a create carries the same TagMap/TagKey constraints as TagResource — a key must be 1..128 characters (and must not use the reserved `aws:` namespace) — so a create carrying a bad key is refused and no resource is created.

Actual: CreateAsset copies the map straight onto the record (`tags: input.tags.clone().unwrap_or_default()`, create_asset.rs line 322) without calling crate::state::check_tag_key, which TagResource does call. The framework's constraint walk reaches a map's VALUES but not its KEYS (stated in state.rs above check_tag_key), so a 200-character tag key, or an `aws:`-prefixed one, is accepted at create time and is then visible through ListTagsForResource — while the identical key is refused when added through TagResource. CreateAssetModel (create_asset_model.rs line 370) and CreateGateway (create_gateway.rs line 289) take the same unchecked path.

## Evidence

Smithy model: CreateAssetRequest.tags targets TagMap, whose key targets TagKey with @length(min:1,max:128). Handlers: src/ops/create_asset.rs:322, create_asset_model.rs:370, create_gateway.rs:289 (no key validation) versus src/ops/tag_resource.rs:171-175 (keys validated). Tests: tests/all/resource_tags.rs exercises TagResource/UntagResource only; no test supplies tags at create time with an invalid key.

## Steps to Reproduce

1. CreateAssetModel with tags={"<129-character key>": "v"} -> HTTP 202 accepted.
2. ListTagsForResource on the new model's ARN -> the over-long key is reported.
3. TagResource the same ARN with the same key -> 400 InvalidRequestException. The two paths disagree about the same constraint.

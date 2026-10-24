Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# Default-policy body parameters are dropped when the request does not name DefaultPolicy

_Filed: 2026-10-23T23:28:46Z_
_Operation: CreateLifecyclePolicy | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInterval`, `RetainInterval`, `CopyTags`, `ExtendDeletion`, `CrossRegionCopyTargets` and `Exclusions` may be stated in the request body, and `PolicyDetails` declares exactly those members, so a read must report what the body stated. The emulator only surfaces them when the record is flagged as a default policy (`DefaultPolicy` named, or `PolicyDetails.PolicyLanguage == SIMPLIFIED`). For any other request that carries them — e.g. body parameters with no `DefaultPolicy` at all, or body parameters alongside a `STANDARD`/custom `PolicyDetails` — the create answers HTTP 200, the values are written into the record, and no read path ever reports them: `LifecyclePolicyRecord::effective_policy_details` returns `self.policy_details.clone()` unchanged for `default_policy == false`. Expected: AWS either refuses the request (these members are documented `[Default policies only]`, and `InvalidRequestException` 400 is modeled for it) or reports the stated configuration back; a silent 200 that swallows the parameters is wrong either way.

## Evidence

Spec: `CreateLifecyclePolicyRequest$CreateInterval|RetainInterval|CopyTags|ExtendDeletion|CrossRegionCopyTargets|Exclusions` are documented `[Default policies only]` and `com.amazonaws.dlm#PolicyDetails` declares the same members, which `GetLifecyclePolicy`'s `LifecyclePolicy$PolicyDetails` reports.
Handler/state: `src/ops/create_lifecycle_policy.rs` — `is_default` is true only when `DefaultPolicy` is present or `PolicyDetails.PolicyLanguage == SIMPLIFIED`; the body parameters are stored on the record regardless. `src/state.rs` `effective_policy_details()` returns early with the raw `policy_details` (often `None`) when `!self.default_policy`, so the stored `create_interval`/`retain_interval`/`copy_tags`/`extend_deletion`/`cross_region_copy_targets`/`exclusions` are unreachable to any client.
Tests: `tests/all/lifecycle_policy_crud.rs` only pairs those body parameters with `DefaultPolicy` (rows "a default policy stated as body parameters"); no test covers body parameters without `DefaultPolicy`, so the drop is uncovered.

## Steps to Reproduce

1. POST /policies with {"ExecutionRoleArn": <role ARN>, "Description": "body params only", "State": "ENABLED", "CreateInterval": 3, "RetainInterval": 14, "CopyTags": true, "Exclusions": {"ExcludeBootVolumes": true}} — note no DefaultPolicy member
2. Observe HTTP 200 and a minted PolicyId (AWS refuses this with InvalidRequestException for [Default policies only] parameters on a non-default policy)
3. GET /policies/{PolicyId}: the response Policy carries no PolicyDetails at all — CreateInterval, RetainInterval, CopyTags and Exclusions stated in the request are reported nowhere
4. Repeat with a custom (STANDARD) PolicyDetails plus "CopyTags": true in the body: the create succeeds and the read reports the PolicyDetails without CopyTags

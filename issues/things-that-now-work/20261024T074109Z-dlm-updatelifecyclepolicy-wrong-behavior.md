Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# An update that replaces a default policy's PolicyDetails is shadowed by the stale body-level parameters

_Filed: 2026-10-24T07:41:09Z_
_Operation: UpdateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a DEFAULT policy, the `[Default policies only]` configuration members (`CreateInterval`, `RetainInterval`, `CopyTags`, `ExtendDeletion`, `CrossRegionCopyTargets`, `Exclusions`) may be stated either in the request body or inside `PolicyDetails`. The emulator stores the two places SEPARATELY on one record (`LifecyclePolicyRecord.create_interval` … beside `LifecyclePolicyRecord.policy_details`) and, on every read, lets the body-level value win unconditionally (`src/state.rs` `effective_policy_details`: `self.create_interval.or(details.create_interval).unwrap_or(DEFAULT_CREATE_INTERVAL)`, and the same `.or(...)` chain for `retain_interval`, `copy_tags`, `extend_deletion`, `exclusions`, plus `if !self.cross_region_copy_targets.is_empty()` for the copy targets).

`UpdateLifecyclePolicy` writes `policy.policy_details = Some(materialized_policy_details(policy_details))` but never clears the body-level members a previous create/update stored. Consequence: a default policy created with body `CreateInterval`/`RetainInterval` and then updated with a `PolicyDetails` structure that states new values is answered HTTP 200, yet every later read (`GetLifecyclePolicy`) keeps reporting the ORIGINAL body values — the update is a hollow success, and the client has no way to make the new configuration take effect.

The handler also compounds this by not enforcing, on the update path, the one cross-member rule the create does enforce (`CreateLifecyclePolicy`: a default policy states these parameters "either in the request body, or in the PolicyDetails request structure, but not both"): the update happily leaves a record that holds BOTH, which is the state that makes the shadowing permanent. Even the update's own validation disagrees with the read: `Cadence::violation` judges the request on `body.or(details).or(stored)` (so it validates the NEW details value), while the read reports the stored body value.

Expected: after an accepted `PATCH /policies/{PolicyId}` that states a default policy's configuration in `PolicyDetails`, a read reports the values that update stated (i.e. the replacement either supersedes the body-level values or the request is refused as stating the configuration twice). Actual: the read reports the pre-update body values and silently drops what the update stated.

## Evidence

Spec: `PolicyDetails$CreateInterval`/`RetainInterval`/`CopyTags`/`ExtendDeletion`/`CrossRegionCopyTargets`/`Exclusions` are the `[Default policies only]` members (`smithy_query(query="shape", shape="PolicyDetails")`), and `UpdateLifecyclePolicyRequest` declares the same members at the body level (`smithy_query(query="operation", operation="UpdateLifecyclePolicy")`); `CreateLifecyclePolicy`'s own documentation states the two are alternatives, "but not both". A control-plane update that returns success must be observable in the subsequent read of the resource.
Handler: `src/ops/update_lifecycle_policy.rs` lines ~299-321 overwrite `policy.policy_details` and the body members independently, with no reconciliation and no refusal when the record ends up holding both; `src/state.rs` `effective_policy_details` (lines ~396-431) resolves every one of those members as `self.<body value>.or(details.<value>)`, so the stored body value permanently outranks the structure the update just wrote.
Tests: `tests/all/lifecycle_policy_crud.rs::update_overwrites_only_the_members_the_request_carries`, `tests/all/default_policy_only_parameters.rs` and `tests/all/interval_validation.rs::an_update_enforces_the_documented_cadence_against_the_stored_policy` only ever restate a default policy's configuration in the SAME place the create used (body→body, details→details); no test creates with body parameters and then updates with a `PolicyDetails` structure, so the shadowing is unasserted. The open issue `dlm-updatelifecyclepolicy-validation` is about interval RANGES, not about this precedence.

## Steps to Reproduce

1. POST /policies with {"ExecutionRoleArn": <emulator-account role ARN>, "Description": "default volume policy", "State": "ENABLED", "DefaultPolicy": "VOLUME", "CreateInterval": 3, "RetainInterval": 10} and note the returned PolicyId
2. GET /policies/{PolicyId} — Policy.PolicyDetails reports CreateInterval 3 / RetainInterval 10
3. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME", "CreateInterval": 5, "RetainInterval": 14, "CopyTags": true, "ExtendDeletion": true}} — observe HTTP 200
4. GET /policies/{PolicyId} again and read Policy.PolicyDetails: CreateInterval is still 3 and RetainInterval still 10 (the stored body values), while CopyTags/ExtendDeletion/Exclusions behave the same way whenever the create stated them in the body
5. Expected: the accepted update is visible — the read reports CreateInterval 5 / RetainInterval 14 — or the request is refused for stating a default policy's configuration both in the body and in PolicyDetails, as the create does

Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5dc70b01fea90015c733b45f7727cf9a955e7fe2

# Scope the lifecycle-policy store to the request's region

_Filed: 2026-10-24T06:46:07Z_
_Operation: CreateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5dc70b01fea90015c733b45f7727cf9a955e7fe2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Data Lifecycle Manager is a regional service: a lifecycle policy created in one Region exists only in that Region. Expected: after a create signed for `us-east-1`, a `GetLifecyclePolicies` signed for `eu-west-1` reports no policies, and `GetLifecyclePolicy`/`UpdateLifecyclePolicy`/`DeleteLifecyclePolicy` for that policy id in `eu-west-1` answer `ResourceNotFoundException` (HTTP 404).

Actual: the store is one region-agnostic map keyed by policy id, so the create places the record where every other Region's requests can see it. The request's region is read only to build the policy ARN, so a cross-region read returns the policy with an ARN naming a DIFFERENT Region than the one the caller signed for — a policy whose ARN says `us-east-1` is listed, read, updated and deleted by `eu-west-1` callers. The minting counter is global too, so policy ids are shared across Regions instead of being per-Region.

## Evidence

Well-established AWS semantics: DLM is a regional control plane (`endpointPrefix: dlm`, per-Region endpoints) and `GetLifecyclePolicies` "gets summary information about all or the specified data lifecycle policies" of the Region the request is made in; a policy ARN embeds the Region it lives in (`PolicyArn` pattern `^arn:aws...:dlm:<region>:<account>:policy/...`).
Handler/state: `src/state.rs` line 591 — `pub policies: BTreeMap<String, LifecyclePolicyRecord>` with no Region dimension; `src/ops/create_lifecycle_policy.rs` line 320 reads `crate::state::request_region(ctx.parts())` only to compute `policy_arn` (state.rs 632) and then inserts into that single map (line 370). `get_lifecycle_policies.rs` (lines 162-230) iterates the whole map with `let _ = ctx;` — the request's region is never consulted — and `get_lifecycle_policy.rs`, `update_lifecycle_policy.rs`, `delete_lifecycle_policy.rs` and the three tag handlers likewise look policies up by id/ARN alone.
Convention in this repo: regional services partition state by the credential-scope region, e.g. `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/state.rs:2271` (`regions: HashMap<String, HashMap<String, Table>>`) and `services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs:508`.
Tests: `tests/all/lifecycle_policy_crud.rs::a_policy_arn_names_the_signed_region` (line 571) asserts only that the ARN carries the signed region; no test creates in one Region and reads from another, so nothing asserts isolation.

## Steps to Reproduce

1. POST /policies signed with a SigV4 credential scope in us-east-1, creating a policy; note the PolicyId and that PolicyArn names us-east-1
2. GET /policies signed with a credential scope in eu-west-1
3. Observe the us-east-1 policy in Policies (expected: an empty Policies list, since the policy does not exist in eu-west-1)
4. GET /policies/{PolicyId} signed for eu-west-1 and observe HTTP 200 with the policy (expected 404 ResourceNotFoundException)
5. DELETE /policies/{PolicyId} signed for eu-west-1 and observe HTTP 200, with the policy then gone from us-east-1 as well

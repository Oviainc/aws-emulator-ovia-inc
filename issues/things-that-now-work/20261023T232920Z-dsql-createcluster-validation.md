Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# CreateCluster attaches its inline `policy` without parsing it or running the policy-lockout safety check, and ignores `bypassPolicyLockoutSafetyCheck`

_Filed: 2026-10-23T23:29:20Z_
_Operation: CreateCluster | Class: validation_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateCluster models both `policy` (PolicyDocument, len 1..20480) and `bypassPolicyLockoutSafetyCheck` (boolean, default false). The bypass flag only exists because the lockout prevention check is applied when a policy is supplied at create time (model doc: "An optional field that controls whether to bypass the lockout prevention check. When set to true, this parameter allows you to apply a policy that might lock you out of the cluster").

Expected: CreateCluster with `policy` set must validate the document the same way PutClusterPolicy does — it must be a parseable JSON policy document, and with `bypassPolicyLockoutSafetyCheck` absent/false a document that denies every principal `dsql:PutClusterPolicy` on this cluster must be refused with HTTP 400 ValidationException and NO cluster/policy created.

Actual: the handler takes `input.policy` verbatim and attaches it with version 1 (`create_cluster.rs` lines 295-298). The document is never parsed, the lockout analysis in `put_cluster_policy.rs` (`locks_out_every_principal`) is never reached, and `input.bypass_policy_lockout_safety_check` is read only to build the idempotency fingerprint (`create_fingerprint`). So `CreateCluster(policy="not json")` and `CreateCluster(policy=<deny-* to Principal "*" for dsql:*>, bypassPolicyLockoutSafetyCheck=false)` both answer 200, and the unusable document is then reported verbatim by GetClusterPolicy — a policy real AWS would never have stored.

Second symptom of the same class on this operation: the model's CreateCluster documentation states "The witness Region specified in multiRegionProperties.witnessRegion cannot be the same as the cluster's Region." The handler stores `input.multi_region_properties` unchanged (lines 278) with no such check, so `CreateCluster(multiRegionProperties={witnessRegion: <the request's own region>})` answers 200 instead of 400 ValidationException.

## Evidence

Spec: CreateCluster input members `policy` and `bypassPolicyLockoutSafetyCheck` plus their documentation in the Smithy model (reproduced in `src/ops/create_cluster.rs` lines 71-74 and 91-92); the witness-region sentence in the same operation's docs (lines 66-67). ValidationException (HTTP 400) is a declared CreateCluster error. Handler: `src/ops/create_cluster.rs` lines 267-301 — no `parse_policy`, no lockout check, no witness-region check; `bypass_policy_lockout_safety_check` appears only in `create_fingerprint` (lines 314-323). The lockout check that DOES exist lives in `src/ops/put_cluster_policy.rs` lines 227-256 and is not shared. Tests: `tests/all/error_contract.rs::a_policy_that_denies_every_principal_the_policy_action_is_refused` and `tests/all/policy_version_conflict_and_lockout.rs` exercise PutClusterPolicy only; `tests/all/cluster_policy.rs::an_inline_policy_from_create_is_the_same_record_the_policy_operations_read` only asserts round-tripping of the create-time document. `grep bypassPolicyLockoutSafetyCheck tests/` shows no CreateCluster case. A sibling (already filed, fixed) issue confirms real DSQL validates policy documents at write time.

## Steps to Reproduce

1. POST /cluster with body {"deletionProtectionEnabled": false, "policy": "this is not json"} → emulator answers 200 and creates the cluster; GET /cluster/{id}/policy returns the non-JSON string. Expected 400 ValidationException (reason cannotParse) and no cluster.
2. POST /cluster with {"deletionProtectionEnabled": false, "bypassPolicyLockoutSafetyCheck": false, "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"dsql:*\",\"Resource\":\"*\"}]}"} → 200. The same document on POST /cluster/{id}/policy is refused 400. Expected: both refused.
3. POST /cluster (request signed for us-east-1) with {"deletionProtectionEnabled": false, "multiRegionProperties": {"witnessRegion": "us-east-1"}} → 200 and GetCluster echoes the witness region. Expected 400 ValidationException.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A clientToken replay answers with the cluster's CURRENT policy version instead of the version the original request created

_Filed: 2026-10-24T00:42:23Z_
_Operation: PutClusterPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`clientToken` is an `@idempotencyToken`: "if the original request completes successfully, the subsequent retries with the same client token return the result from the original successful request and they have no additional effect."

Expected: a replayed `PutClusterPolicy` (same token, same parameters) answers with the `policyVersion` the FIRST call returned, whatever happened to the cluster's policy since.

Actual: the replay branch in `put_cluster_policy.rs` (lines 201-212) ignores what the first call produced and reads the cluster's policy version as it stands NOW (`state.clusters.get(&identifier).policy.version`). The ledger records only the cluster identifier as the token's resource (line 308), so the original result is not retained. Concretely: put document A under token T → 200 policyVersion "1"; put document B with no token → 200 policyVersion "2"; replay put document A under token T → the emulator answers policyVersion "2", a version that the replayed request never produced and which belongs to a different document. (The simple replay case is correct and tested; only the case with an intervening write diverges.) A second symptom of the same class: when the policy has been removed since, the replay branch falls through and attaches a brand-new policy version, i.e. the retry has an additional effect the idempotency contract forbids.

## Evidence

Spec: `PutClusterPolicyInput.clientToken` carries `@idempotencyToken` with the quoted documentation (reproduced in `src/ops/put_cluster_policy.rs` header); the operation is also `@idempotent`. Handler: `src/ops/put_cluster_policy.rs` lines 196-225 (Replay reads the CURRENT `policy.version`; falls through to a fresh attach when `cluster.policy` is None) and line 305-309 (`record(token, fingerprint, &input.identifier)` stores the cluster id, not the version answered). Tests: `tests/all/cluster_policy.rs::a_put_policy_client_token_serves_one_request` replays with no intervening write, so the current version and the original version coincide there; no test performs a tokenless put between the first call and the replay, and no test replays a token after `DeleteClusterPolicy` on the put path.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false}; wait for ACTIVE.
2. POST /cluster/{id}/policy {"policy": <document A>, "clientToken": "T"} → 200 {"policyVersion": "1"}.
3. POST /cluster/{id}/policy {"policy": <document B>} (no token) → 200 {"policyVersion": "2"}.
4. Replay step 2 verbatim (same token, same document A) → emulator answers {"policyVersion": "2"}. Expected the original result, {"policyVersion": "1"}.
5. Variant: after step 2, DELETE /cluster/{id}/policy, then replay step 2 → emulator attaches a new policy version (an additional effect); expected the replay to answer the original result and attach nothing.

Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6441cf5b487e23ac4701fa61f14b24faef1eccc1

# The policy operations' clientToken fingerprint conflates an ABSENT expectedPolicyVersion with an empty one, so a token reused for a request the handler itself refuses replays 200

_Filed: 2026-10-28T17:54:47Z_
_Operation: DeleteClusterPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6441cf5b487e23ac4701fa61f14b24faef1eccc1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both operations that write a cluster's policy record their `clientToken` against a fingerprint that renders `expectedPolicyVersion` with `.unwrap_or_default()`, i.e. an ABSENT member and the empty string `""` render to the same part. The handlers themselves do NOT treat those two inputs alike: `input.expected_policy_version.as_deref()` being `Some("")` is a claim about version `""`, which no attached policy carries, so the handler answers `ConflictException` (409) — while `None` means no claim at all and the call proceeds. `PolicyVersion` is an unconstrained string in the model, so `""` is a legal value a client can send.

The effect is that a request the handler would refuse is answered 200 from the ledger: a `clientToken` first used WITHOUT `expectedPolicyVersion` is recognised as a retry of a later call that carries `expectedPolicyVersion: ""`, so `DeleteClusterPolicy` removes nothing and still answers 200 with the version the earlier call removed (the caller is told a policy version was deleted while the currently attached policy stays attached), and `PutClusterPolicy` attaches nothing and answers 200 with the earlier call's version instead of the 409 its own version check produces. An error is silently turned into a success, and the ledger's own rule — "the same token for a different delete is a conflict" — is not applied to a difference the handler can see.

The sibling instance is `PutClusterPolicy`, where `expectedPolicyVersion` is a BODY member, so `{"expectedPolicyVersion": ""}` deserializes unambiguously to `Some("")` and the path is reachable over the wire with certainty; `DeleteClusterPolicy` binds the same member to the `expected-policy-version` query parameter. This is the same class of defect as the already-filed `fingerprint_tags` ambiguity (`20261027T230801Z-dsql-createcluster-wrong-behavior--r3`), but in a different helper and on the policy operations' own fingerprints, which that issue does not touch.

## Evidence

The rule the crate states for its own fingerprints — `services/service-com-amazonaws-dsql-dsql/src/state.rs:638-647`:
```rust
pub fn request_fingerprint(parts: &[String]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // The length prefix keeps the parts unambiguous: without it
        // ["a", "bc"] and ["ab", "c"] hash alike.
```

The fingerprint that drops the distinction — `src/ops/delete_cluster_policy.rs:177-180`:
```rust
        let fingerprint = crate::state::request_fingerprint(&[
            input.identifier.clone(),
            input.expected_policy_version.clone().unwrap_or_default(),
        ]);
```
and the same rendering in the sibling writer — `src/ops/put_cluster_policy.rs:193-198`:
```rust
        let fingerprint = crate::state::request_fingerprint(&[
            input.identifier.clone(),
            input.policy.clone(),
            input.bypass_policy_lockout_safety_check.to_string(),
            input.expected_policy_version.clone().unwrap_or_default(),
        ]);
```

The SAME handler, distinguishing exactly the two values the fingerprint conflates — `src/ops/delete_cluster_policy.rs:248-257`:
```rust
        // `expectedPolicyVersion` is the caller's claim about what it is deleting:
        // a claim about another version must not delete this one.
        if let Some(expected) = input.expected_policy_version.as_deref() {
            if expected != attached {
                return Err(DeleteClusterPolicyErrorModeled::ConflictException(
                    crate::state::cluster_policy_version_conflict(&input.identifier, expected),
                )
                .into());
            }
        }
```
and `src/ops/put_cluster_policy.rs:271-287` (`Some(policy) if policy.version != expected => ... ConflictException`).

The replay that then answers instead of refusing — `src/ops/delete_cluster_policy.rs:186-208`: "// The version the FIRST delete removed, kept with the // token: the retry answers it instead of the 404 an // independent second delete gets, and removes nothing of its // own." → `return Ok(DeleteClusterPolicyOutput { policy_version: version.to_string() });`

The handler doc the code then breaks — `src/ops/delete_cluster_policy.rs:174-176`: "// `@idempotencyToken`: a retry of the SAME delete answers with the version // the first call removed instead of the 404 a second, independent delete // gets; the same token for a different delete is a conflict."

That `""` is a legal wire value: `smithy_query(query="shape", shape="PolicyVersion")` → `{"shape_id": "com.amazonaws.dsql#PolicyVersion", "type": "string"}` — no `@length`, no `@pattern`.

Test coverage: `tests/all/policy_version_conflict_and_lockout.rs` and `tests/all/cluster_policy.rs` drive `expectedPolicyVersion` present and absent, and `tests/all/idempotent_replay_result.rs` drives the policy replays, but no case pairs a token used without the member with one carrying it empty, so the collision is unpinned.

## Steps to Reproduce

1. Create a cluster and wait for the clock to settle it to ACTIVE (`harness::Dsql::create_settled`).
2. POST /cluster/{id}/policy with {"policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}", "clientToken": "tok-1"} -> 200 with policyVersion "1".
3. POST /cluster/{id}/policy again with the SAME policy and clientToken "tok-1" but adding "expectedPolicyVersion": "" — a claim about a version no policy carries, which the handler's own check at put_cluster_policy.rs:271-287 answers 409 ConflictException when no token is involved.
4. Observe HTTP 200 with policyVersion "1": `request_fingerprint` rendered `""` and the absent member identically, so the ledger reported Replay and the handler returned before its version check.
5. For the delete path: DELETE /cluster/{id}/policy?client-token=tok-2 -> 200 policyVersion "1"; attach a new policy (version "2"); then DELETE /cluster/{id}/policy?client-token=tok-2&expected-policy-version= -> 200 claiming policyVersion "1" was removed while GET /cluster/{id}/policy still reports version "2", instead of the conflict the handler's own check states.

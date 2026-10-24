Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# DeleteCluster's clientToken is not an idempotency ledger: reuse for another request succeeds, and a second delete's result is never recorded

_Filed: 2026-10-24T18:04:35Z_
_Operation: DeleteCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteCluster`'s `clientToken` is an `@idempotencyToken` (bound to the `client-token` query parameter) and the operation models `ConflictException` (409). Every OTHER token-carrying operation of this emulator (CreateCluster, UpdateCluster, PutClusterPolicy, DeleteClusterPolicy, CreateStream) runs the token through a `TokenLedger`, so a token replayed with the SAME parameters answers the first call's result and a token reused for a DIFFERENT request is refused 409 with the AWS sentence `Requests using a client token should supply identical request parameters` (`crate::state::CLIENT_TOKEN_MISMATCH_MESSAGE`). DeleteCluster alone has no ledger: it stashes the token on the cluster record (`Cluster.delete_client_token`) and only on the FIRST transition into DELETING.

Two observable divergences follow.

(a) Token reuse across requests is accepted. Expected: deleting cluster B with a token already used to delete cluster A is a different request wearing a used token, so it is refused 409 ConflictException and B does not start deleting. Actual: 200 with B in DELETING, and B is destroyed.

(b) A token-carrying delete of an already-DELETING cluster records nothing, so the same request answers 200 and then 404. Expected: an `@idempotent` retry of a request that succeeded answers the original result. Actual: the second delete takes the `cluster.status != DELETING` branch (delete_cluster.rs lines 239-247) and therefore never stores its own token; once the clock retires the cluster, the identical retry no longer matches `retired.delete_client_token` and gets 404 ResourceNotFoundException for a request that answered 200 moments earlier.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteCluster")` — `clientToken` (ClientToken, len 1..128, @idempotencyToken, @httpQuery(client-token)); errors ConflictException (409) and ResourceNotFoundException (404); `ClientToken`'s own doc: "Idempotency token so a request is only processed once", and the CreateCluster doc spells the semantics: "the subsequent retries with the same client token return the result from the original successful request and they have no additional effect".

Handler: `src/ops/delete_cluster.rs` lines 185-250 — the only idempotency state is `cluster.delete_client_token`, written inside `if cluster.status != STATUS_DELETING`; the retired-tombstone replay at lines 188-197 requires `retired.delete_client_token == input.client_token`. `src/state.rs` `TokenScope::ALL` lists five scopes (CreateCluster, UpdateCluster, PutClusterPolicy, DeleteClusterPolicy, CreateStream) and no DeleteCluster scope, so no fingerprint/mismatch check exists for this operation.

Tests: `tests/all/cluster_lifecycle.rs` `a_retried_delete_answers_from_the_retired_cluster` (lines 841-893) pins only the same-token retry (200) and the no-token delete after retirement (404). A grep of `tests/` for `client-token` shows no case that reuses one token across two DeleteCluster requests, and none that deletes an already-DELETING cluster under a second token — both behaviors are unpinned.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false} twice -> cluster A and cluster B; wait for both to settle to ACTIVE.
2. DELETE /cluster/{A}?client-token=tok-1 -> 200, status DELETING.
3. DELETE /cluster/{B}?client-token=tok-1 -> emulator answers 200 and starts deleting B; expected 409 ConflictException (the token names the first request), with B untouched.
4. Create cluster C (deletionProtectionEnabled=false), settle, then DELETE /cluster/{C}?client-token=tok-a -> 200 DELETING.
5. Immediately DELETE /cluster/{C}?client-token=tok-b -> 200 DELETING (nothing recorded for tok-b).
6. Wait for the clock to retire C, then retry DELETE /cluster/{C}?client-token=tok-b -> emulator answers 404 ResourceNotFoundException; expected the recorded result of the request that already answered 200.

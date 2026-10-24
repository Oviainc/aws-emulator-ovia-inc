Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# DeleteWorkload ignores ClientRequestToken, so a retried delete answers 404 instead of replaying the original success

_Filed: 2026-10-24T04:57:31Z_
_Operation: DeleteWorkload | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteWorkloadInput$ClientRequestToken` is modelled `@idempotencyToken` and `@required` (bound as the `ClientRequestToken` query parameter), and the `ClientRequestToken` shape documents the rule: "A unique case-sensitive string used to ensure that this request is idempotent (executes only once) ... If you retry a request with the same client request token and the same parameters after the original request has completed successfully, the result of the original request is returned."

The handler accepts the member and throws it away (`let _ = &input.client_request_token;`), keying nothing to it. Expected: a second `DELETE /workloads/{WorkloadId}?ClientRequestToken=<same token>` after a successful delete replays the original result — HTTP 200 with the empty `DeleteWorkload` body — because the token says the request has already executed exactly once. Actual: the workload id no longer resolves, so the retry is answered `404 ResourceNotFoundException`. SDK/CLI retries and at-least-once delivery therefore surface a spurious 404 for a delete that in fact succeeded.

Every OTHER idempotent operation in this crate does remember its token (`CreateWorkload`, `CreateMilestone`, `CreateWorkloadShare`, `CreateReviewTemplate`, `ImportLens`, `CreateLensVersion`, `CreateAgentProfile`, `CreateAgentGoal` all consult and write a `TokenLedger`), so `DeleteWorkload` is the one implemented operation whose documented token has no effect at all. A ledger entry for a deleted workload id would also need to survive `remove_workload`, which today deliberately drops `create_workload_tokens` for the removed workload.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteWorkload")` shows `ClientRequestToken` with `idempotencyToken: true, required: true, httpQuery: "ClientRequestToken"`; `smithy_query(query="shape", shape="ClientRequestToken")` carries the "the result of the original request is returned" sentence. Handler: `src/ops/delete_workload.rs` lines 183-194 — the comment states outright that "`ClientRequestToken` is accepted (the model requires it) but nothing is keyed to it ... so a repeat delete is a 404 rather than a replayed 200", then `store.remove_workload(..)` returns `ResourceNotFoundException` on the second call. Test coverage: `tests/all/workload_lifecycle.rs` and `tests/all/token_ledgers.rs` pin the replay behaviour of the CREATE operations only; no test asserts anything about repeating a delete under the same token, so nothing pins the current 404.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. CreateWorkload with WorkloadName="retry-delete", Description, Environment=PRODUCTION, ReviewOwner, AwsRegions=["us-west-2"], Lenses=["wellarchitected"] and note the returned WorkloadId
3. Call DELETE /workloads/{WorkloadId}?ClientRequestToken=tok-delete-1 and observe HTTP 200
4. Call the identical request again (same WorkloadId, same ClientRequestToken=tok-delete-1)
5. Observe HTTP 404 ResourceNotFoundException; expected the original result replayed (HTTP 200, empty body) because the token marks the request as already executed
6. Contrast with DELETE using a DIFFERENT ClientRequestToken, which is a genuinely new request and should still be a 404

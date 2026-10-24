Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# DeleteReviewTemplate ignores ClientRequestToken, so a retried delete answers 404 instead of replaying the original success

_Filed: 2026-10-24T15:17:52Z_
_Operation: DeleteReviewTemplate | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteReviewTemplateInput$ClientRequestToken` is modelled `@idempotencyToken` and `@required` (bound as the `ClientRequestToken` query parameter), and the `ClientRequestToken` shape documents the rule: "A unique case-sensitive string used to ensure that this request is idempotent (executes only once) ... If you retry a request with the same client request token and the same parameters after the original request has completed successfully, the result of the original request is returned."

The handler accepts the member and throws it away (`let _ = &input.client_request_token;`), keying nothing to it.

Expected: a second `DELETE /reviewTemplates/{TemplateArn}?ClientRequestToken=<same token>` after a successful delete replays the original result — HTTP 200 with the empty `DeleteReviewTemplate` body — because the token says the request has already executed exactly once. Actual: the template ARN no longer resolves, so the retry is answered HTTP 404 `ResourceNotFoundException`. SDK/CLI retries and at-least-once delivery therefore surface a spurious 404 for a delete that in fact succeeded.

This is the same divergence already filed for `DeleteWorkload` (`20261024T045731Z-wellarchitected-deleteworkload-parameter-ignored`), on the other implemented delete of this crate; every CREATE operation here does consult and write a `TokenLedger`, so the two deletes are the only implemented operations whose documented token has no effect. A ledger entry would also have to survive `remove_review_template`, which today drops the template's `create_review_template_tokens` entry.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteReviewTemplate")` shows `ClientRequestToken` with `idempotencyToken: true, required: true, httpQuery: "ClientRequestToken"`; `smithy_query(query="shape", shape="ClientRequestToken")` carries the "the result of the original request is returned" sentence.

Emulator: `src/ops/delete_review_template.rs` lines 189-207 — the comment states outright that "`ClientRequestToken` is accepted (the model requires it) but nothing is keyed to it, exactly as `DeleteWorkload` treats its own ... so a repeat delete is the same 404 any other unknown ARN gets", then `store.remove_review_template(..)` returns `ResourceNotFoundException` on the second call.

Test coverage: `tests/all/review_template_lifecycle.rs` asserts the create/read/delete path and that a deleted ARN 404s for READS; no test repeats the delete under the same token, so nothing pins the current 404 for the retry.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. POST /reviewTemplates (CreateReviewTemplate) with TemplateName="retry-delete", Description, Lenses=["wellarchitected"], ClientRequestToken=tok-create-1; note the returned TemplateArn
3. Call DELETE /reviewTemplates/{TemplateArn}?ClientRequestToken=tok-delete-1 and observe HTTP 200
4. Call the identical request again (same TemplateArn, same ClientRequestToken=tok-delete-1)
5. Observe HTTP 404 ResourceNotFoundException; expected the original result replayed (HTTP 200, empty body) because the token marks the request as already executed
6. Contrast with a DELETE using a DIFFERENT ClientRequestToken, which is a genuinely new request and should still be a 404

Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0ab5854f8833828180776b987700bd810a961abd

# A clientToken that matches a previous request is answered with a 409 ConflictException, which the member's own documentation says must not happen

_Filed: 2026-10-29T05:50:20Z_
_Operation: PutComplianceInquiryFeedback | Class: error-response_

> _Note: this issue was identified against the code base at commit `0ab5854f8833828180776b987700bd810a961abd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutComplianceInquiryFeedbackRequest$clientToken` is documented in this service's Smithy model as: "A unique, case-sensitive identifier to ensure that the operation completes no more than one time. If this token matches a previous request, the service ignores the request, but does not return an error."

The handler honours that sentence only when the repeat's request fingerprint is identical. When the same token arrives on a submission that differs in any member (a different `responseRevisionId`, a different `rating`, a different `comment`, a different `queryIdentifier`, another inquiry), it answers the modelled `ConflictException` (HTTP 409) and records nothing — i.e. for a token that *does* match a previous request the service returns an error, which is exactly what the quoted sentence excludes.

The handler is self-aware about this: it QUOTES the sentence in the branch above to justify the replay, and then returns the error ~30 lines later for the same precondition ("this token matches a previous request"). The effect is that a feedback submission against a response version nothing has rated yet — one the per-version rule would accept, as the sibling branch and the file's own doc state — is lost with a 409 purely because the caller reused a token. The per-version `ConflictException` ("Feedback already submitted for this response version.") is a separate, earlier check and is not what this issue is about: it stays as it is.

Note that `CreateComplianceInquiryRequest$clientToken` carries no such sentence (its doc is only "Idempotency token for the request."), so the same refusal in `CreateComplianceInquiry` is not contradicted by the model and is out of scope here.

## Evidence

Smithy model, `PutComplianceInquiryFeedbackRequest$clientToken` (smithy_query doc, services/service-com-amazonaws-artifact-artifact/smithy-model/com-amazonaws-artifact-artifact.json):
  "A unique, case-sensitive identifier to ensure that the operation completes no more than one time. If this token matches a previous request, the service ignores the request, but does not return an error."

The handler quotes that very sentence for the replay branch — services/service-com-amazonaws-artifact-artifact/src/ops/put_compliance_inquiry_feedback.rs:265-276:
```
        if let Some(record) = &remembered {
            if record.fingerprint == fingerprint {
                // "If this token matches a previous request, the service ignores the
                // request, but does not return an error": nothing further is recorded and
                // the answer is the one the first call gave, read off the remembered
                // record. ...
                return Ok(PutComplianceInquiryFeedbackOutput {
                    submitted_at: ::aws_smithy_types::DateTime::from(record.submitted_at),
                });
```
and then returns an error for a token that also matches a previous request — same file, lines 301-316:
```
        // The idempotency layer on top of it: this token identifies ONE call, and this
        // request is not that call, so honouring it would either hide this submission or
        // answer with another one's timestamp.
        if let Some(record) = &remembered {
            ...
            return Err(PutComplianceInquiryFeedbackErrorModeled::ConflictException(
                crate::state::feedback_token_mismatch(&record.inquiry_id),
            )
            .into());
        }
```
The same file's module doc states the divergence in its own words — put_compliance_inquiry_feedback.rs:27-32:
```
//! * `clientToken` is this operation's `@idempotencyToken`, the replay layer ON TOP of that
//!   rule: "If this token matches a previous request, the service ignores the request, but
//!   does not return an error" — so a repeat of one request records nothing further and
//!   answers the `submittedAt` the first call answered, and the same token on a request
//!   targeting something else is refused with the modelled `ConflictException` ...
```
The refusal payload is services/service-com-amazonaws-artifact-artifact/src/state.rs:2380-2388 (`feedback_token_mismatch`, message "the clientToken was already used to submit different feedback on a compliance inquiry").

Test coverage that pins the current behaviour (its stated reason is "the idempotency rule", not the model sentence, which contradicts it) — services/service-com-amazonaws-artifact-artifact/tests/all/compliance_inquiry_feedback.rs:538-569:
```
async fn the_same_client_token_on_a_different_submission_is_refused_with_a_conflict() {
    ...
    // One row per member a second request can differ in — each the only difference —
    // every one of them targeting a response version NOTHING has rated yet, so what is
    // under test is the idempotency rule alone ...: these requests would be recorded if
    // they carried a token of their own.
```
and the replay side at compliance_inquiry_feedback.rs:473-535 (`a_repeated_submission_under_one_client_token_replays_the_first_answer`), which stays unchanged.

## Steps to Reproduce

1. Boot the emulator and POST /v1/compliance-inquiry/create with {"name":"t","inquiryContent":{"query":"Is S3 in scope?"}} — note the returned complianceInquiryId (it owns queryIdentifier 1).
2. PUT /v1/compliance-inquiry/putFeedback with {"complianceInquiryId":"<id>","queryIdentifier":1,"responseRevisionId":1,"rating":"THUMBS_UP","clientToken":"token-reused"} — observe 200 with submittedAt.
3. PUT /v1/compliance-inquiry/putFeedback again with the SAME clientToken but a submission targeting a version nothing has rated: {"complianceInquiryId":"<id>","queryIdentifier":1,"responseRevisionId":2,"rating":"THUMBS_DOWN","clientToken":"token-reused"}.
4. Observe HTTP 409 ConflictException with message "the clientToken was already used to submit different feedback on a compliance inquiry". The model's clientToken sentence says a token that matches a previous request makes the service ignore the request and NOT return an error.
5. Confirm nothing was recorded: POST /v1/compliance-inquiry/export for the same inquiry and fetch the announced documentPresignedUrl — the document's "feedback" array still holds only the first submission.

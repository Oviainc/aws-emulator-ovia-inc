Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# A replayed clientToken is never matched against the request, so reusing a token for a different agreement answers with the first agreement's cancellation request and stores nothing

_Filed: 2026-10-24T13:27:19Z_
_Operation: SendAgreementCancellationRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SendAgreementCancellationRequestInput$clientToken is @idempotencyToken ("a unique, case-sensitive identifier that you provide to ensure the idempotency of the request"). Idempotency means a REPLAY of the same request answers with the record the first call created; a token reused for a DIFFERENT request is not a replay. Expected: a second SendAgreementCancellationRequest carrying the same clientToken but a different agreementId (or a different reasonCode/description) is refused — this operation models ConflictException (HTTP 409, "Request was denied due to a resource conflict") and the service models ValidationExceptionReason INVALID_CLIENT_TOKEN for exactly this member — and must certainly not report a cancellation request that belongs to another agreement.

Actual: the handler resolves the token to a stored id and returns `to_output(stored)` with no comparison at all against the current input. So:

* `SendAgreementCancellationRequest(agreementId=A, reasonCode=TEST_AGREEMENT, clientToken=T)` then `SendAgreementCancellationRequest(agreementId=B, reasonCode=OTHER, clientToken=T)` answers 200 with `{agreementId: A, agreementCancellationRequestId: acr-…(A's), reasonCode: TEST_AGREEMENT}` — a response about a resource the caller did not name — and NO cancellation request is ever created for agreement B. The caller sees a success and believes B is pending cancellation.
* Because the replay is resolved BEFORE the agreement is looked up and before the ACTIVE check, a replayed token also answers 200 for an agreement that is no longer active (or was never stored), bypassing the INACTIVE_AGREEMENT / ResourceNotFoundException refusals the same handler makes on a first call.

The sibling creates in this same crate already implement the correct rule — SendAgreementPaymentRequest compares `request.matches_send(&input)` and raises ConflictException on a mismatch, and BatchCreateBillingAdjustmentRequest compares `stored.matches_entry(entry)` and reports a per-entry CONFLICT_EXCEPTION — so the cancellation path is the odd one out.

## Evidence

Spec: model marks SendAgreementCancellationRequestInput$clientToken @idempotencyToken; the operation declares ConflictException (409) and ValidationException (400); ValidationExceptionReason includes INVALID_CLIENT_TOKEN (smithy_query shape=ValidationExceptionReason). Handler: src/ops/send_agreement_cancellation_request.rs lines ~201-224 — `let replayed = token…and_then(|id| store.agreement_cancellation_requests.get(&id)); if let Some(stored) = replayed { return Ok(to_output(stored)); }` — returns before the `store.agreements.get(&input.agreement_id)` lookup and the `agreement.status != STATUS_ACTIVE` check, and never compares input.agreement_id / input.reason_code / input.description with the stored record. Compare src/state.rs PaymentRequest::matches_send() and BillingAdjustmentRequest::matches_entry(), which exist for exactly this check. Tests: tests/all/agreement_cancellation_requests.rs:245 `a_replayed_client_token_sends_one_cancellation_request` replays the IDENTICAL request only; no test exercises a token reused with different details, so the gap is untested rather than pinned.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"t1"}]) then AcceptAgreementRequest -> agreement A
2. Repeat step 1 -> a second agreement B
3. SendAgreementCancellationRequest(agreementId=A, reasonCode="TEST_AGREEMENT", clientToken="tok-1") -> 200, note acr-<A>
4. SendAgreementCancellationRequest(agreementId=B, reasonCode="OTHER", description="different", clientToken="tok-1") -> observe 200 echoing agreementId=A and acr-<A> (expected a 409 ConflictException / 400 ValidationException for the reused token, or a new request for B)
5. GetAgreementCancellationRequest(agreementId=B, agreementCancellationRequestId=acr-<A>) -> 404, and ListAgreementCancellationRequests(partyType="Acceptor", agreementId=B) -> empty: nothing was stored for B although the caller was told the send succeeded
6. CancelAgreement(agreementId=A) to leave A non-ACTIVE, then SendAgreementCancellationRequest(agreementId=A, reasonCode="OTHER", clientToken="tok-1") -> observe 200 instead of the ValidationException/INACTIVE_AGREEMENT a first call would get

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# A clientToken replayed with DIFFERENT request details silently returns the first quote instead of being refused

_Filed: 2026-10-24T13:27:19Z_
_Operation: CreateAgreementRequest | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`clientToken` is modelled @idempotencyToken ("a unique, case-sensitive identifier that you provide to ensure the idempotency of the request"). Expected: the token makes a RETRY of the same request return the same quote, while a token reused for a different request (different `intent`, `requestedTerms`, `sourceAgreementIdentifier`, `agreementProposalIdentifier` or `taxConfiguration`) is refused — CreateAgreementRequest models ConflictException (HTTP 409, "Request was denied due to a resource conflict") and the service models ValidationExceptionReason INVALID_CLIENT_TOKEN for this member.

Actual: the handler looks the token up in `store.idempotency`, loads the agreement request it named, and immediately returns that record's `agreementRequestId` and `chargeSummary` without comparing a single member of the current input. So a caller that changes the requested terms but forgets to rotate the token is answered 200 with the quote for the OLD terms, and the new terms are never priced or persisted. The wrong `chargeSummary` is then carried forward verbatim: AcceptAgreementRequest on that id mints an agreement whose accepted terms are the first call's, so the divergence is not just a stale response but a wrongly-constituted agreement.

The correct rule is already implemented twice in this crate — SendAgreementPaymentRequest compares `request.matches_send(&input)` and answers ConflictException on a mismatch, and BatchCreateBillingAdjustmentRequest compares `stored.matches_entry(entry)` and reports a per-entry CONFLICT_EXCEPTION — so CreateAgreementRequest is inconsistent with its own siblings. (Related code-hygiene note: this handler stores the RAW token in `store.idempotency` while SendAgreementCancellationRequest stores `state::idempotency_key(Self::NAME, token)`, so the one map holds two key formats even though its own doc comment says the operation name is part of the key.)

## Evidence

Spec: CreateAgreementRequestInput$clientToken @idempotencyToken; operation declares ConflictException (409) and ValidationException (400); ValidationExceptionReason includes INVALID_CLIENT_TOKEN (smithy_query shape=ValidationExceptionReason). Handler: src/ops/create_agreement_request.rs lines ~236-247 — `let replayed = input.client_token…and_then(|id| store.agreement_requests.get(&id)).map(|request| (request.id.clone(), request.charge_summary.clone())); if let Some((id, charge_summary)) = replayed { return Ok(...) }` — no comparison against input.intent / input.requested_terms / input.source_agreement_identifier. Contrast src/state.rs PaymentRequest::matches_send() and BillingAdjustmentRequest::matches_entry(). Tests: tests/all/agreement_lifecycle.rs replays an identical create only; no test sends the same clientToken with different requestedTerms.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", clientToken="tok-1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:5}]}}}]) -> 200, note ar-X and its chargeSummary
2. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer2", clientToken="tok-1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P36M", dimensions:[{dimensionKey:"Users", dimensionValue:500}]}}}]) -> observe 200 returning ar-X and the FIRST call's chargeSummary (expected 409 ConflictException or 400 ValidationException reason INVALID_CLIENT_TOKEN)
3. AcceptAgreementRequest(agreementRequestId=ar-X) then GetAgreementTerms(agreementId=<new id>) -> observe the 5-user/P12M terms, not the 500-user/P36M terms the second call asked to price
4. Also try the same token with intent="REPLACE" + sourceAgreementIdentifier=<existing agreement> -> observe 200 with the original NEW quote instead of a refusal

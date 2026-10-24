Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f3e823b77ae5163683e789c9c079862bba02cf9

# Reusing a clientToken with different input silently replays the first quote instead of a conflict

_Filed: 2026-10-24T19:28:26Z_
_Operation: CreateAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f3e823b77ae5163683e789c9c079862bba02cf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAgreementRequestInput$clientToken` is modeled `@idempotencyToken`. The emulator replays on the token ALONE: it looks the token up, finds the stored agreement request and answers with that request's id and `chargeSummary` without comparing the replayed call's `intent`, `requestedTerms`, `sourceAgreementIdentifier`, `agreementProposalIdentifier` or `taxConfiguration` to what the first call stored.

Expected: an idempotency token reused with DIFFERENT request content is a conflict — the modeled `ConflictException` (HTTP 409) that this operation declares — not an answer about some other quote. This is the rule the same crate already implements for its sibling idempotent operations.
Actual: HTTP 200 carrying the first quote's `agreementRequestId` and `chargeSummary`, so a caller that changed its requested terms (and would be charged a different amount) is told the old quote was created for the new terms, and the new terms are never priced or stored.

## Evidence

Spec: `clientToken` is `@idempotencyToken` on `CreateAgreementRequestInput`; `CreateAgreementRequest` declares `ConflictException (HTTP 409)` "Request was denied due to a resource conflict" (OPERATIONS.md, CreateAgreementRequest).
Handler: `src/ops/create_agreement_request.rs` lines 236-247 — the replay path maps the token to `(request.id, request.charge_summary)` and returns it with no comparison of the input.
Contrast inside this same crate: `src/ops/send_agreement_payment_request.rs` lines 196-218 returns `ConflictException` when `!request.matches_send(&input)` (helper `PaymentRequest::matches_send`, src/state.rs ~1396), and `src/ops/batch_create_billing_adjustment_request.rs` lines 236-259 pushes a CONFLICT per-entry error when `!stored.matches_entry(entry)` — so the service's own convention is that a token reused for different details is a conflict.
Tests: `tests/all/agreement_lifecycle.rs:522-557` replays the IDENTICAL body only, and checks a different token mints a new quote; the mismatched-details case is untested.

## Steps to Reproduce

1. Call CreateAgreementRequest with {"intent":"NEW","agreementProposalIdentifier":"ap-offer1","clientToken":"tok-1","requestedTerms":[<a configurable upfront term for 3 units>]} and note agreementRequestId + chargeSummary.
2. Call CreateAgreementRequest again with clientToken "tok-1" but requestedTerms for 30 units (or a different intent).
3. Observe HTTP 200 returning the FIRST agreementRequestId and the first chargeSummary; expected HTTP 409 ConflictException because the token was reused with different content.
4. Note that no second agreement request is stored, so the 30-unit terms are never priced.

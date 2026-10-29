Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# ListBillingAdjustmentRequests judges the nextToken before it resolves agreementId, the reverse of its three sibling scanning listings

_Filed: 2026-10-29T09:17:50Z_
_Operation: ListBillingAdjustmentRequests | Class: error-response_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListBillingAdjustmentRequests` parses and refuses its `nextToken` BEFORE it takes the store lock and resolves the optional `agreementId` selector. The page-token scope is built FROM that agreement id (`billing_adjustment_scope(Self::NAME, input.agreement_id, …)`), so no token this service ever minted can validate against the scope of an agreement the store does not hold. Consequently, whenever a request names an unknown `agreementId` AND carries any `nextToken`, the handler answers `ValidationException` with reason `INVALID_NEXT_TOKEN` naming member `nextToken`, and the `unknown_agreement_refusal` branch below it (reason `INVALID_AGREEMENT_ID`, member `agreementId`) is unreachable — the caller is told to repair `nextToken` when the member actually at fault is `agreementId`.

The three sibling scanning listings of this same crate do it the other way round: each takes the lock, settles, resolves `agreementId` through the shared `store.unknown_agreement_refusal`, and only THEN builds the scope and decodes the token. So the identical pair of faults (unknown agreement + a token) is reported under two different `reason` values depending on which of the four listings was called, although `state.rs` documents `unknown_agreement_refusal` as the one refusal "the sibling listings of this service answer the same member with". This is the same ordering defect already filed for `ListAgreementInvoiceLineItems`, on a second operation.

## Evidence

Token refused before the store is read — `src/ops/list_billing_adjustment_requests.rs:200-227`:
```
200        // The token resumes THIS listing: one minted for a different set of
201        // filters would index into a different sequence.
202        let scope = crate::state::billing_adjustment_scope(
203            Self::NAME,
204            input.agreement_id.as_deref(),
…
211        let offset = match input.next_token.as_deref() {
212            None => 0,
213            Some(token) => match crate::state::page_offset(&scope, token) {
214                Some(offset) => offset,
215                None => {
216                    return Err(
217                        ListBillingAdjustmentRequestsErrorModeled::ValidationException(
218                            crate::state::validation_exception(
219                                "INVALID_NEXT_TOKEN",
220                                "nextToken",
```
and only afterwards — `:229-248`:
```
229        let now = ::std::time::SystemTime::now();
230        let mut store = state.lock();
…
237        store.settle_agreements(now);
…
244        if let Some(refusal) = store.unknown_agreement_refusal(input.agreement_id.as_deref()) {
245            return Err(
246                ListBillingAdjustmentRequestsErrorModeled::ValidationException(refusal).into(),
```

The scope is keyed by the agreement id, which is what makes the branch at :244 unreachable with any token — `src/state.rs:3179-3199`: `pub fn billing_adjustment_scope(operation: &str, agreement_id: Option<&str>, …) -> String { listing_scope(operation, &[agreement_id.unwrap_or_default(), status.unwrap_or_default(), …]) }`.

The three siblings resolve the agreement FIRST:
* `src/ops/list_agreement_charges.rs:198-210` — `let mut store = state.lock(); store.settle_agreements(now);` then `if let Some(refusal) = store.unknown_agreement_refusal(input.agreement_id.as_deref()) {` — with the scope/token only at `:248-267`.
* `src/ops/list_agreement_payment_requests.rs:206-218` (`store.unknown_agreement_refusal(...)`) with the scope/token at `:273-281`.
* `src/ops/list_agreement_cancellation_requests.rs:211-223` (`store.unknown_agreement_refusal(...)`) with the scope/token at `:282-290`.

The crate's own single statement of that refusal — `src/state.rs:4681-4697`: "an id no agreement of this account carries is not a filter that selects nothing, so a caller can tell 'this agreement has no such records' from 'this agreement does not exist'. The listing operations model no `ResourceNotFoundException`, so the refusal is their modelled `ValidationException` carrying the `INVALID_AGREEMENT_ID` reason."

Nothing pins the current order: `tests/all/billing_adjustments.rs:935-986` (`a_listing_agreement_id_must_name_a_stored_agreement`) drives five unknown-`agreementId` rows — "a short id", "an absent id beside a status filter", "an absent id beside a page size" — and asserts `answer.reason() == "INVALID_AGREEMENT_ID"`, but NONE of them carries a `nextToken`; the token suite at `tests/all/billing_adjustments.rs:475` only drives bad tokens against an agreement that exists.

## Steps to Reproduce

1. Accept an agreement request so an agreement A exists with an invoice, and submit two billing adjustment requests against it (tests/all/billing_adjustments.rs::invoiced_agreement + submit_all).
2. POST ListBillingAdjustmentRequests {"agreementId": A, "maxResults": 1} and keep the returned nextToken T.
3. POST ListBillingAdjustmentRequests {"agreementId": "agmt-absent", "nextToken": T}.
4. Observed: 400 ValidationException with reason INVALID_NEXT_TOKEN naming member nextToken. Expected (as the same call without nextToken answers, per tests/all/billing_adjustments.rs:980-985): reason INVALID_AGREEMENT_ID naming member agreementId.
5. Compare ListAgreementCharges {"agreementId": "agmt-absent", "nextToken": <any>}: it answers INVALID_AGREEMENT_ID, so two sibling scanning listings disagree about the same pair of faults.

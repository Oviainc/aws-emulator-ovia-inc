Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55

# BatchCreateBillingAdjustmentRequest resolves each entry's agreementId and originalInvoiceId without settling the store, so an adjustment against a renewal's invoice is refused as not found

_Filed: 2026-10-29T07:12:42Z_
_Operation: BatchCreateBillingAdjustmentRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each entry of the batch is resolved against `store.agreements` and then against `store.invoice_of(...)`, but the handler never materialises the end-date transitions the clock has already made due, unlike the sibling `ListBillingAdjustmentRequests`, which resolves the SAME `agreementId` member and calls `store.settle_agreements(now)` first with a comment stating exactly why. A renewal agreement and the invoices its quote bills are created by that settling (`renew_agreement` → `issue_acceptance_invoices`), so a batch naming a renewal's agreement/invoice as the first call on an un-settled store is answered 200 with per-entry `RESOURCE_NOT_FOUND_EXCEPTION` errors and nothing created, while the identical batch sent after any sibling listing or search succeeds. A write is therefore refused for a resource the clock already created, and whether it is refused depends on whether an unrelated call looked at the store first.

## Evidence

No settling anywhere in the handler — `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/batch_create_billing_adjustment_request.rs:234-235`: `let now = ::std::time::SystemTime::now(); let mut store = state.lock();` followed directly by the per-entry loop, which resolves the parent at :272-284: `let Some((agreement_type, catalog)) = store.agreements.get(&entry.agreement_id) … else { errors.push(BatchCreateBillingAdjustmentError { code: crate::state::BILLING_ADJUSTMENT_ERROR_NOT_FOUND.to_string(), …` and the invoice at :306-317: `let Some(invoice) = store.invoice_of(&entry.agreement_id, &entry.original_invoice_id)`.

Sibling resolving the same member DOES settle — `src/ops/list_billing_adjustment_requests.rs:232-244`: "The `agreementId` selector below is resolved against every agreement the store holds, so the end-date transitions already due are materialised over the whole store first — an agreement an end date created exists because that date passed, not because some unrelated call read it first." followed by `store.settle_agreements(now);` and `if let Some(refusal) = store.unknown_agreement_refusal(input.agreement_id.as_deref())`.

The renewal and its invoices only exist once settling runs — `src/state.rs:4982-5007`: `self.agreements.insert(id.clone(), Agreement { … })` then `let _ = self.issue_acceptance_invoices(&id, end);` inside `renew_agreement`; `src/state.rs:4875-4883` (`settle_agreements`): "the renewal agreement its end date created are then the ones the clock already owed".

Test coverage: `tests/all/billing_adjustments.rs` exercises adjustments only against an agreement the caller just accepted, and `tests/all/unsettled_store_listings.rs:160-176` drives the un-settled-store rule for four listings only — no test sends a batch naming a renewal the clock owes.

## Steps to Reproduce

1. Create and accept an agreement request for a one-month prepaid purchase with auto-renewal enabled, so its acceptance issues an invoice and its end date renews it.
2. Wind the stored agreement's window back past its end date (`tests/all/agreement_expiry.rs::aged_agreement`), so one renewal — with its own quote, charges and invoices — is due by the clock.
3. On a store no other call has read, send BatchCreateBillingAdjustmentRequest with one entry whose `agreementId` is the renewal agreement's id and whose `originalInvoiceId` is the invoice its renewal issued (both minted from the store counter, so the identical setup mints identical ids).
4. Observe HTTP 200 with `items` empty and an `errors` entry carrying `RESOURCE_NOT_FOUND_EXCEPTION` for the agreement, so no adjustment is created.
5. Call ListBillingAdjustmentRequests (or ListAgreementCharges/SearchAgreements) once, which settles the whole store, then resend the identical batch.
6. Observe the same batch now creating the adjustment: the write's outcome depends on whether an unrelated call looked first.

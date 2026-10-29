Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55

# ListAgreementInvoiceLineItems resolves its agreementId without settling the store, so a renewal the clock already created and the invoices it issued are answered as not found

_Filed: 2026-10-29T07:12:42Z_
_Operation: ListAgreementInvoiceLineItems | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9e55f581c86f2cc2a9694ee1b8e2f61584dc3f55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This handler resolves its `agreementId` against `store.agreements` and reports the invoices of that agreement, but it is the only listing of this crate that resolves an `agreementId` against the store WITHOUT first materialising the end-date transitions the clock has already made due (`crate::state::settle_agreements`). Its four sibling listings that resolve the same member all call it first, and the renewal agreement an end date creates is created together with the invoices its own quote bills on acceptance (`issue_acceptance_invoices` inside `renew_agreement`). The result is that this listing's answer depends on whether some unrelated call looked at the store first: as the first call after a renewal became due it answers the not-found refusal for the renewal agreement (HTTP 400 carrying `ResourceNotFoundException`, `crate::state::NOT_FOUND_STATUS`), and after any sibling listing or search has run, the very same request answers 200 with that renewal's invoice group. The crate's own test module for this rule states it explicitly and enumerates four listings, omitting this one.

## Evidence

Handler takes a READ-ONLY lock and resolves the member with no settling — `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/list_agreement_invoice_line_items.rs:222`: `let store = state.lock();` then :229-235: `if !store.agreements.contains_key(&input.agreement_id) { return Err(crate::state::not_found( ListAgreementInvoiceLineItemsErrorModeled::ResourceNotFoundException( crate::state::agreement_not_found(&input.agreement_id),`.

Sibling listing resolving the same member, `src/ops/list_agreement_charges.rs:197-199`: `let now = ::std::time::SystemTime::now(); let mut store = state.lock(); store.settle_agreements(now);` with the rule in its comment at :185-196: "A report, of the agreements as the clock has already left them: one past its own stored end date reached that date whether or not anybody listed … This report is over every agreement rather than over one, so it is the whole store that is brought up to date, which is what makes the charges of a renewal the clock already owed part of this answer and the renewal's own id one this listing holds an agreement for, whether or not some unrelated call read something else first." The same call is made by `src/ops/list_agreement_payment_requests.rs:207`, `src/ops/list_agreement_cancellation_requests.rs:212` and `src/ops/list_billing_adjustment_requests.rs:237`.

The renewal's invoices are created by that settling — `src/state.rs:5007`: `let _ = self.issue_acceptance_invoices(&id, end);` inside `renew_agreement`, whose doc at :4916-4918 says the renewal gets "its OWN quote … the invoices that quote bills on acceptance ([`Self::issue_acceptance_invoices`])".

`src/state.rs:4875-4883` (`settle_agreements` doc): "What an operation that SCANS the store calls, where the answer depends on every agreement rather than on one: the status an agreement past its own end date is searched by, the summary status reported beside it, and the renewal agreement its end date created are then the ones the clock already owed."

Test coverage states the rule and omits this operation — `tests/all/unsettled_store_listings.rs:10-13`: "The rule under test: an operation whose answer is over every agreement the store holds reports it as the clock has already left it, so the answer never depends on whether some unrelated call looked first", and `whole_store_listings` at :160-176 lists only `ListAgreementCharges`, `ListAgreementPaymentRequests`, `ListAgreementCancellationRequests` and `ListBillingAdjustmentRequests`; `the_renewal_the_clock_owes_is_an_agreement_these_listings_hold` (:321) asserts each of those four answers 200 for the renewal when it is the FIRST call the store sees. No test drives `ListAgreementInvoiceLineItems` on an unsettled store.

## Steps to Reproduce

1. Create and accept an agreement request whose terms are a one-month prepaid purchase with a renewal term enabling auto-renewal (`tests/all/agreement_renewals.rs::renewing_terms("P1M")`).
2. Wind the stored agreement's window back past its end date, as `tests/all/agreement_expiry.rs::aged_agreement` does, so one renewal is due by the clock.
3. On that store, with no other call made first, send ListAgreementInvoiceLineItems with `groupBy=INVOICE_ID` and `agreementId` = the renewal agreement's id (ids are minted from the store counter, so the identical setup mints the identical id — the technique `tests/all/unsettled_store_listings.rs:363-366` already relies on).
4. Observe HTTP 400 carrying `ResourceNotFoundException` for the renewal agreement.
5. Now call ListAgreementCharges with an empty body (or SearchAgreements), which settles the whole store, and repeat the identical ListAgreementInvoiceLineItems request.
6. Observe 200 with the renewal's invoice group: the same request answered two different ways depending on whether an unrelated call looked first.

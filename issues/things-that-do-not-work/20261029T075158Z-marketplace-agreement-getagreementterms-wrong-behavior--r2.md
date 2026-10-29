Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 97cefcb07ab7801eeb37fff053a06a3892425e6e

# GetAgreementTerms resolves its agreementId without settling the agreement, so the renewal the clock already created is answered as not found

_Filed: 2026-10-29T07:51:58Z_
_Operation: GetAgreementTerms | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `97cefcb07ab7801eeb37fff053a06a3892425e6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetAgreementTerms` is the only single-agreement reader in this crate that resolves its `agreementId` against `store.agreements` WITHOUT first calling `crate::state::settle_agreement`, the crate's single materialiser of the end-date transitions the clock has already made due. Its two sibling single-agreement readers both call it as the first thing they do (`DescribeAgreement`, `GetAgreementEntitlements`), and so does every single-agreement guard (`CancelAgreement`, `SendAgreementCancellationRequest`, `SendAgreementPaymentRequest`, `AcceptAgreementCancellationRequest`, and `AcceptAgreementRequest` for its source).

Consequence: the renewal agreement an auto-renewing agreement's end date creates is inserted by `settle_agreement` -> `renew_agreement`, together with the accepted terms it re-states from the source's quote. Because this handler never settles, its answer depends on whether some unrelated call looked at the store first: as the first call after a renewal became due, `GetAgreementTerms` with the renewal's `agreementId` answers the not-found refusal (`crate::state::agreement_not_found`, at `crate::state::NOT_FOUND_STATUS`), while `DescribeAgreement` or `SearchAgreements` with the very same id answers 200 — and once either of those has run, the identical `GetAgreementTerms` request also answers 200 with that renewal's accepted terms. The accepted terms of a renewal are exactly what this operation exists to report, so the operation cannot report them until a different operation has been called.

This is the same omission already filed for `ListAgreementInvoiceLineItems` and `BatchCreateBillingAdjustmentRequest`, on a third operation: the handler whose comment claims it "writes nothing" is the one that therefore reports less than the clock already owes.

## Evidence

The handler takes a READ-ONLY lock and resolves the member with no settling — `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/get_agreement_terms.rs:164-166`:
```
        // A pure report: it reads the stored accepted terms and writes nothing.
        let store = state.lock();
        let Some(agreement) = store.agreements.get(&input.agreement_id) else {
```
and :169-171 answers `crate::state::not_found(GetAgreementTermsErrorModeled::ResourceNotFoundException(crate::state::agreement_not_found(&input.agreement_id),`. `grep settle_agreement` over `src/ops` shows no occurrence in this file.

Sibling single-agreement reader that does settle — `src/ops/describe_agreement.rs:185-196`: "A report, of the agreement as the clock has already left it: an agreement whose own stored end date has passed ended at that instant whether or not anybody called, so the status that transition is owed ... is materialised before it is reported" followed by `store.settle_agreement(&input.agreement_id, now);`. The other one — `src/ops/get_agreement_entitlements.rs:162-172`: "The agreement's own end date IS settled first: one past its stored end date ended at that instant whether or not anybody called" followed by `store.settle_agreement(&input.agreement_id, now);`.

The crate's own rule, `src/state.rs:4849-4850` (`settle_agreement` doc): "Both are owed at that instant whether or not anybody calls, so every operation that reports or guards on an agreement's status materialises them before it answers." and :4858-4861: "What an operation answering about a single agreement calls: the record it is about is the only one its answer depends on, so it and the chain it starts are the only ones to bring up to date."

That call is what creates the renewal AND its accepted terms — `src/state.rs:4953` `let terms = accepted_terms(&requested_terms, &context);` and :4999 `accepted_terms: terms,` inside `renew_agreement` (`src/state.rs:4932`), reached from `settle_agreement` at :4868 `match self.renew_agreement(&id, now)`. Its doc at :4902-4903 quotes the model: `EndTimeBehaviorType` `RENEW` says "a new agreement is created from the accepted terms of this agreement".

Test coverage leaves it unpinned: `tests/all/agreement_renewals.rs:583` obtains the chain through a SEARCH (`let links = chain_of(&router, &source).await;`) and only then reads `terms_of(&router, &renewal)` at :596, so `GetAgreementTerms` is never driven on an unsettled store. `tests/all/unsettled_store_listings.rs:10-13` states the rule ("the answer never depends on whether some unrelated call looked first") and its `whole_store_listings` (:160-176) enumerates only the four scanning listings.

## Steps to Reproduce

1. Create and accept an agreement request whose terms are a one-month prepaid purchase with a renewal term enabling auto-renewal (`tests/all/agreement_renewals.rs::renewing_terms("P1M")`).
2. Wind the stored agreement's window back past its end date the way `tests/all/agreement_expiry.rs::aged_agreement` does, so one renewal is due by the clock, and make no other call.
3. On that fresh store, send GetAgreementTerms with `agreementId` = the renewal agreement's id (ids are minted from the store's own counter, so the identical setup mints the identical id — the technique `tests/all/unsettled_store_listings.rs:363-366` relies on).
4. Observe the not-found refusal for the renewal agreement, although its end date passed and the clock already owed that agreement and its accepted terms.
5. Call DescribeAgreement (or SearchAgreements) for the same id, which settles the chain, then repeat the identical GetAgreementTerms request.
6. Observe 200 with the renewal's accepted terms: the same request answered two different ways depending on whether an unrelated call looked first.

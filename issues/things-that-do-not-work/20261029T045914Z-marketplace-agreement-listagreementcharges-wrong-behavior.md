Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 060392db00c5b3260ef85f15c64a4c64c2352242

# ListAgreementCharges scans every agreement without settling the clock, so the charges of a renewal the end date already created are missing until an unrelated read materialises it

_Filed: 2026-10-29T04:59:14Z_
_Operation: ListAgreementCharges | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `060392db00c5b3260ef85f15c64a4c64c2352242`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAgreementCharges` is a store-SCANNING reader: with no `agreementId` it iterates `store.agreements.values()` and reports `store.charges_of(agreement)` for each. It never calls `crate::state::settle_agreements`, the crate's single materialiser of what an agreement's own stored `endTime` has already made due — even though the state module states that rule for exactly this shape of operation, and the sibling scanning reader `SearchAgreements` obeys it on the line above its own scan.

The transition that is skipped is not cosmetic: `settle_agreement` -> `renew_agreement` INSERTS a whole new agreement for an auto-renewing agreement whose end date has passed, together with its OWN quote (`charge_summary` at `INTENT_NEW`, whose charge ids are minted there) and its own acceptance invoices. So for an agreement whose renewal the clock already owes:

* `ListAgreementCharges` with no filter reports only the source agreement's charges and silently omits every charge of the renewal the clock created at the end date;
* `ListAgreementCharges` with `agreementId` = the renewal's id is refused with `ValidationException`/`INVALID_AGREEMENT_ID` by `store.unknown_agreement_refusal(...)`, because that agreement has not been materialised yet;
* calling any settling operation in between (`DescribeAgreement`, `SearchAgreements`, `GetAgreementEntitlements`, `CancelAgreement`, either `Send…`) changes the answer of the very same `ListAgreementCharges` request — the page grows and the refusal turns into a 200. A read's answer therefore depends on whether an unrelated caller happened to read something else first, which is the opposite of what the crate says about a transition that is "owed at that instant whether or not anybody calls".

The charge ids the listing fails to report are also the ids `UpdatePurchaseOrders` resolves through `charge_owners()`, so the renewal's charges cannot be associated with a purchase order until some other operation materialises the renewal.

## Evidence

The rule, stated in this crate's own state module — services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:4860-4868 (`settle_agreements` doc):
  "/// Materialise every end-date transition the clock has already made due by
   /// `now`, for every agreement this store holds ([`Self::settle_agreement`]).
   ///
   /// What an operation that SCANS the store calls, where the answer depends on
   /// every agreement rather than on one: the status an agreement past its own
   /// end date is searched by, the summary status reported beside it, and the
   /// renewal agreement its end date created are then the ones the clock
   /// already owed."
and state.rs:2296-2299 (`Agreement::expire` doc): "the transition is owed at that instant whether
   /// or not anybody calls, so every operation that reports or guards on an
   /// agreement's status materialises it before it answers".

The sibling that obeys it — src/ops/search_agreements.rs:250-260:
  "// A report, of the agreements as the clock has already left them: one
   // past its own stored end date ended at that instant whether or not
   // anybody searched, so the status that transition is owed is
   // materialised before anything is selected — which is what lets the
   // `Status` filter select an `EXPIRED` agreement, a `RENEWED` one and the
   // renewal agreement its end date created, and the summary report them.
   ...
   store.settle_agreements(now);"

The handler that skips it — src/ops/list_agreement_charges.rs:182-184:
  "// A pure report: it reads the stored agreements and their quotes and
   // writes nothing.
   let store = state.lock();"
scanning every agreement at list_agreement_charges.rs:203 and :228:
  "for agreement in store.agreements.values() {"
  "matched.extend(store.charges_of(agreement));"
and refusing an id it has not materialised at list_agreement_charges.rs:193:
  "if let Some(refusal) = store.unknown_agreement_refusal(input.agreement_id.as_deref()) {"
(there is no `settle_agreement`/`settle_agreements` call anywhere in the file — `grep settle_ src/ops` lists only accept_agreement_cancellation_request.rs:204, get_billing_adjustment_request.rs:182, cancel_agreement.rs:155, get_agreement_entitlements.rs:172, describe_agreement.rs:196, accept_agreement_request.rs:198, list_billing_adjustment_requests.rs:247, send_agreement_payment_request.rs:193, send_agreement_cancellation_request.rs:203, search_agreements.rs:260).

What the skipped call would have created — state.rs:4931-4934 and :4944/:4967 (`renew_agreement`):
  "// The renewal's own quote, which prices the same requested terms afresh
   // for the new window: its charge ids are minted here, so the renewal
   // reports charges of its own rather than the source's.
   let charge_summary = self.charge_summary(&requested_terms, INTENT_NEW, None);"
  "self.agreement_requests.insert(" … "self.agreements.insert("

Test coverage: tests/all/agreement_renewals.rs:600-609 does read the renewal's charges (`charge_ids(&router, &renewal)`), but only AFTER `chain_of` has run a `SearchAgreements` call (tests/all/agreement_renewals.rs:249-259), which materialises the renewal — so the direct ListAgreementCharges path is never exercised on an unsettled store. tests/all/agreement_charges.rs covers only freshly accepted agreements (it contains no aged/`signed_ago` row).

## Steps to Reproduce

1. Create and accept an agreement request whose terms commit a contract length and enable auto-renewal (e.g. a ConfigurableUpfrontPricingTerm with selectorValue P1M plus a renewalTerm with enableAutoRenew=true), as tests/all/agreement_renewals.rs does with `renewing_terms("P1M")`.
2. Age the stored record past its end date the way tests/all/agreement_renewals.rs / tests/all/agreement_expiry.rs do (wind the agreement's start/acceptance/end instants back by more than the contract length), WITHOUT calling any of DescribeAgreement, SearchAgreements, GetAgreementEntitlements, CancelAgreement, SendAgreementPaymentRequest or SendAgreementCancellationRequest.
3. Call ListAgreementCharges with an empty body: the response carries only the source agreement's charges — none of the renewal's, although the renewal was due at the end date.
4. Call SearchAgreements (filters: AgreementType=PurchaseAgreement) once, which calls settle_agreements, then repeat the identical ListAgreementCharges call from step 3: it now reports strictly more charges (the renewal's), so the read's answer changed because of an unrelated read.
5. Repeat from step 2 and instead call ListAgreementCharges with agreementId set to the renewal id that step 4 reveals: before any settling call it is refused 400 ValidationException with reason INVALID_AGREEMENT_ID; after one it answers 200.

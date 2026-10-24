Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# partyType=Proposer always lists nothing, so the seller that sent a cancellation request can never see it

_Filed: 2026-10-24T01:06:02Z_
_Operation: ListAgreementCancellationRequests | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SendAgreementCancellationRequest is documented as the SELLER's (proposer's) call ("Allows sellers (proposers) to submit a cancellation request for an active agreement"), and ListAgreementCancellationRequestsInput$partyType documents "Use Proposer to list cancellation requests where you are the seller, or Acceptor to list cancellation requests where you are the buyer". Expected: after the caller sends a cancellation request, listing with partyType=Proposer returns that request (it is the caller's own submission as seller). Actual: the listing is always EMPTY for partyType=Proposer, for every request the emulator ever created. Availability is decided by crate::state::caller_is_party(agreement, party_type), which for Proposer compares the caller account against Agreement.proposer.account_id — a field AcceptAgreementRequest always stores as None (src/ops/accept_agreement_request.rs: `proposer: Proposer { account_id: None }`). So no stored agreement ever names any account as proposer and the Proposer branch of the listing is dead code: the whole seller-side view of the cancellation feature returns 200 with an empty items list. Note the same root cause makes SearchAgreements with filter PartyType=Proposer select nothing ever (matches_filter -> caller_is_party), and DescribeAgreement/AgreementViewSummary report `proposer` with no accountId. Fix direction: record the submitting account as the party that owns the cancellation request (ListAgreementPaymentRequests already does exactly this via PaymentRequest.proposer_account_id, which is why its Proposer branch works), or record the emulator account as the agreement's proposer.

## Evidence

Spec: SendAgreementCancellationRequest doc ("Allows sellers (proposers) to submit a cancellation request") and ListAgreementCancellationRequestsInput$partyType doc (Proposer = seller view) in the Smithy model. Handler: src/ops/list_agreement_cancellation_requests.rs lines ~195-210 gate each record on crate::state::caller_is_party(agreement, &input.party_type); src/state.rs caller_is_party() compares against agreement.proposer.account_id, which src/ops/accept_agreement_request.rs sets to None for every agreement. Contrast src/state.rs payment_request_party_matches(), which resolves Proposer against PaymentRequest.proposer_account_id (set by SendAgreementPaymentRequest), so the payment-request listing does work for the seller. Test coverage: tests/all/agreement_cancellation_requests.rs::a_party_type_selects_the_requests_of_the_agreements_it_names (lines 418-446) asserts the CURRENT empty-for-Proposer behavior, so the divergence is locked in by a test that must be updated, not covered by one.

## Steps to Reproduce

1. CreateAgreementRequest(intent=NEW, agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"eula.v1"}]) -> agreementRequestId
2. AcceptAgreementRequest(agreementRequestId=<id>) -> agreementId
3. SendAgreementCancellationRequest(agreementId=<agreementId>, reasonCode="OTHER") -> 200 with an acr- id in PENDING_APPROVAL (the caller acts as the seller here)
4. ListAgreementCancellationRequests(partyType="Proposer") -> observe 200 with items == [] (expected: the acr- request just created)
5. ListAgreementCancellationRequests(partyType="Acceptor") -> returns the request, showing only the buyer-side view is wired

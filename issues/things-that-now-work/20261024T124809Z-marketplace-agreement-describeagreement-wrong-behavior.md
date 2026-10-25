Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# proposer.accountId is never populated, so every agreement reports an empty proposer

_Filed: 2026-10-24T12:48:09Z_
_Operation: DescribeAgreement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeAgreementOutput$proposer is "details of the party proposing the agreement terms, most commonly the seller for PurchaseAgreement" and Proposer$accountId is the proposing AWS account. Expected: an accepted agreement reports both parties — acceptor.accountId (the buyer) and proposer.accountId (the seller who published the proposal the agreement was created from).

Actual: src/ops/accept_agreement_request.rs stores `proposer: Proposer { account_id: None }` for every agreement, so DescribeAgreement (and SearchAgreements' AgreementViewSummary$proposer) answers with an empty proposer object for every agreement that exists. Because state::caller_is_party() decides a party match by comparing the agreement's recorded account to the emulator account, nothing in the store can ever be selected as the Proposer side either: SearchAgreements' PartyType=Proposer filter, ListAgreementCancellationRequests(partyType="Proposer") and the Acceptor-side checks of ListAgreementPaymentRequests all read that never-populated member. A deterministic seller account (derived from the agreementProposalIdentifier the request names) would make the proposer side reportable and selectable.

## Evidence

Spec: DescribeAgreementOutput$proposer and AgreementViewSummary$proposer ("Details of the party proposing the agreement terms, most commonly the seller"); Proposer$accountId; SearchAgreementsInput$filters documents PartyType "The party type of the caller. Use Proposer or Acceptor". Handler/state: src/ops/accept_agreement_request.rs (`proposer: Proposer { account_id: None }`), src/state.rs caller_is_party() PARTY_TYPE_PROPOSER arm. Tests: tests/all/validation_messages.rs:370 and tests/all/agreement_cancellation_requests.rs:437-447 assert that a Proposer-scoped query selects nothing, pinning the current behavior. The existing open issue 20261024T010602Z-marketplace-agreement-listagreementcancellationrequests-wrong-behavior covers the listing symptom for that one operation only; this issue is the DescribeAgreement/SearchAgreements output member that is its root.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[...]) then AcceptAgreementRequest
2. DescribeAgreement(agreementId=<id>) -> observe "proposer": {} with no accountId while "acceptor" carries the emulator account (expected the seller account of the proposal)
3. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]}]) -> every agreementViewSummaries[].proposer is empty
4. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]},{name:"PartyType",values:["Proposer"]}]) -> observe an empty result set, which can never be non-empty

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# agreementId, catalog and agreementType are never resolved: a bogus value answers 200 with an empty page instead of ValidationException

_Filed: 2026-10-24T13:53:46Z_
_Operation: ListAgreementCancellationRequests | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListAgreementCancellationRequests enforces only `partyType`. The remaining selectors (`agreementId`, `status`, `agreementType`, `catalog`) are applied as predicates over the stored cancellation requests, so a value naming a resource or catalog that does not exist is reported as an empty page.

Expected: an `agreementId` that names no agreement is refused with 400 ValidationException carrying reason INVALID_AGREEMENT_ID; a `catalog` other than `AWSMarketplace` is refused with reason INVALID_CATALOG; an `agreementType` outside the documented set is refused with reason INVALID_AGREEMENT_TYPE.

Actual: src/ops/list_agreement_cancellation_requests.rs filters with `is_some_and(|id| id != request.agreement_id)` etc. and answers HTTP 200 with `items: []`, so a caller cannot distinguish "this agreement has no cancellation requests" from "this agreement does not exist". The same crate refuses the identical member on ListAgreementCharges and ListBillingAdjustmentRequests with ValidationException/INVALID_AGREEMENT_ID, so at most one of the two behaviours can match AWS.

## Evidence

Spec: ValidationExceptionReason models INVALID_AGREEMENT_ID, INVALID_CATALOG, INVALID_AGREEMENT_TYPE; ListAgreementCancellationRequestsInput documents agreementId/agreementType/catalog as optional filters over agreements of the caller, and AWS Marketplace exposes the single catalog AWSMarketplace (crate::state::CATALOG). Handler: src/ops/list_agreement_cancellation_requests.rs handle() — only PARTY_TYPE_VALUES is checked before the store scan; no agreement lookup, no catalog/agreementType value check. Test coverage: tests/all/agreement_cancellation_requests.rs lines ~356-371 pin the current behavior ("another catalog selects nothing", a non-matching agreementType selects nothing); no test asserts a refusal for an unknown agreementId here. Distinct from the already-filed wrong-behavior issue on this operation (20261024T010602Z-...-listagreementcancellationrequests-wrong-behavior), which is about partyType=Proposer matching nothing.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest, then SendAgreementCancellationRequest over the agreement
2. ListAgreementCancellationRequests(partyType="Acceptor", agreementId="agmt-does-not-exist") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_ID)
3. ListAgreementCancellationRequests(partyType="Acceptor", catalog="AWSMarketplace.Other") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_CATALOG)
4. ListAgreementCancellationRequests(partyType="Acceptor", agreementType="NotAType") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_TYPE)

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# A search with no AgreementType filter (or an unsupported Acceptor filter combination) is answered 200 instead of ValidationException

_Filed: 2026-10-24T01:06:02Z_
_Operation: SearchAgreements | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SearchAgreementsInput$filters documents the filter rules the service enforces: "A proposer can use any combination of the preceding filters along with AgreementType, which is required", and every supported combination it then lists for PartyType=Acceptor also begins with AgreementType (AgreementType, AgreementType+Status, AgreementType+ResourceIdentifier, AgreementType+OfferId, AgreementType+OfferSetId, AgreementType+ResourceType, each optionally with EndTime). Expected: a SearchAgreements request whose filter list omits AgreementType — including a request carrying no filters at all — is refused with 400 ValidationException (ValidationExceptionReason models UNSUPPORTED_FILTERS and INVALID_FILTERS; the service's wording for the combination case is "Provided combination of filters is not supported."), and so is an Acceptor-scoped request whose extra filters are outside the documented whitelist (e.g. AgreementType+AcceptorAccountId as Acceptor). Actual: crate::state::validate_filters() only checks that each filter names one of the documented names, that a PartyType value is Proposer/Acceptor, and that Before*/After* values parse as timestamps; no rule requires AgreementType and no combination whitelist is applied, so SearchAgreements with filters=[] (or filters naming only Status) returns 200 with the full agreement list. The only combination rule implemented is the sortBy/PartyType pairing (unsupported_filter_combination("sort.sortBy")), which is a different rule from the filter-list rules documented here.

## Evidence

Spec: SearchAgreementsInput$filters documentation in the Smithy model (AgreementType "is required"; the Acceptor combination whitelist); ValidationExceptionReason includes UNSUPPORTED_FILTERS, INVALID_FILTERS, INVALID_AGREEMENT_TYPE. Handler: src/ops/search_agreements.rs lines ~188-214 call crate::state::validate_filters(&filters) and then only the sort/party-type pairing check; src/state.rs validate_filters() (name membership, PartyType value, timestamp parse) has no required-filter or combination rule. Test coverage: tests/all/validation_messages.rs lines ~280-350 cover only the sortBy/PartyType pairing and always pass an AgreementType filter; tests/all/agreement_lifecycle.rs line 1238 searches with AgreementType present. No test asserts the required-AgreementType or combination rules.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest to store one agreement
2. SearchAgreements({}) with no filters at all -> observe 200 with agreementViewSummaries listing the agreement (expected 400 ValidationException, reason UNSUPPORTED_FILTERS/INVALID_FILTERS)
3. SearchAgreements(filters=[{name:"Status", values:["ACTIVE"]}]) -> observe 200 (expected 400 ValidationException: AgreementType is required)
4. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]},{name:"PartyType",values:["Acceptor"]},{name:"AcceptorAccountId",values:["123456789012"]}]) -> observe 200 (expected 400 ValidationException: not one of the combinations documented for PartyType=Acceptor)

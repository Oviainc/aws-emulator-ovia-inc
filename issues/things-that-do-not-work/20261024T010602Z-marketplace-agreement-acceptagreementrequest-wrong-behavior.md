Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Accepted agreements never get an endTime, so endTime/endTimeBehavior are unreportable and every EndTime filter and the default sort are inert

_Filed: 2026-10-24T01:06:02Z_
_Operation: AcceptAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AcceptAgreementRequest stores every agreement with `end_time: None` (src/ops/accept_agreement_request.rs), justified in src/state.rs as "The requested terms this emulator accepts carry no validity dates". That is not true for a prepaid/term purchase: ConfigurableUpfrontPricingTermConfiguration$selectorValue is @required and documented as "Defines the length of time for which the particular pricing/dimension is being purchased by the acceptor" (e.g. "P12M"), and the acceptor supplies it on RequestedTerm$configuration, which the emulator already persists. Expected: an agreement accepted with a ConfigurableUpfrontPricingTerm configuration carrying a duration has an end date (startTime + the purchased duration); DescribeAgreementOutput$endTime documents that the field is null only "for pay-as-you-go agreements, which don't have end dates", and DescribeAgreementOutput$endTimeBehavior is "present for every active agreement that has an end date". Actual: endTime is absent for EVERY agreement, which makes several modeled behaviors permanently unreachable: DescribeAgreement never returns endTimeBehavior (Agreement::end_time_behavior() returns None as its first statement when end_time is None), AgreementViewSummary never carries endTimeBehaviorType/endTimeBehaviorReasonCode, the SearchAgreements filters BeforeEndTime/AfterEndTime/EndTimeBehaviorType/EndTimeBehaviorReasonCode can never match anything, and the documented default sort (sortBy=EndTime) sorts over all-absent keys so it degrades to ordering by agreement id.

## Evidence

Spec: ConfigurableUpfrontPricingTermConfiguration$selectorValue (@required, "length of time for which the particular pricing/dimension is being purchased"); DescribeAgreementOutput$endTime ("null for pay-as-you-go agreements, which don't have end dates"); DescribeAgreementOutput$endTimeBehavior ("present for every active agreement that has an end date"); SearchAgreementsInput$filters documents BeforeEndTime/AfterEndTime/EndTimeBehaviorType/EndTimeBehaviorReasonCode; Sort$sortBy default EndTime. Handler: src/ops/accept_agreement_request.rs sets `end_time: None` unconditionally; src/state.rs Agreement::end_time doc and Agreement::end_time_behavior() (`self.end_time?` guard). Test coverage: tests/all/agreement_lifecycle.rs lines 843-848 assert endTime and endTimeBehavior are null — including for the "a prepaid purchase" case built with upfront_term("cfg", "P12M", ...), so the current behavior is asserted and would need updating rather than already being covered.

## Steps to Reproduce

1. CreateAgreementRequest(intent=NEW, agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:2}]}}}]) -> agreementRequestId
2. AcceptAgreementRequest(agreementRequestId=<id>) -> agreementId
3. DescribeAgreement(agreementId=<id>) -> observe endTime absent and endTimeBehavior absent, although the acceptor purchased a 12-month term (expected an end date and, for an ACTIVE agreement with an end date, an endTimeBehavior of EXPIRE/NO_RENEWAL_TERM or RENEW)
4. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]},{name:"BeforeEndTime",values:["2099-01-01T00:00:00Z"]}]) -> observe an empty result set; the same filter can never match any agreement
5. SearchAgreements(filters=[{name:"AgreementType",values:["PurchaseAgreement"]},{name:"EndTimeBehaviorType",values:["EXPIRE"]}]) -> observe an empty result set

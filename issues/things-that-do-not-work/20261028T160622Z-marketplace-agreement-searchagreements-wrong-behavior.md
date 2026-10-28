Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# AgreementViewSummary suppresses endTimeBehaviorType/ReasonCode for an agreement that is no longer active, applying DescribeAgreement's nullness rule to a member the model gives a different one

_Filed: 2026-10-28T16:06:22Z_
_Operation: SearchAgreements | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states two DIFFERENT nullness rules for the two projections of an agreement's end-time behaviour. `DescribeAgreementOutput$endTimeBehavior` is absent for three reasons, including "an agreement that is no longer active". `AgreementViewSummary$endTimeBehaviorType` states exactly ONE null condition: an agreement with no end date.

The crate computes BOTH from a single helper, `Agreement::end_time_behavior()`, which short-circuits to `None` whenever `self.status != STATUS_ACTIVE` — a rule whose own doc comment cites only the DescribeAgreement sentence. Consequences on SearchAgreements:

1. A CANCELLED or REPLACED agreement that HAS an end date is reported by `SearchAgreements` with `endTimeBehaviorType: null` and `endTimeBehaviorReasonCode: null`, although the summary member's documented null condition ("agreements that have no end date") does not hold for it.
2. `matches_filter` reads the same helper, so the `EndTimeBehaviorType` and `EndTimeBehaviorReasonCode` filters can never select an agreement that is not ACTIVE. A search combining `Status=CANCELLED` with `EndTimeBehaviorType=EXPIRE` — a combination the model admits for a Proposer-scoped search ("A proposer can use any combination of the preceding filters along with AgreementType") — returns an empty page for agreements that do carry an end date and a behaviour at it.

Note the status gate is NOT a clock-driven materialisation: the agreement's stored `end_time` and accepted terms are unchanged by the cancellation, so the behaviour at that end date is still computable from stored state; it is simply withheld.

## Evidence

Smithy, the summary member's single documented null case — `smithy_query(query="doc", shape="AgreementViewSummary", member="endTimeBehaviorType")`:
  "The behavior of the agreement when it reaches its end date. The field is <code>null</code> for agreements that have no end date, because those agreements never reach an end time."

Smithy, the DIFFERENT rule the model states for the describe member — `smithy_query(query="doc", shape="DescribeAgreementOutput", member="endTimeBehavior")`:
  "This field is present for every active agreement that has an end date. It is not present for an agreement that has no end date, because such an agreement never reaches an end time. Pay-as-you-go agreements are the most common example. It is also not present for an agreement that is no longer active."

The handler code that collapses the two, services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:2239-2252 — note the doc cites ONLY the describe member:
```
    /// `DescribeAgreementOutput$endTimeBehavior` states when the member is
    /// reported at all: "This field is present for every active agreement that
    /// has an end date. It is not present for an agreement that has no end
    /// date… It is also not present for an agreement that is no longer active."
    pub fn end_time_behavior(&self) -> Option<EndTimeBehavior> {
        self.end_time?;
        if self.status != STATUS_ACTIVE {
            return None;
        }
```

and the SEARCH projection reusing it, state.rs:2362-2390:
```
    pub fn to_view_summary(&self) -> AgreementViewSummary {
        let behavior = self.end_time_behavior();
        ...
            end_time_behavior_type: behavior.as_ref().map(|b| b.r#type.clone()),
            end_time_behavior_reason_code: behavior.and_then(|b| b.reason_code),
```

and the filter reading the same helper, state.rs:3094 and 3124-3127:
```
    let behavior = agreement.end_time_behavior();
...
        "EndTimeBehaviorType" => behavior.is_some_and(|b| b.r#type == value),
        "EndTimeBehaviorReasonCode" => {
            behavior.is_some_and(|b| b.reason_code.as_deref() == Some(value))
        }
```

Test coverage: tests/all/agreement_end_dates.rs:314-357 and :415-472 pin both projections and both filters for ACTIVE agreements only (the row set is built by `agreement_over`, which never cancels); `grep -r endTimeBehaviorType tests/` returns only agreement_end_dates.rs:347, so the inactive case is unpinned — no test or comment justifies the divergence.

## Steps to Reproduce

1. CreateAgreementRequest with requestedTerms that commit a duration (e.g. a configurableUpfrontPricingTerm with a selectorValue the rate card publishes), then AcceptAgreementRequest to get an agreementId.
2. DescribeAgreement: note endTime is set and endTimeBehavior.type is EXPIRE/RENEW.
3. SearchAgreements with filters [{name: AgreementType, values: [PurchaseAgreement]}]: the summary reports endTimeBehaviorType.
4. CancelAgreement on that agreementId.
5. SearchAgreements again with the same filters: the summary for that agreement now reports endTimeBehaviorType: null and endTimeBehaviorReasonCode: null, although the agreement still has an end date — the only null condition AgreementViewSummary$endTimeBehaviorType documents.
6. SearchAgreements with filters [{name: AgreementType, values: [PurchaseAgreement]}, {name: EndTimeBehaviorType, values: [EXPIRE]}]: the cancelled agreement is never selected.

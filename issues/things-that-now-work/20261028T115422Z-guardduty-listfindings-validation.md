Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# ListFindings (and GetFindingsStatistics) accept and evaluate the `matches`/`notMatches` conditions the model documents as available only to the filter APIs

_Filed: 2026-10-28T11:54:22Z_
_Operation: ListFindings | Class: validation_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Condition` is the one condition shape that `FindingCriteria.Criterion` targets, so it is shared by `CreateFilter`/`UpdateFilter` and by the findings reads `ListFindings` and `GetFindingsStatistics`. The model documents two of its members as restricted to the filter APIs only: `Matches` — "The matches condition is available only for create-filter and update-filter APIs" — and `NotMatches` — "The not-matches condition is available only for create-filter and update-filter APIs".

The crate ignores that restriction on the read side. `crate::state::validate_finding_criteria` — the one up-front criteria check both `ListFindings` and `GetFindingsStatistics` run — only checks that a criterion key names a finding field and that an ordering condition is applied to an orderable field; it never looks at `matches`/`notMatches`. `crate::state::condition_matches` then EVALUATES both members, wildcard-expanding them against the finding's attribute text. So a `findingCriteria` carrying `matches` or `notMatches` is answered HTTP 200 with a wildcard-filtered page by `ListFindings`, and with wildcard-filtered statistics by `GetFindingsStatistics`, where the documented contract makes those two conditions reachable only through `CreateFilter`/`UpdateFilter`.

The crate nowhere acknowledges this: the string "create-filter" does not appear anywhere under `src/`, and `create_filter.rs` states the opposite intent — that there is deliberately "ONE answer on this service to 'is this a finding field'" shared between the filter create and the findings reads — which is true for the key check but silently extends the two filter-only operators to the reads as well.

The defect is on the read path only. `CreateFilter`/`UpdateFilter` accepting and storing `matches`/`notMatches` is exactly what the model says those APIs are for, and nothing here asks for that to change.

## Evidence

Model, Condition$Matches (`smithy_query(query="doc",shape="Condition",member="Matches")`): "Represents the <i>match</i> condition to be applied to a single field when querying for findings. <note><p> The <i>matches</i> condition is available only for create-filter and update-filter APIs. </p></note>"

Model, Condition$NotMatches (`smithy_query(query="doc",shape="Condition",member="NotMatches")`): "<p> The <i>not-matches</i> condition is available only for create-filter and update-filter APIs. </p>"

The shared up-front validator never looks at the two members — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:5255-5277:
```
pub fn validate_finding_criteria(
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<(), CriterionRejection> {
    ...
    for (path, condition) in criterion {
        if !is_finding_field(path) { return Err(CriterionRejection::UnknownField(path.clone())); }
        let orders_numerically = condition.greater_than.is_some() || ... || condition.lte.is_some();
        if orders_numerically && !attribute_is_orderable(path) { return Err(CriterionRejection::NotOrderable(path.clone())); }
    }
    Ok(())
}
```
(no `condition.matches` / `condition.not_matches` branch anywhere in the function)

And the evaluator applies them — state.rs:5178-5196:
```
    // `Matches` / `NotMatches` compare the attribute against a pattern in which
    // `*` stands for any run of characters; like the equality lists, a list holds
    // alternatives.
    if let Some(patterns) = condition.matches.as_ref() {
        let selected = text.as_ref().is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if !patterns.is_empty() && !selected { return Ok(false); }
    }
    if let Some(patterns) = condition.not_matches.as_ref() {
        let excluded = text.as_ref().is_some_and(|value| patterns.iter().any(|p| wildcard_matches(p, value)));
        if excluded { return Ok(false); }
    }
```

Reached from the read handlers — src/ops/list_findings.rs:186-194:
```
        crate::state::validate_finding_criteria(input.finding_criteria.as_ref())
            .map_err(|rejection| refusal(&rejection, FINDING_CRITERIA_MEMBER))?;
        for finding in detector.findings.values() {
            match crate::state::finding_matches(finding, input.finding_criteria.as_ref()) {
                Ok(true) => matched.push(finding),
```
and src/ops/get_findings_statistics.rs:209-218 (the same two calls, on the same `FindingCriteria`).

No comment or doc in the crate justifies the divergence: `grep -n 'create-filter|update-filter|only for create' src/` returns "No matches", while src/ops/create_filter.rs:196-204 states only the shared-key rationale ("The field check is the one `ListFindings` and `GetFindings` apply (`crate::state::validate_finding_criteria`), so there is ONE answer on this service to 'is this a finding field'") and says nothing about the two filter-only operators.

Test coverage that currently pins the accepting behaviour (would need to move with the fix, and does NOT state a reason tied to the model sentence): tests/all/detector_lifecycle.rs:1152-1195 drives ListFindings with `{"findingCriteria":{"criterion":{"type":{"matches":[pattern]}}}}` and `notMatches` over 8 wildcard cases, asserting `status == 200`; tests/all/detector_lifecycle.rs:1057-1059 and tests/all/findings_statistics.rs:695 do the same for the statistics path. tests/all/filter_lifecycle.rs:181-185 and tests/all/filter_detector_timestamp.rs:138 cover `matches`/`notMatches` on CreateFilter/UpdateFilter, which stays unchanged.

## Steps to Reproduce

1. CreateDetector in the request Region and note the detectorId.
2. POST /detector/{detectorId}/findings/create with {"findingTypes": ["Recon:EC2/PortProbeUnprotectedPort", "Discovery:S3/MaliciousIPCaller"]} to seed two findings.
3. POST /detector/{detectorId}/findings with {"findingCriteria": {"criterion": {"type": {"matches": ["Recon:*"]}}}}.
4. Expected per Condition$Matches ("The matches condition is available only for create-filter and update-filter APIs"): the condition is not available on this operation, so the request is refused (HTTP 400, BadRequestException / InvalidInputException naming findingCriteria.criterion) rather than evaluated. Actual: HTTP 200 with findingIds wildcard-filtered to the one Recon finding.
5. Repeat with {"notMatches": ["Recon:*"]} — same acceptance, complement selected.
6. POST /detector/{detectorId}/findings/statistics with {"groupBy": "SEVERITY", "findingCriteria": {"criterion": {"type": {"matches": ["*:EC2/*"]}}}} — HTTP 200 with statistics computed over the wildcard-filtered set, same gap on the statistics path.
7. Confirm the filter APIs are unaffected: CreateFilter with the same `matches` criterion is accepted and GetFilter reports it back, which is what the model documents.

Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c0eb4295fc6918b9011f996ea325c28045b54b65

# Reject an SloIds entry that is no ServiceLevelObjectiveId, as the sibling batch does

_Filed: 2026-10-27T21:15:49Z_
_Operation: BatchUpdateExclusionWindows | Class: validation_

> _Note: this issue was identified against the code base at commit `c0eb4295fc6918b9011f996ea325c28045b54b65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchUpdateExclusionWindows` and `BatchGetServiceLevelObjectiveBudgetReport` take the SAME input list shape (`ServiceLevelObjectiveIds`), whose member targets a plain `String` and therefore carries no `@pattern` the boundary validator can enforce. The budget-report handler refuses a malformed entry up front, with a comment stating the reason: a per-item `Errors` entry identifies the id by members the model pattern-constrains, so a malformed id cannot be stated in the response.

The exclusion-window handler skips that check. A malformed id resolves to no SLO, earns `ExclusionWindowFailure::NoSuchSlo`, and — whenever at least one other id in the same request DID apply — is copied verbatim into `BatchUpdateExclusionWindowsError.SloId`. That output member is `@required` and pattern-constrained to a well-formed SLO name or SLO ARN, so the handler answers 200 with a response field holding a value the model says cannot occur. The two sibling batch operations therefore disagree about the same unvalidated member of the same shape.

## Evidence

Smithy (`smithy_query(query="shape", shape="ServiceLevelObjectiveIds")`): `{"type": "list", "length": {"min": 1, "max": 50}, "member": "String"}` — no pattern, so a malformed entry reaches the handler.

Smithy (`smithy_query(query="shape", shape="BatchUpdateExclusionWindowsError")`): `"SloId": {"target": "ServiceLevelObjectiveId", "required": true, "pattern": "^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$|^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$"}`.

Sibling that DOES check — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/batch_get_service_level_objective_budget_report.rs:545-553:
```
// `SloIds` targets a list of plain `String`, so an entry that is no
// `ServiceLevelObjectiveId` at all reaches this handler. It is refused rather than
// reported: an `Errors` entry identifies the id it is about by `Name` and `Arn`,
// both of which the model pattern-constrains to a well-formed SLO name and ARN, so
// there is no way to state such an id in the response.
...
if input.slo_ids.iter().any(|id| !is_slo_id(id)) {
```

Side that does NOT — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/batch_update_exclusion_windows.rs:196-206 (no id-form check at all before resolution):
```
for id in &input.slo_ids {
    if !seen.insert(crate::state::slo_name_of(id).to_string()) {
        continue;
    }
    match store.update_exclusion_windows(id, &add, remove) {
        Ok(()) => applied.push(id.clone()),
        Err(failure) => failures.push((id.clone(), failure)),
```
and batch_update_exclusion_windows.rs:228-235, which copies the raw id straight into the pattern-constrained member:
```
let mut errors: Vec<BatchUpdateExclusionWindowsError2> = failures
    .iter()
    .map(|(id, failure)| BatchUpdateExclusionWindowsError2 {
        slo_id: id.clone(),
```

Test coverage: the rule is pinned for the sibling only — tests/all/budget_reports.rs:720-769 `budget_report_refuses_an_id_that_is_no_slo_id` ("`SloIds` targets a list of plain `String`, so these entries reach the handler", rows include `"bad!name"`, `"has:colon"`, `"-leading"`, the empty string). `grep -n 'SloIds' tests/all/exclusion_windows.rs` shows only well-formed ids (lines 164, 263, 353, 418, 484) — no malformed-id row exists for this operation.

## Steps to Reproduce

1. POST /slo with {"Name": "good-slo", "SliConfig": {"SliMetricConfig": {"KeyAttributes": {"Type": "Service", "Name": "checkout", "Environment": "prod"}, "MetricType": "LATENCY", "PeriodSeconds": 60}}} to create one stored SLO.
2. PATCH /exclusion-windows with {"SloIds": ["good-slo", "bad!name"], "AddExclusionWindows": [{"Window": {"DurationUnit": "HOUR", "Duration": 1}}]}.
3. Observe HTTP 200 whose Errors[0].SloId is the literal "bad!name" — a value the modelled @pattern on BatchUpdateExclusionWindowsError.SloId cannot hold.
4. Contrast: POST /budget-report with {"SloIds": ["bad!name"], "Timestamp": <now>} answers HTTP 400 with "Value at 'sloIds' failed to satisfy constraint" (batch_get_service_level_objective_budget_report.rs:553).

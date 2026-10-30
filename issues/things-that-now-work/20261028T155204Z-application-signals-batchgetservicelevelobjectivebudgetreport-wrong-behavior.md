Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# One missing SLO named by two ARN spellings earns two Errors entries, against the handler's own "named once" rule and the sibling batch's dedup

_Filed: 2026-10-28T15:52:04Z_
_Operation: BatchGetServiceLevelObjectiveBudgetReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler de-duplicates what it has already answered for by an ARN STRING: for an id that resolves it inserts the STORED record's ARN, and for an id that resolves to nothing it inserts `crate::state::slo_arn_of(&region, id)`, which hands an ARN-form id straight back VERBATIM.

But this crate's id resolution ignores an SLO ARN's partition, region and account fields entirely (`slo_name_of` -> `slo_name_in_arn` -> `resource_id_of_type`, and `state.rs` states the region rule explicitly), and `is_slo_id` admits any of them because the modelled `ServiceLevelObjectiveId` pattern spells the region and account as `[^:]*:[^:]*`. So two ids that denote the SAME SLO name — a bare name plus an ARN whose region is not the one the request is signed for, or two ARNs differing only in region/account/partition — yield two DIFFERENT dedup keys. When that SLO is not stored, the response carries TWO `Errors` entries whose `Name` is the same SLO, i.e. the request is answered as if two distinct resources were missing.

This contradicts the comment sitting directly above the resolution in this handler, which covers both branches of the `match`, and it contradicts the sibling batch over the same `SloIds` member: `BatchUpdateExclusionWindows` keys its dedup on `crate::state::slo_name_of(id)`, so the very same pair of ids names one SLO there and earns one failure. One of the two batches has to be wrong about whether those ids name one resource, and the crate's own identity rule (region/account are not part of an SLO's identity) says the budget report is the wrong one.

The fix keeps every existing response: key `answered` on `crate::state::slo_name_of(id)` in both branches (for a resolved record that string is already `slo.name`), leaving the reported `Arn`/`Name` of each entry exactly as they are today.

## Evidence

The rule this handler states for BOTH branches — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/batch_get_service_level_objective_budget_report.rs:578-581:

    // Either id form the modelled `ServiceLevelObjectiveId` admits resolves to one
    // record, so a request naming the same SLO by name AND by ARN names it once and
    // is answered with one report for it.
    match store.resolve(id) {

The not-found branch keys on the id's own ARN instead — same file:591-599:

    let arn = crate::state::slo_arn_of(&region, id);
    if answered.insert(arn.clone()) {
        errors.push(ServiceLevelObjectiveBudgetReportError {
            name: crate::state::slo_name_of(id).to_string(),
            arn,

and `slo_arn_of` returns an ARN id verbatim — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:644-650:

    pub fn slo_arn_of(region: &str, id: &str) -> String {
        if slo_name_in_arn(id).is_some() {
            id.to_string()
        } else {
            slo_arn(region, id)
        }
    }

while the crate's identity rule says those fields are NOT part of an SLO's identity — state.rs:616-619:

    /// The REGION field is not compared, for the reason the store's own doc gives: this
    /// process holds one account's and one Region's records, and the region in a minted ARN is a
    /// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
    pub fn tagged_slo_name_in_arn(resource_arn: &str) -> Option<&str> {

and resolution drops them — state.rs:583-585:

    pub fn slo_name_of(id: &str) -> &str {
        slo_name_in_arn(id).unwrap_or(id)
    }

The sibling batch over the same member dedups on the resolved NAME — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/batch_update_exclusion_windows.rs:197-201:

    // Either id form the modelled `ServiceLevelObjectiveId` admits resolves to one
    // record, so a request naming the same SLO by name AND by ARN names it once.
    if !seen.insert(crate::state::slo_name_of(id).to_string()) {
        continue;
    }

Test coverage: services/service-com-amazonaws-applicationsignals-applicationsignals/tests/all/budget_reports.rs:645-646 `budget_report_resolves_each_id_independently` — "One request reports every stored SLO it names and errs on every id it does not, and the same SLO named twice is reported once" — exercises "named twice" only for a STORED SLO (lines 666-697 use `"alpha-slo"` and `slo_arn("alpha-slo")`, where `slo_arn` builds the ARN in the very region the suite signs for, so both ids render to one key). The suite's missing-id rows (lines 554-586: "an ARN of another Region", "an ARN of another account") each use a DIFFERENT SLO name, so no row pins one missing SLO named by two spellings — the gap is unpinned.

## Steps to Reproduce

1. Start the emulator with this crate mounted and sign requests for region us-west-2 (the region `tests/all/budget_reports.rs` uses).
2. Do NOT create any SLO called `ghost`.
3. POST /budget-report with body {"Timestamp": <now>, "SloIds": ["ghost", "arn:aws:application-signals:eu-west-1:000000000000:slo/ghost"]}.
4. Observe HTTP 200 with `Errors` holding TWO entries, both with "Name": "ghost" — one `Arn` in us-west-2 (minted from the bare name) and one in eu-west-1 (the id echoed back) — i.e. one SLO reported missing twice.
5. For contrast, PATCH /exclusion-windows with {"SloIds": ["ghost", "arn:aws:application-signals:eu-west-1:000000000000:slo/ghost"]} and observe a single ResourceNotFoundException for the one SLO, because that handler dedups on `slo_name_of`.
6. Expected: the budget report answers ONE `Errors` entry for the one SLO those ids resolve to, as its own comment at batch_get_service_level_objective_budget_report.rs:578-580 states.

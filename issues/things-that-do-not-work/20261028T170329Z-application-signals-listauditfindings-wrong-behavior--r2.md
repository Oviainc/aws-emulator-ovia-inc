Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f965a258785d300a5b0cd6c5f88de592a3c6328

# The SLO auditor hardcodes BudgetStatus=INSUFFICIENT_DATA for an objective the budget report of the same instant computes as OK with 100% attainment

_Filed: 2026-10-28T17:03:29Z_
_Operation: ListAuditFindings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f965a258785d300a5b0cd6c5f88de592a3c6328`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate computes "the budget status of one stored SLO" in two places and the two reach contradictory answers for the same SLO at the same instant.

* `BatchGetServiceLevelObjectiveBudgetReport` derives it: a period-based SLO whose evaluation interval has an elapsed, post-creation window is reported with `BudgetStatus` from `budget_status_of(total, total, warning_threshold)` — i.e. `OK` — and `Attainment: 100.0`, on the stated ground that "a period with no breaching data point is a good period, so every period of the window attained the goal".
* `ListAuditFindings` reports the SLO auditor's result for the very same stored record, and `slo_auditor_result` writes the string constant `BUDGET_STATUS_INSUFFICIENT_DATA` into the `BudgetStatus` entry of `AuditorResult.Data` unconditionally — it never consults the interval, the window the request asked about, or the one budget-status rule the crate already owns — together with a `Description` asserting "No attainment datapoint is recorded for service level objective <name> in the requested period".

So for one SLO, one account and one instant, a client that calls both reads is told both that the objective's budget is intact and that no attainment datapoint exists for it. The `Data` key is deliberately named after the modelled member (`AUDITOR_DATA_BUDGET_STATUS = "BudgetStatus"`, documented as "the budget status the assessment reached") and its value is drawn from the modelled `ServiceLevelObjectiveBudgetStatus` value set, so the two statements are about the same quantity and are directly comparable by a client.

The auditor's constant also makes the audit read insensitive to everything that moves the budget report's answer: the requested `StartTime`/`EndTime`, the SLO's `Goal.Interval`, its `CreatedTime`, and whether the objective is period-based or request-based all change `BatchGetServiceLevelObjectiveBudgetReport`'s `BudgetStatus` while leaving the finding byte-identical. `state.rs` cannot even reach the rule, because `budget_status_of`/`evaluated` live in the budget-report handler rather than beside the state the auditor reads, which is how the two copies drifted.

## Evidence

The budget-report side — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/batch_get_service_level_objective_budget_report.rs:446-461:

    let (budget_status, attainment, budget_seconds_remaining) = match total_budget_seconds {
        // The whole budget remains, so this call site can only produce `OK` today: no part
        // of a budget is spent while nothing is observed in breach. The comparison is
        // [`budget_status_of`]'s all the same, so a spent budget would be classified by the
        // member's own rule rather than by a second one written here.
        Some(total) if evaluated_window => (
            budget_status_of(total, total, goal.warning_threshold),
            Some(FULL_ATTAINMENT),
            Some(total),
        ),

and the reasoning it rests on — same file:406-409:

    /// Nothing in the evaluated window has been observed in breach of the SLI threshold, since
    /// this emulator receives no telemetry and a period with no breaching data point is a good
    /// period, so every period of the window attained the goal and none of that budget has been
    /// spent.

The audit side, which answers the opposite for the same record — src/state.rs:2975-2999:

    fn slo_auditor_result(slo: &ServiceLevelObjective) -> AuditorResult {
        let mut data = HashMap::from([
            (AUDITOR_DATA_SLO_NAME.to_string(), slo.name.clone()),
            (AUDITOR_DATA_SLO_ARN.to_string(), slo.arn.clone()),
            (
                AUDITOR_DATA_BUDGET_STATUS.to_string(),
                BUDGET_STATUS_INSUFFICIENT_DATA.to_string(),
            ),
        ]);
        ...
        AuditorResult {
            auditor: Some(SLO_AUDITOR.to_string()),
            description: Some(format!(
                "No attainment datapoint is recorded for service level objective {} in the \
                 requested period, so no breached threshold is reported.",
                slo.name,
            )),

No interval, window or goal reaches that function: it takes only `slo`. The key and the value set it fills — src/state.rs:2785-2790 and :2800-2801:

    /// The budget status of an objective whose attainment has no evaluated datapoint
    /// (`smithy_query(shape="ServiceLevelObjectiveBudgetStatus")` -> `["OK", "WARNING",
    /// "BREACHED", "INSUFFICIENT_DATA"]`). It is what the SLO auditor's assessment rests on
    /// here: ...
    const BUDGET_STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

    /// [`AUDITOR_DATA_SLO_NAME`], for the budget status the assessment reached.
    const AUDITOR_DATA_BUDGET_STATUS: &str = "BudgetStatus";

The handler that reports it — src/ops/list_audit_findings.rs:224-230:

        let findings = state.lock().audited_findings(
            &input.audit_targets,
            input.auditors.as_deref(),
            detailed,
            &input.start_time,
            &input.end_time,
        );

Test coverage proves the two reads disagree today, each pinned separately and neither pinning their agreement — tests/all/budget_reports.rs:521-526 (an SLO created by the test itself, calendar interval reaching the present):

    assert_eq!(
        report["BudgetStatus"].as_str(),
        Some(STATUS_OK),
        "expected BudgetStatus {STATUS_OK:?} for the current tile, got {:?}",
        report["BudgetStatus"]
    );

versus tests/all/audit_findings.rs:741-747 (an SLO created by the test itself, audited over now-3600..now+3600):

    assert_eq!(
        result["Data"]["BudgetStatus"].as_str(),
        Some("INSUFFICIENT_DATA"),
        "case {:?}: expected budget status INSUFFICIENT_DATA, got {:?}",
        case.case,
        result["Data"]["BudgetStatus"]
    );

## Steps to Reproduce

1. Start the emulator with this crate mounted.
2. POST /slo with {"Name":"checkout-latency","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"orders","Environment":"eks:prod"},"OperationName":"GET /orders","MetricType":"LATENCY"},"MetricThreshold":250.0},"Goal":{"AttainmentGoal":99.0}} — the goal takes the documented default rolling 7-day interval.
3. POST /budget-report with {"Timestamp":<now>,"SloIds":["checkout-latency"]} and observe Reports[0].BudgetStatus == "OK", Attainment == 100.0, BudgetSecondsRemaining == TotalBudgetSeconds (the window from CreatedTime to now is non-empty, so batch_get_service_level_objective_budget_report.rs:451 is taken).
4. POST /auditFindings?StartTime=<now-3600>&EndTime=<now+3600> with {"AuditTargets":[{"Type":"slo","Data":{"Slo":{"SloName":"checkout-latency"}}}]}.
5. Observe AuditFindings[0].AuditorResults[0].Data.BudgetStatus == "INSUFFICIENT_DATA" and Description == "No attainment datapoint is recorded for service level objective checkout-latency in the requested period, so no breached threshold is reported." — the opposite verdict on the same SLO at the same instant.
6. Repeat step 4 with a window entirely before the SLO's CreatedTime, and again with a window entirely in the future: the finding is unchanged, while the budget report's BudgetStatus does change (tests/all/budget_reports.rs:266-353 enumerate the INSUFFICIENT_DATA cases), showing the auditor's value is a constant rather than an assessment.
7. Expected: one budget status for one SLO — the audit result's BudgetStatus is derived by the same rule the budget report applies (the crate's own budget_status_of/evaluated), so the two reads cannot contradict each other.

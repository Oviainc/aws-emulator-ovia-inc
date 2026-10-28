Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# GetRDSDatabaseRecommendations skips the analysis-due gate every sibling reader applies

_Filed: 2026-10-26T23:23:52Z_
_Operation: GetRDSDatabaseRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other recommendation reader in this crate reads its inventory through a `visible_*` accessor that withholds the analysis until BOTH `is_opted_in()` and `analysis_due()` hold, and the exporter of the very same RDS recommendations applies the same two-part gate. `GetRDSDatabaseRecommendations` instead walks `store.analysed_resources` directly after checking only `is_opted_in()`, so a caller that has just called `UpdateEnrollmentStatus(Active)` is answered with full database recommendations while `GetEBSVolumeRecommendations`, `GetEC2InstanceRecommendations`, `GetIdleRecommendations`, `GetLambdaFunctionRecommendations`, `GetLicenseRecommendations` and `GetAutoScalingGroupRecommendations` all answer the empty set for the same account at the same instant, and while `ExportRDSDatabaseRecommendations` writes an export file holding the header row alone. The crate states the rule it is breaking in its own words: the analysis is "an analysis nothing has produced yet" until the delay has elapsed.

## Evidence

The rule, stated once for the crate and applied by ten readers — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/state.rs:6566-6576:
```
    /// The same gate [`visible_auto_scaling_group_analyses`] applies: Compute Optimizer
    /// reports recommendations for an ENROLLED account only, and only once the analysis
    /// that enrollment starts is due ([`ANALYSIS_DELAY`]), so an account that never opted
    /// in, one that opted out, and one that has just opted in all read the empty set
    /// rather than an analysis nothing has produced yet.
    pub fn visible_volumes(&self) -> impl Iterator<Item = &ObservedVolume> {
        let visible = self.enrollment.is_opted_in() && self.enrollment.analysis_due();
        self.volumes.values().filter(move |_| visible)
    }
```
The exporter of the SAME RDS recommendations applies both halves — src/state/export.rs:791-797:
```
    /// Compute Optimizer reports recommendations for an ENROLLED account only, and only once
    /// the analysis its enrollment starts is due, so an export created before then holds the
    /// header row alone — exactly what the recommendation readers report at that moment.
    fn export_rows(&self, plan: &ExportPlan) -> Vec<ExportRow> {
        if !self.enrollment.is_opted_in() || !self.enrollment.analysis_due() {
            return Vec::new();
        }
```
The handler under review checks only the first half — src/ops/get_rds_database_recommendations.rs:284 and :308-312:
```
        if !guard.enrollment.is_opted_in() {
...
        let selected: Vec<RDSDBRecommendation> = store
            .analysed_resources
            .iter()
            .filter(|resource| resource.resource_type == crate::state::SOURCE_TYPE_RDS_DB_INSTANCE)
```
`analysed_resources_by_type`, the only other accessor over that catalog, carries no gate either (src/state.rs:6922-6937).

Test coverage confirms the divergence: every reader test that must see an analysis sleeps for the delay first — e.g. tests/all/ebs_volume_recommendations.rs:21-24 ("([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter" / `tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;`) and tests/all/recommendation_export_objects.rs:105 — while tests/all/rds_database_recommendations.rs contains no `sleep` at all (grep for `sleep` in that file: "No matches."), i.e. it asserts recommendations are reported immediately after opt-in.

## Steps to Reproduce

1. Start the emulator and call `UpdateEnrollmentStatus` with `status=Active`.
2. Immediately (within `ANALYSIS_DELAY`, 500 ms) call `GetEBSVolumeRecommendations` with an empty body — observe `volumeRecommendations: []`, because `visible_volumes()` withholds the analysis until it is due.
3. Immediately call `GetRDSDatabaseRecommendations` with an empty body — observe a non-empty `rdsDBRecommendations` list, i.e. an analysis the crate's own rule says has not been produced yet.
4. Immediately call `ExportRDSDatabaseRecommendations` into an existing bucket, poll `DescribeRecommendationExportJobs` to `Complete`, and download the export file — it holds the header row only, contradicting the recommendations just reported for the same resources.

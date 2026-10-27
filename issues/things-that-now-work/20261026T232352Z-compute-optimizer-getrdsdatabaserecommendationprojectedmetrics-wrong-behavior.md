Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# GetRDSDatabaseRecommendationProjectedMetrics skips the analysis-due gate

_Filed: 2026-10-26T23:23:52Z_
_Operation: GetRDSDatabaseRecommendationProjectedMetrics | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler looks the database up in `guard.analysed_resources` after checking only `is_opted_in()`, skipping the `analysis_due()` half of the visibility gate. Its direct counterpart `GetEC2RecommendationProjectedMetrics` reads through `guard.visible_instances()`, which applies both halves and therefore answers `ResourceNotFoundException` until the analysis the enrollment started is due. So within `ANALYSIS_DELAY` of opting in, the EC2 projected-metrics reader reports "a resource that is required for the action doesn't exist" while the RDS one returns a full projected series for a resource the crate's own rule says has not been analysed yet.

## Evidence

The sibling projected-metrics reader goes through the gate — src/ops/get_ec2_recommendation_projected_metrics.rs:306-308:
```
        let Some(instance) = guard
            .visible_instances()
            .find(|instance| instance.instance_arn(&region) == input.instance_arn)
```
where `visible_instances` is src/state.rs:6616-6619:
```
    pub fn visible_instances(&self) -> impl Iterator<Item = &ObservedInstance> {
        let visible = self.enrollment.is_opted_in() && self.enrollment.analysis_due();
        self.instances.values().filter(move |_| visible)
    }
```
with the rule stated at src/state.rs:6609-6613: "only once the analysis that enrollment starts is due ([`ANALYSIS_DELAY`]), so an account that never opted in, one that opted out, and one that has just opted in all read the empty set rather than an analysis nothing has produced yet."

The handler under review bypasses it — src/ops/get_rds_database_recommendation_projected_metrics.rs:266 and :282-285:
```
        if !guard.enrollment.is_opted_in() {
...
        let Some(resource) = guard.analysed_resources.iter().find(|resource| {
            resource.resource_type == crate::state::SOURCE_TYPE_RDS_DB_INSTANCE
                && resource.arn_in(&region) == input.resource_arn
        }) else {
```
Test coverage: tests/all/ec2_recommendation_projected_metrics.rs:257-260 sleeps `ANALYSIS_DELAY + 100ms` before reading, whereas tests/all/rds_database_recommendation_projected_metrics.rs needs no such wait (`ANALYSIS_DELAY` is imported by nine test modules, not by that one).

## Steps to Reproduce

1. Start the emulator and call `UpdateEnrollmentStatus` with `status=Active`.
2. Within `ANALYSIS_DELAY` (500 ms) call `GetEC2RecommendationProjectedMetrics` for a seeded instance ARN with a valid `stat`, `period`, `startTime`, `endTime` — observe HTTP 404 `ResourceNotFoundException`.
3. In the same window call `GetRDSDatabaseRecommendationProjectedMetrics` for a seeded database ARN with the same window parameters — observe HTTP 200 with a populated `recommendedOptionProjectedMetrics`.
4. The two projected-metrics readers of the same just-enrolled account disagree about whether any analysis exists.

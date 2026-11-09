Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-assessment-of-published-app-reports-compliance
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-assessment-of-published-app-reports-compliance
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|StartAppAssessment|2xx->2xx|$.assessment.startTime
Issue-Key: resiliencehub|StartAppAssessment|2xx->2xx|$.assessment.versionName
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.AZ.achievableRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.AZ.achievableRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.AZ.currentRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.AZ.currentRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Hardware.achievableRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Hardware.achievableRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Hardware.currentRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Hardware.currentRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Software.achievableRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Software.achievableRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Software.complianceStatus
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Software.currentRpoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.compliance.Software.currentRtoInSecs
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.complianceStatus
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.cost
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.driftStatus
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.endTime
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.startTime
Issue-Key: resiliencehub|DescribeAppAssessment|2xx->2xx|$.assessment.versionName
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.complianceStatus
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.creationTime
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.driftStatus
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.lastAppComplianceEvaluationTime
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.lastDriftEvaluationTime
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.lastResiliencyScoreEvaluationTime
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.rpoInSecs
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.rtoInSecs
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# resiliencehub assessment and app compliance rollup diverge from AWS: policy objectives echoed as assessed values, cost/versionName/rpoInSecs omitted, drift never evaluated

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAppAssessment

## Description
Six root causes, all in the assessment path of a PUBLISHED application and in the application record that assessment feeds. They are bundled because they are one fault surface — the `AppAssessment` this service computes, and the `App` projection of it — and every one of them is reached by the same recorded sequence (create policy, create app, compose and resolve a draft, publish, StartAppAssessment on `release`, DescribeAppAssessment, DescribeApp). A fixer must close ALL SIX before submitting; they are numbered below and each names the fixture step that exercises it.

Cause 1 of 6 — DisruptionCompliance echoes the policy's objectives as the ASSESSED recovery times, so a fully mapped application can never be reported non-compliant.
AWS: Smithy `com.amazonaws.resiliencehub#DisruptionCompliance` models `currentRtoInSecs`/`currentRpoInSecs` as "The current RTO, in seconds" / "The current RPO, in seconds" and `achievableRtoInSecs`/`achievableRpoInSecs` as the RTO/RPO "that is achievable", with `complianceStatus` (required, enum `ComplianceStatus`: PolicyBreached|PolicyMet|NotApplicable|MissingPolicy) "The current status of compliance for the resiliency policy". `AppAssessment.complianceStatus` is the rollup. These are values the assessment MEASURES about the application; the policy's `FailurePolicy.rtoInSecs`/`rpoInSecs` are the OBJECTIVES they are compared against — the two are independent, which is what makes a breach expressible at all. Recorded behavior: for an app whose components are all mapped, AWS reports the AZ and Hardware entries with achievableRpoInSecs/achievableRtoInSecs/currentRpoInSecs/currentRtoInSecs all 0, and the Software entry with achievable 0, current one second beyond the policy's Software objective, and `complianceStatus` `PolicyBreached`; the assessment's `complianceStatus` rolls up to `PolicyBreached`. (Hypothesis: AWS expresses a software-disruption recovery it cannot meet as the objective + 1s; the REQUIREMENT the recording establishes is only that current/achievable are assessed values, not copies of the objective, and that an entry whose current value exceeds the objective is PolicyBreached.)
Emulator: `disruption_compliance` assigns `current_rto_in_secs = objective.rto_in_secs`, `achievable_rto_in_secs = objective.rto_in_secs`, and likewise for RPO, with `complianceStatus` hard-set to `PolicyMet`, for every disruption type the policy states — unless some Application Component has no mapped resource, which is the only path to `PolicyBreached`. Replay: every one of the three entries (AZ, Hardware, Software) came back with all four members equal to 2592000 (the policy objective) where the recording has 0/0/0/0 for AZ and Hardware and 2592001 for the Software current pair; the Software entry and the assessment reported `PolicyMet` where AWS reported `PolicyBreached`.
Why: current == objective is a tautology, so compliance is unfalsifiable: a client that assesses an application to decide whether its resiliency policy is breached always reads `PolicyMet` from this emulator whenever every component has a resource mapped, and the achievable/current pair it renders or alerts on carries the numbers the client itself supplied in the policy rather than anything about the application. Breach-driven code paths (alarms, gating a release, drift reporting) can never be exercised.
Fix: the compliance computation in the StartAppAssessment handler — `disruption_compliance` must derive current/achievable from the assessed version's resources and set each entry's `complianceStatus` by comparing the derived current values against that disruption type's objective, leaving `overall_compliance_status`'s rollup intact.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#10 (DescribeAppAssessment), and #task#11 (DescribeApp, which republishes the same wrong rollup as `app.complianceStatus`).

Cause 2 of 6 — the assessment omits `cost` entirely.
AWS: Smithy `AppAssessment.cost` targets `Cost`, documented "Cost for the application", whose three members `amount` (Double), `currency` (CurrencyCode) and `frequency` (CostFrequency) are all REQUIRED — so the member is either absent or a complete object. Recorded behavior: a finished assessment carries `cost` = {amount, currency `USD`, frequency `Monthly`}; the recorded amount for this one-bucket application is 0.0.
Emulator: the handler sets `cost: None` with a comment that it holds no pricing, so the key is absent from both the StartAppAssessment response and every DescribeAppAssessment read of the frozen record.
Why: a client that reports the cost of resiliency alongside the score reads `assessment.cost.amount` and gets a missing key (or a null in a typed SDK) on a `Success` assessment, so cost reporting cannot run at all. "No pricing data" is not an absent cost in AWS; it is a cost object whose amount is 0 in a stated currency and frequency.
Fix: the assessment record built by the StartAppAssessment handler must populate `cost` with a complete `Cost` (currency and frequency stated, amount derived from the assessed resources, 0 when nothing is priced), and the frozen record must carry it so the DescribeAppAssessment read replays it.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#10.

Cause 3 of 6 — a completed assessment reports `driftStatus` `NotChecked`, and the application never records `lastDriftEvaluationTime`.
AWS: Smithy `AppAssessment.driftStatus` (enum `DriftStatus`) "Indicates if compliance drifts (deviations) were detected while running an assessment for your application", `App.driftStatus` (enum `AppDriftStatusType`: NotChecked|NotDetected|Detected) the same, and `App.lastDriftEvaluationTime` "Indicates the last time that a drift was evaluated". Running an assessment IS the drift evaluation, so after one the status is a verdict, not `NotChecked`. Recorded behavior: after the assessment completes, both the assessment and the application report `driftStatus` `NotDetected`, and the application carries a `lastDriftEvaluationTime`.
Emulator: the handler hard-sets `drift_status: Some(DRIFT_STATUS_NOT_CHECKED)` on the assessment "because no assessment is compared against an earlier one", and the `App` projection leaves `driftStatus` at `NotChecked` and `lastDriftEvaluationTime` absent.
Why: `NotChecked` tells a client the drift question was never asked, so drift dashboards and drift-event subscriptions treat an assessed application as unevaluated forever and never reach the `NotDetected`/`Detected` branches; code that reads `lastDriftEvaluationTime` to decide whether an evaluation is stale gets a missing key and cannot schedule.
Fix: the StartAppAssessment handler (and the App record it updates) must set the drift verdict from the comparison the assessment performs — `NotDetected` when the completed assessment finds no deviation from the prior assessment of the application, `Detected` when it does — and stamp `App.lastDriftEvaluationTime` with that evaluation's time.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#10 (assessment.driftStatus) and #task#11 (app.driftStatus, app.lastDriftEvaluationTime).

Cause 4 of 6 — a published version gets no `versionName`, so the assessment reports none.
AWS: Smithy `AppAssessment.versionName` (`EntityVersion`, pattern `^\S{1,50}$`) is "Version name of the published application"; `PublishAppVersion`'s `versionName` input is OPTIONAL. Recorded behavior: the fixture publishes WITHOUT a `versionName`, and both StartAppAssessment and DescribeAppAssessment then report `versionName` `Version-1` for that first published snapshot — i.e. the service names an unnamed published version itself. (Hypothesis: the default is `Version-<publish sequence number>`; the requirement the recording establishes is that a published version always has a non-null name that the assessment of it reports.)
Emulator: `PublishAppVersion` stores `version_name: input.version_name.clone()` verbatim, so an omitted name is stored as `None`, and the assessment's `version_name: assessed.version_name.clone()` is therefore absent from both assessment responses.
Why: a client that labels assessment results by the release they graded ("the assessment of Version-1 breached") reads a missing key and cannot tell two published snapshots apart in its reporting, even though the assessment did pin a distinct snapshot.
Fix: the PublishAppVersion handler's state write must assign a default version name when the request omits one, so the stored snapshot carries it and every reader of the published version — the assessment record included — reports it.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#9 (StartAppAssessment) and #task#10 (DescribeAppAssessment).

Cause 5 of 6 — DescribeApp omits `rpoInSecs` and `rtoInSecs` after an assessment.
AWS: Smithy `App.rtoInSecs` / `App.rpoInSecs` (`IntegerOptional`) are "Recovery Time Objective (RTO) in seconds" / "Recovery Point Objective (RPO) in seconds" on the application record, alongside `lastAppComplianceEvaluationTime`. Recorded behavior: after the assessment, `app.rpoInSecs` and `app.rtoInSecs` are both present and equal the worst assessed current value across the assessment's disruption entries (the Software current value in this recording).
Emulator: the App projection sets `lastAppComplianceEvaluationTime` and `lastResiliencyScoreEvaluationTime` from the assessment but never sets `rpoInSecs`/`rtoInSecs`, so both keys are absent from DescribeApp.
Why: the application summary is where a client reads the application's effective RTO/RPO without fetching an assessment; against this emulator those two members are always missing, so an inventory or compliance view over many applications has nothing to show and cannot compare an application against its objectives.
Fix: the state write-back that projects a completed assessment onto the `App` record (read by DescribeApp/ListApps) must also carry the assessed RTO/RPO rollup; it depends on Cause 1, since the values to roll up are the assessed current values.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#11.

Cause 6 of 6 — assessment and application timestamps are serialized in a shape AWS does not use.
AWS: `AppAssessment.startTime`/`endTime` and `App.creationTime`/`lastAppComplianceEvaluationTime`/`lastResiliencyScoreEvaluationTime` all target `TimeStamp` and are carried in restJson1's default epoch-seconds encoding. Recorded behavior only: the corpus does not assert these values (they are per-run clock readings) but DOES assert their SHAPE, and the emulator's serialization fails that shape check on every one of them. (Hypothesis: AWS's epoch-seconds numbers carry at most millisecond fractional precision, while the emulator emits the full sub-second precision of the host clock — e.g. seven fractional digits. The requirement is the encoding's shape, not the instant: a fix is confirmed by the shape assertion passing, not by matching a recorded number.)
Emulator: `start_time` / `end_time` are built with `DateTime::from(SystemTime::now())` (plus a fixed span) and the App times likewise, and serialize as epoch-seconds floats with microsecond-level fraction.
Why: a client that parses these fields with AWS-shaped expectations (a fixed-precision epoch decode, or a strict schema on a captured response) rejects or mis-parses every timestamp this service emits, even though the instant is right.
Fix: the timestamp serializer for this service's responses (shared, not per-operation) must emit `TimeStamp` members in AWS's epoch-seconds shape; truncate the stored instant to that precision when the record is built rather than at each read, so a start/end pair stays consistent across StartAppAssessment and DescribeAppAssessment.
Traces: resiliencehub-assessment-of-published-app-reports-compliance#task#9, #task#10 and #task#11.

All six causes are 200-response divergences: no HTTP status, error code or error message is involved in any of them.

Related open issue, which does NOT cover any of the above: `20261104T205441Z-resiliencehub-startappassessment-omits-resiliencyscore-componentscore-so-a-finished.md` concerns the omitted `resiliencyScore.componentScore` of the same assessment (and records that the compliance rollup matched in the behavioral suite — it matched only because that check's application had an unmet component, the one path on which this emulator does report `PolicyBreached`). The replay reported these three clusters as owned by no open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator, fixture `resiliencehub-assessment-of-published-app-reports-compliance` (3 divergence clusters, none owned, no stub).

Step task#9 — StartAppAssessment, expected 200, actual 200:
  $.assessment.versionName: missing (expected 'Version-1', got None)
  $.assessment.startTime: value_mismatch (expected '<shaped-not-asserted>', got 1794227898.5605032)

Step task#10 — DescribeAppAssessment, expected 200, actual 200:
  $.assessment.compliance.AZ.achievableRpoInSecs: expected 0, got 2592000
  $.assessment.compliance.AZ.achievableRtoInSecs: expected 0, got 2592000
  $.assessment.compliance.AZ.currentRpoInSecs: expected 0, got 2592000
  $.assessment.compliance.AZ.currentRtoInSecs: expected 0, got 2592000
  $.assessment.compliance.Hardware.achievableRpoInSecs: expected 0, got 2592000
  $.assessment.compliance.Hardware.achievableRtoInSecs: expected 0, got 2592000
  $.assessment.compliance.Hardware.currentRpoInSecs: expected 0, got 2592000
  $.assessment.compliance.Hardware.currentRtoInSecs: expected 0, got 2592000
  $.assessment.compliance.Software.achievableRpoInSecs: expected 0, got 2592000
  $.assessment.compliance.Software.achievableRtoInSecs: expected 0, got 2592000
  $.assessment.compliance.Software.currentRpoInSecs: expected 2592001, got 2592000
  $.assessment.compliance.Software.currentRtoInSecs: expected 2592001, got 2592000
  $.assessment.compliance.Software.complianceStatus: expected 'PolicyBreached', got 'PolicyMet'
  $.assessment.complianceStatus: expected 'PolicyBreached', got 'PolicyMet'
  $.assessment.cost: missing (expected {'amount': 0.0, 'currency': 'USD', 'frequency': 'Monthly'}, got None)
  $.assessment.driftStatus: expected 'NotDetected', got 'NotChecked'
  $.assessment.versionName: missing (expected 'Version-1', got None)
  $.assessment.startTime / $.assessment.endTime: value_mismatch (expected '<shaped-not-asserted>', got 1794227898.5605032 / 1794227901.5605032)

Step task#11 — DescribeApp, expected 200, actual 200:
  $.app.complianceStatus: expected 'PolicyBreached', got 'PolicyMet'
  $.app.driftStatus: expected 'NotDetected', got 'NotChecked'
  $.app.lastDriftEvaluationTime: missing (expected '<shaped-not-asserted>', got None)
  $.app.rpoInSecs: missing (expected 2592001, got None)
  $.app.rtoInSecs: missing (expected 2592001, got None)
  $.app.creationTime / lastAppComplianceEvaluationTime / lastResiliencyScoreEvaluationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794227895.477303 / 1794227901.5605032 / 1794227901.5605032)

Model checked with smithy_query on services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json: operations StartAppAssessment/DescribeAppAssessment/DescribeApp and shapes AppAssessment, App, DisruptionCompliance, Cost, ComplianceStatus, AppDriftStatusType (Cost.amount/currency/frequency all required; DisruptionCompliance.complianceStatus required; App.rtoInSecs/rpoInSecs IntegerOptional; AppDriftStatusType = NotChecked|NotDetected|Detected).

Emulator source read while grounding the causes: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/start_app_assessment.rs lines 255-290 (`cost: None`, `drift_status: NOT_CHECKED`, `version_name: assessed.version_name`, `start_time`/`end_time` from SystemTime::now()) and lines 323-359 (`disruption_compliance` copying `objective.rto_in_secs`/`rpo_in_secs` into both the current and achievable members with `compliance_status = PolicyMet`); services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs line 215 (`version_name: input.version_name.clone()`).

## Steps to Reproduce
1. Boot the emulator and replay fixture `resiliencehub-assessment-of-published-app-reports-compliance` (steps task#9, task#10, task#11).
2. By hand: CreateResiliencyPolicy with objectives for AZ, Hardware and Software (each rtoInSecs/rpoInSecs 2592000); CreateApp naming that policyArn.
3. CreateAppVersionAppComponent for a storage component and CreateAppVersionResource mapping one S3 bucket into it, so NO component is left unmapped; ResolveAppVersionResources on `draft` and poll to Success; PublishAppVersion WITHOUT a versionName.
4. StartAppAssessment with appVersion `release`. Observe the response has no `versionName`, and that `startTime` is an epoch-seconds float with sub-millisecond precision.
5. DescribeAppAssessment on the returned assessmentArn. Observe every compliance entry reports current == achievable == the policy objective (2592000) with complianceStatus `PolicyMet`, the assessment complianceStatus `PolicyMet`, no `cost` key, `driftStatus` `NotChecked`, and no `versionName`. AWS reports AZ/Hardware zeros, Software current beyond the objective with `PolicyBreached`, a `cost` object (USD/Monthly), `driftStatus` `NotDetected` and `versionName`.
6. DescribeApp on the appArn. Observe `complianceStatus` `PolicyMet`, `driftStatus` `NotChecked`, and no `rpoInSecs`, `rtoInSecs` or `lastDriftEvaluationTime` keys.

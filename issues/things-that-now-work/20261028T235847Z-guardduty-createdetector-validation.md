Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 964c2ed1f803ffeae6e06375ca6c19c7cce42b54

# One CreateDetector naming only RUNTIME_MONITORING leaves a detector with both runtime-monitoring features ENABLED, the state the model says "will cause an error"

_Filed: 2026-10-28T23:58:47Z_
_Operation: CreateDetector | Class: validation_

> _Note: this issue was identified against the code base at commit `964c2ed1f803ffeae6e06375ca6c19c7cce42b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDetector` builds the detector's feature list from `crate::state::default_features(now)` and then overlays the request's `Features` entries. `default_features` enables every feature that is not one of the two `OPT_IN_FEATURES`, and `EKS_RUNTIME_MONITORING` is NOT one of them — so every detector starts with `EKS_RUNTIME_MONITORING = ENABLED`.

The documented mutual exclusion between `EKS_RUNTIME_MONITORING` and `RUNTIME_MONITORING` is checked by `validate_feature_configurations`, which calls `validate_runtime_monitoring_pair` over the REQUEST's entries only. A create whose `features` names just `{"name":"RUNTIME_MONITORING","status":"ENABLED"}` names only one of the two, passes that check, and is then written over the default list — so the detector the single call produces reports BOTH `EKS_RUNTIME_MONITORING` and `RUNTIME_MONITORING` as `ENABLED` from `GetDetector`.

That is the combination the model says is an error, and the combination this crate's own refusal message calls impossible ("cannot both be enabled because RUNTIME_MONITORING already includes threat detection for Amazon EKS resources"). The crate implements the same rule the other way on the organization side: `validate_org_feature_configurations` judges the auto-enablement each feature ENDS UP with, explicitly because its list is also applied as a partial overlay — and the shared primitive's own doc says the rule "holds for a DETECTOR's feature list and for an ORGANIZATION's alike".

Note the defect is the resulting feature STATE, not the status code: `tests/all/coverage_listing.rs:191-192` and `tests/all/coverage_statistics.rs:239-240` pin a 200 for exactly this create body, and neither asserts anything about the two runtime switches afterwards, so neither pins the both-enabled outcome.

This is distinct from the open `UpdateDetector` validation issue: that one reproduces the state across two calls (create, then update); here one `CreateDetector` is enough, and the base being overlaid is `default_features` rather than a stored record.

## Evidence

MODEL (the rule, on this very operation): "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and Runtime Monitoring (RUNTIME_MONITORING) will cause an error. You can add only one of these two features because Runtime Monitoring already includes the threat detection for Amazon EKS resources." — `smithy_query(query="doc",shape="CreateDetector")`. The same sentence is reproduced in the handler's own modelled-contract header.

THE CHECK READS THE REQUEST ONLY — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:5684-5693:
```
fn validate_runtime_monitoring_pair<'a>(
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)> + Clone,
) -> Result<(), String> {
    let named = |wanted: &str| {
        entries
            .clone()
            .any(|(name, status)| name == Some(wanted) && status != Some(DISABLED))
    };
    refuse_runtime_monitoring_overlap(named)
```
called from `validate_feature_configurations` at state.rs:5576, which is handed nothing but `features`.

THE MESSAGE THAT CALLS THE STATE IMPOSSIBLE — state.rs:5504-5508:
```
pub fn refuse_runtime_monitoring_overlap(asked_for: impl Fn(&str) -> bool) -> Result<(), String> {
    if asked_for(FEATURE_EKS_RUNTIME_MONITORING) && asked_for(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
```

THE BASE THAT ALREADY HAS EKS ON — state.rs:1259:
```
pub const OPT_IN_FEATURES: [&str; 2] = [FEATURE_RUNTIME_MONITORING, FEATURE_RDS_LOGIN_EVENTS];
```
and state.rs:5524-5532:
```
pub fn default_features(now: i64) -> Vec<FeatureRecord> {
    reported_features()
        .into_iter()
        .map(|name| {
            let status = if OPT_IN_FEATURES.contains(&name) {
                DISABLED
            } else {
                ENABLED
            };
```

THE HANDLER — src/ops/create_detector.rs:171-176 validates the request alone:
```
        // Refuse an unacceptable feature list before anything is written, so a
        // rejected request leaves no detector behind.
        if let Err(message) = crate::state::validate_feature_configurations(input.features.as_ref())
        {
            return Err(bad_request(message));
        }
```
then builds the record with `features: crate::state::default_features(now),` (create_detector.rs:250) and overlays the request at create_detector.rs:287-288:
```
        crate::state::apply_data_sources(&mut detector, input.data_sources.as_ref(), now);
        crate::state::apply_feature_configurations(&mut detector, input.features.as_ref(), now);
```
where the applier is PARTIAL — state.rs:5808-5809:
```
/// Write a `Features` list onto `detector`. A feature the list does not name keeps
/// the status it already had.
```

THE SAME RULE, DONE THE OTHER WAY IN THIS CRATE — state.rs:5502-5503 says the rule is shared ("so each caller supplies `asked_for` — whether its list asks for the feature it names — and the rule and its message live here once"), and the organization caller resolves the value each feature ends up with rather than what the request mentions.

TEST COVERAGE of the affected path: tests/all/detector_lifecycle.rs:530-545 pins the refusal only for a create naming BOTH entries explicitly; tests/all/detector_data_sources_projection.rs:400-401 is the one create that disambiguates the pair by hand (`RUNTIME_MONITORING ENABLED` + `EKS_RUNTIME_MONITORING DISABLED`). tests/all/coverage_listing.rs:191-192 and tests/all/coverage_statistics.rs:239-240 create with only `RUNTIME_MONITORING ENABLED` and assert the 200 and an empty coverage report — neither asserts the resulting feature statuses, so no test pins the both-enabled state.

## Steps to Reproduce

1. POST /detector with body {"enable": true, "features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"}]} — one of the two mutually exclusive features is named, so `validate_feature_configurations` (create_detector.rs:173) passes.
2. The record is built with `default_features` (create_detector.rs:250), which sets `EKS_RUNTIME_MONITORING = ENABLED` because it is not in `OPT_IN_FEATURES` (state.rs:1259).
3. `apply_feature_configurations` (create_detector.rs:288) then sets `RUNTIME_MONITORING = ENABLED` and leaves every feature it does not name alone (state.rs:5808-5809).
4. GET /detector/{detectorId}: `features` reports BOTH `EKS_RUNTIME_MONITORING` and `RUNTIME_MONITORING` as `ENABLED` — the combination the model says "will cause an error" and `refuse_runtime_monitoring_overlap` (state.rs:5504-5508) words as impossible.
5. Contrast: POST /detector with both entries named explicitly IS refused (tests/all/detector_lifecycle.rs:530-545), and POST /detector/{detectorId}/admin with {"features":[{"name":"RUNTIME_MONITORING","autoEnable":"ALL"}]} while `EKS_RUNTIME_MONITORING` is already auto-enabled IS refused, because the organization validator consults the stored configuration.

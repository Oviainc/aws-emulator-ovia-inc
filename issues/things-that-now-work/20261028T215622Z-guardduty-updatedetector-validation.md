Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# The RUNTIME_MONITORING / EKS_RUNTIME_MONITORING exclusion is judged on the request alone, so one UpdateDetector leaves a default detector with both features enabled

_Filed: 2026-10-28T21:56:22Z_
_Operation: UpdateDetector | Class: validation_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDetector` is a PARTIAL update of the detector's feature switches: a feature the request does not name keeps the status it already had. The documented mutual exclusion between `EKS_RUNTIME_MONITORING` and `RUNTIME_MONITORING` is nevertheless evaluated over the REQUEST's entries only, before the store is even locked, so the handler never compares the request against the switches the detector already holds.

That makes the forbidden combination reachable in a single call on a freshly created detector. `crate::state::default_features` enables every optional feature except the two `OPT_IN_FEATURES` (`RUNTIME_MONITORING`, `RDS_LOGIN_EVENTS`), so a detector created without a `Features` list already carries `EKS_RUNTIME_MONITORING = ENABLED`. An `UpdateDetector` naming only `{RUNTIME_MONITORING, ENABLED}` names one of the two features, passes `validate_runtime_monitoring_pair`, and is written — leaving `GetDetector` reporting both features ENABLED, the state the crate's own refusal message says cannot exist ("cannot both be enabled").

The crate implements the SAME rule correctly on the organization side, and says so in that function's own doc: `validate_org_feature_configurations` judges "the auto-enablement each feature ENDS UP with, not on what the request mentions", explicitly because its list is also a partial update. The shared primitive's doc states the rule "holds for a DETECTOR's feature list and for an ORGANIZATION's alike" — yet only the organization caller resolves the stored value. The member-account caller `validate_member_feature_configurations` (used by `UpdateMemberDetectors`) has the same request-only shape and the same partial-update applier.

## Evidence

MODEL (the rule): "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and Runtime Monitoring (RUNTIME_MONITORING) will cause an error. You can add only one of these two features because Runtime Monitoring already includes the threat detection for Amazon EKS resources." — `smithy_query(query="doc",shape="UpdateDetector")`.

THE SHARED PRIMITIVE says the rule spans both sides — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:5480-5490:
```
/// "You can add only one of these two features because Runtime Monitoring already
/// includes the threat detection for Amazon EKS resources"
/// (`smithy_query(query="doc",shape="UpdateOrganizationConfiguration")`, and the
/// same sentence on `CreateDetector`), so the rule holds for a DETECTOR's feature
/// list and for an ORGANIZATION's alike.
pub fn refuse_runtime_monitoring_overlap(asked_for: impl Fn(&str) -> bool) -> Result<(), String> {
    if asked_for(FEATURE_EKS_RUNTIME_MONITORING) && asked_for(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
```

THE DETECTOR SIDE reads the REQUEST only — state.rs:5667-5675:
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
called from state.rs:5559 (`validate_feature_configurations`) and state.rs:5602 (`validate_member_feature_configurations`), neither of which is handed the stored record.

THE APPLIER IS PARTIAL — state.rs:5791-5792:
```
/// Write a `Features` list onto `detector`. A feature the list does not name keeps
/// the status it already had.
```

THE ORGANIZATION SIDE, in the same module, does it the other way AND states why — state.rs:6604-6610:
```
/// The rule is judged on the auto-enablement each feature ENDS UP with, not on what
/// the request mentions: the request's value where it states one, and `configuration`'s
/// stored value otherwise (a list that omits a feature, or names it without an
/// `AutoEnable`, leaves that switch alone — see
/// [`apply_org_feature_configurations`]). So enabling one of the two while the other
/// is already stored enabled is refused
```

THE DEFAULT THAT MAKES IT ONE CALL — state.rs:1259 `pub const OPT_IN_FEATURES: [&str; 2] = [FEATURE_RUNTIME_MONITORING, FEATURE_RDS_LOGIN_EVENTS];` and state.rs:5507-5515 (`default_features`: every feature NOT in `OPT_IN_FEATURES` starts `ENABLED`), so `EKS_RUNTIME_MONITORING` is ENABLED on a detector created with no `Features`.

THE HANDLER validates before the lock — src/ops/update_detector.rs:169-174:
```
        // Refuse an unacceptable feature list before anything is written, so a
        // rejected update leaves the detector exactly as it was.
        if let Err(message) = crate::state::validate_feature_configurations(input.features.as_ref())
        {
            return Err(bad_request(message));
        }
```
and then writes at update_detector.rs:211-212 via `apply_feature_configurations`. Same shape in src/ops/update_member_detectors.rs:173 region (`validate_member_feature_configurations`) with `apply_member_feature_configurations` (state.rs:5827-5830, also partial).

## Steps to Reproduce

1. `POST /detector` with `{"enable": true}` and no `features` member. `default_features` gives the detector `EKS_RUNTIME_MONITORING = ENABLED` and `RUNTIME_MONITORING = DISABLED`.
2. `GET /detector/{detectorId}` and confirm `features` reports `EKS_RUNTIME_MONITORING` as `ENABLED`.
3. `POST /detector/{detectorId}` (UpdateDetector) with `{"features": [{"name": "RUNTIME_MONITORING", "status": "ENABLED"}]}`. `validate_feature_configurations` sees only one of the two names in the request, so `validate_runtime_monitoring_pair` passes and the switch is written.
4. `GET /detector/{detectorId}` again: BOTH `EKS_RUNTIME_MONITORING` and `RUNTIME_MONITORING` now report `ENABLED` — the combination the model says "will cause an error" and the crate's own message calls impossible.
5. Contrast with the organization side: `POST /detector/{detectorId}/admin` (UpdateOrganizationConfiguration) with `{"features":[{"name":"RUNTIME_MONITORING","autoEnable":"ALL"}]}` while `EKS_RUNTIME_MONITORING` is already stored auto-enabled IS refused, because `validate_org_feature_configurations` consults the stored configuration.
6. Same single-call reproduction for a member: `CreateMembers`, then `POST /detector/{detectorId}/member/detector/update` with only `{"name":"RUNTIME_MONITORING","status":"ENABLED"}`; `GetMemberDetectors` then reports both runtime features enabled for that member.

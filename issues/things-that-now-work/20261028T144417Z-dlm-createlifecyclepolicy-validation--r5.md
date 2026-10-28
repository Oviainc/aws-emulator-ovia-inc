Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6c1cc1a79be554bed82ba757ddf92c753ad05f12

# A schedule that specifies a FastRestoreRule, ShareRule or CrossRegionCopyRule and OMITS the RetainRule entirely is accepted, while the same schedule with a retention of 0 is refused

_Filed: 2026-10-28T14:44:17Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `6c1cc1a79be554bed82ba757ddf92c753ad05f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RetainRule`'s documentation makes specifying a retention MANDATORY when the same schedule carries a rule acting on the snapshots it keeps: "If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of `1` or more" (count-based) and "...then you must specify a retention interval of `1` day or more" (age-based).

The crate enforces that floor only when a `RetainRule` structure is PRESENT. `retain_rule_violation` opens with `let rule = schedule.retain_rule.as_ref()?;`, so a schedule that carries no `RetainRule` at all returns `None` before `retention_floor_violation` / `rules_requiring_retention` are ever consulted — even when the schedule carries a `FastRestoreRule`, a non-empty `ShareRules` or a non-empty `CrossRegionCopyRules`.

The result is self-contradictory inside this crate: a schedule stating `RetainRule: {Count: 0}` beside a `CrossRegionCopyRule` is refused with HTTP 400 (that is the behaviour the floor helper was added for), but the SAME schedule with the `RetainRule` member simply left out — which retains strictly less, i.e. nothing at all, and specifies no retention count whatsoever — is accepted with HTTP 200 and persisted. `CreateLifecyclePolicy` stores it and both reads report it; `UpdateLifecyclePolicy` calls the identical helper and accepts it just as readily. So the refusal can be evaded by deleting the member the documentation says must be specified.

The handler's own stated reason for the early return is contradicted by the model sentence in exactly this case: it claims "no documented sentence requires one", while the quoted `RetainRule` paragraph does require one whenever the schedule specifies any of the three named rules. Note the crate ALREADY computes which rules raise the floor (`rules_requiring_retention(schedule)`) and already receives the whole `&Schedule`, so the data needed is in hand at the point of the early return.

This is NOT the already-fixed `Count: 0` / `Interval: 0` case: every row of the existing fixed issue and of `tests/all/schedule_retention_floor.rs` states a `RetainRule` (its `schedule_with` helper always sets one). The absent-`RetainRule` row that IS pinned as acceptable carries no dependent rule at all.

## Evidence

Model, `smithy_query(query="doc", shape="RetainRule")` — the sentence requires the member to be SPECIFIED, in both modes:

> "**Count-based retention** You must specify **Count**. If you specify an ArchiveRule for the schedule, then you can specify a retention count of `0` to archive snapshots immediately after creation. If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of `1` or more."

> "**Age-based retention** You must specify **Interval** and **IntervalUnit**. ... If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention interval of `1` day or more."

The early return that skips the floor, with the stated reason the sentence above contradicts — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1731-1741`:
```
/// The whole `schedule` is judged, not the retention rule alone: the floor on how
/// much a rule may retain depends on which OTHER rules the same schedule carries
/// ([`retention_floor_violation`]). A schedule carrying no retention rule is
/// refused nothing here — the model declares none for a schedule, and no
/// documented sentence requires one.
fn retain_rule_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let rule = schedule.retain_rule.as_ref()?;
```

The floor the crate DOES enforce one branch later, i.e. the other side of the inconsistency — `src/state.rs:1817-1835`:
```
fn retention_floor_violation(
    label: &str,
    schedule: &Schedule,
    member: &str,
    retained: i32,
    requirement: &str,
) -> Option<InvalidRequestException> {
    if retained >= RETENTION_FLOOR_WITH_DEPENDENT_RULE {
        return None;
    }
    let dependent = rules_requiring_retention(schedule);
    if dependent.is_empty() {
        return None;
    }
    Some(invalid_request(format!(
        "The parameter RetainRule.{member} in schedule {{{label}}} was {retained}. A schedule \
         that specifies {} must specify {requirement}.",
```
and the helper that already answers "which rules raise the floor", reachable from the early-returning function — `src/state.rs:1848-1864`:
```
fn rules_requiring_retention(schedule: &Schedule) -> Vec<&'static str> {
    [
        ("FastRestoreRule", schedule.fast_restore_rule.is_some()),
        (
            "ShareRule",
            schedule.share_rules.as_ref().is_some_and(|rules| !rules.is_empty()),
        ),
        (
            "CrossRegionCopyRule",
            schedule.cross_region_copy_rules.as_ref().is_some_and(|rules| !rules.is_empty()),
        ),
```

The two write paths that accept the configuration — `src/ops/create_lifecycle_policy.rs:314-320`:
```
        if let Some(refusal) =
            crate::state::schedule_rule_violation(input.policy_details.as_ref())
        {
            return Err(
                CreateLifecyclePolicyErrorModeled::InvalidRequestException(refusal).into(),
            );
        }
```
and the identical call on the update path, `src/ops/update_lifecycle_policy.rs:191-197`.

Nothing materializes a `RetainRule` either, so the refused combination is what is stored and read back — `src/state.rs:415-417`:
```
        if let Some(retain_rule) = schedule.retain_rule.as_mut() {
            retain_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
```

Test coverage: the row that pins an absent `RetainRule` as acceptable carries NO dependent rule, and its stated reason is the same one the model sentence contradicts — `tests/all/policy_configuration_validation.rs:821-838`:
```
/// The absence of a `RetainRule` is not refused: the model declares no retention
/// rule for a schedule, and nothing in the documentation requires one.
#[tokio::test]
async fn a_retain_rule_retains_by_exactly_one_mode() {
    assert_rule_cases(vec![
        RuleCase {
            case: "a schedule carrying no RetainRule at all",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
            }),
            required: &[],
```
and the dedicated floor test always sends a `RetainRule`, so the omitted-member combination is pinned by nothing — `tests/all/schedule_retention_floor.rs:260-265`:
```
fn schedule_with(name: &str, rule: &Value, retain_rule: &Value) -> Value {
    let mut schedule = json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": retain_rule.clone(),
    });
```

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "copy with retention zero", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"Name": "copied", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 0}, "CrossRegionCopyRules": [{"Target": "us-west-2", "Encrypted": false}]}]}} — observe HTTP 400 InvalidRequestException naming RetainRule.Count and the CrossRegionCopyRule (the floor the crate enforces today)
2. Repeat the SAME request with the "RetainRule" member deleted entirely: {"Schedules": [{"Name": "copied", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "CrossRegionCopyRules": [{"Target": "us-west-2", "Encrypted": false}]}]} — observe HTTP 200 with a PolicyId. Expected: HTTP 400, because RetainRule documents "If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of 1 or more" and this request specifies no retention count at all
3. GET /policies/{PolicyId} — the policy is reported with a CrossRegionCopyRules entry and no RetainRule, i.e. a schedule that copies snapshots it retains none of is now persisted state a client reads back
4. Repeat step 2 with the CrossRegionCopyRules entry replaced by "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]}, and again with "ShareRules": [{"TargetAccounts": ["111122223333"], "UnshareInterval": 1, "UnshareIntervalUnit": "DAYS"}] — HTTP 200 each time, so none of the three rules the sentence names raises the floor when the RetainRule is absent
5. PATCH /policies/{PolicyId} against a policy created with "RetainRule": {"Count": 7}, sending the step-2 PolicyDetails — also HTTP 200, so the update path (which calls the same crate::state::schedule_rule_violation) patches a valid policy into the refused configuration

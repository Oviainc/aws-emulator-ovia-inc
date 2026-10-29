Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a66b2a2f28102a010a545393b9f4e71052de9805

# A schedule's FastRestoreRule and DeprecateRule are never checked against their documented either/or forms, while the same rule for RetainRule in the same walk is enforced

_Filed: 2026-10-28T22:55:14Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `a66b2a2f28102a010a545393b9f4e71052de9805`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::schedule_rule_violation` is the one walk both write handlers run over `PolicyDetails$Schedules`, and it judges exactly two of a schedule's nested rules: `CreateRule` and `RetainRule`. For those two it implements the documented "state exactly one of the two forms, and state it completely" rule (neither form, both forms and an incomplete form are each refused with `InvalidRequestException`).

The other two rules of the same schedule that the model documents with the SAME kind of sentence are never looked at:

* `FastRestoreRule` — "You can enable fast snapshot restore based on either a count or a time interval." A `FastRestoreRule` that states BOTH a count and an interval, or NEITHER, is accepted and stored, while the byte-for-byte parallel `RetainRule` sentence ("You can retain snapshots based on either a count or a time interval") is what the crate quotes as its reason for refusing both-and-neither one structure higher up. Worse, the crate already treats such a rule as SPECIFIED elsewhere: `rules_requiring_retention` counts `schedule.fast_restore_rule.is_some()` and raises the schedule's retention floor for it, so a `FastRestoreRule` that says nothing about what it enables still forces the schedule to retain one or more snapshots.
* `DeprecateRule` — "For age-based schedules, you must specify Interval and IntervalUnit. For count-based schedules, you must specify Count." `DeprecateRule: {"Interval": 3}` (no `IntervalUnit`), `DeprecateRule: {}`, and a `DeprecateRule` whose mode contradicts the schedule's retention mode are all accepted, stored and read back, while the identical omission inside the schedule's `RetainRule` is refused with "age based retention states both Interval and IntervalUnit". `deprecate_rule` is read nowhere in `src/` outside the type definition.

So two requests alike in the relevant respect — an incomplete or doubly-stated either/or rule nested in one schedule — get opposite answers depending on which rule carries the fault, and the crate persists schedule rules whose form its own stated rule rejects. The helpers the fix needs already exist and are used for the enforced half: `both_forms_stated`, `omitted_members` and `required_in_schedule`.

## Evidence

Model, `smithy_query(query="doc", shape="DeprecateRule")`: "[Custom AMI policies only] Specifies an AMI deprecation rule for AMIs created by an AMI lifecycle policy. **For age-based schedules, you must specify Interval and IntervalUnit. For count-based schedules, you must specify Count.**"

Model, `smithy_query(query="doc", shape="FastRestoreRule")`: "[Custom snapshot policies only] Specifies a rule for enabling fast snapshot restore for snapshots created by snapshot policies. **You can enable fast snapshot restore based on either a count or a time interval.**"

The parallel sentence the crate DOES implement, `smithy_query(query="doc", shape="RetainRule")`: "You can retain snapshots based on either a count or a time interval. ... Count-based retention: You must specify Count ... Age-based retention: You must specify Interval and IntervalUnit."

The one walk, which consults only two of the schedule's rules — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1653-1665`:
```
pub fn schedule_rule_violation(details: Option<&PolicyDetails>) -> Option<InvalidRequestException> {
    details?
        .schedules
        .as_ref()?
        .iter()
        .enumerate()
        .find_map(|(position, schedule)| {
            let label = schedule_label(position, schedule);
            let path = schedule_path(position);
            create_rule_violation(&label, &path, schedule.create_rule.as_ref())
                .or_else(|| retain_rule_violation(&label, &path, schedule))
        })
}
```

The form rule as implemented for `RetainRule` — `src/state.rs:1747-1776`:
```
    let count_based = rule.count.is_some();
    let age_based = rule.interval_unit.is_some()
        || rule.interval.is_some_and(|value| value != UNSET_RULE_INTERVAL);

    match (count_based, age_based) {
        (true, true) => Some(both_forms_stated(
            label,
            "Count based RetainRule and interval based RetainRule",
        )),
        (false, false) => Some(required_in_schedule(
            label,
            vec![format!("{path}.RetainRule")],
            "a RetainRule retains either based on a Count or based on an Interval and \
             IntervalUnit",
        )),
        (false, true) => {
            let omitted = omitted_members(
                &format!("{path}.RetainRule"),
                [
                    ("Interval", rule.interval.is_some()),
                    ("IntervalUnit", rule.interval_unit.is_some()),
                ],
            );
```

The only place a `FastRestoreRule` is read — as a rule that is "specified", raising the retention floor, without its own form ever being judged — `src/state.rs:1889-1891`:
```
fn rules_requiring_retention(schedule: &Schedule) -> Vec<&'static str> {
    [
        ("FastRestoreRule", schedule.fast_restore_rule.is_some()),
```

`deprecate_rule` exists only as a stored field — a repo grep for `deprecate_rule|fast_restore_rule|archive_rule` under `src/` matches only `src/state.rs:1891`, `src/state.rs:1895` and the type declarations `src/ops/types/shared_types.rs:51-55`:
```
    pub fast_restore_rule: Option<FastRestoreRule>,
    pub cross_region_copy_rules: Option<Vec<CrossRegionCopyRule>>,
    pub share_rules: Option<Vec<ShareRule>>,
    pub deprecate_rule: Option<DeprecateRule>,
    pub archive_rule: Option<ArchiveRule>,
```

The crate's own statement of the scope of this rule class — `tests/all/policy_configuration_validation.rs:5-14`:
```
//! The rule under test: a `CreateLifecyclePolicy` or `UpdateLifecyclePolicy`
//! request must state a COMPLETE policy configuration — every member the
//! documentation says the kind of policy it describes is built from, and, where
//! the documentation states two alternative forms, exactly one of them — and a
//! request that does not is refused with `InvalidRequestException` (HTTP 400,
//! `Code` `InvalidParameter`) before anything is stored or changed
```
and the list of rules that header enumerates (`tests/all/policy_configuration_validation.rs:33-38`) names only `CreateRule` and `RetainRule`; no comment, doc or test in the crate states a reason for leaving `FastRestoreRule` / `DeprecateRule` unjudged.

Test coverage: `tests/all/schedule_retention_required.rs` and `tests/all/schedule_retention_floor.rs` build `FastRestoreRule`s only in well-formed shapes (`"FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]}` at `schedule_retention_required.rs:335` and the age-based form at `:340-348`), and a grep for `DeprecateRule` under `tests/` matches no assertion at all — so nothing pins the acceptance of a malformed one.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "ami policy", "State": "ENABLED", "PolicyDetails": {"PolicyType": "IMAGE_MANAGEMENT", "ResourceTypes": ["INSTANCE"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}, "DeprecateRule": {"Interval": 3}}]}} — observe HTTP 200 and a PolicyId; GET /policies/{PolicyId} reports the DeprecateRule back with an Interval and no IntervalUnit, although DeprecateRule documents "you must specify Interval and IntervalUnit" (and this is a count-based schedule, for which it documents "you must specify Count")
2. Repeat with the same omission one level up — "RetainRule": {"Interval": 3} instead of "RetainRule": {"Count": 7} — and observe HTTP 400 InvalidRequestException, Code InvalidParameter, Message naming PolicyDetails.Schedules[1].RetainRule.IntervalUnit ("age based retention states both Interval and IntervalUnit"), produced by src/state.rs:1762-1776
3. Repeat step 1 with "DeprecateRule": {} — a rule stating neither form — and observe HTTP 200, while "RetainRule": {} is refused with "a RetainRule retains either based on a Count or based on an Interval and IntervalUnit"
4. POST /policies with a custom SNAPSHOT policy whose schedule carries "RetainRule": {"Count": 7} and "FastRestoreRule": {"Count": 1, "Interval": 3, "IntervalUnit": "DAYS", "AvailabilityZones": ["us-west-2a"]} — both documented forms at once — and observe HTTP 200, while the same double statement in the RetainRule answers 400 "Count based RetainRule and interval based RetainRule cannot both be present in schedule {daily}."
5. POST /policies with "FastRestoreRule": {"AvailabilityZones": ["us-west-2a"]} (neither a count nor an interval) and "RetainRule": {"Count": 1} — observe HTTP 200; then drop the RetainRule and observe that the same empty-form FastRestoreRule DOES raise the retention requirement (400, from rules_requiring_retention at src/state.rs:1891), i.e. the crate counts the rule as specified while never checking that it specifies anything
6. PATCH /policies/{PolicyId} with any of the malformed rules above — UpdateLifecyclePolicy runs the same crate::state::schedule_rule_violation (src/ops/update_lifecycle_policy.rs:191-197), so it accepts them too

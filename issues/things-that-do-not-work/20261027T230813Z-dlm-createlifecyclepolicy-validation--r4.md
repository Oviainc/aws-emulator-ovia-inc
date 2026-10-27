Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# A schedule carrying a FastRestoreRule, ShareRule or CrossRegionCopyRule is accepted with a retention of 0, which RetainRule documents as refused

_Filed: 2026-10-27T23:08:13Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RetainRule`'s documentation states a floor on the retention a schedule may keep that depends on the OTHER rules the same schedule carries: "If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of 1 or more" (count-based mode) and "If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention interval of 1 day or more" (age-based mode). `StandardTierRetainRuleCount` is `@range(min: 0, max: 1000)` and `StandardTierRetainRuleInterval` is `@range(min: 0)`, so the model lets `0` through and this conditional floor is exactly the kind of cross-member rule this crate states in its own handlers (`Cadence::violation`, `schedule_rule_violation`, `schedule_with_unsupported_create_interval`).

The crate's retention validator cannot apply the rule at all: `retain_rule_violation` is handed only `rule: &RetainRule`, so it never sees whether the schedule holding it also carries a `FastRestoreRule`, a `ShareRule` or a `CrossRegionCopyRules` entry, and it deliberately treats `0` as an admissible value in both modes. `FastRestoreRule` and `ShareRule` are named nowhere in `src/state.rs`; the only place the crate looks at `Schedule$CrossRegionCopyRules` is the per-schedule materialization loop, which walks that very list to stamp each copy rule's `CopyTags`. The data the documented floor is conditional on is therefore already in hand one function away, and nothing in the crate justifies ignoring it.

The consequence is that `CreateLifecyclePolicy` persists — and `GetLifecyclePolicy` / `GetLifecyclePolicies` then report — a schedule the documentation says cannot be created: a cross-Region copy (or fast-restore, or share) rule paired with a retention of `0`, i.e. a schedule that archives/copies snapshots it retains none of. `UpdateLifecyclePolicy` runs the same helper (`src/ops/update_lifecycle_policy.rs:190-196`) and accepts it just as readily, so an existing policy can be patched into that state too.

This is NOT the already-fixed interval-range gap: those issues concerned the `[Default policies only]` body/`PolicyDetails` members `CreateInterval` / `RetainInterval` (the 1-7 / 2-14 day cadence of a DEFAULT policy, now enforced by `crate::state::Cadence`). The members here are `Schedule.RetainRule.Count` and `Schedule.RetainRule.Interval` of a CUSTOM snapshot or AMI policy, which `Cadence` never looks at.

## Evidence

The model states the rule, as two sentences on the shape the handler validates — `smithy_query(query="doc", shape="RetainRule")`:

> "**Count-based retention** You must specify **Count**. If you specify an ArchiveRule for the schedule, then you can specify a retention count of `0` to archive snapshots immediately after creation. If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of `1` or more."

> "**Age-based retention** You must specify **Interval** and **IntervalUnit**. If you specify an ArchiveRule for the schedule, then you can specify a retention interval of `0` days to archive snapshots immediately after creation. If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention interval of `1` day or more."

The validator that admits `0` in both modes, and which is structurally unable to see the three rules the floor is conditional on — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:1662-1700`:
```
fn retain_rule_violation(
    label: &str,
    path: &str,
    rule: &RetainRule,
) -> Option<InvalidRequestException> {
    let count_based = rule.count.is_some();
    let age_based = rule.interval_unit.is_some()
        || rule.interval.is_some_and(|value| value != UNSET_RULE_INTERVAL);

    match (count_based, age_based) {
        ...
        (true, false) => None,
    }
}
```
Its own doc enumerates the rules it enforces and stops short of this one — `src/state.rs:1569-1584`: "Two documented either/or rules, neither of them expressible in the model ... `RetainRule` — \"You can retain snapshots based on either a count or a time interval. Count-based retention: You must specify Count ... Age-based retention: You must specify Interval and IntervalUnit.\"" — the quoted sentence is truncated exactly before the `FastRestoreRule, ShareRule, or a CrossRegionCopyRule` clause.

The caller does hold the whole schedule, so the information is present at the point of the check — `src/state.rs:1591-1600`:
```
        .find_map(|(position, schedule)| {
            let label = schedule_label(position, schedule);
            let path = schedule_path(position);
            create_rule_violation(&label, &path, schedule.create_rule.as_ref()).or_else(|| {
                schedule
                    .retain_rule
                    .as_ref()
                    .and_then(|rule| retain_rule_violation(&label, &path, rule))
            })
        })
```
and the crate already walks `CrossRegionCopyRules` per schedule elsewhere — `src/state.rs:399-401`:
```
        for copy_rule in schedule.cross_region_copy_rules.iter_mut().flatten() {
            copy_rule.copy_tags.get_or_insert(DEFAULT_COPY_TAGS_ON_COPY);
        }
```
`FastRestoreRule` / `ShareRule` appear only as stored struct members (`src/ops/types/shared_types.rs:51-53`) and in no check at all.

The entry points that accept the policy: `src/ops/create_lifecycle_policy.rs:312-318`
```
        if let Some(refusal) =
            crate::state::schedule_rule_violation(input.policy_details.as_ref())
        {
            return Err(
                CreateLifecyclePolicyErrorModeled::InvalidRequestException(refusal).into(),
            );
        }
```
and the same call on the update path, `src/ops/update_lifecycle_policy.rs:190-196`.

Test coverage: the two rows that pin `0` as acceptable carry NO `FastRestoreRule`, `ShareRule` or `CrossRegionCopyRules`, and their stated reason quotes only the ArchiveRule clause — `tests/all/policy_configuration_validation.rs:858-869`:
```
        RuleCase {
            // `StandardTierRetainRuleCount` is `@range(min: 0, max: 1000)`, and a
            // count of 0 is documented ("you can specify a retention count of 0
            // to archive snapshots immediately after creation").
            case: "count-based retention at the floor of its range",
            schedule: json!({
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 0},
            }),
            required: &[],
```
and `tests/all/policy_configuration_validation.rs:925-936` for `"RetainRule": {"Interval": 0, "IntervalUnit": "DAYS"}`. A repo-wide grep for `"Count": 0|"Interval": 0` under `tests/` returns only those two rows, so the combination the documentation refuses is pinned by nothing.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "zero retention with a cross region copy", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"Name": "copied", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 0}, "CrossRegionCopyRules": [{"Target": "us-west-2", "Encrypted": false}]}]}}
2. Observe HTTP 200 with a PolicyId. Expected: HTTP 400 InvalidRequestException, because RetainRule documents "If you specify a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify a retention count of 1 or more"
3. GET /policies/{PolicyId} — the policy is reported with Schedules[0].RetainRule.Count = 0 beside a CrossRegionCopyRules entry, so the refused combination is now persisted state a client can read back
4. Repeat step 1 with "RetainRule": {"Interval": 0, "IntervalUnit": "DAYS"} instead of the count — also HTTP 200, though the same paragraph requires "a retention interval of 1 day or more" when a CrossRegionCopyRule is present
5. Repeat step 1 with the CrossRegionCopyRules entry replaced by "ShareRule": [{"TargetAccounts": ["<other account>"], "UnshareInterval": 1, "UnshareIntervalUnit": "DAYS"}] (and again with a FastRestoreRule) — HTTP 200 each time, so none of the three rules the sentence names raises the floor
6. PATCH /policies/{PolicyId} with the step-1 PolicyDetails against a policy created with "RetainRule": {"Count": 7} — also HTTP 200, so the update path accepts the same refused configuration (it calls the identical crate::state::schedule_rule_violation helper)

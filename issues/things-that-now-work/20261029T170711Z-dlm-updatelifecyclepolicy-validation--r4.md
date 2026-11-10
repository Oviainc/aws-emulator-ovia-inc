Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3191709e0b386f11e7ab6b8d83438b183cb32e34

# The second bullet of CreateRule's note is never enforced: a schedule carrying an ArchiveRule is accepted with an interval-form creation frequency, which can never reach the documented 28 days

_Filed: 2026-10-29T17:07:11Z_
_Operation: UpdateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `3191709e0b386f11e7ab6b8d83438b183cb32e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateRule`'s shape documentation is a two-bullet note. The crate's cross-member rule engine enforces (and quotes) the FIRST bullet — "You must specify either CronExpression, or Interval, IntervalUnit, and Times" — and silently drops the SECOND — "If you need to specify an ArchiveRule for the schedule, then you must specify a creation frequency of at least 28 days."

That second bullet is not a soft bound here, it is decidable from the model alone: `CreateRule$IntervalUnit` targets `IntervalUnitValues`, whose only member is `HOURS`, and the crate's own enforced domain for `CreateRule$Interval` is `SUPPORTED_CREATE_RULE_INTERVALS = [1, 2, 3, 4, 6, 8, 12, 24]` (the set the member's documentation publishes). The largest creation frequency an interval-form `CreateRule` can state is therefore 24 HOURS — one day. Every schedule that states its creation rule in the interval form AND carries an `ArchiveRule` is consequently in violation of the documented sentence, and `UpdateLifecyclePolicy` (and `CreateLifecyclePolicy`, through the same helper) accepts it with HTTP 200 and stores it.

The gap is not that the crate never attempts ArchiveRule-conditional rules: `schedule_rule_violation` enumerates the documented cross-member sentences it judges and includes `ArchiveRule`'s own archive-tier retention sentence, and `retention_floor_violation`'s doc reasons explicitly about what the `ArchiveRule` sentence does and does not grant. `schedule.archive_rule` is read in exactly ONE place in the crate (`archive_tier_violation`, `src/state.rs:2163`) and only to reach the nested `RetentionArchiveTier`; it is never looked at beside the schedule's `CreateRule`, so the one documented pairing of those two rules is the one that goes unjudged. Nothing anywhere in the crate mentions 28 days, and no comment, test or doc states a reason for leaving the sentence out.

Both write paths share the helper, so the defect is reachable through `UpdateLifecyclePolicy::handle` (which calls `schedule_rule_violation` and `schedule_with_unsupported_create_interval` and nothing else about the schedule) and through `CreateLifecyclePolicy::handle`.

## Evidence

The model sentence that is never enforced — `smithy_query(query="doc", shape="CreateRule")` (HTML stripped):

> "[Custom snapshot and AMI policies only] Specifies when the policy should create snapshots or AMIs.
>  - You must specify either **CronExpression**, or **Interval**, **IntervalUnit**, and **Times**.
>  - **If you need to specify an ArchiveRule for the schedule, then you must specify a creation frequency of at least 28 days.**"

The only unit the interval form can use — `smithy_query(query="shape", shape="IntervalUnitValues")`: `{"shape_id": "com.amazonaws.dlm#IntervalUnitValues", "type": "enum", "values": ["HOURS"]}`.

The crate's own enforced domain for the interval, which caps it at 24 hours — `services/service-com-amazonaws-dlm-dlm-20180112/src/state.rs:205`:
```
pub const SUPPORTED_CREATE_RULE_INTERVALS: [i32; 8] = [1, 2, 3, 4, 6, 8, 12, 24];
```

The rule engine quotes the FIRST bullet of that two-bullet note and lists it as the whole of what it judges about `CreateRule` — `src/state.rs:1745-1748`:
```
/// * `CreateRule` — "You must specify either CronExpression, or Interval,
///   IntervalUnit, and Times." A schedule states when to create in exactly one
///   of the two forms: neither form, both forms, or an interval form missing a
///   member it names, are each refused.
```
and the handler that drives it enumerates the same closed list — `src/ops/update_lifecycle_policy.rs:181-196`:
```
        // The cross-member rules inside a schedule, which `UpdateLifecyclePolicy`
        // inherits from the shapes it carries: every nested rule the
        // documentation gives two alternative forms — the `CreateRule`, the
        // `RetainRule`, the `FastRestoreRule`, the `DeprecateRule` and the
        // `ArchiveRule`'s archive-tier retention — states exactly one of them
        // and states it completely, and the retention is at least as much as
        // the schedule's other rules require. ...
        if let Some(refusal) =
            crate::state::schedule_rule_violation(input.policy_details.as_ref())
        {
```

`create_rule_violation` — the only place a `CreateRule` is judged — looks at the two forms and nothing else; it has no access to the schedule at all, only to the rule (`src/state.rs:1817-1831`):
```
fn create_rule_violation(
    label: &str,
    path: &str,
    rule: Option<&CreateRule>,
) -> Option<InvalidRequestException> {
    let cron_based = rule.is_some_and(|rule| rule.cron_expression.is_some());
    let interval = rule.and_then(stated_create_interval);
```
and the second check the handler runs over the interval judges only membership of the supported set — `src/state.rs:1594-1607` (`schedule_with_unsupported_create_interval`), reached from `src/ops/update_lifecycle_policy.rs:204-213` and `src/ops/create_lifecycle_policy.rs:326-333`.

The ONE place the crate reads the schedule's `ArchiveRule`, which never looks at the creation rule — `src/state.rs:2157-2168`:
```
fn archive_tier_violation(
    label: &str,
    path: &str,
    schedule: &Schedule,
) -> Option<InvalidRequestException> {
    let tier: &RetentionArchiveTier = &schedule
        .archive_rule
        .as_ref()?
        .retain_rule
        .retention_archive_tier;
```
(`grep -n archive_rule src/` returns exactly `src/state.rs:2163` and the field declaration `src/ops/types/shared_types.rs:55`.)

Nothing in the crate mentions the requirement: `grep -n '28' services/service-com-amazonaws-dlm-dlm-20180112/{src,tests}` matches no comment, constant or assertion about a creation frequency (the only `28` hits in the crate are inside unrelated ARN patterns in `OPERATIONS.md`).

Test coverage: no test asserts anything about the creation frequency of a schedule carrying an `ArchiveRule`. The tests that DO send an `ArchiveRule` pair it with the 24-hour creation rule of `good_schedule` and assert 200 while pinning a different rule — `tests/all/schedule_rule_forms.rs:277-283`:
```
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 2},
    })
}
```
with the archive rows built at `tests/all/schedule_rule_forms.rs:805-807` (`|rule| json!({"ArchiveRule": {"RetainRule": {"RetentionArchiveTier": rule}}})`), and similar ArchiveRule rows at `tests/all/schedule_retention_floor.rs:328-333` and `tests/all/schedule_retention_required.rs:314-319` — each stating a reason for the retention rule it pins, none for the creation frequency.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "archive subject", "State": "ENABLED", "PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 2}}]}} — HTTP 200, note the PolicyId
2. PATCH /policies/{PolicyId} with {"PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 2}, "ArchiveRule": {"RetainRule": {"RetentionArchiveTier": {"Count": 10}}}}]}}
3. Observe HTTP 200: the schedule now carries an ArchiveRule beside a creation frequency of 24 HOURS — one day — although CreateRule's note states "If you need to specify an ArchiveRule for the schedule, then you must specify a creation frequency of at least 28 days"
4. GET /policies/{PolicyId} — Policy.PolicyDetails.Schedules[0] reports both the ArchiveRule and CreateRule {Interval: 24, IntervalUnit: "HOURS"}, so the configuration the documented sentence forbids is persisted and reported
5. Repeat step 2 with every value of SUPPORTED_CREATE_RULE_INTERVALS (1, 2, 3, 4, 6, 8, 12, 24) and with the only member of IntervalUnitValues (HOURS) — every one is accepted, so no interval-form creation rule paired with an ArchiveRule is ever refused; the same request sent to POST /policies (step 1 plus the ArchiveRule) is accepted too, through the same crate::state::schedule_rule_violation helper

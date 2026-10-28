Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# The CreateRule.Interval the service materializes for a cron schedule makes its own reported configuration unacceptable on update

_Filed: 2026-10-27T02:56:07Z_
_Operation: UpdateLifecyclePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`materialized_policy_details` stamps `CreateRule.Interval = 0` (`UNSET_RULE_INTERVAL`) onto EVERY stored schedule, including a cron-driven one, and a read reports that stamped `0` back to the client. The two schedule checks `UpdateLifecyclePolicy` (and `CreateLifecyclePolicy`) run on the way in do NOT know that `0` is the service's own filler:

1. `create_rule_violation` computes `interval_based = interval.is_some() || ...` with no exclusion for `UNSET_RULE_INTERVAL`, so a cron rule carrying the materialized `Interval: 0` counts as BOTH the cron form and the interval form and is refused with `InvalidRequestException` "Interval based CreateRule and Cron based CreateRule cannot both be present in schedule {..}".
2. `schedule_with_unsupported_create_interval` would likewise flag `0`, which is not in `SUPPORTED_CREATE_RULE_INTERVALS = [1, 2, 3, 4, 6, 8, 12, 24]`.

The consequence is that the ordinary read-modify-write flow is impossible for any custom snapshot or AMI policy whose schedule uses a `CronExpression`: `GET /policies/{PolicyId}` returns a `PolicyDetails`, and `PATCH /policies/{PolicyId}` with that very structure is refused 400 — the service rejects the configuration it just reported, and the refusal message accuses the caller of stating two forms when it only echoed one plus the service's own filler. The same request is equally refused by `CreateLifecyclePolicy` when a caller copies a reported cron configuration into a new policy.

The crate already knows about this hazard and handles it on the sibling rule: `retain_rule_violation` explicitly discounts a `RetainRule.Interval` equal to `UNSET_RULE_INTERVAL` "the value this service itself materializes for a rule that stated none". `create_rule_violation`, which sits five lines above it and is fed by the same materialization, does not. The round-trip invariant is stated as a rule in the crate's own test, whose rows cover both retention modes but only interval-based creation rules, so the cron case is unpinned and broken.

## Evidence

The write path stamps the filler onto a cron CreateRule — `src/state.rs:383-390`:
```
        if let Some(create_rule) = schedule.create_rule.as_mut() {
            if creates_snapshots {
                create_rule
                    .location
                    .get_or_insert_with(|| CLOUD_LOCATION.to_string());
            }
            create_rule.interval.get_or_insert(UNSET_RULE_INTERVAL);
        }
```
and the crate states that this is what a read reports — `src/state.rs:120-127`: "/// The value a schedule rule's interval carries when the request states none. /// /// `CreateRule$Interval` and `RetainRule$Interval` come back as `0` on a read of /// a policy whose schedule stated neither — a cron-driven creation rule and a /// count-based retention rule respectively — so the service materializes the /// integer zero for them rather than leaving them out."

The check that then refuses it — `src/state.rs:1499-1512`:
```
    let cron_based = rule.is_some_and(|rule| rule.cron_expression.is_some());
    let interval = rule.and_then(|rule| rule.interval);
    let interval_unit = rule.and_then(|rule| rule.interval_unit.as_deref());
    // Any one of the interval form's three members states the schedule as an
    // interval, whether or not the other two are there.
    let interval_based = interval.is_some()
        || interval_unit.is_some()
        || rule.is_some_and(|rule| rule.times.as_ref().is_some_and(|times| !times.is_empty()));

    match (cron_based, interval_based) {
        (true, true) => Some(both_forms_stated(
            label,
            "Interval based CreateRule and Cron based CreateRule",
```

The SIBLING rule in the same module, which does exclude the filler — `src/state.rs:1542-1553`:
```
/// A retention interval of [`UNSET_RULE_INTERVAL`] is the value this service
/// itself materializes for a rule that stated none, so it is not a time interval
/// the request asked for: a unit, or any other interval, is what states the
/// age-based mode.
fn retain_rule_violation(
...
    let age_based = rule.interval_unit.is_some()
        || rule.interval.is_some_and(|value| value != UNSET_RULE_INTERVAL);
```

The second refusal the same `0` earns — `src/state.rs:183`: `pub const SUPPORTED_CREATE_RULE_INTERVALS: [i32; 8] = [1, 2, 3, 4, 6, 8, 12, 24];` read against `src/state.rs:1329-1331`: `let interval = schedule.create_rule.as_ref()?.interval?; (!SUPPORTED_CREATE_RULE_INTERVALS.contains(&interval)).then(...)`.

Both checks are what the update runs before it touches the store — `src/ops/update_lifecycle_policy.rs:190-196` (`crate::state::schedule_rule_violation(input.policy_details.as_ref())`) and `src/ops/update_lifecycle_policy.rs:202-211` (`schedule_with_unsupported_create_interval`).

The invariant the crate says it holds, and the hole in its rows — `tests/all/policy_configuration_validation.rs:974-979`:
```
/// The configuration this service REPORTS for a policy is one it accepts back:
/// the members it materializes into a stored schedule never turn a rule into a
/// form the either/or checks refuse.
///
/// The rows span both retention modes, so the materialized retention interval of
/// a count-based rule is exercised beside an age-based rule that stated its own.
```
and its only creation form — `tests/all/policy_configuration_validation.rs:993-997`: `"CreateRule": {"Interval": 12, "IntervalUnit": CREATE_INTERVAL_UNIT}`. No row of `a_configuration_read_back_is_accepted_again` uses a `CronExpression`, and the cron rows of `a_create_rule_states_one_form` (`tests/all/policy_configuration_validation.rs:741-785`) all pair cron with a caller-stated `Interval: 12`, never with the materialized `0`.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "cron schedule", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"Name": "round-trip", "CreateRule": {"CronExpression": "cron(0 12 * * ? *)"}, "RetainRule": {"Count": 7}}]}} — observe HTTP 200 and note the PolicyId
2. GET /policies/{PolicyId} — Policy.PolicyDetails.Schedules[0].CreateRule reports {"CronExpression": "cron(0 12 * * ? *)", "Location": "CLOUD", "Interval": 0}: the service materialized the interval the request never stated
3. PATCH /policies/{PolicyId} with {"PolicyDetails": <exactly the structure step 2 reported>} — observe HTTP 400 InvalidRequestException with Message "Interval based CreateRule and Cron based CreateRule cannot both be present in schedule {round-trip}.", so the configuration the service itself reported cannot be sent back and the policy cannot be updated through a read-modify-write
4. Repeat steps 1-3 with "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"} instead of the cron expression — the same round trip answers 200 (this is the case tests/all/policy_configuration_validation.rs:981 pins), showing the refusal is specific to the materialized CreateRule.Interval of a cron rule
5. POST /policies with the step-2 PolicyDetails as a brand-new policy's configuration — refused identically, so a caller cannot clone a reported cron configuration either

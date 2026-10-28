Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: e94a5165da5e1a7dc4e65daf2da688ae995eb229

# A nested parameter inside Schedules is named two ways in one operation's InvalidRequestException: Schedules.1.CreateRule from the boundary walk, Schedules[1].CreateRule from the handler

_Filed: 2026-10-28T21:17:54Z_
_Operation: UpdateLifecyclePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `e94a5165da5e1a7dc4e65daf2da688ae995eb229`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal this service answers for `CreateLifecyclePolicy` / `UpdateLifecyclePolicy` is an `InvalidRequestException` whose `Message` and `RequiredParameters` name the request parameters at fault. Two different code paths in THIS crate build those names, and they spell a list element incompatibly:

* The pre-handler envelope renders the framework's constraint-walk path through `crate::service::parameter_name`, which drops only the `member` collection step and upper-cases each remaining segment. The index therefore stays a DOTTED segment: `policyDetails.schedules.1.member.createRule.cronExpression` becomes `PolicyDetails.Schedules.1.CreateRule.CronExpression`.
* The handler-side cross-member rules in `crate::state` build their own path with `schedule_path`, which BRACKETS the index: `PolicyDetails.Schedules[1].CreateRule.Interval`.

Both are 1-based, both describe the same member of the same list of the same request, and both land in the same `InvalidRequestException` members of the same operation — so a client that parses `RequiredParameters` (or the message clause) to point a user at the field it must correct has to understand two conventions for one parameter path, and cannot match a boundary refusal's name against a handler refusal's name for the same schedule.

The inconsistency is not an oversight about which convention the framework uses: `service.rs` documents that convention verbatim (`schedules.1.member.createRule`), and `schedule_label` in `state.rs` justifies its own numbering by citing the very same convention ("1-based, as a validation path numbers a list element") while rendering it with brackets instead of the dot its own crate drops the `member` step from. One of the two spellings must be wrong; the crate asserts both are "the validation path".

`CreateLifecyclePolicy` exhibits the same split (its handler calls the same `schedule_rule_violation` / `schedule_path` helpers and its requests go through the same `InModelledEnvelope`); it is reported once here.

## Evidence

The crate documents the framework path form, dotted index included — `services/service-com-amazonaws-dlm-dlm-20180112/src/service.rs:106-109`:
```
/// The path segment a validation path puts between a collection and its element
/// (`schedules.1.member.createRule`). It names no parameter of the request, so a
/// parameter name drops it.
const COLLECTION_PATH_STEP: &str = "member";
```
and renders a refused parameter from it by dropping ONLY that step — `src/service.rs:555-567`:
```
/// The parameter a validation path names, as the model spells it.
///
/// A validation path is lowerCamel (`description`, `policyDetails.schedules.1.member.name`)
/// because that is the casing AWS validation paths use; a REQUEST parameter is
/// named as the model declares the member, so each segment's first character is
/// restored and the collection step is dropped.
fn parameter_name(path: &str) -> String {
    path.split('.')
        .filter(|segment| *segment != COLLECTION_PATH_STEP)
        .map(upper_camel)
        .collect::<Vec<String>>()
        .join(".")
}
```
Those names are what the 400 carries, both in the message clauses and in `RequiredParameters` — `src/service.rs:510-534` (`.map(|violation| parameter_name(&violation.path))` for the omitted set, `.map(stated)` for the invalid set, then `crate::state::required_parameters_omitted(message, omitted)`).

The OTHER spelling, built by the handler-side rules — `src/state.rs:1540-1544`:
```
/// The wire path a refusal names the schedule at `position` by, so a caller can
/// tell which member of which list element to correct.
fn schedule_path(position: usize) -> String {
    format!("{POLICY_DETAILS_PREFIX}Schedules[{}]", position + 1)
}
```
whose sibling claims the framework convention as its authority — `src/state.rs:1532-1537`:
```
fn schedule_label(position: usize, schedule: &Schedule) -> String {
    match schedule.name.as_deref() {
        Some(name) if !name.is_empty() => name.to_string(),
        // 1-based, as a validation path numbers a list element.
        _ => format!("Schedules[{}]", position + 1),
    }
}
```
The bracketed path is what every handler-built refusal reports — `src/state.rs:1943-1949` (`omitted_members` → `format!("{path}.{name}")`) and `src/state.rs:1955-1966` (`required_in_schedule` → `required_parameters_omitted(message, parameters)`), reached from `src/ops/update_lifecycle_policy.rs:191-197` (`crate::state::schedule_rule_violation(input.policy_details.as_ref())`) and `src/ops/create_lifecycle_policy.rs:314-320`.

The framework's convention, for the record (citation only) — `vendor/smithy-schema-server/src/constraints.rs:31-38`:
```
//! # Path fidelity
//!
//! The walk maintains a path stack producing the dotted path style AWS uses in
//! `ValidationException` messages. Member names are lowerCamel; list elements
//! contribute a `<1-based-index>.member` segment (`keySchema.1.member.keyType`);
```

Test coverage: the BRACKETED form is pinned for handler refusals — `tests/all/policy_configuration_validation.rs:595` (`.map(|member| format!("PolicyDetails.Schedules[{position}].{member}"))`), `tests/all/schedule_retention_required.rs:437` (`let omitted = format!("PolicyDetails.Schedules[{position}].RetainRule");`) and `tests/all/materialized_create_interval_round_trip.rs:552` (`owed: Owed::Required("PolicyDetails.Schedules[1].CreateRule.Interval")`). No test anywhere under `tests/` asserts the name a BOUNDARY refusal of a member nested inside `Schedules` reports, so the dotted side is unpinned and the two conventions were never compared.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with a valid custom snapshot policy — {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "path spelling subject", "State": "ENABLED", "PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "Daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 1}}]}} — note the PolicyId
2. PATCH /policies/{PolicyId} with a single schedule whose CreateRule states the interval form but omits Interval — {"PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"CreateRule": {"IntervalUnit": "HOURS"}, "RetainRule": {"Count": 1}}]}} — observe 400 InvalidRequestException whose RequiredParameters is ["PolicyDetails.Schedules[1].CreateRule.Interval"], the bracketed spelling built by src/state.rs:1542
3. PATCH /policies/{PolicyId} with the SAME single schedule, this time carrying a CronExpression outside its modelled @pattern (^cron\([^\n]{11,100}\)$) — {"PolicyDetails": {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT", "ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"CreateRule": {"CronExpression": "not-a-cron-expression"}, "RetainRule": {"Count": 1}}]}} — observe 400 InvalidRequestException whose Message reads "The following parameter(s) are invalid: PolicyDetails.Schedules.1.CreateRule.CronExpression {not-a-cron-expression}", the DOTTED spelling produced by src/service.rs:561 from the walk path policyDetails.schedules.1.member.createRule.cronExpression
4. Compare the two names for the same first element of the same Schedules list: Schedules[1] from the handler, Schedules.1 from the boundary walk, both in InvalidRequestException on the same operation; repeat steps 2-3 against POST /policies for the identical split

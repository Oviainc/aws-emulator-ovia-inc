Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# The alarm-template validation rule set is written out twice (create + update), and the length-bound rule `crate::state::bounded_length` already owns is hand-rolled in nine handlers

_Filed: 2026-10-27T07:03:52Z_
_Operation: UpdateCloudWatchAlarmTemplate | Class: maintainability_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two copies of one rule set that can drift, both inside this crate:

1. `crate::state::bounded_length` exists precisely so a create and the update beside it cannot word one length constraint two ways, and the EventBridge rule-template pair already goes through it (that consolidation is what the fixed issue 20261027T043247Z-medialive-updateeventbridgeruletemplate-maintainability produced). Nine other handlers still carry a byte-for-byte identical private `fn bounded` — same `chars().count()` comparison, same `"{member} must be between {min} and {max} characters long, but was {len}"` message — differing only in which operation's error enum they wrap: `update_cloud_watch_alarm_template.rs:390`, `create_cloud_watch_alarm_template.rs:402`, `update_cloud_watch_alarm_template_group.rs:210`, `create_event_bridge_rule_template_group.rs:287`, `update_event_bridge_rule_template_group.rs:216`, `create_cluster.rs:320`, `update_cluster.rs:236`, `purchase_offering.rs:293`, `update_reservation.rs:217`. `at_least_one` is duplicated the same way in `purchase_offering.rs:314` and `update_reservation.rs:238`.

2. The CloudWatch-alarm-template create and update validate the SAME members of the SAME resource with two copies of the same three rules — the four-enum closed-set walk, the `in_range` numeric bounds for `period`/`evaluationPeriods`/`datapointsToAlarm`, and the finite-`threshold` check — differing only in `as_str()` vs `as_deref()`/`Some(...)`. The alarm-template family is the one monitor resource whose shared rule was never moved into its state slice, the way the EventBridge one was.

A fix folds each copy onto one shared rule (the pattern `state::bounded_length` and `state/event_bridge_rule_templates.rs:141` already establish). Every response message and status stays character-identical, so no existing test changes.

## Evidence

The shared rule and its stated purpose, `services/service-com-amazonaws-medialive-medialive/src/state.rs:944-968`:

    /// ONE copy of that length rule and of its wording, so the operations that
    /// check the same member of the same resource against the same bound — a
    /// create and the update beside it — cannot drift into two readings of one
    /// constraint. ...
    pub fn bounded_length(member: &str, value: Option<&str>, min: usize, max: usize) -> Result<(), String> {
        ... let len = value.chars().count(); if len >= min && len <= max { return Ok(()); }
        Err(format!("{member} must be between {min} and {max} characters long, but was {len}"))

The two handlers that use it — `src/ops/create_event_bridge_rule_template.rs:337` and `src/ops/update_event_bridge_rule_template.rs:308`:

    crate::state::bounded_length(member, value, min, max).map_err(bad_request)

versus the hand-rolled copies, e.g. `src/ops/create_cluster.rs:320-340`:

    fn bounded(member: &str, value: Option<&str>, min: usize, max: usize) -> Result<(), CreateClusterError> {
        let Some(value) = value else { return Ok(()); };
        let len = value.chars().count();
        if len >= min && len <= max { return Ok(()); }
        Err(CreateClusterErrorModeled::BadRequestException(BadRequestException {
            message: Some(format!("{member} must be between {min} and {max} characters long, but was {len}")),

and `src/ops/update_cluster.rs:236-256`, identical but for `UpdateClusterError`/`UpdateClusterErrorModeled`.

The second copy pair: `src/ops/create_cloud_watch_alarm_template.rs:239-298` (the `for (member, values, value) in [ ("comparisonOperator", CLOUD_WATCH_ALARM_TEMPLATE_COMPARISON_OPERATOR_VALUES, ...), ("statistic", ...), ("targetResourceType", ...), ("treatMissingData", ...) ]` walk, the three `in_range(...)` calls, `if !input.threshold.is_finite()`) against `src/ops/update_cloud_watch_alarm_template.rs:243-306` — the same four rows, the same three `in_range` calls, the same finite check, with the comment at 238-242 admitting it is "the same rule the create applies, because these are the same members of the same resource". `in_range` itself is duplicated at `create_cloud_watch_alarm_template.rs:422` and `update_cloud_watch_alarm_template.rs` beside it.

Tests the fix must leave green and unchanged: `tests/all/cluster_creation.rs:107-129` and `tests/all/cluster_update.rs:467-514` (name bound at/one past), `tests/all/reservation_purchase.rs:954` (`name`/`requestId` bounds), `tests/all/reservation_update.rs:204-213,386`, `tests/all/cloud_watch_alarm_templates.rs` and `tests/all/cloud_watch_alarm_template_update.rs` (the enum/range/threshold refusals), `tests/all/event_bridge_rule_template_targets.rs:93` (the already-shared wording).

## Steps to Reproduce

1. grep -n 'fn bounded(' services/service-com-amazonaws-medialive-medialive/src/ops/*.rs — nine identical private copies, while src/state.rs:952 already exports `bounded_length` and two handlers call it.
2. Diff src/ops/create_cloud_watch_alarm_template.rs:239-298 against src/ops/update_cloud_watch_alarm_template.rs:243-306: the four-enum catalog walk, the three in_range calls and the finite-threshold check are the same rule written twice.
3. Change one copy's bound or wording (e.g. the metricName bound) and the sibling operation keeps the old reading — the drift the state.rs:944-948 comment says the shared helper exists to prevent.

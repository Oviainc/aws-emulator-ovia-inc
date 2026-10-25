Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# The RecurrenceSettings validity rule is implemented twice (CreateRotation and UpdateRotation)

_Filed: 2026-10-25T00:38:02Z_
_Operation: CreateRotation | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same recurrence rule — check every `WeeklySettings` day and every `ShiftCoverages` key against `DAY_OF_WEEK_VALUES`, then require at least one non-empty settings list, refusing with one identical message — is written out twice, once in `create_rotation.rs` and once in `update_rotation.rs`, including a verbatim copy of the private helper `carries_a_setting`. The crate already centralises exactly this kind of cross-operation rule in `state.rs` (its module doc says it holds "the helpers more than one operation needs — ARN minting, the page/token codec, the shared enum check and the output-shape rebuilders — so the handlers ... cannot drift apart", and `check_enum` lives there for the same reason), so the two copies can drift: a change to the permitted recurrence forms or to the refusal message has to be made in both places. Hoisting the block into `state.rs` beside `check_enum` keeps every response byte-identical.

## Evidence

Copy 1 — src/ops/create_rotation.rs:223-246 and 336-338:
```
        for setting in input.recurrence.weekly_settings.iter().flatten() {
            check_enum("recurrence.weeklySettings.dayOfWeek", &setting.day_of_week, DAY_OF_WEEK_VALUES).map_err(validation)?;
        }
        ... let describes_a_schedule = carries_a_setting(&input.recurrence.daily_settings)
            || carries_a_setting(&input.recurrence.weekly_settings)
            || carries_a_setting(&input.recurrence.monthly_settings);
        if !describes_a_schedule { return Err(validation("Value at 'recurrence' failed to satisfy constraint: Member must carry one of dailySettings, weeklySettings or monthlySettings")); }
...
fn carries_a_setting<T>(settings: &Option<Vec<T>>) -> bool {
    settings.as_ref().is_some_and(|settings| !settings.is_empty())
}
```
Copy 2 — src/ops/update_rotation.rs:186-212 and 275-277: the same two loops with the same member names, the same `describes_a_schedule` disjunction, the same message string, and the same `fn carries_a_setting<T>(settings: &Option<Vec<T>>) -> bool { settings.as_ref().is_some_and(|settings| !settings.is_empty()) }`. update_rotation.rs:186-187 even says so: "`DayOfWeek` is an enum SHAPE, so it arrives as a plain string — the same two / places `CreateRotation` checks it in a `RecurrenceSettings`."

Where such shared rules already live — src/state.rs:21-23: "Besides the state itself this module holds the helpers more than one operation needs — ARN minting, the page/token codec, the shared enum check and the output-shape rebuilders — so the handlers of the contact/channel lifecycle cannot drift apart."

Existing tests that pin both copies' responses: tests/all/data_plane_precondition.rs:1000-1055 ("a recurrence with none of the three settings lists", "a recurrence whose settings lists are all empty", "a weekly setting naming a day outside DayOfWeek", "a shift coverage keyed by a day outside DayOfWeek" for CreateRotation) and tests/all/rotation_updates.rs:753-830 (`a_recurrence_that_describes_no_shift_is_refused_and_writes_nothing`, with the same four rows for UpdateRotation).

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_rotation.rs:223-246 and src/ops/update_rotation.rs:186-212 side by side — the two validation blocks are the same logic with the same refusal message.
2. Compare create_rotation.rs:336-338 with update_rotation.rs:275-277 — `carries_a_setting` is duplicated verbatim.
3. Change the rule or the message in one file only: tests/all/data_plane_precondition.rs and tests/all/rotation_updates.rs then disagree about the same recurrence, which is the drift the duplication allows.

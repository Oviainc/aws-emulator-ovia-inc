Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# A rotation created without StartTime has no effective start time, so GetRotation and ListRotations disagree

_Filed: 2026-10-24T19:49:18Z_
_Operation: CreateRotation | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateRotation stores `start_time: input.start_time` verbatim (src/ops/create_rotation.rs:297-307), leaving it `None` when the optional member is omitted. The two readers then answer differently for the same rotation: GetRotation substitutes the record's creation instant because the modeled `GetRotationOutput.StartTime` is `@required` (src/ops/get_rotation.rs:216-218), while `rotation_summary` used by ListRotations passes the stored `Option` straight through (src/state.rs:935-944), reporting no `StartTime` at all.

Expected: the service assigns an effective start time at creation (which is why `GetRotation.StartTime` is required), and both GetRotation and ListRotations report that same value for the same rotation. Actual: GetRotation reports an instant, ListRotations reports nothing, for one unchanged rotation — the value is also not persisted, so it is derived on each read path independently.

## Evidence

Model: `CreateRotation.StartTime` is optional, `GetRotationOutput.StartTime` is `@required`, and `Rotation.StartTime` (the ListRotations summary member) is optional — a required output member means the service always holds a start time for every rotation. Handler: src/ops/create_rotation.rs:301 (`start_time: input.start_time`), src/ops/get_rotation.rs:210-218 (synthesizes `created_at`), src/state.rs:940 (`start_time: record.start_time`). Test coverage: tests/all/data_plane_precondition.rs:876-880 asserts ListRotations reports `null` for the row created without StartTime, i.e. the current divergence is pinned by a test rather than by the model — the same rotation read through GetRotation reports a time, which no test cross-checks.

## Steps to Reproduce

1. Onboard the account and create a PERSONAL contact.
2. CreateRotation with Name/ContactIds/TimeZoneId/Recurrence but NO StartTime.
3. GetRotation for the returned RotationArn: StartTime is present (the creation instant).
4. ListRotations with RotationNamePrefix for the same rotation: the summary carries no StartTime.
5. Compare the two: one rotation reports two different start times (one absent) where AWS reports the single start time it assigned.

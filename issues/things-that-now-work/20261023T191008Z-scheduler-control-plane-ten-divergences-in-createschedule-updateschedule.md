Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.create-schedule-invalid-cron-expression scheduler.create-schedule-malformed-rate-expression-rejected scheduler.create-schedule-rate-expression-zero-value-rejected scheduler.create-schedule-invalid-timezone-rejected scheduler.create-schedule-start-after-end-date scheduler.schedule-end-date-before-start-date-rejected scheduler.events-scheduler-semantics-create-with-a-past-end-date-and-no-start-date-rejected scheduler.schedule-start-end-date-timestamp-precision-preserved scheduler.schedule-start-end-date-timestamp-precision-round-trip scheduler.schedule-timestamp-precision-round-trip scheduler.create-schedule-in-deleting-group-blocked scheduler.update-schedule-target-role-deleted-succeeds scheduler.create-get-delete-schedule-smoke scheduler.delete-schedule-not-found scheduler.concurrent-delete-schedule-race-one-wins-one-404 scheduler.delete-schedule-group-cascade scheduler.schedule-group-lifecycle-cascade-delete scheduler.create-schedule-client-token-reuse-conflict-rejected scheduler.create-schedule-missing-target-rejected scheduler.create-schedule-flexible-window-exceeds-max-rejected scheduler.create-schedule-flexible-window-max-out-of-range-rejected scheduler.create-schedule-flexible-window-out-of-range-rejected scheduler.flexible-time-window-max-minutes-out-of-range-rejected scheduler.flexible-window-minutes-out-of-range-rejected scheduler.schedule-flexible-time-window-out-of-range-rejected scheduler.schedule-nonexistent-target-role-rejected scheduler.update-schedule-change-group-forbidden scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion scheduler.update-schedule-on-deleted-group-blocked scheduler.create-schedule-action-after-completion-with-recurring-expression-rejected scheduler.schedule-description-exceeds-max-length-rejected
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.create-schedule-invalid-cron-expression|_error#9|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.create-schedule-malformed-rate-expression-rejected|_error#12|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.create-schedule-rate-expression-zero-value-rejected|_error#8|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.create-schedule-invalid-timezone-rejected|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.create-schedule-start-after-end-date|_error#7|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.schedule-end-date-before-start-date-rejected|_error#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: scheduler.events-scheduler-semantics-create-with-a-past-end-date-and-no-start-date-rejected|run#11|AssertionError|assert code == "ValidationException", (
Issue-Key: scheduler.schedule-start-end-date-timestamp-precision-preserved|run#15|AssertionError|assert got_start.microsecond % 1000 == 0, f"expected StartDate rounded to millisecond precision, got {got_start!r}"
Issue-Key: scheduler.schedule-start-end-date-timestamp-precision-round-trip|run#13|AssertionError|assert fetched["StartDate"] == expected_start, (
Issue-Key: scheduler.schedule-timestamp-precision-round-trip|run#14|AssertionError|assert got_start == expected_start, (
Issue-Key: scheduler.create-schedule-in-deleting-group-blocked|run#23|AssertionError|assert rejection_code == "ValidationException", f"expected ValidationException, got {rejection_code!r}"
Issue-Key: scheduler|UpdateSchedule|ValidationException
Issue-Key: scheduler.create-get-delete-schedule-smoke|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: scheduler.delete-schedule-not-found|run#15|AssertionError|assert gone == ("ResourceNotFoundException", expected), (
Issue-Key: scheduler.concurrent-delete-schedule-race-one-wins-one-404|run#14|AssertionError|assert errors[0][2] == expected_message, (
Issue-Key: scheduler.delete-schedule-group-cascade|run#15|AssertionError|assert member_error == ("ResourceNotFoundException", gone_message), (
Issue-Key: scheduler|GetSchedule|ResourceNotFoundException
Issue-Key: scheduler.create-schedule-client-token-reuse-conflict-rejected|run#25|AssertionError|assert deleted_message == f"Schedule {schedule_name} does not exist.", (
Issue-Key: scheduler.create-schedule-missing-target-rejected|run#19|AssertionError|assert got_message == expected_missing, f"expected message {expected_missing!r}, got {got_message!r}"
Issue-Key: scheduler.create-schedule-flexible-window-exceeds-max-rejected|run#13|AssertionError|assert get_message == f"Schedule {bad_name} does not exist.", (
Issue-Key: scheduler.create-schedule-flexible-window-out-of-range-rejected|run#12|AssertionError|assert not_found_message == f"Schedule {bad_name} does not exist.", (
Issue-Key: scheduler.flexible-time-window-max-minutes-out-of-range-rejected|run#16|AssertionError|assert not_found_message == f"Schedule {bad_name} does not exist.", (
Issue-Key: scheduler.flexible-window-minutes-out-of-range-rejected|run#11|AssertionError|assert not_found_message == f"Schedule {rejected_name} does not exist.", (
Issue-Key: scheduler.schedule-flexible-time-window-out-of-range-rejected|run#12|AssertionError|assert not_found_message == f"Schedule {rejected_name} does not exist.", (
Issue-Key: scheduler.schedule-nonexistent-target-role-rejected|run#14|AssertionError|assert not_found_message == f"Schedule {bad_name} does not exist.", (
Issue-Key: scheduler.update-schedule-change-group-forbidden|run#20|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion|run#17|AssertionError|assert message == f"Schedule {name} does not exist.", (
Issue-Key: scheduler.update-schedule-on-deleted-group-blocked|run#15|AssertionError|assert err.get("Message") == expected_update_message, (
Issue-Key: scheduler.create-schedule-action-after-completion-with-recurring-expression-rejected|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: scheduler.schedule-description-exceeds-max-length-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# Scheduler control plane: ten divergences in CreateSchedule/UpdateSchedule validation, timestamp precision, error codes and not-found reporting

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Ten distinct root causes in the scheduler control plane. Each must be closed before this issue is submitted.

Cause 1 of 10: ScheduleExpression is not validated at all.
AWS: ScheduleExpression (required, len 1..256) must be one of `at(yyyy-mm-ddThh:mm:ss)`, `rate(value unit)` with value >= 1 and unit in minute(s)|hour(s)|day(s), or `cron(fields)` with six valid fields (Smithy, scheduler model, CreateSchedule$ScheduleExpression doc). A malformed cron, a malformed rate, or `rate(0 minutes)` is refused with HTTP 400 ValidationException.
Emulator: CreateSchedule returns HTTP 200 with a ScheduleArn for `cron(...)` garbage, for a malformed `rate(...)` and for a zero-valued rate — the handler never inspects the expression string.
Why: a client that typos a cron/rate expression gets a schedule that can never run and no error, instead of the 400 that would surface the typo at deploy time; IaC drift and 'why did my job never fire' debugging follow.
Fix: validation in the CreateSchedule handler (and UpdateSchedule), services/service-com-amazonaws-scheduler-awschronosservice/src/ops/create_schedule.rs (handle() currently goes straight from flexible-window/role checks to `store.schedules.insert`), with the parser in src/state.rs.
Traces: scheduler.create-schedule-invalid-cron-expression, scheduler.create-schedule-malformed-rate-expression-rejected, scheduler.create-schedule-rate-expression-zero-value-rejected.

Cause 2 of 10: ScheduleExpressionTimezone is not validated.
AWS: ScheduleExpressionTimezone (len 1..50) must name an IANA zone; a bogus zone is refused with HTTP 400 ValidationException (recorded behavior; Smithy states only the length bound and 'The timezone in which the scheduling expression is evaluated').
Emulator: CreateSchedule returns HTTP 200 and persists the bogus zone.
Why: the schedule's firing instants are undefined — the client believes it configured a zone the service never accepted.
Fix: validation in src/ops/create_schedule.rs / update_schedule.rs.
Traces: scheduler.create-schedule-invalid-timezone-rejected.

Cause 3 of 10: StartDate/EndDate consistency is not validated.
AWS: a CreateSchedule whose StartDate is after its EndDate, or whose EndDate is already in the past with no StartDate, is refused with HTTP 400 ValidationException (recorded behavior; Smithy documents EndDate as 'The date, in UTC, before which the schedule can invoke its target').
Emulator: both are accepted with HTTP 200 and a ScheduleArn.
Why: the caller is handed a schedule whose window is empty — it can never fire — and no error says so.
Fix: validation in src/ops/create_schedule.rs / update_schedule.rs.
Traces: scheduler.create-schedule-start-after-end-date, scheduler.schedule-end-date-before-start-date-rejected, scheduler.events-scheduler-semantics-create-with-a-past-end-date-and-no-start-date-rejected.

Cause 4 of 10: StartDate/EndDate are stored and returned at sub-millisecond precision.
AWS: Scheduler truncates these timestamps to millisecond precision, so GetSchedule returns ...123000 for a submitted ...123456 (recorded behavior across three independent checks; Smithy types them as plain timestamps).
Emulator: GetSchedule echoes the full microsecond value (e.g. 2026-10-24T18:43:56.123456+00:00 for a submitted .123456).
Why: a client that round-trips its own timestamps and compares them to what it sent sees a value AWS would never return, and any logic keyed on the service-normalised instant diverges.
Fix: normalise on write in src/ops/create_schedule.rs / update_schedule.rs (or in the `ScheduleRecord` start_date/end_date fields in src/state.rs).
Traces: scheduler.schedule-start-end-date-timestamp-precision-preserved, scheduler.schedule-start-end-date-timestamp-precision-round-trip, scheduler.schedule-timestamp-precision-round-trip.

Cause 5 of 10: CreateSchedule into a DELETING group answers the wrong error code.
AWS: creating a schedule in a schedule group that is in state DELETING is refused with HTTP 400 ValidationException (recorded behavior).
Emulator: answers ConflictException (HTTP 409) — src/ops/create_schedule.rs returns `ConflictException { message: "Schedule group {group} is being deleted." }` from its `store.group_is_deleting` branch.
Why: the status class differs (409 vs 400) and SDK retry/error-handling branches on the modelled error type, so a client that treats ConflictException as retryable will retry a request AWS calls permanently invalid.
Fix: the group_is_deleting branch of src/ops/create_schedule.rs.
Traces: scheduler.create-schedule-in-deleting-group-blocked.

Cause 6 of 10: UpdateSchedule eagerly re-validates the target execution role.
AWS: role assumability is resolved when the schedule is invoked, not on every control-plane write; once a schedule exists, deleting its target role out of band and then calling UpdateSchedule (even re-affirming the now dangling RoleArn) SUCCEEDS and the update applies, dangling reference and all (recorded behavior, documented in the check as verified against real AWS).
Emulator: UpdateSchedule answers HTTP 400 ValidationException, so the update is lost.
Why: a client reconciling a schedule (enabling it, editing its description) fails hard whenever the referenced role is temporarily absent or being replaced — an ordering constraint AWS does not impose.
Fix: the execution-role check (`crate::state::execution_role_violation`, src/state.rs) must not gate UpdateSchedule in src/ops/update_schedule.rs.
Traces: scheduler.update-schedule-target-role-deleted-succeeds.

Cause 7 of 10: the schedule-not-found message carries an extra schedule-group clause.
AWS: GetSchedule/DeleteSchedule/UpdateSchedule for a missing schedule answer ResourceNotFoundException (HTTP 404) with a message built from the request naming only the schedule: `Schedule <Name> does not exist.` (recorded identically by 16 independent checks).
Emulator: appends the group — `Schedule <Name> does not exist in schedule group <Group>.` The status and error code match; only the request-derived message form diverges.
Fix: the not-found message helper in services/service-com-amazonaws-scheduler-awschronosservice/src/state.rs (sibling of `group_does_not_exist`) used by src/ops/get_schedule.rs, delete_schedule.rs and update_schedule.rs.
Why: callers and tooling that match on the documented not-found text (and every check listed here) read the emulator's message as a different condition.
Traces: scheduler.create-get-delete-schedule-smoke, scheduler.delete-schedule-not-found, scheduler.concurrent-delete-schedule-race-one-wins-one-404, scheduler.delete-schedule-group-cascade, scheduler.schedule-group-lifecycle-cascade-delete, scheduler.create-schedule-client-token-reuse-conflict-rejected, scheduler.create-schedule-missing-target-rejected, scheduler.create-schedule-flexible-window-exceeds-max-rejected, scheduler.create-schedule-flexible-window-max-out-of-range-rejected, scheduler.create-schedule-flexible-window-out-of-range-rejected, scheduler.flexible-time-window-max-minutes-out-of-range-rejected, scheduler.flexible-window-minutes-out-of-range-rejected, scheduler.schedule-flexible-time-window-out-of-range-rejected, scheduler.schedule-nonexistent-target-role-rejected, scheduler.update-schedule-change-group-forbidden, scheduler.events-scheduler-semantics-same-schedule-name-in-two-groups-is-isolated-and-survives-the-sibling-group-deletion.

Cause 8 of 10: UpdateSchedule against a non-existent schedule group reports the wrong missing resource.
AWS: when the named GroupName does not exist, UpdateSchedule's ResourceNotFoundException (HTTP 404) names the GROUP: `Schedule group <Group> does not exist.` (recorded behavior).
Emulator: names the schedule instead — `Schedule <Name> does not exist in schedule group <Group>.` — asserting the group exists and the schedule does not.
Why: the client cannot tell a deleted group from a deleted schedule, so reconciliation recreates the wrong resource (a schedule into a group that is gone).
Fix: the group-existence branch must precede the schedule lookup in src/ops/update_schedule.rs, using `crate::state::group_does_not_exist` (src/state.rs).
Traces: scheduler.update-schedule-on-deleted-group-blocked.

Cause 9 of 10: the enum value set in a ValidationException message is emitted in a different order than the model declares.
AWS: an out-of-range ActionAfterCompletion is refused with `1 validation error detected: Value 'ARCHIVE' at 'actionAfterCompletion' failed to satisfy constraint: Member must satisfy enum value set: [DELETE, NONE]` — the set is listed in the model's declaration order. Smithy (scheduler model, CreateSchedule$ActionAfterCompletion) declares `ActionAfterCompletion, one of: DELETE | NONE`.
Emulator: emits `[NONE, DELETE]`.
Why: the message is machine-read by the constraint-validation surface and reproduced in docs/tests; the emulator contradicts its own model's declared order.
Fix: enum-set rendering in the shared constraint-validation serializer used by the scheduler ops (src/ops/types.rs / the boundary validator that produces `1 validation error detected: ...`).
Traces: scheduler.create-schedule-action-after-completion-with-recurring-expression-rejected.

Cause 10 of 10: a length-constraint ValidationException elides the offending value instead of echoing it.
AWS: the message is built from the request and quotes the submitted value in full: `1 validation error detected: Value '<the submitted string>' at 'description' failed to satisfy constraint: Member must have length less than or equal to 512`. Smithy: CreateSchedule$Description is `Description, len 0..512`.
Emulator: replaces the tail of the value with an elision marker — `Value 'ddd…ddd... [513 bytes]' at 'description' ...` — so the request-derived part of the message is not what AWS produces.
Why: clients and operators diff the echoed value to find which field/record tripped the bound; the elided form loses that.
Fix: the constraint-violation message builder in the scheduler crate's boundary validation (same component as Cause 9).
Traces: scheduler.schedule-description-exceeds-max-length-rejected.

## Evidence
Cause 1 — expected HTTP 400 ValidationException; actual HTTP 200 {'ScheduleArn': 'arn:aws:scheduler:us-west-2:000000000000:schedule/default/sim2real-sched-badcron-...'} ('expected a refusal, but the call SUCCEEDED').
Cause 2 — expected ValidationException; actual HTTP 200 with a ScheduleArn.
Cause 3 — expected ValidationException; actual '<accepted>' / HTTP 200 with a ScheduleArn for StartDate>EndDate and for an already-past EndDate.
Cause 4 — expected StartDate 2026-10-24T18:43:56.123000+00:00; actual 2026-10-24T18:43:56.123456+00:00.
Cause 5 — expected ValidationException; actual 'ConflictException'.
Cause 6 — expected UpdateSchedule to succeed; actual 'ValidationException'.
Cause 7 — expected message 'Schedule sim2real-behav-sched-897626271b54 does not exist.'; actual 'Schedule sim2real-behav-sched-897626271b54 does not exist in schedule group default.' (code ResourceNotFoundException matches in all 16 checks).
Cause 8 — expected UpdateSchedule message 'Schedule group sim2real-sg-del-5180d50ba250 does not exist.'; actual 'Schedule sim2real-sched-ingroup-99e59c906d9e does not exist in schedule group sim2real-sg-del-5180d50ba250.'
Cause 9 — expected '... enum value set: [DELETE, NONE]'; actual '... enum value set: [NONE, DELETE]'.
Cause 10 — expected the full 513-byte value echoed; actual "Value 'ddd...[truncated]... [513 bytes]' at 'description' failed to satisfy constraint: Member must have length less than or equal to 512".

## Steps to Reproduce
Common setup: iam.create_role trusting scheduler.amazonaws.com with an sqs:SendMessage inline policy; sqs.create_queue for the target ARN.
1 (Cause 1): scheduler.create_schedule(Name=n, ScheduleExpression='cron(not a cron)', FlexibleTimeWindow={'Mode':'OFF'}, Target=t) -> 200 instead of 400 ValidationException. Repeat with 'rate(bogus)' and 'rate(0 minutes)'.
2 (Cause 2): same call with ScheduleExpression='rate(5 minutes)', ScheduleExpressionTimezone='Not/AZone' -> 200.
3 (Cause 3): same call with StartDate=now+2d, EndDate=now+1d -> 200; and with EndDate=now-1h and no StartDate -> 200.
4 (Cause 4): create with StartDate=2099-06-15T12:30:45.123456Z, then get_schedule -> microseconds preserved.
5 (Cause 5): create_schedule_group(g); delete_schedule_group(g); immediately create_schedule(GroupName=g) -> ConflictException instead of ValidationException.
6 (Cause 6): create a schedule with a valid role, iam.delete_role that role, then update_schedule re-affirming the same RoleArn -> ValidationException instead of success.
7 (Cause 7): create then delete_schedule(Name=n); get_schedule(Name=n) -> message has the extra ' in schedule group default' clause.
8 (Cause 8): create group g + schedule s in it, delete_schedule_group(g), then update_schedule(Name=s, GroupName=g) -> message names the schedule, not the group.
9 (Cause 9): create_schedule(..., ActionAfterCompletion='ARCHIVE') -> enum set printed as [NONE, DELETE].
10 (Cause 10): create_schedule(..., Description='d'*513) -> value elided in the message.

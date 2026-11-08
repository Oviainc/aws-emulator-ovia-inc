Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.snapshot-schedule-invalid-definition-fails
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.snapshot-schedule-invalid-definition-fails|_error#7|AssertionError|raise AssertionError("the call succeeded where AWS rejects it")
Issue-Commit: a644cee8f6fac0997b2852eed7edf2fa5d997735

# CreateSnapshotSchedule accepts a sub-hourly rate() definition instead of refusing it with InvalidSchedule

> _Note: this issue was identified against the code base at commit `a644cee8f6fac0997b2852eed7edf2fa5d997735`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshotSchedule

## Description
Cause 1 of 1: `CreateSnapshotSchedule` enforces no MINIMUM interval on a `rate(value unit)` schedule definition, so a definition finer than one hour is accepted and a schedule AWS would never create is stored. Failing check id: redshift.snapshot-schedule-invalid-definition-fails.

AWS: Amazon Redshift snapshot schedules fire at most once an hour; a `rate(...)` definition stating an interval shorter than 60 minutes is REFUSED and nothing is created. The refusal is the operation's modeled `InvalidScheduleFault` — Smithy: CreateSnapshotSchedule errors include `InvalidScheduleFault` (errorKind client, httpError 400, awsQueryError code `InvalidSchedule`), documented "The schedule you submitted isn't valid." `CreateSnapshotScheduleMessage$ScheduleDefinitions` is documented only as "made up of schedule expressions, for example \"cron(30 12 *)\" or \"rate(12 hours)\"" — the model states no bound, so the minimum interval is recorded behavior only: the recording shows HTTP 400, Error.Code `InvalidSchedule`, Error.Message `Schedule interval rate(5 minutes) is less than minimum interval of 60 minutes`. In that message the FIXED text is `Schedule interval ` ... ` is less than minimum interval of 60 minutes`; the only variable part is the offending definition, echoed from the request's `ScheduleDefinitions` entry verbatim (`rate(5 minutes)` in this recording — a request value, not part of the template). `60 minutes` is the published rule and is fixed text. A definition at the minimum (`rate(1 hour)`) succeeds.

Emulator: the create SUCCEEDS. The replay reports `AssertionError: the call succeeded where AWS rejects it` from the check's `_error()` helper, which demands a ClientError from `create_snapshot_schedule(ScheduleDefinitions=["rate(5 minutes)"])` and got a 200 with the schedule echoed back; the sub-hourly schedule is then stored and visible to `DescribeSnapshotSchedules`. The definition parser admits any positive integer paired with a minute/hour/day unit and rejects only a zero value, a non-decimal value, a malformed field count or an unknown expression type — there is no interval floor at all.

Why: a client that relies on AWS's rejection to validate operator- or template-supplied backup schedules gets a false positive. Against the emulator a `rate(5 minutes)` (or `rate(1 minute)`) schedule is created, associates to clusters and reports NextInvocations five minutes apart; against AWS the same call fails with `InvalidSchedule` at 400. Infrastructure code and tests that assert the refusal pass on the emulator and break on AWS, and emulator-side snapshot workloads are driven at a cadence AWS cannot produce.

Fix: the schedule-definition validation shared by the create path — the rate branch of `crate::state::parse_schedule_definition` (state/snapshot_schedules.rs, `parse_rate_expression`), which must return `ScheduleDefinitionRefusal::Invalid` with the minimum-interval message when the stated interval is under 60 minutes, so the handler lifts it to `InvalidScheduleFault` and writes nothing. The same rule must hold on every path that accepts a definition (ModifySnapshotSchedule included) and for the DryRun form, so a checked create is refused exactly as the performed one is.

Traces: redshift.snapshot-schedule-invalid-definition-fails (phase: positive control `rate(1 hour)` created, then `rate(5 minutes)` expected to be refused with `InvalidSchedule` and to leave `DescribeSnapshotSchedules(ScheduleIdentifier=<bad id>)` returning an empty `SnapshotSchedules` list). Handler citation: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_snapshot_schedule.rs lines 244-248 (the per-definition parse loop); parser citation: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/state/snapshot_schedules.rs lines 154-161 (RATE_UNITS admits `minute`/`minutes`) and 298-332 (`parse_rate_expression`, which checks only for a zero or non-decimal value).

## Evidence
Request: CreateSnapshotSchedule ScheduleIdentifier=<minted id> ScheduleDescription="behavioral fidelity invalid schedule" ScheduleDefinitions=["rate(5 minutes)"]

Expected (AWS, recorded): HTTP 400, Error.Code = InvalidSchedule (modeled InvalidScheduleFault, awsQueryError code InvalidSchedule), Error.Message = "Schedule interval rate(5 minutes) is less than minimum interval of 60 minutes" (fixed text "Schedule interval " + the request's definition + " is less than minimum interval of 60 minutes"); no schedule created, so DescribeSnapshotSchedules(ScheduleIdentifier=<that id>) answers 200 with SnapshotSchedules = [].

Actual (emulator): HTTP 200 with the schedule echoed back (ScheduleDefinitions = ["rate(5 minutes)"]) and the schedule stored. Replay output:
  File "check_redshift_snapshot_schedule_invalid_definition_fails.py", line 98, in run
    code, message = _error(lambda: rs.create_snapshot_schedule(
  File "check_redshift_snapshot_schedule_invalid_definition_fails.py", line 57, in _error
    raise AssertionError("the call succeeded where AWS rejects it")
  AssertionError: the call succeeded where AWS rejects it

Positive control in the same check (CreateSnapshotSchedule with ScheduleDefinitions=["rate(1 hour)"], exactly at the 60-minute minimum) succeeds on both, so the divergence is confined to the missing interval floor.

## Steps to Reproduce
1. Boot the emulator and point a redshift client at it.
2. CreateSnapshotSchedule with ScheduleIdentifier=<unique>, ScheduleDefinitions=["rate(1 hour)"] — succeeds (positive control; AWS agrees).
3. CreateSnapshotSchedule with a second unique ScheduleIdentifier and ScheduleDefinitions=["rate(5 minutes)"].
4. Observe HTTP 200 and the schedule echoed back. AWS answers HTTP 400 with Error.Code InvalidSchedule and Error.Message "Schedule interval rate(5 minutes) is less than minimum interval of 60 minutes".
5. DescribeSnapshotSchedules(ScheduleIdentifier=<the second id>) — the emulator reports the stored schedule; on AWS SnapshotSchedules is [] because nothing was created.
6. Equivalent with any sub-hourly rate, e.g. "rate(1 minute)" or "rate(59 minutes)".

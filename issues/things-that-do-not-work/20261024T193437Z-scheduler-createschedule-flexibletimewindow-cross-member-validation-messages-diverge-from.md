Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.create-schedule-flexible-off-with-max-forbidden scheduler.create-schedule-flexible-window-missing-max
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.create-schedule-flexible-off-with-max-forbidden|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: scheduler.create-schedule-flexible-window-missing-max|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# CreateSchedule FlexibleTimeWindow cross-member validation messages diverge from AWS's form (OFF+max, FLEXIBLE without max)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Two distinct root causes in the FlexibleTimeWindow cross-member validation of CreateSchedule. In both, the emulator answers the correct status and error code (HTTP 400 ValidationException) and correctly persists nothing; what diverges is the request-derived validation message, whose form AWS builds from the offending member and the mode it conflicts with. Both causes live in one helper and must both be closed before this issue is submitted. (Neither check is carried by the open issue 20261023T191008Z-scheduler-control-plane-ten-divergences-in-createschedule-updateschedule.md, which covers other CreateSchedule/UpdateSchedule causes.)

Cause 1 of 2: Mode=OFF with MaximumWindowInMinutes set.
AWS: Smithy (com.amazonaws.scheduler#FlexibleTimeWindow) models Mode (FlexibleTimeWindowMode, required) and MaximumWindowInMinutes (integer, range 1..1440) and states no cross-member rule, so recorded behavior only: a CreateSchedule carrying FlexibleTimeWindow={Mode: OFF, MaximumWindowInMinutes: 15} is refused with HTTP 400 ValidationException whose message is built from the request's offending member and the mode it conflicts with, naming both by their MODELED member names: 'MaximumWindowInMinutes must not be provided when FlexibleTimeWindowMode is set to OFF.'
Emulator: refuses with the same code, but in a different message form that names the members by an emulator-invented dotted path and drops the terminating period: 'FlexibleTimeWindow.MaximumWindowInMinutes must not be set when FlexibleTimeWindow.Mode is OFF'.
Why: the rule that produces this message is 'name the offending member and the conflicting member as the model names them'; a client or tool that matches on the member names AWS reports (or that surfaces the message to an operator alongside the modeled member names) reads a different field path than any AWS member, and the check asserting AWS's text fails.
Fix: the message built by crate::state::flexible_time_window_violation, called from the CreateSchedule handler (and UpdateSchedule) before anything is persisted.
Traces: scheduler.create-schedule-flexible-off-with-max-forbidden (file citation: services/service-com-amazonaws-scheduler-awschronosservice/src/ops/create_schedule.rs lines 195-202 calls crate::state::flexible_time_window_violation in services/service-com-amazonaws-scheduler-awschronosservice/src/state.rs).

Cause 2 of 2: Mode=FLEXIBLE with MaximumWindowInMinutes absent.
AWS: same model basis (no cross-member rule modeled), recorded behavior only: a CreateSchedule carrying FlexibleTimeWindow={Mode: FLEXIBLE} with no MaximumWindowInMinutes is refused with HTTP 400 ValidationException and the mirror-image message, again naming the modeled members: 'MaximumWindowInMinutes must be provided when FlexibleTimeWindowMode is set to FLEXIBLE.'
Emulator: same code, different form: 'FlexibleTimeWindow.MaximumWindowInMinutes is required when FlexibleTimeWindow.Mode is FLEXIBLE'.
Why: as Cause 1 — the required-when half of the same rule reports member names AWS never emits, so the two halves of the rule diverge together and neither can be matched against AWS's reported form.
Fix: the same helper as Cause 1 (both branches of crate::state::flexible_time_window_violation).
Traces: scheduler.create-schedule-flexible-window-missing-max.

## Evidence
Cause 1 — CreateSchedule with FlexibleTimeWindow={'Mode': 'OFF', 'MaximumWindowInMinutes': 15}: code ValidationException matches (HTTP 400) and GetSchedule correctly reports ResourceNotFoundException afterwards; expected message 'MaximumWindowInMinutes must not be provided when FlexibleTimeWindowMode is set to OFF.', actual 'FlexibleTimeWindow.MaximumWindowInMinutes must not be set when FlexibleTimeWindow.Mode is OFF' (AssertionError at check_scheduler_create_schedule_flexible_off_with_max_forbidden.py line 160).
Cause 2 — CreateSchedule with FlexibleTimeWindow={'Mode': 'FLEXIBLE'} (no MaximumWindowInMinutes): code ValidationException matches; expected message 'MaximumWindowInMinutes must be provided when FlexibleTimeWindowMode is set to FLEXIBLE.', actual 'FlexibleTimeWindow.MaximumWindowInMinutes is required when FlexibleTimeWindow.Mode is FLEXIBLE' (AssertionError at check_scheduler_create_schedule_flexible_window_missing_max.py line 126).

## Steps to Reproduce
Setup (both causes): iam.create_role trusting scheduler.amazonaws.com with an inline sqs:SendMessage policy; use any SQS queue ARN as Target.Arn with that RoleArn.
1 (Cause 1): scheduler.create_schedule(Name=n, ScheduleExpression='rate(5 minutes)', FlexibleTimeWindow={'Mode': 'OFF', 'MaximumWindowInMinutes': 15}, Target=t) -> ValidationException, but the message names 'FlexibleTimeWindow.MaximumWindowInMinutes'/'FlexibleTimeWindow.Mode' instead of the modeled 'MaximumWindowInMinutes'/'FlexibleTimeWindowMode' form AWS emits.
2 (Cause 2): scheduler.create_schedule(Name=n2, ScheduleExpression='rate(5 minutes)', FlexibleTimeWindow={'Mode': 'FLEXIBLE'}, Target=t) -> ValidationException with 'FlexibleTimeWindow.MaximumWindowInMinutes is required when FlexibleTimeWindow.Mode is FLEXIBLE' instead of 'MaximumWindowInMinutes must be provided when FlexibleTimeWindowMode is set to FLEXIBLE.'

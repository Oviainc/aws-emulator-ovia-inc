Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__scheduler.create-schedule-target-input-262144-byte-boundary iam__scheduler.one-time-at-schedule-start-after-end-date-rejected
Issue-Key: iam__scheduler.create-schedule-target-input-262144-byte-boundary|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iam__scheduler.one-time-at-schedule-start-after-end-date-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# iam__scheduler: 2 net-new causes — an over-maximum Target.Input is accepted, and the StartDate/EndDate refusal is misworded

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Two distinct root causes in EventBridge Scheduler's CreateSchedule validation.

Cause 1 of 2: a Target.Input above the service maximum is accepted.
AWS: CreateSchedule caps the target payload at 262144 bytes; an Input one byte over the boundary is refused with a `ValidationException` and no schedule is created, while the payload exactly at the boundary is accepted. Smithy: scheduler#Target.Input targets TargetInput, which carries only @length(min 1) — the model states no maximum, so the 262144-byte cap is service behaviour: recorded behavior only, and the recording shows the over-boundary call refused with a client error.
Emulator: the over-boundary CreateSchedule returns HTTP 200 with a ScheduleArn — the schedule is created.
Why: a client that sizes its payloads against the documented limit gets no feedback when it exceeds it; the schedule is accepted here and will be rejected (or will fail at invocation) against AWS, so a boundary that the client tested as enforced is not.
Fix: Target.Input size validation in the CreateSchedule/UpdateSchedule validation path.
Traces: iam__scheduler.create-schedule-target-input-262144-byte-boundary.

Cause 2 of 2: the StartDate-after-EndDate refusal uses emulator wording.
AWS: a one-time `at(...)` schedule whose StartDate is after its EndDate is refused with message `The StartDate you specify must come before the EndDate.` — fixed text with no variable part. Smithy: scheduler#CreateSchedule declares ValidationException; the message is a service fact — recorded behavior only, pinned by the recording.
Emulator: the message is `StartDate must be earlier than EndDate.`
Why: a client that matches on or surfaces the message does not recognise the condition; message-equality against AWS fails.
Fix: the StartDate/EndDate precondition message in the CreateSchedule validation path.
Traces: iam__scheduler.one-time-at-schedule-start-after-end-date-rejected.

## Evidence
Cause 1 — Expected: a ClientError for the over-boundary Target.Input. Actual: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {… 'ScheduleArn': 'arn:aws:scheduler:us-west-2:000000000000:schedule/default/…'}.
Cause 2 — Expected message 'The StartDate you specify must come before the EndDate.'; Actual 'StartDate must be earlier than EndDate.'

## Steps to Reproduce
Cause 1: CreateSchedule with Target.Input of 262145 bytes; observe a 200 with a ScheduleArn.
Cause 2: CreateSchedule with ScheduleExpression at(...) and StartDate later than EndDate; compare the ValidationException message.

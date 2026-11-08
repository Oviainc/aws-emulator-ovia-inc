Issue-Source: trace-sweep-divergence
Issue-Trace: scheduler-schedule-start-end-date-timestamp-precision-preserved
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: scheduler-schedule-start-end-date-timestamp-precision-preserved
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler|GetSchedule|2xx->2xx|$.StartDate
Issue-Related: 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md
Issue-Commit: 4cf020d2c1273a702afbbf64c24c2520a1d80374

# scheduler GetSchedule rounds StartDate/EndDate down a millisecond instead of echoing the submitted instant

> _Note: this issue was identified against the code base at commit `4cf020d2c1273a702afbbf64c24c2520a1d80374`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSchedule

## Description
One root cause in the EventBridge Scheduler wire surface, found by replaying simparity-wire-aws-v1 against the emulator. It is the only divergence cluster in the corpus that no open issue owns: the cluster's other asserted keys ($.ActionAfterCompletion, $.ScheduleExpressionTimezone, $.Target.RetryPolicy, all 'missing') are Cause 1 of the open issue 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md and are NOT re-filed here; the $.StartDate value_mismatch below is not covered by that issue (nor by the behavioral issue 20261023T191008Z-scheduler-control-plane-ten-divergences-in-createschedule-updateschedule.md) and is what this issue is about.

Cause 1 of 1: a schedule's StartDate (and, by the same code path, EndDate) comes back one millisecond EARLIER than the instant the create request stated.
AWS: CreateSchedule accepts StartDate/EndDate as modeled timestamps (Smithy: CreateScheduleInput$StartDate -> StartDate, type timestamp, 'The date, in UTC, after which the schedule can begin invoking its target'; GetScheduleOutput$StartDate likewise) and GetSchedule answers HTTP 200 echoing the instant that was stored, at millisecond resolution, with no loss. The recording shows a create whose StartDate carried a sub-second part and a GetSchedule that reports the SAME instant back: the sub-second remainder that was submitted, to the millisecond, is what every later read returns. The service never reports an instant strictly earlier than the one it accepted.
Emulator: GetSchedule answers 200 but $.StartDate is one millisecond below the submitted instant (replay: expected 4085209845.12, got 4085209845.119) — i.e. the last whole millisecond of the submitted value is dropped. The emulator normalizes both instants on write via crate::state::to_millisecond_precision, which TRUNCATES subsec_nanos toward zero (whole_milliseconds = subsec_nanos / 1_000_000). Hypothesis: the epoch-seconds wire value is deserialized through a binary floating-point number, so a decimal sub-second part that is not exactly representable (e.g. .12) lands at 119_999_999ns rather than 120_000_000ns, and truncation then floors it to 119ms instead of recovering the submitted 120ms. The emulator's own unit test (services/service-com-amazonaws-scheduler-awschronosservice/src/state/schedule_definition.rs, an_instant_is_held_to_the_millisecond) asserts the truncating behavior, so this is a deliberate normalization that is wrong for float-derived values.
Why: a client that writes a schedule with a sub-second StartDate and reads it back gets an instant that differs from the one it sent, so an idempotent reconciler comparing desired-vs-actual timestamps sees a permanent diff and re-writes the schedule forever; assertions and CloudFormation/Terraform-style drift detection on StartDate/EndDate fail against the emulator while passing against AWS, and the error compounds for EndDate, where a one-millisecond-early close can fall on the wrong side of a boundary check.
Fix: the instant normalization in services/service-com-amazonaws-scheduler-awschronosservice/src/state/schedule_definition.rs (to_millisecond_precision, applied at services/service-com-amazonaws-scheduler-awschronosservice/src/ops/create_schedule.rs where start_date/end_date are stored, and the equivalent in update_schedule) must not lose a millisecond that was actually submitted: either derive the millisecond from the wire decimal exactly rather than from a float remainder, or round to the nearest millisecond instead of truncating. The GetSchedule response projection must then echo the stored instant unchanged.
Traces: scheduler-schedule-start-end-date-timestamp-precision-preserved#task#4.

## Evidence
Replay of simparity-wire-aws-v1: 103 steps, 87 passed, 16 mismatched, score 84.5%, emulator up (/__admin/health 200, data listener accepts). 10 divergence clusters; 9 carry owned_by (8 scheduler clusters owned by 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md, 1 iam CreateRole policy-document cluster owned by 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md) and are excluded.
This cluster — GetSchedule, expected 200 / actual 200, fixture scheduler-schedule-start-end-date-timestamp-precision-preserved step task#4:
  $.StartDate: value_mismatch (expected 4085209845.12, got 4085209845.119)
The recorded expectation is the instant the create request stated; the emulator reports an instant 1ms earlier. The quoted numbers are this recording's values — the requirement is that the reported instant EQUALS the submitted one to the millisecond, for any submitted value.
The same step also reports $.ActionAfterCompletion / $.ScheduleExpressionTimezone / $.Target.RetryPolicy missing; those keys are Cause 1 of the owning issue above and are not part of this finding.

## Steps to Reproduce
Setup as the fixture does: iam.create_role trusting scheduler.amazonaws.com with an sqs:SendMessage inline policy, and sqs.create_queue for the target ARN.
1. create_schedule(Name=n, ScheduleExpression='rate(10 minutes)', FlexibleTimeWindow={'Mode':'OFF'}, Target={'Arn': queue_arn, 'RoleArn': role_arn}, StartDate=<an instant with a non-zero sub-second part whose decimal epoch-seconds form is not exactly representable in binary floating point, e.g. ...845.12>, EndDate=<a later such instant>).
2. get_schedule(Name=n) -> $.StartDate is one millisecond below the submitted value (…845.119 instead of …845.12). Expected: the submitted instant, to the millisecond.
Replay fixture scheduler-schedule-start-end-date-timestamp-precision-preserved, step task#4.

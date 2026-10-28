Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected|run#9|AssertionError|assert msg == message, (
Issue-Key: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120|run#10|AssertionError|assert message == _BAD_PERIOD_MESSAGE, (
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# PutMetricAlarm refusal messages diverge from AWS: evaluation-horizon (day/week) and wall-clock period rules both return documentation prose instead of the ValidationError text

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Two net-new root causes in the CloudWatch PutMetricAlarm validation path. In BOTH cases the emulator correctly refuses the request with HTTP 400 and error code `ValidationError` — only the error MESSAGE diverges, and in both cases the emulator's message is a sentence lifted from / paraphrasing the Smithy documentation prose rather than the message real CloudWatch returns for that error class. A client (or a CLI/SDK wrapper, or any assertion on the refusal) that reads the message gets text AWS never emits.

=== Cause 1 of 2: EvaluationPeriods x Period evaluation-horizon refusal message ===
AWS: PutMetricAlarm refuses an alarm whose total evaluation horizon (EvaluationPeriods * Period) exceeds the regime ceiling for its Period — 86,400 s (one day) when Period < 3600, 604,800 s (one week) when Period >= 3600. Smithy: PutMetricAlarm.Period is `Period` with `range {min: 1}` and PutMetricAlarm.EvaluationPeriods is `EvaluationPeriods` with `range {min: 1}`; the model declares NO relation between the two and does not model `ValidationError` at all (the only modeled error on PutMetricAlarm is LimitExceededFault/`LimitExceeded`), so this rule is recorded behavior only. Recorded response: HTTP 400, Code `ValidationError`, and one of exactly two messages. Sub-hour regime: `Metrics cannot be checked across more than a day (EvaluationPeriods * Period must be <= 86400) for alarms using period < 3600`. Hour-or-greater regime: `Metrics cannot be checked across more than a week (EvaluationPeriods * Period must be <= 604800) for alarms using period >= 3600`. Both messages are entirely FIXED text — there are no request- or state-derived variable parts at all: the member names (`EvaluationPeriods`, `Period`), the ceilings (`86400`, `604800`) and the Period pivot (`3600`) are constants of the rule, not values echoed from the request. Which of the two messages is returned is selected solely by whether the request's Period is < 3600 or >= 3600.
Emulator: refuses with the right status and the right code, but the message for the sub-hour regime is `For alarms with a period of less than one hour (3,600 seconds), the total evaluation period can't be longer than one day (86,400 seconds).` — different wording, different punctuation, digit grouping (`3,600`, `86,400`) AWS does not use, and it names neither `EvaluationPeriods * Period` nor the bare numeric ceiling. Hypothesis: the message is composed from documentation prose about the limit instead of the AWS ValidationError string. The week-regime leg (Period=3600 x 169, Period=86400 x 8) is behind the first failing assertion and so is unverified; it must be made to return the week message verbatim too.
Why: the refusal is unmatchable. Clients and operators routinely branch or alert on the ValidationError message because the code `ValidationError` is shared by dozens of unrelated CloudWatch refusals; with AWS's text replaced, no consumer can tell an evaluation-horizon refusal from any other validation failure, and golden-output/parity tests on the message fail against the emulator.
Fix: the PutMetricAlarm validation component for the Period x EvaluationPeriods horizon rule — it must emit AWS's two fixed strings verbatim, selected by the Period < 3600 / Period >= 3600 regime, and must keep doing so on both CREATE and UPDATE without partially writing the alarm.
Traces: cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected (assertion at the Period=60 x EvaluationPeriods=1441 leg).

=== Cause 2 of 2: WallClockWindow unsupported-Period refusal message ===
AWS: PutMetricAlarm with EvaluationWindow={"WallClockWindow": {...}} refuses any Period other than 60, 300, 3600, 86400 or 604800 — the identical request with EvaluationWindow={"SlidingWindow": {}} is accepted. Smithy: the rule lives ONLY in the `WallClockWindow` shape's documentation ("When you use a wall clock window, the alarm's period must be 1 minute (60 seconds), 5 minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week (604,800 seconds). Other period values aren't supported with a wall clock window."); `Period` itself is only `range {min: 1}` and `ValidationError` is not modeled, so the refusal is recorded behavior only. Recorded response: HTTP 400, Code `ValidationError`, Message `Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week.` — wholly fixed text, with no request- or state-derived variable parts (the offending Period is NOT echoed into the message).
Emulator: refuses with the right status and code, but returns `When you use a wall clock window, the alarm's period must be 1 minute (60 seconds), 5 minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week (604,800 seconds). Other period values aren't supported with a wall clock window.` — i.e. the WallClockWindow documentation paragraph quoted verbatim as the error message. This confirms the shared root cause with Cause 1: refusal text in this handler is being sourced from model documentation rather than from AWS's error strings.
Why: same consequence as Cause 1 — the ValidationError message is the only thing distinguishing this refusal, and the emulator's substitute is a doc sentence AWS never returns, so message-based handling and parity assertions diverge. It also leaks internal model documentation onto the wire as service error text.
Fix: the PutMetricAlarm validation component for EvaluationWindow/WallClockWindow Period admissibility — emit AWS's fixed string verbatim and stop sourcing error text from shape documentation.
Traces: cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120 (the Period=120 + WallClockWindow negative leg; the alarm-evaluation legs of that check are behind this first assertion and unverified).

Note: no existing open CloudWatch issue covers either message. The two open PutMetricAlarm issues (20261023T140640Z-cloudwatch-cloudfront-sns-sqs-putmetricalarm-accepts-an-sns-alarmaction-in-another-region.md and 20261024T182555Z-cloudwatch-cloudfront-sns-sqs-putmetricalarm-accepts-a-cross.md) concern cross-region SNS AlarmAction acceptance, a different validation and a different service token; and the open cloudwatch alarm-evaluation issues concern alarms never evaluating published data, not refusal wording.

## Evidence
Cause 1 — cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected:
  Request: PutMetricAlarm(AlarmName=<unique>, Namespace=<unique>, MetricName="Horizon", Statistic="Sum", Threshold=1.0, ComparisonOperator="GreaterThanThreshold", DatapointsToAlarm=1, TreatMissingData="notBreaching", ActionsEnabled=false, Period=60, EvaluationPeriods=1441)
  Expected (real AWS, us-west-2): HTTP 400, Code "ValidationError", Message "Metrics cannot be checked across more than a day (EvaluationPeriods * Period must be <= 86400) for alarms using period < 3600"
  Actual (emulator):             HTTP 400, Code "ValidationError", Message "For alarms with a period of less than one hour (3,600 seconds), the total evaluation period can't be longer than one day (86,400 seconds)."
  Replay assertion: AssertionError: Period=60 x EvaluationPeriods=1441: the refusal must state the day rule verbatim, got "For alarms with a period of less than one hour (3,600 seconds), the total evaluation period can't be longer than one day (86,400 seconds)."
  Same divergence is expected on the remaining legs the check never reached: (300, 289) -> day message; (60, 1500) -> day message; (3600, 169) -> "Metrics cannot be checked across more than a week (EvaluationPeriods * Period must be <= 604800) for alarms using period >= 3600"; (86400, 8) -> week message. Legal neighbours 60x1440, 300x288, 3600x168, 86400x7 and 3600x25 must still be accepted.

Cause 2 — cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120:
  Request: PutMetricAlarm(AlarmName=<unique>, Namespace=<unique>, MetricName="WindowSignal", Statistic="Maximum", EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator="GreaterThanThreshold", ActionsEnabled=false, Period=120, EvaluationWindow={"WallClockWindow": {"Timezone": "UTC"}})
  Expected (real AWS, us-west-2): HTTP 400, Code "ValidationError", Message "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week."
  Actual (emulator):             HTTP 400, Code "ValidationError", Message "When you use a wall clock window, the alarm's period must be 1 minute (60 seconds), 5 minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week (604,800 seconds). Other period values aren't supported with a wall clock window."
  Replay assertion: AssertionError: Period=120 with a WallClockWindow returned message "When you use a wall clock window, ...", expected 'Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week.'
  The emulator's string is byte-identical to the second paragraph of the WallClockWindow shape documentation in the service's own Smithy model, which is where it is being taken from.
  Control that must keep passing: the identical request with EvaluationWindow={"SlidingWindow": {}} and Period=120 is ACCEPTED and reads back Period=120 with EvaluationWindow={"SlidingWindow": {}}.

## Steps to Reproduce
Cause 1:
1. cw = client("cloudwatch")
2. PutMetricAlarm(AlarmName="horizon-probe", Namespace="ProbeNs", MetricName="Horizon", Statistic="Sum", Threshold=1.0, ComparisonOperator="GreaterThanThreshold", DatapointsToAlarm=1, TreatMissingData="notBreaching", ActionsEnabled=false, Period=60, EvaluationPeriods=1440) -> must succeed (positive control; 86,400 s exactly).
3. Repeat the same call with EvaluationPeriods=1441. Observe HTTP 400 / ValidationError, and compare the Message to "Metrics cannot be checked across more than a day (EvaluationPeriods * Period must be <= 86400) for alarms using period < 3600" — the emulator returns the "For alarms with a period of less than one hour ..." paraphrase instead.
4. Repeat with Period=3600, EvaluationPeriods=169 and with Period=86400, EvaluationPeriods=8; the Message must be "Metrics cannot be checked across more than a week (EvaluationPeriods * Period must be <= 604800) for alarms using period >= 3600".
5. Confirm DescribeAlarms shows the alarm still at the last accepted (Period, EvaluationPeriods) after each refusal, and that a refused CREATE under a fresh name leaves no alarm.

Cause 2:
1. cw = client("cloudwatch")
2. PutMetricAlarm(AlarmName="wallclock-p120", Namespace="ProbeNs", MetricName="WindowSignal", Statistic="Maximum", EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator="GreaterThanThreshold", ActionsEnabled=false, Period=120, EvaluationWindow={"WallClockWindow": {"Timezone": "UTC"}}).
3. Observe HTTP 400 / ValidationError and compare Message to "Wall Clock evaluation period must be 1 minute, 5 minutes, 1 hour, 1 day or 1 week." — the emulator returns the WallClockWindow documentation paragraph instead.
4. DescribeAlarms(AlarmNames=["wallclock-p120"], AlarmTypes=["MetricAlarm"]) must return [] (no leaked alarm).
5. Control: the same call with EvaluationWindow={"SlidingWindow": {}} must be accepted and read back with Period=120.

Replay both with: run_behavioral_suite, checks cloudwatch.alarm-missing-data-and-m-of-n-evaluation-horizon-beyond-day-or-week-rejected and cloudwatch.deep-cloudwatch-1-cw-warmup-wall-clock-window-gates-evaluation-and-rejects-period-120.

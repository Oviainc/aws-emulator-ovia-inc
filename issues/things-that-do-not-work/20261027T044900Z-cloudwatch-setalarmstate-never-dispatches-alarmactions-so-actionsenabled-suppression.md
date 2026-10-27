Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action|_drain_queue#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected|run#59|AssertionError|assert error.get("Message") == (
Issue-Commit: 6d436f6d30cd83c53e890485fd5b45dc369d520d

# CloudWatch: SetAlarmState never dispatches AlarmActions (so ActionsEnabled suppression is unobservable), and the PutCompositeAlarm AlarmRule parse-error message is not AWS's (2 causes)

> _Note: this issue was identified against the code base at commit `6d436f6d30cd83c53e890485fd5b45dc369d520d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Two net-new root causes in the pure-`cloudwatch` service, both grounded in failing behavioral checks that the replay report did not mark ALREADY FILED and did not mark OWNED by any open issue.

Cause 1 of 2: a forced alarm transition never dispatches the alarm's actions, so neither delivery nor its ActionsEnabled suppression can be observed.
AWS: SetAlarmState (Smithy: SetAlarmStateInput — AlarmName @required @length 1..255, StateValue @required enum, StateReason @required @length 0..1023, StateReasonData @length 0..4000; modeled errors InvalidFormatFault httpError 400 / awsQueryError InvalidFormat, ResourceNotFound httpError 404 / awsQueryError ResourceNotFound) forces the named alarm into the given state, and that forced transition is a real state change: CloudWatch invokes the action list for the state entered. For an alarm created by PutMetricAlarm with AlarmActions=[an SNS topic ARN], a forced transition into ALARM publishes an alarm notification to that topic, whose JSON body carries AlarmName (the name from the request), NewStateValue=ALARM and StateReason, and an SQS queue subscribed to the topic receives it. DisableAlarmActions (Smithy: DisableAlarmActionsInput.AlarmNames @required @length 0..100, no modeled errors) sets ActionsEnabled=false, which suppresses that dispatch while leaving the state transition itself intact; EnableAlarmActions restores it. Recorded behavior: the check's positive control forces ALARM with actions still enabled and measures the delivery latency on this exact wiring, then uses >= 3x that measured latency as the calibrated absence window for the suppressed leg.
Emulator: no notification is ever delivered. The check's first blocking wait is that positive control, and the check is killed by the harness 60 s cap while still polling ReceiveMessage on the subscribed queue — so the alarm's AlarmActions are never invoked at all, and the two legs that depend on it (DisableAlarmActions suppressing a fresh ALARM transition, EnableAlarmActions restoring delivery for the next one) are never reached. Nothing after the first wait — including the DescribeAlarms assertions that ActionsEnabled flips to false and back to true — is exercised.
Why: a client that forces an alarm state to drive a downstream action (the standard way runbooks, tests and remediation pipelines trigger a paging or scaling path deterministically) gets a 200 from SetAlarmState and then nothing: no SNS publish, no SQS message, no action. Worse, the ActionsEnabled contract becomes unverifiable — an operator who disables actions to silence an alarm cannot distinguish "suppressed correctly" from "nothing was ever going to fire", so a change that re-enables actions in production is untested.
Fix: alarm action dispatch in the cloudwatch service's SetAlarmState handler and its alarm state — the transition path must invoke the state's action list (AlarmActions / OKActions / InsufficientDataActions) and must gate that invocation on the alarm's ActionsEnabled flag as set by DisableAlarmActions/EnableAlarmActions.
Related already-open issue: 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md lists this check id among its Cause-1 traces, but the suite report did not mark this failure as OWNED by it (the replay key that issue carries for this check does not match this run's), so the cause is restated here with this run's evidence rather than assumed covered.
Traces: cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action.

Cause 2 of 2: the PutCompositeAlarm AlarmRule parse-failure message diverges from AWS's wording, collapsing the distinct refusal classes into one indistinguishable string.
AWS: PutCompositeAlarm (Smithy: PutCompositeAlarmInput — AlarmName @required @length 1..255, AlarmRule @required @length 1..10240, ActionsEnabled, AlarmActions/OKActions/InsufficientDataActions @length 0..5; the ONLY modeled error is LimitExceededFault httpError 400 / awsQueryError LimitExceeded, so the rule-validation refusal is not in the model — recorded behavior only, measured by this check against real AWS in us-west-2). A syntactically malformed AlarmRule is refused with HTTP 400 and awsQuery error code ValidationError, and the message is a PARSE diagnosis of the form:
  Error in AlarmRule [Unsupported token '<token>' at char <index>, must be: '(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA]
Fixed text (the requirement): the literals "Error in AlarmRule [Unsupported token '", "' at char ", and ", must be: '(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA]". Variable parts, taken from the submitted request: <token> is the offending token as it appears in the submitted AlarmRule, and <index> is its character offset within that AlarmRule. This message shape is what separates a parse failure from the OTHER two ValidationError classes the same operation produces — "Could not save the composite alarm as alarms [<arn>] in the alarm rule do not exist" (unknown / self-referencing child) and DeleteAlarms' "Cannot delete <name> as there are composite alarm(s) depending on it."
Emulator: the code is right (ValidationError) but the message is its own prose: "AlarmRule: expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(` at offset <n>". It drops the "Error in AlarmRule [...]" wrapper, never names the offending token, says "at offset" instead of "at char", omits AT_LEAST from the expected-token set and reorders the rest, and backticks the open paren instead of single-quoting it. (The numeric offset in the recorded run was a request-derived value — the index of the duplicated AND in that run's generated rule — and is part of the template's variable slot, not of the divergence.) Every other leg of this check passed, so composite derivation over ALARM()/OK(), the referenced-child delete protection and the two existence refusals are already faithful; only the parse message diverges.
Why: a client or IaC tool that classifies a PutCompositeAlarm refusal by message — the only way to tell a typo in the rule from a reference to an alarm that does not exist yet, since all three share the ValidationError code — misclassifies every malformed rule, and any test or retry policy keyed on AWS's wording breaks.
Fix: the AlarmRule expression parser's error rendering, in the cloudwatch service's PutCompositeAlarm handler validation path.
Traces: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected.

## Evidence
Cause 1 — cloudwatch.alarm-actions-disabled-suppresses-state-triggered-action [mismatch]:
  Error: Check timed out after 60.0s
The only wait reachable inside 60 s is the positive control, which forces ALARM with actions enabled and then polls the SNS-subscribed SQS queue:
  cw.set_alarm_state(AlarmName=alarm_name, StateValue="ALARM", StateReason="behavioral-check-baseline")
  baseline = _wait_for_messages(ctx, queue["url"], expected_count=1, timeout_s=90.0)
  -> _wait_for_messages: raise AssertionError(f"expected {expected_count} messages, got {len(out)} within {timeout_s}s")
Expected: one SNS->SQS notification whose JSON Message has AlarmName == the alarm just created and NewStateValue == "ALARM".
Actual: zero messages; the check is killed at the 60 s cap before DisableAlarmActions / the calibrated absence window / EnableAlarmActions ever run.

Cause 2 — cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected [mismatch]:
  File "check_cloudwatch_composite_alarm_rule_cycle_and_unknown_child_rejected.py", line 340, in run
    assert error.get("Message") == (...)
  AssertionError: a malformed rule must be refused as a PARSE failure, got 'AlarmRule: expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(` at offset 55'
Expected message (AWS, 400 / ValidationError):
  Error in AlarmRule [Unsupported token 'AND' at char <index of the offending token in the submitted AlarmRule>, must be: '(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA]
Actual message (400 / ValidationError):
  AlarmRule: expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(` at offset <index>
Status and error code agree; only the message text diverges. The preceding assertions in the same check — AlarmRule verbatim round-trip, ALARM()/OK() AND derivation, "Cannot delete <name> as there are composite alarm(s) depending on it.", and "Could not save the composite alarm as alarms [<arn>] in the alarm rule do not exist" for both the self-reference and the ghost child — all passed.

## Steps to Reproduce
Cause 1 (SetAlarmState action dispatch / ActionsEnabled):
1. SQS CreateQueue, GetQueueAttributes(QueueArn); SNS CreateTopic; SetQueueAttributes with a Policy allowing sns.amazonaws.com sqs:SendMessage from that topic ARN; SNS Subscribe(TopicArn, Protocol="sqs", Endpoint=<queue arn>).
2. PutMetricAlarm(AlarmName=A, Namespace=NS, MetricName="Errors", Statistic="Average", Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator="GreaterThanThreshold", TreatMissingData="missing", AlarmActions=[<topic arn>]).
3. SetAlarmState(AlarmName=A, StateValue="ALARM", StateReason="baseline").
4. ReceiveMessage on the queue -> AWS delivers one notification whose Message JSON has AlarmName=A and NewStateValue="ALARM". Emulator delivers nothing (polls to the harness cap).
5. (Blocked by step 4.) DisableAlarmActions(AlarmNames=[A]); DescribeAlarms(AlarmNames=[A]) -> ActionsEnabled must be false; SetAlarmState(A, "OK") then SetAlarmState(A, "ALARM") -> NO notification for >= 3x the latency measured in step 4. Then EnableAlarmActions(AlarmNames=[A]); SetAlarmState(A, "OK") then SetAlarmState(A, "ALARM") -> exactly one notification with NewStateValue="ALARM".

Cause 2 (PutCompositeAlarm AlarmRule parse message):
1. PutMetricAlarm(AlarmName=leafA, Namespace=NS, MetricName="Errors", Statistic="Sum", Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator="GreaterThanThreshold", ActionsEnabled=False) and the same for leafB.
2. PutCompositeAlarm(AlarmName=C, AlarmRule='ALARM("leafA") AND AND OK("leafB")', ActionsEnabled=False) — i.e. a rule with a duplicated AND so the second AND sits where an operand is expected.
3. Observe the refusal: HTTP 400, Code ValidationError. AWS Message: Error in AlarmRule [Unsupported token 'AND' at char <offset of that second AND in the rule string>, must be: '(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA]. Emulator Message: AlarmRule: expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(` at offset <n>.
4. DescribeAlarms(AlarmNames=[C], AlarmTypes=["CompositeAlarm"]) -> CompositeAlarms must be empty (the emulator is already correct here).

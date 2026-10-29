Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.composite-alarm-unsatisfied-rule-no-action
Issue-Key: cloudwatch__sns__sqs.composite-alarm-unsatisfied-rule-no-action|_assert_stays#15|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# A composite alarm whose rule is unsatisfied never produces its observable no-action outcome (chain never settles)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutCompositeAlarm

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: the composite-alarm → SNS → SQS no-action path never settles, because the composite is never evaluated against its children.
AWS: recorded behavior only (AlarmRule is modelled as an opaque string). A composite alarm whose AlarmRule is NOT satisfied by its children's current states stays out of ALARM and dispatches no action, so the SQS queue subscribed to its SNS action receives nothing — and that emptiness is only meaningful once a positive control has shown the same wiring CAN deliver, which is what this check establishes first.
Emulator: the check exhausts its 60s budget; no composite state and no notification is ever observed, so the positive control never settles and the no-action assertion cannot be made.
Why: composite alarms neither fire nor demonstrably stay quiet, so an operator cannot validate de-noising rules at all. This is the SNS/SQS-wired instance of the composite evaluation gap; the `cloudwatch`-service composite evaluation causes are filed separately under this sweep.
Fix: the composite-alarm evaluator (AlarmRule over child states) and the alarm action dispatcher it feeds.
Traces: cloudwatch__sns__sqs.composite-alarm-unsatisfied-rule-no-action.

## Evidence
cloudwatch__sns__sqs.composite-alarm-unsatisfied-rule-no-action — `Error: Check timed out after 60.0s`. The check creates child metric alarms, a composite alarm with an AlarmRule that the children do not satisfy and an SNS action, and an SQS queue subscribed to the topic; it then polls the composite's state and the queue. Neither the composite state nor the control delivery ever appears.

## Steps to Reproduce
Replay cloudwatch__sns__sqs.composite-alarm-unsatisfied-rule-no-action: create child alarms, PutCompositeAlarm with an unsatisfied AlarmRule and an SNS AlarmAction, subscribe an SQS queue to the topic, drive the children, then poll DescribeAlarms (AlarmTypes=['CompositeAlarm']) and the queue.

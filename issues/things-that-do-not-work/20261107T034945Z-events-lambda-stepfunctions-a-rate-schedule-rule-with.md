Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-schedule-rule-target-omits-input-when-unset
Issue-Key: events__lambda__stepfunctions.refarch-sfn-schedule-rule-target-omits-input-when-unset|run#18|Timeout|time.sleep(5)
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# events__lambda__stepfunctions: a rate() schedule rule with a Step Functions target never fires, so no StartExecution ever happens

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1 — an EventBridge rule whose ScheduleExpression is rate(1 minute) never invokes its target.
AWS: recorded behavior: PutRule(ScheduleExpression='rate(1 minute)', State='ENABLED') plus PutTargets(Arn=<state machine arn>, RoleArn=<events role>) makes EventBridge call states:StartExecution on its own cadence — the first firing lands within about a minute of PutRule and the next one a rate period later. A target that declares no Input receives EventBridge's own scheduled-event envelope as the execution input (version, id, detail-type "Scheduled Event", source aws.events, account, time, region, resources [the rule ARN], detail {}), not a reshaped or empty payload, and the envelope's id is fresh on every firing.
Emulator: the rule and target are accepted and ListTargetsByRule reports the wiring, but no execution is ever started — the check polls for the first firing until the replay budget ends, so neither the cadence nor the envelope can be observed.
Why: every scheduled workflow (the poller topology this check models) is dead on arrival: a client sees a healthy ENABLED rule with a correctly wired target and no invocations, and anything that derives work from the scheduled envelope never runs.
Fix: the EventBridge rule scheduler — a dispatch loop that evaluates ScheduleExpression and invokes each target with the scheduled-event envelope (plus the states:StartExecution target integration).
Related OPEN issues that name adjacent symptoms but not this one: 20261102T091831Z-events-api-destinations-never-deliver-or-dead-letter.md (PutRule's handling of ScheduleExpressions) and 20261023T191008Z-scheduler-eventbridge-scheduler-never-fires-a-schedule-no-invocation.md (the separate EventBridge Scheduler service).
Traces: events__lambda__stepfunctions.refarch-sfn-schedule-rule-target-omits-input-when-unset

## Evidence
From this replay of simparity-behavioral-aws-v1: the check 'refarch-sfn-schedule-rule-target-omits-input-when-unset' ended with "Check timed out after 60.0s" (its own first-firing deadline is 360 s) after PutRule(rate(1 minute), ENABLED) + PutTargets(Arn=<state machine>, RoleArn=<events role>) and after asserting the target was wired; no execution of the target state machine had appeared, so the per-firing envelope assertions (source aws.events, detail-type "Scheduled Event", resources [rule ARN], fresh id) never ran.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com and a trivial STANDARD state machine (a single Pass state).
2. Create an IAM role trusted by events.amazonaws.com with states:StartExecution on that state machine.
3. PutRule(Name=<name>, ScheduleExpression='rate(1 minute)', State='ENABLED') and PutTargets(Rule=<name>, Targets=[{Id: 'Target0', Arn: <state machine arn>, RoleArn: <events role>}]).
4. Poll ListExecutions on the state machine for the first and second firings, then read each execution's input: it must be EventBridge's scheduled-event envelope with a fresh id per firing.

Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__logs.cdk-deploy-log-metric-filter-alarm-ignores-non-matching-events
Issue-Key: cloudformation__cloudwatch__logs.cdk-deploy-log-metric-filter-alarm-ignores-non-matching-events|run#19|IndexError|initial = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]["StateValue"]
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A CDK-deployed log metric filter alarm exposes no state/history rows, so the non-matching-event check cannot read a datapoint

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarmHistory

## Description
Cause 1 of 1: the deployed metric-filter alarm surface answers an empty list where the recording has at least one row.
AWS: after a CDK deploy of a log group, a metric filter and an alarm on the filter's metric, the alarm is described with a state and its history/datapoint rows are non-empty once events are ingested, so a client can assert that a non-matching event moved nothing. The recorded run reads row [0] of that listing.
Emulator: the listing the check indexes is empty, so the check dies with 'IndexError: list index out of range' — the emulator returned no alarm state/history or metric row at all for the deployed alarm.
Why: a client cannot distinguish 'the filter correctly ignored the event' from 'no alarm evaluation ever happened', which is the whole point of the negative control.
Fix: the alarm evaluation/history state in the cloudwatch crate as reached through a CloudFormation-deployed AWS::CloudWatch::Alarm plus AWS::Logs::MetricFilter (an alarm must expose its state and history rows once deployed).
Traces: cloudformation__cloudwatch__logs.cdk-deploy-log-metric-filter-alarm-ignores-non-matching-events.

## Evidence
Expected: at least one alarm state/history row to inspect after the deploy. Actual: the check raises 'IndexError: list index out of range' while indexing the emulator's empty listing.

## Steps to Reproduce
Replay cloudformation__cloudwatch__logs.cdk-deploy-log-metric-filter-alarm-ignores-non-matching-events: deploy the CDK stack (log group + metric filter + alarm), put a non-matching event and read the alarm's state/history rows.

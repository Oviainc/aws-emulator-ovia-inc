Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-m-of-n-breach-count-drives-alarm cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-unassumable-query-role-evaluation-error
Issue-Key: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-m-of-n-breach-count-drives-alarm|_poll_until#34|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero|_poll_until#15|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-unassumable-query-role-evaluation-error|_poll_until#23|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CloudWatch alarms backed by a Logs ScheduledQuery never evaluate: they stay INSUFFICIENT_DATA 'Unchecked: Initial alarm creation'

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: an alarm with a ScheduledQueryConfiguration is never evaluated on its schedule.
AWS: an alarm whose ScheduledQueryConfiguration names a query, log groups, a role and a rate(1 minute) schedule runs that query each period and transitions: enough breaching query results drive it to ALARM (m-of-n over QueryResultsToEvaluate/QueryResultsToAlarm), a zero-match period is treated as missing rather than zero, and an unassumable ScheduledQueryRoleARN produces an evaluation error outcome. StateReason then names the evaluation, not the creation.
Emulator: in all three scenarios the alarm never leaves 'StateValue': 'INSUFFICIENT_DATA' with 'StateReason': 'Unchecked: Initial alarm creation' and ActionLogLineCount 0 — no evaluation ever runs, so no SNS action fires either.
Why: log-query alarms a client creates never alarm, never recover and never report an evaluation error; the configuration is accepted and then ignored.
Fix: the scheduled-query alarm evaluator in the cloudwatch crate (periodic evaluation, m-of-n state machine, missing-data treatment, query-role assumption errors and alarm actions).
Traces: the three checks in trace_ids.

## Evidence
Expected: ALARM after 7 BREACH lines; OK for a zero-match control; an evaluation-error outcome for an unassumable query role. Actual (all three): the alarm is still {'StateValue': 'INSUFFICIENT_DATA', 'StateReason': 'Unchecked: Initial alarm creation', 'ActionLogLineCount': 0} when the 240–300s budgets expire.

## Steps to Reproduce
Replay cloudwatch__logs__sns__sqs.deep-cloudwatch-2-cwlogalarm-m-of-n-breach-count-drives-alarm, .deep-cloudwatch-2-cwlogalarm-zero-match-count-is-missing-not-zero and .deep-cloudwatch-2-cwlogalarm-unassumable-query-role-evaluation-error: create the alarm with a ScheduledQueryConfiguration, ingest events and poll DescribeAlarms.

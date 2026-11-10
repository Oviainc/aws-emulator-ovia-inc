Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation|_await#10|Timeout|time.sleep(0.5)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# logs: a PPL Insights query over logGroups(...) with backticked fields never reaches a terminal state, so its aggregation is ungradeable (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Cause 1 of 1: a PPL query that filters on backticked field names never completes, so neither its aggregates nor the unbackticked contrast can be graded.
AWS: StartQuery accepts a PPL query of the form 'source = logGroups(...)' and the query progresses Scheduled -> Running -> Complete; GetQueryResults then returns the per-group buckets, where backticked `@logStream` and `status-code` select the fields by name while unbackticked status-code is parsed as the expression status - code and selects nothing. Recorded behavior only for the bucket values (the check computes them by hand). Smithy logs StartQuery/GetQueryResults: status is a QueryStatus enum including Scheduled, Running, Complete, Failed, Cancelled, Timeout.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'. The query never reaches Complete (nor Failed/Cancelled/Timeout), so GetQueryResults is never gradeable.
Why: a client polling GetQueryResults for a terminal status waits forever and never gets rows or an error, so PPL queries are unusable and indistinguishable from a hung service.
Fix: the logs Insights query engine's PPL parsing/execution and the query-status state machine behind StartQuery/GetQueryResults (a query it cannot run must end Failed with a refusal, never stay non-terminal).
Traces: logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: the query reaches status Complete (or a terminal Failed) and GetQueryResults returns the hand-computed per-svc buckets for the backticked form and no rows for the unbackticked form. Actual: no terminal query status inside the cap.

## Steps to Reproduce
Replay logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation: create a log group, seed events in two streams, StartQuery with a PPL query 'source = logGroups(...)' filtering on backticked `@logStream` and `status-code`, then poll GetQueryResults for a terminal status.

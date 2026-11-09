Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__stepfunctions.redrive-logs-executionredriven-and-second-executionfailed-across-levels
Issue-Key: logs__stepfunctions.redrive-logs-executionredriven-and-second-executionfailed-across-levels|run#7|AssertionError|assert len(failed_logs) == 2, (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A redriven Step Functions execution logs the wrong number of ExecutionFailed events

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RedriveExecution

## Description
Cause 1 of 1: the execution-history log of a redriven execution duplicates its failure records.
AWS: an execution that fails, is redriven and fails again writes exactly two ExecutionFailed records to its log group (one per attempt), alongside the ExecutionRedriven record, at every logging level that includes them (Smithy sfn RedriveExecution; LoggingConfiguration.level ALL/ERROR/FATAL).
Emulator: the group holds many more — the per-level counts read ['6', '6', '11'] where exactly two ExecutionFailed events are expected at each level, so the history is emitted repeatedly instead of once per attempt.
Why: a client counting failures from execution logs (alarms, metric filters, audit reports) over-counts by 3–5x for every redriven execution.
Fix: execution-history log emission for redrive in the stepfunctions crate (one record per attempt, per configured level).
Traces: logs__stepfunctions.redrive-logs-executionredriven-and-second-executionfailed-across-levels.

## Evidence
Expected: exactly two ExecutionFailed log events (per level). Actual: 'AssertionError: all: exactly two ExecutionFailed log events expected, got ['6', '6', '11']'.

## Steps to Reproduce
Replay logs__stepfunctions.redrive-logs-executionredriven-and-second-executionfailed-across-levels: run an execution that fails, call RedriveExecution, let it fail again and count ExecutionFailed records in the log group at each logging level.

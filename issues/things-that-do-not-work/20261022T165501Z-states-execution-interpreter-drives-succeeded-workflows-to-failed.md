Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-distributed-map-terminal-map-run
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-distributed-map-terminal-map-run sfn-execution-included-data-and-history-paging sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeExecution|2xx->2xx|$.output
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatus
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatusReason
Issue-Related: 20261022T120501Z-states-executions-of-valid-state-machines-terminate-failed-instead.md
Issue-Related: 20261022T165501Z-states-describe-responses-omit-the-type-discriminator-on.md
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# states execution interpreter drives SUCCEEDED workflows to FAILED and marks them REDRIVABLE

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Two recorded state machines that AWS runs to completion are driven to terminal FAILED by the emulator's execution interpreter, and the resulting terminal execution is additionally reported as REDRIVABLE with no redriveStatusReason. Related: the open issue 20261022T120501Z-states-executions-of-valid-state-machines-terminate-failed-instead.md describes the FAILED-instead-of-SUCCEEDED symptom and binds the first fixture below as its replay step, but its Issue-Trace-Ids are bare fixture ids with no step half, so the second cluster below (and the redrive-status half of both) is not bound to any replayable step by it; the redrive-field issue 20261022T120501Z-states-describeexecution-omits-redrive-fields-and-execution-data-details.md is RESOLVED and so covers no live failure. 2 causes.

Cause 1 of 2: a Distributed Map state machine ends FAILED with no output, and its terminal execution is mislabelled redrivable.
AWS: Smithy: DescribeExecution output status -> ExecutionStatus (required), output -> SensitiveData, outputDetails -> CloudWatchEventsExecutionDataDetails, redriveStatus -> ExecutionRedriveStatus [REDRIVABLE, NOT_REDRIVABLE, REDRIVABLE_BY_MAP_RUN], redriveStatusReason -> SensitiveData. Recorded behavior: the execution reaches status SUCCEEDED at 200 with output present, outputDetails.included true, redriveStatus NOT_REDRIVABLE and a redriveStatusReason explaining that a SUCCEEDED execution cannot be redriven (recorded example text: 'Execution is SUCCEEDED and cannot be redriven'; the requirement is that a non-redrivable terminal execution reports NOT_REDRIVABLE and carries a reason, not the exact sentence). The rule for redriveStatus is the one the service's own RedriveExecution refusal states: a SUCCEEDED execution is not redrivable.
Emulator: status 200 but $.status is FAILED on the first poll and stays FAILED (the replay poller aborts after 2 attempts because FAILED is terminal), $.output and $.outputDetails are absent, and $.redriveStatus is REDRIVABLE with $.redriveStatusReason absent.
Why: a client cannot execute a Map/Distributed-Map workflow at all on the emulator - every downstream step that reads the execution output gets nothing - and the redriveStatus it does return contradicts the service's own RedriveExecution refusal, so a client that branches on redriveStatus will attempt a redrive that can only fail.
Fix: the execution interpreter in the sfn service (the Distributed Map / Map Run execution path and the terminal status transition) plus the redriveStatus/redriveStatusReason derivation that the DescribeExecution handler applies to a terminal execution.
Traces: sfn-distributed-map-terminal-map-run#task#5 (cluster also covers sfn-execution-included-data-and-history-paging#task#5 and sfn-validation-reason-labeled-arn-and-completed-map-run#task#5).

Cause 2 of 2: the included-data / history-paging state machine also ends FAILED and reports REDRIVABLE.
AWS: Recorded behavior: the same execution reaches SUCCEEDED at 200 with redriveStatus NOT_REDRIVABLE and a redriveStatusReason present.
Emulator: status 200 with $.status FAILED (terminal, poller gives up after 2 attempts), $.redriveStatus REDRIVABLE and $.redriveStatusReason absent.
Why: a second, non-Map definition fails the same way, so the defect is not confined to Distributed Map; it also makes the fixture's downstream history/included-data assertions unreachable.
Fix: the same execution interpreter status transition and the same redrive-status derivation; this cluster is a distinct replay step and must be re-run separately from cause 1.
Traces: sfn-execution-included-data-and-history-paging#task#4.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, state_reset_failures 0), both clusters verdict=mismatch with expected and actual status BOTH 200:
- sfn-distributed-map-terminal-map-run#task#5 DescribeExecution: $.status value_mismatch (expected 'SUCCEEDED', got 'FAILED') and poll note "'FAILED' is in fail_on after 2 attempt(s)"; $.output missing (expected '[{"id":"a"},{"id":"b"}]'); $.outputDetails missing (expected {included:true}); $.redriveStatus value_mismatch (expected 'NOT_REDRIVABLE', got 'REDRIVABLE'); $.redriveStatusReason missing. affected_count 3.
- sfn-execution-included-data-and-history-paging#task#4 DescribeExecution: $.status value_mismatch (expected 'SUCCEEDED', got 'FAILED'), $.redriveStatus value_mismatch (expected 'NOT_REDRIVABLE', got 'REDRIVABLE'), $.redriveStatusReason missing. affected_count 1.
Smithy evidence: ExecutionRedriveStatus values [REDRIVABLE, NOT_REDRIVABLE, REDRIVABLE_BY_MAP_RUN]; RedriveExecution models ExecutionNotRedrivable (400) for an execution that cannot be redriven.

## Steps to Reproduce
1. Replay fixture sfn-distributed-map-terminal-map-run; at step task#5 DescribeExecution polls the execution started earlier in the fixture - the emulator answers 200 with status FAILED immediately, never produces output/outputDetails, and reports redriveStatus REDRIVABLE.
2. Replay fixture sfn-execution-included-data-and-history-paging; at step task#4 DescribeExecution polls for SUCCEEDED and sees FAILED with redriveStatus REDRIVABLE.
3. After the fix, re-replay BOTH steps (task#5 of the first fixture and task#4 of the second) - they are separate clusters; passing only the first leaves cause 2 unexercised.

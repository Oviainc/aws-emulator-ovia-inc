Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-distributed-map-terminal-map-run
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-distributed-map-terminal-map-run sfn-execution-included-data-and-history-paging sfn-validation-reason-labeled-arn-and-completed-map-run
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeExecution|2xx->2xx|$.outputDetails
Issue-Key: states|DescribeExecution|2xx->2xx|$.status
Issue-Related: 20261022T120501Z-states-describeexecution-omits-redrive-fields-and-execution-data-details.md
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Executions of valid state machines terminate FAILED instead of SUCCEEDED with no output

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Two recorded state machines that AWS runs to completion are driven to a terminal FAILED status by the emulator, so the polling step gives up and no output is ever produced. 2 causes.

Cause 1 of 2: a Distributed Map state machine fails instead of succeeding.
AWS: Smithy: DescribeExecution output status -> ExecutionStatus enum [RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED, PENDING_REDRIVE], with output -> SensitiveData and outputDetails -> CloudWatchEventsExecutionDataDetails. The recording shows the execution reaching SUCCEEDED with output '[{"id":"a"},{"id":"b"}]', outputDetails.included true, redriveStatus 'NOT_REDRIVABLE' and redriveStatusReason 'Execution is SUCCEEDED and cannot be redriven' (example text).
Emulator: status is FAILED on the first poll and stays FAILED (the replay poller aborts after 2 attempts because FAILED is terminal), and output/outputDetails/redriveStatus/redriveStatusReason are all absent.
Why: every client workflow built on a Map state sees a failed execution and no result, so the emulator cannot be used to exercise map/iteration behavior at all; downstream steps that read the output get nothing.
Fix: the execution interpreter in the sfn service (the Distributed Map / Map Run execution path and the execution status transition), not the serializer.
Traces: sfn-distributed-map-terminal-map-run#task#5 (also sfn-execution-included-data-and-history-paging#task#5, sfn-validation-reason-labeled-arn-and-completed-map-run#task#5).

Cause 2 of 2: the included-data/history-paging state machine also ends FAILED.
AWS: the recording shows the same execution reaching SUCCEEDED with redriveStatus 'NOT_REDRIVABLE' and redriveStatusReason 'Execution is SUCCEEDED and cannot be redriven'.
Emulator: status FAILED (terminal after 2 polls), redriveStatus/redriveStatusReason absent.
Why: as above - a valid definition cannot be executed successfully, which also makes the history and included-data assertions downstream unreachable.
Fix: the same execution interpreter path and status transition.
Traces: sfn-execution-included-data-and-history-paging#task#4.

## Evidence
- sfn-distributed-map-terminal-map-run#task#5 (status 200 both sides): expected $.status='SUCCEEDED', $.output='[{"id":"a"},{"id":"b"}]', $.outputDetails={included:true}, $.redriveStatus='NOT_REDRIVABLE'; actual $.status='FAILED' and output/outputDetails/redriveStatus/redriveStatusReason all None. Poller: "'FAILED' is in fail_on after 2 attempt(s)".
- sfn-execution-included-data-and-history-paging#task#4 (status 200 both sides): expected $.status='SUCCEEDED', redriveStatus='NOT_REDRIVABLE'; actual $.status='FAILED'.

## Steps to Reproduce
1. Replay fixture sfn-distributed-map-terminal-map-run; at step task#5 DescribeExecution polls the execution started earlier in the fixture -> the emulator reports status FAILED immediately and never produces output.
2. Replay fixture sfn-execution-included-data-and-history-paging; step task#4 polls DescribeExecution for SUCCEEDED and sees FAILED.

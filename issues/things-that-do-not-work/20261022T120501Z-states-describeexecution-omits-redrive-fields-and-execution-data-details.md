Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-start-execution-idempotent-while-running
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-start-execution-idempotent-while-running sfn-state-machine-deleting-rejections stepfunctions-redrive-execution-error-paths stepfunctions-standard-execution-start-monitor-stop states-execution-failure-and-redrive states-execution-lifecycle-success-and-abort states-standard-execution-lifecycle
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeExecution|2xx->2xx|$.inputDetails.__type
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatus
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveStatusReason
Issue-Key: states|DescribeExecution|2xx->2xx|$.outputDetails.__type
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveCount
Issue-Key: states|DescribeExecution|2xx->2xx|$.redriveDate
Issue-Key: states|DescribeExecution|2xx->2xx|$.output
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# DescribeExecution omits redrive fields and execution-data details and never records a redrive

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
DescribeExecution answers HTTP 200 but its body is missing the whole redrive projection and the execution-data detail structures, for every execution state, and a redrive is not recorded on the execution at all. 5 causes.

Cause 1 of 5: redrive projection absent for a RUNNING execution.
AWS: DescribeExecution output models redriveStatus (Smithy: DescribeExecution output member redriveStatus -> ExecutionRedriveStatus enum [REDRIVABLE, NOT_REDRIVABLE, REDRIVABLE_BY_MAP_RUN]), redriveStatusReason (SensitiveData, length 0..262144) and inputDetails/outputDetails -> CloudWatchEventsExecutionDataDetails {included: boolean, default false}. The recording shows a running, never-redriven execution answering redriveStatus 'NOT_REDRIVABLE' with redriveStatusReason 'Execution is RUNNING and cannot be redriven' (example text; the rule is that a non-terminal execution is not redrivable), and every nested structure carrying the awsJson1_0 type discriminator (inputDetails.__type = 'com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails').
Emulator: status/startDate/executionArn match, but inputDetails.__type, redriveStatus and redriveStatusReason are all absent from the body.
Why: a client that reads redriveStatus to decide whether RedriveExecution is callable sees null and cannot distinguish 'not redrivable' from 'field unsupported'; an SDK that keys nested structure deserialization off __type does not populate inputDetails.
Fix: the DescribeExecution response projection/serializer in the sfn service (redrive fields plus the nested-structure __type discriminator).
Traces: sfn-start-execution-idempotent-while-running#task#7 (also sfn-state-machine-deleting-rejections#task#10, stepfunctions-redrive-execution-error-paths#task#4, stepfunctions-standard-execution-start-monitor-stop#task#4).

Cause 2 of 5: redriveStatus absent for a terminal FAILED execution that AWS reports as REDRIVABLE.
AWS: a failed standard execution within the redrive window answers redriveStatus 'REDRIVABLE' with no redriveStatusReason (Smithy: ExecutionRedriveStatus).
Emulator: redriveStatus and inputDetails.__type absent.
Why: clients cannot discover that the failed execution can be redriven, so redrive flows are untestable against the emulator.
Fix: same response projection, driven off the execution's terminal status.
Traces: sfn-start-execution-idempotent-while-running#task#10 (also states-execution-failure-and-redrive#task#7, states-execution-lifecycle-success-and-abort#task#14, stepfunctions-standard-execution-start-monitor-stop#task#11).

Cause 3 of 5: redrive projection and outputDetails absent for a SUCCEEDED execution.
AWS: a succeeded execution answers redriveStatus 'NOT_REDRIVABLE' with redriveStatusReason 'Execution is SUCCEEDED and cannot be redriven' (example text; rule: a succeeded execution is not redrivable) and carries both inputDetails and outputDetails with the CloudWatchEventsExecutionDataDetails discriminator.
Emulator: inputDetails.__type, outputDetails.__type, redriveStatus and redriveStatusReason all absent.
Fix: same response projection.
Traces: states-execution-failure-and-redrive#task#14 (also states-execution-lifecycle-success-and-abort#task#6).

Cause 4 of 5: a performed redrive is not recorded on the execution.
AWS: Smithy: DescribeExecution output redriveCount -> RedriveCount (integer) and redriveDate -> Timestamp. After RedriveExecution succeeds, the recording shows redriveCount 1 and a populated redriveDate, with redriveStatus 'REDRIVABLE' again.
Emulator: redriveCount stays 0, redriveDate is absent, redriveStatus is absent.
Why: redrive is invisible in state, so clients cannot tell a redriven execution from a fresh one or bound retries on redriveCount.
Fix: the RedriveExecution handler's execution state mutation plus the DescribeExecution projection.
Traces: states-execution-failure-and-redrive#task#10.

Cause 5 of 5: succeeded-execution body misses the same fields and re-serializes output.
AWS: output is echoed as the document the state machine produced; the recording shows '{"probe": true}' (recorded behavior only for the inter-token spacing), with inputDetails/outputDetails discriminators and the NOT_REDRIVABLE projection.
Emulator: inputDetails.__type, outputDetails.__type, redriveStatus and redriveStatusReason absent, and output is re-serialized compactly as '{"probe":true}'.
Why: the missing fields are the defect above; the re-serialization means a client cannot byte-compare the output it round-tripped.
Fix: the DescribeExecution projection, and keeping the execution output as the stored document rather than re-encoding it.
Traces: states-standard-execution-lifecycle#task#4.

## Evidence
Expected vs actual (status 200 in every case):
- sfn-start-execution-idempotent-while-running#task#7: expected inputDetails.__type='com.amazonaws.swf.base.model#CloudWatchEventsExecutionDataDetails', redriveStatus='NOT_REDRIVABLE', redriveStatusReason='Execution is RUNNING and cannot be redriven'; actual: all three absent (None).
- sfn-start-execution-idempotent-while-running#task#10: expected redriveStatus='REDRIVABLE', inputDetails.__type set; actual: both absent.
- states-execution-failure-and-redrive#task#14: expected inputDetails.__type, outputDetails.__type, redriveStatus='NOT_REDRIVABLE', redriveStatusReason='Execution is SUCCEEDED and cannot be redriven'; actual: all absent.
- states-execution-failure-and-redrive#task#10: expected redriveCount=1, redriveDate present, redriveStatus='REDRIVABLE'; actual redriveCount=0, redriveDate absent, redriveStatus absent.
- states-standard-execution-lifecycle#task#4: expected output='{"probe": true}' plus the four fields above; actual output='{"probe":true}', fields absent.

## Steps to Reproduce
1. Replay fixture sfn-start-execution-idempotent-while-running and inspect step task#7: StartExecution on a standard state machine, then DescribeExecution while it is RUNNING -> body has no redriveStatus/redriveStatusReason and inputDetails carries no __type.
2. Replay states-execution-failure-and-redrive step task#10: let an execution FAIL, call RedriveExecution, then DescribeExecution -> redriveCount is still 0 and redriveDate/redriveStatus are absent.
3. Replay states-standard-execution-lifecycle step task#4: DescribeExecution on a succeeded execution -> output is compacted and the four fields above are missing.

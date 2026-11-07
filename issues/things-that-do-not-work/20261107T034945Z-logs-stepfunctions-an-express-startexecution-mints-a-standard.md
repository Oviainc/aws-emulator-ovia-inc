Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events logs__stepfunctions.xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution logs__stepfunctions.xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it logs__stepfunctions.xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload
Issue-Key: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events|run#16|AssertionError|assert f":express:{sm_name}:" in exec_arn, (
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution|run#8|AssertionError|assert ":express:" in resp["executionArn"], (
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it|run#10|AssertionError|assert ":express:" in arn, f"expected an express execution ARN, got {arn!r}"
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload|_await#10|Timeout|time.sleep(_POLL_S)
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# logs__stepfunctions: an EXPRESS StartExecution mints a standard execution ARN, and an EXPRESS run's vended log events never arrive (2 causes)

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two root causes behind the five express-logging checks of the logs__stepfunctions suite; the four failures below are the ones the replay report does not mark ALREADY FILED or OWNED.

Cause 1 of 2 — StartExecution on an EXPRESS state machine returns an `execution` ARN instead of an `express` ARN.
AWS: Smithy: StartExecution.output.executionArn -> Arn (required). For an EXPRESS state machine AWS mints arn:aws:states:<region>:<account>:express:<stateMachineName>:<executionName>:<invocationUuid> — resource type `express`, with a trailing invocation UUID segment. That resource type is what makes DescribeExecution and GetExecutionHistory refuse the ARN with InvalidArn ("Invalid Arn: 'Resource type not valid in this context: express'"), which is why CloudWatch Logs is the only record of an EXPRESS run.
Emulator: returns arn:aws:states:<region>:<account>:execution:<stateMachineName>:<executionName-uuid> — resource type `execution`.
Why: a client that reads the resource type to learn it must go to logs (rather than to the history API) is told the wrong thing, and every subsequent call on the ARN takes the STANDARD path instead of being refused.
Fix: the StartExecution handler's ARN construction, keyed on the state machine's type.
Related OPEN issue: 20261103T052008Z-states-express-executions-get-a-standard-execution-arn-so.md reports the same minting defect from the wire corpus (service `states`) and does not carry these behavioral checks; it does not cover them.
Traces: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events, logs__stepfunctions.xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution, logs__stepfunctions.xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it

Cause 2 of 2 — an EXPRESS execution delivers no vended CloudWatch Logs events, so includeExecutionData cannot be observed.
AWS: recorded behavior (cw-logs.html): an EXPRESS state machine with loggingConfiguration level=ALL and a vended destination (/aws/vendedlogs/states/...) delivers the whole execution history to that log group — ExecutionStarted, the state-transition events, ExecutionSucceeded/ExecutionFailed — and with includeExecutionData=false the SAME (id, type) sequence is delivered with input, inputDetails, output, outputDetails and parameters stripped. Delivery happens for every run, not only the first.
Emulator: the check polls the twin log groups (includeExecutionData true and false) for the complete event sequence and never sees it; the replay ends on its budget with nothing delivered.
Why: for EXPRESS workflows the log group is the only record of a run, so an operator has no history at all, and the redaction contract (true vs false) is unobservable.
Fix: the EXPRESS execution path's vended-log delivery (the log-delivery writer that the loggingConfiguration selects).
Related OPEN issue: 20261104T035532Z-logs-stepfunctions-express-executions-deliver-no-vended-cloudwatch.md describes the same symptom and owns a different check of this suite; it does not carry this check, so this cause is filed with it named.
Traces: logs__stepfunctions.xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload

## Evidence
From this replay of simparity-behavioral-aws-v1:

Cause 1 (three checks, same assertion): AssertionError: an EXPRESS machine must mint an express execution ARN; got 'arn:aws:states:us-west-2:<account>:execution:<state-machine-name>:<uuid>'. Expected the ARN to contain ':express:<stateMachineName>:'.

Cause 2: the check timed out waiting for "the full/redacted arm's complete event sequence" in the two vended log groups — no ALL-level events for the EXPRESS executions ever appeared, so the (id, type) comparison between the includeExecutionData=true and =false arms never ran.

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com and a log group under /aws/vendedlogs/states/.
2. CreateStateMachine with type=EXPRESS and loggingConfiguration {level: ALL, includeExecutionData: true, destinations: [that log group ARN]}.
3. Call StartExecution and inspect the returned executionArn: its resource-type segment must be `express` (cause 1).
4. Call DescribeExecution / GetExecutionHistory on that ARN: AWS refuses both with InvalidArn "Invalid Arn: 'Resource type not valid in this context: express'".
5. Poll the log group with FilterLogEvents for that execution's ExecutionStarted -> TaskStateEntered -> ExecutionSucceeded sequence (cause 2); repeat against a twin state machine with includeExecutionData=false and compare the (id, type) sequences.

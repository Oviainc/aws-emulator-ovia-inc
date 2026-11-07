Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution
Issue-Key: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution|_probe#8|Timeout|return sfn.list_executions(stateMachineArn=sm_arn, maxResults=5).get(
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# pipes__sqs__stepfunctions: a pipe with an SQS source and a Step Functions target never starts an execution

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: messages sent to a RUNNING pipe's SQS source are never delivered to its Step Functions state-machine target, so no execution is ever started and the check polling for one runs to the harness deadline.
  AWS: recorded behavior only for the delivery (EventBridge Pipes' Smithy model declares CreatePipe/DescribePipe and the PipeState enum, not the data plane). The recording shows that once the pipe reports RUNNING, a message put on the source queue starts exactly one execution of the target state machine whose input is the SQS source RECORD — body, messageId and eventSourceARN — not a re-wrapped envelope, and that execution is listed/describable promptly.
  Emulator: 'Check timed out after 180.0s' — the pipe accepts the source and target and the check never observes an execution, so neither the delivery nor the record shape can be graded.
  Why: a pipe is a managed, codeless integration; a client that routes a queue into a workflow through Pipes sees messages silently disappear — no execution, no failure, no dead-letter — and nothing in the API reports the gap.
  Note: open issue 20261107T205659Z-pipes-sqs-stepfunctions-a-running-pipe-with-an.md describes the same symptom for this service but does not carry this check's replay key (the suite did not report this failure as owned), so it is filed here; the two should be reconciled rather than fixed twice.
  Fix: the pipes service crate's source poller/target dispatcher — an SQS source must be polled while the pipe is RUNNING and each record handed to the Step Functions StartExecution target with the source record as the execution input.
  Traces: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution.

## Evidence
Error: Check timed out after 180.0s

The check creates a source queue, a Pass-only state machine (so the target needs no permissions), a pipes.amazonaws.com role granted sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes plus states:StartExecution, creates the pipe, waits for RUNNING and sends a message. Expected: one execution of the target state machine whose input carries the source record's body, messageId and eventSourceARN. Observed: no execution ever appears.

## Steps to Reproduce
1. Create an SQS queue and a state machine whose only state is a Pass.
2. Create a role trusted by pipes.amazonaws.com allowing sqs:ReceiveMessage, sqs:DeleteMessage, sqs:GetQueueAttributes on the queue and states:StartExecution on the state machine.
3. CreatePipe with that source, target and role; poll DescribePipe until CurrentState is RUNNING.
4. SendMessage to the source queue, then poll states:ListExecutions/DescribeExecution on the target — AWS shows one execution whose input is the SQS record; the emulator shows none.

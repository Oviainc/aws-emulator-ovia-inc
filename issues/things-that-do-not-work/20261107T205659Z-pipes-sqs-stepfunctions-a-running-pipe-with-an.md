Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution
Issue-Key: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution|_poll_until#8|Timeout|last = probe()
Issue-Commit: bd7849fb6f4a3f25191f75f48a6699331b4cc5b4

# pipes__sqs__stepfunctions: a RUNNING pipe with an SQS source and a Step Functions state-machine target never starts an execution (check hangs to the harness cap)

> _Note: this issue was identified against the code base at commit `bd7849fb6f4a3f25191f75f48a6699331b4cc5b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: a pipe whose Target is a Step Functions state-machine ARN, created with TargetParameters.StepFunctionStateMachineParameters.InvocationType=FIRE_AND_FORGET and SourceParameters.SqsQueueParameters.BatchSize=1, reaches CurrentState RUNNING but never polls its SQS source and never calls states:StartExecution on the target, so no execution is ever created.

AWS: Smithy (services/service-com-amazonaws-pipes-pipes) models PipeTargetParameters.StepFunctionStateMachineParameters -> PipeTargetStateMachineParameters.InvocationType (PipeTargetInvocationType); its documentation states FIRE_AND_FORGET invokes the state machine asynchronously via the Step Functions StartExecution API, and that REQUEST_RESPONSE (StartSyncExecution) is not supported for STANDARD state machines. So for a RUNNING pipe with an SQS source and a STANDARD state-machine target, AWS polls the source queue, and for every batch it delivers it issues StartExecution against the target state machine using the pipe's RoleArn. The execution input is the JSON array of source records, one element per message at BatchSize=1, each element being the SQS record envelope with the source message's body (an unmodified string when no InputTemplate is configured), messageId equal to the MessageId that SendMessage returned, and eventSourceARN equal to the source queue's ARN. The delivered message is then deleted from the source queue. Pipes also validates the target parameters at CreatePipe time (an InvocationType that is invalid for the target type is a create-time ValidationException), so a pipe that reaches RUNNING is one AWS would deliver through.

Emulator: CreatePipe accepts the pipe and reports it RUNNING, and the SQS SendMessage into the source queue succeeds, but no execution ever appears for the target state machine. The check polls ListExecutions for the state machine and never observes one; it is killed by the suite's 180s cap and reported as [mismatch] "Check timed out after 180.0s". The emulator therefore stores the Target/TargetParameters configuration and echoes the pipe's state but has no runtime that consumes the source and invokes the Step Functions target. Nothing is served for the SFN-target path at all — there is no wrong status or error code to quote, the StartExecution call is simply never made.

Why: a client that wires SQS -> Pipes -> Step Functions (the standard "queue drives a workflow" pattern) sees a healthy RUNNING pipe and a message that is accepted, yet no execution starts and the message is never consumed from the source queue. Any test or application that waits on an execution, on the pipe-supplied execution input (records[].body / messageId / eventSourceARN), or on the source queue draining, blocks forever instead of progressing, and no error anywhere points at the pipe.

Fix: the Pipes runtime/poller component — the part that, for a RUNNING pipe, reads batches from the configured source and dispatches them to the configured target — needs a Step Functions state-machine target dispatcher that honours StepFunctionStateMachineParameters.InvocationType (FIRE_AND_FORGET -> StartExecution), builds the execution input as the list of SQS source-record envelopes (body kept as the source string when no InputTemplate is set, messageId, eventSourceARN), assumes/uses the pipe's RoleArn, respects SqsQueueParameters.BatchSize for the number of records per execution input, and deletes delivered messages from the source queue. Related already-open issues cover other target types and the general absence of a poller (20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md; 20261107T175408Z-pipes-3-net-new-causes-a-running-pipe.md covers the SQS and EventBridge event-bus targets; 20261107T175408Z-lambda-pipes-sqs-2-net-new-causes-a-0e28d4.md covers the Lambda target) — none of them names the Step Functions state-machine target, which is this cause.

Traces: pipes__sqs__stepfunctions.pipes-sqs-source-starts-sfn-target-execution.

## Evidence
Expected (AWS): after SendMessage into the pipe's source queue, a Step Functions execution exists for the target state machine; it reaches status SUCCEEDED for the Pass-state definition {"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}; DescribeExecution.input parses to a one-element JSON array (BatchSize=1); that element has messageId == the MessageId returned by SendMessage, eventSourceARN == the source queue ARN, and body == the exact body string that was sent; and DescribeExecution.output equals the input (Pass echoes it).

Actual (emulator): no execution is ever created for the target state machine. ListExecutions for the state machine stays empty while the check polls it, so the first assertion is never reached and the check is terminated by the harness: verdict [mismatch], error "Check timed out after 180.0s". The pipe meanwhile reports CurrentState RUNNING, and the sent message is not consumed from the source queue.

## Steps to Reproduce
1. Create an SQS queue Q (source).
2. Create a STANDARD Step Functions state machine SM with definition {"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}} and any execution role.
3. Create an IAM role R trusting pipes.amazonaws.com with an inline policy allowing sqs:ReceiveMessage/sqs:DeleteMessage/sqs:GetQueueAttributes on Q's ARN and states:StartExecution on SM's ARN.
4. CreatePipe with Source=Q's ARN, Target=SM's ARN, RoleArn=R's ARN, SourceParameters={"SqsQueueParameters":{"BatchSize":1}}, TargetParameters={"StepFunctionStateMachineParameters":{"InvocationType":"FIRE_AND_FORGET"}}; wait until DescribePipe reports CurrentState=RUNNING.
5. sqs:SendMessage to Q with body {"marker":"<unique>"}.
6. Poll stepfunctions:ListExecutions(stateMachineArn=SM) — on AWS an execution appears and succeeds; on the emulator the list stays empty until the check's 180s cap, and the message stays on Q.

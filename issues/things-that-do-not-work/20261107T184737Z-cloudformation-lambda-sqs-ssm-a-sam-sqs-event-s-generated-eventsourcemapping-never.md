Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes
Issue-Key: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes|_poll#32|AssertionError|raise AssertionError(f"{what} not reached within {timeout_s}s; last {last!r}")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# A SAM SQS event's generated EventSourceMapping never consumes: the sent message is never processed and the queue never drains

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (AWS::Serverless::Function SQS event -> AWS::Lambda::EventSourceMapping)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: the EventSourceMapping that the SAM transform generates from a function's SQS event never consumes from the queue — a message sent after the stack is deployed is never handed to the function, so the handler's effect never appears and the queue never drains.
Failing check ids for this cause: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes.

AWS: recorded behavior only for the consumption timing; the surface is the SAM SQS event property (which expands to an AWS::Lambda::EventSourceMapping plus the SQS poller policy on the generated role) together with Lambda CreateEventSourceMapping, SQS SendMessage and SSM PutParameter/GetParameter. Reference: https://docs.aws.amazon.com/serverless-application-model/latest/developerguide/sam-property-function-sqs.html. The recording shows that after the stack reaches CREATE_COMPLETE a SendMessage to the queue is consumed within a short bounded wait: the function writes the message body to its SSM parameter (so GetParameter returns that body) and the queue drains to zero.
Emulator: the handler's effect never appears — the check polls SSM for 120 s and reports "the function writing the message body to SSM not reached within 120.0s; last None", i.e. GetParameter never resolves the marker parameter because the function was never invoked for the message.
Why: the SAM SQS event — the declarative way to attach a queue consumer — produces a mapping that does not poll. A deployed application silently accumulates messages instead of processing them, and the generated role's poller policy cannot be distinguished from a mapping that never runs.
Fix: the Lambda event-source-mapping poller for mappings created through CloudFormation/SAM (AWS::Lambda::EventSourceMapping over an SQS source) — poll the mapped queue, invoke the function with the Records batch and delete the consumed records, so the queue drains and the handler's side effects land.
Traces: cloudformation__lambda__sqs__ssm.sam-sqs-event-creates-esm-that-consumes.

The open issue 20261106T153504Z-cloudformation-lambda-sqs-ssm-a-sam-sqs-event.md carries the replay key of sam-sqs-event-role-without-queue-access-fails-the-mapping-create only; it does not carry this check's key.

## Evidence
Expected (recorded AWS): after CREATE_COMPLETE, SendMessage to the queue is consumed; GetParameter on the marker returns the message body and the queue drains to zero.

Actual (emulator): AssertionError: the function writing the message body to SSM not reached within 120.0s; last None

## Steps to Reproduce
1. CreateStack (CAPABILITY_IAM, CAPABILITY_AUTO_EXPAND) with a SAM template whose AWS::Serverless::Function declares an SQS event on a queue in the same stack, the handler writing the record body to an SSM parameter.
2. Wait for CREATE_COMPLETE and confirm the generated AWS::Lambda::EventSourceMapping exists.
3. SendMessage to the queue.
4. Poll SSM GetParameter for the marker parameter.
5. Observe it never resolves and the queue does not drain.
Or replay the check id in trace_ids.

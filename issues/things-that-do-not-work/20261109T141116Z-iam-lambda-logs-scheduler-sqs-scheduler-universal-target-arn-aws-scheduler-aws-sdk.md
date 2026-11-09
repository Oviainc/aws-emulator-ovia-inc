Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters
Issue-Key: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters|_await_message#27|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler universal target arn:aws:scheduler:::aws-sdk:lambda:invoke never invokes the function

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence/dispatch component's handling of UNIVERSAL targets.

Cause 1 of 1: a schedule whose Target.Arn is arn:aws:scheduler:::aws-sdk:lambda:invoke is accepted and reads back correctly but never issues the lambda:Invoke call, so a correctly-formed universal target delivers nothing.
AWS: Smithy (com.amazonaws.scheduler) — the Target shape documents that Scheduler supports "universal targets that you can customize to invoke over 6,000 API operations across more than 270 services"; Target.Arn is required (@length 1..1600) and Target.Input is "The text, or well-formed JSON, passed to the target". For a universal target the Input IS the request body of the named SDK operation, so arn:aws:scheduler:::aws-sdk:lambda:invoke with an Input carrying FunctionName and Payload must cause Scheduler, under Target.RoleArn, to call lambda:Invoke on that function with that payload at every occurrence (recorded behavior: the function runs and its own log/queue side-effect is observable within the delivery window).
Emulator: nothing arrives from the correctly-formed universal target within 225 s. The check's subject is the SIBLING case — a universal target whose Input is invalid for the operation must dead-letter — but it fails on this positive control first, so the emulator never issues the SDK call for a universal Lambda target at all.
Why: a client that schedules a Lambda through a universal target (rather than the templated Lambda target) sees CreateSchedule succeed and GetSchedule report State=ENABLED while the function is never invoked. The work silently never runs and nothing is reported. It also means the malformed-input dead-letter path for universal targets is entirely unexercised.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/). It must parse Target.Arn of the form arn:aws:scheduler:::aws-sdk:<service>:<operation>, dispatch to that emulated service operation with Target.Input as the request body under Target.RoleArn, and treat a request the operation rejects as a failed delivery subject to RetryPolicy and DeadLetterConfig.
Traces: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters.

## Evidence
Replay of simparity-behavioral-aws-v1: iam__lambda__logs__scheduler__sqs 0/1.

[mismatch] iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters
AssertionError: the correctly-formed sibling target invoking the function: nothing arrived within 225s

Expected: the sibling schedule whose Target.Arn is arn:aws:scheduler:::aws-sdk:lambda:invoke and whose Target.Input is a valid lambda:Invoke request invokes the function at each occurrence, observable through the function's side-effect.
Actual: no invocation at all within 225 s, so the check cannot reach its subject (the malformed-Input twin dead-lettering).

## Steps to Reproduce
1. lambda.create_function for a function with an observable side-effect (e.g. it writes to an SQS queue or to CloudWatch Logs); sqs.create_queue for the dead-letter queue.
2. iam.create_role trusting scheduler.amazonaws.com with inline lambda:InvokeFunction on the function and sqs:SendMessage on the DLQ.
3. scheduler.create_schedule(Name=n, ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn':'arn:aws:scheduler:::aws-sdk:lambda:invoke', 'RoleArn':role_arn, 'Input': json.dumps({'FunctionName': <name>, 'Payload': json.dumps({...})}), 'DeadLetterConfig': {'Arn': dlq_arn}}).
4. Observe the function's side-effect past the first occurrence. On AWS the function runs. On the emulator nothing is observed (within 225 s).
Check: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters.

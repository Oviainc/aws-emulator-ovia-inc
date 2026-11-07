Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer|run#20|Timeout|exec_arn = sfn.start_execution(
Issue-Commit: dcaafa445ca563c5947bd284ba4541134d1f4f9e

# stepfunctions: the events:putEvents.waitForTaskToken optimized integration never completes — the execution stays RUNNING after the worker's SendTaskSuccess

> _Note: this issue was identified against the code base at commit `dcaafa445ca563c5947bd284ba4541134d1f4f9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a state machine whose single Task uses Resource 'arn:aws:states:::events:putEvents.waitForTaskToken' never reaches SUCCEEDED. The execution is started, but no attempt of the round trip SFN -> EventBridge PutEvents -> rule -> Lambda worker -> SendTaskSuccess -> execution output ever settles, and the check is cut off by the harness bound (60s) while every DescribeExecution still reports RUNNING. Related but NOT the same filing: 20261101T225826Z-events-sfn-sqs-the-events-putevents-waitfortaskToken-producer.md reports the same round trip under service token events__sfn__sqs, and 20261107T044335Z-lambda-sqs-stepfunctions-waitfortasktoken-callbacks-never-resume-the.md reports the sqs:sendMessage.waitForTaskToken variant; this check is listed under events__sqs__stepfunctions and was not marked owned by either.

AWS: recorded behavior only — the .waitForTaskToken callback pattern is not described in the Smithy model (the optimized service integration is a Resource ARN interpreted by Step Functions, not a modeled member). On AWS: StartExecution on such a machine publishes one EventBridge entry to the named bus with Source/DetailType from Parameters and Detail carrying the substituted context value for '$$.Task.Token' (a real, >50-character task token, never the literal path), the event envelope is attributed to the caller's account and region; a rule matching source + detail-type delivers that envelope to its targets; when a target calls SendTaskSuccess(taskToken, output) the Task state completes and DescribeExecution returns status SUCCEEDED with output exactly the string passed to SendTaskSuccess; a second SendTaskSuccess with the same, now-spent token is refused with error code TaskTimedOut. Values such as the bus name, job id, event id and token are per-run and are not part of the requirement; the state transition and the terminal output are.

Emulator: no execution of the machine ever leaves RUNNING, so none of the above is observable: the check is terminated by the 60s harness bound with no execution having reached SUCCEEDED. Nothing resumes the Task when the token is honoured, so the published event, the token substitution, the execution output and the spent-token refusal are all unverifiable.

Why: every callback-style workflow is permanently stuck. A client that models human approval, a worker queue or any cross-service hand-off with .waitForTaskToken sees its execution hang until TimeoutSeconds expires and then fail with States.Timeout, instead of completing with the worker's payload. Orchestration tests and deployment gates built on this pattern never finish.

Fix: the Step Functions interpreter's task-token/callback plane plus the events:putEvents optimized integration — the Task must publish the entry through EventBridge PutEvents with '$$.Task.Token' substituted into Detail, register the token against the paused Task, resume the state on SendTaskSuccess with the supplied output as the execution result, and refuse a re-use of a spent token with TaskTimedOut.

Traces: events__sqs__stepfunctions.xsvc-sfn-2-waittoken-producer.

## Evidence
Expected (AWS): the state machine's Publish Task (Resource 'arn:aws:states:::events:putEvents.waitForTaskToken', TimeoutSeconds 240) publishes one event; DescribeExecution eventually returns status 'SUCCEEDED' and output == json.dumps({'worker': ..., 'job': <the launch's job id>, 'eventId': <published event's id>, 'detailType': 'task.requested', 'source': 'sim.sfn'}, sort_keys=True); detail.taskToken is a substituted token string (>50 chars, contains no '$$'); a second SendTaskSuccess with that token fails with Error.Code == 'TaskTimedOut'.

Actual (emulator): Check timed out after 60.0s — no execution reached SUCCEEDED; DescribeExecution reports RUNNING for the whole window, so the assertion "no execution of the waitForTaskToken machine reached SUCCEEDED" is what the check is driving at when the bound cuts it off.

## Steps to Reproduce
1. Create a custom EventBridge bus and two rules on it: one matching {source:['sim.sfn'], detail-type:['task.requested'], detail:{nonce:[N]}} and a near-miss on detail-type 'task.requested.x'.
2. Target the matching rule at an SQS control queue and at a Lambda worker that reads event['detail']['taskToken'] and calls stepfunctions.send_task_success(taskToken=..., output=json.dumps({...})).
3. Create a role trusted by states.amazonaws.com with events:PutEvents on the bus ARN.
4. Create a state machine with one Task: Resource 'arn:aws:states:::events:putEvents.waitForTaskToken', Parameters.Entries[0] = {EventBusName: <bus arn>, Source: 'sim.sfn', DetailType: 'task.requested', Detail: {'taskToken.$': '$$.Task.Token', 'job.$': '$.job', 'nonce': N}}, TimeoutSeconds 240, End true.
5. StartExecution with input {"job": "job-N-0"} and poll DescribeExecution.
6. Observe the execution never leaves RUNNING; on AWS it reaches SUCCEEDED with the worker's payload as output within seconds.

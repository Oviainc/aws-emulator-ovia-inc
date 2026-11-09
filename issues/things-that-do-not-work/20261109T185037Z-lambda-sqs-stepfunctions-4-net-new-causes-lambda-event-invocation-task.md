Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-lambda-task-event-invocation-returns-202-and-the-function-delivers-to-sqs lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative lambda__sqs__stepfunctions.sfn-task-token-heartbeat-timeout-fails-execution lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-task-event-invocation-returns-202-and-the-function-delivers-to-sqs|run#21|AssertionError|assert desc["status"] == "SUCCEEDED", f"Expected SUCCEEDED, got {desc['status']}: {desc.get('cause')!r}"
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|run#15|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-heartbeat-timeout-fails-execution|run#14|AssertionError|assert elapsed_ok >= 5, f"succeeded too fast to have required the heartbeat: {elapsed_ok}s"
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", f"expected SUCCEEDED, got {desc['status']}: {desc.get('error')}"
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 4 net-new causes: Lambda Event-invocation task output, task-token timeout message, and heartbeat handling in Step Functions

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Four distinct root causes in the lambda__sqs__stepfunctions suite. Each is listed with its own failing check ids; all four must be closed.

Cause 1 of 4 — a lambda:invoke task with InvocationType 'Event' returns a result the state's ResultSelector cannot read. AWS: the optimized lambda:invoke integration with InvocationType='Event' returns {"StatusCode":202,"Payload":null,"SdkHttpMetadata":...,"SdkResponseMetadata":...} — the Payload member is present (null) so '$.Payload' resolves, and the execution SUCCEEDs while the function runs asynchronously and delivers to SQS. Emulator: the task output is {"StatusCode":202,"SdkHttpMetadata":{...}} with no Payload member, so the state machine fails with 'The JSONPath '$.Payload' specified for the field 'payload.$' could not be found in the input'. Why: a state machine that reads $.Payload after an asynchronous invoke — the documented shape — fails outright. Fix: the lambda:invoke task-integration result serializer (include Payload for InvocationType='Event'). Traces: lambda__sqs__stepfunctions.sfn-lambda-task-event-invocation-returns-202-and-the-function-delivers-to-sqs.

Cause 2 of 4 — the duplicate-SendTaskSuccess refusal carries the wrong message. AWS: calling SendTaskSuccess a second time for a token that has already completed fails the task with the fixed text "Task Timed Out: 'Provided task does not exist anymore'". Emulator: "Task Timed Out: 'arn:aws:states:...:execution:<sm>:<exec>'" — it interpolates the execution ARN where AWS states the fixed reason. Why: a client matching the documented cause string cannot recognise a duplicate/expired token, and the emulator's message leaks an identifier instead of the reason. Fix: the SendTaskSuccess/SendTaskFailure duplicate-token path's cause text. Traces: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative.

Cause 3 of 4 — HeartbeatSeconds is not enforced: a waitForTaskToken task that sends no heartbeat still succeeds. AWS: a .waitForTaskToken task with HeartbeatSeconds fails with States.Timeout when no SendTaskHeartbeat arrives inside the window. Emulator: the execution 'succeeded too fast to have required the heartbeat: 0.5s' — the heartbeat requirement is ignored. Why: a worker that silently dies looks healthy; the liveness guarantee HeartbeatSeconds exists for does not exist. Fix: heartbeat tracking/expiry in the Step Functions task-token state. Traces: lambda__sqs__stepfunctions.sfn-task-token-heartbeat-timeout-fails-execution.

Cause 4 of 4 — SendTaskHeartbeat does not keep a task alive. AWS: each SendTaskHeartbeat resets the heartbeat window, so a worker that heartbeats and then answers SendTaskSuccess makes the execution SUCCEED. Emulator: the execution FAILS with States.Timeout despite the heartbeats. Why: the mirror image of Cause 3 — long-running callbacks cannot be used at all, since heartbeating neither is required nor helps. Fix: the SendTaskHeartbeat handler must reset the task's heartbeat deadline. Traces: lambda__sqs__stepfunctions.sfn-task-token-sendtaskheartbeat-keeps-execution-alive.

## Evidence
Cause 1: AssertionError: Expected SUCCEEDED, got FAILED: 'The JSONPath \'$.Payload\' specified for the field \'payload.$\' could not be found in the input \'{"StatusCode":202,"SdkHttpMetadata":{"AllHttpHeaders":{"content-type":["applica…\''
Cause 2: AssertionError: expected message "Task Timed Out: 'Provided task does not exist anymore'", got "Task Timed Out: 'arn:aws:states:us-west-2:000000000000:execution:sim2real-sm-...:sim2real-exec-...'"
Cause 3: AssertionError: succeeded too fast to have required the heartbeat: 0.5s
Cause 4: AssertionError: expected SUCCEEDED, got FAILED: States.Timeout

## Steps to Reproduce
Cause 1: a Task state 'arn:aws:states:::lambda:invoke' with InvocationType='Event' and a ResultSelector reading $.Payload.
Cause 2: SendTaskSuccess twice for the same token; read the second call's / execution's failure cause.
Cause 3: a .waitForTaskToken task with HeartbeatSeconds=5 and a worker that never heartbeats; expect States.Timeout.
Cause 4: the same task with a worker that heartbeats every 2s and then calls SendTaskSuccess; expect SUCCEEDED.

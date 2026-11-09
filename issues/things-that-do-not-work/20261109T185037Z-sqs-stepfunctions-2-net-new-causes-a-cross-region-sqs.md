Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs__stepfunctions.cross-region-sqs-sendmessage-optimized-integration-succeeds sqs__stepfunctions.cross-region-sqs-sendmessage-without-permission-on-far-queue-denied sqs__stepfunctions.sfn-task-token-timeout-on-no-callback
Issue-Key: sqs__stepfunctions.cross-region-sqs-sendmessage-optimized-integration-succeeds|run#7|AssertionError|assert resp["status"] == "SUCCEEDED", (
Issue-Key: sqs__stepfunctions.cross-region-sqs-sendmessage-without-permission-on-far-queue-denied|run#9|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Key: sqs__stepfunctions.sfn-task-token-timeout-on-no-callback|run#7|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 2 net-new causes: a cross-region sqs:sendMessage task cannot resolve the far queue, and a Catch on States.Timeout does not handle the task-token timeout

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two distinct root causes in the sqs__stepfunctions suite. Each is listed with its own failing check ids; both must be closed.

Cause 1 of 2 — the optimized sqs:sendMessage integration cannot address a queue in another region. AWS: a Task state 'arn:aws:states:::sqs:sendMessage' whose QueueUrl names a queue in a different region sends to that queue and SUCCEEDs, provided the state machine's role holds sqs:SendMessage on it (and is denied with AccessDenied when it does not). Emulator: both the permitted call and the control fail with error 'SQS.QueueDoesNotExistException' / cause 'The specified queue does not exist. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Proxy: null)' — the cross-region QueueUrl is resolved against the local region only, so an existing queue reads as nonexistent and the authorization distinction the second check grades cannot be reached. Why: a client's multi-region workflow fails with 'queue does not exist' for a queue it can see, and a real permission error is indistinguishable from this one. Fix: QueueUrl/region resolution in the sqs:sendMessage task integration (and the SQS queue lookup it calls). Traces: sqs__stepfunctions.cross-region-sqs-sendmessage-optimized-integration-succeeds, sqs__stepfunctions.cross-region-sqs-sendmessage-without-permission-on-far-queue-denied.

Cause 2 of 2 — a States.Timeout on a waitForTaskToken task is not routed to the state's Catch. AWS: a .waitForTaskToken task that exceeds TimeoutSeconds raises States.Timeout, and a Catch on States.Timeout handles it, so the execution ends SUCCEEDED through the catch branch. Emulator: the execution ends FAILED with error='States.Timeout' and cause "state 'WaitForCallback' ran its task token for longer than the 5 seconds its 'TimeoutSeconds' allows" — the catcher is never applied (and the cause text is the emulator's own sentence rather than AWS's). Why: a state machine that handles callback timeouts correctly still fails, so no workflow can recover from a missing callback. Fix: error-matching/Catch evaluation for States.Timeout in the task-token timeout path. Traces: sqs__stepfunctions.sfn-task-token-timeout-on-no-callback.

## Evidence
Cause 1: AssertionError: a cross-region sqs:sendMessage task must SUCCEED; got 'FAILED' error='SQS.QueueDoesNotExistException' cause='The specified queue does not exist. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Proxy: null)'; AssertionError: control: the role holding sqs:SendMessage on the far queue must SUCCEED; got 'FAILED' error='SQS.QueueDoesNotExistException' ...
Cause 2: AssertionError: Expected SUCCEEDED (via Catch->TimedOut), got FAILED error='States.Timeout' cause="state 'WaitForCallback' ran its task token for longer than the 5 seconds its 'TimeoutSeconds' allows"

## Steps to Reproduce
Cause 1: create a queue in region B; run a state machine in region A with a Task 'arn:aws:states:::sqs:sendMessage' whose QueueUrl is the region-B queue (role granted sqs:SendMessage on it).
Cause 2: a Task with Resource 'arn:aws:states:::sqs:sendMessage.waitForTaskToken', TimeoutSeconds=5 and a Catch on States.Timeout; never answer the token.

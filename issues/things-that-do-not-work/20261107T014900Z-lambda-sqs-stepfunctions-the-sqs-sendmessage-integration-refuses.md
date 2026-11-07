Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.deploy-cdk-synth-express-workflow-lambda-sqs-stack lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative lambda__sqs__stepfunctions.deploy-stack-update-changes-timeoutseconds-behavior lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative
Issue-Key: lambda__sqs__stepfunctions.deploy-cdk-synth-express-workflow-lambda-sqs-stack|run#13|AssertionError|assert result["status"] == "SUCCEEDED", (
Issue-Key: lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative|run#17|AssertionError|assert cause.startswith("User: arn:aws:sts::"), f"expected an authorization failure, got: {cause}"
Issue-Key: lambda__sqs__stepfunctions.deploy-stack-update-changes-timeoutseconds-behavior|run#10|AssertionError|assert status1 == "SUCCEEDED", f"expected SUCCEEDED under the 30s timeout, got {status1}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs|_drain_queue#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|_start_execution#7|Timeout|resp = sfn.start_execution(stateMachineArn=sm_arn, name=name, input=json.dumps(input_dict))
Issue-Commit: 486a0852ab6c107011e8dec7ea6574940ef62e53

# lambda__sqs__stepfunctions: the sqs:sendMessage integration refuses an object MessageBody, a Lambda Task fails under a generous TimeoutSeconds, AccessDenied names the role ARN, and Retry/Catch and task-token flows never settle (5 causes)

> _Note: this issue was identified against the code base at commit `486a0852ab6c107011e8dec7ea6574940ef62e53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Five net-new root causes in the Step Functions service-integration surface. None of these failing checks is marked ALREADY FILED or OWNED in the replay report; where an open issue describes a neighbouring cause it is named below and does not carry these checks' replay keys.

Cause 1 of 5: the arn:aws:states:::sqs:sendMessage optimized integration refuses a non-string MessageBody instead of serializing it.
AWS: recorded behavior only (Step Functions service-integration parameter marshalling for SQS SendMessage). A Task whose Parameters supply MessageBody as a JSON object (what CDK's SqsSendMessage with TaskInput.from_object emits) succeeds: Step Functions serializes the object to a JSON string before calling SendMessage, and the queue receives that JSON document as the message body.
Emulator: the execution ends FAILED with error 'SQS.ValidationException' and cause 'expected string, got object start' — the object is handed to SQS SendMessage unserialized and the request is rejected.
Why: the most common CDK/ASL idiom for sending structured payloads to a queue cannot run at all; a state machine that works on AWS fails on the emulator with an error AWS never produces for that input.
Fix: the sqs:sendMessage integration's parameter marshalling in the Step Functions task executor (JSON-serialize a non-string MessageBody before the SendMessage call).
Traces: lambda__sqs__stepfunctions.deploy-cdk-synth-express-workflow-lambda-sqs-stack.

Cause 2 of 5: a Lambda Task whose function runs for a few seconds fails although TimeoutSeconds is far larger.
AWS: recorded behavior only (States language TimeoutSeconds on a Task). A Task with Resource arn:aws:states:::lambda:invoke and TimeoutSeconds=30 invoking a function that sleeps 4 s succeeds (status SUCCEEDED); only after the stack update tightens TimeoutSeconds below the function's runtime does the execution fail with States.Timeout.
Emulator: the first leg already FAILS — 'expected SUCCEEDED under the 30s timeout, got FAILED' — so the generous-timeout control never passes and the tightened-timeout comparison is unreachable.
Why: any workflow whose Lambda step takes more than a moment is unrunnable, and TimeoutSeconds cannot be distinguished from an unconditional failure. The open issue 20261105T081644Z-lambda-sqs-stepfunctions-no-aws-sdk-in-the.md reports that Lambda Task failures and timeouts never fail the Task — the inverse symptom — and does not cover this check.
Fix: the lambda:invoke task executor's timeout/settlement handling (honour the state's TimeoutSeconds against the invocation's actual duration).
Traces: lambda__sqs__stepfunctions.deploy-stack-update-changes-timeoutseconds-behavior.

Cause 3 of 5: the AccessDenied cause for a denied lambda:InvokeFunction names the role ARN instead of the assumed-role session ARN.
AWS: recorded behavior only (Step Functions task failure cause carrying the Lambda AccessDeniedException). When a state machine's role has no lambda:InvokeFunction grant, the execution's cause begins 'User: arn:aws:sts::<account>:assumed-role/<role name>/<session name> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action' — the principal is the STS assumed-role session ARN, not the IAM role ARN.
Emulator: the cause begins 'User: arn:aws:iam::<account>:role/<role name> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> ...' — an arn:aws:iam::...:role/... principal, so the check's 'expected an authorization failure, got: ...' assertion on the arn:aws:sts:: prefix fails.
Why: clients and operators parse that principal to identify the acting session; an IAM role ARN where AWS writes an assumed-role session ARN breaks that attribution and any prefix match on arn:aws:sts::. The open issue 20261106T134622Z-lambda-sqs-stepfunctions-an-accessdenied-names-the-role.md reports the same wording defect and owns the sibling CLI-driven check, but not this CloudFormation-deployed one; the two should be closed together.
Fix: the principal rendering in the authorization refusal's message (emit the assumed-role session ARN for a role-assuming caller).
Traces: lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative.

Cause 4 of 5: a Retry-with-backoff then Catch chain ending in an SQS send never settles.
AWS: recorded behavior only (States language Retry/Catch). A Task that keeps failing is retried on its Retry policy's schedule, then its Catch transition runs and the catch branch's sqs:sendMessage delivers the error payload to the queue; the execution reaches a terminal status well inside the check's budget.
Emulator: the check is abandoned with 'Error: Check timed out after 60.0s' — no catch-branch message ever reaches the queue and no verdict is reached.
Why: the standard error-handling idiom (retry, then route the failure onward) produces nothing, so a workflow's failure path is unobservable.
Fix: the Retry/Catch driver of the execution engine and its hand-off to the catch branch's task.
Traces: lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs.

Cause 5 of 5: a duplicate SendTaskSuccess on an already-completed task token is never refused, because the callback never resolves.
AWS: recorded behavior only (SendTaskSuccess on a .waitForTaskToken task). The first SendTaskSuccess resumes the execution; a second call with the same token is refused with TaskTimedOut/TaskDoesNotExist rather than resuming anything a second time.
Emulator: the check is abandoned with 'Error: Check timed out after 60.0s' — the waitForTaskToken callback never resolves, so the first SendTaskSuccess never has an effect and the duplicate's refusal is never observable. The open issue 20261106T214711Z-lambda-sqs-stepfunctions-the-arn-aws-states-sqs.md reports the unresolved sqs:sendMessage.waitForTaskToken callback and owns neighbouring checks but not this one.
Why: callback-pattern workflows cannot be completed at all, and a client cannot rely on the at-most-once guarantee of a task token.
Fix: the waitForTaskToken callback store and the SendTaskSuccess/SendTaskFailure handlers (resolve the token once, refuse later calls for the same token).
Traces: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative.

## Evidence
Cause 1 — Expected: StartSyncExecution status SUCCEEDED and a queue message carrying the Lambda output. Actual: 'AssertionError: expected SUCCEEDED, got FAILED: SQS.ValidationException expected string, got object start'.
Cause 2 — Expected: SUCCEEDED with TimeoutSeconds=30 over a 4 s function. Actual: 'AssertionError: expected SUCCEEDED under the 30s timeout, got FAILED'.
Cause 3 — Expected cause prefix: 'User: arn:aws:sts::<account>:assumed-role/<role>/<session>'. Actual: 'AssertionError: expected an authorization failure, got: User: arn:aws:iam::000000000000:role/<stack role> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:... because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)' — the account, role and function names there are per-run values; the divergence is the arn:aws:iam::...:role/ principal form.
Cause 4 — Actual: 'Error: Check timed out after 60.0s' (no catch-branch message on the queue).
Cause 5 — Actual: 'Error: Check timed out after 60.0s' (the task token never resumes the execution, so the duplicate SendTaskSuccess is never reached).

## Steps to Reproduce
Cause 1: deploy a state machine with a Task Resource arn:aws:states:::sqs:sendMessage whose Parameters.MessageBody is a JSON object (CDK SqsSendMessage with TaskInput.from_object) and StartSyncExecution. Cause 2: deploy a state machine with Resource arn:aws:states:::lambda:invoke, TimeoutSeconds=30, over a function that sleeps 4 s, and start an execution. Cause 3: deploy a state machine whose role lacks lambda:InvokeFunction and start an execution; read DescribeExecution.cause. Cause 4: run a Task that always fails with a Retry policy and a Catch transition to an sqs:sendMessage state; poll the queue. Cause 5: run an arn:aws:states:::lambda:invoke.waitForTaskToken (or sqs:sendMessage.waitForTaskToken) task, call SendTaskSuccess once, then again with the same token. Replay the check ids in trace_ids.

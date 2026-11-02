Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sqs-esm-lambda-trigger-positive lambda__sqs__stepfunctions.sqs-esm-batch-size-respected lambda__sqs__stepfunctions.sqs-esm-maximum-batching-window-respected lambda__sqs__stepfunctions.sqs-esm-message-attributes-preserved lambda__sqs__stepfunctions.sqs-esm-filter-policy-positive lambda__sqs__stepfunctions.sqs-esm-visibility-timeout-respected lambda__sqs__stepfunctions.sqs-esm-report-batch-item-failures-partial-retry lambda__sqs__stepfunctions.sqs-esm-dlq-on-lambda-failure-positive lambda__sqs__stepfunctions.sqs-esm-fifo-duplicate-message-not-redelivered-negative lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative lambda__sqs__stepfunctions.cli-purge-queue-then-esm-no-messages-processed-negative lambda__sqs__stepfunctions.cli-recreate-queue-changes-arn-stale-esm-negative lambda__sqs__stepfunctions.cli-update-event-source-mapping-batchsize-live lambda__sqs__stepfunctions.deploy-lambda-esm-reportbatchitemfailures-enabled-stack lambda__sqs__stepfunctions.sqs-esm-missing-receive-permission-negative lambda__sqs__stepfunctions.cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e lambda__sqs__stepfunctions.sfn-lambda-sqs-pipeline-positive lambda__sqs__stepfunctions.sfn-lambda-arn-in-definition-exact-match lambda__sqs__stepfunctions.sfn-choice-state-routes-lambda-output-to-sqs-queue-selection lambda__sqs__stepfunctions.sfn-catch-lambda-error-to-sqs lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs lambda__sqs__stepfunctions.sfn-lambda-task-timeoutseconds-triggers-retry lambda__sqs__stepfunctions.sfn-lambda-invoke-payload-too-large-negative lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency lambda__sqs__stepfunctions.sfn-parallel-lambda-sqs-branches lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error lambda__sqs__stepfunctions.sfn-map-state-itembatcher-lambda-sqs-fanout lambda__sqs__stepfunctions.sfn-parallel-branch-failure-negative-no-partial-sqs-delivery lambda__sqs__stepfunctions.sfn-sqs-fifo-missing-messagegroupid-negative lambda__sqs__stepfunctions.sfn-sqs-task-wrong-queueurl-negative lambda__sqs__stepfunctions.cfn-validate-sfn-lambda-sqs-stack lambda__sqs__stepfunctions.deploy-sfn-fifo-queue-integration-stack lambda__sqs__stepfunctions.cli-send-task-token-success-via-cli-resumes-execution lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative lambda__sqs__stepfunctions.sfn-invoke-lambda-missing-permission-negative lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative lambda__sqs__stepfunctions.sfn-send-sqs-missing-permission-negative lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-lambda-trigger-positive|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-batch-size-respected|_wait_for_messages#12|AssertionError|raise AssertionError(
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-maximum-batching-window-respected|run#11|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-message-attributes-preserved|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-filter-policy-positive|_wait_for_messages#14|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-visibility-timeout-respected|_wait_for_messages#10|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-report-batch-item-failures-partial-retry|run#14|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-dlq-on-lambda-failure-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-fifo-duplicate-message-not-redelivered-negative|run#14|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-concurrent-batches-scale-with-queue-depth|run#10|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-mismatched-queue-arn-not-triggered-negative|run#11|Timeout|resp = sqs.receive_message(QueueUrl=results["url"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.cli-purge-queue-then-esm-no-messages-processed-negative|run#39|Timeout|resp = sqs.receive_message(QueueUrl=res_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.cli-recreate-queue-changes-arn-stale-esm-negative|_drain#26|Timeout|resp = sqs_client.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.cli-update-event-source-mapping-batchsize-live|_drain_counts#26|Timeout|resp = sqs_client.receive_message(QueueUrl=results_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.deploy-lambda-esm-reportbatchitemfailures-enabled-stack|run#11|Timeout|time.sleep(5.0)
Issue-Key: lambda__sqs__stepfunctions.sqs-esm-missing-receive-permission-negative|run#14|AssertionError|assert state != "Enabled", (
Issue-Key: lambda__sqs__stepfunctions.cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e|run#38|AssertionError|assert result["status"] == "SUCCEEDED", f"execution ended in {result['status']}: {result.get('error')}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-sqs-pipeline-positive|run#24|AssertionError|assert status == "SUCCEEDED", f"Expected SUCCEEDED, got: {status}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-arn-in-definition-exact-match|run#18|AssertionError|assert desc_ok["status"] == "SUCCEEDED", (
Issue-Key: lambda__sqs__stepfunctions.sfn-choice-state-routes-lambda-output-to-sqs-queue-selection|run#14|AssertionError|assert desc["status"] == "SUCCEEDED", desc
Issue-Key: lambda__sqs__stepfunctions.sfn-catch-lambda-error-to-sqs|run#20|AssertionError|assert status == "SUCCEEDED", f"Expected SUCCEEDED via catch, got: {status}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-retry-backoff-then-catch-to-sqs|run#11|AssertionError|assert desc["status"] == "SUCCEEDED", f"expected SUCCEEDED (caught), got {desc['status']}: {desc.get('error')}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-task-timeoutseconds-triggers-retry|run#11|AssertionError|assert desc.get("error") == "States.Timeout", desc.get("error")
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-invoke-payload-too-large-negative|run#14|AssertionError|assert desc_big.get("error") == "States.DataLimitExceeded", desc_big
Issue-Key: lambda__sqs__stepfunctions.sfn-map-state-lambda-concurrency|run#24|AssertionError|assert desc["status"] == "SUCCEEDED", f"Expected SUCCEEDED, got: {desc['status']}"
Issue-Key: lambda__sqs__stepfunctions.sfn-parallel-lambda-sqs-branches|run#20|AssertionError|assert desc["status"] == "SUCCEEDED", f"Expected SUCCEEDED, got: {desc['status']}"
Issue-Key: lambda__sqs__stepfunctions.sfn-lambda-wait-for-task-token-positive|run#21|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-sendtaskfailure-from-lambda-propagates-error|run#9|AssertionError|assert desc["status"] == "SUCCEEDED", f"expected SUCCEEDED, got {desc['status']}: {desc.get('error')}"
Issue-Key: lambda__sqs__stepfunctions.sfn-map-state-itembatcher-lambda-sqs-fanout|run#19|AssertionError|assert "AccessDenied" in blob or "not authorized" in blob, desc
Issue-Key: lambda__sqs__stepfunctions.sfn-parallel-branch-failure-negative-no-partial-sqs-delivery|run#18|AssertionError|assert seen == [], f"message delivered despite sibling branch failure: {seen}"
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-fifo-missing-messagegroupid-negative|run#15|AssertionError|assert neg_execution.get("error") == "SQS.AmazonSQSException", neg_execution
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-task-wrong-queueurl-negative|run#13|AssertionError|assert neg_execution.get("error") == "SQS.QueueDoesNotExistException", neg_execution
Issue-Key: cloudformation|ValidateTemplate|ValidationError
Issue-Key: lambda__sqs__stepfunctions.deploy-sfn-fifo-queue-integration-stack|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__sqs__stepfunctions.cli-send-task-token-success-via-cli-resumes-execution|run#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|_wait_execution_terminal#7|Timeout|time.sleep(poll_s)
Issue-Key: lambda__sqs__stepfunctions.sfn-invoke-lambda-missing-permission-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative|run#37|Timeout|time.sleep(5)
Issue-Key: lambda__sqs__stepfunctions.sfn-send-sqs-missing-permission-negative|_run_sfn_until_permission_revoked#24|Timeout|time.sleep(5 + attempt * 5)
Issue-Key: lambda__sqs__stepfunctions.sfn-sqs-send-message-wait-for-task-token-missing-perm-negative|_poll_for_message#14|Timeout|resp = sqs.receive_message(
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# lambda__sfn__sqs: nine net-new causes — SQS event-source mappings never deliver, ESM creation is unauthorized, Lambda Tasks never dispatch, ItemBatcher is uninterpreted, Parallel leaks a sibling's SQS send, SQS task error names diverge, ValidateTemplate wording diverges, the FIFO stack rolls back, and task-token scenarios never converge (9 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Nine net-new root causes in this service group.

Cause 1 of 9: an SQS event-source mapping never polls its queue or invokes its function. The mapping reports State=Enabled, but no batch is ever delivered, so batch size, batching window, filter policies, message attributes, visibility timeout, partial-batch failure reporting, FIFO de-duplication, DLQ-on-failure, concurrency scaling, the mismatched-ARN negative, purge-then-consume, stale-ARN-after-recreate and live BatchSize updates are all unobservable.
  AWS: recorded behavior only for the poller; Smithy (lambda) models CreateEventSourceMapping/UpdateEventSourceMapping with `State` reaching `Enabled`, `BatchSize` (1-10000), `MaximumBatchingWindowInSeconds`, `FilterCriteria`, `FunctionResponseTypes` (ReportBatchItemFailures). After Enabled, AWS polls and invokes the function with an SQS `Records` batch bounded by BatchSize and the batching window; a handler error (or a reported itemIdentifier) returns exactly those messages after the visibility timeout.
  Emulator: no invocation ever occurs; the listed checks either time out at 60.0s or assert `Expected 1 messages, got 0`.
  Why: SQS-triggered Lambda does not work at all, and every negative in the family passes-by-vacuum rather than by contract.
  Fix: the event-source-mapping poller in the lambda service crate.
  Checks: .sqs-esm-lambda-trigger-positive, .sqs-esm-batch-size-respected, .sqs-esm-maximum-batching-window-respected, .sqs-esm-message-attributes-preserved, .sqs-esm-filter-policy-positive, .sqs-esm-visibility-timeout-respected, .sqs-esm-report-batch-item-failures-partial-retry, .sqs-esm-dlq-on-lambda-failure-positive, .sqs-esm-fifo-duplicate-message-not-redelivered-negative, .sqs-esm-concurrent-batches-scale-with-queue-depth, .sqs-esm-mismatched-queue-arn-not-triggered-negative, .cli-purge-queue-then-esm-no-messages-processed-negative, .cli-recreate-queue-changes-arn-stale-esm-negative, .cli-update-event-source-mapping-batchsize-live, .deploy-lambda-esm-reportbatchitemfailures-enabled-stack.

Cause 2 of 9: CreateEventSourceMapping performs no authorization — a mapping whose function execution role has no sqs:ReceiveMessage on the source queue is created and reaches Enabled.
  AWS: Smithy (lambda) CreateEventSourceMapping errors include InvalidParameterValueException (400); AWS validates the role's access to the source at admission and refuses, with the source ARN taken from the request filling the message template.
  Emulator: `state='Enabled'`.
  Why: a mapping that can never read its queue is accepted silently instead of refused.
  Fix: admission validation in the CreateEventSourceMapping handler of the lambda service crate.
  Check: .sqs-esm-missing-receive-permission-negative.

Cause 3 of 9: Step Functions never dispatches a Lambda Task, in either the `arn:aws:states:::lambda:invoke` integration form or the bare-function-ARN form; the execution fails immediately with `States.Runtime` and an emulator-authored cause.
  AWS: recorded behavior only for the dispatch. AWS invokes the function under the execution role; `lambda:invoke` yields the `{Payload, StatusCode, ExecutedVersion}` envelope, a bare ARN yields the raw payload, a handler exception becomes a TaskFailed whose error is the exception type, TimeoutSeconds yields `States.Timeout`, and an oversized result yields `States.DataLimitExceeded`.
  Emulator: `error='States.Runtime'`, `cause="state '<name>': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"`.
  Why: no workflow can call a function; Retry/Catch and timeout contracts keyed on Lambda error names never match.
  Fix: Task-state integration dispatch in the sfn service crate (the already-open issue 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describes this gap but does not carry these checks).
  Checks: .cli-deploy-and-run-sfn-lambda-sqs-pipeline-e2e, .sfn-lambda-sqs-pipeline-positive, .sfn-lambda-arn-in-definition-exact-match, .sfn-choice-state-routes-lambda-output-to-sqs-queue-selection, .sfn-catch-lambda-error-to-sqs, .sfn-lambda-retry-backoff-then-catch-to-sqs, .sfn-lambda-task-timeoutseconds-triggers-retry, .sfn-lambda-invoke-payload-too-large-negative, .sfn-map-state-lambda-concurrency, .sfn-parallel-lambda-sqs-branches, .sfn-lambda-wait-for-task-token-positive, .sfn-task-token-sendtaskfailure-from-lambda-propagates-error.

Cause 4 of 9: a Map state carrying `ItemBatcher` is not interpreted — the execution fails with `States.Runtime` cause "state 'BatchMap' carries the member 'ItemBatcher', which this emulator does not interpret, so nothing was run for it".
  AWS: recorded behavior only for the ASL member. AWS groups the Map's items into batches per MaxItemsPerBatch/MaxInputBytesPerBatch and passes each iteration `{"Items": [...]}` (plus BatchInput when given).
  Why: batched Map fan-out is unusable and the failure names an emulator limitation rather than any AWS error.
  Fix: ASL Map-state interpretation in the sfn service crate.
  Check: .sfn-map-state-itembatcher-lambda-sqs-fanout.

Cause 5 of 9: when one Parallel branch fails, the sibling branch's SQS send is still delivered.
  AWS: recorded behavior only. AWS aborts the in-flight siblings as soon as a branch fails, and a send that had not completed does not occur, so the queue stays empty.
  Emulator: the message arrives — `message delivered despite sibling branch failure`.
  Why: a client relying on all-or-nothing Parallel semantics gets a partially-applied side effect, which is a correctness (not merely timing) divergence.
  Fix: Parallel-state branch cancellation in the sfn service crate's execution driver.
  Check: .sfn-parallel-branch-failure-negative-no-partial-sqs-delivery.

Cause 6 of 9: the error name on a failed `sqs:sendMessage` Task is the bare SQS code instead of AWS's exception-suffixed, service-prefixed name.
  AWS: recorded behavior only for the mapping. AWS reports `SQS.AmazonSQSException` for a missing required parameter (cause carrying `MissingParameter` and the member name `MessageGroupId` from the request) and `SQS.QueueDoesNotExistException` for a nonexistent queue (cause "The specified queue does not exist. (Service: AmazonSQS; Status Code: 400; Error Code: AWS.SimpleQueueService.NonExistentQueue; Request ID: <id>...)" — the request id is per-call and not part of the template).
  Emulator: `error='SQS.MissingParameter'` / `error='SQS.QueueDoesNotExist'`.
  Why: Retry/Catch rules written against AWS's error names never match.
  Fix: the integration error-name mapping in the sfn service crate's Task-state failure path.
  Checks: .sfn-sqs-fifo-missing-messagegroupid-negative, .sfn-sqs-task-wrong-queueurl-negative.

Cause 7 of 9: ValidateTemplate's unresolved-reference refusal uses the wrong message class.
  AWS: recorded behavior only for the wording. For a Fn::GetAtt on an undeclared resource AWS answers `Template error: instance of Fn::GetAtt references undefined resource <LogicalId>`, where the logical id comes from the template.
  Emulator: `Template format error: Unresolved resource dependencies [<LogicalId>] in the Outputs block of the template`.
  Why: clients that classify template problems by message see a different error class than AWS reports.
  Fix: the template-reference validator in the cloudformation service crate's ValidateTemplate handler.
  Check: .cfn-validate-sfn-lambda-sqs-stack.

Cause 8 of 9: a CloudFormation stack declaring a state machine integrated with a FIFO SQS queue ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
  AWS: recorded behavior only; the same template reaches CREATE_COMPLETE in the recording.
  Emulator: `Stack <name> ended in ROLLBACK_COMPLETE`.
  Fix: the AWS::SQS::Queue (FIFO) and AWS::StepFunctions::StateMachine provisioners invoked by the CloudFormation stack-create handler.
  Check: .deploy-sfn-fifo-queue-integration-stack.

Cause 9 of 9: task-token and permission-denial scenarios never converge — the execution never parks at a `.waitForTaskToken` Task, so neither SendTaskSuccess resumption, duplicate-token rejection, nor the denied-permission arms ever produce an observable result; the checks run to the 60s cap.
  AWS: recorded behavior only; AWS parks the Task, hands out a token observable in the execution history (TaskScheduled/TaskSubmitted), resumes on SendTaskSuccess, and refuses a second SendTaskSuccess for the same token with `TaskTimedOut`/`TaskDoesNotExist`.
  Emulator: nothing parks, nothing is handed out, nothing converges.
  Fix: `.waitForTaskToken` Task parking and token bookkeeping in the sfn service crate.
  Checks: .cli-send-task-token-success-via-cli-resumes-execution, .sfn-task-token-duplicate-sendtasksuccess-rejected-negative, .sfn-invoke-lambda-missing-permission-negative, .cli-detach-lambda-invoke-policy-breaks-sfn-task-negative, .sfn-send-sqs-missing-permission-negative, .sfn-sqs-send-message-wait-for-task-token-missing-perm-negative.

## Evidence
Cause 1: `AssertionError: Expected 1 messages, got 0 within 5s` (.sqs-esm-batch-size-respected) and 14 further checks ending `Check timed out after 60.0s`.
Cause 2: `AssertionError: ESM reached Enabled state without sqs:ReceiveMessage on the execution role, got state='Enabled'`.
Cause 3: `AssertionError: ... status 'FAILED', 'error': 'States.Runtime', 'cause': "state 'InvokeLambda': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"`.
Cause 4: `'error': 'States.Runtime', 'cause': "state 'BatchMap' carries the member 'ItemBatcher', which this emulator does not interpret, so nothing was run for it"`.
Cause 5: `AssertionError: message delivered despite sibling branch failure: [{'Body': 'should-not-arrive-if-sibling-failed', ...}]`.
Cause 6: expected `error == 'SQS.AmazonSQSException'`, got `'SQS.MissingParameter'`; expected `error == 'SQS.QueueDoesNotExistException'`, got `'SQS.QueueDoesNotExist'`.
Cause 7: `AssertionError: expected the exact unresolved-reference message 'Template error: instance of Fn::GetAtt references undefined resource GhostFn', got: 'Template format error: Unresolved resource dependencies [GhostFn] in the Outputs block of the template'`.
Cause 8: `AssertionError: Stack sim2real-xsvc-stk-d25cb9876761 ended in ROLLBACK_COMPLETE`.
Cause 9: six checks end `Check timed out after 60.0s` with no TaskScheduled/token ever observable.

## Steps to Reproduce
Cause 1: CreateEventSourceMapping(queue -> function, BatchSize=N), SendMessage, poll the function's invocation record — none appears.
Cause 2: CreateEventSourceMapping where the function role lacks sqs:ReceiveMessage — the mapping reaches Enabled.
Cause 3: StartExecution on a state machine whose Task Resource is arn:aws:states:::lambda:invoke — FAILED with States.Runtime.
Cause 4: StartExecution on a Map state carrying ItemBatcher — FAILED with States.Runtime naming ItemBatcher.
Cause 5: StartExecution on a Parallel whose branch A fails and branch B sends to SQS; ReceiveMessage — the message is present.
Cause 6: StartExecution on a Task Resource=arn:aws:states:::sqs:sendMessage against a FIFO queue without MessageGroupId, and against a nonexistent QueueUrl; read DescribeExecution.error.
Cause 7: ValidateTemplate on a template whose Outputs Fn::GetAtt names an undeclared resource.
Cause 8: CreateStack with a template declaring a FIFO AWS::SQS::Queue plus AWS::StepFunctions::StateMachine.
Cause 9: StartExecution on a Task Resource=...:lambda:invoke.waitForTaskToken and poll GetExecutionHistory for TaskScheduled — nothing parks.

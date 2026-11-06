Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write cloudformation.stack-continue-update-rollback-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift|_cdk#14|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write|_invoke#31|AssertionError|assert "FunctionError" not in response, (
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_next_request#26|Timeout|return json.loads(envelope["Message"])
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# cloudformation: 3 net-new root causes — the AWS::DynamoDB::Table provider refuses PointInTimeRecoverySpecification instead of applying it, the Lambda python runtime has no boto3, and ContinueUpdateRollback never converges

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new behavioral root causes of service `cloudformation`, grounded in three failing behavioral checks. A fixer must close all three before submitting.

Cause 1 of 3: the AWS::DynamoDB::Table resource provider actively REFUSES a template property it does not implement (PointInTimeRecoverySpecification) instead of applying it, so every stack that declares PITR on a table fails to create.
AWS: PointInTimeRecoverySpecification is a supported property of AWS::DynamoDB::Table; CloudFormation applies it by calling DynamoDB UpdateContinuousBackups after the table is created (Smithy dynamodb: UpdateContinuousBackups / DescribeContinuousBackups carry PointInTimeRecoverySpecification.PointInTimeRecoveryEnabled; CreateTable itself has no PITR member, so it is a post-create apply). The stack reaches CREATE_COMPLETE and DescribeContinuousBackups reports the declared value. AWS emits no error for this property.
Emulator: the resource goes CREATE_FAILED and the stack rolls back with `Resource handler returned message: "this emulator does not apply the PointInTimeRecoverySpecification property of AWS::DynamoDB::Table" (HandlerErrorCode: InvalidRequest)`. The whole message is fixed emulator text — only the property name and the RequestToken vary; the RequestToken is per-run and is not part of the requirement. No AWS error has this wording, and InvalidRequest tells the client its own template is wrong when it is not.
Why: a self-announced emulator limitation is surfaced to the client as a terminal, user-attributed template rejection. `cdk deploy` of any table with PITR (the CDK L2 default idiom `pointInTimeRecoverySpecification`) exits 1, so no PITR/drift scenario can be deployed, and a client triaging the failure is told to fix a template that is valid.
Fix: the AWS::DynamoDB::Table resource provider in the cloudformation resource-provisioning layer — the property applier must apply PointInTimeRecoverySpecification (post-create UpdateContinuousBackups) rather than raising an unsupported-property refusal. The same unsupported-property refusal path fails other table properties under a different suite heading; see the companion issue filed for service `cloudformation__dynamodb` in this same sweep (TimeToLiveSpecification, StreamSpecification, SSESpecification, TableClass, ContributorInsightsSpecification).
Traces: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift.

Cause 2 of 3: the Lambda python runtime that executes a deployed function provides no AWS SDK, so every handler that does `import boto3` dies at module load.
AWS: recorded behavior only — the managed `python3.x` Lambda runtimes bundle boto3/botocore, so `import boto3` at module scope succeeds and the handler runs under its execution role, returning either the service result or the service's own refusal (here an `AccessDenied`/`AccessDeniedException` from DynamoDB when the grant does not cover the call). Smithy (lambda Invoke): `FunctionError` is the `X-Amz-Function-Error` header and is set only when the handler itself faulted. AWS has no error for a missing SDK in the runtime.
Emulator: Invoke answers 200 with `FunctionError: Unhandled` and payload `{"errorMessage":"Unable to import module '<handler module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. Fixed text: `Unable to import module '<module>': No module named 'boto3'` and `errorType` `Runtime.ImportModuleError`; the module name comes from the function's Handler and the requestId is per-invocation, so neither is part of the requirement.
Why: an IAM grant cannot be observed at all. The allowed call and the denied call return the identical runtime fault, so a too-broad or missing `grant_read_data`/`grant_read_write_data` expansion is indistinguishable from a correct one, and any CloudFormation scenario whose function touches another AWS service is unrunnable.
Fix: the python runtime environment of the Lambda invoke path — boto3/botocore must be importable inside the in-process runtime that executes the deployment package. This is the same class already recorded as Cause 4 of the open issue 20261105T132157Z-cloudformation-8-net-new-root-causes-in-the-684719.md (grounded there in cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle); this check is a second, independently failing grounding of it under the same `cloudformation` heading and the suite did not report it as owned, so it is filed here and the two should be closed together.
Traces: cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write.

Cause 3 of 3: the ContinueUpdateRollback lifecycle never reaches a verdict — a stack driven to UPDATE_ROLLBACK_FAILED by a custom resource does not settle, so the check is cut off.
AWS: recorded behavior only — a stack whose update rollback failed sits in UPDATE_ROLLBACK_FAILED, ContinueUpdateRollback moves it through UPDATE_ROLLBACK_IN_PROGRESS to UPDATE_ROLLBACK_COMPLETE, and ContinueUpdateRollback on a deleted stack is refused `ValidationError` (HTTP 400) with the stack-not-found text. Each transition is observable from DescribeStacks well inside the budget, and each Custom::Resource handler request is delivered to the declared ServiceToken (an SNS topic fanned out to SQS) so the caller can answer it.
Emulator: the check is cut off with `Error: Check timed out after 60.0s` and no response to compare. The 60 s figure is the suite's budget, not a requirement — the requirement is that the polled surfaces (the Custom::Resource request arriving on the SNS-subscribed queue, then DescribeStacks StackStatus) reach their terminal values.
Why: a client calling ContinueUpdateRollback and waiting on `stack_update_complete` hangs forever instead of being told what happened, and the documented recovery path out of UPDATE_ROLLBACK_FAILED is unusable.
Fix: whichever polled surface does not settle — the SNS-backed Custom::Resource request delivery in the cloudformation provisioning path, and the UPDATE_ROLLBACK_FAILED -> ContinueUpdateRollback -> UPDATE_ROLLBACK_COMPLETE state machine. The open issue 20261029T214321Z-cloudformation-five-behavioral-checks-exhaust-the-harness-s.md records other non-converging checks but does not carry this one's replay key.
Traces: cloudformation.stack-continue-update-rollback-lifecycle.

## Evidence
Cause 1 — cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift [mismatch]:
  AssertionError: `cdk deploy sim2real-cdkdiff-<id> --require-approval never` exited 1, expected 0; output tail: `| AWS::DynamoDB::Table | Table (TableCD117FA1) Resource handler returned message: "this emulator does not apply the PointInTimeRecoverySpecification property of AWS::DynamoDB::Table" (RequestToken: <per-run token>, HandlerErrorCode: InvalidRequest)` ... `DeploymentError: Resource updates failed`.
  Expected (AWS): the deploy exits 0, the stack reaches CREATE_COMPLETE and DescribeContinuousBackups on the table reports the declared PointInTimeRecoveryEnabled value. (The stack name, request token and CDK logical-id suffix are per-run and are not part of the requirement.)

Cause 2 — cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write [mismatch]:
  AssertionError: invoking '<stack>-ReadOnly<suffix>' raised 'Unhandled': b'{"errorMessage":"Unable to import module \'index\': No module named \'boto3\'","errorType":"Runtime.ImportModuleError","requestId":"<per-run id>","stackTrace":[]}'
  Expected (AWS): 200 with no FunctionError and the handler's own result — the GetItem it was granted succeeding, and the PutItem it was not granted returning DynamoDB's AccessDeniedException.

Cause 3 — cloudformation.stack-continue-update-rollback-lifecycle [mismatch]:
  Error: Check timed out after 60.0s (no response observed; the check polls for a Custom::Resource request on an SNS->SQS queue, then for DescribeStacks to reach CREATE_COMPLETE/UPDATE_ROLLBACK_FAILED/UPDATE_ROLLBACK_COMPLETE).
  Expected (AWS): UPDATE_ROLLBACK_FAILED -> ContinueUpdateRollback -> UPDATE_ROLLBACK_COMPLETE, then ContinueUpdateRollback on the deleted stack refused with ValidationError (HTTP 400).

Smithy consulted: dynamodb CreateTable/UpdateTable/DescribeTable inputs (no PITR member on CreateTable — PITR is applied by UpdateContinuousBackups, so CloudFormation applies it post-create); lambda Invoke FunctionError is the X-Amz-Function-Error header, set only on a handler fault; cloudformation Stack.StackStatusReason and StackEvent.ResourceStatusReason are free strings, so the reason texts above are recorded behavior.

## Steps to Reproduce
1. Cause 1: CreateStack (or `cdk deploy`) a template with an AWS::DynamoDB::Table carrying `PointInTimeRecoverySpecification: {PointInTimeRecoveryEnabled: false}` (the CDK L2 default property). Observe the Table resource go CREATE_FAILED with `Resource handler returned message: "this emulator does not apply the PointInTimeRecoverySpecification property of AWS::DynamoDB::Table"` / HandlerErrorCode InvalidRequest and the stack roll back; expect CREATE_COMPLETE.
2. Cause 2: deploy any function whose handler module does `import boto3` and Invoke it; observe FunctionError Unhandled with Runtime.ImportModuleError / No module named 'boto3' instead of the handler's result.
3. Cause 3: create a stack with an SNS-backed Custom::Resource, answer its Create, UpdateStack so the update and then its rollback fail, poll DescribeStacks for UPDATE_ROLLBACK_FAILED, call ContinueUpdateRollback and poll for UPDATE_ROLLBACK_COMPLETE; then DeleteStack and call ContinueUpdateRollback again expecting ValidationError.
4. Re-replay: cloudformation.cdk-diff-sees-template-change-not-out-of-band-drift, cloudformation.cdk-grant-read-data-permits-real-read-denies-real-write, cloudformation.stack-continue-update-rollback-lifecycle.

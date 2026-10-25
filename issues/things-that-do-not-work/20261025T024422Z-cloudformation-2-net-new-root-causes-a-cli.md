Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cli-script-cfn-deploy-and-verify cloudformation.concurrent-resources-limit-exceeded
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cli-script-cfn-deploy-and-verify|_run_shell#6|Timeout|result = subprocess.run(
Issue-Key: cloudformation.concurrent-resources-limit-exceeded|run#11|AssertionError|assert created, (
Issue-Commit: 448dbaa5987c679f4ac9bf289637b03678756bff

# cloudformation: 2 net-new root causes — a CLI-driven create-stack workflow never converges (harness cap), and CreateGeneratedTemplate's concurrent in-flight cap is never enforced because the operation answers NotImplemented

> _Note: this issue was identified against the code base at commit `448dbaa5987c679f4ac9bf289637b03678756bff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the cloudformation service, both grounded in failing behavioral checks that the replay report did not mark ALREADY FILED or OWNED.

Cause 1 of 2: a CLI-driven CreateStack -> wait stack-create-complete -> use resource -> DeleteStack -> wait stack-delete-complete workflow never converges, so the entire `aws cloudformation` scripted deploy path is unusable.
Failing check id: cloudformation.cli-script-cfn-deploy-and-verify.
AWS: Smithy: CreateStack returns a StackId and the stack advances through CREATE_IN_PROGRESS to the terminal CREATE_COMPLETE; DescribeStacks (no modeled errors, paginated on Stacks) is the documented read the `stack-create-complete` waiter polls, and DeleteStack drives the stack to DELETE_COMPLETE, after which DescribeStacks on the name refuses. For a single-resource AWS::SQS::Queue template real CloudFormation reaches CREATE_COMPLETE in seconds, so both CLI waiters return promptly and the stack's Outputs then expose the queue URL.
Emulator: the check is cut off by the harness after 60s with 'Error: Check timed out after 60.0s' while the bash script is still inside `aws cloudformation wait stack-create-complete`. The emulator answers the DescribeStacks polls but the stack never reaches the terminal status the waiter is waiting for, so the script never gets as far as reading the QueueUrl output, sending/receiving through the deployed queue, or deleting the stack.
Why: a client that uses the standard AWS CLI waiters (or any SDK waiter) to deploy a stack hangs forever. No scripted/CI deploy can complete, and because the script never returns, nothing downstream of the deploy (stack Outputs, the provisioned resource, teardown) can be exercised at all. This is a convergence failure, not a wrong value.
Note on an existing issue: open issue 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md describes the same convergence family, but its Issue-Trace-Ids / Issue-Key list does NOT contain cli-script-cfn-deploy-and-verify, so this check is not covered by it. The fixer should treat this check as an additional replay key for that convergence defect rather than as unrelated work.
Fix: the asynchronous stack-operation driver in the cloudformation service crate — the task that advances a CreateStack/DeleteStack operation through its status machine and the status each DescribeStacks read publishes.
Traces: cloudformation.cli-script-cfn-deploy-and-verify ('Error: Check timed out after 60.0s', timing out inside `aws cloudformation wait stack-create-complete`).

Cause 2 of 2: CreateGeneratedTemplate refuses every call with NotImplemented, so its concurrent in-flight quota (and the ConcurrentResourcesLimitExceeded refusal that enforces it) does not exist.
Failing check id: cloudformation.concurrent-resources-limit-exceeded.
AWS: Smithy: CreateGeneratedTemplate(GeneratedTemplateName required, @length 1..128; StackName; Resources @length 1..500) returns GeneratedTemplateId and models the error ConcurrentResourcesLimitExceededException with awsQueryError code 'ConcurrentResourcesLimitExceeded' at HTTP 429. CloudFormation caps the number of generated templates that may be Pending/InProgress simultaneously per account/Region (documented cap: 5); calls beyond the cap are refused with wire code 'ConcurrentResourcesLimitExceeded' (429) while the calls within the cap succeed, and a refused call must leave no generated template behind — DescribeGeneratedTemplate for that name then refuses with GeneratedTemplateNotFoundException, awsQueryError code 'GeneratedTemplateNotFound' at HTTP 404. The cap is transient: once the in-flight generations reach a terminal status, an identical call succeeds and reaches Status=COMPLETE.
Emulator: all 15 concurrent CreateGeneratedTemplate calls in the check's burst are refused with error code 'NotImplemented' (the scaffold-stub refusal), so not one call lands in-progress. The check's positive control fails: other_codes=['NotImplemented' x15]. Neither the success path nor the 429 ConcurrentResourcesLimitExceeded refusal nor the 404 GeneratedTemplateNotFound follow-up read is reachable.
Why: a client cannot generate a template from an existing stack at all, and — the net-new requirement here — once the operation is implemented it must also carry the in-flight concurrency cap: a client that fires more than the cap must receive 429 'ConcurrentResourcesLimitExceeded' (not a success, not a generic throttle, not 'NotImplemented'), must be able to confirm via DescribeGeneratedTemplate that the refused name was never created (404 'GeneratedTemplateNotFound'), and must see the cap lift once the in-flight generations finish.
Note on an existing issue: open issue 20261025T015306Z-cloudformation-creategeneratedtemplate-stub.md covers only standing the operation up. It does not name the concurrency cap or the ConcurrentResourcesLimitExceeded/GeneratedTemplateNotFound refusal contract, which is what this check grades, so this cause is net-new work layered on that one.
Fix: the CreateGeneratedTemplate handler and its validation/quota check in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_generated_template.rs), plus the generated-template state that tracks per-account/Region in-flight generations and the status machine DescribeGeneratedTemplate reads.
Traces: cloudformation.concurrent-resources-limit-exceeded (AssertionError: 'no concurrent CreateGeneratedTemplate call succeeded at all (positive control failed): other_codes=[NotImplemented x15]').

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 80/405 passed; cloudformation 72/242. The emulator is serving (no bootstrap signal, 80 checks pass), so these are fidelity divergences.

Cause 1 — cloudformation.cli-script-cfn-deploy-and-verify, verdict [mismatch]:
  Expected: `aws cloudformation create-stack` on a one-resource AWS::SQS::Queue template, then `aws cloudformation wait stack-create-complete`, returns within seconds; DescribeStacks then exposes the QueueUrl output; `delete-stack` + `wait stack-delete-complete` likewise return.
  Actual: 'Error: Check timed out after 60.0s' — the harness cut the check off while the script was still waiting for CREATE_COMPLETE. No stack output was ever read.

Cause 2 — cloudformation.concurrent-resources-limit-exceeded, verdict [mismatch]:
  Expected: of 15 concurrent CreateGeneratedTemplate calls against a 25-resource stack, up to the in-flight cap succeed (HTTP 200 with GeneratedTemplateId) and the excess are refused with HTTP 429, error code 'ConcurrentResourcesLimitExceeded'; DescribeGeneratedTemplate on a refused name answers HTTP 404, error code 'GeneratedTemplateNotFound'; after the in-flight set reaches a terminal status an identical call reaches Status='COMPLETE'.
  Actual: every one of the 15 calls was refused with error code 'NotImplemented'. Replay detail: AssertionError: no concurrent CreateGeneratedTemplate call succeeded at all (positive control failed): other_codes=['NotImplemented' x 15].
  Smithy model (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): CreateGeneratedTemplate errors = AlreadyExistsException(400), ConcurrentResourcesLimitExceededException(429, awsQueryError code 'ConcurrentResourcesLimitExceeded'), LimitExceededException(400); DescribeGeneratedTemplate errors = GeneratedTemplateNotFoundException(404, awsQueryError code 'GeneratedTemplateNotFound').

## Steps to Reproduce
Cause 1 (cloudformation.cli-script-cfn-deploy-and-verify):
1. Boot the emulator and point the AWS CLI at it.
2. Run: aws cloudformation create-stack --stack-name <name> --template-body '{"Resources":{"Q":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"<q>"}}},"Outputs":{"QueueUrl":{"Value":{"Ref":"Q"}}}}'
3. Run: aws cloudformation wait stack-create-complete --stack-name <name>. It never returns; poll DescribeStacks directly and observe the stack never reaches CREATE_COMPLETE.
4. Replay the check: it reports 'Check timed out after 60.0s'.

Cause 2 (cloudformation.concurrent-resources-limit-exceeded):
1. CreateStack with 25 AWS::SNS::Topic resources and wait for it to settle.
2. Fire 15 concurrent CreateGeneratedTemplate calls with distinct GeneratedTemplateName values and StackName=<that stack>.
3. Observe every call is refused with error code 'NotImplemented' instead of some succeeding (200, GeneratedTemplateId) and the excess being refused with HTTP 429 / 'ConcurrentResourcesLimitExceeded'.
4. For any refused name, call DescribeGeneratedTemplate and observe it does not answer HTTP 404 / 'GeneratedTemplateNotFound' as the contract requires.
5. After the in-flight set reaches a terminal status, re-issue one identical CreateGeneratedTemplate and observe it does not reach Status='COMPLETE' with StatusReason='Template complete'.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke|run#13|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=function_name, WaiterConfig={"Delay": 2, "MaxAttempts": 150})
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# A stack that reaches CREATE_COMPLETE never creates its AWS::Lambda::Function, so the function is not found (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — CloudFormation reports a stack CREATE_COMPLETE while the AWS::Lambda::Function it declares does not exist.
AWS: Smithy (com.amazonaws.cloudformation) CreateStack; recorded behavior only. A stack whose template declares an AWS::Lambda::Function (with its execution role) reaches CREATE_COMPLETE only once that function exists: GetFunctionConfiguration resolves it, the FunctionActiveV2 waiter settles on State=Active, and the function's implicit /aws/lambda/<name> log group is created on its first invocation.
Emulator: the stack reports CREATE_COMPLETE, but lambda has no such function — the FunctionActiveV2 waiter fails immediately with ResourceNotFoundException `Function not found: arn:aws:lambda:<region>:<account>:function:<name>`, so the check never reaches the behavior it measures.
Why: CREATE_COMPLETE is the signal every deployment pipeline and IaC provider gates on. Reporting it for resources that were never created makes the stack status worthless: callers proceed to use a function that does not exist and get a not-found error from a resource their deploy just told them was ready.
Fix: the AWS::Lambda::Function resource provisioner in the cloudformation crate's resource-type dispatch — the stack must actually create the function (and fail the stack when it cannot) before the stack is marked CREATE_COMPLETE. Note: open issues 20261029T072657Z-cloudformation-iam-lambda-logs-a-stack-declaring-aws-e5cb3e.md and 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md record this family and carry the two sibling checks' keys but not this one's.
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

deploy-logs-2-implicit-log-group-created-on-first-invoke (verdict: error):
  botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:<region>:<account>:function:<name>

The function name and account/region in that ARN are this run's values. The divergence is that the stack reported CREATE_COMPLETE for a declared AWS::Lambda::Function that lambda does not hold.

## Steps to Reproduce
Boot the emulator from this checkout.

1. aws cloudformation create-stack --stack-name t --template-body <template declaring an AWS::IAM::Role and an AWS::Lambda::Function on it> --capabilities CAPABILITY_IAM
2. aws cloudformation wait stack-create-complete --stack-name t  -> succeeds (CREATE_COMPLETE)
3. aws lambda get-function-configuration --function-name <the declared function name>  -> ResourceNotFoundException 'Function not found: ...'
AWS resolves the function and the FunctionActiveV2 waiter settles.

Then re-replay: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke

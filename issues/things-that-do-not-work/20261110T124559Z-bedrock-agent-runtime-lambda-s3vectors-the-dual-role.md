Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative|_invoke_and_parse#10|Timeout|return payload
Issue-Commit: e21403e06647cad18ab089dcba740f1e2399789f

# bedrock-agent-runtime__lambda__s3vectors: the dual-role (permitted + mismatched) CFN Lambda stack never settles, so the overly-permissive negative check hangs instead of observing two AccessDeniedExceptions

> _Note: this issue was identified against the code base at commit `e21403e06647cad18ab089dcba740f1e2399789f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that declares two AWS::IAM::Role resources (one carrying s3vectors:ListVectorBuckets + bedrock:ListSessions, one carrying only logs:* permissions) and two AWS::Lambda::Function resources bound to them never produces an observable terminal outcome, so the check that compares the permitted function against the mismatched one is killed by the harness at its 60s limit. Failing check: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative.

AWS: CreateStack is accepted and DescribeStacks then always converges to a terminal StackStatus for an accepted template — Smithy (cloudformation): StackStatus enum includes CREATE_IN_PROGRESS, CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED — so a client polling DescribeStacks terminates. With the stack complete, Lambda Invoke of the function on the permitted role returns a 200 payload reporting success for both s3vectors:ListVectorBuckets and bedrock-agent-runtime ListSessions, and Invoke of the identical function on the mismatched role (a role that exists and trusts lambda.amazonaws.com but holds neither permission) returns a 200 Invoke whose handler payload records error code AccessDeniedException for both calls — the per-request authorization decision, not a deploy-time failure. Recorded behavior only for the per-resource provisioning sequence of this template (two roles + two functions in one stack): the recording shows the stack settling and both functions answering an invoke.
Emulator: the check never gets past its polling. It fails with "Error: Check timed out after 60.0s" — no terminal stack status and no function reply is ever observed. Nothing is refused: the request is accepted and then never converges, which is worse for a client than an error, because there is no status or error code to handle. The emulator therefore also never demonstrates the IAM outcome the check is about (AccessDeniedException from both services for the mismatched role).
Why: a caller that deploys a multi-role/multi-function stack and waits for it (a CFN waiter, `aws cloudformation deploy`, CDK, or any pipeline step) blocks until its own timeout rather than receiving CREATE_COMPLETE or a failure reason, and a negative test that relies on a least-privilege role being denied can never run at all — an overly-permissive emulator looks identical to a hung one from the client's side.
Fix: the CloudFormation stack state machine and the AWS::IAM::Role / AWS::Lambda::Function resource providers it drives in the cloudformation service — every accepted stack must reach a terminal StackStatus, and a stack holding two roles and two functions must signal completion for each resource; then the Lambda Invoke path must evaluate the invoked function's execution-role policy so an unprivileged role yields AccessDeniedException inside the handler. File and line citations belong with whichever provider never signals completion.
Traces: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative.

Related but not covering this check: open issues 20261106T225443Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-create-and.md, 20261107T085049Z-bedrock-agent-runtime-lambda-s3vectors-the-cloudformation-lambda.md and 20261107T110223Z-bedrock-agent-runtime-lambda-s3vectors-cfn-lambda-execution.md describe non-convergence of other stacks in this group; none carries this check's replay key, and this template is the only one in the suite with two roles and two functions in a single stack.

## Evidence
Replayed failure under the suite heading `### bedrock-agent-runtime__lambda__s3vectors`, marked neither ALREADY FILED nor OWNED:
- [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative — "Error: Check timed out after 60.0s" (GROUP overly-permissive, SERVICES = [bedrock-agent-runtime, lambda, s3vectors]).

Expected: CreateStack accepted; DescribeStacks reports a terminal StackStatus (CREATE_COMPLETE) within the poll window; Invoke of the function on the permitted role returns {"s3vectors": {"ok": true, ...}, "bedrock": {"ok": true, ...}}; Invoke of the function on the mismatched role returns ok=false with error_code "AccessDeniedException" for both the s3vectors ListVectorBuckets call and the bedrock-agent-runtime ListSessions call (fixed text: the error code AccessDeniedException; the variable parts of the accompanying message are the caller's assumed-role ARN, the action name and the resource ARN taken from the request and emulator state — never the literal identifiers of one recorded run).
Actual: no terminal stack status and no invoke result are ever observed; the check is killed at the 60s harness limit, so neither the positive control nor the deny assertion is reached.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative.
2. By hand: CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and a template containing (a) GoodRole — an AWS::IAM::Role trusting lambda.amazonaws.com with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents, s3vectors:ListVectorBuckets and bedrock:ListSessions; (b) BadRole — the same role but with only the logs statements; (c) GoodFn and BadFn — two AWS::Lambda::Function resources (python3.12, index.handler, inline ZipFile code calling s3vectors ListVectorBuckets then bedrock-agent-runtime ListSessions) whose Role is Fn::GetAtt of the respective role; and Outputs exporting both function names.
3. Poll DescribeStacks(StackName=...) — the StackStatus never becomes terminal (expected CREATE_COMPLETE).
4. Once a stack does settle, Invoke GoodFunctionName (expect both calls ok) and BadFunctionName (expect error_code AccessDeniedException from both s3vectors and bedrock-agent-runtime).

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive|_invoke_and_parse#4|Timeout|return payload
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# bedrock-agent-runtime__lambda__s3vectors: a stack update that adds s3vectors:ListVectorBuckets to a Lambda execution role never converges, so the denied-then-allowed transition is never observable

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the CloudFormation create -> invoke -> UpdateStack -> invoke loop for a Lambda function whose execution role gains s3vectors:ListVectorBuckets never reaches the point where the identical in-function call flips from refused to allowed. The check is killed by the harness at its 60s budget without ever observing a terminal outcome. Failing check: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive.

AWS: UpdateStack (Smithy: cloudformation UpdateStack, input StackName required, TemplateBody @length min 1, Capabilities list; modeled errors only InsufficientCapabilitiesException 400 and TokenAlreadyExistsException 400 — neither applies here) accepts the revised template and the stack then reaches a terminal status that DescribeStacks reports (UPDATE_COMPLETE, or UPDATE_ROLLBACK_COMPLETE / UPDATE_FAILED / UPDATE_ROLLBACK_FAILED); DescribeStacks (Smithy: cloudformation DescribeStacks, output Stacks, no modeled errors other than the service-common ValidationError for an absent stack) always answers, so a polling client always terminates. Recorded behavior only for the IAM effect: before the update the in-function s3vectors ListVectorBuckets is refused with error code AccessDeniedException (the recording's handler reports {"ok": false, "error_code": "AccessDeniedException"}); after the update the same call succeeds and ListVectorBuckets answers 200 with a vectorBuckets list, and two back-to-back reads agree on its length. The fixed part of that requirement is the error code AccessDeniedException and the allow-after-update success; the function name, the bucket count and the stack name are values taken from the request and the emulator's state, never literals from one run.

Emulator: the check never gets that far — "Error: Check timed out after 60.0s". Nothing is refused and no status is reported as terminal: the emulator accepts CreateStack/UpdateStack and then the client's DescribeStacks poll (or the post-update invoke poll on the deployed function) never satisfies its predicate inside the window, so there is no response for the client to act on.

Why: a caller that models permission changes as a CloudFormation stack update (CLI `aws cloudformation deploy`, CDK `cdk deploy`, or a waiter on UpdateStack) blocks indefinitely instead of getting UPDATE_COMPLETE or a failure reason, and the role change it just applied never becomes effective for the deployed function. Every subsequent step of a deployment pipeline — and any test that asserts a least-privilege role was widened correctly — stalls rather than passing or failing.

Fix: the CloudFormation stack-update state machine and the IAM-role/Lambda resource providers it drives (the UpdateStack handler and the per-resource update path that must signal completion back to the stack), plus whatever propagates an updated execution-role policy to the Lambda invocation path so the in-function s3vectors call is re-authorized. The stack must always reach a terminal status, and an updated role must take effect for later invokes.

Related but not covering this check: open issue 20261106T225443Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-create-and.md describes the same never-settling stack lifecycle but carries only the replay keys for cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive and cfn-stack-update-add-bedrock-agent-permission-positive; the s3vectors-permission update check below is not in it, and the suite did not mark it OWNED. Open issue 20261106T131020Z-bedrock-agent-runtime-lambda-s3vectors-the-lambda-python.md owns the sibling in-function-SDK failures in this group.

Traces: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive.

## Evidence
Replayed failure under the suite heading `### bedrock-agent-runtime__lambda__s3vectors`, marked neither ALREADY FILED nor OWNED:

  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive
  Error: Check timed out after 60.0s

Check metadata: CHECK = "cfn-stack-update-add-s3vectors-permission-positive", GROUP = "deployment-order", SERVICES = [bedrock-agent-runtime, lambda, s3vectors].

Expected (AWS / recording):
1. CreateStack with the template that omits the s3vectors statement reaches CREATE_COMPLETE and DescribeStacks reports it; Outputs.FunctionName names the deployed function.
2. Invoking that function returns {"s3vectors": {"ok": false, "error_code": "AccessDeniedException"}, "bedrock": {"ok": true, "count": <int>}} — the s3vectors refusal carries error code AccessDeniedException exactly.
3. UpdateStack with the template that adds {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} reaches UPDATE_COMPLETE.
4. The identical invoke then returns {"s3vectors": {"ok": true, "count": <int>}}, and a second back-to-back invoke returns the same count.

Actual (emulator): the check never reaches a verdict on any of those four steps; it is killed at the harness's 60.0s limit while polling. No terminal stack status and no allowed s3vectors call is ever observed, and no error is returned for the client to handle.

Suite totals for context: 87/136 passed; this is the only failing check in the run that the report did not mark ALREADY FILED or OWNED.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the single check bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive.

By hand:
1. CreateStack with a template containing an AWS::IAM::Role (assume-role for lambda.amazonaws.com, inline policy allowing only logs:CreateLogGroup/CreateLogStream/PutLogEvents and bedrock:ListSessions) and an AWS::Lambda::Function (python3.12, index.handler, Role = Fn::GetAtt ExecRole.Arn, inline ZipFile handler that calls s3vectors ListVectorBuckets and bedrock-agent-runtime ListSessions and reports ok/error_code for each), with Outputs.FunctionName = Ref Fn, Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM].
2. Poll DescribeStacks until a terminal status — observe it never becomes CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE within the window.
3. Invoke the function and confirm s3vectors reports error_code AccessDeniedException.
4. UpdateStack with the same template plus the statement {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} and poll DescribeStacks for UPDATE_COMPLETE.
5. Invoke twice more and confirm s3vectors.ok is true both times with an equal integer count.

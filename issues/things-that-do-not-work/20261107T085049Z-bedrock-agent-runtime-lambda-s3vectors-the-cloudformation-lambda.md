Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive|_poll_until#4|Timeout|raise AssertionError(
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Commit: 9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83

# bedrock-agent-runtime__lambda__s3vectors: the CloudFormation Lambda+execution-role stack lifecycle never converges, so all four CFN deploy/update checks are killed at the 60s harness budget

> _Note: this issue was identified against the code base at commit `9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: every check in this group that provisions a Lambda function plus its IAM execution role through CloudFormation and then drives the deployed function reaches no verdict at all — the check is killed by the harness at its 60.0s budget while still polling, so neither a terminal stack status nor the deployed function's reply is ever observed. Failing check ids (4): bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive, bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative, bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive, bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive.

AWS: Smithy (cloudformation CreateStack, input StackName required, TemplateBody @length min 1, Capabilities list, Tags @length 0..50; modeled errors AlreadyExistsException 400, InsufficientCapabilitiesException 400, LimitExceededException 400, TokenAlreadyExistsException 400 — none of which applies to these accepted templates) accepts the template and the stack then advances to a TERMINAL status that DescribeStacks reports: CREATE_COMPLETE, or CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED. Smithy (cloudformation UpdateStack, same required StackName / TemplateBody constraints; modeled errors only InsufficientCapabilitiesException 400 and TokenAlreadyExistsException 400) likewise settles on UPDATE_COMPLETE or UPDATE_ROLLBACK_COMPLETE / UPDATE_FAILED / UPDATE_ROLLBACK_FAILED. Smithy (cloudformation DescribeStacks, output Stacks, paginated on NextToken, no modeled errors beyond the service-common ValidationError for an absent stack) always answers, so a polling client always terminates. Recorded behavior only for the IAM effect the checks then assert: a deployed function whose role carries the statement answers its in-function call successfully, and one whose role omits it reports the service's refusal with error code AccessDeniedException exactly (the handler records {"ok": false, "error_code": "AccessDeniedException"}); after an UpdateStack that adds {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} (resp. bedrock:ListSessions) the identical call succeeds. The requirement there is the error code AccessDeniedException and the deny-then-allow transition; the stack name, function name and the returned counts are values taken from the request and the emulator's state, never literals from one run.

Emulator: nothing is refused and nothing settles. Each of the four checks ends as "Error: Check timed out after 60.0s" with no exception and no response to diagnose: the emulator accepts CreateStack/UpdateStack, and then the client's DescribeStacks poll for a terminal status (_poll_until over describe_stacks, 5s interval) and/or the subsequent lambda Invoke poll on the stack's Outputs.FunctionName never satisfies its predicate inside the window. This is strictly worse than an error status: there is no failure for a client to handle.

Why: a client that models a Lambda deployment, or a least-privilege role widening, as a CloudFormation stack create/update — `aws cloudformation deploy`, CDK `cdk deploy`, or a stack-create/stack-update waiter — blocks indefinitely instead of receiving CREATE_COMPLETE/UPDATE_COMPLETE or a failure reason, and every downstream step of the pipeline stalls. The negative arms are affected too: the mismatched-execution-role scenario can never show that the under-privileged function is denied with AccessDeniedException while the permitted twin in the same stack succeeds, so an emulator user cannot distinguish a correct IAM boundary from a broken one.

Fix: the cloudformation service's stack create/update state machine and the per-resource providers it drives for AWS::IAM::Role and AWS::Lambda::Function (the CreateStack/UpdateStack handlers and whatever must signal per-resource completion back to the stack so DescribeStacks reports a terminal status), plus the path that makes an updated execution-role policy effective for later lambda Invoke calls. Not the DescribeStacks serializer: the response shape is not what diverges — the stack never leaves its in-progress state.

Related but not covering these checks: open issues 20261106T225443Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-create-and.md and 20261107T054220Z-bedrock-agent-runtime-lambda-s3vectors-a-stack-update.md describe this same never-settling lifecycle, and 20261105T105400Z-bedrock-agent-runtime-lambda-s3vectors-2-net-new.md lists these check ids against the earlier in-function-SDK failure signature; this run's report marked none of the four checks ALREADY FILED or OWNED, so their current replay keys (a 60s timeout, not an AssertionError) are not carried by any open issue. If the lifecycle is repaired centrally, re-replay the four check ids listed here to confirm.

Traces: the four failing check ids above; see trace_ids.

## Evidence
All four are replayed failures under the suite heading `### bedrock-agent-runtime__lambda__s3vectors` (87/136 passed overall), marked neither ALREADY FILED nor OWNED:
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive — Error: Check timed out after 60.0s
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative — Error: Check timed out after 60.0s
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive — Error: Check timed out after 60.0s
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive — Error: Check timed out after 60.0s
Check metadata (all four): GROUP deployment-order / overly-permissive, SERVICES = [bedrock-agent-runtime, lambda, s3vectors]. Each calls _deploy_cfn(ctx, template, timeout_s=180) — CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and Tags, then _poll_until over cfn.describe_stacks(...)['Stacks'][0]['StackStatus'] at a 5s interval waiting for a terminal status, and only then lambda Invoke on Outputs.FunctionName (_invoke_and_parse).

Expected (AWS / recording):
1. CreateStack with an AWS::IAM::Role (assume-role for lambda.amazonaws.com plus an inline policy over logs:CreateLogGroup/CreateLogStream/PutLogEvents and, per variant, s3vectors:ListVectorBuckets and/or bedrock:ListSessions) and an AWS::Lambda::Function (Runtime python3.12, Handler index.handler, Role = Fn::GetAtt ExecRole.Arn, Timeout 15, inline Code.ZipFile) -> DescribeStacks reports a terminal status (CREATE_COMPLETE for the accepted templates) and Outputs.FunctionName names the deployed function.
2. Invoking that function returns the handler's JSON, e.g. {"s3vectors": {"ok": true, "count": <int>}, "bedrock": {"ok": true, "count": <int>}} on a permitted role and {"ok": false, "error_code": "AccessDeniedException"} for the arm whose role lacks the statement — the refusal's fixed part is the error code AccessDeniedException.
3. UpdateStack with the statement added -> UPDATE_COMPLETE, and the identical invoke then reports ok=true with an integer count.
4. In the dual-role stack, the permitted function succeeds at both services while the un-permitted one is denied at both with error code AccessDeniedException.

Actual (emulator): no verdict is reached on any of those steps in any of the four checks; each is killed at the harness's 60.0s limit while polling. No terminal stack status, no UPDATE_COMPLETE, and no allowed/denied in-function call is ever observed, and no error is returned for the client to handle. Generated stack/function/role names, request ids and wall-clock timings differ per run and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run the four checks under `bedrock-agent-runtime__lambda__s3vectors`: cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive, cfn-deploy-wrong-execution-role-mismatch-negative, cfn-stack-update-add-bedrock-agent-permission-positive, cfn-stack-update-add-s3vectors-permission-positive.

By hand:
1. CreateStack(StackName=<name>, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM], TemplateBody=a template with ExecRole (AWS::IAM::Role, assume-role for lambda.amazonaws.com, inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents plus s3vectors:ListVectorBuckets and bedrock:ListSessions) and Fn (AWS::Lambda::Function, Runtime python3.12, Handler index.handler, Role Fn::GetAtt ExecRole.Arn, Timeout 15, Code.ZipFile = a handler calling s3vectors ListVectorBuckets and bedrock-agent-runtime ListSessions and reporting ok/error_code for each), Outputs.FunctionName = Ref Fn.
2. Poll DescribeStacks(StackName=<name>) for a terminal status — observe it never becomes CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE within the poll window.
3. Repeat with a dual-role template (one permitted function, one function on a role with neither permission) and confirm the un-permitted function is denied at both services with error code AccessDeniedException while the permitted one succeeds.
4. UpdateStack(StackName=<name>, TemplateBody=the same template with {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} (resp. bedrock:ListSessions) added) and poll DescribeStacks for UPDATE_COMPLETE — observe it never settles.
5. Invoke the function after the update and confirm the previously denied call now succeeds.

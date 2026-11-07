Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative|_poll_until#10|Timeout|raise AssertionError(
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive|_poll_until#4|Timeout|raise AssertionError(
Issue-Commit: 46dd75ad5ad4e002f82bb21fdacb370faf08a983

# bedrock-agent-runtime__lambda__s3vectors: CFN Lambda+execution-role stack create/update never reaches a terminal status and the deployed function's IAM allow/deny effect is never observable, so both remaining deployment-order checks are killed at the 60s budget

> _Note: this issue was identified against the code base at commit `46dd75ad5ad4e002f82bb21fdacb370faf08a983`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the two checks in this group that provision an IAM execution role plus a Lambda function through CloudFormation and then drive the deployed function reach no verdict at all. Each is killed by the harness at its 60.0s budget while still polling, so neither a terminal stack status nor the deployed function's allow/deny reply is ever observed. Failing check ids (2): bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative, bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive.

AWS: Smithy (cloudformation CreateStack: input StackName required/clientOptional, TemplateBody @length min 1, Capabilities list, Tags @length 0..50; output StackId; modeled errors AlreadyExistsException 400, InsufficientCapabilitiesException 400, LimitExceededException 400, TokenAlreadyExistsException 400 — none applies to these accepted templates) accepts the template, and the stack then advances to a TERMINAL status that DescribeStacks reports: CREATE_COMPLETE, or CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED. Smithy (cloudformation UpdateStack: same StackName required and TemplateBody @length min 1, output StackId, modeled errors only InsufficientCapabilitiesException 400 and TokenAlreadyExistsException 400) likewise settles on UPDATE_COMPLETE or UPDATE_ROLLBACK_COMPLETE / UPDATE_FAILED / UPDATE_ROLLBACK_FAILED. Smithy (cloudformation DescribeStacks: input StackName/NextToken, output Stacks, paginated on NextToken, no modeled errors beyond the service-common ValidationError for an absent stack) always answers, so a polling client always terminates. Recorded behavior only for the IAM effect the checks then assert: a deployed function whose execution role carries the statement answers its in-function call successfully, and one whose role omits it reports the service's refusal with error code AccessDeniedException exactly (the handler records {"ok": false, "error_code": "AccessDeniedException"}); after an UpdateStack that adds {"Effect":"Allow","Action":"bedrock:ListSessions","Resource":"*"} the identical call succeeds and returns an integer count. The requirement is the error code AccessDeniedException and the deny-then-allow transition; the stack/role/function names and the returned counts are values taken from the request and the emulator's state, never literals from one run.

Emulator: nothing settles and nothing is refused. Both checks end as "Error: Check timed out after 60.0s" with no exception and no response to diagnose: CreateStack/UpdateStack are accepted, and then the client's DescribeStacks poll for a terminal status (_poll_until over describe_stacks at a 5s interval) and/or the subsequent lambda Invoke poll on the stack's Outputs.FunctionName never satisfies its predicate inside the window. That is strictly worse than a failed stack: there is no status and no error for a client to handle.

Why: a client that models a Lambda deployment, or a least-privilege role widening, as a CloudFormation stack create/update — `aws cloudformation deploy`, CDK `cdk deploy`, or a stack-create/stack-update waiter — blocks instead of receiving CREATE_COMPLETE/UPDATE_COMPLETE or a failure reason, and every downstream pipeline step stalls. The negative arm is equally lost: the dual-role stack can never show that the under-privileged function is denied at both s3vectors and bedrock-agent-runtime with error code AccessDeniedException while its permitted twin in the same stack succeeds, so a user cannot tell a correct IAM boundary from a missing one; and the update arm can never show the denied bedrock:ListSessions call turning into a success.

Fix: the cloudformation service's stack create/update state machine and the per-resource providers it drives for AWS::IAM::Role and AWS::Lambda::Function (the CreateStack/UpdateStack handlers and whatever must signal per-resource completion back to the stack so DescribeStacks reports a terminal status), plus the path that makes an execution-role policy — original or newly updated — effective for later lambda Invoke calls out to s3vectors and bedrock-agent-runtime. Not the DescribeStacks serializer: the response shape is not what diverges, the stack never leaves its in-progress state.

Related but NOT covering these two checks: open issue 20261107T085049Z-bedrock-agent-runtime-lambda-s3vectors-the-cloudformation-lambda.md describes the same never-settling lifecycle and names these check ids, and 20261106T225443Z-bedrock-agent-runtime-lambda-s3vectors-cloudformation-create-and.md and 20261107T054220Z-bedrock-agent-runtime-lambda-s3vectors-a-stack-update.md describe neighbouring arms of it; this run's report marked these two checks neither ALREADY FILED nor OWNED, so their current replay keys are not carried by any open issue. If the lifecycle is repaired centrally, re-replay both check ids listed here to confirm.

Traces: the two failing check ids above; see trace_ids.

## Evidence
Both are replayed failures under the suite heading `### bedrock-agent-runtime__lambda__s3vectors` (87/136 passed overall, 46 mismatched / 3 errored), marked neither ALREADY FILED nor OWNED:
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-wrong-execution-role-mismatch-negative — Error: Check timed out after 60.0s
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive — Error: Check timed out after 60.0s

Check metadata: GROUP overly-permissive (first) / deployment-order (second), SERVICES = [bedrock-agent-runtime, lambda, s3vectors]. Both call _deploy_cfn(ctx, template, timeout_s=180) — CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and Tags, then _poll_until over cfn.describe_stacks(...)['Stacks'][0]['StackStatus'] at a 5s interval waiting for CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE/ROLLBACK_FAILED, and only then lambda Invoke on Outputs.FunctionName (_invoke_and_parse). The second additionally calls _update_stack_and_wait, polling for UPDATE_COMPLETE/UPDATE_ROLLBACK_COMPLETE/UPDATE_FAILED/UPDATE_ROLLBACK_FAILED.

Expected (AWS / recording):
1. CreateStack with an AWS::IAM::Role (assume-role for lambda.amazonaws.com plus an inline policy over logs:CreateLogGroup/CreateLogStream/PutLogEvents and, per arm, s3vectors:ListVectorBuckets and/or bedrock:ListSessions) and an AWS::Lambda::Function (Runtime python3.12, Handler index.handler, Role = Fn::GetAtt ExecRole.Arn, Timeout 15, inline Code.ZipFile) -> DescribeStacks reports a terminal status (CREATE_COMPLETE for these accepted templates) and Outputs names the deployed function(s).
2. cfn-deploy-wrong-execution-role-mismatch-negative: in one stack carrying GoodRole/GoodFn and BadRole/BadFn, invoking GoodFn returns {"s3vectors": {"ok": true, "count": <int>}, "bedrock": {"ok": true, "count": <int>}} while invoking BadFn (role trusts lambda.amazonaws.com but carries only the logs statements) returns ok=false for BOTH services with error_code exactly "AccessDeniedException".
3. cfn-stack-update-add-bedrock-agent-permission-positive: before the update the function's bedrock-agent-runtime ListSessions call is denied with error_code exactly "AccessDeniedException" while s3vectors ListVectorBuckets succeeds; UpdateStack adding {"Effect":"Allow","Action":"bedrock:ListSessions","Resource":"*"} -> UPDATE_COMPLETE, after which the same invoke reports bedrock ok=true with an integer count and s3vectors still ok=true.

Actual (emulator): no verdict is reached on any of those steps in either check; each is killed at the harness's 60.0s limit while polling. No terminal stack status, no UPDATE_COMPLETE, and no allowed/denied in-function call is ever observed, and no error is returned for the client to handle. Generated stack/role/function names, request ids and wall-clock timings differ per run and are not part of the requirement; the error code AccessDeniedException and the terminal stack statuses are.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run the two checks under `bedrock-agent-runtime__lambda__s3vectors`: cfn-deploy-wrong-execution-role-mismatch-negative and cfn-stack-update-add-bedrock-agent-permission-positive.

By hand (negative arm):
1. CreateStack(StackName=<name>, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM], TemplateBody=a template with GoodRole and BadRole (both AWS::IAM::Role, assume-role for lambda.amazonaws.com; GoodRole allows logs:CreateLogGroup/CreateLogStream/PutLogEvents + s3vectors:ListVectorBuckets + bedrock:ListSessions, BadRole allows only the logs actions) and GoodFn/BadFn (AWS::Lambda::Function, Runtime python3.12, Handler index.handler, Role Fn::GetAtt <Role>.Arn, Timeout 15, Code.ZipFile = a handler calling s3vectors ListVectorBuckets and bedrock-agent-runtime ListSessions and reporting ok/error_code for each), Outputs GoodFunctionName/BadFunctionName).
2. Poll DescribeStacks(StackName=<name>) for a terminal status — observe it never becomes CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE within the poll window.
3. Invoke GoodFunctionName and confirm both services answer ok=true; invoke BadFunctionName and confirm both answer ok=false with error_code "AccessDeniedException".

By hand (update arm):
4. CreateStack the same way with a single ExecRole whose policy allows the logs actions and s3vectors:ListVectorBuckets but NOT bedrock:ListSessions, plus Fn and Outputs.FunctionName = Ref Fn.
5. Invoke the function: expect s3vectors ok=true and bedrock ok=false with error_code "AccessDeniedException".
6. UpdateStack(StackName=<name>, TemplateBody=the same template with {"Effect":"Allow","Action":"bedrock:ListSessions","Resource":"*"} added, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) and poll DescribeStacks for UPDATE_COMPLETE — observe it never settles.
7. Invoke the function again and confirm the previously denied bedrock call now succeeds with an integer session count while s3vectors still succeeds.

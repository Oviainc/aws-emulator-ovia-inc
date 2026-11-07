Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive|_poll_until#4|Timeout|raise AssertionError(
Issue-Commit: 8eb87efb7c0d9225ef0465c3f1ea0697326bb256

# bedrock-agent-runtime__lambda__s3vectors: the CFN stack update that adds s3vectors:ListVectorBuckets to a Lambda execution role never settles — the check is killed at its 60s budget with no terminal stack status and no deny→allow transition

> _Note: this issue was identified against the code base at commit `8eb87efb7c0d9225ef0465c3f1ea0697326bb256`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: the deployment-order check that provisions an IAM execution role plus a Lambda function through CloudFormation, confirms the function is denied at s3vectors, then widens the role via UpdateStack and re-invokes, reaches no verdict at all. It is killed by the harness at its 60.0s budget while still polling, so neither a terminal stack status nor the deployed function's allow/deny reply is ever observed. Failing check id (1): bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive.

AWS: Smithy (cloudformation UpdateStack: input StackName required/clientOptional, TemplateBody @length min 1, Capabilities list, Tags @length 0..50, ClientRequestToken @length 1..128 pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$; output StackId; modeled errors only InsufficientCapabilitiesException 400 and TokenAlreadyExistsException 400 — neither applies to this accepted template submitted WITH [CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) accepts the update and the stack then advances to a TERMINAL status: UPDATE_COMPLETE, or UPDATE_ROLLBACK_COMPLETE / UPDATE_FAILED / UPDATE_ROLLBACK_FAILED. Smithy (cloudformation CreateStack) likewise settles on CREATE_COMPLETE or CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED. Smithy (cloudformation DescribeStacks: input StackName/NextToken, output Stacks, paginated on NextToken, no modeled errors beyond the service-common ValidationError for an absent stack) always answers, so a polling client always terminates. Recorded behavior only for the IAM effect the check then asserts: a deployed function whose execution role omits the statement reports the called service's refusal with error code AccessDeniedException exactly (the handler records {"ok": false, "error_code": "AccessDeniedException"}), and after an UpdateStack that adds {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} the identical ListVectorBuckets call succeeds and returns an integer count, stably across two back-to-back invocations. The requirement is the terminal stack statuses, the error code AccessDeniedException, and the deny-then-allow transition; the generated stack/role/function names, the returned bucket count, request ids and wall-clock timings are values taken from the request and the emulator's state, never literals from one run.

Emulator: nothing settles and nothing is refused. The check ends as "Error: Check timed out after 60.0s" with no exception and no response to diagnose: CreateStack/UpdateStack are accepted, and then the client's DescribeStacks poll for a terminal status (_poll_until over describe_stacks at a 5s interval) and/or the subsequent lambda Invoke poll on the stack's Outputs.FunctionName never satisfies its predicate inside the window. That is strictly worse than a failed stack or a refused update: there is no status and no error code for a client to handle.

Why: a client that models widening a least-privilege Lambda execution role as a CloudFormation stack update — `aws cloudformation deploy`, CDK `cdk deploy`, or a stack-update waiter — blocks instead of receiving UPDATE_COMPLETE or a failure reason, and every downstream pipeline step stalls. The behavioral contract the check exists to prove is also lost: the emulator can never show the function's s3vectors ListVectorBuckets call being denied with error code AccessDeniedException before the update and succeeding with an integer bucket count after it, so a user cannot distinguish a correctly enforced IAM boundary from one that is never evaluated.

Fix: the cloudformation service's stack create/update state machine and the per-resource providers it drives for AWS::IAM::Role and AWS::Lambda::Function (the CreateStack/UpdateStack handlers and whatever must signal per-resource completion back to the stack so DescribeStacks reports a terminal status), plus the path that makes an execution-role policy — original or newly updated — effective for later lambda Invoke calls out to s3vectors. Not the DescribeStacks serializer: the response shape is not what diverges, the stack never leaves its in-progress state.

Related but NOT covering this check: open issue 20261107T110223Z-bedrock-agent-runtime-lambda-s3vectors-cfn-lambda-execution.md describes the same never-settling lifecycle for the sibling arm (cfn-stack-update-add-bedrock-agent-permission-positive and cfn-deploy-wrong-execution-role-mismatch-negative), and 20261107T085049Z-bedrock-agent-runtime-lambda-s3vectors-the-cloudformation-lambda.md covers the full-stack create arm. This run's report marked this check neither ALREADY FILED nor OWNED, so its replay key is carried by no open issue. If the lifecycle is repaired centrally, re-replay the check id listed here to confirm.

Traces: the failing check id above; see trace_ids.

## Evidence
Replayed failure under the suite heading `### bedrock-agent-runtime__lambda__s3vectors` (75/154 passed overall, 66 mismatched / 13 errored), marked neither ALREADY FILED nor OWNED:
  [mismatch] bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-s3vectors-permission-positive — Error: Check timed out after 60.0s

Check metadata: CHECK = cfn-stack-update-add-s3vectors-permission-positive, GROUP = deployment-order, SERVICES = [bedrock-agent-runtime, lambda, s3vectors]. The check calls _deploy_cfn(ctx, template_before, timeout_s=180) — CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and Tags — then _poll_until over cfn.describe_stacks(StackName=...)['Stacks'][0]['StackStatus'] at a 5s interval for CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE/ROLLBACK_FAILED, then lambda Invoke on Outputs.FunctionName (_invoke_and_parse), then _update_stack_and_wait polling for UPDATE_COMPLETE/UPDATE_ROLLBACK_COMPLETE/UPDATE_FAILED/UPDATE_ROLLBACK_FAILED.

Expected (AWS / recording):
1. CreateStack with an AWS::IAM::Role (assume-role for lambda.amazonaws.com plus an inline policy over logs:CreateLogGroup/CreateLogStream/PutLogEvents and bedrock:ListSessions, deliberately WITHOUT s3vectors:ListVectorBuckets) and an AWS::Lambda::Function (Runtime python3.12, Handler index.handler, Role = Fn::GetAtt ExecRole.Arn, Timeout 15, inline Code.ZipFile) -> DescribeStacks reports a terminal status (CREATE_COMPLETE for this accepted template) and Outputs.FunctionName = Ref Fn names the deployed function.
2. Invoking that function returns {"s3vectors": {"ok": false, "error_code": "AccessDeniedException"}, "bedrock": {"ok": true, "count": <int>}} — the s3vectors arm refused with error code exactly "AccessDeniedException".
3. UpdateStack(StackName=<name>, TemplateBody=the same template with {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} appended, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) -> DescribeStacks reports UPDATE_COMPLETE.
4. The identical invoke now returns s3vectors ok=true with an integer count, and a second back-to-back invoke returns the SAME count.

Actual (emulator): no verdict is reached on any of those steps. The check is killed at the harness's 60.0s limit while polling; no terminal stack status (neither CREATE_* nor UPDATE_*), no AccessDeniedException, and no deny->allow transition is ever observed, and no error is returned for the client to handle. Generated stack/role/function names, the bucket count, request ids and wall-clock timings differ per run and are not part of the requirement; the terminal stack statuses and the error code AccessDeniedException are.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run the check under `bedrock-agent-runtime__lambda__s3vectors`: cfn-stack-update-add-s3vectors-permission-positive.

By hand:
1. CreateStack(StackName=<name>, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM], Tags=[...], TemplateBody=a template with ExecRole (AWS::IAM::Role, assume-role for lambda.amazonaws.com, inline policy "data-plane" allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents and bedrock:ListSessions but NOT s3vectors:ListVectorBuckets) and Fn (AWS::Lambda::Function, Runtime python3.12, Handler index.handler, Role Fn::GetAtt ExecRole.Arn, Timeout 15, Code.ZipFile = a handler that calls s3vectors ListVectorBuckets and bedrock-agent-runtime ListSessions and reports {ok, count} or {ok: false, error_code} for each), Outputs.FunctionName = Ref Fn).
2. Poll DescribeStacks(StackName=<name>) for a terminal status — observe it never becomes CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE/ROLLBACK_FAILED within the poll window.
3. Invoke Outputs.FunctionName and confirm the payload reports s3vectors ok=false with error_code exactly "AccessDeniedException" while bedrock reports ok=true with an integer count.
4. UpdateStack(StackName=<name>, TemplateBody=the same template with {"Effect":"Allow","Action":"s3vectors:ListVectorBuckets","Resource":"*"} added to the inline policy, Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM]) and poll DescribeStacks for UPDATE_COMPLETE — observe it never settles.
5. Invoke the function again and confirm the previously denied s3vectors ListVectorBuckets call now succeeds with an integer bucket count; invoke once more back-to-back and confirm the two counts agree.

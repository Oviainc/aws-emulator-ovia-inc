Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_http_until#14|Timeout|time.sleep(3.0)
Issue-Commit: 632275b31d4caea8230a511abd27a180321ca025

# cloudformation: a CDK-deployed Function+Table+LambdaRestApi stack never becomes a working deployment — the check hangs to the 60 s cap with nothing observable after `cdk deploy` (1 cause)

> _Note: this issue was identified against the code base at commit `632275b31d4caea8230a511abd27a180321ca025`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: a CloudFormation stack synthesised by CDK that holds a Lambda Function, a DynamoDB Table and a LambdaRestApi is never driven to a usable deployment — the check is killed at the harness 60 s cap, and no leg after `cdk deploy` produces any observable result. Failing check: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists.

AWS: recorded behavior only (the check is a DRAFT whose claims are hypotheses about real AWS), with model grounding for the shapes it reads. On real AWS the sequence completes: `cdk deploy` of a LegacyStackSynthesizer app sends TemplateBody (no bootstrapped environment needed) and the stack reaches CREATE_COMPLETE; DescribeStacks then returns Stacks[0].Outputs containing the four CfnOutputs the app declares (TableName, FunctionName, RestApiId, InvokeUrl), each with a populated OutputValue, and the InvokeUrl contains the stack's own RestApiId; an unsigned HTTPS POST to that stage URL is answered 200 by the AWS_PROXY integration with the handler's JSON body; the row the handler wrote is then readable with DynamoDB GetItem (ConsistentRead) and the same function is invokable directly; a path the API never declared is NOT served by the proxy integration; and after `cdk destroy --force` each resource reports its own refusal — CloudFormation DescribeStacks: ValidationError, message template `Stack with id <StackName> does not exist`; DynamoDB DescribeTable: ResourceNotFoundException, `Requested resource not found: Table: <TableName> not found`; DynamoDB PutItem on the deleted table: ResourceNotFoundException, `Requested resource not found`; Lambda GetFunction: ResourceNotFoundException, `Function not found: arn:aws:lambda:<region>:<account-id>:function:<FunctionName>`; API Gateway GetRestApi: NotFoundException, `Invalid API identifier specified <account-id>:<rest-api-id>`. In each of those messages the quoted wording is the fixed text; the stack name, table name, function name/ARN, account id, region and REST API id are values taken from the request and from the emulator's own state, never literals from one run. Smithy (com-amazonaws-cloudformation-cloudformation.json): Output = {OutputKey, OutputValue, Description, ExportName}, so an Output with no OutputValue is not a representable successful answer.

Emulator: the check never gets far enough to report any of that. The only replay output is `Error: Check timed out after 60.0s` — no exception frame, no HTTP status, no error code. The check's own declared budgets are deploy 600 s, HTTP readiness 180 s, destroy 420 s, teardown 300 s (sum 1500 s, under the suite's 1800 s ceiling), so the 60 s kill means the run produced nothing in a window in which a local emulator should have answered a five-resource CreateStack and one HTTPS request within seconds. The external `cdk`/`node` CLIs were present (require_tools() did not fail); the stall is on the target side of those calls. Hypothesis: the stall is at one of the three polls that have no terminal condition in this emulator — DescribeStacks never reaching CREATE_COMPLETE for the synthesised template, or Outputs arriving with no OutputValue so the first assertion loops/fails, or the API Gateway stage never answering 200 so `_http_until` polls to its 180 s deadline. The replay output does not say which, which is itself part of the defect surface: no partial progress is observable.

Why: a client that deploys the single most common CDK shape (HTTP API in front of a function in front of a table) gets no working deployment and no diagnosis: no terminal stack status, no usable stage URL, and no refusal it can act on. Every CDK- or SAM-style workflow built on this chain hangs instead of failing fast, so an integration test suite against the emulator stalls rather than reporting a defect, and the post-destroy refusal taxonomy (which codes and messages a deleted stack/table/function/API return) is never exercised at all.

Fix: the CloudFormation deploy pipeline and the wiring it is supposed to realise — the stack lifecycle and terminal-status transition plus Outputs resolution in the CloudFormation crate's create_stack/describe_stacks handlers and its stack state, and the resource providers that must make the deployed API Gateway stage, its AWS::Lambda::Permission statements, the function and the table actually serviceable, together with the post-delete refusal of each of those resources. Related open issues, named for overlap but NOT relied on as cover (none carries this check id): 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md, 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md, 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261024T043741Z-apigateway-cloudformation-lambda-a-sam-synthesised-rest-api.md. This check is the CDK L2 trio end-to-end leg: HTTPS -> stage -> AWS_PROXY -> Lambda -> DynamoDB, plus destroy-and-prove-unusable.

Traces: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving; bootstrap_signal false; 122/411 checks passed overall).

[mismatch] cloudformation.cdk-deployed-l2-trio-serves-http-and-persists
  Error: Check timed out after 60.0s

This is one of only two failing checks in the whole replay that is neither marked ALREADY FILED (blocked by an unimplemented scaffold stub) nor OWNED by an existing open issue.

What the check does, and where it can stall (from the replayed source):
  - require_tools("cdk", "node") — did NOT fail, so both CLIs were available to the runner;
  - `cdk deploy <stack> --require-approval never --ci --no-version-reporting --no-color` with timeout 600 s, asserting exit code 0;
  - DescribeStacks(StackName=stack), asserting StackStatus in (CREATE_COMPLETE, UPDATE_COMPLETE) and that Outputs include TableName, FunctionName, RestApiId, InvokeUrl, and that InvokeUrl contains RestApiId;
  - an unsigned HTTPS POST {"op":"put",...} to <InvokeUrl>/ polled until 200 with a 180 s deadline, expecting body {"wrote": <key>};
  - DynamoDB GetItem(ConsistentRead=True) for that key, expecting the marker the HTTP request's handler wrote;
  - a second POST for a never-written key expecting {"marker": null}; a direct Lambda Invoke expecting {"marker": <marker>}; a GET of an undeclared path expecting the marker NOT to be served;
  - `cdk destroy --force` (420 s), then the five refusals quoted in the description (ValidationError / ResourceNotFoundException x3 / NotFoundException), and the stage URL answering anything other than 200.

Declared budgets in the check: _DEPLOY_TIMEOUT_S=600, _DESTROY_TIMEOUT_S=420, _HTTP_READY_DEADLINE_S=180, _TEARDOWN_DEADLINE_S=300. The replay reports no response, no status and no error code from the emulator — only the 60 s cap — so no leg of this chain produced an observable result.

Model grounding (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): Output = structure{OutputKey, OutputValue, Description, ExportName}; DescribeStacks returns Stacks[].Outputs, which is the only place the CDK app's four CfnOutputs are reported.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay cloudformation.cdk-deployed-l2-trio-serves-http-and-persists (give it the check's own declared budget, not the 60 s cap, so the stalling leg is identifiable).
3. Manually, with the CDK app at the check's deployments/cdk_trio: `cdk deploy <stack> --require-approval never --ci` against the emulator endpoint and watch for a terminal stack status.
4. aws cloudformation describe-stacks --stack-name <stack>: observe whether StackStatus reaches CREATE_COMPLETE and whether Outputs carry TableName/FunctionName/RestApiId/InvokeUrl each with an OutputValue.
5. POST {"op":"put","key":"k1","marker":"m1"} to <InvokeUrl>/ : observe whether the stage answers 200 with {"wrote":"k1"}, then aws dynamodb get-item --table-name <TableName> --key '{"pk":{"S":"k1"}}' --consistent-read to see whether the function's write landed.
6. `cdk destroy <stack> --force`, then describe-stacks / dynamodb describe-table / dynamodb put-item / lambda get-function / apigateway get-rest-api and compare each error code and message with the templates quoted in the description.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table
Issue-Key: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table|run#26|KeyError|env = lam.get_function_configuration(FunctionName=function)["Environment"]["Variables"]
Issue-Commit: 228033d439b60358a40317f8e8a680bd7ef66315

# cloudformation__dynamodb__lambda: a CloudFormation/CDK-deployed Lambda function serves a FunctionConfiguration with no Environment member, so its declared environment variables are unobservable

> _Note: this issue was identified against the code base at commit `228033d439b60358a40317f8e8a680bd7ef66315`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionConfiguration

## Description
1 net-new root cause (the only failing check in this suite run that is neither already filed against an unimplemented stub nor owned by an open issue).

Cause 1 of 1: GetFunctionConfiguration omits the Environment member for a function CloudFormation created from a template that declares Environment.Variables.

AWS: Smithy (service-com-amazonaws-lambda-awsgirapiservice): GetFunctionConfiguration -> GET /2015-03-31/functions/{FunctionName}/configuration, 200, output shape FunctionConfiguration, whose member `Environment` targets `EnvironmentResponse` (members `Variables`: EnvironmentVariables map, `Error`). AWS::Lambda::Function carries an `Environment` property with a `Variables` map, and CloudFormation passes it through to CreateFunction; a function created with environment variables therefore answers GetFunctionConfiguration (and GetFunction's Configuration) with `Environment.Variables` holding exactly the declared name/value pairs, and the running function sees those names in os.environ. The emulator does not refuse the call — it answers 200 — so the divergence is the missing member, not a status or error code.

Emulator: after a successful deploy (cdk deploy exits 0; the parent stack and the nested AWS::CloudFormation::Stack both report CREATE_COMPLETE and the nested stack's single AWS::DynamoDB::Table is listed), GetFunctionConfiguration for the parent stack's function returns a configuration dict with no `Environment` key at all — the check fails with `KeyError: 'Environment'` at `lam.get_function_configuration(FunctionName=function)["Environment"]["Variables"]`. The expected body is `Environment.Variables == {"TABLE_NAME": <nested table name>}`, where `TABLE_NAME` is the fixed variable name from the template and the value is the physical id CloudFormation minted for the nested table (a per-run value, not part of the requirement).

Why: a client that configures a function through CloudFormation/CDK and then reads back its configuration cannot see any environment variable: every consumer that resolves a deployed table name, queue URL or endpoint out of Environment.Variables breaks, and a handler that reads os.environ["TABLE_NAME"] cannot work, so the whole deploy-then-invoke path is unusable even though the stack reports CREATE_COMPLETE.

Fix: either the CloudFormation AWS::Lambda::Function resource provider is not forwarding the template's Environment property into the created function's state, or the Lambda function state / FunctionConfiguration serializer never emits the Environment member. Hypothesis: the property is dropped at provisioning time, because the stack otherwise settles cleanly — a fixer should check both the CFN Lambda resource provider and the GetFunctionConfiguration/GetFunction serializer. Related but NOT covering this failure: open issue 20261102T101202Z-cloudformation-dynamodb-lambda-3-net-new-root-causes.md lists this same check id under its Cause 3 (a nested child stack was not describable at all); that symptom is gone — DescribeStacks on the nested stack now succeeds and the check progresses past it — and none of that issue's replay keys match this failure, so this missing-member cause is net-new.

Traces: cloudformation__dynamodb__lambda.cdk-nested-stack-output-feeds-a-parent-function-that-writes-the-nested-table.

## Evidence
Expected: GetFunctionConfiguration answers 200 with Environment.Variables == {"TABLE_NAME": "<physical id of the nested stack's DynamoDB table>"} (fixed text: the variable name TABLE_NAME declared in the template; variable part: the table's physical id, per run). The subsequent Invoke then returns {"written": <pk>} and the item is readable from the nested table.

Actual: GetFunctionConfiguration returns 200 with no Environment member:
  File "check_cloudformation__dynamodb__lambda_cdk_nested_stack_output_feeds_a_parent_function_that_writes_the_nested_table.py", line 89, in run
    env = lam.get_function_configuration(FunctionName=function)["Environment"]["Variables"]
  KeyError: 'Environment'

Everything before that assertion passed in the emulator: `cdk deploy` exited 0, DescribeStacks reports the parent CREATE_COMPLETE, the parent's AWS::CloudFormation::Stack resource is CREATE_COMPLETE, DescribeStacks on the nested physical id returns the child with ParentId == RootId == the parent stack id, and the child holds exactly one AWS::DynamoDB::Table. Stack names, function names, table physical ids and the pk/marker values are per-run values; the requirement is that Environment.Variables is present and carries the declared variables.

## Steps to Reproduce
1. Deploy a CloudFormation/CDK app in which a parent stack holds an AWS::Lambda::Function whose Environment.Variables maps TABLE_NAME to a nested stack's DynamoDB table name (the CDK app cdk_nested_table_writer in the check's deployments/ tree: a NestedStack holding the table, plus a parent lambda_.Function with environment={"TABLE_NAME": data.table.table_name}).
2. Wait for the parent and nested stacks to reach CREATE_COMPLETE and read the nested table's physical id from DescribeStackResources.
3. Call lambda GetFunctionConfiguration(FunctionName=<the parent function>) and read Environment.Variables — the emulator's 200 response has no Environment member (KeyError), where AWS returns {"TABLE_NAME": "<nested table name>"}.
4. Invoke the function with {"pk": ..., "marker": ...}: the handler reads os.environ["TABLE_NAME"], so the write to the nested table cannot happen either.

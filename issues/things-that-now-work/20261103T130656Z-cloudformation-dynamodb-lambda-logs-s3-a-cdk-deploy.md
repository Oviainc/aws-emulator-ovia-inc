Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata
Issue-Key: cloudformation__dynamodb__lambda__logs__s3|DescribeTable|ResourceNotFoundException
Issue-Commit: 00a44e0a832af30b9f0560f7a9da7e33ea0ede80

# cloudformation__dynamodb__lambda__logs__s3: a `cdk deploy` stack reaches CREATE_COMPLETE and outputs a table name, but the declared AWS::DynamoDB::Table was never created (1 cause)

> _Note: this issue was identified against the code base at commit `00a44e0a832af30b9f0560f7a9da7e33ea0ede80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::DynamoDB::Table reaches CREATE_COMPLETE, reports a physical table name in its stack Outputs, and yet no table exists in the DynamoDB service state, so the very next DescribeTable on that reported name is refused.

AWS: CloudFormation only reports CREATE_COMPLETE for a stack once every declared resource has actually been provisioned in its owning service. For AWS::DynamoDB::Table that means the table exists in DynamoDB under the physical resource id CloudFormation assigned (the generated form `<stack-name>-<LogicalId>-<suffix>` when no TableName is given), and the value a `Fn::Ref`/CfnOutput on that table resolves to is that same physical name. DescribeTable on it must then answer 200 with `Table.TableStatus` = `ACTIVE`, `Table.KeySchema` = the declared key schema, and `Table.BillingModeSummary.BillingMode` = `PAY_PER_REQUEST` for a PAY_PER_REQUEST table. Smithy (services/service-com-amazonaws-dynamodb-dynamodb-20120810): `DescribeTable` input `TableName` (required, @length 1..1024), output `Table` -> `TableDescription`, modeled error `ResourceNotFoundException` (client, httpError 400) — that error is the correct answer only when the named table genuinely does not exist, which is exactly the state the emulator is in. Recorded behavior for the stack side: the CDK app (inlined in the check) declares `ddb.Table` with partition key `pk` (S) and `BillingMode.PAY_PER_REQUEST`, a `_lambda.Function`, `table.grant_read_write_data(fn)` and CfnOutputs `TableName`/`FunctionName`; AWS completes that deploy with all three resources real.

Emulator: `cdk bootstrap` and `cdk deploy` both exit 0; DescribeStacks reports `CREATE_COMPLETE`; DescribeStackResources lists `CDKMetadata` as `AWS::CDK::Metadata`/`CREATE_COMPLETE` and GetTemplate carries its `Analytics` property (so the previously filed AWS::CDK::Metadata rejection is gone — see the already-fixed `human-cloudformation-must-accept-aws-cdk-metadata-as-a-no-op-resou.md`); the stack Outputs do carry a `TableName`. But calling `DescribeTable` with that exact output value is refused:
  status 400, code `ResourceNotFoundException`, message `Requested resource not found: Table: <table-name> not found`.
In that message `Requested resource not found: Table: ` and ` not found` are DynamoDB's fixed text and `<table-name>` is the value taken from the request (here the name the stack itself just emitted in its Outputs); the message itself is well-formed — the divergence is that this refusal happens at all after a CREATE_COMPLETE stack reported that name. Because the call raises, the check never gets to assert the table shape, GetFunction's runtime/handler/State/Environment, the Invoke that writes the item, or the GetItem read-back; those downstream behaviors are unverified, not proven good.

Why: a client that deploys with the CDK or with raw CloudFormation treats CREATE_COMPLETE plus the stack Output as the contract that the resource is usable. Here every subsequent data-plane and control-plane call against the deployed table fails with ResourceNotFoundException, so no CDK/CloudFormation-provisioned DynamoDB estate can be used at all: IaC deploys silently produce an empty account while reporting success, and application code that reads `TABLE` out of the function environment hits a table that does not exist. A 'successful' deploy that provisions nothing is worse than a failed one, because nothing signals the failure.

Fix: the CloudFormation stack-provisioning path in the cloudformation service crate — the resource provisioner/dispatch that handles a template's `Resources` entries during CreateStack / ExecuteChangeSet must, for `AWS::DynamoDB::Table`, actually create the table in the DynamoDB service's state (key schema, attribute definitions, billing mode, status ACTIVE) and bind the created table's name as the resource's physical resource id, rather than only synthesizing a physical id and recording the resource as CREATE_COMPLETE in CloudFormation's own stack state. Deleting the stack must correspondingly delete the table (RemovalPolicy.DESTROY is declared here).

Related open issues that do NOT cover this cause: `20261102T090835Z-cloudformation-dynamodb-lambda-logs-s3-cloudformation-rejects-the-aws-cdk-metadata-resource-type.md` is about CreateChangeSet refusing the `AWS::CDK::Metadata` type, which no longer happens on this check; the various 'CREATE_COMPLETE for resources it never created' issues filed for other resource types (`20261102T090835Z-cloudformation-iam-lambda-logs-a-stack-that-reaches-create-complete-never-creates.md` for AWS::Lambda::Function, `20261102T091623Z-ecr-...` for ECR repositories, `20261102T114105Z-rds-secretsmanager-...` for AWS::RDS::DBCluster) name other resource types and other services, and none carries this check's replay key.

Traces: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata (verdict: error).

## Evidence
Failing check: cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata [error]

Expected (AWS):
  - `cdk deploy` exits 0, DescribeStacks -> StackStatus CREATE_COMPLETE (observed: matches)
  - DescribeStackResources -> CDKMetadata = (AWS::CDK::Metadata, CREATE_COMPLETE) and GetTemplate carries CDKMetadata.Properties.Analytics (observed: matches)
  - stack Outputs carry TableName and FunctionName (observed: present)
  - DescribeTable(TableName=<Outputs.TableName>) -> 200 with
      (Table.TableStatus, Table.KeySchema, Table.BillingModeSummary.BillingMode)
      == ("ACTIVE", [{"AttributeName": "pk", "KeyType": "HASH"}], "PAY_PER_REQUEST")

Actual (emulator), from the replay output:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException)
  when calling the DescribeTable operation: Requested resource not found:
  Table: <stack-name>-TableCD117FA1-<suffix> not found

i.e. HTTP 400 / `ResourceNotFoundException` / message `Requested resource not found: Table: <table-name> not found`, where `<table-name>` is verbatim the value the stack itself returned in Outputs["TableName"]. The table CloudFormation claimed to have created does not exist in DynamoDB.

Not reached because of the raise (so still unverified): GetFunction's (Runtime, Handler, State, Environment.Variables) == ("python3.12", "index.handler", "Active", {"TABLE": <table>}), Invoke returning {"written": <pk>} with no FunctionError, and GetItem returning {"pk": {"S": <pk>}, "marker": {"S": <marker>}}.

Smithy citation: DescribeTable — input TableName required @length 1..1024; output Table -> TableDescription; errors InternalServerError (500), InvalidEndpointException (421), ResourceNotFoundException (client, 400).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point the AWS CLI / CDK CLI and boto3 at it (the behavioral harness's subprocess env).
2. Run `cdk bootstrap aws://<account>/<region> --qualifier <q> --toolkit-stack-name <toolkit-stack> --bootstrap-kms-key-id AWS_MANAGED_KEY --ci --no-color --no-notices`. It exits 0.
3. Synthesize and deploy the CDK app inlined in the check (deployments/cdk_default_table_writer/app.py): a `cdk.Stack` using `DefaultStackSynthesizer(qualifier=<q>)` containing `ddb.Table` (partition key `pk` type STRING, `BillingMode.PAY_PER_REQUEST`, `RemovalPolicy.DESTROY`), a `_lambda.Function` (python3.12, handler index.handler, inline code, env {"TABLE": table.table_name}), `table.grant_read_write_data(fn)` and CfnOutputs `TableName` and `FunctionName`. Deploy with `cdk deploy <stack> --require-approval never --toolkit-stack-name <toolkit-stack> --ci --no-color --no-notices`. It exits 0.
4. `aws cloudformation describe-stacks --stack-name <stack>` -> StackStatus CREATE_COMPLETE, Outputs contain TableName and FunctionName.
5. `aws dynamodb describe-table --table-name "$(the Outputs.TableName value from step 4)"`.
   Observed: HTTP 400, ResourceNotFoundException, "Requested resource not found: Table: <that name> not found".
   Expected: 200 with Table.TableStatus=ACTIVE, Table.KeySchema=[{AttributeName: pk, KeyType: HASH}], Table.BillingModeSummary.BillingMode=PAY_PER_REQUEST.
6. Re-replay the behavioral check `cloudformation__dynamodb__lambda__logs__s3.cdk-default-deploy-function-writes-granted-table-and-carries-cdk-metadata` to confirm the fix; it must also get past the GetFunction, Invoke and GetItem assertions that step 5 currently blocks.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export
Issue-Key: cloudformation__dynamodb__s3__ssm|DescribeTable|ResourceNotFoundException
Issue-Commit: 8dff66f107a565e96cf5c76c4273d39dd30a4c27

# cloudformation__dynamodb__s3__ssm: a CREATE_COMPLETE CDK stack's AWS::DynamoDB::Table is unknown to DynamoDB — DescribeTable answers ResourceNotFoundException for the physical id DescribeStackResources reports

> _Note: this issue was identified against the code base at commit `8dff66f107a565e96cf5c76c4273d39dd30a4c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
=== Cause 1 of 1: a CloudFormation-provisioned AWS::DynamoDB::Table with a CloudFormation-generated name never reaches DynamoDB's table state ===

AWS: recorded behavior only — the Smithy model does not enumerate CloudFormation resource types, but DynamoDB's model does bound the refusal: DescribeTable takes a required TableName and models ResourceNotFoundException (client, HTTP 400) documented as "The operation tried to access a nonexistent table or index. The resource might not be specified correctly, or its status might not be ACTIVE." In the recording, `cdk deploy --all` of a two-stack app (producer declaring an AWS::DynamoDB::Table with no TableName, consumer importing the producer's export into an SSM parameter) completes, both stacks are CREATE_COMPLETE, and the table CloudFormation says it created is live in DynamoDB: DescribeTable on the PhysicalResourceId that DescribeStackResources reports for that logical resource returns Table.TableStatus == ACTIVE, and PutItem/GetItem on it round-trip. A physical id CloudFormation minted for a resource it reports as CREATE_COMPLETE must resolve in the owning service, so that read can never be the modeled not-found refusal.

Emulator: everything at the stack level now succeeds — `cdk deploy --all` exits 0, both stacks report CREATE_COMPLETE, each carries a CREATE_COMPLETE AWS::CDK::Metadata resource, the producer publishes exactly one export named `<producer-stack-name>:ExportsOutputRefTable<suffix>` whose value is the table's physical id, ListImports on that export names the consumer stack, and the consumer's SSM parameter holds the same value. The very next call, dynamodb DescribeTable on that physical id, is refused: HTTP 400 `ResourceNotFoundException` with message `Requested resource not found: Table: <table-name> not found` (fixed text `Requested resource not found: Table: ` + the requested TableName + ` not found`; the table name itself — `sim2real-cdkmeta-producer-<id>-TableCD117FA1-<suffix>` in the replay — is the CloudFormation-generated physical id taken from the request, not part of the template). So the stack minted and published a generated table name, exported it and stored it in SSM, but no table exists under that name in DynamoDB's state.

Why: the stack status and the exported/parameterised physical id are the only evidence the deployment produces, and both say the table exists. A client that provisions a table through CloudFormation or CDK and then uses it through DynamoDB — the ordinary pattern this check models, and the one the cross-stack export exists to serve — gets a 400 ResourceNotFoundException for a resource CloudFormation claims to own, on a name it was handed by CloudFormation itself. Every data-plane call on the deployed table (DescribeTable, PutItem, GetItem) is unreachable, so no CDK-deployed table can be exercised even though the deploy reports success.

Fix: the AWS::DynamoDB::Table provisioner in the cloudformation crate — when the template omits TableName it must create the table in the dynamodb service's state under the SAME generated physical id it reports through DescribeStackResources, Ref and Fn::ImportValue (and fail the resource rather than reporting CREATE_COMPLETE if it cannot), so DynamoDB's reads resolve it.

Note: open issue 20261103T061702Z-cloudformation-dynamodb-s3-ssm-cloudformation-rejects-the-aws.md carries this same check, but it is keyed to the earlier failure (`cdk deploy --all` exiting 1 on `Template format error: Unrecognized resource types: [AWS::CDK::Metadata]`), which no longer reproduces — the deploy now completes and the check fails further on, at DescribeTable, so that issue does not cover this cause.

Traces: cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export

## Evidence
cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export [error]

Expected: after `cdk deploy --all`, ddb.describe_table(TableName=<producer's AWS::DynamoDB::Table PhysicalResourceId>)["Table"]["TableStatus"] == "ACTIVE", followed by a PutItem/GetItem round-trip of {"pk": {"S": <key>}, "marker": {"S": <marker>}}.

Actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: sim2real-cdkmeta-producer-1bf047d2485b-TableCD117FA1-45CD40D7EC85 not found

Everything before that call passed in this replay, which is what localises the defect: rc == 0 for `cdk deploy --all`; both stacks CREATE_COMPLETE; CDKMetadata listed as ("AWS::CDK::Metadata", "CREATE_COMPLETE") in both stacks with Properties == ["Analytics"]; exactly one AWS::DynamoDB::Table resource in the producer; the producer's single export value equal to that table's PhysicalResourceId with the name matching `<producer>:ExportsOutputRefTable[A-Za-z0-9]+`; ListImports returning [<consumer>]; and the consumer's SSM parameter holding the same table name. Only the owning service disagrees.

## Steps to Reproduce
1. Replay cloudformation__dynamodb__s3__ssm.cdk-default-deploy-all-two-stacks-share-an-export.
2. Equivalently, without CDK: CreateStack with a template whose only resource is {"Type": "AWS::DynamoDB::Table", "Properties": {"BillingMode": "PAY_PER_REQUEST", "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}], "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}]}} — note NO TableName, so CloudFormation generates the physical id.
3. Wait for the stack to reach CREATE_COMPLETE, then read the table's PhysicalResourceId from DescribeStackResources (or Ref/an Output export).
4. Call dynamodb DescribeTable(TableName=<that physical id>): expect 200 with Table.TableStatus == ACTIVE; the emulator answers HTTP 400 ResourceNotFoundException "Requested resource not found: Table: <that physical id> not found". PutItem/GetItem on the same name are refused the same way.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-sam-lambda-secret-s3-dynamodb-full-stack
Issue-Key: dynamodb|GetItem|ResourceNotFoundException
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# dynamodb__lambda__s3__secretsmanager: a SAM (AUTO_EXPAND) stack reaches CREATE_COMPLETE and outputs a table name, but the declared AWS::DynamoDB::Table does not exist — GetItem answers ResourceNotFoundException (1 cause)

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CREATE_COMPLETE SAM-transform stack never materialises its AWS::DynamoDB::Table, so the table name the stack publishes in Outputs is unknown to DynamoDB.

AWS: a CreateStack carrying `Transform: AWS::Serverless-2016-10-31` and Capabilities CAPABILITY_AUTO_EXPAND is expanded and then provisioned resource by resource; when DescribeStacks reports StackStatus CREATE_COMPLETE (Smithy: cloudformation StackStatus enum includes CREATE_COMPLETE) every declared resource exists in its owning service. For AWS::DynamoDB::Table the physical resource id — the value `Ref` yields and therefore the value published as the stack Output — is the table name, and a DynamoDB GetItem against that name succeeds: it returns a response with no `Item` member until something writes the key. DynamoDB answers ResourceNotFoundException, HTTP 400, message `Requested resource not found` (recorded behavior; fixed text, no variable part) only for a table that does not exist. So after this stack completes, GetItem on the `TableName` output must never be a ResourceNotFoundException.

Emulator: the stack settles in CREATE_COMPLETE and its Outputs do carry `TableName` (the check's `_deploy_cfn` helper raises on any non-CREATE_COMPLETE status, and it got past that), but the first data-plane call against that name fails: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetItem operation: Requested resource not found`. The declared table was never created in DynamoDB's state (or the name the stack published is not the name the table was created under). Because the table is missing, the rest of the scenario — S3 PutObject firing the bucket's Lambda notification, the function reading the secret with GetSecretValue and writing `{pk, secret, bucket}` with PutItem — cannot be observed at all.

Why: a client relying on AWS's behavior treats CREATE_COMPLETE plus the stack Output as a contract that the resource is addressable. Here every call made with the stack's own output is refused 400 ResourceNotFoundException, so provisioning code (and any application the stack hands the table name to) breaks immediately after a deploy the emulator reported as successful; a stack that should have either created the table or failed and rolled back does neither.

Fix: the CloudFormation provider's resource create path for AWS::DynamoDB::Table (and the AWS::Serverless-2016-10-31 / AUTO_EXPAND expansion that feeds it) must actually create the table in the DynamoDB service state from the template's KeySchema/AttributeDefinitions/BillingMode, make the resource's physical id and `Ref` resolve to that created table's name, and fail the resource (CREATE_FAILED → stack rollback) when it cannot.

Related already-open issues for the same shape of defect on other headings, none of which carries this check's replay key: `20261103T130656Z-cloudformation-dynamodb-lambda-logs-s3-a-cdk-deploy.md` and `20261103T144427Z-cloudformation-dynamodb-s3-ssm-a-create-complete-cdk.md`.

Traces: dynamodb__lambda__s3__secretsmanager.deploy-sam-lambda-secret-s3-dynamodb-full-stack (error).

## Evidence
Expected (AWS): after the SAM stack reaches CREATE_COMPLETE, `dynamodb GetItem(TableName=<Outputs.TableName>, Key={'pk': {'S': 'incoming/file1.txt'}})` returns 200 (no `Item` until the notification-driven Lambda writes it), and within the poll window the item appears with `secret` equal to the secret's SecretString and `bucket` equal to the stack's bucket name.

Actual (emulator): the same GetItem is refused — HTTP 400, error code `ResourceNotFoundException`, message `Requested resource not found` — i.e. the table declared as `MyTable` (AWS::DynamoDB::Table, PAY_PER_REQUEST, HASH key `pk`) and published as the `TableName` output does not exist, even though DescribeStacks reported CREATE_COMPLETE for the stack.

## Steps to Reproduce
1. cloudformation CreateStack with Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_AUTO_EXPAND] and a template whose `Transform` is `AWS::Serverless-2016-10-31`, containing: AWS::SecretsManager::Secret, AWS::DynamoDB::Table (BillingMode PAY_PER_REQUEST, KeySchema pk HASH), an AWS::IAM::Role trusting lambda.amazonaws.com, an AWS::Serverless::Function with InlineCode and env vars SECRET_ARN={Ref MySecret} / TABLE_NAME={Ref MyTable}, an AWS::Lambda::Permission for s3.amazonaws.com, and an AWS::S3::Bucket with a LambdaConfigurations notification on s3:ObjectCreated:*; Outputs publish BucketName, TableName, SecretArn.
2. Poll DescribeStacks until terminal — the emulator reports CREATE_COMPLETE — and read Outputs.TableName / Outputs.BucketName.
3. s3 PutObject into the output bucket with key `incoming/file1.txt`.
4. dynamodb GetItem(TableName=Outputs.TableName, Key={'pk': {'S': 'incoming/file1.txt'}}) — the emulator answers 400 ResourceNotFoundException 'Requested resource not found' instead of a 200 (and, once the notification runs, the written item).

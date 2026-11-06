Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-sse-enabled-without-kms-key-uses-aws-managed-key dynamodb.awsjson-epoch-timestamp-survives-two-deserializers dynamodb.deploy-cfn-stream-arn-captures-writes dynamodb.deploy-cfn-ttl-attribute-expires-item dynamodb.deploy-cfn-gsi-queryable-after-deploy
Issue-Key: dynamodb|CreateTable|NotImplemented
Issue-Key: dynamodb.awsjson-epoch-timestamp-survives-two-deserializers|run#59|AssertionError|assert invoked.get("FunctionError") is None, (
Issue-Key: dynamodb.deploy-cfn-stream-arn-captures-writes|_deploy_cfn#3|WaiterError|waiter.wait(StackName=stack_name, WaiterConfig={"Delay": 2, "MaxAttempts": 300})
Issue-Key: dynamodb.deploy-cfn-ttl-attribute-expires-item|_deploy_cfn#3|WaiterError|waiter.wait(StackName=stack_name, WaiterConfig={"Delay": 2, "MaxAttempts": 300})
Issue-Key: dynamodb.deploy-cfn-gsi-queryable-after-deploy|run#20|AssertionError|raise AssertionError(f"table {table_name!r} still describable after its owning stack was deleted")
Issue-Commit: 77172d4b913d3cd289b60abc3e2fa4659d3ee8a2

# dynamodb: 4 net-new root causes — CreateTable with SSESpecification still answers NotImplemented, the Node.js Lambda runtime cannot resolve node: built-in modules, CFN tables carrying StreamSpecification/TimeToLiveSpecification roll the stack back, and DeleteStack leaves the table behind

> _Note: this issue was identified against the code base at commit `77172d4b913d3cd289b60abc3e2fa4659d3ee8a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
FOUR net-new root causes under the `dynamodb` service heading. None of the four failing checks is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 4: CreateTable with an SSESpecification is refused NotImplemented, so an AWS-managed-key encrypted table cannot exist.
AWS: Smithy (com.amazonaws.dynamodb) models CreateTable.SSESpecification as a structure with members Enabled (boolean), SSEType (enum, only value KMS) and KMSMasterKeyId; its documentation states that when Enabled is true and no KMSMasterKeyId is given, "server-side encryption type is set to KMS and an Amazon Web Services managed key is used", and KMSMasterKeyId is to be supplied "only ... if the key is different from the default DynamoDB key alias/aws/dynamodb". DescribeTable must then report SSEDescription with Status=ENABLED, SSEType=KMS and KMSMasterKeyArn — the ARN of a key in the caller's own account and region whose KMS KeyMetadata.KeyManager is AWS and which carries the alias alias/aws/dynamodb. CreateTable's modeled errors are InternalServerError, InvalidEndpointException, LimitExceededException and ResourceInUseException; NotImplemented is not among them.
Emulator: CreateTable(TableName=<name>, KeySchema=[pk HASH], AttributeDefinitions=[pk S], BillingMode=PAY_PER_REQUEST, SSESpecification={Enabled: true}) is refused with error code `NotImplemented` and the message `operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator` — a scaffold refusal, not an AWS error class. The table is never created, so DescribeTable's SSEDescription and the alias/aws/dynamodb key resolution are unreachable.
Why: any client that encrypts its tables at rest — the default for most IaC and the only way to exercise KMS key-state rejections against DynamoDB — cannot create a table at all, and the refusal carries a code no AWS SDK models, so retry/error handling diverges.
Fix: the CreateTable handler's SSESpecification branch (plus the table state it records and the DescribeTable serializer that renders SSEDescription). Nearest open neighbour, named so it is not re-fixed and is not mistaken for coverage of this replay key: 20261024T102105Z-dynamodb-kms-dynamodb-createtable-with-an-ssespecification-is-an-unimplemented.md (same scaffold branch reached through a customer CMK; it does not carry this check id, which is the no-KMSMasterKeyId / AWS-managed-key path).
Traces: dynamodb.create-table-sse-enabled-without-kms-key-uses-aws-managed-key.

Cause 2 of 4: the emulator's Node.js Lambda runtime cannot resolve Node's built-in modules, so every nodejs handler that imports one fails with a ReferenceError.
AWS: recorded behavior only (no Smithy member governs a runtime's module resolution). On AWS, a nodejs managed runtime is a real Node.js process: `import http from "node:http"` (and every other node: built-in) resolves, and Invoke returns a 200 with the handler's payload and NO FunctionError.
Emulator: Invoke of a nodejs function whose index.mjs does `import http from "node:http"` returns a FunctionError payload: {"errorMessage":"Error resolving module 'node:http' from 'index.mjs'","errorType":"ReferenceError","requestId":"<per-invoke uuid>","trace":[]}. The fixed text is "Error resolving module '", "' from '", "'" with the module specifier and the entry file as the variable parts; the requestId is a per-run value and not part of the requirement.
Why: the handler body never runs, so any scenario that exercises an AWS SDK for JavaScript v3 client inside Lambda (here, re-reading a DescribeTable response through the v3 deserializer) is impossible, and a client cannot tell a genuine handler bug from the runtime refusing its imports.
Fix: the Lambda nodejs runtime's module loader/resolver — node: scheme built-ins must be resolvable from the handler's entry module.
Traces: dynamodb.awsjson-epoch-timestamp-survives-two-deserializers.

Cause 3 of 4: a CloudFormation create of AWS::DynamoDB::Table carrying StreamSpecification or TimeToLiveSpecification fails and the stack ends ROLLBACK_COMPLETE.
AWS: recorded behavior only for the stack outcome. AWS::DynamoDB::Table supports StreamSpecification (StreamViewType, e.g. NEW_AND_OLD_IMAGES) and TimeToLiveSpecification (AttributeName + Enabled) as table properties; CreateStack with such a template reaches CREATE_COMPLETE, the table becomes ACTIVE, Fn::GetAtt <Table>.StreamArn resolves to the table's LatestStreamArn and TTL deletion applies to items past their TTL attribute.
Emulator: CreateStack with either property drives the stack to `ROLLBACK_COMPLETE` (the StackCreateComplete waiter matches "ROLLBACK_COMPLETE" on Stacks[].StackStatus), so neither the stream nor the TTL table is ever built.
Why: every template that declares a streamed or TTL-enabled table — the standard shape for change-data-capture and expiry — cannot be deployed, so the resources downstream of it (event source mappings, consumers) are unreachable too.
Fix: the CloudFormation AWS::DynamoDB::Table resource provider — it must accept StreamSpecification and TimeToLiveSpecification and map them onto CreateTable/UpdateTimeToLive instead of failing the resource; the stack-event failure reason for an unsupported property must name it.
Traces: dynamodb.deploy-cfn-stream-arn-captures-writes, dynamodb.deploy-cfn-ttl-attribute-expires-item.

Cause 4 of 4: DeleteStack reports the stack gone but leaves its AWS::DynamoDB::Table in place.
AWS: recorded behavior only. With no DeletionPolicy on the resource, the default is Delete: DeleteStack deletes the table, and a subsequent DescribeTable on that name is refused ResourceNotFoundException ("Requested resource not found: Table: <name> not found").
Emulator: after the owning stack is deleted, DescribeTable on the table name still succeeds — replay assertion: "table '<generated name>' still describable after its owning stack was deleted" (the name is this run's generated value, not part of the requirement).
Why: stack teardown does not reclaim resources, so names collide on the next deploy, data outlives the stack that owned it, and a client cannot use DeleteStack as the cleanup contract it is.
Fix: the CloudFormation stack-delete path for the AWS::DynamoDB::Table provider — the resource's physical id must be deleted (honouring DeletionPolicy when present) before the stack reaches DELETE_COMPLETE.
Traces: dynamodb.deploy-cfn-gsi-queryable-after-deploy.

## Evidence
Suite simparity-behavioral-aws-v1 replayed against this checkout: 630/1315 passed. Service heading `dynamodb` (544/766 passed, 222 failed); dynamodb is stood up (only iot is pending bootstrap). The four checks below are the ones in that heading that the report marks neither ALREADY FILED nor OWNED.

[error] dynamodb.create-table-sse-enabled-without-kms-key-uses-aws-managed-key
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator
  Request: CreateTable(KeySchema=[{pk,HASH}], AttributeDefinitions=[{pk,S}], BillingMode='PAY_PER_REQUEST', SSESpecification={'Enabled': True}).
  Expected: 200, then DescribeTable().Table.SSEDescription == Status ENABLED / SSEType KMS / KMSMasterKeyArn arn:aws:kms:<region>:<account>:key/<id>, and kms:DescribeKey(<id>).KeyMetadata.KeyManager == 'AWS' with alias/aws/dynamodb among its aliases.

[mismatch] dynamodb.awsjson-epoch-timestamp-survives-two-deserializers (check line 345)
  AssertionError: the JS v3 consumer failed: b'{"errorMessage":"Error resolving module \'node:http\' from \'index.mjs\'","errorType":"ReferenceError","requestId":"44f10c05-f8cc-4470-b531-2f6338db575e","trace":[]}'
  Expected: Invoke returns no FunctionError; the handler's `import http from "node:http"` resolves.

[error] dynamodb.deploy-cfn-stream-arn-captures-writes
  botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: Waiter encountered a terminal failure state: For expression "Stacks[].StackStatus" we matched expected path: "ROLLBACK_COMPLETE" at least once
  Template: one AWS::DynamoDB::Table, pk(S) HASH, BillingMode PAY_PER_REQUEST, StreamSpecification.StreamViewType NEW_AND_OLD_IMAGES; Outputs TableName (!Ref) and StreamArn (!GetAtt Table.StreamArn).
  Expected: CREATE_COMPLETE, table ACTIVE, StreamArn output usable by DescribeStream/GetRecords.

[error] dynamodb.deploy-cfn-ttl-attribute-expires-item
  botocore.exceptions.WaiterError: Waiter StackCreateComplete failed: ... matched expected path: "ROLLBACK_COMPLETE" at least once
  Template: one AWS::DynamoDB::Table with TimeToLiveSpecification (AttributeName + Enabled true).
  Expected: CREATE_COMPLETE, then an item past its TTL is removed while a control item with a future TTL survives.

[mismatch] dynamodb.deploy-cfn-gsi-queryable-after-deploy (check line 123)
  AssertionError: table 'sim2real-ddb-gsi-ccd2f381f29c' still describable after its owning stack was deleted
  Expected: after DeleteStack completes, DescribeTable on that name is refused ResourceNotFoundException.

Generated table/stack names, the requestId and the uuid above are this run's values filling the templates, not part of any requirement.

## Steps to Reproduce
Cause 1: boot the emulator from this checkout, point a dynamodb client at it, and call CreateTable(TableName=T, KeySchema=[{AttributeName:'pk',KeyType:'HASH'}], AttributeDefinitions=[{AttributeName:'pk',AttributeType:'S'}], BillingMode='PAY_PER_REQUEST', SSESpecification={'Enabled': true}). Observe error code NotImplemented with "operation CreateTable with SSESpecification is scaffolded but not implemented in this emulator" instead of a created table. Then (once created) DescribeTable(T) must report SSEDescription {Status:'ENABLED', SSEType:'KMS', KMSMasterKeyArn:'arn:aws:kms:<region>:<account>:key/<id>'}, and kms.DescribeKey(<id>).KeyMetadata.KeyManager must be 'AWS' with alias/aws/dynamodb in kms.ListAliases(KeyId=<id>).
  Replay: dynamodb.create-table-sse-enabled-without-kms-key-uses-aws-managed-key

Cause 2: CreateFunction with a nodejs runtime whose index.mjs begins `import http from "node:http";` and an `export const handler = async () => ({ok:true});`. Invoke it. Observe a FunctionError payload {"errorMessage":"Error resolving module 'node:http' from 'index.mjs'","errorType":"ReferenceError",...} instead of a 200 with no FunctionError.
  Replay: dynamodb.awsjson-epoch-timestamp-survives-two-deserializers

Cause 3: CreateStack with a template containing a single AWS::DynamoDB::Table (pk(S) HASH, BillingMode PAY_PER_REQUEST) plus StreamSpecification: {StreamViewType: NEW_AND_OLD_IMAGES}; wait on StackCreateComplete. Observe ROLLBACK_COMPLETE. Repeat with TimeToLiveSpecification: {AttributeName: 'expires_at', Enabled: true} instead — same rollback. Read DescribeStackEvents for the failing resource and its reason. Both must reach CREATE_COMPLETE with an ACTIVE table; !GetAtt Table.StreamArn must equal the table's LatestStreamArn, and an item whose TTL attribute is in the past must be removed.
  Replay: dynamodb.deploy-cfn-stream-arn-captures-writes, dynamodb.deploy-cfn-ttl-attribute-expires-item

Cause 4: CreateStack with a template containing one AWS::DynamoDB::Table with an explicit TableName and a GlobalSecondaryIndex, wait for CREATE_COMPLETE, then DeleteStack and wait for DELETE_COMPLETE. Call DescribeTable on that TableName: it still succeeds; it must be refused ResourceNotFoundException.
  Replay: dynamodb.deploy-cfn-gsi-queryable-after-deploy

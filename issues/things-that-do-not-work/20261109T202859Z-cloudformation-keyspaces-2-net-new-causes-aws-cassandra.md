Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__keyspaces.deploy-cassandra-keyspace-table-type-round-trip-and-delete cloudformation__keyspaces.deploy-cassandra-table-missing-keyspace-rejected-and-rolled-back
Issue-Key: cloudformation__keyspaces|CreateStack|ValidationError
Issue-Key: cloudformation__keyspaces.deploy-cassandra-table-missing-keyspace-rejected-and-rolled-back|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# cloudformation__keyspaces: 2 net-new causes — AWS::Cassandra::Type is an unregistered resource type, and an AWS::Cassandra::Table naming a non-existent keyspace reaches CREATE_COMPLETE instead of failing and rolling back

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation provider for the AWS::Cassandra::* resource types. Both are grounded in the behavioral checks listed per cause below.

--- Cause 1 of 2: AWS::Cassandra::Type is not a registered resource type, so no template that declares a Keyspaces user-defined type can be created at all. ---

AWS: CloudFormation registers AWS::Cassandra::Keyspace, AWS::Cassandra::Table and AWS::Cassandra::Type (AWS_Cassandra template reference). A template declaring a Type with KeyspaceName/TypeName/Fields is accepted by CreateStack, the resource is provisioned through the Keyspaces control plane (Smithy: keyspaces CreateType, required keyspaceName/typeName/fieldDefinitions, output keyspaceArn + typeName; errors AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400), and the resource's Ref resolves to the composite physical id "<keyspaceName>|<typeName>". Recorded behavior shows the sibling Table resource likewise Refs as "<keyspaceName>|<tableName>", and that removing the Type from the template on UpdateStack deletes the UDT while its keyspace survives, after which keyspaces GetType refuses with ResourceNotFoundException and the message "UDT <typeName> does not exist" (fixed text "UDT " + " does not exist"; the variable part is the type name from the request).

Emulator: CreateStack is refused outright with HTTP 400, error code ValidationError and the message "Template format error: Unrecognized resource types: [AWS::Cassandra::Type]" — the fixed text being "Template format error: Unrecognized resource types: [" + "]" and the variable part the list of type names the template declared that the processor does not know. AWS::Cassandra::Keyspace and AWS::Cassandra::Table are evidently registered (Cause 2's table-only template is accepted), so only the Type provider is missing.

Why: a client cannot deploy any Keyspaces UDT through infrastructure-as-code. The stack never exists, so every downstream behavior the check grades — the three Refs, GetKeyspace replicationStrategy SINGLE_REGION, GetTable ACTIVE/PAY_PER_REQUEST/schema read-back, GetType ACTIVE with its fieldDefinitions, the per-child delete on UpdateStack and the cascade on DeleteStack — is unobservable. A real template that deploys cleanly on AWS is rejected as malformed, which sends the user to debug their template rather than the emulator.

Fix: the CloudFormation resource-type registry and a new AWS::Cassandra::Type resource provider: register the type, map Properties KeyspaceName/TypeName/Fields[{FieldName,FieldType}] onto keyspaces CreateType, set the physical resource id (and therefore Ref) to "<keyspaceName>|<typeName>", and implement the delete path through DeleteType so a template update that drops the resource removes only the UDT.

Traces: cloudformation__keyspaces.deploy-cassandra-keyspace-table-type-round-trip-and-delete.

--- Cause 2 of 2: an AWS::Cassandra::Table whose KeyspaceName was never created is provisioned successfully; the stack reaches CREATE_COMPLETE instead of CREATE_FAILED + ROLLBACK_COMPLETE. ---

AWS: the AWS::Cassandra::Table provider creates the table through keyspaces CreateTable, which refuses a keyspace that does not exist (Smithy: CreateTable errors include ResourceNotFoundException, client, HTTP 404). CloudFormation surfaces that refusal as a CREATE_FAILED event on the table's logical resource, with ResourceStatusReason in the handler-reason envelope 'Resource handler returned message: "<service message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)' where the inner message is "Keyspace <keyspaceName> does not exist" and the HandlerErrorCode is InvalidRequest; the stack then rolls back and ends ROLLBACK_COMPLETE. The fixed text is "Keyspace " + " does not exist"; the variable part is the keyspace name taken from the template/request. The request token and the stack/resource ids are per-run values, not part of the requirement. The failed resource must also leave no side effect: GetKeyspace on that name still refuses with ResourceNotFoundException and the same "Keyspace <keyspaceName> does not exist" message.

Emulator: the stack ends CREATE_COMPLETE — the table resource is created even though its keyspace was never created, so there is no CREATE_FAILED event, no handler reason, and no rollback. The check asserts the terminal status and fails: "a table naming the never-created keyspace '<ks>' ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'".

Why: the provider swallows (or never performs) the keyspace-existence check, so an invalid deployment reports success. A client gets a stack it believes created a table, while the Keyspaces control plane holds either nothing or a table attached to a keyspace that does not exist — and, because CREATE_COMPLETE is reported, rollback semantics and the failure-reason plumbing that operators rely on to diagnose a bad template are never exercised. Negative deployments are the primary way users validate templates, so a success here is worse than a wrong message.

Fix: the AWS::Cassandra::Table resource provider's create path — it must call the Keyspaces CreateTable handler and propagate its ResourceNotFoundException as a resource-level failure, plus the CloudFormation stack engine's failure/rollback path that turns a handler error into a CREATE_FAILED event carrying the 'Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: InvalidRequest)' reason and drives the stack to ROLLBACK_COMPLETE.

Traces: cloudformation__keyspaces.deploy-cassandra-table-missing-keyspace-rejected-and-rolled-back.

## Evidence
Cause 1 — cloudformation__keyspaces.deploy-cassandra-keyspace-table-type-round-trip-and-delete [error]:
  File "client.py", line 1094, in _make_api_call -> raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Cassandra::Type]

  Template declared: Keyspace (AWS::Cassandra::Keyspace), CassandraTable (AWS::Cassandra::Table, PartitionKeyColumns=[{id,TEXT}], BillingMode ON_DEMAND) and CassandraType (AWS::Cassandra::Type, Fields=[{FieldName: field_1, FieldType: int}]), with Outputs KeyspaceRef/TableRef/TypeRef.
  Expected: CreateStack accepted; stack reaches CREATE_COMPLETE; Ref values <ks>, <ks>|<table>, <ks>|<type>.
  Actual: CreateStack refused, HTTP 400 ValidationError, "Template format error: Unrecognized resource types: [AWS::Cassandra::Type]". No stack created.

Cause 2 — cloudformation__keyspaces.deploy-cassandra-table-missing-keyspace-rejected-and-rolled-back [mismatch]:
  File "check_cloudformation__keyspaces_deploy_cassandra_table_missing_keyspace_rejected_and_rolled_back.py", line 209, in run -> assert status == "ROLLBACK_COMPLETE"
  AssertionError: a table naming the never-created keyspace 'sim2realmissingks_9765e0bc0b98' ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'

  Expected: terminal stack status ROLLBACK_COMPLETE; exactly one CREATE_FAILED event, on LogicalResourceId 'CassandraTable', whose ResourceStatusReason parses as inner message "Keyspace <missing-keyspace-name> does not exist" with HandlerErrorCode 'InvalidRequest'; afterwards keyspaces GetKeyspace(<missing-keyspace-name>) still refuses ResourceNotFoundException "Keyspace <missing-keyspace-name> does not exist".
  Actual: terminal stack status CREATE_COMPLETE; no CREATE_FAILED event and no handler reason at all.
  (The generated keyspace name shown is a per-run value; the requirement is the message template, not that literal.)

## Steps to Reproduce
Cause 1:
1. CreateStack with a template whose Resources are: Keyspace = AWS::Cassandra::Keyspace {KeyspaceName: K}; CassandraTable = AWS::Cassandra::Table {KeyspaceName: K, TableName: TB, PartitionKeyColumns: [{ColumnName: id, ColumnType: TEXT}], BillingMode: {Mode: ON_DEMAND}, DependsOn: Keyspace}; CassandraType = AWS::Cassandra::Type {KeyspaceName: K, TypeName: TY, Fields: [{FieldName: field_1, FieldType: int}], DependsOn: Keyspace}; Outputs Ref each resource.
2. Observe: CreateStack is refused with 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Cassandra::Type]".
3. Expected instead: the stack is created and reaches CREATE_COMPLETE; Outputs are K, K|TB, K|TY; keyspaces GetType(K, TY) reports status ACTIVE with fieldDefinitions [(field_1, int)]; an UpdateStack to a keyspace-only template deletes the table and the type (GetType then refuses ResourceNotFoundException "UDT TY does not exist") while GetKeyspace(K) still answers; DeleteStack removes the keyspace.

Cause 2:
1. Pick a keyspace name KMISS and never create it.
2. CreateStack with a single resource CassandraTable = AWS::Cassandra::Table {KeyspaceName: KMISS, TableName: TB, PartitionKeyColumns: [{ColumnName: id, ColumnType: TEXT}], BillingMode: {Mode: ON_DEMAND}}.
3. Poll DescribeStacks(StackName=<StackId>) until a terminal status.
4. Observe: CREATE_COMPLETE, with no CREATE_FAILED event.
5. Expected instead: ROLLBACK_COMPLETE, with exactly one CREATE_FAILED event on 'CassandraTable' whose ResourceStatusReason is 'Resource handler returned message: "Keyspace KMISS does not exist" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)', and keyspaces GetKeyspace(KMISS) still refusing ResourceNotFoundException "Keyspace KMISS does not exist".
6. Positive control (must keep working): create keyspace KREAL via keyspaces CreateKeyspace, then deploy the identical template against KREAL — the stack reaches CREATE_COMPLETE, TableRef is "KREAL|TB", and GetTable reports ACTIVE / PAY_PER_REQUEST / partition key [id] / columns [(id, text)].

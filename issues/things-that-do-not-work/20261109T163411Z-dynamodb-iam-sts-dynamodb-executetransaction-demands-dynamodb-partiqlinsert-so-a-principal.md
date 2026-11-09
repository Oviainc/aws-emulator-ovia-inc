Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied
Issue-Key: dynamodb|ExecuteTransaction|AccessDeniedException
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# DynamoDB ExecuteTransaction demands dynamodb:PartiQLInsert, so a principal granted only dynamodb:ExecuteTransaction (plus PartiQL* on the table) is denied

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteTransaction

## Description
Cause 1 of 1: the authorization gate for PartiQL-over-transaction charges the per-statement PartiQL action against a policy that scopes the grant to ExecuteTransaction, so a correctly scoped principal is refused.

AWS: an INSERT statement issued through ExecuteTransaction under a policy whose statements authorize the transactional PartiQL path succeeds, and only a principal lacking that grant is denied. The denial AWS produces when a PartiQL action genuinely is not allowed reads `User: <principal arn> is not authorized to perform: dynamodb:<Action> on resource: <table arn> because no identity-based policy allows the dynamodb:<Action> action` — fixed wording with the principal ARN, action name and table ARN taken from the request. Smithy: dynamodb#ExecuteTransaction takes TransactStatements[].Statement and declares AccessDeniedException-class refusals via the service's common errors; which IAM actions it charges is a service fact — recorded behavior only, and the recording shows the INSERT succeeding under this policy.

Emulator: ExecuteTransaction answers `AccessDeniedException` with `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: dynamodb:PartiQLInsert on resource: <table arn> because no identity-based policy allows the dynamodb:PartiQLInsert action`, i.e. it charges dynamodb:PartiQLInsert against a policy scoped to the transaction path.

Why: a client whose policy AWS accepts is locked out of transactional PartiQL writes; it is driven to broaden the policy to satisfy the emulator, producing a grant that is wider than the one it tested, and the scoping guarantee it wanted to prove is inverted.

Fix: the action-mapping used by the DynamoDB authorization gate for ExecuteTransaction's PartiQL statements, in the ExecuteTransaction handler's authorization step.

Traces: dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied.

## Evidence
Expected: the ExecuteTransaction INSERT succeeds under the ExecuteTransaction-scoped policy.
Actual: AssertionError: ExecuteTransaction INSERT under the ExecuteTransaction-only policy failed with 'AccessDeniedException': 'User: arn:aws:sts::000000000000:assumed-role/… is not authorized to perform: dynamodb:PartiQLInsert on resource: arn:aws:dynamodb:us-west-2:000000000000:table/… because no identity-based policy allows the dynamodb:PartiQLInsert action'

## Steps to Reproduce
1. Create a table and a role whose policy grants the ExecuteTransaction-scoped PartiQL grant for that table.
2. AssumeRole and call ExecuteTransaction with a single INSERT TransactStatement.
3. Observe AccessDeniedException naming dynamodb:PartiQLInsert.

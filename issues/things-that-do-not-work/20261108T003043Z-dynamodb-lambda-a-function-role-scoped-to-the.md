Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle
Issue-Key: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# dynamodb__lambda: a function role scoped to the old table does not refuse GetItem after the handler is repointed — the call is served and fails on key schema instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
Cause 1 of 1: DynamoDB does not evaluate the caller's identity policy resource scope, so a GetItem against a table the function's role does not name is executed rather than refused.
AWS: when a Lambda's environment variable is repointed to a second table but its execution role's policy still names only the first table's ARN, the handler's `GetItem` is refused `400 AccessDeniedException` with the implicit-deny sentence `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: dynamodb:GetItem on resource: <new-table-arn> because no identity-based policy allows the dynamodb:GetItem action` — fixed text with the principal, action and resource ARN taken from the request and IAM state. The refusal happens before any key validation. Smithy: `GetItem` models `ResourceNotFoundException`, `ProvisionedThroughputExceededException`, `RequestLimitExceeded`, `InternalServerError`; `AccessDeniedException` is the service-common IAM refusal and `ValidationException` is the key/schema refusal.
Emulator: the call is authorized, reaches the new table and is refused on key schema instead — the handler reports `An error occurred (ValidationException) when calling the GetItem operation: The provided key element does not match the schema`. The error code, the HTTP class and the message are all wrong for this request.
Why: the diagnosis a client is meant to reach (the role's resource scoping is stale) is replaced by a misleading schema complaint, and more seriously resource-scoped DynamoDB policies are not enforced at all, so a function can read tables its role never granted.
Fix: the DynamoDB data-plane authorization step must evaluate the caller's identity policy (action and resource ARN) and refuse with `AccessDeniedException` before request/key validation runs.
Traces: dynamodb__lambda.e2e-lambda-env-var-repoint-denied-by-stale-role-scoping-lifecycle. Sibling checks in this service are OWNED by 20261106T151148Z-dynamodb-lambda-reserved-concurrency-is-a-global-invocation.md and others; none carries this check's replay key.

## Evidence
Expected: the handler's GetItem against the repointed table is refused `AccessDeniedException` naming `dynamodb:GetItem`, the assumed-role principal and the new table ARN.
Actual: `AssertionError: [phase 2/5 solve] the refusal should be AccessDeniedException from GetItem; the handler reported 'An error occurred (ValidationException) when calling the GetItem operation: The provided key element does not match the schema'`.

## Steps to Reproduce
1. Create two DynamoDB tables with different key schemas, and a python Lambda that GetItems from the table named by an environment variable, with a role policy whose Resource is only table A's ARN.
2. Confirm the handler reads table A.
3. Repoint the environment variable to table B without touching the role policy, invoke again, and read the handler's reported error — it must be `AccessDeniedException` for `dynamodb:GetItem` on table B's ARN; the emulator returns `ValidationException: The provided key element does not match the schema`.

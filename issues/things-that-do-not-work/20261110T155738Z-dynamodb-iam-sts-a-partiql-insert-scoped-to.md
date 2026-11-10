Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied
Issue-Key: dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied|_retry_denied#25|Timeout|time.sleep(interval_s)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# dynamodb__iam__sts: a PartiQL INSERT scoped to dynamodb:PartiQLInsert/ExecuteTransaction never reaches a verdict — the deny probe hangs out its budget

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement, ExecuteTransaction

## Description
Cause 1 of 1: a PartiQL write authorized through an assumed role whose policy scopes the PartiQL actions produces no observable outcome — neither the allowed write nor the expected denial.

AWS: DynamoDB's PartiQL surface authorizes per statement verb — `dynamodb:PartiQLInsert` for an INSERT through ExecuteStatement, and `dynamodb:ExecuteTransaction` (plus the per-statement PartiQL action) for the same INSERT inside ExecuteTransaction. A role granted only the transaction-scoped action is refused on the bare ExecuteStatement with HTTP 400 `AccessDeniedException` (message of the form `User: <assumed-role ARN> is not authorized to perform: dynamodb:PartiQLInsert on resource: <table ARN> because no identity-based policy allows the dynamodb:PartiQLInsert action` — fixed frame, with the principal ARN, action and resource ARN taken from the request and the policy), while the permitted path succeeds. Smithy: ExecuteStatement and ExecuteTransaction are modelled operations of the dynamodb service; the per-verb IAM action names and the AccessDeniedException frame are AWS documented behavior.

Emulator: the check ends with `Check timed out after 60.0s` — no allow, no deny and no error is observed, so there is no status, code or message to compare. Hypothesis: one of the calls on the assumed-role path never returns a terminal response (the assumed-role credential path or the PartiQL authorization decision blocks), rather than being answered.

Why: a client cannot tell whether a narrowly scoped PartiQL policy is enforced — the over-permissive case (an INSERT that should be denied succeeding) and the correct case are indistinguishable because neither call completes. Least-privilege policies for PartiQL cannot be validated against the emulator at all.

Fix: the PartiQL authorization path in the dynamodb service crate (per-statement-verb action mapping for ExecuteStatement vs ExecuteTransaction and the AccessDeniedException it must return), plus whatever in the assumed-role request path fails to produce a terminal response.

Traces: dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied.

## Evidence
Expected: an assumed role granted only the ExecuteTransaction-scoped PartiQL action is refused on a bare ExecuteStatement INSERT with HTTP 400 AccessDeniedException naming the principal, dynamodb:PartiQLInsert and the table ARN, while the transaction-scoped path succeeds.
Actual: `Error: Check timed out after 60.0s` — neither call returns a terminal response.

## Steps to Reproduce
1. Create a table; create an IAM role trusted by the caller with a policy granting dynamodb:ExecuteTransaction (and the PartiQL action) but not the bare PartiQL INSERT path, and AssumeRole into it.
2. With the assumed-role credentials call ExecuteStatement with an INSERT statement against the table.
3. Observe that neither an AccessDeniedException nor a success is returned inside the budget.
Replay dynamodb__iam__sts.xsvc-dynamodb-partiql-insert-scoped-to-execute-transaction-denied.

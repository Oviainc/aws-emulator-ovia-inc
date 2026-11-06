Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.execute-statement-http-endpoint-disabled-rejected rds-data.rds-data-own-cluster-execute-statement-lifecycle rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes rds-data.execute-statement-unassociated-secret-rejected rds-data.missing-iam-permission-access-denied-rejected rds-data.commit-transaction-twice-rejected rds-data.execute-statement-fabricated-transaction-id-rejected rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation rds-data.batch-execute-statement-rejects-whole-batch-on-one-failure rds-data.execute-statement-failure-aborts-transaction-and-rollback-discards-writes rds-data.named-parameters-bind-literally-and-reject-unbound-placeholders rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data.execute-statement-http-endpoint-disabled-rejected|_refusal#4|AssertionError|raise AssertionError(f"the call was ACCEPTED and returned {resp!r}")
Issue-Key: rds-data.rds-data-own-cluster-execute-statement-lifecycle|_client_error#40|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {resp!r}")
Issue-Key: rds-data.rejects-bad-resource-secret-and-database-with-distinct-codes|_refusal#5|AssertionError|raise AssertionError(
Issue-Key: rds-data.execute-statement-unassociated-secret-rejected|_rejects#7|AssertionError|raise AssertionError(f"expected DatabaseErrorException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: rds-data.missing-iam-permission-access-denied-rejected|_refusal#13|AssertionError|raise AssertionError(f"the call was ACCEPTED and returned {resp!r}")
Issue-Key: rds-data.commit-transaction-twice-rejected|run#8|AssertionError|assert error.get("Message") == "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction", f"unexpected refusal mess
Issue-Key: rds-data.execute-statement-fabricated-transaction-id-rejected|run#9|AssertionError|assert error.get("Message") == "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction", f"unexpected refusal mess
Issue-Key: rds-data.batch-execute-statement-atomic-rollback-on-constraint-violation|run#9|AssertionError|assert error.get("Code") == "DatabaseErrorException", f"expected DatabaseErrorException, got {error!r}"
Issue-Key: rds-data.batch-execute-statement-rejects-whole-batch-on-one-failure|_grade#4|AssertionError|assert _code(middle) == "DatabaseErrorException", (
Issue-Key: rds-data.execute-statement-failure-aborts-transaction-and-rollback-discards-writes|assert_db_error#4|AssertionError|assert _code(exc) == code, (
Issue-Key: rds-data.named-parameters-bind-literally-and-reject-unbound-placeholders|_grade#4|AssertionError|assert _code(unbound) == "ValidationException", (
Issue-Key: rds-data.rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed|_grade#4|AssertionError|assert after_multidim == landed, (
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# rds-data: 8 net-new root causes — ExecuteStatement runs the caller's SQL with no endpoint, resource, secret or IAM gate, and classifies the resulting refusals as BadRequestException instead of the modelled error classes

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Eight net-new causes, all in rds-data, all in the refusal path: either a call AWS refuses is ACCEPTED, or it is refused with the wrong modelled class/message. A fixer must close all eight before submitting.

Cause 1 of 8: a cluster created with the Data API HTTP endpoint disabled serves statements.
AWS: Smithy: ExecuteStatement models HttpEndpointNotEnabledException, errorKind client @httpError(400), doc "The HTTP endpoint for using RDS Data API isn't enabled for the DB cluster". AWS refuses with that code at 400 and the message "HttpEndpoint is not enabled for resource <resourceArn>." — fixed text "HttpEndpoint is not enabled for resource " + the resourceArn from the request + "." — and does not run the statement.
Emulator: HTTP 200 with a result set (records [[{longValue 1}]], numberOfRecordsUpdated 0) for `SELECT 1` on an EnableHttpEndpoint=false cluster.
Why: readiness and configuration gating built on this refusal never engages, and statements really execute against a cluster AWS would never have touched.
Fix: the resource/endpoint leg of the shared precondition chain, ahead of statement execution in the ExecuteStatement handler.
Traces: execute-statement-http-endpoint-disabled-rejected.

Cause 2 of 8: a resourceArn with no live cluster behind it (deleted, or never existing in that account/region) serves statements.
AWS: recorded behavior only for which class wins: the Data API reports such an ARN as having no HTTP endpoint — HttpEndpointNotEnabledException at 400, the message naming the ARN — and the four single-fault variants of one working call (broken cluster, malformed cluster ARN, unusable secret, bad database, bad sql shape) must come back as four PAIRWISE DISTINCT modelled codes: DatabaseErrorException, HttpEndpointNotEnabledException, SecretsErrorException and ValidationException (ValidationException "SQL is empty" pre-empts resource resolution; the bad cluster pre-empts the bad secret). No refused call writes anything.
Emulator: answers HTTP 200 with a result set after the cluster has been deleted, and accepts single-fault variants AWS refuses, so the taxonomy collapses.
Why: a client cannot tell a dead cluster, a bad ARN, a bad secret and a bad statement apart, and a lifecycle test that asserts a cluster is unusable once deleted sees it still serving SQL.
Fix: the resource-existence/ownership leg of the same precondition chain, and the ordering between SQL-shape validation, resource resolution and secret resolution.
Traces: rds-data-own-cluster-execute-statement-lifecycle, rejects-bad-resource-secret-and-database-with-distinct-codes.

Cause 3 of 8: a valid secret whose credentials do not belong to the cluster is accepted.
AWS: Smithy: ExecuteStatement models DatabaseErrorException (client, 400, doc "There was an error in processing the SQL statement"), InvalidSecretException and SecretsErrorException. AWS authenticates with the secret and refuses with DatabaseErrorException at 400, passing the engine's own authentication diagnostic through: "Access denied for user '<username from the secret>'@'<client address>' (using password: YES); Error code: 1045; SQLState: 28000" — fixed text is the "Access denied for user", "(using password: YES)", "Error code: 1045" and "SQLState: 28000" frame; the user name comes from the secret and the address is the Data API's own client address.
Emulator: HTTP 200 with a result set — the secret is never fetched or used to log in.
Why: wrong or stale credentials pass in test/CI and only fail in production; authentication regressions are invisible.
Fix: the secretArn resolution/authentication leg of the precondition chain, and the DatabaseErrorException mapping of the engine's auth failure.
Traces: execute-statement-unassociated-secret-rejected.

Cause 4 of 8: no IAM authorization is applied to the Data API, so an identity with no permissions succeeds.
AWS: Smithy: ExecuteStatement models AccessDeniedException, errorKind client @httpError(403), doc "You don't have sufficient access to perform this action". A brand-new IAM user with no attached policy is refused by implicit deny: 403 AccessDeniedException with the message "User: <caller ARN> is not authorized to perform: rds-data:ExecuteStatement on resource: <resourceArn> because no identity-based policy allows the rds-data:ExecuteStatement action" — fixed text is the "User: … is not authorized to perform: … on resource: … because no identity-based policy allows the … action" frame; the variable parts are the caller's ARN, the action and the resourceArn from the request.
Emulator: the call is ACCEPTED (the check's refusal helper reports the call returned instead of raising) for credentials of a user with no policies.
Why: every least-privilege assertion built on the Data API passes vacuously; a deployment that would be denied in production is blessed by the emulator.
Fix: the IAM authorization gate in front of the rds-data data plane (signing identity -> rds-data:<action> on the cluster resource), evaluated before the statement runs.
Traces: missing-iam-permission-access-denied-rejected.

Cause 5 of 8: a transaction in a terminal state is reported as an id that was never seen.
AWS: Smithy: TransactionNotFoundException is client @httpError(404) with the doc "The transaction ID wasn't found"; the first CommitTransaction returns transactionStatus "Transaction Committed". A second CommitTransaction on that id, or a statement on an id whose transaction was already committed/terminated, answers 404 TransactionNotFoundException with the message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" (entirely fixed text).
Emulator: 404 TransactionNotFoundException with "The transaction ID wasn't found" — the terminal transaction is deleted from state, so it cannot be told apart from an unknown id.
Why: a client that re-begins on 'unknown' but must stop on 'terminated' takes the wrong branch, and diagnostics misreport the cause.
Fix: the rds-data transaction state machine — keep committed/rolled-back/terminated transactions in a terminal state and report that state from ExecuteStatement, CommitTransaction and RollbackTransaction.
Traces: commit-transaction-twice-rejected, execute-statement-fabricated-transaction-id-rejected.

Cause 6 of 8: an engine constraint violation is surfaced as the deprecated BadRequestException carrying the backing engine's wording, not as DatabaseErrorException.
AWS: Smithy: both ExecuteStatement and BatchExecuteStatement model DatabaseErrorException (client, 400, "There was an error in processing the SQL statement"), which is the class AWS uses for an error the database raised; the message is the engine's own diagnostic for a duplicate key. Inside a transaction the failing statement also terminates the transaction, and in a batch the whole batch is refused and nothing lands.
Emulator: 400 with Code BadRequestException and the SQLite wording "UNIQUE constraint failed: <table>.<column>", on the single-statement path, the batch path and the in-transaction path alike.
Why: a client branching on DatabaseErrorException to distinguish 'the database rejected my data' from a malformed API call sees the generic code, and the message exposes a backing engine (SQLite) that is not Aurora, so retry/backoff and error-mapping code written against AWS misfires.
Fix: the engine-error-to-modelled-error mapping shared by the ExecuteStatement and BatchExecuteStatement handlers (classify as DatabaseErrorException and emit the engine's duplicate-key diagnostic in the engine's wording).
Traces: batch-execute-statement-atomic-rollback-on-constraint-violation, batch-execute-statement-rejects-whole-batch-on-one-failure, execute-statement-failure-aborts-transaction-and-rollback-discards-writes.

Cause 7 of 8: parameter-binding faults are BadRequestException with emulator wording instead of ValidationException with AWS's per-fault messages.
AWS: Smithy: SqlParameter{name, value: Field, typeHint}; the Field union declares an arrayValue arm which the service nevertheless refuses as a PARAMETER. AWS refuses each distinct binding fault at 400 with ValidationException and its own message: "Cannot find parameter: <name>" for a placeholder with no parameter (the name from the statement, without the ':'), "Duplicate parameter <name>" for a repeated parameter name, "Named parameter syntax is invalid, input: <name>" for a parameter whose NAME carries the ':' prefix, and "Array parameters are not supported." for an arrayValue parameter. The rejected statement writes nothing.
Emulator: 400 with Code BadRequestException and its own sentence, e.g. "no value supplied for the named parameter :missing" where AWS says "Cannot find parameter: missing"; the faults do not come back as distinct modelled validation errors.
Why: the error class and the message are both the client's contract — a client that treats ValidationException as 'my request is wrong, do not retry' and BadRequestException as a SQL/call error misroutes every binding mistake, and cannot tell a malformed parameter NAME from a missing binding.
Fix: the parameter-binding validation in the ExecuteStatement handler, ahead of execution, with one modelled ValidationException per fault and AWS's message for each.
Traces: named-parameters-bind-literally-and-reject-unbound-placeholders.

Cause 8 of 8: an UnsupportedResultException refusal also discards the write it was shaping a result for.
AWS: Smithy: UnsupportedResultException is client @httpError(400) with the doc listing its conditions (an unsupported data type, a multidimensional array, a result that is too large). For INSERT … RETURNING <unrepresentable value>, AWS runs and COMMITS the row and only then refuses the RESULT, with messages such as "The result contains a multidimensional array.", "The result contains the unsupported data type INTERVAL.", "NaN and Infinity values are not supported" and "The result exceeds the size limit 1 MB." (fixed text, except the data-type name and the limit); the inserted row remains present. By contrast a statement refused for its own length (ValidationException "Number of characters in SQLStatement is longer than 65536") or for an unparseable secretArn never runs at all.
Emulator: the refusal is raised instead of the write — the row the statement inserted is absent afterwards, so the refused call behaved as if the statement had been rolled back.
Why: the client's state after the error differs from AWS's. Code that re-inserts on UnsupportedResultException duplicates the row against AWS, and code that reads the row back finds nothing against the emulator; 'refused result' and 'refused statement' become indistinguishable.
Fix: the ordering between statement execution/commit and result shaping in the ExecuteStatement handler — shape (and refuse) the result after the statement has been applied, and keep the effect.
Traces: rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed.

Related open issues, none of which carries these behavioral checks: 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md (same precondition family, wire corpus, other inputs), 20261029T191739Z-rds-data-committransaction-skips-the-http-endpoint-enabled.md (CommitTransaction's endpoint gate only) and 20261103T222520Z-rds-data-executestatement-result-fidelity-postgresql-typed-values.md (successful-result content).

## Evidence
Cause 1 — execute-statement-http-endpoint-disabled-rejected: AssertionError: the call was ACCEPTED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'records': [[{'longValue': 1}]], 'numberOfRecordsUpdated': 0}; expected Code HttpEndpointNotEnabledException with Message "HttpEndpoint is not enabled for resource <resourceArn>." (the positive control on the endpoint-enabled sibling cluster must still return 1).
Cause 2 — rds-data-own-cluster-execute-statement-lifecycle: AssertionError: expected the call to be rejected, but it succeeded: {... 'records': [[{'longValue': 1}]], 'numberOfRecordsUpdated': 0} (expected HttpEndpointNotEnabledException, 400, naming the deleted cluster's ARN). rejects-bad-resource-secret-and-database-with-distinct-codes: AssertionError: the call was ACCEPTED and returned {'records': [[{'longValue': 1}]], 'numberOfRecordsUpdated': 0}; the check requires the four distinct codes DatabaseErrorException / HttpEndpointNotEnabledException / SecretsErrorException / ValidationException over nine single-fault variants.
Cause 3 — execute-statement-unassociated-secret-rejected: AssertionError: expected DatabaseErrorException, but the call SUCCEEDED and returned {... 'records': [[{'longValue': 1}]]}; expected 400 DatabaseErrorException with "Access denied for user 'nobody'@'<client>' (using password: YES); Error code: 1045; SQLState: 28000" (the user name comes from the secret the request names; the client address is normalised by the check).
Cause 4 — missing-iam-permission-access-denied-rejected: AssertionError: the call was ACCEPTED and returned None; expected 403 AccessDeniedException with "User: <new user's ARN> is not authorized to perform: rds-data:ExecuteStatement on resource: <cluster ARN> because no identity-based policy allows the rds-data:ExecuteStatement action".
Cause 5 — commit-transaction-twice-rejected and execute-statement-fabricated-transaction-id-rejected: AssertionError: unexpected refusal message: "The transaction ID wasn't found"; expected Code TransactionNotFoundException (which matches) with Message "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction" at HTTP 404.
Cause 6 — batch-execute-statement-atomic-rollback-on-constraint-violation: AssertionError: expected DatabaseErrorException, got {'Message': 'UNIQUE constraint failed: <table>.id', 'Code': 'BadRequestException'}; batch-execute-statement-rejects-whole-batch-on-one-failure and execute-statement-failure-aborts-transaction-and-rollback-discards-writes report the same pair ("a constraint violation … must surface as the modelled DatabaseErrorException, not a generic BadRequestException").
Cause 7 — named-parameters-bind-literally-and-reject-unbound-placeholders: AssertionError: a placeholder with no parameter must be refused with ValidationException; got 'BadRequestException': 'no value supplied for the named parameter :missing' (expected ValidationException / "Cannot find parameter: missing").
Cause 8 — rds-data-unsupported-result-refusal-arrives-after-the-row-is-committed: AssertionError: the refusal is raised while shaping the result, after the row was already committed, so row 2 must be present; expected ids [1, 2], got [1].

## Steps to Reproduce
1. Build and boot the emulator from this checkout; replay the behavioral suite group rds-data (or the individual checks in trace_ids).
2. Cause 1: create an Aurora Serverless v2 cluster with EnableHttpEndpoint=false, then POST /Execute `SELECT 1` on it -> expect 400 HttpEndpointNotEnabledException "HttpEndpoint is not enabled for resource <arn>."; observe 200 with records.
3. Cause 2: delete a working cluster, then POST /Execute on its ARN -> expect 400 HttpEndpointNotEnabledException; observe 200. Then send nine single-fault copies of a working call (bad cluster, malformed cluster ARN, unusable secret, bad database, empty sql) and observe that they do not produce four distinct modelled codes.
4. Cause 3: create a Secrets Manager secret holding unrelated {username,password} and POST /Execute with it against a working cluster -> expect 400 DatabaseErrorException with the engine's 1045/28000 access-denied diagnostic; observe 200.
5. Cause 4: create an IAM user with no policies and an access key, sign POST /Execute with it -> expect 403 AccessDeniedException with the implicit-deny sentence; observe success.
6. Cause 5: BeginTransaction, CommitTransaction (status "Transaction Committed"), CommitTransaction again -> expect 404 TransactionNotFoundException "INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction"; observe "The transaction ID wasn't found".
7. Cause 6: insert a duplicate primary key via POST /Execute, via POST /BatchExecute, and inside an open transaction -> expect 400 DatabaseErrorException with the engine's duplicate-key diagnostic; observe 400 BadRequestException "UNIQUE constraint failed: …".
8. Cause 7: POST /Execute with `INSERT … VALUES (:id, :missing)` binding only :id -> expect 400 ValidationException "Cannot find parameter: missing"; observe 400 BadRequestException "no value supplied for the named parameter :missing". Repeat with a duplicate parameter name, a parameter named ":v" and an arrayValue parameter.
9. Cause 8: POST /Execute `INSERT … RETURNING <a value the Data API cannot represent>` -> expect 400 UnsupportedResultException AND the row present on a following SELECT; observe the row missing.

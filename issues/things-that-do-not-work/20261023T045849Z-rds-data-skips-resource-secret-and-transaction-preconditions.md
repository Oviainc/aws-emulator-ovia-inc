Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-batch-execute-statement-parameter-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-batch-execute-statement-parameter-validation rdsdata-transaction-ops-resource-validation rdsdata-execute-statement-field-union-and-error-envelopes rdsdata-execute-statement-resource-and-sql-validation rdsdata-execute-statement-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|BatchExecuteStatement|4xx->4xx|$.message
Issue-Key: rds-data|BatchExecuteStatement|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: rds-data|BeginTransaction|4xx->2xx|$.message
Issue-Key: rds-data|BeginTransaction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rds-data|BeginTransaction|4xx->2xx|status
Issue-Key: rds-data|ExecuteStatement|4xx->2xx|$.message
Issue-Key: rds-data|ExecuteStatement|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: rds-data|ExecuteStatement|4xx->2xx|status
Issue-Key: rds-data|CommitTransaction|4xx->4xx|$.message
Issue-Key: rds-data|CommitTransaction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: rds-data|ExecuteStatement|4xx->4xx|$.message
Issue-Key: rds-data|ExecuteStatement|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: rds-data|ExecuteStatement|4xx->4xx|status
Issue-Key: rds-data|RollbackTransaction|4xx->4xx|$.message
Issue-Key: rds-data|RollbackTransaction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: rds-data|RollbackTransaction|4xx->4xx|status
Issue-Key: rds-data|BeginTransaction|2xx->2xx|$.transactionId
Issue-Commit: 0f903c65ef17614b948ed7843da2bc7f944d5d1a

# rds-data skips resource, secret and transaction preconditions and misclassifies the resulting errors

> _Note: this issue was identified against the code base at commit `0f903c65ef17614b948ed7843da2bc7f944d5d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchExecuteStatement

## Description
The RDS Data API front end runs the caller's SQL before (or instead of) validating the resource, the secret and the transaction it was given, so requests AWS refuses at the envelope level either succeed or fail with the wrong status class and error code. Ten causes, all in rds-data, all from the same gap: the per-request precondition chain (resourceArn -> HTTP-endpoint-enabled -> secretArn -> transactionId) and the error it raises. No open issue covers rds-data; the open RDS issues ('RDS omits state and existence preconditions and misclassifies the resulting errors' and 'RDS awsQuery error envelopes omit RequestId and misclassify parameter validation') are about the awsQuery rds control plane, not the restJson1 RDS Data API, and do not cover any step below.

Cause 1 of 10: BatchExecuteStatement does not refuse a cluster whose Data API HTTP endpoint is not enabled.
AWS: Smithy: BatchExecuteStatement models HttpEndpointNotEnabledException (errorKind client, httpError 400, doc 'The HTTP endpoint for using RDS Data API isn't enabled for the DB cluster'). AWS answers HTTP 400 with x-amzn-errortype HttpEndpointNotEnabledException:http://internal.amazon.com/coral/com.amazon.rdsdataservice/ and does NOT run the statement (recorded message, as an example: 'HttpEndpoint is not enabled for resource arn:aws:rds:...:cluster:<id>.').
Emulator: answers HTTP 400 with x-amzn-errortype BadRequestException, because it skipped the endpoint gate, executed the SQL against its own store and surfaced the engine failure instead (recorded example: 'no such table: sim2real_probe_t').
Why: the status happens to match but the error code does not, so a client branching on HttpEndpointNotEnabledException to tell the operator to enable the Data API instead sees a generic deprecated BadRequestException and reports a SQL fault for a cluster-configuration problem. It also means the statement is really executed against a cluster AWS would never have touched.
Fix: the resource precondition chain shared by the BatchExecuteStatement handler, ahead of statement execution.
Traces: rdsdata-batch-execute-statement-parameter-validation#task#1 (cluster of 4: task#1..task#4).

Cause 2 of 10: BeginTransaction does not refuse a cluster whose HTTP endpoint is not enabled either.
AWS: Smithy: BeginTransaction also models HttpEndpointNotEnabledException (client, httpError 400); recorded response is HTTP 400 with x-amzn-errortype HttpEndpointNotEnabledException:http://internal.amazon.com/coral/com.amazon.rdsdataservice/.
Emulator: answers HTTP 200 and mints a transaction id.
Why: the client is handed a transaction id for a cluster that cannot serve the Data API, so every later statement on that handle is issued against a resource AWS would have refused up front, and the failure surfaces late and in the wrong place.
Fix: same precondition chain, applied in the BeginTransaction handler.
Traces: rdsdata-transaction-ops-resource-validation#task#1.

Cause 3 of 10: ExecuteStatement does not validate the Secrets Manager secret named by secretArn.
AWS: Smithy: ExecuteStatement models InvalidSecretException (client, httpError 400, doc 'The Secrets Manager secret used with the request isn't valid') and SecretsErrorException. AWS refuses with HTTP 400 when the retrieved secret is not usable credentials (recorded message, as an example: "The secret retrieved isn't valid JSON.").
Emulator: answers HTTP 200 with a normal statement result - no secret is fetched or inspected at all.
Why: a client that relies on the Data API to authenticate via the secret gets a successful result for credentials AWS would have rejected, so broken or wrong-shaped secrets are never surfaced in test/CI and authentication regressions pass unnoticed.
Fix: the secretArn resolution/validation step of the shared request precondition chain.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#21 (cluster of 8, spanning rdsdata-execute-statement-field-union-and-error-envelopes#task#21..task#25 and rdsdata-execute-statement-resource-and-sql-validation#task#1, #4, #5).

Cause 4 of 10: ExecuteStatement does not report a cluster that has no DB instance to serve the request.
AWS: Smithy: ExecuteStatement models DatabaseNotFoundException and the service-common NotFoundException (client, httpError 404, doc 'The resourceArn, secretArn, or transactionId value can't be found'). AWS answers HTTP 404 when the cluster has no serving instance (recorded message, as an example: 'Cannot find DBInstance in DBCluster arn:aws:rds:...:cluster:<id>.').
Emulator: answers HTTP 200 with a result body.
Why: a client that provisions a cluster and polls ExecuteStatement until it stops 404-ing concludes the database is serving immediately; readiness gating built on that 404 never engages.
Fix: the resource-existence leg of the precondition chain in the ExecuteStatement handler.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#setup#11.

Cause 5 of 10: a resourceArn that is not an ARN is reported as BadRequestException instead of a validation fault.
AWS: recorded behavior only - no ValidationException is in the rds-data Smithy model, so this is the Coral framework validation layer. AWS answers HTTP 400 with x-amzn-errortype ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/ (recorded message, as an example: 'Invalid cluster arn: <value>').
Emulator: answers HTTP 400 with x-amzn-errortype BadRequestException (recorded example message: 'resourceArn must be an Amazon Resource Name of the form arn:<partition>:<service>:<region>:<account-id>:<resource>').
Why: the status matches but the error code does not. A client matching ValidationException to flag a malformed argument sees the deprecated BadRequestException, which the API docs describe as a SQL/call error, and reports a database fault for a client-side typo.
Fix: the input-shape validation layer that rejects malformed ARNs, and the error type it emits, for the transaction operations.
Traces: rdsdata-transaction-ops-resource-validation#task#2.

Cause 6 of 10: an invalid transactionId is reported as 404 TransactionNotFoundException instead of a 400 validation fault.
AWS: recorded behavior only (framework validation; the model has TransactionNotFoundException at httpError 404 for ids that are well-formed but unknown). For a transactionId that is not a well-formed id AWS answers HTTP 400 with x-amzn-errortype ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/ (recorded message, as an example: 'Invalid transaction ID').
Emulator: answers HTTP 404 with x-amzn-errortype TransactionNotFoundException.
Why: both the status class and the code differ, so a client that retries or re-begins a transaction on 404/TransactionNotFound will loop on a request that can never succeed, instead of surfacing the malformed argument.
Fix: ordering/branching in the transactionId validation of the ExecuteStatement handler - shape validation before store lookup.
Traces: rdsdata-execute-statement-live-cluster-round-trip#task#22.

Cause 7 of 10: RollbackTransaction looks the transaction up before validating cluster ownership, so the wrong fault wins.
AWS: recorded behavior only - AWS validates the resourceArn/account relationship first and answers HTTP 400 with x-amzn-errortype ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/ (recorded message, as an example: "The AWS account ID 000000000000 doesn't own DB cluster arn:aws:rds:...:cluster:<id>.").
Emulator: answers HTTP 404 with x-amzn-errortype TransactionNotFoundException - it resolved the transaction id first and never reached the ownership check.
Why: a client is told its transaction expired when the real problem is that the cluster belongs to another account; it retries the transaction rather than fixing the resourceArn or the credentials.
Fix: the ordering of ownership validation vs transaction lookup in the RollbackTransaction handler's precondition chain.
Traces: rdsdata-transaction-ops-resource-validation#task#3.

Cause 8 of 10: a transaction terminated by a failed statement is reported by CommitTransaction as an unknown id.
AWS: recorded behavior only - per BeginTransaction's doc a transaction is terminated when a statement in it fails. AWS keeps the terminated transaction and, at HTTP 404, distinguishes it from an id it has never seen (recorded message, as an example: 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction').
Emulator: at the same HTTP 404 it reports the generic not-found text ('The transaction ID wasn't found'), i.e. it drops the transaction record on the failing statement and keeps no terminated state.
Why: the status and code match, so this is only visible in the message - but the underlying state model differs: the emulator cannot tell 'your transaction was killed by your own failed statement' from 'that id never existed', so a client's diagnostics and its decision whether to re-begin or to fix the statement are driven off the wrong distinction.
Fix: the transaction state machine in rds-data state - mark a transaction terminated on statement failure instead of removing it, and report that state from CommitTransaction.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#28.

Cause 9 of 10: the same terminated-transaction state is missing from RollbackTransaction.
AWS: recorded behavior only - same rule as cause 8; HTTP 404 with the termination message (recorded example: 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction').
Emulator: HTTP 404 with 'The transaction ID wasn't found'.
Why: as cause 8 - a client that rolls back defensively after an error cannot distinguish a terminated transaction from an unknown one.
Fix: same transaction state machine, read by the RollbackTransaction handler.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#29.

Cause 10 of 10: BeginTransaction's transactionId does not have the shape AWS returns.
AWS: Smithy: BeginTransactionResponse.transactionId targets Id with @length max 192; recorded transactions are long opaque tokens, and the fixture asserts that shape rather than a literal value.
Emulator: returns a 32-character lowercase hex id (recorded example: '21f4c757c45a4f44a7700c808981f9b6'), which fails the shape assertion.
Why: a client or SDK test that validates the id shape, or any store sized for the real token, is exercised against an id of the wrong form; the emulator is not a faithful stand-in for the format AWS hands out.
Fix: the transaction-id generator behind the BeginTransaction handler.
Traces: rdsdata-execute-statement-field-union-and-error-envelopes#task#26 (cluster of 3, also rdsdata-execute-statement-live-cluster-round-trip#task#11 and #task#15).

## Evidence
Cause 1 (rdsdata-batch-execute-statement-parameter-validation#task#1, 400->400): $headers.x-amzn-errortype expected 'HttpEndpointNotEnabledException:http://internal.amazon.com/coral/com.amazon.rdsdataservice/', got 'BadRequestException'; $.message expected 'HttpEndpoint is not enabled for resource arn:aws:rds:us-west-2:000000000000:cluster:sim2real-probe-rdsdata-batch-9c24af10.', got 'no such table: sim2real_probe_t'. Cause 2 (rdsdata-transaction-ops-resource-validation#task#1): status expected 400, got 200; x-amzn-errortype expected 'HttpEndpointNotEnabledException:...', got none. Cause 3 (rdsdata-execute-statement-field-union-and-error-envelopes#task#21, 8 steps): status expected 400, got 200; $.message expected "The secret retrieved isn't valid JSON.", got None. Cause 4 (same fixture setup#11): status expected 404, got 200; $.message expected 'Cannot find DBInstance in DBCluster arn:aws:rds:us-west-2:000000000000:cluster:<id>.', got None. Cause 5 (rdsdata-transaction-ops-resource-validation#task#2, 400->400): x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'BadRequestException'. Cause 6 (rdsdata-execute-statement-live-cluster-round-trip#task#22): status expected 400, got 404; x-amzn-errortype expected 'ValidationException:...coral.validate/', got 'TransactionNotFoundException'. Cause 7 (rdsdata-transaction-ops-resource-validation#task#3): status expected 400, got 404; x-amzn-errortype expected 'ValidationException:...coral.validate/', got 'TransactionNotFoundException'. Cause 8/9 (field-union fixture task#28 CommitTransaction, task#29 RollbackTransaction, 404->404): $.message expected 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction', got "The transaction ID wasn't found". Cause 10 (field-union fixture task#26, 200->200): $.transactionId failed its shape assertion, got '21f4c757c45a4f44a7700c808981f9b6'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. rdsdata-batch-execute-statement-parameter-validation#task#1: POST /BatchExecute naming a cluster with the Data API HTTP endpoint disabled -> 400/BadRequestException with a SQL engine message instead of 400/HttpEndpointNotEnabledException.
3. rdsdata-transaction-ops-resource-validation#task#1: POST /BeginTransaction on the same cluster -> 200 with a transactionId instead of 400/HttpEndpointNotEnabledException.
4. rdsdata-execute-statement-field-union-and-error-envelopes#task#21: POST /Execute with a secretArn whose secret is not valid credentials -> 200 instead of 400 ("The secret retrieved isn't valid JSON.").
5. rdsdata-execute-statement-field-union-and-error-envelopes#setup#11: POST /Execute against a cluster with no serving DB instance -> 200 instead of 404.
6. rdsdata-transaction-ops-resource-validation#task#2: POST /CommitTransaction with a resourceArn that is not an ARN -> 400/BadRequestException instead of 400/ValidationException.
7. rdsdata-execute-statement-live-cluster-round-trip#task#22: POST /Execute with an invalid transactionId -> 404/TransactionNotFoundException instead of 400/ValidationException.
8. rdsdata-transaction-ops-resource-validation#task#3: POST /RollbackTransaction with a cluster the account does not own -> 404/TransactionNotFoundException instead of 400/ValidationException.
9. rdsdata-execute-statement-field-union-and-error-envelopes#task#28 and #task#29: after a statement fails inside a transaction, commit/rollback report 'The transaction ID wasn't found' instead of the terminated-transaction message at 404.
10. rdsdata-execute-statement-field-union-and-error-envelopes#task#26: POST /BeginTransaction returns a 32-hex transactionId, which fails the recorded id shape.

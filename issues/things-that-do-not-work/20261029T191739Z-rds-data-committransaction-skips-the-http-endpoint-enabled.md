Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-rejection-is-deterministic-on-repeat
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-rejection-is-deterministic-on-repeat
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|CommitTransaction|4xx->4xx|status
Issue-Related: 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md
Issue-Commit: 0e13384b9161da1d0d65830e33a6518406bea9f8

# rds-data CommitTransaction skips the HTTP-endpoint-enabled gate and answers 404 TransactionNotFoundException

> _Note: this issue was identified against the code base at commit `0e13384b9161da1d0d65830e33a6518406bea9f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CommitTransaction

## Description
One cause, in rds-data.

Cause 1 of 1: CommitTransaction does not refuse a cluster whose RDS Data API HTTP endpoint is not enabled; it resolves the transactionId first and reports the transaction as unknown. Fixture rdsdata-rejection-is-deterministic-on-repeat, step task#3 (cluster also covers task#4, the repeat of the same call that proves the wrong answer is stable).

AWS: Smithy: CommitTransaction (POST /CommitTransaction, restJson1) models HttpEndpointNotEnabledException with errorKind client and httpError 400, alongside TransactionNotFoundException at httpError 404. Input members resourceArn and secretArn are both @required (target Arn, @length min 11 max 570) and transactionId is @required (target Id, @length max 192). AWS validates the resource envelope - resourceArn, then whether that cluster has the Data API HTTP endpoint enabled - BEFORE it looks the transaction up, so a commit aimed at a cluster with the endpoint disabled is answered HTTP 400 with x-amzn-errortype HttpEndpointNotEnabledException:http://internal.amazon.com/coral/com.amazon.rdsdataservice/ and the message 'HttpEndpoint is not enabled for resource <resourceArn>.' - fixed text 'HttpEndpoint is not enabled for resource ' and a trailing '.', with the cluster ARN interpolated verbatim from the request's resourceArn (the recorded run's 'arn:aws:rds:us-west-2:000000000000:cluster:sim2real-probe-rdsdata-repeat-commit-9c24af16' is one request's value filling that slot, not part of the requirement). The transaction id is never consulted, so the answer does not depend on whether the id exists.

Emulator: answers HTTP 404 with x-amzn-errortype TransactionNotFoundException and the message "The transaction ID wasn't found" - it went straight to the transaction store, missed the endpoint-enabled precondition entirely, and reported the only thing it could not find.

Why: the status CLASS is wrong (404 instead of 400) and the error code is wrong. A client that treats TransactionNotFoundException/404 as a transient or expired-handle condition will re-begin the transaction and retry the commit in a loop against a cluster that can never serve the Data API, instead of surfacing HttpEndpointNotEnabledException and telling the operator to enable the Data API on the cluster. Because the fixture repeats the call, the emulator is consistently wrong rather than flaky, so nothing in a retry loop will ever break out of it. This is the same missing precondition that the open issue '20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions.md' files for BatchExecuteStatement (its cause 1) and BeginTransaction (its cause 2); that issue does NOT cover CommitTransaction's endpoint gate, which is why this is filed separately.

Fix: the shared per-request precondition chain read by the CommitTransaction handler - resourceArn shape, then HTTP-endpoint-enabled on the named cluster, then secretArn, and only then the transactionId lookup in rds-data transaction state. The endpoint check must run ahead of the store lookup so its 400 wins over the 404.

Traces: rdsdata-rejection-is-deterministic-on-repeat#task#3 (cluster of 2: task#3 and task#4).

## Evidence
rdsdata-rejection-is-deterministic-on-repeat#task#3 (CommitTransaction), repeated identically at #task#4: status expected 400, got 404. $.message expected 'HttpEndpoint is not enabled for resource arn:aws:rds:us-west-2:000000000000:cluster:sim2real-probe-rdsdata-repeat-commit-9c24af16.', got "The transaction ID wasn't found". $headers.x-amzn-errortype expected the shaped HttpEndpointNotEnabledException envelope, got 'TransactionNotFoundException'. Of the expected message, 'HttpEndpoint is not enabled for resource ' and the terminating '.' are fixed text; the ARN between them is the request's own resourceArn. Smithy: CommitTransaction errors include HttpEndpointNotEnabledException (client, httpError 400) and TransactionNotFoundException (client, httpError 404).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. rdsdata-rejection-is-deterministic-on-repeat#task#3: POST /CommitTransaction with resourceArn naming an Aurora cluster whose RDS Data API HTTP endpoint is NOT enabled, a valid secretArn, and any transactionId.
3. Observe HTTP 404 with x-amzn-errortype TransactionNotFoundException and body message "The transaction ID wasn't found", instead of HTTP 400 with x-amzn-errortype HttpEndpointNotEnabledException:http://internal.amazon.com/coral/com.amazon.rdsdataservice/ and message 'HttpEndpoint is not enabled for resource <resourceArn>.'
4. rdsdata-rejection-is-deterministic-on-repeat#task#4: repeat the identical request and observe the same wrong 404, confirming the misclassification is deterministic rather than a race.

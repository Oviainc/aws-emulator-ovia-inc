Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive
Issue-Key: dynamodb|Scan|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# DynamoDB Scan with a FilterExpression is an unimplemented scaffold stub (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Cause 1 of 1 — `Scan` refuses any request that carries a FilterExpression.
AWS: `Scan` accepts FilterExpression together with ExpressionAttributeNames/ExpressionAttributeValues, applies the filter server-side AFTER the scan reads items, and returns the surviving `Items` with `Count` (items returned) and `ScannedCount` (items examined) — ScannedCount >= Count. A plain Scan and a filtered Scan are the same operation; the filter is not a separate API. Modeled client errors are ProvisionedThroughputExceededException, RequestLimitExceeded, ResourceNotFoundException and InternalServerError.
Emulator: answers HTTP 400 with error code `NotImplemented` and "operation Scan with FilterExpression is scaffolded but not implemented in this emulator". `NotImplemented` is not a modeled Scan error, and an unfiltered Scan is served, so the refusal depends purely on the presence of the parameter.
Why: FilterExpression is how nearly every Scan in real code narrows its result; a client gets a non-modeled error code it cannot classify, cannot fall back (the alternative is to scan everything and filter client-side, which changes Count/ScannedCount semantics), and here it blocks verifying a DynamoDB-stream event-source mapping's BatchSize configuration, because the check reads back the processed items with a filtered Scan.
Fix: FilterExpression evaluation in the Scan handler, services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/scan.rs (and the Count/ScannedCount accounting that must accompany it).
Traces: dynamodb__events__lambda.ddb-stream-esm-batch-size-config-positive.

## Evidence
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Scan operation: operation Scan with FilterExpression is scaffolded but not implemented in this emulator". Expected: HTTP 200 with Items filtered server-side, Count = number returned and ScannedCount = number examined.

## Steps to Reproduce
1. CreateTable(t) with a simple key schema; PutItem a few items with an attribute `kind`.
2. Scan(TableName=t) -> 200 (works).
3. Scan(TableName=t, FilterExpression='#k = :v', ExpressionAttributeNames={'#k':'kind'}, ExpressionAttributeValues={':v':{'S':'a'}}) -> HTTP 400, code NotImplemented.

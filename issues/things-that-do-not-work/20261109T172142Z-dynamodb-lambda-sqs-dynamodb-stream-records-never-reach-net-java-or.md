Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.dotnet-dynamodb-stream-new-and-old-image-fidelity-via-onfailure-destination dynamodb__lambda__sqs.java-ddb-stream-new-and-old-image-reach-the-handler dynamodb__lambda__sqs.ruby-ddb-stream-new-and-old-image-reach-the-handler
Issue-Key: dynamodb__lambda__sqs.dotnet-dynamodb-stream-new-and-old-image-fidelity-via-onfailure-destination|_await_failure_records#12|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: dynamodb__lambda__sqs.java-ddb-stream-new-and-old-image-reach-the-handler|_poll_until#18|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: dynamodb__lambda__sqs.ruby-ddb-stream-new-and-old-image-reach-the-handler|_poll#17|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# DynamoDB stream records never reach .NET, Java or Ruby managed-runtime handlers

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Lambda runtime delivers DynamoDB stream events only to its Python path; .NET, Java and Ruby handlers are never fed.

AWS: a stream event source mapping delivers the same `Records` envelope — `eventName` INSERT/MODIFY/REMOVE with `dynamodb.Keys`, `NewImage`, `OldImage`, `SequenceNumber`, `StreamViewType` — to a function on any managed runtime (`dotnet8`, `java21`, `ruby3.x`), and the handler observes all three event kinds for an item that is inserted, updated and deleted. Smithy (lambda): CreateFunction input Runtime enum covers these runtimes; the ESM event envelope is runtime-independent.

Emulator: for all three runtimes the records never arrive. The Java and Ruby handlers never log the INSERT, MODIFY and REMOVE records for the item, and the .NET check never sees its handler reject a mismatched item — in each case the poll ran until the run budget ended with nothing delivered. The function exists and the mapping reports healthy; only the delivery to these runtimes is missing.

Why: any stream consumer not written in Python is silently dead. The mapping and function both report healthy while every record is lost, which is the worst possible failure mode for a data pipeline.

Fix: the Lambda runtime/event-delivery path for the .NET, Java and Ruby runtimes (runtime API loop and event-envelope marshalling), not the DynamoDB stream itself — the Python path over the same stream works.

Traces: dynamodb__lambda__sqs.dotnet-dynamodb-stream-new-and-old-image-fidelity-via-onfailure-destination, dynamodb__lambda__sqs.java-ddb-stream-new-and-old-image-reach-the-handler, dynamodb__lambda__sqs.ruby-ddb-stream-new-and-old-image-reach-the-handler. The Go runtime shows the same symptom and is filed under the dynamodb__lambda grouping.

## Evidence
Expected: the .NET/Java/Ruby handler receives INSERT, MODIFY and REMOVE records with NewImage/OldImage for the item.
Actual: TimeoutError: a mismatched item must be rejected by the handler: the run budget ended first (dotnet); TimeoutError: INSERT, MODIFY and REMOVE stream records for the item reaching the handler: the run budget ended first (java); TimeoutError: three stream records for the item reach the ruby handler: the run budget ended first (ruby).

## Steps to Reproduce
1. Create a table with StreamViewType NEW_AND_OLD_IMAGES.
2. CreateFunction with Runtime dotnet8 (then java21, then ruby3.x) and a handler that logs each record's eventName/NewImage/OldImage.
3. CreateEventSourceMapping from the table stream to the function.
4. PutItem, UpdateItem, DeleteItem on one key.
5. Observe the handler never receives any record.

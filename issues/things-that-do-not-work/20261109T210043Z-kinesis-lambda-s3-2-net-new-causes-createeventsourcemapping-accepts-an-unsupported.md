Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__s3.kinesis-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service kinesis__lambda__s3.kinesis-esm-s3-on-failure-destination-writes-the-full-invocation-record
Issue-Key: kinesis__lambda__s3.kinesis-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service|_error#8|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it")
Issue-Key: kinesis__lambda__s3.kinesis-esm-s3-on-failure-destination-writes-the-full-invocation-record|_poll_until#13|TimeoutError|raise TimeoutError(f"{description} not observed within {timeout_s:.0f}s; last saw {last!r}")
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# 2 net-new causes: CreateEventSourceMapping accepts an unsupported OnFailure destination, and a Kinesis ESM never writes its S3 OnFailure record

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two distinct root causes on the Kinesis event-source-mapping failure path. Both must be closed.

Cause 1 of 2 — CreateEventSourceMapping accepts a DestinationConfig.OnFailure.Destination whose service is not a supported on-failure destination.
AWS: Smithy models DestinationConfig.OnFailure.Destination as an ARN, and for a stream event source Lambda supports only an SQS queue, an SNS topic or an S3 bucket as the on-failure destination. An ARN naming any other service is refused at create with InvalidParameterValueException (HTTP 400) and no mapping is left behind.
Emulator: the call SUCCEEDS — the mapping is created with the unsupported destination recorded in DestinationConfig.
Why: a client's negative configuration test passes, and the bad destination is persisted; at failure time there is nowhere for Lambda to write, so failed batches are lost with no error ever surfaced at configuration time, which is where AWS surfaces it.
Fix: CreateEventSourceMapping's validation of DestinationConfig.OnFailure.Destination — resolve the ARN's service and refuse anything outside the supported set with InvalidParameterValueException before persisting the mapping.
Traces: kinesis__lambda__s3.kinesis-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service.

Cause 2 of 2 — a Kinesis ESM with an S3 on-failure destination never writes the invocation record.
AWS: when a batch exhausts its retries, Lambda writes one object under the configured S3 destination prefix containing the full invocation record (the `requestContext`, `responseContext` and the batch's `KinesisBatchInfo` with shard id, start/end sequence numbers, batch size and the approximate arrival timestamps). The object appears under the destination prefix without further action.
Emulator: no object ever appears under the ESM's S3 prefix (the check's listing stays empty until the run budget ends).
Why: the S3 on-failure destination is how a client recovers poison batches. With nothing written, failed records are unrecoverable and a consumer cannot tell a healthy stream from one that has been discarding batches.
Fix: the event-source-mapping failure path's destination writer for the S3 destination type (it must serialise the invocation record and PutObject under the configured prefix).
Traces: kinesis__lambda__s3.kinesis-esm-s3-on-failure-destination-writes-the-full-invocation-record.

## Evidence
Cause 1 — kinesis__lambda__s3.kinesis-esm-create-rejects-an-onfailure-destination-of-an-unsupported-service:
AssertionError: the call SUCCEEDED; real AWS rejects it
(expected: InvalidParameterValueException, HTTP 400, and no mapping left behind)

Cause 2 — kinesis__lambda__s3.kinesis-esm-s3-on-failure-destination-writes-the-full-invocation-record:
TimeoutError: an OnFailure object under the ESM's S3 prefix not observed within 49s; last saw []
(bucket/prefix names and the 49 s budget are per-run values; the invariant is that nothing was written.)

## Steps to Reproduce
Cause 1: CreateEventSourceMapping(EventSourceArn=<kinesis stream>, FunctionName=<fn>, StartingPosition='TRIM_HORIZON', DestinationConfig={'OnFailure': {'Destination': <ARN of a service that is not SQS/SNS/S3>}}) — AWS refuses with InvalidParameterValueException; the emulator creates the mapping.
Cause 2: CreateEventSourceMapping with DestinationConfig.OnFailure.Destination = an S3 bucket ARN (plus prefix) and MaximumRetryAttempts=0; put a record that makes the handler raise; list the destination prefix — AWS writes one invocation-record object, the emulator writes none.

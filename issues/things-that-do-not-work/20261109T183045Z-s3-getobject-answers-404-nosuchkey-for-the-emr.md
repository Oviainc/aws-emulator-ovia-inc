Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-spark-job-run-writes-s3-output
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-spark-job-run-writes-s3-output
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-checksum-crc64nvme
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# S3 GetObject answers 404 NoSuchKey for the EMR Serverless job output object that the recorded run wrote

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One cause.

Cause 1 of 1 — the object a completed EMR Serverless Spark job writes to its S3 output prefix is not readable afterwards (fixture emr-serverless-spark-job-run-writes-s3-output, step task#9).
AWS: Smithy S3 GetObject on a key that exists returns 200 with the object's bytes as the payload and the object metadata in headers — ETag, Last-Modified, Accept-Ranges: bytes, Content-Type (binary/octet-stream for an object stored without an explicit content type), the server-side-encryption header for a bucket with default encryption, and the checksum headers (x-amz-checksum-* plus x-amz-checksum-type) for an object stored with a checksum algorithm. GetObject only answers 404 NoSuchKey when no such key exists in the bucket. The recorded run shows the Spark job's output object present and readable immediately after the run reaches a successful terminal state, with its CSV payload as the body. The exact bytes, ETag, checksum value and timestamp are per-run values, not requirements; the requirement is that the key the job wrote is retrievable with 200 and the metadata headers above.
Emulator: GET of the output key returns 404 with the S3 error document Code 'NoSuchKey', Message 'The specified key does not exist.' and the requested Key, and Content-Type application/xml — so the body is an error document instead of the object's bytes, and Last-Modified, ETag, Accept-Ranges, x-amz-server-side-encryption and the checksum headers are all absent. The error code and message text themselves match AWS's wording for a genuinely missing key (fixed text 'The specified key does not exist.' with Key taken from the request, RequestId/HostId per run); the divergence is that the key is missing at all.
Why: the S3 side of any compute-writes-to-S3 flow is unverifiable. A client that submits a job, waits for it to finish and then reads the result gets NoSuchKey, which it cannot distinguish from a genuinely wrong key or prefix — so it retries or fails on a path that cannot ever succeed, and every downstream consumer of the job output (a second job, a load step, a report) is blocked.
Why (hypothesis): nothing ever writes the object. The emr-serverless job run in the same fixture terminates without executing the submitted Spark workload, so the PutObject that the real run performs never happens and S3 is correctly reporting an empty prefix. That execution gap is filed separately against emr-serverless (representative step emr-serverless-spark-job-run-writes-s3-output#task#8); this issue records the S3-side divergence, which is the step that actually fails and which must be re-replayed to confirm the output object is readable with the full 200 metadata header set.
Fix: the compute path that is expected to materialize job output into S3 must perform the object write, and the s3 GetObject handler/serializer must then serve that object with the complete metadata header set (ETag, Last-Modified, Accept-Ranges, Content-Type, server-side-encryption, checksum headers) rather than falling through to the NoSuchKey error path.
Traces: emr-serverless-spark-job-run-writes-s3-output#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 77 steps, 65 passed, score 84.4%).

Fixture emr-serverless-spark-job-run-writes-s3-output, step task#9, GetObject — expected 200, got 404:
  status: expected 200, got 404
  $: type_mismatch — expected the object's CSV payload ('0,1683\n1,1717\n2,1650\n'), got an S3 error document {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': 'sums.csv', 'RequestId': ..., 'HostId': ...}}
  $headers.content-type: expected 'binary/octet-stream', got 'application/xml'
  $headers.last-modified: expected present, got None
  $headers.etag: expected present, got None
  $headers.accept-ranges: expected 'bytes', got None
  $headers.x-amz-server-side-encryption: expected 'AES256', got None
  $headers.x-amz-checksum-type: expected 'FULL_OBJECT', got None
  $headers.x-amz-checksum-crc64nvme: expected present, got None

The recorded body bytes, ETag, checksum value and Last-Modified are per-run values quoted only to show the expected payload was the object itself, not an error document. The emulator's NoSuchKey wording matches AWS; the defect is the 404 for a key the recorded run had written. Preceding step task#8 in the same fixture shows the job run terminating FAILED without executing the workload (filed separately under emr-serverless).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture emr-serverless-spark-job-run-writes-s3-output in order: it creates the S3 bucket, creates an EMR Serverless application, starts a Spark job run whose driver writes a CSV result under the bucket's output prefix, polls the run, then at step task#9 issues GET on the output object key.
3. Observe step task#9 returns HTTP 404 with an XML error document (Code NoSuchKey, Message 'The specified key does not exist.') instead of HTTP 200 with the object bytes, and that none of Last-Modified, ETag, Accept-Ranges, x-amz-server-side-encryption or the x-amz-checksum-* headers are present.
4. Independently, list the bucket's output prefix after the run: it is empty, confirming no writer created the key.
5. Re-replay step emr-serverless-spark-job-run-writes-s3-output#task#9 after fixing; it must return 200 with the object payload and the metadata headers above.

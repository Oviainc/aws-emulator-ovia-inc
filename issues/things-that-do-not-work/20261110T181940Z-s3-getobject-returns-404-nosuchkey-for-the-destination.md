Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-to-s3-task-execution-copies-object
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-to-s3-task-execution-copies-object
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: s3|GetObject|2xx->4xx|$headers.x-amz-meta-last-modified
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Related: 20261109T183045Z-s3-getobject-answers-404-nosuchkey-for-the-emr.md
Issue-Commit: 122d2e23ed306dec374f1768a21e001304c75e9d

# S3 GetObject returns 404 NoSuchKey for the destination object of a completed DataSync transfer

> _Note: this issue was identified against the code base at commit `122d2e23ed306dec374f1768a21e001304c75e9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause, grounded in one wire divergence cluster.

=== Cause 1 of 1: an object that a terminal DataSync S3->S3 task execution copied into the destination bucket is not readable, so GetObject 404s instead of serving it ===

AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json, GetObject) binds GET /{Bucket}/{Key+} with success code 200 and the object bytes on the httpPayload member Body, and declares NoSuchKey (client, HTTP 404) as the only not-found error. AWS returns 404 NoSuchKey — fixed message text "The specified key does not exist." with the requested Key echoed and per-run RequestId/HostId as the variable parts — ONLY when no object exists at that key. After a DataSync task execution reaches a terminal successful status, the keys it transferred exist in the destination bucket and a GET of one returns 200 with the source bytes and the normal object headers: accept-ranges: bytes (AcceptRanges), Last-Modified, ETag, Content-Type carried from the stored object, the object's user metadata as x-amz-meta-*, x-amz-server-side-encryption: AES256 for a bucket with default SSE-S3, and the checksum headers x-amz-checksum-crc64nvme plus x-amz-checksum-type: FULL_OBJECT (GetObject's httpChecksum responseAlgorithms include CRC64NVME). The recorded step shows exactly that shape; the particular key, body bytes, etag and checksum value in the recording fill the template and are not themselves the requirement — the requirement is 200 with the transferred object's bytes and those headers.

Emulator: the same GET returns HTTP 404 with an XML error body `Error.Code=NoSuchKey`, `Error.Message="The specified key does not exist."` and Content-Type application/xml, i.e. the object is simply not in the emulator's S3 object state. The 404 payload itself is correctly shaped; the defect is that this key has no object behind it at all — the preceding DataSync execution reported itself finished without ever writing the transferred objects into S3, so none of the success-path headers (accept-ranges, Last-Modified, ETag, Content-Type text/plain, x-amz-checksum-crc64nvme, x-amz-checksum-type, x-amz-meta-*, x-amz-server-side-encryption) are emitted.

Why: reading the destination is the entire point of running a transfer. A client that completes a DataSync execution and then consumes the copied object — a downstream job, a verification step, a pipeline stage keyed off the destination prefix — gets NoSuchKey and concludes the data was lost, even though the control plane reported the execution finished. No end-to-end data-movement scenario can be exercised against the emulator, and a test that asserts the copy landed can never pass.

Fix: the write path into S3 object state that a DataSync task execution's transfer phase must drive — the execution engine has to materialize each transferred object (bytes, content type, user metadata and the computed ETag/CRC64NVME checksum) in the destination bucket's object store so S3's GetObject handler serves it with 200 instead of falling through to NoSuchKey. S3's GetObject handler itself needs no change beyond having the object present.

Related but not covering this: open issue 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md notes in its Cause 6 that the destination side-effect is not produced, but it is a behavioral-corpus issue with no wire replay key, and the replay report lists this cluster as unowned.

Traces: datasync-s3-to-s3-task-execution-copies-object#task#12

## Evidence
Fixture datasync-s3-to-s3-task-execution-copies-object, step task#12, operation GetObject, service s3, verdict mismatch. Expected status 200, actual status 404.

  status: expected 200, got 404
  $: expected the object body ('sim2real datasync payload line 1\nline 2\n'), got {'Error': {'Code': 'NoSuchKey', 'Message': 'The specified key does not exist.', 'Key': 'data/payload.txt', 'RequestId': ..., 'HostId': ...}}
  $headers.accept-ranges: missing (expected 'bytes')
  $headers.last-modified: missing (expected '<shaped-not-asserted>')
  $headers.etag: missing (expected '<shaped-not-asserted>')
  $headers.x-amz-checksum-crc64nvme: missing (expected a CRC64NVME value)
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT')
  $headers.x-amz-meta-last-modified: missing (expected '<shaped-not-asserted>')
  $headers.x-amz-server-side-encryption: missing (expected 'AES256')
  $headers.content-type: expected 'text/plain', got 'application/xml'

The preceding step in the same fixture (task#11, DescribeTaskExecution) reports the execution with all transfer counters at 0, which is the same no-op transfer seen from the control-plane side.

Model references: GetObject http {method: GET, uri: /{Bucket}/{Key+}, code: 200}; Body is the httpPayload streaming blob; output headers AcceptRanges->accept-ranges, LastModified->Last-Modified, ETag->ETag, ContentType->Content-Type, Metadata->httpPrefixHeaders x-amz-meta-, ServerSideEncryption->x-amz-server-side-encryption, ChecksumCRC64NVME->x-amz-checksum-crc64nvme, ChecksumType->x-amz-checksum-type; errors = [InvalidObjectState(403), NoSuchKey(404)]; httpChecksum responseAlgorithms include CRC64NVME.

## Steps to Reproduce
Replay fixture datasync-s3-to-s3-task-execution-copies-object and inspect step task#12.

Manually: PutObject a small text object into a source bucket; create an empty destination bucket; CreateLocationS3 for both; CreateTask over them; StartTaskExecution and poll DescribeTaskExecution until the execution is terminal; then GetObject the transferred key from the destination bucket. Observe HTTP 404 with an XML NoSuchKey error body instead of HTTP 200 carrying the source bytes with accept-ranges, Last-Modified, ETag, Content-Type, x-amz-meta-*, x-amz-server-side-encryption and the CRC64NVME checksum headers. A ListObjectsV2 on the destination bucket likewise shows it empty.

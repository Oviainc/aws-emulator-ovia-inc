Issue-Source: trace-sweep-divergence
Issue-Trace: braket-sv1-deterministic-circuit-results-in-s3
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-sv1-deterministic-circuit-results-in-s3
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: s3|GetObject|2xx->4xx|$.additionalMetadata
Issue-Key: s3|GetObject|2xx->4xx|$.braketSchemaHeader
Issue-Key: s3|GetObject|2xx->4xx|$.measuredQubits
Issue-Key: s3|GetObject|2xx->4xx|$.measurements
Issue-Key: s3|GetObject|2xx->4xx|$.resultTypes
Issue-Key: s3|GetObject|2xx->4xx|$.taskMetadata
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Commit: afd3b182fafbbe8cee1c009b5b1f0d78bd466684

# S3 GetObject 404 for the Braket quantum-task results object that no task execution ever writes

> _Note: this issue was identified against the code base at commit `afd3b182fafbbe8cee1c009b5b1f0d78bd466684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause: the object a completed Braket quantum task leaves in the caller's output bucket is never created, so the S3 read that ends the workflow answers 404 instead of serving the results document.

=== Cause 1 of 1: the quantum-task results object is absent from S3, so GetObject on it answers 404 ===
AWS: a quantum task that reaches COMPLETED has written its result document into the caller's output bucket under the task's outputS3Directory (Smithy, braket: GetQuantumTask output members outputS3Bucket (required) and outputS3Directory (required) — 'The folder in the S3 bucket where quantum task results are stored'). Reading that key with S3 GetObject then returns HTTP 200 and the object: content-type binary/octet-stream, with accept-ranges: bytes, last-modified, etag, x-amz-checksum-crc32, x-amz-checksum-type: FULL_OBJECT and x-amz-server-side-encryption on the response (recorded behavior for the result object; the etag, last-modified, checksum values and the bucket/key names are per-run and are not requirements). The body is a braket.task_result.gate_model_task_result document: braketSchemaHeader, measurements (one row per shot, one bit per measured qubit), resultTypes, measuredQubits, taskMetadata (braketSchemaHeader, id = the task ARN, shots, deviceId, deviceParameters, createdAt, endedAt, status COMPLETED) and additionalMetadata (the submitted action plus simulatorMetadata). Only the SHAPE is the requirement — for the recorded deterministic circuit (x on q[0], both qubits measured, 10 shots) each measurement row is the circuit's deterministic outcome, repeated once per shot.
Emulator: GetObject on that key returns HTTP 404 with an application/xml error body instead of the object — replay: 'status: expected 200, got 404', every result-document member reported missing, '$headers.content-type: value_mismatch (expected binary/octet-stream, got application/xml)', and the object metadata headers (accept-ranges, last-modified, etag, x-amz-checksum-crc32, x-amz-checksum-type, x-amz-server-side-encryption) all missing. The 404 is a truthful answer about empty S3 state: nothing ever PUT the key, because the braket task did not execute (same fixture, step task#4: status FAILED, failureReason 'This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.').
Why: the Braket data plane is the API plus S3. A client that submits a circuit and reads its measurements off S3 — which is what every Amazon Braket SDK workflow does — gets a NoSuchKey 404 where AWS serves the result document, so no circuit result can ever be consumed against the emulator.
Fix: not in S3's GetObject handler, which is correctly reporting a key that does not exist — the missing write belongs to braket's quantum-task execution/terminal-transition path, which must put the gate_model_task_result document at <outputS3Bucket>/<outputS3Directory>/results.json through the same S3 state GetObject reads, so the object (and its standard object metadata headers) is there when the task reports COMPLETED. The braket side of this is filed as the quantum-task bundle for braket-sv1-deterministic-circuit-results-in-s3#task#3/#task#4; the behavioral issue 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not.md describes the same non-execution symptom but carries no wire replay key and does not cover this step.
Traces: braket-sv1-deterministic-circuit-results-in-s3#task#5 (s3 GetObject), with braket-sv1-deterministic-circuit-results-in-s3#task#4 (braket GetQuantumTask, FAILED) as the upstream evidence.

## Evidence
Replay of trace set simparity-wire-aws-v1: 39 steps, 30 passed, 9 mismatched, score 76.9%.

Fixture braket-sv1-deterministic-circuit-results-in-s3, step task#5, service s3, operation GetObject, verdict mismatch, divergence_key 'service=s3|GetObject|status=200->404|keys=$.additionalMetadata,$.braketSchemaHeader,$.measuredQubits,$.measurements,$.resultTypes,$.taskMetadata,$headers.accept-ranges,$headers.content-type,$headers.etag,$headers.last-modified,$headers.x-amz-checksum-crc32,$headers.x-amz-checksum-type,$headers.x-amz-server-side-encryption,status':
  status: expected 200, got 404
  $.braketSchemaHeader: missing (expected {'name': 'braket.task_result.gate_model_task_result', 'version': '1'}, got None)
  $.measurements: missing (expected [[1, 0] x 10], got None)
  $.resultTypes: missing (expected [], got None)
  $.measuredQubits: missing (expected [0, 1], got None)
  $.taskMetadata: missing (expected {braketSchemaHeader braket.task_result.task_metadata, id <task ARN>, shots 10, deviceId arn:aws:braket:::device/quantum-simulator/amazon/sv1, deviceParameters <submitted document>, createdAt/endedAt <shaped-not-asserted>, status COMPLETED}, got None)
  $.additionalMetadata: missing (expected {action <submitted openqasm program>, simulatorMetadata {braketSchemaHeader braket.task_result.simulator_metadata, executionDuration <shaped-not-asserted>}}, got None)
  $headers.content-type: value_mismatch (expected 'binary/octet-stream', got 'application/xml')
  $headers.accept-ranges: missing (expected 'bytes', got None)
  $headers.last-modified / $headers.etag / $headers.x-amz-checksum-crc32: missing (expected '<shaped-not-asserted>', got None)
  $headers.x-amz-checksum-type: missing (expected 'FULL_OBJECT', got None)
  $headers.x-amz-server-side-encryption: missing

Upstream, same fixture, step task#4 (braket GetQuantumTask): '$.status: value_mismatch (expected COMPLETED, got FAILED)' with failureReason 'This emulator does not execute quantum programs: no device or simulator ran this quantum task, so no result was written to the output S3 location.' — i.e. the key GetObject is asked for was never written.

Model consulted: smithy_query(query='operation', operation='GetQuantumTask', model_path='services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json') — outputS3Bucket and outputS3Directory are required output members naming where results are stored.

## Steps to Reproduce
Boot the emulator and replay fixture braket-sv1-deterministic-circuit-results-in-s3 (replay_wire_corpus), or by hand with botocore clients in us-west-2:

1. s3.create_bucket(Bucket='amazon-braket-<uniq>').
2. braket.create_quantum_task(clientToken=<uniq>, deviceArn='arn:aws:braket:::device/quantum-simulator/amazon/sv1', shots=10, outputS3Bucket=<bucket>, outputS3KeyPrefix='results', action=json.dumps({'braketSchemaHeader': {'name': 'braket.ir.openqasm.program', 'version': '1'}, 'source': 'OPENQASM 3.0; bit[2] b; qubit[2] q; x q[0]; b[0] = measure q[0]; b[1] = measure q[1];'})).
3. t = braket.get_quantum_task(quantumTaskArn=<arn>); poll to a terminal status (observe FAILED).
4. s3.get_object(Bucket=t['outputS3Bucket'], Key=t['outputS3Directory'] + '/results.json')
   -> observe HTTP 404 with an application/xml error body. Expected: HTTP 200, content-type binary/octet-stream, accept-ranges/last-modified/etag/x-amz-checksum-crc32/x-amz-checksum-type: FULL_OBJECT/x-amz-server-side-encryption headers present, and a braket.task_result.gate_model_task_result body whose measurements array has one row per shot.

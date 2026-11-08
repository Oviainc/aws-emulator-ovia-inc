Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-job-cancel-before-start
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-job-cancel-before-start dataexchange-job-error-paths
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|CancelJob|2xx->2xx|$headers.content-length
Issue-Commit: b92e5a49c7997ea6ad631d05a06b3840cf6aa1ce

# DataExchange CancelJob 204 response omits Content-Length: 0 header

> _Note: this issue was identified against the code base at commit `b92e5a49c7997ea6ad631d05a06b3840cf6aa1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Cause 1 of 1: the emulator's empty-body success response for DataExchange CancelJob is serialized without a Content-Length header.

AWS: Smithy: CancelJob (DELETE /v1/jobs/{JobId}) has output shape Unit and @http code 204, i.e. a modeled empty response body; errors are ConflictException(409), ResourceNotFoundException(404), ValidationException(400), ThrottlingException(429), InternalServerException(500). The model does not itself state header framing, so for the framing this is recorded behavior only: the recorded AWS response for a successful CancelJob is HTTP 204 with an empty body and an explicit 'Content-Length: 0' header. There is no error code or error message involved - the step is a success response, and the only diverging element is the response framing header.

Emulator: the emulator answers the same request with the correct status 204 and an empty body, but emits no Content-Length header at all (replay detail: "$headers.content-length: missing (expected '0', got None)"). Status and body match; only the header is absent.

Why: a client that relies on AWS's framing for a zero-length DataExchange response - an HTTP client or proxy that reads the declared entity length rather than inferring it, or a strict assertion/recording harness comparing response headers - sees a response whose framing differs from AWS. On the real service every 204 empty-body reply declares its zero length explicitly, so code or middleware written against that framing can mis-handle or reject the emulator's reply, and byte-level response comparisons against AWS fail even though the semantic result is correct.

Fix: the CancelJob response serializer for the dataexchange service (the shared empty-body/204 success serialization path - any operation whose Smithy output is Unit with @http code 204 is affected the same way), which must set Content-Length: 0 when it emits an empty success body.

Traces: dataexchange-job-cancel-before-start#task#5 (representative) and dataexchange-job-error-paths#task#8 - both are CancelJob steps, both expected and got status 204, both missing the Content-Length header.

## Evidence
Divergence key service=dataexchange|CancelJob|status=204->204|keys=$headers.content-length, verdict=mismatch.
Expected (recorded AWS): HTTP 204, empty body, header 'Content-Length: 0'.
Actual (emulator): HTTP 204, empty body, NO Content-Length header - detail "$headers.content-length: missing (expected '0', got None)".
Cluster members (affected_count=2): dataexchange-job-cancel-before-start#task#5, dataexchange-job-error-paths#task#8.
Smithy evidence: CancelJob http {method: DELETE, uri: /v1/jobs/{JobId}, code: 204}, output_shape Unit (empty output).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture dataexchange-job-cancel-before-start and stop at step task#5: a DataExchange CancelJob call, DELETE /v1/jobs/{JobId} with a JobId matching ^[a-zA-Z0-9]{30,40}$ for an existing job that has not started.
3. Inspect the raw response headers (e.g. curl -i -X DELETE http://<emulator-host>:<port>/v1/jobs/<jobId>).
4. Observe HTTP 204 with an empty body but no Content-Length header; AWS returns 204 with 'Content-Length: 0'.
5. The same divergence reproduces at dataexchange-job-error-paths#task#8.

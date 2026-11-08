Issue-Source: trace-sweep-divergence
Issue-Trace: dataexchange-job-cancel-before-start
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dataexchange-job-cancel-before-start dataexchange-job-error-paths
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange|CancelJob|2xx->2xx|$headers.content-length
Issue-Commit: 20487886de73e0d49e1ee92e88e94f424cdb9e47

# DataExchange CancelJob 204 No Content response omits the Content-Length header

> _Note: this issue was identified against the code base at commit `20487886de73e0d49e1ee92e88e94f424cdb9e47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJob

## Description
Cause 1 of 1: a successful CancelJob returns the correct empty 204 response but does not emit a Content-Length header.
AWS: CancelJob is DELETE /v1/jobs/{JobId} with an empty output and HTTP success code 204 (Smithy: CancelJob http { method: DELETE, uri: /v1/jobs/{JobId}, code: 204 }, output_shape Unit, i.e. no bound members and no payload). Recorded behavior only for the framing: the recorded AWS response for the 204 carries the header 'content-length: 0' alongside the empty body. The header's value is fixed at 0 for this operation because the output shape has no payload; nothing about it is derived from the request or from state.
Emulator: the status (204) and the empty body are correct, but the response carries no content-length header at all — the replay reports $headers.content-length as missing, expected '0', got None.
Why: a client or intermediary that distinguishes 'explicitly empty' from 'framing unknown' reads this differently from AWS. An SDK or proxy that keys on an explicit zero-length body — connection-reuse logic, HTTP/1.1 intermediaries that must choose between Content-Length and chunked framing, and strict response validators — sees a response AWS never serves, and a test or gateway asserting the recorded header set fails against the emulator while passing against AWS. Because the divergence is in framing rather than in the body, it is invisible to a client that only inspects the parsed result, which makes it easy to ship against.
Fix: the dataexchange crate's response serializer for empty-output operations — the component that builds the 204 for CancelJob must set an explicit Content-Length of 0 for a no-payload response rather than leaving the header off. This is serialization, not CancelJob's state logic, which is correct here (the job was WAITING and is cancelled).
Traces: dataexchange-job-cancel-before-start#task#5, and the same cluster's dataexchange-job-error-paths#task#8.

Note on related issues: 20261029T183418Z-dataexchange-canceljob-stub.md is an open 'unimplemented scaffold stub' issue for CancelJob, but the replay shows CancelJob is in fact serving (it returns 204, 404 and 409 bodies) and reports no stub_path for this step, so that issue does not own this divergence. No open issue names the response framing of any dataexchange empty-output operation.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.

Fixture dataexchange-job-cancel-before-start, step task#5, operation CancelJob (DELETE /v1/jobs/{JobId}) on a job in WAITING state.
  expected status 204 / actual status 204 (match)
  response body: empty in both (match)
  $headers.content-length: missing — expected '0', got None

The same divergence cluster also covers fixture dataexchange-job-error-paths step task#8 (affected_count 2), the other recorded successful CancelJob, confirming it is the operation's serializer and not one recording.

Smithy: CancelJob http { method: DELETE, uri: "/v1/jobs/{JobId}", code: 204 }, output_shape Unit (no members, no payload) — so the expected header value 0 follows from the modelled empty output and is not a per-run value. No status, error code or error message diverges on this step.

## Steps to Reproduce
1. Boot the emulator and replay the simparity-wire-aws-v1 corpus; the step is fixture dataexchange-job-cancel-before-start step task#5 (and dataexchange-job-error-paths step task#8).
2. CreateDataSet(AssetType=S3_SNAPSHOT) -> CreateRevision -> CreateJob(Type=IMPORT_ASSET_FROM_SIGNED_URL, Details.ImportAssetFromSignedUrl{AssetName, DataSetId, Md5Hash, RevisionId}) so the job is in state WAITING.
3. Call CancelJob: DELETE /v1/jobs/{JobId} on that job, before StartJob.
4. Inspect the RAW response headers, not the parsed SDK result: the status is 204 and the body is empty, but there is no content-length header. AWS's recorded response for the same call includes 'content-length: 0'.
5. Confirm it is not request-specific by repeating against a second cancellable job (as dataexchange-job-error-paths#task#8 does) — the header is absent every time.

## Resolution

The rule, one level above the cited call: an answer that carries no content carries no body
and STATES the length it has (`content-length: 0`), rather than leaving the framing header
off. It is now served at the one seam every response of a modelled operation passes
through (`Service::override_protocol` in
`services/service-com-amazonaws-dataexchange-dataexchange/src/service.rs`, delegating to
the shared `service_common::no_content_responses::without_content`), so it holds for every
operation of this service whose modelled output is empty — `CancelJob`, the five deletes and
the two tag mutations — and not only for the recorded cancel. A response that carries
content keeps its body and its own framing.

CAVEAT: a client reached over HTTP/1.1 still does not see the header, because the HTTP
writer the vendored transport uses removes `content-length` from a `204` (RFC 9110 §15.3.5
forbids a body there). That writer is under the read-only vendored tree, so the wire-level
half of this cause is a platform limitation rather than something this service can state its
way out of; `tests/all/no_content_wire_probe.rs` pins what the socket really delivers, and
`tests/all/no_content_framing.rs` pins the framing the service itself states, which the
in-process front door delivers.

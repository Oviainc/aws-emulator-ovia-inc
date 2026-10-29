Issue-Source: trace-sweep-divergence
Issue-Trace: rum-app-monitor-lifecycle-with-tags
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-app-monitor-lifecycle-with-tags rum-empty-and-boundary-tag-operations rum-multiple-app-monitors-tag-isolation rum-tag-unicode-value-round-trip rum-tag-value-unicode-and-empty-string-preserved rum-tag-operations-invalid-arn-error rum-tag-operations-on-invalid-resource
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|ListTagsForResource|2xx->2xx|$headers.content-type
Issue-Key: rum|UntagResource|2xx->2xx|$headers.content-type
Issue-Key: rum|ListTagsForResource|4xx->4xx|$headers.content-type
Issue-Key: rum|UntagResource|4xx->4xx|$headers.content-type
Issue-Commit: 1c82a6d99cb1f7308f6a13b71a678a6984dedc7f

# RUM tagging responses are serialized with the awsJson content type instead of restJson1's application/json

> _Note: this issue was identified against the code base at commit `1c82a6d99cb1f7308f6a13b71a678a6984dedc7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
This issue bundles TWO distinct net-new root causes in the RUM tagging frontend's response serialization. Both must be closed before submitting; each is numbered and carries its own failing fixture steps. The fault they share: the ARN-addressed tagging operations emit the RPC JSON media type `application/x-amz-json-1.1` on the wire, although RUM is a REST-JSON service.

Cause 1 of 2: the SUCCESS responses of the tagging operations carry Content-Type `application/x-amz-json-1.1`.
AWS: Smithy: the service `com.amazonaws.rum#RUM` (version 2018-05-10) declares protocols `["restJson1"]` and no other protocol trait; `ListTagsForResource` is `GET /tags/{ResourceArn}` with output ListTagsForResourceResponse{ResourceArn, Tags} and `UntagResource` is `DELETE /tags/{ResourceArn}` with an EMPTY output shape. Under restJson1 a response that carries a JSON document is labelled `application/json`, and the awsJson media types (`application/x-amz-json-1.0`/`1.1`) are never emitted by a restJson1 service. Recorded behavior agrees: the recorded responses carry `content-type: application/json` on both the 200 ListTagsForResource body and the 204 UntagResource response.
Emulator: the same steps answer with the right status and the right body but `content-type: application/x-amz-json-1.1` — observed on ListTagsForResource (expected 200 / got 200) and on UntagResource (expected 204 / got 204).
Why: the media type is how a client decides how to read the payload and which protocol it is speaking. A restJson1 client that validates the response media type (or any generic HTTP intermediary, cache or proxy that keys off it) sees a protocol the service does not implement; tooling that routes awsJson responses through an RPC decoder will mis-handle a REST body, and the mismatch makes the emulator distinguishable from AWS on every tag read.
Fix: the response serializer used by the tagging frontend for these two operations — it must emit the restJson1 media type (`application/json`) rather than the awsJson one, and the empty-output UntagResource response must be labelled the same way the recording shows.
Traces: rum-app-monitor-lifecycle-with-tags#task#4 (ListTagsForResource, 13 affected steps across rum-app-monitor-lifecycle-with-tags, rum-empty-and-boundary-tag-operations, rum-multiple-app-monitors-tag-isolation, rum-tag-unicode-value-round-trip, rum-tag-value-unicode-and-empty-string-preserved), rum-app-monitor-lifecycle-with-tags#task#7 (UntagResource, 4 affected steps).

Cause 2 of 2: the ERROR responses of the tagging operations carry the same wrong Content-Type.
AWS: Smithy: ListTagsForResource and UntagResource both model ValidationException (client, httpError 400) and ResourceNotFoundException (client, httpError 404); the ResourceArn member carries @pattern "arn:[^:]*:[^:]*:[^:]*:[^:]*:.*". Under restJson1 an error response is a JSON document labelled `application/json`, with the error code in the `x-amzn-errortype` header. Recorded behavior: both the 400 (malformed ARN) and the 404 (well-formed ARN naming no resource) responses carry `content-type: application/json`.
Emulator: status and error body match, but `content-type: application/x-amz-json-1.1` on all four error legs — ListTagsForResource 400, UntagResource 400, ListTagsForResource 404, UntagResource 404. This is a SEPARATE emission path from Cause 1 (the error serializer rather than the success serializer), so fixing one does not necessarily fix the other.
Why: same as Cause 1, on the path where clients are most sensitive — a client that refuses to parse an error document whose media type does not match the negotiated protocol surfaces a transport failure instead of the modeled ValidationException/ResourceNotFoundException.
Fix: the error serializer for the tagging operations (the shared restJson1 error-response construction), so that modeled errors are labelled `application/json`.
Traces: rum-tag-operations-invalid-arn-error#task#1 (ListTagsForResource 400), rum-tag-operations-invalid-arn-error#task#3 (UntagResource 400), rum-tag-operations-on-invalid-resource#task#2 (ListTagsForResource 404), rum-tag-operations-on-invalid-resource#task#3 (UntagResource 404).

## Evidence
Wire replay of trace set simparity-wire-aws-v1: 239/279 steps passed, 40 mismatched, 0 blocked; every divergence is in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). None of the six clusters below reported a stub_path or an owned_by, i.e. none is already filed.

Cause 1:
  rum-app-monitor-lifecycle-with-tags#task#4 — ListTagsForResource, expected status 200, got 200; $headers.content-type: expected 'application/json', got 'application/x-amz-json-1.1'. affected_count 13: rum-app-monitor-lifecycle-with-tags#task#4, #task#6, #task#8, rum-empty-and-boundary-tag-operations#task#3, #task#5, #task#7, #task#9, rum-multiple-app-monitors-tag-isolation#task#5, #task#6, #task#8, #task#9, rum-tag-unicode-value-round-trip#task#3, rum-tag-value-unicode-and-empty-string-preserved#task#3.
  rum-app-monitor-lifecycle-with-tags#task#7 — UntagResource, expected status 204, got 204; same header divergence. affected_count 4: rum-app-monitor-lifecycle-with-tags#task#7, rum-empty-and-boundary-tag-operations#task#4, #task#6, #task#8.

Cause 2:
  rum-tag-operations-invalid-arn-error#task#1 — ListTagsForResource, expected 400, got 400; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1'.
  rum-tag-operations-invalid-arn-error#task#3 — UntagResource, expected 400, got 400; same.
  rum-tag-operations-on-invalid-resource#task#2 — ListTagsForResource, expected 404, got 404; same.
  rum-tag-operations-on-invalid-resource#task#3 — UntagResource, expected 404, got 404; same.

In every one of the 18 clusters the status and the JSON body matched; only the media type diverged, which localises the defect to the media-type label and not to the payload.

## Steps to Reproduce
Replay fixture rum-app-monitor-lifecycle-with-tags step task#4 (the representative) and the five fixture#step bindings in trace_ids against the emulator. Manual repro:

Cause 1: CreateAppMonitor(Name=m, Domain=example.com); TagResource(ResourceArn=arn:aws:rum:<region>:<account>:appmonitor/m, Tags={k: v}); then GET /tags/arn%3Aaws%3Arum%3A<region>%3A<account>%3Aappmonitor%2Fm and inspect the RESPONSE headers -> observed 'content-type: application/x-amz-json-1.1'; expected 'content-type: application/json'. Then DELETE /tags/<same encoded arn>?tagKeys=k -> observed 204 with the same wrong media type; expected 204 labelled application/json.

Cause 2: issue the same two requests with a ResourceArn that breaks the modeled ARN pattern (e.g. 'not-an-arn') -> 400 ValidationException, and with a well-formed ARN naming a nonexistent app monitor -> 404 ResourceNotFoundException. In all four cases the response body and x-amzn-errortype are correct; only 'content-type' must change from 'application/x-amz-json-1.1' to 'application/json'.

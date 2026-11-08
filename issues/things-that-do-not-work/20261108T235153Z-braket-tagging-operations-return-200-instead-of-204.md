Issue-Source: trace-sweep-divergence
Issue-Trace: braket-spending-limit-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-spending-limit-lifecycle
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|TagResource|2xx->2xx|status
Issue-Key: braket|UntagResource|2xx->2xx|status
Issue-Commit: 20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6

# Braket tagging operations return 200 instead of 204 for their empty responses

> _Note: this issue was identified against the code base at commit `20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The emulator's Braket tagging operations answer with HTTP 200 where AWS answers 204 No Content. Both operations are modelled with an EMPTY output shape and no response members, so there is no body to carry and the recorded service replies 204; the emulator instead returns the REST-JSON default 200. This is a shared serializer/handler defect on Braket's two tag-mutation routes, not two unrelated behaviours: both are empty-output mutations under /tags/{resourceArn} and both are off by the same status code.

Cause 1 of 2: TagResource (POST /tags/{resourceArn}).
  AWS: Smithy: braket TagResource -> input {resourceArn @httpLabel @required, tags: TagsMap @required}, output shape TagResourceResponse with NO members; http {method: POST, uri: "/tags/{resourceArn}"} and code 200 only as the protocol default ("codeDefaulted": true), i.e. the model does not pin a success status. Recorded behavior: the response status is 204 with no body. Modelled errors are ValidationException (400), ResourceNotFoundException (404) and InternalServiceException (500); none of those is involved here — the call succeeds.
  Emulator: returns status 200 for the same successful tag write.
  Why: a client that distinguishes no-content from a body-bearing success mis-reads the reply. SDKs and proxies keyed on 204 (no entity expected, safe to skip body parsing) see a 200 that promises a body and then parse an empty/absent entity; conditional and caching layers, and any assertion that a successful tag write is content-free, break. The tag mutation itself appears to be applied, so the divergence is purely the success status of the route.
  Fix: the TagResource handler/serializer for the Braket service — the success-status selection for empty-output operations on the tagging routes, which must emit 204 rather than falling through to the REST-JSON default 200.
  Traces: fixture braket-spending-limit-lifecycle, step task#5 (expected 204, got 200).

Cause 2 of 2: UntagResource (DELETE /tags/{resourceArn}).
  AWS: Smithy: braket UntagResource -> input {resourceArn @httpLabel @required, tagKeys: TagKeys @httpQuery("tagKeys") @required}, output shape UntagResourceResponse with NO members, @idempotent; http {method: DELETE, uri: "/tags/{resourceArn}"} with code 200 again only defaulted ("codeDefaulted": true). Recorded behavior: the response status is 204 with no body. Modelled errors ValidationException (400), ResourceNotFoundException (404), InternalServiceException (500) are not involved — the untag succeeds.
  Emulator: returns status 200 for the same successful tag removal.
  Why: identical client impact to cause 1, and worse for a DELETE route, where 204 is the status clients treat as "removed, nothing to read". Code that branches on 204 to confirm an idempotent delete took effect instead takes the body-bearing 200 branch and attempts to decode an empty entity.
  Fix: the UntagResource handler/serializer for the Braket service — same empty-output success-status selection as cause 1; fixing one route without the other leaves this cause open.
  Traces: fixture braket-spending-limit-lifecycle, step task#7 (expected 204, got 200).

Note: this is NOT covered by the open issue 20261103T044954Z-braket-read-paths-serialize-response-bodies-wrongly-spending.md, which owns the response-BODY serialization defects on Braket's read paths (spending-limit amount/timestamp encoding and the GetDevice capability document). That issue carries no replay key for task#5 or task#7 and concerns no status code; these two causes are the success STATUS of the tag-mutation routes.

## Evidence
Replay of fixture braket-spending-limit-lifecycle against the emulator:

  step task#5  TagResource    expected status 204, actual status 200 (detail: "status: expected 204, got 200"; divergence_key service=braket|TagResource|status=204->200|keys=status)
  step task#7  UntagResource  expected status 204, actual status 200 (detail: "status: expected 204, got 200"; divergence_key service=braket|UntagResource|status=204->200|keys=status)

Neither cluster carries a stub_path (the operations are implemented) and neither carries an owned_by (no open issue holds their replay keys). No body keys diverged in either step — the ONLY diverging field is the HTTP status line, so the requirement at issue is exactly: a successful empty-output Braket tag mutation responds 204 No Content, with no response body and no error code.

Smithy model (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json): TagResourceResponse and UntagResourceResponse are both member-less output shapes; both operations' http traits report "codeDefaulted": true, so the 200 the emulator emits is the protocol default and not a modelled requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture braket-spending-limit-lifecycle.
2. Cause 1 — step task#5: POST /tags/{resourceArn} with a tags map against the spending limit ARN created earlier in the fixture. Observe the response status is 200; AWS/the recording answers 204 with no body.
3. Cause 2 — step task#7: DELETE /tags/{resourceArn}?tagKeys=<key> for the same ARN. Observe the response status is 200; AWS/the recording answers 204 with no body.
4. Both steps must report status 204 for the fixture to pass; re-replay braket-spending-limit-lifecycle steps task#5 AND task#7 to confirm a fix.

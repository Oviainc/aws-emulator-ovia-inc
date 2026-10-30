Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-untag-resource-removes-specific-key
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-untag-resource-removes-specific-key
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|UntagResource|2xx->2xx|status
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# Amplify UntagResource answers 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Cause 1 of 1: Amplify's UntagResource (DELETE /tags/{resourceArn}?tagKeys=...) returns the wrong HTTP status on success.

AWS: recorded behavior only. The recorded response for a successful UntagResource is HTTP 204 with an empty body. Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) binds UntagResource to @http(method: "DELETE", uri: "/tags/{resourceArn}") with an EMPTY output shape (UntagResourceResponse has no members) and does not pin a success code, so smithy_query reports the protocol default 200; the recording is therefore the authority for the on-the-wire status and it is 204. Errors modeled for the operation are BadRequestException (400), ResourceNotFoundException (404) and InternalFailureException (500) — none of them is involved here, the request succeeded and the tag key was in fact removed.

Emulator: the same successful request is answered with HTTP 200 (empty body). The divergence is status-only: expected 204, got 200, with no body or header key differing.

Why: a client that keys off the success status — an SDK or a hand-rolled HTTP client that treats 204 as "no content, do not attempt to parse a body", a CDK/Terraform-style asserter that compares the status literally, or a proxy/test harness that distinguishes 2xx codes — sees a 200 with an empty body where AWS sends 204. Code that branches on `status == 204` for tag-removal completion takes the wrong path, and recorded-vs-live status comparisons fail even though the state change is correct.

Fix: the UntagResource handler's response serialization in the amplify service crate — the success path must emit 204 (No Content) with an empty body for the empty UntagResourceResponse shape, rather than defaulting to 200. Check whether the sibling TagResource success status matches its own recording while making this change, since both share the /tags/{resourceArn} binding and likely the same response-construction helper.

Traces: amplify-untag-resource-removes-specific-key#task#2 (divergence_key service=amplify|UntagResource|status=204->200|keys=status).

Note: three other amplify divergences in this replay (CreateDeployment fileUploadUrls, StartDeployment missing 400 BadRequestException, DeleteBranch stale activeJobId/totalNumberOfJobs) are already owned by the open issue 20261029T215038Z-amplify-job-and-deployment-starts-that-aws-refuses.md and are not re-filed here.

## Evidence
Fixture amplify-untag-resource-removes-specific-key, step task#2, operation UntagResource, verdict mismatch.
Expected (recorded AWS): status 204, empty body.
Actual (emulator): status 200, empty body.
detail: "status: expected 204, got 200"
No body-key or header divergence reported — the mismatch set is exactly {status}.
Smithy: UntagResource @http(method: DELETE, uri: /tags/{resourceArn}); output shape UntagResourceResponse has no members; success code not pinned in the model (smithy_query reports the 200 default); modeled errors BadRequestException(400), ResourceNotFoundException(404), InternalFailureException(500).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture amplify-untag-resource-removes-specific-key.
2. The fixture creates an Amplify app, tags it, then at step task#2 issues UntagResource: DELETE /tags/<app arn>?tagKeys=<one of the applied keys> (resourceArn matching ^arn:aws:amplify:, tagKeys a 1..50 element list).
3. Observe the response status: the emulator answers 200 where the recording has 204. The body is empty in both cases and the key is removed, so only the status line diverges.

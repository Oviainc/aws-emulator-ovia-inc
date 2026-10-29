Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-tag-resource-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-tag-resource-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|TagResource|2xx->2xx|status
Issue-Key: cleanrooms|UntagResource|2xx->2xx|status
Issue-Key: cleanrooms|TagResource|4xx->4xx|$.message
Issue-Key: cleanrooms|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: a1be48b455f77208774c140f08575194e8ab84c8

# cleanrooms tagging responses answer 200 instead of 204 and the not-found refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `a1be48b455f77208774c140f08575194e8ab84c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new root causes on the cleanrooms tagging plane (TagResource / UntagResource): both empty-output success replies carry the wrong HTTP status, and the ResourceNotFoundException refusal is written in the emulator's own words. 3 causes must be closed before this issue is submitted.

Cause 1 of 3: TagResource success answers HTTP 200 instead of 204 No Content.
AWS: TagResource (POST /tags/{resourceArn}) has an empty output shape (TagResourceOutput has no members). Recorded behavior: the success reply is HTTP 204 with no body. (The Smithy model's @http code on this operation is the 200 default; the recording is the authority for the wire status and shows 204, so treat the 200 in the model as not reflecting the served status.)
Emulator: answers HTTP 200. Replay reports "status: expected 204, got 200".
Why: a client or proxy that distinguishes 204 from 200 (conditional body parsing, cache and CDN rules, strict SDK/response-code assertions, HTTP middleware that expects no entity on 204) sees a different response class than AWS returns, and any test pinned to the documented no-content status fails against the emulator.
Fix: the TagResource handler's success response construction/serializer — return 204 with an empty body for the empty output shape.
Traces: cleanrooms-tag-resource-round-trip#task#3.

Cause 2 of 3: UntagResource success answers HTTP 200 instead of 204 No Content.
AWS: UntagResource (DELETE /tags/{resourceArn}?tagKeys=...) likewise has an empty output shape (UntagResourceOutput has no members) and the recording shows the success reply is HTTP 204 with no body.
Emulator: answers HTTP 200. Replay reports "status: expected 204, got 200".
Why: same class of breakage as Cause 1 on the untag path — the two tagging operations are served by the same success-status defect, so a client sees 200 for a no-content operation.
Fix: the UntagResource handler's success response construction/serializer — return 204 with an empty body.
Traces: cleanrooms-tag-resource-round-trip#task#5.

Cause 3 of 3: the tagging ResourceNotFoundException body is emulator-authored ("No such resource: <arn>") and its error-type header does not match AWS's.
AWS: Smithy: TagResource errors ResourceNotFoundException (httpError 404) | ValidationException (400); ResourceNotFoundException requires members message, resourceId and resourceType (ResourceType = CONFIGURED_TABLE | COLLABORATION | MEMBERSHIP | CONFIGURED_TABLE_ASSOCIATION), documented as "Request references a resource which does not exist." Recorded behavior: tagging an ARN that names no existing resource is refused 404 with the fixed message text "Resource was not found" — that sentence is entirely fixed, with no interpolated ARN, id or type — and with the restJson1 x-amzn-errortype header in the form the recording asserts for this service's error shape.
Emulator: answers 404 but with "$.message: value_mismatch (expected 'Resource was not found', got 'No such resource: arn:aws:cleanrooms:us-west-2:000000000000:collaboration/00000000-0000-0000-0000-000000000000')" — i.e. its own wording with the requested ARN interpolated into it — and "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')", so the error-type header value does not match the shape the recording asserts for AWS's header. The account id, region and resource id in the recorded message are per-run values, not part of any requirement; the requirement is the fixed sentence with no interpolation.
Why: clients and test suites that match on the refusal text (log scraping, error-classification tables, assertions on AWS's documented message) do not recognise the emulator's sentence, and a client keying off the x-amzn-errortype header to classify the error before parsing the body fails to resolve it to ResourceNotFoundException. The body also omits the modeled required resourceId/resourceType members, so SDK deserialization of a fully-populated exception has nothing to bind.
Fix: the shared cleanrooms not-found error mapper used by the tagging handlers — emit AWS's fixed message, populate the required resourceId/resourceType members, and set the x-amzn-errortype header in AWS's form.
Traces: cleanrooms-tag-resource-round-trip#task#7.

No existing open cleanrooms issue covers any of these (list_existing_issues query 'cleanrooms' returns none); the s3 issue "s3 PutBucketTagging answers HTTP 200 instead of 204 No Content" (20261029T173211Z) is the same class of fault in a different service and does not cover cleanrooms.

## Evidence
Replay of fixture cleanrooms-tag-resource-round-trip (trace set simparity-wire-aws-v1), emulator up (/__admin/health 200):

Step task#3 TagResource — "status: expected 204, got 200" (divergence_key service=cleanrooms|TagResource|status=204->200|keys=status).

Step task#5 UntagResource — "status: expected 204, got 200" (divergence_key service=cleanrooms|UntagResource|status=204->200|keys=status).

Step task#7 TagResource — expected 404 / actual 404 but body and headers diverge: "$.message: value_mismatch (expected 'Resource was not found', got 'No such resource: arn:aws:cleanrooms:us-west-2:000000000000:collaboration/00000000-0000-0000-0000-000000000000')"; "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')".

Model evidence: TagResourceOutput and UntagResourceOutput have no members; TagResource/UntagResource model ResourceNotFoundException (404) and ValidationException (400); ResourceNotFoundException requires message, resourceId and resourceType.

## Steps to Reproduce
1. Replay fixture cleanrooms-tag-resource-round-trip from trace set simparity-wire-aws-v1 against the emulator.
2. Cause 1 — step task#3: POST /tags/{resourceArn} with a tags map for an existing cleanrooms resource. Observe HTTP 200 instead of 204 No Content.
3. Cause 2 — step task#5: DELETE /tags/{resourceArn}?tagKeys=<key> for that resource. Observe HTTP 200 instead of 204 No Content.
4. Cause 3 — step task#7: POST /tags/{resourceArn} for a well-formed cleanrooms ARN that names no existing resource. Observe 404 whose body message is "No such resource: <arn>" rather than AWS's fixed "Resource was not found", with an x-amzn-errortype header value that does not match AWS's form and no resourceId/resourceType members.

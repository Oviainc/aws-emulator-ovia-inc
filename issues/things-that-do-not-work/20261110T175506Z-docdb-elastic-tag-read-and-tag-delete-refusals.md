Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|ListTagsForResource|4xx->4xx|$headers.content-type
Issue-Key: docdb-elastic|UntagResource|4xx->4xx|$headers.content-type
Issue-Related: 20261030T050126Z-docdb-elastic-listtagsforresource-not-found-answers-the-cluster.md
Issue-Related: 20261104T053156Z-docdb-elastic-error-serializer-emits-a-coral-namespace.md
Issue-Commit: e3918e3196b905fc39a07b01b5a611077b4de5f0

# DocDB Elastic tag-read and tag-delete refusals carry wrong Content-Type and a Coral-qualified x-amzn-errortype

> _Note: this issue was identified against the code base at commit `e3918e3196b905fc39a07b01b5a611077b4de5f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The DocDB Elastic error-response serializer stamps two wrong headers on every modeled refusal of the ARN-addressed tag operations: the media type and the error-type header. Both are set in the shared service-wide error path, not per operation, so the two clusters below are the same fault observed on two operations. This bundle contains 2 causes; both must be closed.

Shared AWS requirement for the media type: Smithy ChimeraDbLionfishServiceLambda (sdkId "DocDB Elastic", version 2022-11-28) declares protocols [restJson1] over sigv4. Under restJson1 a JSON response payload is announced as Content-Type: application/json, on the success path and on the modeled-error path alike; the model pins no @mediaType override, and the corpus asserts exactly 'application/json' for these responses. That string is fixed text of the protocol - no part of it comes from the request or from emulator state.

Shared AWS requirement for the error-type header: the model says nothing about x-amzn-errortype (the service only @cors-exposes it), so for this header this is recorded behavior only. The recording asserts the header's SHAPE rather than a literal, and the stable requirement it carries is that the value's leading token is the error code of the modeled error being served - here exactly 'ResourceNotFoundException', fixed text taken from the error shape name, with nothing in it drawn from the request or state. Hypothesis: the shape AWS serves for a restJson1 DocDB Elastic error is the bare error code (optionally qualified by the modeled shape id), never a qualification by an Amazon-internal Coral namespace URL, which belongs to a different, internal protocol family and is not part of the public wire contract.

Cause 1 of 2: ListTagsForResource against an ARN no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#2).
AWS: 404 ResourceNotFoundException. Smithy: ListTagsForResource is GET /tags/{resourceArn}, @readonly, resourceArn @required @httpLabel @length 1-1011; ResourceNotFoundException is bound at httpError 404 ("The specified resource could not be located") with @required members message, resourceId and resourceType. Response headers: content-type application/json, and an x-amzn-errortype whose leading token is 'ResourceNotFoundException'.
Emulator: status 404 and the error class are right, but the response announces content-type 'application/x-amz-json-1.1' where AWS answers 'application/json', and x-amzn-errortype is 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/' - the leading error code is correct, the ':'-separated suffix is an internal Coral namespace URL, and the combined value fails the recorded header's shape assertion.
Why: a client that dispatches on the response media type (SDK/middleware strict media-type checks, proxies, content negotiation, cassette matching, log pipelines) sees a media type AWS never serves for a restJson1 service, so a byte-identical body is handled differently than against AWS. And x-amzn-errortype is the header SDKs and proxies read to classify an error before parsing the body: a client that compares it to the error name or validates its shape sees an unknown error type and takes a generic-failure branch instead of the not-found branch, so retry and resource-provisioning logic keyed on ResourceNotFoundException goes wrong. It also leaks an Amazon-internal hostname into a response a real client never sees it in.
Fix: the DocDB Elastic crate's restJson1 response serializer - the single place the outgoing content-type and x-amzn-errortype headers are written for this service's modeled errors - not the ListTagsForResource handler, whose status, error selection and body code are already correct.

Cause 2 of 2: UntagResource against an ARN no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#3).
AWS: 404 ResourceNotFoundException. Smithy: UntagResource is DELETE /tags/{resourceArn}, @idempotent, resourceArn @required @httpLabel @length 1-1011 and tagKeys @required @httpQuery 'tagKeys' @length 0-50, with the same ResourceNotFoundException at httpError 404. Same two header requirements as Cause 1: content-type application/json, x-amzn-errortype leading with the fixed error code 'ResourceNotFoundException'.
Emulator: 404 with the right error class, but content-type 'application/x-amz-json-1.1' instead of 'application/json', and the same Coral-qualified header value 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/'.
Why: same as Cause 1 on the tag-delete path - an otherwise correct refusal that a media-type- or errortype-classifying client cannot place.
Fix: same serializer; the error path must announce the protocol's application/json and must not append the internal Coral namespace URL.

Note on existing open issues (none of them covers these clusters - replay reports stub_path and owned_by EMPTY for both steps):
- 20261104T053156Z-docdb-elastic-error-serializer-emits-a-coral-namespace.md owns only the TagResource cluster (task#1 of this same fixture) and mentions task#3 in prose; replay now reports task#2 and task#3 as unowned because their clusters additionally diverge on $headers.content-type, which that issue does not carry. Closing it can leave both header defects standing on these two operations.
- 20261030T050126Z-docdb-elastic-listtagsforresource-not-found-answers-the-cluster.md concerns the not-found BODY wording for ListTagsForResource and is no longer reported as owning task#2.
- 20261029T172712Z-docdb-elastic-responses-announce-content-type-application-json.md and 20261030T050126Z-docdb-elastic-pending-maintenance-action-responses-announce-content.md assert the OPPOSITE direction for this header (they ask for application/x-amz-json-1.1); the corpus expects application/json for these restJson1 responses, so a fix following those issues is what produces the divergence recorded here.

Traces: docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#2 (ListTagsForResource), docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3 (UntagResource).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; /__admin/health answered 200 and the data listener accepts), verdict 'mismatch' on both clusters; stub_path empty and owned_by empty for both, so neither is already filed.
- ListTagsForResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#2: expected_status 404, actual_status 404; $headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1'); $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/'). divergence_key service=docdb-elastic|ListTagsForResource|status=404->404|keys=$headers.content-type,$headers.x-amzn-errortype.
- UntagResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3: expected_status 404, actual_status 404; $headers.content-type: value_mismatch (expected 'application/json', got 'application/x-amz-json-1.1'); $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/'). divergence_key service=docdb-elastic|UntagResource|status=404->404|keys=$headers.content-type,$headers.x-amzn-errortype.
Requirements: HTTP 404 and error code ResourceNotFoundException (both already correct), Content-Type exactly 'application/json' (fixed text of restJson1, the protocol the service's Smithy model declares; nothing in it from request or state), and an x-amzn-errortype whose leading token is the fixed error code 'ResourceNotFoundException' with no internal Coral namespace URL appended. The error-type header is asserted by shape, so no single recorded literal is itself the requirement.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /tags/{resourceArn} (ListTagsForResource) with a well-formed docdb-elastic ARN that no resource carries.
3. Observe 404 with the ResourceNotFoundException error class, then inspect the response headers: content-type is 'application/x-amz-json-1.1' where a restJson1 service answers 'application/json', and x-amzn-errortype is 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/' instead of a value whose leading token is the bare error code.
4. Repeat with DELETE /tags/{resourceArn}?tagKeys=<key> (UntagResource) against the same unknown ARN; both headers are wrong in the same way.
5. Or replay fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, steps task#2 and task#3.

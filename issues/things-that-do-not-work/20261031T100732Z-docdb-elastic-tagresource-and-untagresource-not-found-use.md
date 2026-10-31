Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|TagResource|4xx->4xx|$.message
Issue-Key: docdb-elastic|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: docdb-elastic|UntagResource|4xx->4xx|$.message
Issue-Key: docdb-elastic|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 3227a483a6739d1d495ce0f6668dabbc01e8fd5a

# DocDB Elastic TagResource and UntagResource not-found use the cluster sentence instead of the generic resource sentence

> _Note: this issue was identified against the code base at commit `3227a483a6739d1d495ce0f6668dabbc01e8fd5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The two ARN-addressed tag-WRITING operations refuse an unknown resourceArn with the correct status and error code (404 / ResourceNotFoundException) but with the wrong message sentence, and with an x-amzn-errortype header whose shape does not match what AWS serves. Both operations share one root cause: the tag path resolves the ARN through the cluster lookup and reuses the cluster-specific not-found constructor instead of the generic resource one. 2 causes in this issue; both must be closed.

Cause 1 of 2: TagResource on an ARN that no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#1).
AWS: 404 ResourceNotFoundException. Smithy: TagResource is POST /tags/{resourceArn} with resourceArn @required @httpLabel @length 1-1011 and tags @required; its modeled errors are InternalServerException (500), ResourceNotFoundException (httpError 404, doc "The specified resource could not be located."), ThrottlingException (429) and ValidationException (400). Smithy: ResourceNotFoundException requires message, resourceId and resourceType. The recorded response body's message is the fixed sentence "Resource was not found" - the ENTIRE string is fixed text with NO variable part; the ARN taken from the request belongs in the envelope's resourceId member, not interpolated into the message. The tag operations address a resource generically by ARN (any docdb-elastic resource class, not only a cluster), which is why they carry this generic resource sentence rather than a per-resource-class one. For the header, the recording asserts the SHAPE of x-amzn-errortype rather than a literal value, and the emulator's bare 'ResourceNotFoundException' does not satisfy that shape (recorded behavior only). Hypothesis: AWS serves the qualified Coral form - the error code followed by a ':'-separated shape/namespace suffix - of which only the leading error code is stable across runs.
Emulator: 404 with error code ResourceNotFoundException, message "Unable to find the cluster", and header x-amzn-errortype: ResourceNotFoundException.
Why: a client that asserts on, logs, localizes or branches on AWS's not-found wording never matches, and the emulator mislabels the resource class - a caller tagging a non-cluster ARN is told a cluster could not be found, so error handling that branches on which resource class was missing takes the wrong branch. A client that splits x-amzn-errortype at ':' or '#' still works, but one that validates the header's full shape rejects the emulator's.
Fix: TagResource's ARN-resolution path and the shared not-found error constructor it calls - it must select the generic resource sentence for ARN-addressed tag operations and populate resourceId from the request's ARN, instead of reusing the cluster lookup's cluster sentence; plus the DocDB Elastic error serializer that writes x-amzn-errortype, which must emit the qualified form AWS serves.
Traces: docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#1.

Cause 2 of 2: UntagResource on an ARN that no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#3).
AWS: 404 ResourceNotFoundException. Smithy: UntagResource is DELETE /tags/{resourceArn} with resourceArn @required @httpLabel @length 1-1011 and tagKeys @required @httpQuery("tagKeys") @length 0-50; it is @idempotent and models the same four errors, ResourceNotFoundException at httpError 404 with required message/resourceId/resourceType. The recorded message is again the fixed sentence "Resource was not found" in full, with no variable part. Header assertion is the same shaped (non-literal) x-amzn-errortype as Cause 1 (recorded behavior only).
Emulator: 404 with error code ResourceNotFoundException, message "Unable to find the cluster", and header x-amzn-errortype: ResourceNotFoundException.
Why: same client impact as Cause 1 - untagging an ARN of any resource class reports a missing cluster, so the message is unusable for diagnosis and the resource class reported is wrong.
Fix: UntagResource's ARN-resolution path and the same shared not-found error constructor plus the error serializer's x-amzn-errortype emission.
Traces: docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3.

Note on existing issues: 20261030T050126Z-docdb-elastic-listtagsforresource-not-found-answers-the-cluster.md carries the SAME fixture but only step task#2 (ListTagsForResource, the read path) in its replay key; 20261029T172712Z-docdb-elastic-resourcenotfoundexception-bodies-invent-interpolated-prose-and.md covers the cluster/snapshot not-found bodies; 20261029T200415Z-docdb-elastic-cluster-not-found-refusals-interpolate-the.md covers cluster operations where "Unable to find the cluster" IS the required sentence. None of them carries task#1 or task#3, so these two causes are net-new: closing those issues can still leave TagResource and UntagResource emitting the cluster sentence.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict mismatch on both steps; neither cluster had stub_path or owned_by set.
- TagResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#1: expected_status 404, actual_status 404; $.message: value_mismatch (expected 'Resource was not found', got 'Unable to find the cluster'); $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException').
- UntagResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3: expected_status 404, actual_status 404; $.message: value_mismatch (expected 'Resource was not found', got 'Unable to find the cluster'); $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException').
Requirements: HTTP status 404, error code ResourceNotFoundException, and the message "Resource was not found" reproduced in full as fixed text (the whole sentence is fixed; there is no variable part to substitute). The request's ARN is a value from the request and belongs in the required resourceId member, with resourceType naming the resource class - not interpolated into the message. For x-amzn-errortype the leading error code is the fixed part; any namespace/shape suffix after it is environment-dependent and is not itself asserted by the recording.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /tags/{resourceArn} (TagResource) with a well-formed docdb-elastic ARN that no resource carries, and any tags map.
3. Observe the 404 body: message is 'Unable to find the cluster' where AWS answers the fixed sentence 'Resource was not found'; also inspect x-amzn-errortype, which is the bare 'ResourceNotFoundException' and does not match the shape AWS serves.
4. DELETE /tags/{resourceArn}?tagKeys=<key> (UntagResource) against the same unknown ARN and observe the identical wrong message and header.
5. Or replay fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, steps task#1 and task#3.

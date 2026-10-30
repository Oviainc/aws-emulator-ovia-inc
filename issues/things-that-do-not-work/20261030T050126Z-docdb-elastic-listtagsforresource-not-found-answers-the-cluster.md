Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|ListTagsForResource|4xx->4xx|$.message
Issue-Key: docdb-elastic|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 2d55507234edce158ad24cb25218df0b3fd7399f

# DocDB Elastic ListTagsForResource not-found answers the cluster sentence and an unshaped x-amzn-errortype

> _Note: this issue was identified against the code base at commit `2d55507234edce158ad24cb25218df0b3fd7399f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The tag-reading path refuses an unknown resourceArn with the right status and error code (404 / ResourceNotFoundException) but the wrong message and a differently shaped x-amzn-errortype header. One cause, two diverging keys on the same response.

Cause 1 of 1: ListTagsForResource on an ARN no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#2).
AWS: 404 ResourceNotFoundException. Smithy: ListTagsForResource is GET /tags/{resourceArn} (resourceArn @required, @length 1-1011) with ResourceNotFoundException at httpError 404 ("The specified resource could not be located"), whose required members are message, resourceId and resourceType. The recorded message is the fixed sentence "Resource was not found" — the whole string is fixed text with NO variable part; the ARN from the request belongs in the envelope's resourceId member, not in the message. The tag operations address a resource generically by ARN, so they use this generic resource sentence rather than a per-resource-class one. For the header, the recording asserts the SHAPE of x-amzn-errortype rather than a literal value, and the emulator's bare value 'ResourceNotFoundException' does not satisfy it (recorded behavior only). Hypothesis: AWS serves the qualified Coral form (the error code followed by a ':'-separated shape/namespace suffix), of which only the leading code is stable across runs.
Emulator: 404 ResourceNotFoundException with message "Unable to find the cluster" and header x-amzn-errortype: ResourceNotFoundException.
Why: a client or test that asserts on, logs or localizes AWS's not-found wording never matches, and the emulator mislabels the resource class — a caller listing tags on a non-cluster ARN is told the cluster could not be found, so error-handling that branches on which resource class was missing takes the wrong branch. A client that parses x-amzn-errortype by splitting the qualified value at ':' or '#' still works, but one that validates the header's shape rejects the emulator's.
Fix: ListTagsForResource's resource-resolution path and the shared not-found error constructor it calls — it must select the generic resource sentence for ARN-addressed tag operations (and populate resourceId with the request's ARN), not reuse the cluster sentence; plus the DocDB Elastic error serializer that writes x-amzn-errortype, which must emit the qualified form AWS serves.

Note on existing issues: 20261029T200415Z-docdb-elastic-cluster-not-found-refusals-interpolate-the.md covers GetCluster/CreateClusterSnapshot (behavioral checks) where the required sentence IS "Unable to find the cluster", and 20261029T172712Z-docdb-elastic-resourcenotfoundexception-bodies-invent-interpolated-prose-and.md covers the cluster/snapshot not-found bodies. Neither carries this fixture#step nor the generic-resource sentence, so this divergence is net-new: fixing those two can still leave ListTagsForResource emitting the cluster sentence.

Traces: docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict mismatch:
- ListTagsForResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#2: expected_status 404, actual_status 404; $.message: value_mismatch (expected 'Resource was not found', got 'Unable to find the cluster'); $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException'). Neither stub_path nor owned_by was set for this cluster.
Requirements: status 404, error code ResourceNotFoundException, and the message "Resource was not found" in full as fixed text (no variable part). The request's ARN is a value from the request and belongs in the resourceId member. The x-amzn-errortype value's leading error code is fixed; any namespace/shape suffix after it is environment-dependent and is not itself asserted.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /tags/{resourceArn} (ListTagsForResource) with a well-formed docdb-elastic ARN that no resource carries.
3. Observe the 404 body: message is 'Unable to find the cluster' where AWS answers the fixed sentence 'Resource was not found'.
4. Inspect the response headers: x-amzn-errortype is the bare 'ResourceNotFoundException', which does not match the shape AWS serves for this header.
5. Or replay fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found (task#2).

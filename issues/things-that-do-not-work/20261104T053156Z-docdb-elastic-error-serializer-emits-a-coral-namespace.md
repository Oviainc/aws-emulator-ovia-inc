Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-tag-resource-ops-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: docdb-elastic|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# DocDB Elastic error serializer emits a Coral-namespace-qualified x-amzn-errortype on tag-write not-found refusals

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The ARN-addressed tag-write operations refuse an unknown resourceArn with the correct HTTP status and the correct modeled error, but the response's x-amzn-errortype header carries a value whose shape is not the one AWS serves for this service: the error code followed by a ':'-separated internal Coral namespace URL. The fault is in the shared DocDB Elastic error serializer that writes that header, so it shows up identically on every operation that refuses with an error; two tag-write operations in the corpus are affected and are reported as two clusters.

Cause 1 of 2: TagResource on an ARN no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#1).
AWS: 404 ResourceNotFoundException. Smithy: TagResource is POST /tags/{resourceArn} (resourceArn @required, @httpLabel, @length 1-1011; tags @required), with ResourceNotFoundException bound at httpError 404 ("The specified resource could not be located"), required members message, resourceId and resourceType. The model says nothing about the x-amzn-errortype header, so for the header this is recorded behavior only: the recording asserts the SHAPE of x-amzn-errortype rather than a literal value, and the emulator's value does not satisfy that shape. The one stable requirement the recording carries is that the header's leading token is the error code of the modeled error being served - here exactly 'ResourceNotFoundException' - which is fixed text taken from the error shape name, with no part of it coming from the request or from state. Hypothesis: the shape AWS serves for a restJson1 DocDB Elastic error is the bare error code (optionally qualified by the modeled shape id), not a qualification by an internal Coral namespace URL; that URL is an Amazon-internal detail of a different protocol family and is not part of the public wire contract.
Emulator: 404, body error code ResourceNotFoundException, but header x-amzn-errortype: 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/' - the leading error code is right, the ':'-separated suffix is an internal Coral namespace URL, and the combined value fails the recorded header's shape assertion.

Cause 2 of 2: UntagResource on an ARN no resource carries (fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, step task#3).
AWS: 404 ResourceNotFoundException. Smithy: UntagResource is DELETE /tags/{resourceArn} (resourceArn @required, @httpLabel, @length 1-1011; tagKeys @required, @httpQuery 'tagKeys', @length 0-50), @idempotent, with the same ResourceNotFoundException at httpError 404. Same header requirement as Cause 1: recorded behavior only, leading token must be the error code 'ResourceNotFoundException'.
Emulator: 404, body error code ResourceNotFoundException, same header value 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/', same shape mismatch.

Why: x-amzn-errortype is the header SDKs and proxies read to classify an error before parsing the body. A client that splits the value at ':' still recovers the code, but one that compares the header to the error name, validates its shape, or routes/metricises on it, sees an unknown error type and falls back to a generic failure instead of the ResourceNotFoundException branch - so retry and resource-provisioning logic that keys on the not-found class takes the wrong path. It also leaks an Amazon-internal hostname into a response a real client never sees it in.

Fix: the DocDB Elastic error serializer (the shared error-to-HTTP response path that writes the x-amzn-errortype header for every modeled error of this service), not the TagResource/UntagResource handlers themselves - their status, error selection and body code are already correct. The header must carry the error code in the shape AWS serves for this service and must not append the internal Coral namespace URL.

Note on existing issues: 20261030T050126Z-docdb-elastic-listtagsforresource-not-found-answers-the-cluster.md carries the same header key for ListTagsForResource (task#2 of this same fixture) and is marked as owning that cluster; it was written against an emulator that emitted the BARE 'ResourceNotFoundException' and hypothesised the qualified Coral form as the fix, which the current emulator now emits and which still fails the recorded shape assertion. That issue carries neither task#1 nor task#3, so these two causes are net-new: closing it can leave TagResource and UntagResource still emitting the Coral-qualified header. 20261029T172712Z-docdb-elastic-resourcenotfoundexception-bodies-invent-interpolated-prose-and.md and 20261029T172712Z-docdb-elastic-responses-announce-content-type-application-json.md cover not-found BODIES and the Content-Type header, not this header.

Traces: docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#1 (TagResource), docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3 (UntagResource).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200), verdict mismatch on both clusters; stub_path empty and owned_by empty for both, so neither is already filed.
- TagResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#1: expected_status 404, actual_status 404; $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/'). divergence_key service=docdb-elastic|TagResource|status=404->404|keys=$headers.x-amzn-errortype.
- UntagResource, docdb-elastic-tag-resource-ops-missing-cluster-not-found#task#3: expected_status 404, actual_status 404; $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/'). divergence_key service=docdb-elastic|UntagResource|status=404->404|keys=$headers.x-amzn-errortype.
Requirements: status 404 and error code ResourceNotFoundException (both already correct), and an x-amzn-errortype header whose value matches the shape AWS serves - its leading token the fixed error code 'ResourceNotFoundException' (fixed text from the error shape name; nothing in it comes from the request or state) with no internal Coral namespace URL appended. The recording asserts only the header's shape, so no single recorded literal is itself the requirement.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /tags/{resourceArn} (TagResource) with a well-formed docdb-elastic ARN that no resource carries and a non-empty tag map.
3. Observe 404 with body error code ResourceNotFoundException, then inspect the response headers: x-amzn-errortype is 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.docdbelastic/' - the error code qualified by an internal Coral namespace URL, which does not match the header shape AWS serves.
4. Repeat with DELETE /tags/{resourceArn}?tagKeys=<key> (UntagResource) against the same unknown ARN; the same header value is returned.
5. Or replay fixture docdb-elastic-tag-resource-ops-missing-cluster-not-found, steps task#1 and task#3.

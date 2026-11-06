Issue-Source: trace-sweep-divergence
Issue-Trace: pcs-tag-resource-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pcs-tag-resource-errors
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs|ListTagsForResource|4xx->4xx|$.message
Issue-Key: pcs|ListTagsForResource|4xx->4xx|$.resourceArn
Issue-Key: pcs|ListTagsForResource|4xx->4xx|status
Issue-Key: pcs|TagResource|4xx->4xx|$.message
Issue-Key: pcs|TagResource|4xx->4xx|$.resourceArn
Issue-Key: pcs|TagResource|4xx->4xx|status
Issue-Key: pcs|UntagResource|4xx->4xx|$.message
Issue-Key: pcs|UntagResource|4xx->4xx|$.resourceArn
Issue-Key: pcs|UntagResource|4xx->4xx|status
Issue-Commit: 27c152d33effb619223e02901a2ac305c297c2c5

# pcs tag APIs refuse an unknown resource ARN with the wrong HTTP status and an emulator-authored not-found body missing resourceArn

> _Note: this issue was identified against the code base at commit `27c152d33effb619223e02901a2ac305c297c2c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Two independent root causes on the PCS tagging surface (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying wire corpus simparity-wire-aws-v1, fixture pcs-tag-resource-errors. The fixture exercises the three resource-ARN-addressed operations against an ARN that names a cluster which was never created: ListTagsForResource (task#1), TagResource (task#2), UntagResource (task#3). BOTH causes must be closed, and all three steps must pass, before this issue is submitted; each cause names its own fixture#step so each can be re-replayed independently.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0). Smithy: TagResource / UntagResource / ListTagsForResource each take resourceArn (Arn, required, @length 1..1011, @pattern ^arn:aws.*:pcs:.*:[0-9]{12}:.*/[a-z0-9_\-]+$) and each models ResourceNotFoundException (errorKind client, @httpError 404) with required members message, resourceId, resourceType.

Related but NOT covering: open issue 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md describes the same status-mapping fault (its Cause 1) and the same shared not-found message helper (its Cause 2), but it carries replay keys ONLY for pcs-cluster-error-paths, pcs-compute-node-group-error-paths and pcs-queue-error-paths — no step of pcs-tag-resource-errors appears in it, and the tagging surface's distinct message text and resourceArn member (Cause 2 below) are not in it at all. Open issue 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md is behavioral and carries no wire replay key either.

=== Cause 1 of 2: the tag operations' not-found refusal is served with the modeled @httpError 404 instead of the HTTP 400 PCS serves on the wire ===
Fixture/steps: pcs-tag-resource-errors#task#1 (ListTagsForResource), pcs-tag-resource-errors#task#2 (TagResource), pcs-tag-resource-errors#task#3 (UntagResource).
AWS: recorded behavior only. The model annotates ResourceNotFoundException with @httpError 404, but every recorded PCS client-error response in this corpus — including all three tag-API refusals — was served with HTTP status 400; under awsJson1_0 the error class is conveyed by the body's __type and message, not by the status line. That status-for-every-client-error rule is the requirement, and the error code itself (ResourceNotFoundException) already matches.
Emulator: derives the status from the error shape's @httpError trait, so each of the three refusals answered HTTP 404 where the recording shows 400.
Why: a client that branches on the HTTP status class — SDK retry/throttle classification, a proxy or load balancer that treats 4xx subclasses differently, test code asserting on the status — takes a different path than against PCS: 404 reads as "route or resource gone" while the service's 400 reads as "your request was rejected, read the body".
Fix: the service's error-to-HTTP-status mapping used by the awsJson1_0 error serializer for this crate's modeled client errors (not the individual handlers).
Traces: pcs-tag-resource-errors#task#1, pcs-tag-resource-errors#task#2, pcs-tag-resource-errors#task#3.

=== Cause 2 of 2: an ARN-addressed not-found carries the per-resource-type cluster sentence and omits the resourceArn member, instead of the tagging surface's generic sentence plus the ARN from the request ===
Fixture/steps: pcs-tag-resource-errors#task#1 (ListTagsForResource), pcs-tag-resource-errors#task#2 (TagResource), pcs-tag-resource-errors#task#3 (UntagResource).
AWS: Smithy states the member set of ResourceNotFoundException but not its text, so recorded behavior only. When the resource is addressed by ARN (the three tag operations), the refusal body carries the FIXED sentence "Resource cannot be found." — the whole sentence is fixed text, it names neither a resource type nor an identifier — and a resourceArn member echoing the resourceArn the request supplied (the only variable part; the recorded value arn:aws:pcs:us-west-2:000000000000:cluster/pcs_0000000000000000 is that run's ARN, not part of the requirement, and this member is not declared in the model). This differs from the identifier-addressed operations, where PCS names the resource kind ("AWS PCS couldn't find the cluster you specified.").
Emulator: answers the identifier-addressed wording for whatever kind it parses out of the ARN — "AWS PCS couldn't find the cluster you specified." — and emits no resourceArn member at all. Confirmed in the checkout: ops/tag_resource.rs (and the ListTagsForResource / UntagResource handlers) build the refusal from crate::state::arn_resource (state.rs:1057) and crate::state::not_found (state.rs:916), whose message is format!("AWS PCS couldn't find the {} you specified.", kind.noun) and whose struct carries only message, resource_id and resource_type.
Why: a client that classifies or surfaces a PCS tagging failure by its message text never matches, and tooling that reads resourceArn off the refusal to report WHICH ARN failed (common when tagging many resources in a loop) finds nothing and cannot attribute the failure. The emulator also asserts a resource type the caller never asked about.
Fix: the not-found construction on the ARN-addressed path — the tag operation handlers (ops/tag_resource.rs, ops/untag_resource.rs, ops/list_tags_for_resource.rs) and the helper they share in crate::state — must raise the generic "Resource cannot be found." refusal and populate a resourceArn member with the request's resourceArn; since the model does not declare that member, the error type and this crate's awsJson1_0 error serializer must be extended to emit it. Leave the identifier-addressed per-kind wording alone.
Traces: pcs-tag-resource-errors#task#1, pcs-tag-resource-errors#task#2, pcs-tag-resource-errors#task#3.

## Evidence
Wire replay of simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200): steps_total 14, passed 2, mismatched 12, 10 divergence clusters, score 14.3%. Every divergence is service pcs; seven are owned by open issues and none is blocked by a stub. The three clusters below carry an EMPTY owned_by, i.e. no open issue carries their replay key.

Cause 1 + Cause 2, pcs-tag-resource-errors#task#1 ListTagsForResource [mismatch]:
- status: expected 400, got 404
- $.message: expected 'Resource cannot be found.', got "AWS PCS couldn't find the cluster you specified."
- $.resourceArn: expected 'arn:aws:pcs:us-west-2:000000000000:cluster/pcs_0000000000000000', got None (member absent)

Cause 1 + Cause 2, pcs-tag-resource-errors#task#2 TagResource [mismatch]: identical — status expected 400, got 404; $.message expected 'Resource cannot be found.', got "AWS PCS couldn't find the cluster you specified."; $.resourceArn missing.

Cause 1 + Cause 2, pcs-tag-resource-errors#task#3 UntagResource [mismatch]: identical — status expected 400, got 404; $.message expected 'Resource cannot be found.', got "AWS PCS couldn't find the cluster you specified."; $.resourceArn missing.

The recorded ARN (arn:aws:pcs:us-west-2:000000000000:cluster/pcs_0000000000000000) is that run's value filling the template; the requirement is the 400 status, the ResourceNotFoundException class (already correct), the fixed sentence 'Resource cannot be found.' and a resourceArn echoing whatever ARN the request carried.

Source confirmation in the checkout: services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/tag_resource.rs lines 149-169 build the refusal via crate::state::arn_resource + crate::state::not_found; state.rs:916 formats the per-kind cluster sentence and state.rs:905-908 shows the only generic kind the helper knows is the noun "resource", not the ARN-addressed wording or member.

## Steps to Reproduce
Boot the emulator and point an AWS SDK/botocore PCS client at it (awsJson1_0, sigv4), then replay fixture pcs-tag-resource-errors (steps task#1, task#2, task#3).

Cause 1 and Cause 2 are observed together on each of the three steps. Pick any ARN that is well formed for PCS but names a cluster that was never created, e.g. arn:aws:pcs:<region>:<12-digit account>:cluster/pcs_<16 hex digits>, then:

task#1: pcs.list_tags_for_resource(resourceArn=<that ARN>)
task#2: pcs.tag_resource(resourceArn=<that ARN>, tags={'k': 'v'})
task#3: pcs.untag_resource(resourceArn=<that ARN>, tagKeys=['k'])

Expect for each: HTTP 400, body __type ResourceNotFoundException, message exactly 'Resource cannot be found.', and a resourceArn member equal to the resourceArn supplied in the request.
Observe for each: HTTP 404, message "AWS PCS couldn't find the cluster you specified.", and no resourceArn member.

Contrast with the identifier-addressed path (pcs.get_cluster on an unknown clusterIdentifier), which legitimately uses the per-kind sentence — the fix must not collapse the two wordings.

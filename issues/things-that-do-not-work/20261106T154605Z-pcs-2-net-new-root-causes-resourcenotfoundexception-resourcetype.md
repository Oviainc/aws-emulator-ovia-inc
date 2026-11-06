Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs.get-cluster-nonexistent-not-found pcs.list-tags-for-resource-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs.get-cluster-nonexistent-not-found|run#12|AssertionError|assert resource_type == "Cluster ID", f"expected resourceType 'Cluster ID', got {resource_type!r}"
Issue-Key: pcs.list-tags-for-resource-nonexistent-not-found|run#8|AssertionError|assert message == _NOT_FOUND_MESSAGE, f"expected message {_NOT_FOUND_MESSAGE!r}, got {message!r}"
Issue-Commit: 1c1883c6054d46788e1b73e60943e17eb26a078a

# pcs: 2 net-new root causes — ResourceNotFoundException.resourceType carries the wire token 'cluster' instead of 'Cluster ID', and the ARN-addressed tag APIs serve the per-kind cluster sentence instead of 'Resource cannot be found.'

> _Note: this issue was identified against the code base at commit `1c1883c6054d46788e1b73e60943e17eb26a078a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Two independent, net-new root causes on the PCS not-found surface (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the behavioral suite simparity-behavioral-aws-v1. BOTH must be closed before this issue is submitted; each cause names its own failing check ids so each can be re-replayed independently.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0). Smithy: ResourceNotFoundException (errorKind client, @httpError 404) with required members message, resourceId ("The unique identifier of the resource that was not found.") and resourceType ("The type or category of the resource that was not found."); the model states the member set but none of their text.

Related but NOT covering (both still open, both carry WIRE replay keys only — neither carries either behavioral check id below, so neither covers these failures):
- 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md — its Cause 2 describes the same resourceType token fault for GetCluster/DeleteCluster from the wire corpus (keys pcs|GetCluster|4xx->4xx|$.resourceType etc.), but names no behavioral check.
- 20261106T111530Z-pcs-tag-apis-refuse-an-unknown-resource-arn.md — its Cause 2 describes the same ARN-addressed message fault from the wire corpus (keys pcs|ListTagsForResource|4xx->4xx|$.message etc.), but names no behavioral check.
- 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md is behavioral and DOES name both check ids, but its replay keys are the EARLIER assertions in those same checks (get-cluster: the message assertion; list-tags: the error-code assertion). Both of those assertions now pass and the checks have advanced to the two assertions below, which that issue does not carry.

=== Cause 1 of 2: the identifier-addressed cluster not-found reports resourceType 'cluster' instead of PCS's identifier-kind token 'Cluster ID' ===
Failing check: pcs.get-cluster-nonexistent-not-found
AWS: Smithy declares ResourceNotFoundException.resourceType as a required String documented as "The type or category of the resource that was not found" but does not state its vocabulary, so recorded behavior only. On the identifier-addressed lookups (GetCluster and its siblings that take a clusterIdentifier), the recording shows resourceType is the human-readable identifier-kind token for the identifier the caller supplied — for a cluster, exactly 'Cluster ID' (all fixed text; it names no instance) — while resourceId carries the identifier from the request (the only variable part). The HTTP status, the error code (ResourceNotFoundException) and the message ("AWS PCS couldn't find the cluster you specified.", fixed text) already match.
Emulator: emits the lower-cased, space-free wire-ish token 'cluster'. Built from the ResourceKind table in the crate's state module — CLUSTER.resource_type is the literal "cluster" — and copied into the error by the shared not_found() constructor, so every cluster not-found on the identifier path reports it. QUEUE ('queue') and COMPUTE_NODE_GROUP ('computenodegroup') are on the same table and will diverge the same way the moment a check reaches their resourceType assertion.
Why: a client that switches on resourceType to decide what kind of identifier it got wrong — error-reporting UIs, retry/branch logic, test assertions on the modeled member — never matches PCS: it sees 'cluster' where the service says 'Cluster ID', and would see 'computenodegroup' where the service names the node-group identifier kind. The member is modeled required precisely so callers can branch on it, so the divergence is silent and total.
Fix: the resource-kind vocabulary and the shared not-found construction in the crate's state module (the ResourceKind constants and the not_found() helper that copies resource_type into ResourceNotFoundException), used by the cluster/queue/node-group resolution helpers and their callers in the identifier-addressed handlers. resourceId keeps the request's identifier; the message is already correct.
Traces: pcs.get-cluster-nonexistent-not-found (file/line citations: src/state.rs lines 884-908 define the ResourceKind constants; src/state.rs lines 916-922 is not_found()).

=== Cause 2 of 2: an ARN-addressed not-found carries the per-kind cluster sentence instead of the tagging surface's generic fixed sentence ===
Failing check: pcs.list-tags-for-resource-nonexistent-not-found
AWS: recorded behavior only (the model states ResourceNotFoundException's members, not their text). When the resource is addressed by ARN — ListTagsForResource, TagResource, UntagResource, each taking resourceArn (Arn, required, @length 1..1011, @pattern ^arn:aws.*:pcs:.*:[0-9]{12}:.*/[a-z0-9_\-]+$) — the refusal's message is the FIXED sentence 'Resource cannot be found.': the whole sentence is fixed text, it names neither a resource type nor an identifier, and it has no variable part. This is deliberately different from the identifier-addressed path, where PCS names the kind ("AWS PCS couldn't find the cluster you specified."). The error code ResourceNotFoundException already matches on all three operations.
Emulator: parses a kind out of the ARN and answers the identifier-addressed per-kind wording — "AWS PCS couldn't find the cluster you specified." — for an ARN that names a cluster which was never created. The handler reads the kind off the ARN via crate::state::arn_resource and builds the refusal with crate::state::not_found, whose message is format!("AWS PCS couldn't find the {} you specified.", kind.noun); tag_resource.rs and untag_resource.rs take the same path, so all three ARN-addressed operations diverge identically.
Why: a client that classifies or surfaces a PCS tagging failure by its message text never matches the service, and the emulator asserts a resource KIND the caller never named (it inferred it from the ARN string), so a malformed-but-parseable ARN can be reported as a missing cluster where PCS says only that the resource is not there. Tagging loops that log the service's sentence per failure produce text no PCS-written matcher recognises.
Fix: the not-found construction on the ARN-addressed path — the three tag handlers (src/ops/list_tags_for_resource.rs, src/ops/tag_resource.rs, src/ops/untag_resource.rs) and the state helper they share — must raise the generic 'Resource cannot be found.' refusal for an ARN that resolves to nothing, leaving the identifier-addressed per-kind wording untouched.
Traces: pcs.list-tags-for-resource-nonexistent-not-found (file/line citations: src/ops/list_tags_for_resource.rs lines 138-162 build the refusal from crate::state::arn_resource + crate::state::not_found; src/state.rs line 918 formats the per-kind sentence).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: pcs 6/13 passed (46%), 7 failed (6 mismatched, 1 errored). Five of the seven are marked OWNED by open issues and skipped; the two below are net-new and carry no owner.

Cause 1 — pcs.get-cluster-nonexistent-not-found [mismatch]:
  check_pcs_get_cluster_nonexistent_not_found.py line 44:
  AssertionError: expected resourceType 'Cluster ID', got 'cluster'
The check calls pcs.get_cluster(clusterIdentifier='pcs_' + '0'*20). The earlier assertions in the same check now pass: code == ResourceNotFoundException, message == "AWS PCS couldn't find the cluster you specified.", resourceId == the supplied identifier. Only the modeled resourceType member diverges. The identifier in the check ('pcs_0000...') is that run's value filling the template; the requirement is the token 'Cluster ID'.

Cause 2 — pcs.list-tags-for-resource-nonexistent-not-found [mismatch]:
  check_pcs_list_tags_for_resource_nonexistent_not_found.py line 43:
  AssertionError: expected message 'Resource cannot be found.', got "AWS PCS couldn't find the cluster you specified."
The check calls pcs.list_tags_for_resource(resourceArn='arn:aws:pcs:<region>:<12-digit account>:cluster/pcs_<suffix>') for a cluster that was never created. code == ResourceNotFoundException matches (this is progress on 20261023T123102Z's Cause 4, which recorded a 501 NotImplemented there); the message text is what diverges. The check goes on to assert the same code and the same 'Resource cannot be found.' message for pcs.tag_resource and pcs.untag_resource against the same ARN — those assertions were not reached because the ListTagsForResource one failed first, and the same shared helper feeds all three, so verify all three while fixing. The ARN's account id, region and cluster suffix are per-run values; the requirement is the fixed sentence.

Source confirmation in the checkout: src/state.rs:885-888 `pub const CLUSTER: ResourceKind = ResourceKind { resource_type: "cluster", noun: "cluster" };` and src/state.rs:916-922 `pub fn not_found(kind, identifier) -> ResourceNotFoundException { message: format!("AWS PCS couldn't find the {} you specified.", kind.noun), resource_id: ..., resource_type: kind.resource_type.to_string() }`; src/ops/list_tags_for_resource.rs:140-146 builds its refusal from arn_resource + not_found.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK PCS client at it (awsJson1_0, sigv4).

Cause 1 (pcs.get-cluster-nonexistent-not-found):
  pcs.get_cluster(clusterIdentifier='pcs_' + '0'*20)
Inspect the raised ClientError. Under awsJson1_0 the modeled exception members surface as top-level keys on e.response, not under 'Error'.
Expect: code ResourceNotFoundException, Error.Message "AWS PCS couldn't find the cluster you specified.", e.response['resourceId'] == the clusterIdentifier supplied, e.response['resourceType'] == 'Cluster ID'.
Observe: resourceType == 'cluster' (everything else already matches).

Cause 2 (pcs.list-tags-for-resource-nonexistent-not-found): pick a well-formed PCS cluster ARN for a cluster that was never created, e.g. arn:aws:pcs:<region>:<12-digit account>:cluster/pcs_<lowercase alnum suffix>, then:
  pcs.list_tags_for_resource(resourceArn=<that ARN>)
  pcs.tag_resource(resourceArn=<that ARN>, tags={'k': 'v'})
  pcs.untag_resource(resourceArn=<that ARN>, tagKeys=['k'])
Expect for each: code ResourceNotFoundException and Error.Message exactly 'Resource cannot be found.'
Observe for each: code ResourceNotFoundException (correct) but Error.Message "AWS PCS couldn't find the cluster you specified."
Contrast with the identifier-addressed path (pcs.get_cluster on an unknown clusterIdentifier), which legitimately uses the per-kind sentence — the fix must not collapse the two wordings.

Full replay: run the behavioral suite and re-run the check ids in trace_ids.

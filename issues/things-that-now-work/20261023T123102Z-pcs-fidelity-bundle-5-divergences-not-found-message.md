Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs.get-cluster-nonexistent-not-found pcs.create-queue-nonexistent-cluster-not-found pcs.create-cluster-reserved-name-prefix-rejected pcs.create-compute-node-group-nonexistent-cluster-not-found pcs.list-tags-for-resource-nonexistent-not-found pcs.register-compute-node-group-instance-bad-bootstrap-rejected
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs.get-cluster-nonexistent-not-found|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: pcs.create-queue-nonexistent-cluster-not-found|_assert_error#5|AssertionError|assert error.get("Message") == message, (
Issue-Key: pcs.create-cluster-reserved-name-prefix-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: pcs.create-compute-node-group-nonexistent-cluster-not-found|run#6|AssertionError|assert create_error.get("Code") == "ValidationException", (
Issue-Key: pcs.list-tags-for-resource-nonexistent-not-found|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: pcs.register-compute-node-group-instance-bad-bootstrap-rejected|run#8|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# PCS fidelity bundle: 5 divergences — not-found message text, ValidationException message format, CreateComputeNodeGroup validation ordering, and 4 unimplemented tagging/bootstrap operations

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputeNodeGroup

## Description
Five independent root causes in the PCS emulator (service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the behavioral suite. All five must be closed before this issue is submitted.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0).

=== Cause 1 of 5: ResourceNotFoundException carries an emulator-invented message instead of PCS's fixed not-found sentence ===
Failing checks: pcs.get-cluster-nonexistent-not-found, pcs.create-queue-nonexistent-cluster-not-found
AWS: recorded behavior only — the Smithy model states ResourceNotFoundException (errorKind client, httpError 404, required members message/resourceId/resourceType) but does not state the message text. The replayed recordings show PCS uses a FIXED sentence per resource type on every not-found path, independent of the identifier supplied: cluster -> "AWS PCS couldn't find the cluster you specified.", queue -> "AWS PCS couldn't find the queue you specified.", compute node group -> "AWS PCS couldn't find the compute node group you specified." That constant-per-resource-type rule is the requirement; the exact sentences above are the recorded examples of it. The status (404) and code (ResourceNotFoundException) already match.
Emulator: builds the message from the request identifier instead. GetCluster on cluster id pcs_00000000000000000000 answered message 'no cluster identified by pcs_00000000000000000000'; CreateQueue (and ListQueues) against a nonexistent clusterIdentifier answered 'no cluster identified by pcs_22222222222222222222'.
Why: a client that classifies or surfaces PCS failures by the service's not-found wording never matches, and the emulator additionally echoes the caller-supplied identifier into the message where AWS does not, so tests and log scrapers written against PCS behave differently under the emulator. Note the same checks also assert the required resourceId/resourceType members on the identifier-keyed path; those assertions were never reached because the message assertion failed first, so verify them while fixing.
Fix: the shared not-found error construction used by the cluster/queue/compute-node-group lookup paths in this crate's state module, and its callers in the GetCluster / CreateQueue / ListQueues handlers.
Traces: pcs.get-cluster-nonexistent-not-found, pcs.create-queue-nonexistent-cluster-not-found.

=== Cause 2 of 5: ValidationException message does not follow AWS's constraint-violation format ===
Failing check: pcs.create-cluster-reserved-name-prefix-rejected
AWS: Smithy: CreateClusterRequest.clusterName -> ClusterName, required, @length 3..40, @pattern ^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$; CreateCluster models ValidationException (errorKind client, httpError 400). AWS reports a modeled-constraint violation with the standard, request-derived framing: a violation count, the member path, and the constraint that failed — recorded example: "1 validation error(s) detected: Value at 'clusterName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!pcs_)^[A-Za-z][A-Za-z0-9-]+". Because the message is built from the request (member name) and the model (the constraint), that construction rule is the requirement, not the literal recorded sentence. Status 400 / code ValidationException already match, and no cluster is created.
Emulator: answered message 'clusterName must satisfy regular expression pattern: ^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$' — it omits the "N validation error(s) detected: Value at '<member>' failed to satisfy constraint: Member must satisfy ..." framing entirely and emits its own shorthand.
Why: clients and test suites that parse AWS's standard validation framing (counting violations, extracting the offending member path) get no match, and multi-field validation cannot be represented at all since the count/aggregation structure is absent. The modeled ValidationException.reason/fieldList members are also the documented place for the per-field detail.
Fix: the PCS validation-message helpers in this crate's state module (resource_name_violation / field_validation, as called from create_cluster.rs and create_compute_node_group.rs) — they must emit the standard constraint-violation framing and populate the modeled reason/fieldList.
Traces: pcs.create-cluster-reserved-name-prefix-rejected.

=== Cause 3 of 5: CreateComputeNodeGroup resolves the cluster before validating the IAM instance profile, inverting AWS's ordering ===
Failing check: pcs.create-compute-node-group-nonexistent-cluster-not-found
AWS: Smithy: CreateComputeNodeGroup models both ResourceNotFoundException (404) and ValidationException (400). Recorded behavior: unlike the read/delete node-group operations (GetComputeNodeGroup, ListComputeNodeGroups, DeleteComputeNodeGroup), which resolve the cluster up front and fail 404 ResourceNotFoundException, CreateComputeNodeGroup validates the referenced IAM instance profile against a live IAM lookup FIRST. A request naming a nonexistent cluster together with a nonexistent instance profile therefore fails 400 ValidationException about the missing profile, never ResourceNotFoundException on the cluster. (The recorded message embeds a per-call IAM request id and an SDK attempt count, so it is an example only; the requirement is status 400 / code ValidationException.)
Emulator: returned 404 ResourceNotFoundException on the cluster — cluster resolution runs ahead of instance-profile validation in CreateComputeNodeGroup::handle (services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_compute_node_group.rs).
Why: the HTTP status AND the error code both differ, so a client's retry/branching logic diverges: a 404 reads as "the cluster is gone, stop" while AWS's 400 reads as "your instance profile reference is wrong, fix the request". Provisioning code that creates a cluster and node group concurrently will take the wrong recovery path.
Fix: the CreateComputeNodeGroup handler's validation order — validate the instanceProfileArn reference before resolving clusterIdentifier. The read/delete paths keep their existing cluster-first ordering.
Traces: pcs.create-compute-node-group-nonexistent-cluster-not-found.

=== Cause 4 of 5: the tagging operations are unimplemented and answer 501 NotImplemented ===
Failing check: pcs.list-tags-for-resource-nonexistent-not-found
AWS: Smithy: ListTagsForResource (errors: ResourceNotFoundException), TagResource (ResourceNotFoundException, ServiceQuotaExceededException), UntagResource (ResourceNotFoundException); ListTagsForResourceRequest.resourceArn -> Arn, required, @length 1..1011, @pattern ^arn:aws.*:pcs:.*:[0-9]{12}:.*/[a-z0-9_\-]+$. A well-formed PCS ARN naming a resource that does not exist is rejected with 404 ResourceNotFoundException on all three. Recorded behavior adds that this ARN-keyed lookup path does NOT populate the resourceId/resourceType members the shared exception shape models as required, unlike the identifier-keyed path (GetCluster).
Emulator: answered error code NotImplemented (HTTP 501). services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/list_tags_for_resource.rs is still the generated scaffold and returns crate::scaffolded::Scaffolded::not_implemented("ListTagsForResource"); tag_resource.rs and untag_resource.rs are on the same path (the check asserts all three and failed on the first).
Why: tagging is unusable — a client cannot read, set or remove PCS tags, and a 501 is neither modeled nor retry-classified by the SDK, so error handling written for ResourceNotFoundException never triggers.
Fix: implement the handlers in ops/list_tags_for_resource.rs, ops/tag_resource.rs and ops/untag_resource.rs against the ARN-keyed resource lookup in the crate's state module, and drop their SCAFFOLDED markers.
Traces: pcs.list-tags-for-resource-nonexistent-not-found.

=== Cause 5 of 5: RegisterComputeNodeGroupInstance is unimplemented and answers 501 NotImplemented ===
Failing check: pcs.register-compute-node-group-instance-bad-bootstrap-rejected
AWS: Smithy: RegisterComputeNodeGroupInstance models exactly AccessDeniedException (errorKind client, httpError 403) and InternalServerException; its docs say "This API action isn't intended for you to use. PCS uses this API action to register the compute node...". Input: clusterIdentifier (@pattern ^(pcs_[a-zA-Z0-9]+|[A-Za-z][A-Za-z0-9-]{2,40})$) and bootstrapId (@length 1..1000) both required. Called by an ordinary SDK caller (not an EC2 instance the bootstrapId was minted for) with a syntactically valid but bogus clusterIdentifier/bootstrapId, AWS rejects it with 403 AccessDeniedException — recorded message example "This API must be called from an EC2 instance." — and returns no bootstrap material: the modeled required response members nodeID, sharedSecret (sensitive) and endpoints are never handed back.
Emulator: answered error code NotImplemented (HTTP 501); services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/register_compute_node_group_instance.rs is still the generated scaffold.
Why: the operation's one externally observable contract — that a non-instance caller is denied, and that Slurm bootstrap material (shared Munge key, node id) is never leaked to it — is not exercised at all; a client sees an unmodeled 501 instead of the modeled 403.
Fix: implement ops/register_compute_node_group_instance.rs to deny callers that cannot be attributed to a registered compute node instance with AccessDeniedException, returning no response body, and drop its SCAFFOLDED marker.
Traces: pcs.register-compute-node-group-instance-bad-bootstrap-rejected.

## Evidence
Behavioral suite: pcs 0/13 passed (0%); Mismatched 6, Errored 7. The 6 mismatches are the 5 causes above.

Cause 1:
- pcs.get-cluster-nonexistent-not-found [mismatch]: AssertionError: expected message "AWS PCS couldn't find the cluster you specified.", got 'no cluster identified by pcs_00000000000000000000'
- pcs.create-queue-nonexistent-cluster-not-found [mismatch]: AssertionError: expected message "AWS PCS couldn't find the cluster you specified.", got 'no cluster identified by pcs_22222222222222222222'
(code ResourceNotFoundException and status 404 matched in both)

Cause 2:
- pcs.create-cluster-reserved-name-prefix-rejected [mismatch]: AssertionError: expected message "1 validation error(s) detected: Value at 'clusterName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?!pcs_)^[A-Za-z][A-Za-z0-9-]+", got 'clusterName must satisfy regular expression pattern: ^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$'
(code ValidationException and status 400 matched)

Cause 3:
- pcs.create-compute-node-group-nonexistent-cluster-not-found [mismatch]: AssertionError: expected ValidationException (the nonexistent IAM instance profile short-circuits cluster resolution), got ResourceNotFoundException. Expected HTTP 400 / ValidationException; emulator gave HTTP 404 / ResourceNotFoundException.

Cause 4:
- pcs.list-tags-for-resource-nonexistent-not-found [mismatch]: AssertionError: expected ResourceNotFoundException, got NotImplemented. Expected HTTP 404 / ResourceNotFoundException; emulator gave HTTP 501 / NotImplemented. Confirmed in the checkout: ops/list_tags_for_resource.rs line 4 '//! SCAFFOLDED, NOT IMPLEMENTED' and line 153 'Err(crate::scaffolded::Scaffolded::not_implemented("ListTagsForResource"))'.

Cause 5:
- pcs.register-compute-node-group-instance-bad-bootstrap-rejected [mismatch]: AssertionError: expected AccessDeniedException, got NotImplemented. Expected HTTP 403 / AccessDeniedException with no response body; emulator gave HTTP 501 / NotImplemented.

Not part of this issue (already covered): pcs.cluster-create-active-delete-state-transition-lifecycle and 4 sibling checks are ALREADY FILED under EC2 CreateInternetGateway; pcs.delete-dependents-out-of-order-conflict-rejected is ALREADY FILED under SSM GetParametersByPath; pcs.compute-node-group-and-queue-lifecycle is OWNED by 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK PCS client at it (awsJson1_0, sigv4).

Cause 1: pcs.get_cluster(clusterIdentifier='pcs_' + '0'*20) -> inspect Error.Message; expect "AWS PCS couldn't find the cluster you specified.", observe 'no cluster identified by pcs_000...'. Same for pcs.create_queue(clusterIdentifier='pcs_' + '2'*20, queueName=<name>) and pcs.list_queues(clusterIdentifier='pcs_' + '2'*20).

Cause 2: pcs.create_cluster(clusterName='pcs_reserved-name', scheduler={'type':'SLURM','version':'25.05'}, size='SMALL', networking={...}) -> code is ValidationException (correct) but Message lacks the "N validation error(s) detected: Value at '<member>' failed to satisfy constraint: ..." framing.

Cause 3: pcs.create_compute_node_group(clusterIdentifier='pcs_'+'2'*20, computeNodeGroupName=<name>, subnetIds=[...], customLaunchTemplate={...}, iamInstanceProfileArn=<ARN of a profile that does not exist>, scalingConfiguration={'minInstanceCount':0,'maxInstanceCount':0}, instanceConfigs=[{'instanceType':'c5.large'}]) -> expect 400 ValidationException about the missing instance profile, observe 404 ResourceNotFoundException about the cluster. Contrast with get/list/delete_compute_node_group on the same bogus cluster, which correctly give ResourceNotFoundException.

Cause 4: pcs.list_tags_for_resource(resourceArn='arn:aws:pcs:<region>:<12-digit account>:cluster/pcs_'+'3'*20) -> expect 404 ResourceNotFoundException, observe 501 NotImplemented. Same for pcs.tag_resource(resourceArn=<same>, tags={'k':'v'}) and pcs.untag_resource(resourceArn=<same>, tagKeys=['k']).

Cause 5: pcs.register_compute_node_group_instance(clusterIdentifier='pcs_'+'4'*20, bootstrapId='bogus-bootstrap-id') -> expect 403 AccessDeniedException and no nodeID/sharedSecret/endpoints in the response, observe 501 NotImplemented.

Full replay: run the behavioral suite and re-run the check ids in trace_ids.

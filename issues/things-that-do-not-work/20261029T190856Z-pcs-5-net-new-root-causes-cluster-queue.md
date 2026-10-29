Issue-Source: trace-sweep-divergence
Issue-Trace: pcs-cluster-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pcs-cluster-error-paths pcs-compute-node-group-error-paths pcs-queue-error-paths
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs|GetCluster|4xx->4xx|$.message
Issue-Key: pcs|GetCluster|4xx->4xx|$.resourceType
Issue-Key: pcs|GetCluster|4xx->4xx|status
Issue-Key: pcs|DeleteCluster|4xx->4xx|$.message
Issue-Key: pcs|DeleteCluster|4xx->4xx|$.resourceType
Issue-Key: pcs|DeleteCluster|4xx->4xx|status
Issue-Key: pcs|CreateCluster|4xx->2xx|$.__type
Issue-Key: pcs|CreateCluster|4xx->2xx|$.message
Issue-Key: pcs|CreateCluster|4xx->2xx|status
Issue-Key: pcs|GetComputeNodeGroup|4xx->4xx|$.Message
Issue-Key: pcs|GetComputeNodeGroup|4xx->4xx|$.__type
Issue-Key: pcs|GetComputeNodeGroup|4xx->4xx|status
Issue-Key: pcs|CreateComputeNodeGroup|4xx->4xx|$.__type
Issue-Key: pcs|CreateComputeNodeGroup|4xx->4xx|$.message
Issue-Key: pcs|CreateComputeNodeGroup|4xx->4xx|status
Issue-Key: pcs|GetQueue|4xx->4xx|$.Message
Issue-Key: pcs|GetQueue|4xx->4xx|$.__type
Issue-Key: pcs|GetQueue|4xx->4xx|status
Issue-Key: pcs|CreateQueue|4xx->4xx|$.Message
Issue-Key: pcs|CreateQueue|4xx->4xx|$.__type
Issue-Key: pcs|CreateQueue|4xx->4xx|status
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# pcs: 5 net-new root causes — cluster, queue and node-group error paths serve the wrong HTTP status, the wrong error class and emulator-authored messages, and CreateCluster never validates subnets

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Five independent root causes in the PCS emulator (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the wire corpus simparity-wire-aws-v1 (fixtures pcs-cluster-error-paths, pcs-compute-node-group-error-paths, pcs-queue-error-paths). ALL FIVE must be closed before this issue is submitted; each names its own fixture#step so each can be re-replayed independently.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0).

Related but NOT covering: open issue 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md was filed from the BEHAVIORAL corpus and carries no replay key for any of the wire steps below; its Cause 1 touches the same not-found message helper as Cause 2 here and its Cause 3 the same validation ordering as Cause 4 here, but neither of those causes names these fixtures, and Causes 1, 3 and 5 here are not in it at all.

=== Cause 1 of 5: client-error responses carry the modeled @httpError status instead of the HTTP 400 PCS serves on the wire ===
Fixture/steps: pcs-cluster-error-paths#task#1 (GetCluster), pcs-cluster-error-paths#task#2 (DeleteCluster), pcs-compute-node-group-error-paths#task#2 (GetComputeNodeGroup), pcs-compute-node-group-error-paths#task#4 (CreateComputeNodeGroup), pcs-queue-error-paths#task#2 (GetQueue), pcs-queue-error-paths#task#4 (CreateQueue).
AWS: recorded behavior only. The Smithy model annotates the client errors with distinct @httpError values (ResourceNotFoundException 404, AccessDeniedException 403, ValidationException 400), but every recorded PCS client-error response in this corpus was served with HTTP status 400 regardless of which error class the body carried — the awsJson1_0 error class is conveyed by the body's __type and message, not by the status line. That status-for-every-client-error rule is the requirement.
Emulator: derives the status from the error shape's @httpError trait, so each of the six refusals above answered HTTP 404 where the recording shows 400.
Why: a client that branches on the HTTP status class — SDK retry/throttle classification, a proxy or load balancer that treats 4xx subclasses differently, test code asserting on response status — takes a different path under the emulator than against PCS: a 404 reads as "route or resource gone" while the service's 400 reads as "your request was rejected, read the body".
Fix: the service's error-to-HTTP-response mapping (the status the awsJson1_0 error serializer picks for this crate's modeled client errors), not the individual handlers.
Traces: pcs-cluster-error-paths#task#1, pcs-cluster-error-paths#task#2, pcs-compute-node-group-error-paths#task#2, pcs-compute-node-group-error-paths#task#4, pcs-queue-error-paths#task#2, pcs-queue-error-paths#task#4.

=== Cause 2 of 5: the cluster not-found refusal carries an emulator-authored message and the wrong resourceType token ===
Fixture/steps: pcs-cluster-error-paths#task#1 (GetCluster), pcs-cluster-error-paths#task#2 (DeleteCluster).
AWS: Smithy: ResourceNotFoundException (errorKind client) with required members message, resourceId, resourceType; the model does not state their text, so recorded behavior only. The recording shows the message is a FIXED sentence for the resource type and does NOT embed the caller-supplied identifier — for a cluster, exactly "AWS PCS couldn't find the cluster you specified." (all of it fixed text, no variable part) — and that resourceType is the human-readable identifier-kind token "Cluster ID", with resourceId carrying the identifier from the request (the variable part). The error code (ResourceNotFoundException) already matches.
Emulator: builds the message from the request instead and lower-cases the resource type: GetCluster and DeleteCluster on an unknown cluster identifier answered message 'no cluster identified by <the clusterIdentifier from the request>' and resourceType 'cluster'.
Why: a client that classifies or surfaces a PCS not-found by its message text never matches, and code that switches on resourceType sees 'cluster' where PCS says 'Cluster ID'. The emulator also leaks the caller-supplied identifier into a message where the service does not, so message-equality assertions and log scrapers behave differently.
Fix: the shared not-found error construction in this crate's state module (the cluster/queue/node-group resolution helpers that build ResourceNotFoundException with resource_type "cluster") and its callers in the GetCluster and DeleteCluster handlers — the message must be the fixed per-resource-type sentence and resourceType the identifier-kind token, with resourceId keeping the request's identifier.
Traces: pcs-cluster-error-paths#task#1, pcs-cluster-error-paths#task#2.

=== Cause 3 of 5: cluster-scoped child operations on an unknown cluster are refused as ResourceNotFoundException where PCS refuses them as AccessDeniedException ===
Fixture/steps: pcs-compute-node-group-error-paths#task#2 (GetComputeNodeGroup), pcs-queue-error-paths#task#2 (GetQueue), pcs-queue-error-paths#task#4 (CreateQueue).
AWS: Smithy: GetComputeNodeGroup, GetQueue and CreateQueue all model AccessDeniedException (errorKind client, required member message) alongside ResourceNotFoundException, and AccessDeniedException's own docs list "The cluster ID doesn't exist." as one of its examples. Recorded behavior confirms it: a child-resource operation naming a clusterIdentifier that does not exist is refused with the IAM authorization error, not a not-found — __type AccessDeniedException (recorded as 'com.amazon.coral.service#AccessDeniedException'), its text in the body member `Message` (capital M), reading "User: <caller's principal ARN> is not authorized to perform: pcs:<OperationName> on resource: <the clusterIdentifier from the request> as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Variable parts, all taken from the request and the caller identity: the principal ARN, the action name (pcs:GetComputeNodeGroup, pcs:GetQueue, pcs:CreateQueue) and the resource (the clusterIdentifier supplied). The ARN and cluster name in any one recording are that run's values, not part of the template.
Emulator: resolves the cluster first and answers ResourceNotFoundException with its own 'no cluster identified by <identifier>' text and no `Message` member at all, for all three operations.
Why: the error CODE differs, so client branching diverges in the worst direction: AWS's AccessDeniedException is a terminal permissions failure a client reports to a human and never retries, while the emulator's ResourceNotFoundException reads as "create the cluster and retry". Provisioning code and permission-diagnostics tooling written against PCS take the wrong branch, and anything reading the error's `Message` member finds nothing.
Fix: the cluster-resolution step shared by the compute-node-group and queue handlers (ops/get_compute_node_group.rs, ops/get_queue.rs, ops/create_queue.rs and the state module helper they call) — an unresolvable clusterIdentifier on a child-scoped operation must raise AccessDeniedException with the not-authorized message rendered from the caller principal, the operation's IAM action and the supplied identifier, instead of ResourceNotFoundException.
Traces: pcs-compute-node-group-error-paths#task#2, pcs-queue-error-paths#task#2, pcs-queue-error-paths#task#4.

=== Cause 4 of 5: CreateComputeNodeGroup resolves the cluster before validating iamInstanceProfileArn, inverting PCS's ordering ===
Fixture/step: pcs-compute-node-group-error-paths#task#4 (CreateComputeNodeGroup).
AWS: Smithy: CreateComputeNodeGroup requires iamInstanceProfileArn (InstanceProfileArn, @pattern ^arn:aws([a-zA-Z-]{0,10})?:iam::[0-9]{12}:instance-profile/([!-~]{1,510}/)?([\w+=,.@-]{1,128})$) and models both ValidationException (errorKind client) and ResourceNotFoundException. Recorded behavior: unlike the read/delete node-group operations, the CREATE path validates the referenced IAM instance profile against IAM FIRST, so a request naming both a nonexistent cluster and a nonexistent instance profile is refused as a validation error about the PROFILE — __type ValidationException (recorded as 'com.amazon.coral.validate#ValidationException'), message "Instance Profile <profile name from the request's iamInstanceProfileArn> cannot be found." followed by the relayed IAM-call framing "(Service: Iam, Status Code: 404, Request ID: <per-call id>) (SDK Attempt Count: <n>)". Fixed text: "Instance Profile " / " cannot be found."; the profile name is taken from the request, and the request id and attempt count are per-run values that are NOT part of the requirement. The requirement is the error class ValidationException and the "Instance Profile <name> cannot be found." sentence.
Emulator: answered ResourceNotFoundException about the cluster ('no cluster identified by <clusterIdentifier>') — cluster resolution runs ahead of instance-profile validation in ops/create_compute_node_group.rs.
Why: the error code differs, so a client's recovery path inverts: a not-found on the cluster reads as "wait for the cluster, then retry" while PCS's ValidationException reads as "the instance profile in your request is wrong, do not retry unchanged". Provisioning code that creates a cluster and a node group concurrently loops instead of failing fast on the bad profile reference.
Fix: the validation ordering in ops/create_compute_node_group.rs — validate the iamInstanceProfileArn reference before resolving clusterIdentifier; the read/delete node-group handlers keep cluster-first ordering.
Traces: pcs-compute-node-group-error-paths#task#4.

=== Cause 5 of 5: CreateCluster accepts networking.subnetIds that do not exist, creating the cluster instead of refusing the request ===
Fixture/step: pcs-cluster-error-paths#task#3 (CreateCluster).
AWS: Smithy: CreateClusterRequest.networking (NetworkingRequest, required) carries subnetIds (@length min 1), whose docs state the subnets must already exist ("The subnets and security groups for the cluster must already exist before you use this API action"), and CreateCluster models ValidationException (errorKind client), whose docs list "There are networking related errors, such as network validation failure" as an example. Recorded behavior: a CreateCluster naming a subnet id that does not exist in the account is refused with __type ValidationException (recorded as 'com.amazon.aws.pcs.service.exceptions#ValidationException') and the fixed message "AWS PCS couldn't find 1 or more of the subnets you provided." — the whole sentence is fixed text, it names no subnet id, and no cluster is created.
Emulator: answered HTTP 200 with a created cluster. ops/create_cluster.rs validates only clusterName's pattern and the sizes of the caller-sized lists it persists (crate::state::stored_list_violation) and then mints the id and inserts the record — it never checks the subnet ids against the account's EC2 subnets, and there is no subnet existence gate anywhere on the path.
Why: the refusal is missing entirely, which is worse than wrong wording: a client that relies on CreateCluster to reject an unusable network gets an ACTIVE cluster whose control-plane ENIs could never have been placed, so the failure surfaces later (or never) and infrastructure tests asserting the guardrail pass against the emulator and fail against PCS. The cluster also pollutes state and counts against the cluster quota.
Fix: add the networking validation to the CreateCluster handler (ops/create_cluster.rs, before the record is minted and inserted): resolve every networking.subnetIds entry against the emulator's EC2 subnet state and, if any is unknown, return the modeled ValidationException with the fixed "AWS PCS couldn't find 1 or more of the subnets you provided." message and persist nothing.
Traces: pcs-cluster-error-paths#task#3.

## Evidence
Wire replay of simparity-wire-aws-v1 against the booted emulator: steps_total 14, passed 2, mismatched 12, 7 divergence clusters, score 14.3% — every divergence is pcs and none is blocked by a stub or owned by an open issue.

Cause 1 (status class — all six error steps):
- pcs-cluster-error-paths#task#1 GetCluster: status expected 400, got 404
- pcs-cluster-error-paths#task#2 DeleteCluster: status expected 400, got 404
- pcs-compute-node-group-error-paths#task#2 GetComputeNodeGroup: status expected 400, got 404
- pcs-compute-node-group-error-paths#task#4 CreateComputeNodeGroup: status expected 400, got 404
- pcs-queue-error-paths#task#2 GetQueue: status expected 400, got 404
- pcs-queue-error-paths#task#4 CreateQueue: status expected 400, got 404

Cause 2 (cluster not-found body):
- pcs-cluster-error-paths#task#1 GetCluster [mismatch]: $.message expected "AWS PCS couldn't find the cluster you specified.", got 'no cluster identified by pcs_0000000000003747461e'; $.resourceType expected 'Cluster ID', got 'cluster'
- pcs-cluster-error-paths#task#2 DeleteCluster [mismatch]: identical body divergence on the same unknown cluster identifier
(the recorded id pcs_0000000000003747461e is that run's value, not a requirement)

Cause 3 (wrong error class on child-scoped operations against an unknown cluster):
- pcs-compute-node-group-error-paths#task#2 GetComputeNodeGroup [mismatch]: $.__type expected AccessDeniedException ('com.amazon.coral.service#AccessDeniedException'), got 'ResourceNotFoundException'; $.Message expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: pcs:GetComputeNodeGroup on resource: sim2real-probe-pcs-nosuchcluster-9b88bcc0 as either the resource does not exist, some policy explicitly denies access, or no policy grants access', got None
- pcs-queue-error-paths#task#2 GetQueue [mismatch]: same, action pcs:GetQueue, resource sim2real-probe-pcs-nosuchcluster-ff2602bf
- pcs-queue-error-paths#task#4 CreateQueue [mismatch]: same, action pcs:CreateQueue, resource sim2real-probe-pcs-nosuchcluster-ff2602bf
(the principal ARN and the probe cluster names are per-run values filling the template)

Cause 4 (CreateComputeNodeGroup validation ordering):
- pcs-compute-node-group-error-paths#task#4 [mismatch]: $.__type expected ValidationException ('com.amazon.coral.validate#ValidationException'), got 'ResourceNotFoundException'; $.message expected 'Instance Profile does-not-exist cannot be found. (Service: Iam, Status Code: 404, Request ID: 2e74c9a3-6d72-4955-a894-4a20412a0d34) (SDK Attempt Count: 1)', got 'no cluster identified by sim2real-probe-pcs-nosuchcluster-9b88bcc0'. The IAM request id in the recorded message is per-run; the requirement is the class and the "Instance Profile <name> cannot be found." sentence.

Cause 5 (CreateCluster accepts unknown subnets):
- pcs-cluster-error-paths#task#3 CreateCluster [mismatch]: status expected 400, got 200; $.__type expected 'com.amazon.aws.pcs.service.exceptions#ValidationException', got None; $.message expected "AWS PCS couldn't find 1 or more of the subnets you provided.", got None — i.e. the emulator created the cluster. Confirmed in the checkout: ops/create_cluster.rs performs only the clusterName pattern check and stored-list size checks before minting the id and inserting the ClusterRecord; no subnet lookup exists on the path.

## Steps to Reproduce
Boot the emulator and point an AWS SDK/botocore PCS client at it (awsJson1_0, sigv4), then replay the three fixtures: pcs-cluster-error-paths, pcs-compute-node-group-error-paths, pcs-queue-error-paths.

Cause 1 + Cause 2 (pcs-cluster-error-paths#task#1, #task#2): pcs.get_cluster(clusterIdentifier=<a well-formed pcs_... id that was never created>) and pcs.delete_cluster(clusterIdentifier=<same>). Expect HTTP 400 with ResourceNotFoundException, message exactly "AWS PCS couldn't find the cluster you specified.", resourceType "Cluster ID" and resourceId echoing the supplied identifier. Observe HTTP 404, message 'no cluster identified by <identifier>', resourceType 'cluster'.

Cause 3 (pcs-compute-node-group-error-paths#task#2, pcs-queue-error-paths#task#2, #task#4): pcs.get_compute_node_group(clusterIdentifier=<a cluster name that does not exist>, computeNodeGroupIdentifier=<any valid name>), pcs.get_queue(clusterIdentifier=<same>, queueIdentifier=<any valid name>), pcs.create_queue(clusterIdentifier=<same>, queueName=<any valid name>). Expect HTTP 400 with AccessDeniedException whose `Message` member reads "User: <caller principal ARN> is not authorized to perform: pcs:<Operation> on resource: <the clusterIdentifier supplied> as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Observe HTTP 404 ResourceNotFoundException with 'no cluster identified by <identifier>' and no `Message` member.

Cause 4 (pcs-compute-node-group-error-paths#task#4): pcs.create_compute_node_group(clusterIdentifier=<a cluster that does not exist>, computeNodeGroupName=<valid name>, subnetIds=[<a subnet>], customLaunchTemplate={...}, iamInstanceProfileArn='arn:aws:iam::<12-digit account>:instance-profile/does-not-exist', scalingConfiguration={'minInstanceCount':0,'maxInstanceCount':0}, instanceConfigs=[{'instanceType':'c5.large'}]). Expect HTTP 400 ValidationException with message 'Instance Profile does-not-exist cannot be found. ...'. Observe HTTP 404 ResourceNotFoundException about the cluster. Contrast with get/delete_compute_node_group on the same bogus cluster, which correctly report a not-found class.

Cause 5 (pcs-cluster-error-paths#task#3): pcs.create_cluster(clusterName=<valid name>, scheduler={'type':'SLURM','version':'...'}, size='SMALL', networking={'subnetIds':['subnet-<an id that exists in no EC2 state>']}). Expect HTTP 400 ValidationException with message "AWS PCS couldn't find 1 or more of the subnets you provided." and no cluster created. Observe HTTP 200 and a cluster in CREATING; a following pcs.list_clusters() shows the cluster that should not exist.

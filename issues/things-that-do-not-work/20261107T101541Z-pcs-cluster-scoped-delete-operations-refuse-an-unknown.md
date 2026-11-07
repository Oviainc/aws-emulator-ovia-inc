Issue-Source: trace-sweep-divergence
Issue-Trace: pcs-compute-node-group-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pcs-compute-node-group-error-paths pcs-queue-error-paths
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs|DeleteComputeNodeGroup|4xx->4xx|$.Message
Issue-Key: pcs|DeleteComputeNodeGroup|4xx->4xx|$.__type
Issue-Key: pcs|DeleteComputeNodeGroup|4xx->4xx|status
Issue-Key: pcs|DeleteQueue|4xx->4xx|$.Message
Issue-Key: pcs|DeleteQueue|4xx->4xx|$.__type
Issue-Key: pcs|DeleteQueue|4xx->4xx|status
Issue-Commit: 89dcb84bc001d630dd4b5e9068fc0fc72dd2f2e3

# pcs: cluster-scoped delete operations refuse an unknown cluster as ResourceNotFoundException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `89dcb84bc001d630dd4b5e9068fc0fc72dd2f2e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteComputeNodeGroup

## Description
Two net-new root causes in the PCS emulator (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the wire corpus simparity-wire-aws-v1 (fixtures pcs-compute-node-group-error-paths, pcs-queue-error-paths). BOTH must be closed before this issue is submitted; each names its own fixture#step so each can be re-replayed independently.

They share one fault: when a cluster-scoped DELETE operation names a clusterIdentifier that does not exist, the emulator resolves the cluster first and refuses the request as a not-found, where PCS refuses it as an IAM authorization failure. The two causes are separated because they live in two different handlers (DeleteComputeNodeGroup and DeleteQueue) and each has its own recorded step.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0).

Related but NOT covering: open issue 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md. Its Cause 3 is the same wrong-error-class fault, but only for the READ and CREATE child operations (GetComputeNodeGroup, GetQueue, CreateQueue) — it names no DELETE step in its trace list and carries no replay key for either step below, so neither delete path is exercised or closed by it. Its Cause 1 (every client error must be served with HTTP 400, not the modeled @httpError status) likewise names only the six non-delete steps; the same status divergence appears on both steps here and is restated per cause so these steps are actually re-replayed.

=== Cause 1 of 2: DeleteComputeNodeGroup on an unknown cluster answers ResourceNotFoundException/404 instead of AccessDeniedException/400 ===
Fixture/step: pcs-compute-node-group-error-paths#task#3 (DeleteComputeNodeGroup).
AWS: Smithy: DeleteComputeNodeGroup models AccessDeniedException (errorKind client, required member message) alongside ResourceNotFoundException, and AccessDeniedException's own documentation lists "The cluster ID doesn't exist." as one of its examples. Recorded behavior confirms this is the branch PCS takes for a child-resource operation scoped to a nonexistent cluster: HTTP status 400 (the awsJson1_0 error class is carried by the body, not the status line — the modeled @httpError 403/404 values are not what the wire shows), body __type 'com.amazon.coral.service#AccessDeniedException', and the text in the body member `Message` (capital M, not the modeled lowercase `message`), reading "User: <caller's principal ARN> is not authorized to perform: pcs:DeleteComputeNodeGroup on resource: <the clusterIdentifier from the request> as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Fixed text: "User: ", " is not authorized to perform: ", " on resource: ", " as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Variable parts, taken from the request and the caller identity: the principal ARN, the IAM action name (pcs:DeleteComputeNodeGroup) and the resource (the clusterIdentifier supplied). The particular ARN and cluster name in any one recording are that run's values and are not part of the requirement.
Emulator: resolves the clusterIdentifier first and answers HTTP 404 with __type 'ResourceNotFoundException' and no `Message` member at all.
Why: the error CODE differs, and in the worst direction — AWS's AccessDeniedException is a terminal permissions failure a client reports to a human and never retries, while the emulator's ResourceNotFoundException reads as "the cluster is not there yet, create it and retry". Teardown and provisioning code written against PCS takes the wrong branch (loops or silently treats the delete as already done), permission-diagnostics tooling never sees the authorization error, and anything reading the error's `Message` member finds nothing. The status also differs, so a client branching on the HTTP status class sees 404 ("route or resource gone") where PCS says 400 ("request rejected, read the body").
Fix: the cluster-resolution step in the DeleteComputeNodeGroup handler (ops/delete_compute_node_group.rs and the state-module cluster lookup helper it calls) — an unresolvable clusterIdentifier on this child-scoped operation must raise AccessDeniedException with the not-authorized message rendered from the caller principal, pcs:DeleteComputeNodeGroup and the supplied identifier, serialized with the `Message` member and HTTP 400, instead of ResourceNotFoundException.
Traces: pcs-compute-node-group-error-paths#task#3.

=== Cause 2 of 2: DeleteQueue on an unknown cluster answers ResourceNotFoundException/404 instead of AccessDeniedException/400 ===
Fixture/step: pcs-queue-error-paths#task#3 (DeleteQueue).
AWS: Smithy: DeleteQueue models AccessDeniedException (errorKind client, required member message) alongside ResourceNotFoundException; AccessDeniedException's docs again list "The cluster ID doesn't exist.". Recorded behavior: HTTP 400, __type 'com.amazon.coral.service#AccessDeniedException', body member `Message` reading "User: <caller's principal ARN> is not authorized to perform: pcs:DeleteQueue on resource: <the clusterIdentifier from the request> as either the resource does not exist, some policy explicitly denies access, or no policy grants access" — same fixed text as Cause 1, with the action rendered as pcs:DeleteQueue and the resource taken from the request.
Emulator: answers HTTP 404 with __type 'ResourceNotFoundException' and no `Message` member.
Why: same client-visible consequence as Cause 1 — a retryable-looking not-found replaces a terminal authorization refusal, and the status class flips from 400 to 404. A queue teardown loop that tolerates not-found but must surface permission errors behaves differently against the emulator than against PCS.
Fix: the cluster-resolution step in the DeleteQueue handler (ops/delete_queue.rs and the shared state-module cluster lookup) — raise AccessDeniedException with the not-authorized message (action pcs:DeleteQueue) and HTTP 400 instead of ResourceNotFoundException when the clusterIdentifier does not resolve.
Traces: pcs-queue-error-paths#task#3.

## Evidence
Wire replay of simparity-wire-aws-v1 against the booted emulator: steps_total 14, passed 2, mismatched 12, 12 divergence clusters, score 14.3%. Every divergence is service pcs; none is blocked by a stub. Ten clusters are owned by open issues; the two below are owned by none and are what this issue files.

Cause 1 — pcs-compute-node-group-error-paths#task#3, DeleteComputeNodeGroup [mismatch]:
- status: expected 400, got 404
- $.__type: expected 'com.amazon.coral.service#AccessDeniedException', got 'ResourceNotFoundException'
- $.Message: expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: pcs:DeleteComputeNodeGroup on resource: sim2real-probe-pcs-nosuchcluster-9b88bcc0 as either the resource does not exist, some policy explicitly denies access, or no policy grants access', got None (the member is absent)
(the principal ARN and the probe cluster name sim2real-probe-pcs-nosuchcluster-9b88bcc0 are that run's values filling the message template, not requirements)

Cause 2 — pcs-queue-error-paths#task#3, DeleteQueue [mismatch]:
- status: expected 400, got 404
- $.__type: expected 'com.amazon.coral.service#AccessDeniedException', got 'ResourceNotFoundException'
- $.Message: expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: pcs:DeleteQueue on resource: sim2real-probe-pcs-nosuchcluster-ff2602bf as either the resource does not exist, some policy explicitly denies access, or no policy grants access', got None (the member is absent)
(principal ARN and cluster name are again per-run values)

Model support: smithy_query on the PCS model shows both DeleteComputeNodeGroup and DeleteQueue bind AccessDeniedException (errorKind client, required member message) as well as ResourceNotFoundException, and AccessDeniedException's doc enumerates "The cluster ID doesn't exist." among its causes — so the recorded AccessDeniedException is the modeled branch, not an artifact. The contrast with the sibling read/create operations (filed as Cause 3 of 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md) shows the whole child-scoped family shares the defect, but neither delete step is listed there.

## Steps to Reproduce
Boot the emulator and point an AWS SDK/botocore PCS client at it (awsJson1_0, sigv4), then replay fixtures pcs-compute-node-group-error-paths and pcs-queue-error-paths.

Cause 1 (pcs-compute-node-group-error-paths#task#3): pcs.delete_compute_node_group(clusterIdentifier=<a well-formed cluster name that was never created>, computeNodeGroupIdentifier=<any valid node-group name>). Expect HTTP 400 with __type 'com.amazon.coral.service#AccessDeniedException' and a body member `Message` reading "User: <caller principal ARN> is not authorized to perform: pcs:DeleteComputeNodeGroup on resource: <the clusterIdentifier supplied> as either the resource does not exist, some policy explicitly denies access, or no policy grants access". Observe HTTP 404 with __type 'ResourceNotFoundException' and no `Message` member.

Cause 2 (pcs-queue-error-paths#task#3): pcs.delete_queue(clusterIdentifier=<the same nonexistent cluster name>, queueIdentifier=<any valid queue name>). Expect HTTP 400 AccessDeniedException with the same message template and action pcs:DeleteQueue. Observe HTTP 404 ResourceNotFoundException with no `Message` member.

In both cases the cluster must be absent from emulator state; if the cluster exists, the child-resource not-found path (ResourceNotFoundException) is correct and is not what this issue is about.

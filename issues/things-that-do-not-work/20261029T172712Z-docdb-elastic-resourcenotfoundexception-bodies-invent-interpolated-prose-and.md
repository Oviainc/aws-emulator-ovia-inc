Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-elastic-get-cluster-nonexistent-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-elastic-get-cluster-nonexistent-not-found docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found docdb-elastic-cluster-snapshot-ops-missing-resource-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic|GetCluster|4xx->4xx|$.message
Issue-Key: docdb-elastic|GetCluster|4xx->4xx|$headers.content-type
Issue-Key: docdb-elastic|UpdateCluster|4xx->4xx|$.message
Issue-Key: docdb-elastic|DeleteCluster|4xx->4xx|$.message
Issue-Key: docdb-elastic|DeleteCluster|4xx->4xx|$headers.content-type
Issue-Key: docdb-elastic|CreateClusterSnapshot|4xx->4xx|$.message
Issue-Key: docdb-elastic|GetClusterSnapshot|4xx->4xx|$.message
Issue-Key: docdb-elastic|GetClusterSnapshot|4xx->4xx|$.resourceType
Issue-Key: docdb-elastic|GetClusterSnapshot|4xx->4xx|$headers.content-type
Issue-Key: docdb-elastic|DeleteClusterSnapshot|4xx->4xx|$.message
Issue-Key: docdb-elastic|DeleteClusterSnapshot|4xx->4xx|$.resourceType
Issue-Key: docdb-elastic|DeleteClusterSnapshot|4xx->4xx|$headers.content-type
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# DocDB Elastic ResourceNotFoundException bodies invent interpolated prose and an internal resourceType

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Every DocDB Elastic lookup that cannot resolve its target answers HTTP 404 ResourceNotFoundException with the right status and error code, but the envelope's human-readable members are the emulator's own prose rather than the wording AWS serves, and for snapshot targets the required resourceType member reports the emulator's internal type name. Six net-new causes, all in the same not-found envelope builder.

Smithy: ResourceNotFoundException (httpError 404) requires message, resourceId and resourceType; resourceId carries "The ID of the resource that could not be located" and resourceType "The type of the resource that could not be found", so the ARN belongs in resourceId and the message is fixed per resource class.

Cause 1 of 6: UpdateCluster on a clusterArn no cluster carries (fixture docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found, step task#1).
AWS: 404 ResourceNotFoundException whose message is the fixed sentence "Unable to find the cluster" — the whole message is fixed text, with NO variable part; the requested ARN is reported in resourceId, not spliced into the message (Smithy: ResourceNotFoundException.message/resourceId/resourceType all required; recorded behavior for the wording).
Emulator: message is "Cluster <clusterArn> not found" — invented template text with the request's ARN interpolated into it.
Why: a client or test asserting on AWS's not-found wording (or logging it) sees a sentence AWS never emits, and the message varies per request where AWS's does not.
Fix: the UpdateCluster handler's cluster-resolution path, via the crate's shared not-found error constructor.

Cause 2 of 6: DeleteCluster on a clusterArn no cluster carries (fixture docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found, step task#3).
AWS: 404 with the same fixed message "Unable to find the cluster".
Emulator: "Cluster <clusterArn> not found".
Why: same as cause 1, on the delete path.
Fix: the DeleteCluster handler's cluster-resolution path / shared not-found constructor.

Cause 3 of 6: CreateClusterSnapshot naming a clusterArn no cluster carries (fixture docdb-elastic-cluster-snapshot-ops-missing-resource-not-found, step task#1).
AWS: 404 with the fixed message "Unable to find the cluster" — the refusal is about the source cluster, so it uses the cluster wording.
Emulator: "Cluster <clusterArn> not found".
Why: same as cause 1, on the snapshot-create path.
Fix: CreateClusterSnapshot's source-cluster resolution / shared not-found constructor.

Cause 4 of 6: GetClusterSnapshot on a snapshotArn no snapshot carries (fixture docdb-elastic-cluster-snapshot-ops-missing-resource-not-found, step task#2).
AWS: 404 whose message is the fixed sentence "Unable to find the snapshot" and whose resourceType is the AWS resource-type name "Snapshot".
Emulator: message "ClusterSnapshot <snapshotArn> not found" and resourceType "ClusterSnapshot".
Why: the message is again invented and ARN-interpolated, and resourceType publishes the emulator's internal/operation-name spelling, so a client switching on resourceType to classify the missing resource never matches AWS's "Snapshot".
Fix: GetClusterSnapshot's snapshot-resolution path and the resource-type label the shared not-found constructor stamps for snapshots.

Cause 5 of 6: DeleteClusterSnapshot on a snapshotArn no snapshot carries (fixture docdb-elastic-cluster-snapshot-ops-missing-resource-not-found, step task#3).
AWS: 404, message "Unable to find the snapshot", resourceType "Snapshot".
Emulator: message "ClusterSnapshot <snapshotArn> not found", resourceType "ClusterSnapshot".
Why: same as cause 4, on the snapshot-delete path.
Fix: DeleteClusterSnapshot's snapshot-resolution path / the snapshot resource-type label.

Cause 6 of 6: GetCluster on a clusterArn no cluster carries (fixture docdb-elastic-get-cluster-nonexistent-not-found, step task#1).
AWS: 404 with the fixed message "Unable to find the cluster".
Emulator: "Cluster <clusterArn> not found".
Why: same as cause 1, on the primary cluster read — the most commonly exercised not-found path of the service.
Fix: GetCluster's cluster-resolution path / shared not-found constructor.

Note: the earlier issue "DocDB Elastic ResourceNotFoundException reports lowercase resourceType instead of the AWS resource type name" is RESOLVED and does not cover these — the remaining defect is the invented message template and the snapshot type name being "ClusterSnapshot" rather than "Snapshot". The content-type header these same steps also get wrong is filed separately.

## Evidence
Replay of trace set simparity-wire-aws-v1, verdict mismatch on all six steps (status matched, body members diverged):
- UpdateCluster, docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found#task#1: 404 vs 404; $.message expected 'Unable to find the cluster', got 'Cluster arn:aws:docdb-elastic:<region>:<account>:cluster/<id> not found'.
- DeleteCluster, same fixture#task#3: $.message expected 'Unable to find the cluster', got 'Cluster arn:aws:docdb-elastic:<region>:<account>:cluster/<id> not found'.
- CreateClusterSnapshot, docdb-elastic-cluster-snapshot-ops-missing-resource-not-found#task#1: $.message expected 'Unable to find the cluster', got 'Cluster arn:...:cluster/<id> not found'.
- GetClusterSnapshot, same fixture#task#2: $.message expected 'Unable to find the snapshot', got 'ClusterSnapshot arn:...:cluster-snapshot/<id> not found'; $.resourceType expected 'Snapshot', got 'ClusterSnapshot'.
- DeleteClusterSnapshot, same fixture#task#3: identical message and resourceType divergence.
- GetCluster, docdb-elastic-get-cluster-nonexistent-not-found#task#1: $.message expected 'Unable to find the cluster', got 'Cluster arn:...:cluster/<id> not found'.
Fixed text per class: "Unable to find the cluster" / "Unable to find the snapshot" (no variable parts). Variable parts of the envelope: resourceId = the ARN from the request; resourceType = the AWS type name ('Cluster', 'Snapshot'). The ARNs, account ids and uuids in the recordings are per-run values, not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /cluster/{clusterArn} with an ARN for a cluster that was never created -> observe 404 whose message is 'Cluster <arn> not found' instead of 'Unable to find the cluster'.
3. PUT /cluster/{clusterArn} (UpdateCluster) and DELETE /cluster/{clusterArn} with the same unknown ARN -> same invented message.
4. POST /cluster-snapshot naming that unknown clusterArn (CreateClusterSnapshot) -> same invented cluster message.
5. GET /cluster-snapshot/{snapshotArn} and DELETE /cluster-snapshot/{snapshotArn} with an unknown snapshot ARN -> message 'ClusterSnapshot <arn> not found' instead of 'Unable to find the snapshot', and resourceType 'ClusterSnapshot' instead of 'Snapshot'.
6. Or replay fixtures docdb-elastic-get-cluster-nonexistent-not-found (task#1), docdb-elastic-cluster-lifecycle-ops-missing-cluster-not-found (task#1, task#3) and docdb-elastic-cluster-snapshot-ops-missing-resource-not-found (task#1, task#2, task#3).

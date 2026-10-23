Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-cluster-and-multiregion-request-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-cluster-and-multiregion-request-validation memorydb-cluster-and-snapshot-not-found-envelopes memorydb-parameter-group-lifecycle memorydb-parameter-update-and-reset-rules memorydb-user-authentication-rules memorydb-user-lifecycle-and-credential-rules
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|UpdateCluster|4xx->4xx|$.message
Issue-Key: memorydb|UpdateCluster|4xx->4xx|status
Issue-Key: memorydb|CreateCluster|4xx->4xx|$.message
Issue-Key: memorydb|CreateCluster|4xx->4xx|status
Issue-Key: memorydb|DescribeClusters|4xx->4xx|status
Issue-Key: memorydb|DeleteCluster|4xx->4xx|$.message
Issue-Key: memorydb|DeleteCluster|4xx->4xx|status
Issue-Key: memorydb|DescribeParameterGroups|4xx->4xx|$.message
Issue-Key: memorydb|DescribeParameterGroups|4xx->4xx|status
Issue-Key: memorydb|UpdateParameterGroup|4xx->4xx|status
Issue-Key: memorydb|DescribeUsers|4xx->4xx|status
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# Cluster, user and parameter-group not-found faults answer 404

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCluster

## Description
Seven causes, one shared fault, the cluster/user/parameter-group half of the same status-mapping defect: the emulator answers the MemoryDB *NotFoundFault family with HTTP 404 where the recorded AWS endpoint answers HTTP 400 with the same __type. Smithy declares @httpError 404 on these fault shapes (e.g. ClusterNotFoundFault, ACLNotFoundFault, ParameterGroupNotFoundFault, UserNotFoundFault — all errorKind client, awsQueryError inert), but the recording shows the live awsJson1_1 endpoint returning 400, and the recorded wire status is the requirement. All seven causes must be closed before submitting.

Cause 1 of 7: UpdateCluster on a missing cluster. AWS: recorded HTTP 400, __type ClusterNotFoundFault. Emulator: HTTP 404 (message 'Cluster <name> not found.' vs recorded 'Cluster not found.' — example only). Why: a client distinguishing client faults (400) from endpoint/routing failures (404) misclassifies a missing cluster. Fix: the memorydb error serializer's fault-to-status mapping. Traces: memorydb-cluster-and-multiregion-request-validation#task#2.

Cause 2 of 7: CreateCluster naming an ACL that does not exist. AWS: recorded HTTP 400 __type ACLNotFoundFault (Smithy: CreateCluster errors include ACLNotFoundFault). Emulator: HTTP 404. Fix: same mapping via the CreateCluster handler's ACL existence check. Traces: memorydb-cluster-and-multiregion-request-validation#task#8.

Cause 3 of 7: DescribeClusters for a missing cluster. AWS: HTTP 400 ClusterNotFoundFault. Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-cluster-and-snapshot-not-found-envelopes#task#2.

Cause 4 of 7: DeleteCluster on a missing cluster. AWS: HTTP 400 ClusterNotFoundFault, recorded message 'Specified cluster does not exist'. Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-cluster-and-snapshot-not-found-envelopes#task#3.

Cause 5 of 7: DescribeParameterGroups for a missing parameter group. AWS: HTTP 400 ParameterGroupNotFoundFault. Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-parameter-group-lifecycle#task#5.

Cause 6 of 7: UpdateParameterGroup on a missing parameter group. AWS: HTTP 400. Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-parameter-update-and-reset-rules#task#7.

Cause 7 of 7: DescribeUsers for a missing user. AWS: HTTP 400 UserNotFoundFault (Smithy: DescribeUsers errors are InvalidParameterCombinationException and UserNotFoundFault). Emulator: HTTP 404. Fix: same mapping. Traces: memorydb-user-authentication-rules#task#11.

## Evidence
Cause 1: expected 400, got 404; $.message 'Cluster not found.' vs 'Cluster <name> not found.' (memorydb-cluster-and-multiregion-request-validation task#2). Cause 2: expected 400, got 404 (same fixture task#8, CreateCluster, ACLNotFoundFault). Cause 3: expected 400, got 404 (memorydb-cluster-and-snapshot-not-found-envelopes task#2, DescribeClusters). Cause 4: expected 400, got 404 (same fixture task#3, DeleteCluster). Cause 5: expected 400, got 404 (memorydb-parameter-group-lifecycle task#5, DescribeParameterGroups). Cause 6: expected 400, got 404 (memorydb-parameter-update-and-reset-rules task#7, UpdateParameterGroup). Cause 7: expected 400, got 404 (memorydb-user-authentication-rules task#11, DescribeUsers; cluster also covers task#13 and memorydb-user-lifecycle-and-credential-rules task#6/teardown#12).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 and issue each call against a resource that does not exist: UpdateCluster, DescribeClusters, DeleteCluster (missing cluster), CreateCluster with a missing ACLName, DescribeParameterGroups and UpdateParameterGroup (missing parameter group), DescribeUsers (missing user). Each must answer HTTP 400 with the modeled NotFound __type; the emulator answers HTTP 404. Fixtures/steps: memorydb-cluster-and-multiregion-request-validation#task#2 and #task#8, memorydb-cluster-and-snapshot-not-found-envelopes#task#2 and #task#3, memorydb-parameter-group-lifecycle#task#5, memorydb-parameter-update-and-reset-rules#task#7, memorydb-user-authentication-rules#task#11.

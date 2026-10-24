Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-arn-validation-by-resource-type
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-arn-validation-by-resource-type memorydb-cluster-and-multiregion-request-validation memorydb-events-updates-and-offerings-validation memorydb-user-authentication-rules
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|ListTags|4xx->4xx|$.__type
Issue-Key: memorydb|DeleteCluster|4xx->4xx|$.__type
Issue-Key: memorydb|CreateCluster|4xx->4xx|$.__type
Issue-Key: memorydb|DescribeEvents|4xx->4xx|$.__type
Issue-Key: memorydb|DescribeEvents|4xx->4xx|$.message
Issue-Key: memorydb|DescribeEngineVersions|4xx->4xx|$.__type
Issue-Key: memorydb|DescribeEngineVersions|4xx->4xx|$.message
Issue-Key: memorydb|UpdateUser|4xx->4xx|$.__type
Issue-Key: memorydb|UpdateUser|4xx->4xx|$.message
Issue-Related: 20261023T161602Z-memorydb-acl-and-tagging-not-found-faults-answer-http.md
Issue-Related: 20261023T161602Z-memorydb-cluster-user-and-parameter-group-not-found-faults.md
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# MemoryDB error codes diverge: wrong fault type for rejected requests

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTags

## Description
Eight causes, one shared fault: the emulator rejects the request but with a different __type (error code) than AWS, so clients that branch on the error code take the wrong path. In several causes the emulator also applies its validation in a different order than AWS, surfacing a later check's fault instead of the one AWS reports first. All eight causes must be closed before submitting.

Cause 1 of 8: ListTags on a multi-region-cluster ARN whose cluster does not exist. AWS: HTTP 400, __type 'MultiRegionClusterNotFoundFault' (Smithy: ListTags errors include MultiRegionClusterNotFoundFault and InvalidARNFault), recorded message '<name> is not present'. Emulator: HTTP 400 with __type 'InvalidARNFault' — it treats the well-formed multiregioncluster ARN as malformed instead of resolving it and reporting the resource missing. Why: a client distinguishing 'bad ARN' from 'resource gone' cannot. Fix: the ListTags ARN parser's resource-type table (accept the multiregioncluster type) plus the not-found branch. Traces: memorydb-arn-validation-by-resource-type#task#15.

Cause 2 of 8: ListTags on an ARN whose region/account does not match the endpoint. AWS: HTTP 400, __type 'InvalidParameterValueException', recorded message 'Please check the region or customer ID specified in the request.'. Emulator: HTTP 404 with __type 'ParameterGroupNotFoundFault' — both the status and the code are wrong; it skips the region/account check and falls through to a resource lookup. Fix: the ListTags ARN validator's region/account check, ahead of the resource lookup. Traces: memorydb-arn-validation-by-resource-type#task#17.

Cause 3 of 8: DeleteCluster with FinalSnapshotName for a cluster that does not exist. AWS: HTTP 400, __type 'ClusterNotFoundFault', recorded message 'Specified cluster does not exist'. Emulator: HTTP 400 with __type 'InvalidParameterValueException' and a message stating that snapshots are not implemented. The cluster-existence check must come first and report ClusterNotFoundFault regardless of the unimplemented snapshot path. Fix: the DeleteCluster handler's validation order. Traces: memorydb-cluster-and-multiregion-request-validation#task#3.

Cause 4 of 8: CreateCluster with an invalid NodeType and a nonexistent ACL. AWS: HTTP 400, __type 'InvalidParameterValueException', recorded message 'Invalid node type specified.' — the node-type check runs before ACL resolution. Emulator: HTTP 404 with __type 'ACLNotFoundFault'. Fix: the CreateCluster handler's validation order (NodeType before ACLName). Traces: memorydb-cluster-and-multiregion-request-validation#task#9.

Cause 5 of 8: CreateCluster with an engine that does not support replication. AWS: HTTP 400, __type 'InvalidParameterCombinationException' (Smithy: CreateCluster declares both InvalidParameterCombinationException and InvalidParameterValueException), recorded message 'Specified engine does not support replication.'. Emulator: __type 'InvalidParameterValueException'. Fix: the CreateCluster engine/replica validation's chosen fault shape. Traces: memorydb-cluster-and-multiregion-request-validation#task#15.

Cause 6 of 8: DescribeEvents asking for events older than the retention window. AWS: HTTP 400, __type 'InvalidParameterCombinationException', recorded message 'Events occurring more than 14 days in the past are not available.'. Emulator: __type 'InvalidParameterValueException' from a Duration 1..20160 bound — a different rule and a different code. Fix: the DescribeEvents StartTime/Duration window check. Traces: memorydb-events-updates-and-offerings-validation#task#5.

Cause 7 of 8: DescribeEngineVersions with an unknown ParameterGroupFamily. AWS: HTTP 400, __type 'InvalidParameterValueException', recorded message 'Invalid engine version or parameter group family was provided.'. Emulator: __type 'InvalidParameterCombinationException' — the two codes are swapped relative to cause 6. Fix: the DescribeEngineVersions filter validation's fault shape. Traces: memorydb-events-updates-and-offerings-validation#task#19.

Cause 8 of 8: UpdateUser with no modifiable field supplied. AWS: HTTP 400, __type 'InvalidParameterValueException', recorded message 'Update user needs to have at least one field specified.'. Emulator: __type 'InvalidParameterCombinationException'. Fix: the UpdateUser handler's empty-update check. Traces: memorydb-user-authentication-rules#task#10.

## Evidence
Cause 1: $.__type expected 'MultiRegionClusterNotFoundFault', got 'InvalidARNFault' (memorydb-arn-validation-by-resource-type task#15, also task#16). Cause 2: expected 400, got 404; $.__type expected 'InvalidParameterValueException', got 'ParameterGroupNotFoundFault' (task#17, also task#18). Cause 3: $.__type expected 'ClusterNotFoundFault', got 'InvalidParameterValueException' (memorydb-cluster-and-multiregion-request-validation task#3). Cause 4: expected 400, got 404; $.__type expected 'InvalidParameterValueException', got 'ACLNotFoundFault' (same fixture task#9, also task#13/task#14). Cause 5: $.__type expected 'InvalidParameterCombinationException', got 'InvalidParameterValueException' (task#15). Cause 6: $.__type expected 'InvalidParameterCombinationException', got 'InvalidParameterValueException' (memorydb-events-updates-and-offerings-validation task#5, also task#7). Cause 7: $.__type expected 'InvalidParameterValueException', got 'InvalidParameterCombinationException' (same fixture task#19, also task#20). Cause 8: $.__type expected 'InvalidParameterValueException', got 'InvalidParameterCombinationException' (memorydb-user-authentication-rules task#10). Smithy: both InvalidParameterValueException and InvalidParameterCombinationException are modeled @httpError 400 on these operations, so the code — not the status — is the contract being broken.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 and compare the __type of each rejection: ListTags on a multiregioncluster ARN for a missing cluster, and on an ARN with a foreign region/account; DeleteCluster with FinalSnapshotName for a missing cluster; CreateCluster with an invalid NodeType plus a missing ACL, and with a non-replicating engine; DescribeEvents with a StartTime older than 14 days; DescribeEngineVersions with an unknown ParameterGroupFamily; UpdateUser with neither AccessString nor AuthenticationMode. Fixtures/steps: memorydb-arn-validation-by-resource-type#task#15 and #task#17, memorydb-cluster-and-multiregion-request-validation#task#3, #task#9, #task#15, memorydb-events-updates-and-offerings-validation#task#5 and #task#19, memorydb-user-authentication-rules#task#10.

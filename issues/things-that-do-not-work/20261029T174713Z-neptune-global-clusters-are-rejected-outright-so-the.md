Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-global-cluster-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-global-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|CreateGlobalCluster|2xx->4xx|$.CreateGlobalClusterResponse
Issue-Key: neptune|CreateGlobalCluster|2xx->4xx|status
Issue-Key: neptune|DescribeGlobalClusters|2xx->4xx|$
Issue-Key: neptune|DescribeGlobalClusters|2xx->4xx|$.DescribeGlobalClustersResponse
Issue-Key: neptune|DescribeGlobalClusters|2xx->4xx|$.DescribeGlobalClustersResponse.DescribeGlobalClustersResult.GlobalClusters.GlobalClusterMember.Status
Issue-Key: neptune|DescribeGlobalClusters|2xx->4xx|status
Issue-Key: neptune|CreateGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: neptune|CreateGlobalCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|DeleteGlobalCluster|2xx->4xx|$.DeleteGlobalClusterResponse
Issue-Key: neptune|DeleteGlobalCluster|2xx->4xx|status
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# Neptune global clusters are rejected outright, so the whole global-database lifecycle is unusable

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGlobalCluster

## Description
The emulator refuses to create a Neptune global cluster at all — it validates Engine against an Aurora-only engine list — and every downstream global-cluster operation then fails because no global cluster record exists. THIS ISSUE HAS 4 CAUSES — all four must be closed before submitting.

Cause 1 of 4: CreateGlobalCluster rejects Engine 'neptune'. AWS: Smithy com.amazonaws.neptune#CreateGlobalCluster takes GlobalClusterIdentifier (required, length 1-255, pattern ^[A-Za-z][0-9A-Za-z-:._]*$) and Engine, and returns a GlobalCluster whose Engine member is documented as 'The Neptune database engine used by the global database ("neptune")' — so 'neptune' is the engine this API exists for. The recording answers 200 with a GlobalCluster carrying Status 'available', Engine 'neptune', EngineVersion, GlobalClusterArn, GlobalClusterResourceId, StorageEncrypted, DeletionProtection and empty GlobalClusterMembers/TagList. Its modelled errors are DBClusterNotFoundFault, GlobalClusterAlreadyExistsFault, GlobalClusterQuotaExceededFault and InvalidDBClusterStateFault — none covers an unsupported engine. Emulator: 400 InvalidParameterValue, 'invalid value neptune for Engine: a global database cluster runs one of aurora-mysql, aurora-postgresql'. Why: Neptune global databases cannot be created at all; the engine check is inherited from an Aurora code path and excludes the only engine this service supports. Fix: the CreateGlobalCluster handler's Engine validation and the global-cluster record it mints in state. Traces: neptune-global-cluster-lifecycle#task#1.

Cause 2 of 4: DescribeGlobalClusters never reports the global cluster. AWS: DescribeGlobalClusters answers 200 with the global cluster reaching Status 'available'. Emulator: 404 GlobalClusterNotFoundFault, "Global cluster '<identifier>' not found", for the full 60s poll window. Why: a client waiting for the global cluster to become available hangs until its own timeout. This follows from cause 1 but must be re-verified on the read path once creation works, including the Status transition to 'available'. Fix: the DescribeGlobalClusters handler and the global-cluster store it reads. Traces: neptune-global-cluster-lifecycle#task#2.

Cause 3 of 4: a duplicate CreateGlobalCluster is refused under the wrong code and message. AWS: 400 with Error.Code GlobalClusterAlreadyExistsFault (modelled, httpError 400, awsQueryError code GlobalClusterAlreadyExistsFault) worded 'Global cluster <identifier> already exists' — fixed text around the requested GlobalClusterIdentifier. Emulator: 400 InvalidParameterValue with the Aurora engine-list sentence from cause 1, because the engine gate runs before any conflict check. Why: a client that treats GlobalClusterAlreadyExistsFault as idempotent success instead sees an unrelated validation error. Fix: the CreateGlobalCluster handler's refusal ordering — the identifier-conflict check must be reachable and must raise GlobalClusterAlreadyExistsFault. Traces: neptune-global-cluster-lifecycle#task#3.

Cause 4 of 4: DeleteGlobalCluster 404s instead of returning the deleted global cluster. AWS: DeleteGlobalCluster answers 200 with the GlobalCluster it removed (identifier, ARN, Engine, EngineVersion, Status, StorageEncrypted, DeletionProtection, GlobalClusterMembers, TagList). Emulator: 404 with no DeleteGlobalClusterResponse. Why: teardown of a global database cannot complete and the caller cannot confirm what was deleted. Fix: the DeleteGlobalCluster handler and its GlobalCluster projection. Traces: neptune-global-cluster-lifecycle#task#6.

## Evidence
Replay of simparity-wire-aws-v1, fixture neptune-global-cluster-lifecycle:
task#1 CreateGlobalCluster: status expected 200, got 400; $.CreateGlobalClusterResponse missing (expected CreateGlobalClusterResult.GlobalCluster with Engine 'neptune', Status 'available', GlobalClusterArn, StorageEncrypted 'true', DeletionProtection 'false', empty GlobalClusterMembers/TagList).
task#2 DescribeGlobalClusters: status expected 200, got 404; poll capped at 60s — GlobalClusterMember.Status not in ['available'] after 61 attempts; body was ErrorResponse{Code 'GlobalClusterNotFoundFault', Message "Global cluster '<id>' not found"}.
task#3 CreateGlobalCluster (duplicate): Error.Code expected 'GlobalClusterAlreadyExistsFault', got 'InvalidParameterValue'; Message expected 'Global cluster <id> already exists', got 'invalid value neptune for Engine: a global database cluster runs one of aurora-mysql, aurora-postgresql'.
task#6 DeleteGlobalCluster: status expected 200, got 404; $.DeleteGlobalClusterResponse missing.

## Steps to Reproduce
1. Replay fixture neptune-global-cluster-lifecycle.
2. Step task#1 CreateGlobalCluster with Engine 'neptune' -> observe 400 InvalidParameterValue instead of 200 with a GlobalCluster.
3. Step task#2 DescribeGlobalClusters -> observe a 404 poll that never reaches Status 'available'.
4. Step task#3 re-create the same identifier -> observe InvalidParameterValue instead of GlobalClusterAlreadyExistsFault.
5. Step task#6 DeleteGlobalCluster -> observe 404 instead of 200.

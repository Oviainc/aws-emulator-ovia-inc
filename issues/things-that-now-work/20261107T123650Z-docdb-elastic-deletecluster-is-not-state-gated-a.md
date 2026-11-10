Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.delete-cluster-while-stopped-rejected
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.delete-cluster-while-stopped-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# DocDB Elastic DeleteCluster is not state-gated: a STOPPED cluster is deleted with 200 instead of being refused with ValidationException

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
Cause 1 of 1: DeleteCluster performs no cluster-status gate, so it accepts and destroys a cluster that AWS refuses to delete in its current state.

AWS: An elastic cluster that has been stopped (StopCluster -> status STOPPED) cannot be deleted until it is started again. AWS refuses DeleteCluster on a STOPPED cluster with HTTP 400 ValidationException (Smithy: DeleteCluster errors include ValidationException @httpError 400, "A structure defining a validation exception"; the model also binds ConflictException 409, ResourceNotFoundException 404), and the cluster remains STOPPED and still readable via GetCluster. The model does not state the state machine itself, so the message is recorded behavior only: the fixed text is "Can't delete the cluster because it is in STOPPED state." — the only variable part of that template is the cluster's current status word taken from service state (the recorded refusal for a stopped cluster renders it as STOPPED). The positive control in the same recording shows the same cluster deletes normally once StartCluster has returned it to ACTIVE, so the refusal is purely a function of the stored status, not of the ARN.

Emulator: DeleteCluster answers HTTP 200 and removes the record unconditionally. Replay of docdb-elastic.delete-cluster-while-stopped-rejected shows the call on a STOPPED cluster returning 200 with cluster.status = "DELETING" and every shard status "DELETING" (the check asserted a refusal and reported "expected the call to be rejected, but it SUCCEEDED and returned {...'status': 'DELETING'...}"). No ValidationException is produced for any status: the handler removes the cluster from the store the moment the ARN resolves, and the only error path it has is ResourceNotFoundException for an ARN that is already absent. Because the record is removed before the status is even inspected, the cluster is also unrecoverable — GetCluster then answers ResourceNotFoundException rather than reporting it still STOPPED.

Why: a client that relies on AWS's state gate loses data. Teardown and lifecycle code commonly issues DeleteCluster speculatively and treats the 400 ValidationException as "start it first, then delete"; against the emulator that guard never fires, so a stopped cluster that AWS would have preserved is destroyed, and the branch that calls StartCluster before deleting is never exercised. Conversely, code that asserts the refusal (error-class and negative-path tests, operator confirmation flows) sees a success and concludes the service permits an operation that real AWS rejects. The missing gate is generic, so the same hole exists for any status AWS refuses deletion in, not only STOPPED.

Fix: belongs in the DeleteCluster handler's pre-conditions — the operation must read the stored cluster's status and refuse before mutating the store, returning the modeled ValidationException (400) with AWS's sentence for a cluster whose status forbids deletion, and leaving the record and its shards untouched so a following GetCluster still reports the cluster in that status. The deletion itself should only run for a status AWS accepts.

Traces: docdb-elastic.delete-cluster-while-stopped-rejected (the suite's only unowned docdb-elastic failure; the other docdb-elastic failures in this run are already owned by 20261102T113456Z-docdb-elastic-conflict-validation-refusals-on-createcluster-updatecluster.md, 20261031T144015Z-docdb-elastic-createclustersnapshot-duplicate-name-conflictexception-message-is.md, and the filed StartCluster unimplemented-stub issue, and are not part of this cause).

## Evidence
Check docdb-elastic.delete-cluster-while-stopped-rejected, after CreateCluster -> ACTIVE -> StopCluster -> STOPPED:

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "Can't delete the cluster because it is in STOPPED state." (fixed text; the status word is taken from the cluster's stored status). GetCluster afterwards still reports status STOPPED. StartCluster then DeleteCluster succeeds.

Actual (emulator): HTTP 200 with a DeleteClusterOutput body, e.g. cluster = {authType: PLAIN_TEXT, clusterArn: arn:aws:docdb-elastic:<region>:<account>:cluster/<id>, clusterName: <name>, shardCapacity: 2, shardCount: 1, shards: [{shardId: <id>, status: DELETING}], status: DELETING, ...} — the ARN, name, ids and timestamps are per-run values; the divergence is the 200 plus status DELETING where a 400 ValidationException was required. The record is removed from the store, so the cluster is gone rather than preserved in STOPPED.

Handler evidence: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/delete_cluster.rs lines 157-179 — `guard.clusters.remove(&input.cluster_arn)` runs first and the only modeled error raised is ResourceNotFoundException; `record.status` is never consulted before deletion, so no status can refuse the call.

## Steps to Reproduce
1. CreateCluster(clusterName=<name>, authType=PLAIN_TEXT, adminUserName=..., adminUserPassword=..., shardCapacity=2, shardCount=1).
2. Poll GetCluster until status == ACTIVE.
3. StopCluster(clusterArn=<arn>); poll GetCluster until status == STOPPED.
4. Call DeleteCluster(clusterArn=<arn>).
   Expected: HTTP 400 ValidationException "Can't delete the cluster because it is in STOPPED state."; GetCluster still reports STOPPED.
   Observed: HTTP 200 with cluster.status == "DELETING"; the cluster is removed and GetCluster then answers ResourceNotFoundException.
5. (Positive control that must keep working) StartCluster -> ACTIVE -> DeleteCluster succeeds.

Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.start-cluster-when-active-rejected
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.start-cluster-when-active-rejected|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: eaa123f65a60b02b9ee1461dec055a780335fce9

# DocDB Elastic StartCluster is not state-gated: starting an ACTIVE cluster answers NotImplemented instead of ValidationException

> _Note: this issue was identified against the code base at commit `eaa123f65a60b02b9ee1461dec055a780335fce9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCluster

## Description
Cause 1 of 1: StartCluster has no state gate (and, at this commit, no handler at all), so the refusal real DocDB Elastic gives for starting a cluster that is not STOPPED never happens.

AWS: StartCluster (POST /cluster/{clusterArn}/start, Smithy: StartClusterInput.clusterArn @required @httpLabel; modeled errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400) only starts a cluster whose status is STOPPED. Called on a cluster whose status is ACTIVE, AWS refuses with HTTP 400 ValidationException and the fixed sentence "Can't start the cluster as it's currently in state unavailable for starting." (recorded behavior: the whole sentence is fixed text — it interpolates no cluster name, ARN or status). The refusal has no effect: the cluster stays ACTIVE and no STARTING transition is entered. A subsequent StopCluster then StartCluster on the same cluster succeeds and returns it to ACTIVE, so the gate is on the current status only, not on the cluster or the caller.

Emulator: the call is refused with the scaffold answer — HTTP 501, error code `NotImplemented`, message "operation StartCluster is scaffolded but not implemented in this emulator". The check asserts the error code and fails with `AssertionError: expected ValidationException, got NotImplemented`, so neither the status/code/message triple nor the no-effect and positive-control halves of the behavior are reachable.

Why: a client that drives the documented cluster lifecycle cannot distinguish "this cluster is already running" from "this API does not exist". Code that catches ValidationException with that sentence to treat an already-running cluster as success instead sees an unmodeled 501 that botocore surfaces as a non-retryable client-visible failure, and a client that trusts a 2xx-or-ValidationException contract to confirm the cluster never left ACTIVE gets no answer at all. The positive path (STOPPED -> StartCluster -> ACTIVE) is equally unavailable.

Fix: the StartCluster handler and its state gate — `services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/start_cluster.rs`. It must read the cluster's current status from the service state and, for any status other than STOPPED, return the modeled ValidationException (HTTP 400) with AWS's fixed sentence and leave the stored cluster untouched; for STOPPED it must transition the cluster back to ACTIVE (via STARTING) and return the Cluster shape. Related open issue `20261107T123650Z-docdb-elastic-startcluster-stub.md` asks for StartCluster to be implemented at all and carries only `docdb-elastic.update-cluster-while-stopped-rejected` as its replay key; it does not state this refusal requirement nor carry this check, so implementing a happy-path StartCluster would close that issue and still leave this check failing.

Traces: docdb-elastic.start-cluster-when-active-rejected.

## Evidence
Check docdb-elastic.start-cluster-when-active-rejected (verdict: mismatch).

Expected (real AWS): StartCluster on an ACTIVE cluster -> HTTP 400, Error.Code = "ValidationException", Error.Message = "Can't start the cluster as it's currently in state unavailable for starting." (entirely fixed text), cluster still ACTIVE on a following GetCluster; then StopCluster -> STOPPED, StartCluster -> ACTIVE.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation StartCluster is scaffolded but not implemented in this emulator".

Failure output:
  File "check_docdb_elastic_start_cluster_when_active_rejected.py", line 37, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}"
  AssertionError: expected ValidationException, got NotImplemented

Smithy (com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda): StartCluster POST /cluster/{clusterArn}/start, success 200, errors include ValidationException (client, httpError 400). NotImplemented/501 is not a modeled error of this operation.

## Steps to Reproduce
1. Boot the docdb-elastic emulator.
2. CreateCluster with authType=PLAIN_TEXT, an admin user/password, shardCapacity=2, shardCount=1; wait until GetCluster reports status ACTIVE.
3. Call StartCluster(clusterArn=<that ARN>) while the cluster is ACTIVE.
4. Observe HTTP 501 / NotImplemented instead of HTTP 400 ValidationException with "Can't start the cluster as it's currently in state unavailable for starting."
5. Call GetCluster and confirm the cluster is still ACTIVE (the refusal must have no effect).
6. Positive control: StopCluster, wait for STOPPED, then StartCluster and wait for ACTIVE — this path must succeed.

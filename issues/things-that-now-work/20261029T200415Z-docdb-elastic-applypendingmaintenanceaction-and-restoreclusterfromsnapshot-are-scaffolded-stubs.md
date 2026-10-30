Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.apply-maintenance-action-missing-resource-not-found docdb-elastic.apply-pending-maintenance-action-missing-apply-on-rejected docdb-elastic.apply-pending-maintenance-action-not-pending-rejected docdb-elastic.restore-cluster-missing-snapshot-not-found docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.apply-maintenance-action-missing-resource-not-found|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: docdb-elastic.apply-pending-maintenance-action-missing-apply-on-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: docdb-elastic.apply-pending-maintenance-action-not-pending-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: docdb-elastic.restore-cluster-missing-snapshot-not-found|run#6|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# DocDB Elastic ApplyPendingMaintenanceAction and RestoreClusterFromSnapshot are scaffolded stubs that answer NotImplemented

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyPendingMaintenanceAction

## Description
Two modeled DocDB Elastic control-plane operations are still scaffold stubs: every call reaches `Scaffolded::not_implemented(..)` and the client sees HTTP 501 with error code `NotImplemented` and message "operation <Op> is scaffolded but not implemented in this emulator", so none of their modeled success or refusal behavior exists. Two causes, one per operation. (The suite report did not mark these as already filed, and no open issue names either operation; the two open docdb-elastic wire issues cover CreateCluster wording and not-found wording only.)

Cause 1 of 2: ApplyPendingMaintenanceAction is unimplemented.
AWS: POST /pending-action, 200 on success with a required `resourcePendingMaintenanceAction`. Smithy: ApplyPendingMaintenanceAction input resourceArn/applyAction/optInType required (optInType one of APPLY_ON | IMMEDIATE | NEXT_MAINTENANCE | UNDO_OPT_IN), applyOn documented "Required if opt-in-type is APPLY_ON. Format: yyyy/MM/dd HH:mm-yyyy/MM/dd HH:mm"; modeled errors include ResourceNotFoundException (404), ValidationException (400), ConflictException (409). The three failing checks pin three distinct refusals AWS serves from this operation: (a) a syntactically valid but nonexistent resourceArn -> 404 ResourceNotFoundException with the message "Unable to find the cluster" (whole message is fixed text, no variable part — the ARN is reported in resourceId, not spliced into the message); (b) optInType=APPLY_ON with no applyOn -> 400 ValidationException with the message "ApplyOn is required when OptInType is APPLY_ON" (entirely fixed text; "ApplyOn"/"OptInType" are the member names, not request values); (c) an applyAction that is not actually pending for the resource -> 400 ValidationException with the message "Pending maintenance action not found for ResourceArn <resourceArn>", whose fixed text is "Pending maintenance action not found for ResourceArn " and whose only variable part is the resourceArn taken verbatim from the request. In all three cases the call must have no effect: no pending-action record is created, and GetPendingMaintenanceAction on an existing resource with nothing pending still answers 200 with an empty pendingMaintenanceActionDetails list (recorded behavior only — it is NOT a 404).
Emulator: every call answers 501 NotImplemented, so the error code is `NotImplemented` where ResourceNotFoundException or ValidationException is required, and the handler body is the generated stub.
Why: a client cannot opt in to or trigger a maintenance action at all, and error-handling code that branches on ResourceNotFoundException/ValidationException sees an unmodeled 501 instead — retry/classification logic treats it as a server-side gap rather than a client-side refusal, and nothing validates applyOn or the pending-action set.
Fix: the operation's handler, services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/apply_pending_maintenance_action.rs (replace the `not_implemented` return with resourceArn resolution against cluster state, the APPLY_ON/applyOn validation, and the pending-action lookup, backed by the service state the pending-maintenance reads share).
Traces: docdb-elastic.apply-maintenance-action-missing-resource-not-found, docdb-elastic.apply-pending-maintenance-action-missing-apply-on-rejected, docdb-elastic.apply-pending-maintenance-action-not-pending-rejected.

Cause 2 of 2: RestoreClusterFromSnapshot is unimplemented.
AWS: POST /cluster-snapshot/{snapshotArn}/restore, 200 on success with a required `cluster` whose clusterArn is a NEW, distinct cluster. Smithy: RestoreClusterFromSnapshot input clusterName and snapshotArn required; modeled errors include ResourceNotFoundException (404), ValidationException (400), ConflictException (409). The two failing checks pin: (a) a syntactically valid but nonexistent snapshotArn -> 404 ResourceNotFoundException with the fixed message "Unable to find the snapshot" (no variable part), and NO cluster created — the requested clusterName must not appear in ListClusters afterwards; (b) a clusterName that already names an existing cluster -> 400 ValidationException (AWS validates clusterName uniqueness as request validation, NOT as a state conflict, so ConflictException is wrong here), with the source cluster left untouched (same clusterArn, still ACTIVE), while a restore under a fresh unused name still succeeds and yields a cluster ARN distinct from the source.
Emulator: every call answers 501 NotImplemented; the handler body is the generated stub.
Why: snapshot restore — the whole point of taking a cluster snapshot — is unavailable, and a client gets an unmodeled 501 where AWS distinguishes "snapshot missing" (404) from "name already taken" (400 ValidationException), so neither branch of its error handling is exercisable.
Fix: the operation's handler, services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/restore_cluster_from_snapshot.rs (snapshot resolution, clusterName-uniqueness validation, and creation of a new cluster in service state from the snapshot's recorded configuration).
Traces: docdb-elastic.restore-cluster-missing-snapshot-not-found, docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service docdb-elastic (10/34 passed).

Cause 1 — ApplyPendingMaintenanceAction (3 checks, all verdict mismatch):
- docdb-elastic.apply-maintenance-action-missing-resource-not-found: AssertionError: expected ResourceNotFoundException, got NotImplemented (the check also requires message == "Unable to find the cluster").
- docdb-elastic.apply-pending-maintenance-action-missing-apply-on-rejected: AssertionError: expected ValidationException, got 'NotImplemented' (required message "ApplyOn is required when OptInType is APPLY_ON").
- docdb-elastic.apply-pending-maintenance-action-not-pending-rejected: AssertionError: expected ValidationException, got 'NotImplemented' (required message "Pending maintenance action not found for ResourceArn <arn>").
Underlying response in all three: HTTP 501, code NotImplemented, message "operation ApplyPendingMaintenanceAction is scaffolded but not implemented in this emulator". Source: src/ops/apply_pending_maintenance_action.rs line 4 carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and line 171 returns Scaffolded::not_implemented("ApplyPendingMaintenanceAction").

Cause 2 — RestoreClusterFromSnapshot (2 checks, verdict mismatch):
- docdb-elastic.restore-cluster-missing-snapshot-not-found: AssertionError: expected ResourceNotFoundException, got NotImplemented (required message "Unable to find the snapshot").
- docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected: AssertionError: expected ValidationException, got 'NotImplemented'.
Underlying response: HTTP 501, code NotImplemented, message "operation RestoreClusterFromSnapshot is scaffolded but not implemented in this emulator". Source: src/ops/restore_cluster_from_snapshot.rs line 4 marker, line 186 returns Scaffolded::not_implemented("RestoreClusterFromSnapshot").

ARNs, account ids and uuids in the failing checks are per-run values and are not part of the requirement; the fixed message texts quoted above are.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /pending-action (ApplyPendingMaintenanceAction) with resourceArn=arn:aws:docdb-elastic:<region>:<account>:cluster/<unknown-uuid>, applyAction=system-update, optInType=IMMEDIATE -> observe 501 NotImplemented instead of 404 ResourceNotFoundException "Unable to find the cluster".
3. Create a cluster, wait for ACTIVE, then call ApplyPendingMaintenanceAction on it with optInType=APPLY_ON and no applyOn -> observe 501 instead of 400 ValidationException "ApplyOn is required when OptInType is APPLY_ON"; and with applyAction=ENGINE_UPDATE (not pending) -> observe 501 instead of 400 ValidationException "Pending maintenance action not found for ResourceArn <arn>".
4. POST /cluster-snapshot/{snapshotArn}/restore (RestoreClusterFromSnapshot) with an unknown snapshot ARN -> observe 501 instead of 404 ResourceNotFoundException "Unable to find the snapshot".
5. Take a snapshot of an ACTIVE cluster and restore it using the source cluster's own clusterName -> observe 501 instead of 400 ValidationException.
6. Or re-replay behavioral checks docdb-elastic.apply-maintenance-action-missing-resource-not-found, docdb-elastic.apply-pending-maintenance-action-missing-apply-on-rejected, docdb-elastic.apply-pending-maintenance-action-not-pending-rejected, docdb-elastic.restore-cluster-missing-snapshot-not-found, docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected.

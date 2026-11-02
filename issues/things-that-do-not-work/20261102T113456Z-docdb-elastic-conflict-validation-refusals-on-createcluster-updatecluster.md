Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.concurrent-create-cluster-duplicate-name-conflict docdb-elastic.concurrent-update-cluster-shard-capacity-conflict docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.concurrent-create-cluster-duplicate-name-conflict|run#11|AssertionError|assert errors[0][1] == "A cluster already exists with given name", f"unexpected message: {errors[0]!r}"
Issue-Key: docdb-elastic.concurrent-update-cluster-shard-capacity-conflict|run#12|AssertionError|assert errors[0][1] == "Can't perform operation on cluster as it's in use", f"unexpected message: {errors[0]!r}"
Issue-Key: docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected|run#14|AssertionError|assert message == expected, f"unexpected message: {message!r}"
Issue-Commit: 406bd2208b15ed2c2c5b06aef0a099e8d15489d9

# DocDB Elastic conflict/validation refusals on CreateCluster, UpdateCluster and RestoreClusterFromSnapshot use interpolated prose instead of AWS's sentences

> _Note: this issue was identified against the code base at commit `406bd2208b15ed2c2c5b06aef0a099e8d15489d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Three distinct net-new root causes, all in docdb-elastic refusal messages. Each is its own cause with its own handler site; a fixer must close all three before submitting.

Same family as the already-open issue 20261031T144015Z-docdb-elastic-createclustersnapshot-duplicate-name-conflictexception-message-is.md (CreateClusterSnapshot duplicate name) and 20261029T200415Z-docdb-elastic-createcluster-input-validation-uses-generic-framework.md (CreateCluster field validation wording) — neither covers the three operations/refusals below, so those issues do not close these causes.

Cause 1 of 3 — CreateCluster duplicate clusterName ConflictException message is interpolated prose.
AWS: CreateCluster models ConflictException (Smithy: CreateCluster errors include ConflictException, httpError 409; the model documents only "There was an access conflict.", so the message text is recorded behavior only). The recording shows AWS answering HTTP 409 ConflictException with the FIXED sentence "A cluster already exists with given name" — the whole message is fixed text; it interpolates nothing from the request (no cluster name, no ARN).
Emulator: answers ConflictException (code correct) with a generated sentence that interpolates the stored, lowercased cluster name: 'An elastic cluster named <clusterName> already exists' (replay: 'An elastic cluster named sim2real-docdbe-race-045df3d2bb56 already exists'; the name shown is that run's value, the defect is the template).
Why: a client (or a test/runbook) that recognises the duplicate-name case by the error message — the documented way to tell a name collision from any other access conflict, since both share code ConflictException — does not recognise the emulator's message; and the emulator leaks a resource name into a message AWS keeps value-free.
Fix: the duplicate-name branch of the CreateCluster handler, which builds the ConflictException message.
Traces: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/create_cluster.rs lines 251-258 (format!("An elastic cluster named {cluster_name} already exists")); failing check docdb-elastic.concurrent-create-cluster-duplicate-name-conflict.

Cause 2 of 3 — UpdateCluster on a cluster that is not ACTIVE answers the wrong ConflictException message.
AWS: UpdateCluster models ConflictException (Smithy: UpdateCluster errors include ConflictException, httpError 409; model doc "There was an access conflict." only — message text is recorded behavior only). The recording shows AWS refusing the second of two overlapping UpdateCluster calls with HTTP 409 ConflictException and the FIXED sentence "Can't perform operation on cluster as it's in use" — fully fixed text, no interpolated status or cluster name.
Emulator: answers ConflictException with 'The elastic cluster <clusterName> is in <status> status and cannot be modified until it is ACTIVE' (replay: 'The elastic cluster sim2real-docdbe-60bd3b8f7c08 is in UPDATING status and cannot be modified until it is ACTIVE'; cluster name and status are that run's values, the template is the defect).
Why: the in-use refusal is the one a caller is expected to retry on, and real clients/waiters match it by this exact sentence (the suite's own retry helper keys on the prefix "Can't perform operation on cluster as it's in use"); against the emulator that match never fires, so a retryable in-use refusal is treated as a hard failure.
Fix: the shared state guard that builds this ConflictException for every state-refused cluster modification (used by UpdateCluster and the other lifecycle ops).
Traces: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs lines 865-877 (ensure_modifiable); call site services/.../src/ops/update_cluster.rs line 241; failing check docdb-elastic.concurrent-update-cluster-shard-capacity-conflict.

Cause 3 of 3 — RestoreClusterFromSnapshot under an existing cluster's name answers generic "Invalid Input" framework wording.
AWS: RestoreClusterFromSnapshot models ValidationException (Smithy: errors include ValidationException, httpError 400; model doc "A structure defining a validation exception." — message text is recorded behavior only). The recording shows HTTP 400 ValidationException with the message template: fixed text "Invalid cluster name: Cluster names are case-insensitive and cannot be the same as name as an existing cluster. Cluster name " + <clusterName from the request> + " is not valid because cluster " + <name of the existing cluster in state> + " exists." Only those two names are variable; the rest is fixed.
Emulator: answers ValidationException (status and code correct) with the service's generic validation template instead: "Invalid Input - '<clusterName>'. ClusterName must not name an elastic cluster that already exists." (replay: "Invalid Input - 'sim2real-docdbe-8d282f4d606e'. ClusterName must not name an elastic cluster that already exists.").
Why: a client that distinguishes a name-already-taken restore from other request-validation rejections by the message gets neither the AWS sentence nor the two names AWS names in it, so the specific remediation ("pick another cluster name") is not machine-recognisable.
Fix: the duplicate-cluster-name branch of the RestoreClusterFromSnapshot handler, which should emit this operation's own message rather than the shared invalid_input() framework builder.
Traces: services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/restore_cluster_from_snapshot.rs lines 237-245 (state::invalid_input(...)); failing check docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected.

## Evidence
Cause 1 (docdb-elastic.concurrent-create-cluster-duplicate-name-conflict):
  expected: ('ConflictException', "A cluster already exists with given name")  [HTTP 409]
  actual:   ('ConflictException', 'An elastic cluster named sim2real-docdbe-race-045df3d2bb56 already exists')
  AssertionError: unexpected message: ('ConflictException', 'An elastic cluster named sim2real-docdbe-race-045df3d2bb56 already exists')

Cause 2 (docdb-elastic.concurrent-update-cluster-shard-capacity-conflict):
  expected: ('ConflictException', "Can't perform operation on cluster as it's in use")  [HTTP 409]
  actual:   ('ConflictException', 'The elastic cluster sim2real-docdbe-60bd3b8f7c08 is in UPDATING status and cannot be modified until it is ACTIVE')
  AssertionError: unexpected message: ('ConflictException', 'The elastic cluster sim2real-docdbe-60bd3b8f7c08 is in UPDATING status and cannot be modified until it is ACTIVE')

Cause 3 (docdb-elastic.restore-cluster-from-snapshot-duplicate-cluster-name-rejected):
  expected code ValidationException [HTTP 400] (matched) and message:
    "Invalid cluster name: Cluster names are case-insensitive and cannot be the same as name as an existing cluster. Cluster name <name> is not valid because cluster <name> exists."
  actual message:
    "Invalid Input - 'sim2real-docdbe-8d282f4d606e'. ClusterName must not name an elastic cluster that already exists."
  AssertionError: unexpected message: "Invalid Input - 'sim2real-docdbe-8d282f4d606e'. ClusterName must not name an elastic cluster that already exists."

In all three the HTTP status and error code match AWS; only the message text diverges. Resource names/ids shown are per-run values, not requirements.

## Steps to Reproduce
Cause 1: issue two concurrent CreateCluster calls with the same clusterName (authType=PLAIN_TEXT, adminUserName, adminUserPassword, shardCapacity=2, shardCount=1). One succeeds; read the loser's error — it must be HTTP 409 ConflictException with message exactly "A cluster already exists with given name".

Cause 2: create a cluster, wait for status ACTIVE, then issue two concurrent UpdateCluster calls changing shardCapacity on that clusterArn. One is accepted; the rejected one must be HTTP 409 ConflictException with message exactly "Can't perform operation on cluster as it's in use".

Cause 3: create a cluster (name N), wait ACTIVE, CreateClusterSnapshot on it, wait snapshot ACTIVE, then call RestoreClusterFromSnapshot(clusterName=N, snapshotArn=<snapshot>). It must be refused with HTTP 400 ValidationException and message "Invalid cluster name: Cluster names are case-insensitive and cannot be the same as name as an existing cluster. Cluster name N is not valid because cluster N exists.", leaving the original cluster ACTIVE under its ARN; a restore under an unused name still succeeds.

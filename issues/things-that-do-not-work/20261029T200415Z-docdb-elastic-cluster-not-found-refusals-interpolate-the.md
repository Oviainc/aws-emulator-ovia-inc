Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.get-cluster-nonexistent-not-found docdb-elastic.create-cluster-snapshot-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.get-cluster-nonexistent-not-found|run#6|AssertionError|assert message == "Unable to find the cluster", f"unexpected message: {message!r}"
Issue-Key: docdb-elastic.create-cluster-snapshot-missing-cluster-not-found|run#7|AssertionError|assert message == "Unable to find the cluster", f"unexpected message: {message!r}"
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# DocDB Elastic cluster not-found refusals interpolate the ARN into an emulator-authored message instead of AWS's fixed sentence

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Every DocDB Elastic path that fails to resolve a clusterArn answers the right status and error code (404 / ResourceNotFoundException) with the wrong message: the emulator builds "Cluster <clusterArn> not found", splicing the request's ARN into a template AWS does not use. Two causes, both reached through the crate's shared not-found error constructor. Note: the open issue 20261029T172712Z-docdb-elastic-resourcenotfoundexception-bodies-invent-interpolated-prose-and.md reports the same wording defect from the WIRE corpus (fixtures/steps) and does not carry these behavioral check keys; the two checks below are recorded here so a fixer re-replays them.

Cause 1 of 2: GetCluster on a clusterArn no cluster carries.
AWS: 404 ResourceNotFoundException whose message is the fixed sentence "Unable to find the cluster" — the entire message is fixed text with NO variable part; the ARN from the request is reported in the envelope's resourceId member, never inside the message. Smithy: GetCluster models ResourceNotFoundException at httpError 404 ("The specified resource could not be located"); the exact sentence is recorded behavior. ListClusters on the same client must keep succeeding.
Emulator: 404 ResourceNotFoundException with message "Cluster arn:aws:docdb-elastic:<region>:<account>:cluster/<uuid> not found" — invented template text with the request's ARN interpolated.
Why: a client or test that asserts on, logs or localizes AWS's not-found wording never matches, and the message varies per request where AWS's is constant — so message-based deduplication or caching of the refusal behaves differently than against AWS.
Fix: GetCluster's cluster-resolution path and the shared not-found error constructor it calls, which must emit the fixed per-resource-class sentence and put the ARN in resourceId.
Traces: docdb-elastic.get-cluster-nonexistent-not-found.

Cause 2 of 2: CreateClusterSnapshot naming a clusterArn no cluster carries.
AWS: 404 ResourceNotFoundException with the same fixed message "Unable to find the cluster" — the refusal concerns the source cluster, so it uses the cluster sentence — and no snapshot is created; ListClusterSnapshots on the same client still returns a well-formed response.
Emulator: 404 with message "Cluster arn:aws:docdb-elastic:<region>:<account>:cluster/<uuid> not found".
Why: same as cause 1, on the snapshot-create path.
Fix: CreateClusterSnapshot's source-cluster resolution via the same shared not-found constructor.
Traces: docdb-elastic.create-cluster-snapshot-missing-cluster-not-found.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service docdb-elastic. Both checks verdict mismatch: the error CODE (ResourceNotFoundException) and status (404) matched; only the message diverged.
- [mismatch] docdb-elastic.get-cluster-nonexistent-not-found: AssertionError: unexpected message: 'Cluster arn:aws:docdb-elastic:us-west-2:000000000000:cluster/00000000-0000-0000-0000-000000000000 not found' — expected 'Unable to find the cluster'.
- [mismatch] docdb-elastic.create-cluster-snapshot-missing-cluster-not-found: AssertionError: unexpected message: 'Cluster arn:aws:docdb-elastic:us-west-2:000000000000:cluster/00000000-0000-0000-0000-000000000000 not found' — expected 'Unable to find the cluster'.
Fixed text requirement: the whole message "Unable to find the cluster", with no variable part. The region, account id and uuid in the recorded strings are per-run values and are not part of the requirement; the ARN belongs in the envelope's resourceId member.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. GET /cluster/{clusterArn} (GetCluster) with arn:aws:docdb-elastic:<region>:<account>:cluster/<never-created-uuid> -> observe 404 ResourceNotFoundException whose message is 'Cluster <arn> not found' instead of 'Unable to find the cluster'.
3. POST /cluster-snapshot (CreateClusterSnapshot) with that same unknown clusterArn and any valid snapshotName -> observe the same interpolated cluster message instead of 'Unable to find the cluster'.
4. Confirm ListClusters / ListClusterSnapshots still answer on the same client (both checks' positive controls).
5. Or re-replay behavioral checks docdb-elastic.get-cluster-nonexistent-not-found and docdb-elastic.create-cluster-snapshot-missing-cluster-not-found.

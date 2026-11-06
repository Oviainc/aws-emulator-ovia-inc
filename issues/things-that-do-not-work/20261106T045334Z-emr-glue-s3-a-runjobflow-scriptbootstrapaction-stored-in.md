Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive
Issue-Key: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive|_poll_for_object#13|Timeout|obj = s3.get_object(Bucket=bucket, Key=key)
Issue-Commit: 3c2de54b538cfaae335289815f0a91bb1b09b1bd

# emr__glue__s3: a RunJobFlow ScriptBootstrapAction stored in S3 is never executed, so the cluster's bootstrap side effects never land and the check is cut off at the harness budget

> _Note: this issue was identified against the code base at commit `3c2de54b538cfaae335289815f0a91bb1b09b1bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
Cause 1 of 1: a cluster created by RunJobFlow with a `BootstrapActions` entry whose `ScriptBootstrapAction.Path` points at an S3 object never runs that script, so none of the script's side effects (here: an S3 object the script writes) ever appear. Failing check: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive.

AWS: Smithy `RunJobFlow` input member `BootstrapActions` -> `BootstrapActionConfig{Name (required), ScriptBootstrapAction (required) -> ScriptBootstrapActionConfig{Path (required, S3 location of the script), Args}}`; `ClusterStatus.State` -> `ClusterState` enum = STARTING | BOOTSTRAPPING | RUNNING | WAITING | TERMINATING | TERMINATED | TERMINATED_WITH_ERRORS. On AWS the cluster passes through BOOTSTRAPPING, during which EMR fetches each bootstrap script from its S3 `Path` and runs it on every provisioned node with the EC2 instance profile's credentials; the script's observable effects (objects it writes to S3, files it leaves on the node) are in place by the time the cluster reaches RUNNING/WAITING, and ListBootstrapActions reports the action. A bootstrap script that exits non-zero drives the cluster to TERMINATED_WITH_ERRORS with a bootstrap-failure StateChangeReason rather than leaving the cluster indefinitely non-converged.
Emulator: RunJobFlow accepts the BootstrapActions block and returns a JobFlowId, but no bootstrap phase ever executes the script: the object the script is supposed to write never appears, so the check's poll for that object (`_poll_for_object`, budgeted 300 s against a 1200 s cluster-state poll) never converges and the replay ends with `Error: Check timed out after 60.0s` and no further attribution. Nothing in the response or the cluster read tells the caller the bootstrap action was dropped.
Why: a client cannot use bootstrap actions at all against this emulator. Every provisioning step real clusters do in bootstrap (installing packages, fetching configuration, writing a readiness marker) silently does not happen, while the cluster still claims to come up; and the failure mode is an indefinite hang rather than a terminal state, so a positive bootstrap path and a failing bootstrap path (which AWS resolves as TERMINATED_WITH_ERRORS) are indistinguishable — both never converge.
Fix: the EMR cluster lifecycle/bootstrap execution in the emr service's RunJobFlow handler and cluster state machine — a cluster with BootstrapActions must enter BOOTSTRAPPING, fetch each `ScriptBootstrapAction.Path` from S3 and execute it (with its Args) so its side effects land before the cluster reports RUNNING/WAITING, and a non-zero script exit must drive the cluster to a terminal TERMINATED_WITH_ERRORS with a StateChangeReason.
Related open issue that does NOT carry this run's replay key for this check (so it does not cover it): 20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md describes step execution never reaching a terminal state; this finding is the bootstrap-action phase of RunJobFlow, which the replay report marked neither ALREADY FILED nor OWNED. 20261029T175551Z-emr-listbootstrapactions-stub.md covers only the unimplemented ListBootstrapActions read, not the execution of the action.
Traces: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive.

## Evidence
Check: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive [mismatch]
  Expected (AWS): RunJobFlow with BootstrapActions=[{Name, ScriptBootstrapAction:{Path: s3://<bucket>/<script>}}] produces a cluster that reaches BOOTSTRAPPING and then WAITING/RUNNING, and by that time the script has run: the S3 object the script writes exists and holds exactly the bytes the script wrote (the check asserts the marker body equals `bootstrap-executed`). Bucket/key/cluster-id values are per-run; the requirement is that the script executes and its S3 write is visible.
  Actual (emulator): `Error: Check timed out after 60.0s` — the script is never fetched or executed, the marker object is never created, and the poll for it (`_poll_for_object`) never converges; no terminal cluster state or error attributes the omission.

## Steps to Reproduce
1. CreateBucket; PutObject a shell script at s3://<bucket>/bootstrap.sh that writes a known body to s3://<bucket>/bootstrap-marker.txt.
2. Create an EMR service role and an EC2 instance profile (EMR default managed policies).
3. RunJobFlow with ReleaseLabel emr-6.15.0, one MASTER m5.xlarge instance group, KeepJobFlowAliveWhenNoSteps=true, and BootstrapActions=[{Name: 'fetch-and-run-from-s3', ScriptBootstrapAction: {Path: 's3://<bucket>/bootstrap.sh'}}].
4. Poll DescribeCluster until Status.State is WAITING/RUNNING (or terminal).
5. Poll HeadObject/GetObject for s3://<bucket>/bootstrap-marker.txt.
Against AWS the cluster becomes WAITING and the marker object exists with the script's body. Against the emulator the marker object never appears and the caller exhausts its budget. Re-replay emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive to confirm a fix.

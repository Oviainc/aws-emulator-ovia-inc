Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative
Issue-Key: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative|run#17|AssertionError|assert allowed_found, "positive control cluster (unrestricted instance role) never got its logs to S3"
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# emr__glue__s3: a terminated cluster with an unrestricted instance role uploads no logs to its LogUri, so the S3 log-permission boundary cannot be observed at all (1 cause)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
One net-new root cause from this run's failures under the `emr__glue__s3` service heading.

--- Cause 1 of 1: EMR never writes cluster logs to the LogUri prefix, so even a cluster whose instance role is allowed s3:PutObject has no log objects after it terminates ---
AWS: recorded behavior only (Smithy: RunJobFlow.LogUri is 'The location in Amazon S3 to write the log files of the job flow'; the model carries no delivery semantics). A cluster launched with LogUri='s3://<bucket>/<prefix>/' and an instance profile that is allowed s3:PutObject on that prefix uploads its logs there: after the cluster reaches a terminal state (TERMINATED / TERMINATED_WITH_ERRORS), ListObjectsV2(Bucket=<bucket>, Prefix='<prefix>/<cluster-id>/') returns at least one object within the scenario's wait. A sibling cluster whose instance role is explicitly DENIED s3:PutObject on its own prefix gets none, which is what makes the denial observable.
Emulator: the clusters do reach a terminal state, but NOTHING is ever written under the LogUri prefix — the positive-control cluster (unrestricted instance profile, same wiring, same run) has no object under '<prefix>/<cluster-id>/' within the 600 s wait, so the check cannot even establish its baseline and the denied leg is meaningless (an absence that would hold whatever the policy says).
Why: a client that reads cluster logs from LogUri — the standard way to diagnose a cluster or a step after the fact — finds an empty prefix for every cluster, and any IAM policy that governs log delivery is unverifiable because log delivery never happens on the allowed path either. A permission boundary that is never exercised silently 'passes'.
Fix: the EMR crate's cluster-lifecycle log delivery — upload the cluster's log artifacts to the configured LogUri (under '<prefix>/<cluster-id>/') through the S3 service state as the cluster runs and terminates, and make that write subject to the instance profile's IAM evaluation so a Deny on s3:PutObject really suppresses it.
Related open issue, which does NOT carry this check's replay key: 20261108T104909Z-emr-glue-s3-2-net-new-causes-cluster.md (its cause 'cluster logs are never uploaded'), which owns the positive-side check emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive; the permission-boundary check below is not in it and must be re-replayed to confirm the fix.
Traces: emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (service heading `emr__glue__s3`: 7/47 passing).

emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative [mismatch]:
  setup: one bucket; an EMR service role; two instance profiles — one with an explicit Deny on s3:PutObject for 'arn:aws:s3:::<bucket>/logs-denied/*', one unrestricted. Two clusters are run concurrently with the same trivial command-runner step, LogUri='s3://<bucket>/logs-denied/' and 's3://<bucket>/logs-allowed/' respectively, KeepJobFlowAliveWhenNoSteps=false. Both clusters reach a terminal state.
  expected: ListObjectsV2(Prefix='logs-allowed/<cluster-id>/') eventually returns at least one object (positive control), and 'logs-denied/<cluster-id>/' stays empty for at least 3x the positive control's observed latency.
  actual:   AssertionError: positive control cluster (unrestricted instance role) never got its logs to S3 — no object appeared under the allowed prefix within the 600 s wait.

## Steps to Reproduce
Boot the emulator and replay emr__glue__s3.emr-cluster-missing-s3-logs-permission-negative from suite simparity-behavioral-aws-v1:
1. CreateBucket; create an EMR service role and two EC2 instance profiles — profile A unrestricted, profile B carrying an explicit Deny of s3:PutObject on 'arn:aws:s3:::<bucket>/logs-denied/*'.
2. RunJobFlow twice (one trivial command-runner step each, KeepJobFlowAliveWhenNoSteps=false): cluster A with JobFlowInstancesConfig using profile A and LogUri='s3://<bucket>/logs-allowed/', cluster B using profile B and LogUri='s3://<bucket>/logs-denied/'.
3. Poll DescribeCluster until both reach TERMINATED/TERMINATED_WITH_ERRORS.
4. Poll ListObjectsV2(Bucket=<bucket>, Prefix='logs-allowed/<cluster-A-id>/'): expect at least one log object; observe none within 600 s.
5. Then poll 'logs-denied/<cluster-B-id>/' for 3x the latency measured in step 4 and expect it to stay empty.

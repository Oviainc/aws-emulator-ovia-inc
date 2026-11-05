Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-permission-missing-negative emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive emr__glue__s3.emr-step-missing-s3-input-read-permission-negative emr__glue__s3.eventbridge-emr-cluster-state-change-rule-positive
Issue-Key: emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative|_wait_for_step#12|TimeoutError|raise TimeoutError(f"step {step_id} reaching a terminal state: the run budget ended first; not graded")
Issue-Key: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive|_wait_for_step#16|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-permission-missing-negative|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.emr-cluster-bootstrap-action-script-fetched-from-s3-positive|_poll_for_object#13|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive|_wait_for_step#10|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative|_wait_for_step#12|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.emr-cluster-instance-role-trust-mismatch-negative|_poll_cluster_state#19|Timeout|return cluster
Issue-Key: emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive|_poll_step_state#15|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive|_poll_step_state#13|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive|_wait_for_step#12|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive|_wait_for_step#12|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.emr-step-missing-s3-input-read-permission-negative|_poll_step_state#20|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.eventbridge-emr-cluster-state-change-rule-positive|_poll_until#29|Timeout|return last
Issue-Commit: 6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003

# emr__glue__s3: EMR cluster steps never reach a terminal state, so all 13 remaining cross-service EMR checks are cut off at the harness budget

> _Note: this issue was identified against the code base at commit `6cc7c0f4d58adfa1ddf47e8354a4acf1282f7003`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
Cause 1 of 1: a step submitted to a running EMR cluster never reaches a terminal StepState, so every emr__glue__s3 check that waits on step (or on the cluster/step state change that follows it) exhausts its budget and is never graded.
AWS: Smithy `StepState` enum = PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED | INTERRUPTED, and `StepStatus` carries `State`, `StateChangeReason`, `FailureDetails` and `Timeline`. A step submitted with AddJobFlowSteps on a running cluster advances PENDING -> RUNNING -> one of COMPLETED / FAILED / CANCELLED / INTERRUPTED, and DescribeStep/ListSteps report that terminal state together with its Timeline; the step's side effects (the object a Hadoop/Hive/Spark step writes under the S3 output location, and the step/bootstrap logs under LogUri) land with it. A permission-denied or configuration-invalid step reaches FAILED with FailureDetails rather than staying non-terminal.
Emulator: the step is accepted and then never leaves its non-terminal state. One check attributes it exactly — `TimeoutError: step s-0000000000001 reaching a terminal state: the run budget ended first; not graded` (emr-cluster-glue-catalog-disabled-configuration-negative); the other twelve checks in this group poll the same step/cluster convergence and report `Error: Check timed out after 60.0s` with no attribution. The step id in that message is a per-run value; the requirement is that SOME terminal state is reached.
Why: nothing downstream of a step is observable — a client cannot see a Hive/Spark step complete, cannot read the output it should have written to S3, cannot distinguish a step that FAILED for a missing s3:GetObject grant or a missing hive-site Glue classification from one that is merely slow, and cannot drive the EMR cluster/step state-change events that EventBridge rules match on. Positive and negative EMR paths become indistinguishable: both hang.
Fix: the EMR step execution/lifecycle in the emr service state — AddJobFlowSteps must enqueue a step that the cluster advances to RUNNING and then to a terminal StepState with StateChangeReason/FailureDetails/Timeline populated, and the step's S3 output and log writes must happen on that transition.
Related open issues that do NOT carry these checks' replay keys (so they do not cover them): 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md and 20261104T055008Z-emr-glue-s3-2-net-new-root-causes.md describe the same symptom for other checks; this issue is grounded in the 13 checks listed in trace_ids, which the replay report marked neither ALREADY FILED nor OWNED.
Traces: the 13 check ids in trace_ids.

## Evidence
Attributed instance — emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative
  Expected: the shell/Hive step reaches a terminal StepState (COMPLETED on the happy path, FAILED with FailureDetails on the denied/misconfigured path) and DescribeStep reports it with a Timeline.
  Actual:   TimeoutError: step s-0000000000001 reaching a terminal state: the run budget ended first; not graded  (raised from _wait_for_step; the step id is a per-run value)

Unattributed instances — the other 12 checks listed in trace_ids
  Expected: each polled EMR convergence (step terminal state, cluster self-termination, step output object under the S3 output location, step/bootstrap logs under LogUri, cluster/step state-change event delivery) is reached.
  Actual:   Error: Check timed out after 60.0s — the poll never converged and no step attribution was produced.

## Steps to Reproduce
RunJobFlow a cluster (service role + EC2 instance profile, LogUri on an S3 bucket); wait for it to be usable; AddJobFlowSteps with a shell/Hive/Spark step; poll DescribeStep(ClusterId, StepId) for Status.State. Expect it to advance to RUNNING and then to a terminal state (COMPLETED, or FAILED with FailureDetails when the instance role lacks the needed S3/Glue grant or the hive-site Glue classification is absent). Against the emulator the step never becomes terminal and the caller exhausts its budget — replay the 13 checks in trace_ids to confirm.

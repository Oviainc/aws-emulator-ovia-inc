Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive emr__glue__s3.emr-step-missing-s3-input-read-permission-negative
Issue-Key: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive|run#17|AssertionError|assert state == "COMPLETED", f"hive SET query step did not complete: {state}"
Issue-Key: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive|run#12|AssertionError|assert steps[0]["Status"]["State"] == "COMPLETED", steps[0]["Status"]
Issue-Key: emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive|run#11|AssertionError|assert state == "COMPLETED", f"hive SET query step did not complete: {state}"
Issue-Key: emr__glue__s3|GetObject|NoSuchKey
Issue-Key: emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive|_poll_for_log_objects#13|Timeout|time.sleep(interval_s)
Issue-Key: emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive|run#16|AssertionError|assert state == "COMPLETED", f"expected read step to COMPLETE, got {state}"
Issue-Key: emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive|run#14|AssertionError|assert state == "COMPLETED", f"expected write step to COMPLETE, got {state}"
Issue-Key: emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive|run#13|AssertionError|assert state == "COMPLETED", f"hive query against Glue-catalog-backed table failed: {state}"
Issue-Key: emr__glue__s3.emr-step-missing-s3-input-read-permission-negative|run#23|AssertionError|assert allowed_state == "COMPLETED", f"expected allowed-path (positive control) step to COMPLETE, got {allowed_state}"
Issue-Commit: b173a8e674d7ba975def7d68c06d3de96ae43616

# emr__glue__s3: 2 net-new causes — cluster steps always end FAILED because no Hive/Spark/Hadoop application is run, and cluster logs are never uploaded under LogUri

> _Note: this issue was identified against the code base at commit `b173a8e674d7ba975def7d68c06d3de96ae43616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
Two net-new root causes in the EMR job-flow data plane, both observed on checks the replay report did not mark as owned or already filed. Related open issues exist for neighbouring symptoms (20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md, 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md, 20261108T093128Z-ec2-emr-emr-job-flow-steps-always-end-failed-a.md) — none of them carries the replay keys below, so this issue is filed for them and names those issues for context.

Cause 1 of 2: a submitted step is driven through the step lifecycle but no application is ever executed, so every step ends FAILED.
AWS: RunJobFlow/AddJobFlowSteps submit a step that the cluster executes; DescribeStep/ListSteps report Step.Status.State progressing PENDING -> RUNNING -> COMPLETED for a step whose application succeeds, and the step's own side effects (objects written by a hadoop-jar/spark-submit/hive -e step, rows read back through a Glue-catalog-backed table) are real. FAILED is reserved for a step whose application actually failed, and then Status.FailureDetails/StateChangeReason describe that application failure. Smithy (com-amazonaws-elasticmapreduce): StepState enum includes PENDING/RUNNING/COMPLETED/CANCELLED/FAILED/INTERRUPTED; StepStatus carries State, StateChangeReason and FailureDetails. Beyond the state machine the model says nothing about execution — recorded behavior only for the application's side effects.
Emulator: every step reaches State=FAILED with an emulator-authored explanation. Recorded verbatim from DescribeStep: StateChangeReason.Message = "This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application." and FailureDetails.Message = "No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen." (the step id, cluster id and the Timeline timestamps in the recording are per-run values, not part of the requirement). Because the application never runs, the follow-on reads fail too: the S3 object a step was supposed to write is absent, so GetObject answers 404 NoSuchKey "The specified key does not exist.", and a Hive SET / Hive query step against a Glue Data Catalog metastore never reaches COMPLETED.
Why: a client that submits work to EMR and waits on DescribeStep can never observe success, and nothing downstream of a step exists — no output object, no Glue-catalog-backed query result, no positive control for a negative (permission-denied) test. Every EMR-driven pipeline is unusable and negative tests cannot be distinguished from the blanket failure.
Fix: the EMR step executor behind AddJobFlowSteps/DescribeStep in the elasticmapreduce service crate (step state machine plus the missing step-application execution), and the S3 writes a step performs.
Traces: emr__glue__s3.cfn-deploy-emr-cluster-glue-catalog-config-positive ("hive SET query step did not complete: FAILED"), emr__glue__s3.emr-cluster-configured-with-glue-catalog-metastore-positive (same), emr__glue__s3.emr-hive-query-reads-glue-catalog-table-positive ("hive query against Glue-catalog-backed table failed: FAILED"), emr__glue__s3.emr-cluster-step-reads-input-from-s3-positive ("expected read step to COMPLETE, got FAILED"), emr__glue__s3.emr-cluster-step-writes-output-to-s3-positive ("expected write step to COMPLETE, got FAILED"), emr__glue__s3.emr-step-missing-s3-input-read-permission-negative ("expected allowed-path (positive control) step to COMPLETE, got FAILED"), emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive (DescribeStep State FAILED with the two messages above), emr__glue__s3.emr-cluster-glue-catalog-disabled-configuration-negative, emr__glue__s3.emr-cluster-glue-catalog-missing-permission-negative and emr__glue__s3.emr-spark-job-writes-result-via-glue-catalog-table-location-s3-positive (all three: GetObject of the step's declared output answers NoSuchKey "The specified key does not exist.").

Cause 2 of 2: a cluster configured with a LogUri uploads no log artifacts to that S3 prefix.
AWS: when RunJobFlow is given LogUri, EMR uploads cluster and step logs under that prefix (per-cluster/per-step key hierarchy) and they survive cluster termination, so a ListObjectsV2 of the LogUri prefix eventually returns objects. Recorded behavior only (the Smithy model declares LogUri on RunJobFlowInput/Cluster but says nothing about delivery).
Emulator: no object is ever written under the LogUri prefix; the check polling that prefix never sees a key and the check is killed at its 180 s budget ("Check timed out after 180.0s").
Why: an operator who sizes or debugs a cluster from its archived logs gets an empty prefix, and any workflow that waits for log delivery hangs instead of failing.
Fix: the EMR cluster/step log-delivery path in the elasticmapreduce service crate (the LogUri writer invoked from the cluster and step lifecycle).
Traces: emr__glue__s3.emr-cluster-logs-uploaded-to-s3-log-uri-positive.

## Evidence
Cause 1 — expected (AWS): DescribeStep reports Step.Status.State = COMPLETED for a successful hadoop-jar/spark-submit/hive step, and the step's declared S3 output object exists so GetObject answers 200. Actual (emulator): Status.State = FAILED with StateChangeReason.Message = "This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application." and FailureDetails.Message = "No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen."; the output object is absent, so GetObject answers 404 NoSuchKey with message "The specified key does not exist." Fixed text of the refusal: "The specified key does not exist."; no variable parts. The step id, cluster id, bucket name and Timeline timestamps quoted from the recording are per-run values.

Cause 2 — expected (AWS): objects appear under the cluster's LogUri prefix and remain after termination. Actual (emulator): the prefix stays empty; the polling check is terminated by its own 180 s budget ("Check timed out after 180.0s").

## Steps to Reproduce
1. RunJobFlow a cluster with LogUri set to an s3:// prefix and an instance profile that can read/write the data bucket.
2. AddJobFlowSteps one step per form: a hadoop-jar step that writes an object to S3, a `hive -e 'SET ...'` step, and a Hive query over a table whose metastore is the Glue Data Catalog.
3. Poll DescribeStep until a terminal state. Observed: State=FAILED with the emulator-authored StateChangeReason/FailureDetails above; expected COMPLETED.
4. GetObject the key the write step declared. Observed: 404 NoSuchKey "The specified key does not exist."; expected 200 with the step's bytes.
5. ListObjectsV2 the cluster's LogUri prefix (before and after TerminateJobFlows). Observed: no keys ever appear; expected EMR-written cluster and step logs.

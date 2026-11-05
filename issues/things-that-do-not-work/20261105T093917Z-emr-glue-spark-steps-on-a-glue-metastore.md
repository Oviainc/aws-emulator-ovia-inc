Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails
Issue-Key: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails|_poll#29|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# emr__glue: Spark steps on a Glue-metastore cluster never reach a terminal state, so the unregistered-table failure can never be observed

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStep

## Description
Cause 1 of 1: a step submitted to a live cluster configured with the Glue Data Catalog as the Hive metastore stays PENDING forever, so neither the positive (registered table) nor the negative (unregistered table) step ever reports an outcome.
  AWS: AddJobFlowSteps returns StepIds and each step advances PENDING -> RUNNING -> COMPLETED/FAILED; a Spark step reading a table that is NOT registered in the Glue catalog reaches FAILED (the driver raises a table-not-found), while the sibling step on a registered table reaches COMPLETED and writes its output. Smithy: AddJobFlowSteps -> StepIds (the operation models only InternalServerError); DescribeStep/ListSteps report Status.State and Status.Timeline. Recorded behavior: both steps reach terminal states in the recording.
  Emulator: both Glue-metastore steps remain in a non-terminal state for the whole budget — the check's poll for 'both Glue-metastore steps on j-… reaching terminal states' exhausts its budget with no transition.
  Why: a client that routes EMR Spark work through the Glue catalog can never tell a successful query from a missing-table failure: no step ever finishes, so the catalog-backed failure mode is invisible and any wait hangs.
  Fix: the EMR step state machine behind AddJobFlowSteps/DescribeStep (the cluster's step runner and the step state it records), which must advance steps on a live cluster and settle them in a terminal state.
  Note: open issue 20261104T082656Z-emr-glue-ec2-createvpcendpoint-refuses-any-request-carrying.md covers a CreateVpcEndpoint refusal in this same service pairing and does not carry this check id; 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md is the closest emr-side item and owns other checks' replay keys.
  Traces: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails.

## Evidence
Replay of simparity-behavioral-aws-v1, '### emr__glue' (0/2 passed):

emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails — error: File "check_emr__glue_emr_spark_step_on_an_unregistered_glue_table_fails.py", line 498, in run / finals = _poll(…) -> TimeoutError: both Glue-metastore steps on j-0000000000001 reaching terminal states: the run budget ended first. The steps were accepted (StepIds returned) but DescribeStep never reported a terminal State.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator.
2. Create a Glue database and one registered table over an S3 location.
3. RunJobFlow a cluster whose Configurations set hive.metastore.client.factory.class to the Glue catalog factory, and wait for it to be live.
4. AddJobFlowSteps two command-runner Spark steps: one reading the registered table, one reading a table name that is not registered in the catalog.
5. Poll DescribeStep for both — expect COMPLETED and FAILED respectively; observe neither leaves its non-terminal state.
6. Replay emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails to confirm.

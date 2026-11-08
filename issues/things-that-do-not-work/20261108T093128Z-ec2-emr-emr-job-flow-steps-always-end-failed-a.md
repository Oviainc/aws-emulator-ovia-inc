Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode
Issue-Key: ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode|run#16|AssertionError|assert got == want, f"terminal step states should be {want!r}, got {got!r}"
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# EMR job-flow steps always end FAILED: a right-sized and a client-mode spark-submit step never COMPLETE

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
One net-new root cause.

Cause 1 of 1: every step added to a WAITING cluster terminates FAILED regardless of what it submits.
AWS: recorded behavior only — emr's Smithy model describes AddJobFlowSteps/ListSteps and the StepState enum but says nothing about execution. In the recording, three command-runner.jar steps added to the same WAITING cluster reach different terminal states that depend on what each one runs: a cluster-mode spark-submit whose driver does not fit the YARN container ends FAILED, while the right-sized cluster-mode submit and the same oversized job in client mode both end COMPLETED. The distinguishing input is the step's HadoopJarStep.Args (--deploy-mode cluster|client, --driver-memory/--executor-memory), never the step name.
Emulator: all three steps reach FAILED, so step outcome is independent of the submitted command — the two steps AWS completes are failed too. ListSteps reports {'oversized-client-mode': 'FAILED', 'right-sized-cluster-mode': 'FAILED', 'oversized-cluster-mode': 'FAILED'}. The cluster itself reaches WAITING and both instance fleets provision their one on-demand unit, so the divergence is in step execution, not cluster provisioning.
Why: a client cannot tell a genuinely broken submit from a working one — every job looks broken, so retry/alerting logic fires on healthy jobs, and no EMR scenario that depends on a successful step (or on comparing a failing submit against a working control) can be emulated.
Fix: the emr step execution/state machine behind AddJobFlowSteps and ListSteps (deriving each step's terminal StepState from its HadoopJarStep command and the cluster's resources) rather than a blanket failure, in the emr handlers.
Traces: ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode.

## Evidence
From the behavioral replay, ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode [mismatch]:

File "check_ec2__emr_oversized_driver_spark_step_fails_only_in_cluster_deploy_mode.py", line 57, in run
assert got == want, f"terminal step states should be {want!r}, got {got!r}"
AssertionError: terminal step states should be {'oversized-cluster-mode': 'FAILED', 'right-sized-cluster-mode': 'COMPLETED', 'oversized-client-mode': 'COMPLETED'}, got {'oversized-client-mode': 'FAILED', 'right-sized-cluster-mode': 'FAILED', 'oversized-cluster-mode': 'FAILED'}

The three steps are added in one AddJobFlowSteps call with ActionOnFailure=CONTINUE and Jar=command-runner.jar:
- oversized-cluster-mode: spark-submit --deploy-mode cluster --class org.apache.spark.examples.SparkPi <examples jar> 10
- right-sized-cluster-mode: the same plus --driver-memory 2G --executor-memory 4G --conf spark.yarn.executor.memoryOverhead=1G --num-executors 1
- oversized-client-mode: spark-submit --deploy-mode client <same job>
The preceding assertions pass: the cluster reaches WAITING and ListInstanceFleets reports [('CORE', 1), ('MASTER', 1)] provisioned on-demand capacity. Only the terminal step states diverge.

## Steps to Reproduce
1. Boot the emulator. RunJobFlow an EMR cluster with instance fleets (one MASTER, one CORE on-demand unit) and KeepJobFlowAliveWhenNoSteps=true; wait for WAITING.
2. AddJobFlowSteps with three command-runner.jar steps, ActionOnFailure=CONTINUE: (a) spark-submit --deploy-mode cluster --class org.apache.spark.examples.SparkPi <spark-examples.jar> 10, (b) the same with --driver-memory 2G --executor-memory 4G --conf spark.yarn.executor.memoryOverhead=1G --num-executors 1, (c) the same job with --deploy-mode client.
3. Poll ListSteps(ClusterId, StepIds) until all three are terminal.
4. Observe all three read FAILED; AWS completes (b) and (c) and fails only (a).
5. Re-replay ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode to confirm the fix.

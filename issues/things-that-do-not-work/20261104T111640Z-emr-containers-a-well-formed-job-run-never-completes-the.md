Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.start-job-run-spark-pi-native-round-trip
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.start-job-run-spark-pi-native-round-trip|run#10|AssertionError|assert final["state"] == "COMPLETED", (
Issue-Commit: 6e4639cd3b0eb321e74af4094a3461ae2de27150

# A well-formed job run never COMPLETES: the emulator drives every StartJobRun to FAILED / CLUSTER_UNAVAILABLE instead of running it to COMPLETED with finishedAt

> _Note: this issue was identified against the code base at commit `6e4639cd3b0eb321e74af4094a3461ae2de27150`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One root cause: a job run accepted by StartJobRun against a RUNNING virtual cluster always ends in the terminal state FAILED with failureReason CLUSTER_UNAVAILABLE, so the success half of the job-run lifecycle is unreachable and no client can ever observe a COMPLETED job run.

AWS: Smithy: StartJobRun (POST /virtualclusters/{virtualClusterId}/jobruns) and DescribeJobRun (GET /virtualclusters/{virtualClusterId}/jobruns/{id}, output JobRun); JobRunState enum = PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED, and FailureReason enum = INTERNAL_ERROR, USER_ERROR, VALIDATION_ERROR, CLUSTER_UNAVAILABLE. Recorded behavior: a job run submitted with a valid executionRoleArn, releaseLabel and a sparkSubmitJobDriver (the bundled SparkPi example) onto a virtual cluster registered against a healthy EKS cluster advances through the non-terminal states and settles in state COMPLETED, with finishedAt strictly greater than createdAt and no failureReason. CLUSTER_UNAVAILABLE is the reason AWS uses when the underlying compute really is unavailable, not the outcome of a healthy submission.
Emulator: DescribeJobRun settles the job run in state FAILED with failureReason CLUSTER_UNAVAILABLE and stateDetails 'The job driver was not executed: this emulator records job runs and runs no Spark workload, so the job run ended without running.' — i.e. the lifecycle records the submission and then fails it unconditionally.
Why: every client that submits work and waits for it gets a hard failure for a submission AWS completes: wait-for-completion loops terminate on the failure branch, retry/alerting paths fire for a job that was never run, and a caller that reads failureReason concludes its EKS compute is unavailable. The success path of the data plane — COMPLETED plus a finishedAt after createdAt — cannot be exercised at all, so neither can anything downstream of it.
Fix: the job-run lifecycle/state machine of the emr-containers service crate (the state transition that advances a submitted run), not the StartJobRun accept path: a job run whose driver is well-formed and whose virtual cluster is RUNNING must advance to COMPLETED and carry finishedAt strictly after createdAt, with no failureReason; reserve FAILED/CLUSTER_UNAVAILABLE for the conditions AWS uses it for, and leave the CANCELLED path (which replays clean today) alone. Related but not covering: resolved issue 20261024T011835Z-emr-containers-describejobrun-wrong-behavior.md gave job runs a terminal state and finishedAt at all — the run now terminates, but always on the failure branch, which that issue does not address.
Traces: check emr-containers.start-job-run-spark-pi-native-round-trip.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: emr-containers 14/20 passed, 6 mismatched, 0 errored.

[mismatch] emr-containers.start-job-run-spark-pi-native-round-trip:
  request: StartJobRun(virtualClusterId=<vc on a healthy EKS cluster>, clientToken=<tok>, executionRoleArn=<job execution role>, releaseLabel='emr-6.15.0-latest', jobDriver={sparkSubmitJobDriver:{entryPoint:'local:///usr/lib/spark/examples/jars/spark-examples.jar', sparkSubmitParameters:'--class org.apache.spark.examples.SparkPi --conf spark.executor.instances=1 ...'}})
  the check then polls DescribeJobRun until the state is terminal (COMPLETED|FAILED|CANCELLED) and asserts COMPLETED and finishedAt > createdAt.
  expected: state COMPLETED, finishedAt > createdAt, no failureReason
  actual:   AssertionError: SparkPi job run did not COMPLETE: state=FAILED details=The job driver was not executed: this emulator records job runs and runs no Spark workload, so the job run ended without running. failureReason=CLUSTER_UNAVAILABLE
  (StartJobRun itself answered 200 and the job run was created and polled successfully — the divergence is the terminal state the lifecycle picks.)

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query:
  DescribeJobRun: GET /virtualclusters/{virtualClusterId}/jobruns/{id} code 200, output jobRun -> JobRun; errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  JobRunState enum = PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED.
  FailureReason enum = INTERNAL_ERROR, USER_ERROR, VALIDATION_ERROR, CLUSTER_UNAVAILABLE.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1, check emr-containers.start-job-run-spark-pi-native-round-trip.
3. Register a virtual cluster against a RUNNING EKS cluster, then call StartJobRun with a valid executionRoleArn, releaseLabel 'emr-6.15.0-latest' and a sparkSubmitJobDriver naming the bundled SparkPi example.
4. Poll DescribeJobRun until the state is terminal. Expect state COMPLETED with finishedAt strictly after createdAt and no failureReason; observed state FAILED, failureReason CLUSTER_UNAVAILABLE and a stateDetails sentence stating no workload was run.
5. Confirm the cancellation path is unaffected: StartJobRun -> CancelJobRun must still settle the run in CANCELLED (checks emr-containers.cancel-job-run-terminal-state-rejected and emr-containers.delete-virtual-cluster-with-active-job-run-rejected depend on it).
6. Re-replay emr-containers.start-job-run-spark-pi-native-round-trip to confirm the fix.

Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.get-session-endpoint-certificate-blob-preserved emr.get-session-endpoint-for-terminated-cluster-rejected emr.get-session-endpoint-returns-endpoint-for-started-session emr.get-session-endpoint-returns-reachable-endpoint emr.list-sessions-newest-first emr.list-sessions-ordering-newest-first emr.list-sessions-orders-by-creation-time emr.session-endpoint-becomes-ready-within-ttl emr.session-lifecycle-start-active-terminate emr.session-lifecycle-start-get-terminate emr.session-tags-empty-vs-absent-preserved emr.session-terminate-then-get-reflects-terminated-state emr.start-session-non-studio-cluster-rejected emr.start-session-on-non-running-cluster-rejected emr.start-session-transitions-active-then-terminate-then-get-fails emr.terminate-session-already-terminated-rejected emr.terminate-session-twice-rejected emr.action-on-failure-continue-then-terminate-cluster emr.deploy-cfn-cluster-executes-step emr.deploy-emr-cluster-cfn-then-step-executes emr.deploy-emr-cluster-via-cloudformation-executes-step emr.e2e-cluster-create-step-execute-terminate-flow emr.emr-cancel-and-wait-on-a-failed-step-cancels-the-following-step emr.emr-cancel-steps-on-a-running-step-prevents-its-output emr.emr-step-args-are-passed-verbatim-to-the-application emr.emr-step-concurrency-level-two-runs-two-steps-with-overlapping-intervals emr.emrfs-write-through-hadoop-fs-is-read-by-the-next-step emr.hdfs-write-survives-the-step-boundary emr.installed-application-versions-agree-between-describe-cluster-and-the-node emr.parquet-written-by-one-spark-step-is-read-back-by-the-next emr.pending-spark-step-listed-then-cancelled-on-terminate emr.persistent-app-ui-creation-then-describe-eventually-ready emr.persistent-app-ui-describe-survives-cluster-termination-window emr.persistent-app-ui-presigned-url-expires-after-ttl emr.persistent-app-ui-presigned-url-flow emr.s3-dist-cp-group-by-aggregates-through-a-real-mapreduce-job emr.scaling-emrfs-read-of-a-deleted-object-is-rejected-and-writes-no-output emr.security-configuration-sse-kms-applies-to-emrfs-writes emr.spark-defaults-classification-governs-the-shuffle-partition-count emr.spark-partitioned-input-pruned-then-written-back-partitioned emr.spark-terminate-cluster-action-rejected-at-step-concurrency-two emr.e2e-notebook-execution-persistent-app-ui-flow emr.e2e-notebook-execution-run-to-completion emr.notebook-execution-lifecycle-transitions emr.notebook-execution-state-transitions-starting-to-finished emr.list-notebook-executions-ordering-and-status-filter emr.step-added-while-cluster-starting-queues-then-runs emr.persistent-app-ui-presigned-url-token-preserved emr.multi-master-forces-termination-protection-on emr.terminate-protected-multi-master-cluster-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|CreateVpcEndpoint|InvalidServiceName
Issue-Key: emr.action-on-failure-continue-then-terminate-cluster|_poll#13|Timeout|return last
Issue-Key: emr.deploy-cfn-cluster-executes-step|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: emr.deploy-emr-cluster-cfn-then-step-executes|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: emr.deploy-emr-cluster-via-cloudformation-executes-step|_wait_step_state#9|Timeout|time.sleep(interval_s)
Issue-Key: emr.e2e-cluster-create-step-execute-terminate-flow|_wait_step_state#8|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-cancel-and-wait-on-a-failed-step-cancels-the-following-step|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-cancel-steps-on-a-running-step-prevents-its-output|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-step-args-are-passed-verbatim-to-the-application|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-step-concurrency-level-two-runs-two-steps-with-overlapping-intervals|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: emr.emrfs-write-through-hadoop-fs-is-read-by-the-next-step|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: emr.hdfs-write-survives-the-step-boundary|_poll#21|Timeout|time.sleep(interval_s)
Issue-Key: emr.installed-application-versions-agree-between-describe-cluster-and-the-node|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: emr.parquet-written-by-one-spark-step-is-read-back-by-the-next|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: emr.pending-spark-step-listed-then-cancelled-on-terminate|_poll#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: emr.persistent-app-ui-creation-then-describe-eventually-ready|_poll_until#4|Timeout|time.sleep(interval_s)
Issue-Key: emr.persistent-app-ui-describe-survives-cluster-termination-window|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: emr.persistent-app-ui-presigned-url-expires-after-ttl|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: emr.persistent-app-ui-presigned-url-flow|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: emr.s3-dist-cp-group-by-aggregates-through-a-real-mapreduce-job|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: emr.scaling-emrfs-read-of-a-deleted-object-is-rejected-and-writes-no-output|_poll#21|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: emr.security-configuration-sse-kms-applies-to-emrfs-writes|_poll#24|Timeout|time.sleep(interval_s)
Issue-Key: emr.spark-defaults-classification-governs-the-shuffle-partition-count|_poll#23|TimeoutError|raise TimeoutError(
Issue-Key: emr.spark-partitioned-input-pruned-then-written-back-partitioned|_poll#19|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: emr.spark-terminate-cluster-action-rejected-at-step-concurrency-two|_poll#18|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: emr.e2e-notebook-execution-persistent-app-ui-flow|run#14|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: emr.e2e-notebook-execution-run-to-completion|run#19|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: emr.notebook-execution-lifecycle-transitions|run#16|AssertionError|assert progressed["Status"] not in {"FAILED", "FAILING"}, (
Issue-Key: emr.notebook-execution-state-transitions-starting-to-finished|run#11|AssertionError|assert last_status == "FINISHED", (
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: emr.step-added-while-cluster-starting-queues-then-runs|run#19|AssertionError|assert state in PRE_READY_STATES, (
Issue-Key: emr.persistent-app-ui-presigned-url-token-preserved|run#19|AssertionError|assert first_url != second_url, (
Issue-Key: emr.multi-master-forces-termination-protection-on|run#8|AssertionError|assert got is True, (
Issue-Key: emr.terminate-protected-multi-master-cluster-rejected|run#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects a protected cluster's terminate")
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# EMR: 6 net-new root causes — the EMR session VPC endpoint service is unknown to CreateVpcEndpoint, steps never leave PENDING, notebook executions never run, and three lifecycle/representation gaps

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
Six independent defects in the EMR control/data plane, each reached by its own behavioral checks. THIS ISSUE BUNDLES 6 CAUSES. All six must be closed.

Cause 1 of 6: EC2's VPC-endpoint service catalog does not offer the EMR session endpoint service, so every Spark-Connect session check is refused before it reaches EMR.
  AWS: CreateVpcEndpoint for an interface endpoint whose ServiceName is the regional EMR session endpoint service — 'aws.api.<region>.emr-service-cell01' — succeeds (200) and returns a VpcEndpoint; the service name exists in every region EMR offers sessions in. Recorded behavior only (EC2 uses the query protocol and models no per-case error for an endpoint service name), and the recordings show the call succeeding as a prerequisite of the session checks.
  Emulator: 400 'InvalidServiceName' with message "The Vpc Endpoint Service 'aws.api.us-west-2.emr-service-cell01' does not exist" — fixed text "The Vpc Endpoint Service '" + "' does not exist" with the requested ServiceName interpolated. The refusal happens on the prerequisite, so 17 EMR session/session-endpoint checks never exercise an EMR operation at all.
  Why: a client that stands up the private estate EMR sessions require cannot create the endpoint, so GetSessionEndpoint/StartSession/ListSessions/TerminateSession are unreachable even though EMR itself may implement them.
  Fix: EC2's CreateVpcEndpoint handler and the endpoint-service catalog it validates ServiceName against must recognise the EMR session endpoint service names ('aws.api.<region>.emr-service-cell*'), and reserve InvalidServiceName for names AWS does not offer.
  Note: open issue 20261104T102001Z-emr-8-net-new-root-causes-createvpcendpoint-refuses.md describes a DIFFERENT CreateVpcEndpoint refusal (a gateway endpoint naming RouteTableIds answered 501 NotImplemented) and does not cover this InvalidServiceName refusal of an interface endpoint.
  Traces: emr.get-session-endpoint-certificate-blob-preserved, emr.get-session-endpoint-for-terminated-cluster-rejected, emr.get-session-endpoint-returns-endpoint-for-started-session, emr.get-session-endpoint-returns-reachable-endpoint, emr.list-sessions-newest-first, emr.list-sessions-ordering-newest-first, emr.list-sessions-orders-by-creation-time, emr.session-endpoint-becomes-ready-within-ttl, emr.session-lifecycle-start-active-terminate, emr.session-lifecycle-start-get-terminate, emr.session-tags-empty-vs-absent-preserved, emr.session-terminate-then-get-reflects-terminated-state, emr.start-session-non-studio-cluster-rejected, emr.start-session-on-non-running-cluster-rejected, emr.start-session-transitions-active-then-terminate-then-get-fails, emr.terminate-session-already-terminated-rejected, emr.terminate-session-twice-rejected.

Cause 2 of 6: a submitted step is accepted and listed but never leaves PENDING, so no step ever reaches a terminal state.
  AWS: AddJobFlowSteps on a live cluster returns StepIds (Smithy: AddJobFlowSteps.StepIds; the operation models only InternalServerError), and the step then advances PENDING -> RUNNING -> COMPLETED (or FAILED/CANCELLED/INTERRUPTED), DescribeStep/ListSteps reporting Status.Timeline.StartDateTime and EndDateTime, with the step's side effects (S3/HDFS writes, step logs under LogUri) landing. Recorded behavior: every one of these checks polls DescribeStep for a terminal state and the recordings reach one.
  Emulator: DescribeStep keeps answering State 'PENDING' with only CreationDateTime in the Timeline, for the whole budget — e.g. last observed step {'Id': 's-…', 'Status': {'State': 'PENDING', 'Timeline': {'CreationDateTime': …}}}. Nothing runs, nothing is written, nothing self-terminates, so the checks are cut off at the harness budget.
  Why: every client that submits work to EMR and waits on it hangs: no step output, no step log, no cancellation semantics, no step-concurrency behavior and no bootstrap/step ordering can be observed.
  Fix: the step state machine behind AddJobFlowSteps/DescribeStep/ListSteps (the cluster's step runner and the step/cluster state in the emr service state), which must advance queued steps on a live cluster and record their timeline and effects.
  Note: open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md is the closest open item and owns other checks' replay keys; it does not carry the check ids listed here.
  Traces: emr.action-on-failure-continue-then-terminate-cluster, emr.deploy-cfn-cluster-executes-step, emr.deploy-emr-cluster-cfn-then-step-executes, emr.deploy-emr-cluster-via-cloudformation-executes-step, emr.e2e-cluster-create-step-execute-terminate-flow, emr.emr-cancel-and-wait-on-a-failed-step-cancels-the-following-step, emr.emr-cancel-steps-on-a-running-step-prevents-its-output, emr.emr-step-args-are-passed-verbatim-to-the-application, emr.emr-step-concurrency-level-two-runs-two-steps-with-overlapping-intervals, emr.emrfs-write-through-hadoop-fs-is-read-by-the-next-step, emr.hdfs-write-survives-the-step-boundary, emr.installed-application-versions-agree-between-describe-cluster-and-the-node, emr.parquet-written-by-one-spark-step-is-read-back-by-the-next, emr.pending-spark-step-listed-then-cancelled-on-terminate, emr.persistent-app-ui-creation-then-describe-eventually-ready, emr.persistent-app-ui-describe-survives-cluster-termination-window, emr.persistent-app-ui-presigned-url-expires-after-ttl, emr.persistent-app-ui-presigned-url-flow, emr.s3-dist-cp-group-by-aggregates-through-a-real-mapreduce-job, emr.scaling-emrfs-read-of-a-deleted-object-is-rejected-and-writes-no-output, emr.security-configuration-sse-kms-applies-to-emrfs-writes, emr.spark-defaults-classification-governs-the-shuffle-partition-count, emr.spark-partitioned-input-pruned-then-written-back-partitioned, emr.spark-terminate-cluster-action-rejected-at-step-concurrency-two.

Cause 3 of 6: a notebook execution is recorded and then driven straight to FAILED without running the notebook.
  AWS: StartNotebookExecution returns a NotebookExecutionId (Smithy: StartNotebookExecution -> NotebookExecutionId; errors InvalidRequestException 400, InternalServerException 500) and the execution advances START_PENDING -> STARTING -> RUNNING -> FINISHING -> FINISHED for a notebook whose cells succeed, writing the output notebook to OutputNotebookS3Location; a notebook that raises ends FAILED with the engine's reason. Recorded behavior: the checks poll DescribeNotebookExecution to FINISHED.
  Emulator: the execution reaches terminal 'FAILED' within ~0.5 s of StartTime with LastStateChangeReason "This emulator records notebook executions but runs no notebook, so the execution ended without running." — an emulator-authored sentence, and a state AWS only reaches for a failing notebook.
  Why: no notebook execution can ever succeed, so FINISHED/FAILED partitioning, status filters, ordering and output-notebook delivery are all unobservable, and a client sees a spurious failure for a healthy notebook.
  Fix: the notebook-execution lifecycle behind StartNotebookExecution/DescribeNotebookExecution (the execution state machine and its output-notebook write).
  Traces: emr.e2e-notebook-execution-persistent-app-ui-flow, emr.e2e-notebook-execution-run-to-completion, emr.notebook-execution-lifecycle-transitions, emr.notebook-execution-state-transitions-starting-to-finished, emr.list-notebook-executions-ordering-and-status-filter.

Cause 4 of 6: a freshly created cluster is already WAITING on the first read, so the STARTING/BOOTSTRAPPING window never exists.
  AWS: RunJobFlow's cluster passes through STARTING and BOOTSTRAPPING (provisioning real EC2 capacity takes minutes) before WAITING/RUNNING, and a step added during that window is listed PENDING until the cluster is live. Recorded behavior: the check reads the cluster three times and observes a non-WAITING state first.
  Emulator: DescribeCluster reports 'WAITING' on the first read after RunJobFlow (the check saw it leave ['BOOTSTRAPPING','STARTING'] after only 1 of 3 reads), so the queued-step window is never observable.
  Why: a client that waits for a cluster to become ready, or that asserts on the provisioning phases (bootstrap actions running before the first step, steps queueing while STARTING), sees a shape AWS never serves.
  Fix: the cluster state machine behind RunJobFlow/DescribeCluster — the cluster must occupy STARTING and BOOTSTRAPPING before it reports WAITING.
  Traces: emr.step-added-while-cluster-starting-queues-then-runs.

Cause 5 of 6: GetPersistentAppUIPresignedURL hands back a byte-identical URL on every call.
  AWS: each GetPersistentAppUIPresignedURL call mints a FRESH presigned URL — the signed payload carries a per-call session token (sessionId/authTokenId/iat/exp) — so two consecutive calls for the same PersistentAppUIId never return identical bytes. Smithy: GetPersistentAppUIPresignedURL -> {PresignedURLReady: Boolean, PresignedURL: XmlString}; errors InvalidRequestException 400, InternalServerError 500. The per-call freshness is recorded behavior only.
  Emulator: the second call returns the IDENTICAL PresignedURL string (a bare 'https://p-<id>.emrappui-prod.<region>.amazonaws.com/shs/' with no signed token at all), i.e. the credential is cached rather than minted.
  Why: a client that caches or shares the URL cannot observe expiry, and a URL carrying no session token is not usable the way AWS's is; TTL behavior is untestable.
  Fix: the GetPersistentAppUIPresignedURL handler's URL minting (it must sign a fresh per-call token into the URL rather than return a stored constant).
  Traces: emr.persistent-app-ui-presigned-url-token-preserved.

Cause 6 of 6: a multi-master cluster is not force-protected, and its bare terminate is accepted.
  AWS: RunJobFlow on a 3-primary cluster turns termination protection ON regardless of what Instances.TerminationProtected requested, so DescribeCluster reports TerminationProtected=true (Smithy: Cluster.TerminationProtected Boolean), and TerminateJobFlows on it is refused 400 ValidationException "Could not shut down one or more job flows since they are termination protected." — entirely fixed text — leaving the cluster running until SetTerminationProtection(false) clears it.
  Emulator: DescribeCluster reports TerminationProtected=false for a 3-master cluster requested with TerminationProtected=False, and TerminateJobFlows on a 3-master cluster returns 200 and shuts it down.
  Why: a client relying on EMR's multi-master safety net loses a highly available cluster to a bare terminate that AWS refuses.
  Fix: RunJobFlow's cluster-creation logic (force protection on when the primary count is >1) and the termination-protection gate in the TerminateJobFlows handler.
  Note: open issues 20261031T142501Z-emr-terminatejobflows-on-a-termination-protected-cluster-answers.md and 20261101T225156Z-emr-terminatejobflows-answers-200-for-a-termination-protected.md cover the refusal for an explicitly protected cluster and own other checks; neither carries these check ids nor the force-on rule.
  Traces: emr.multi-master-forces-termination-protection-on, emr.terminate-protected-multi-master-cluster-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emr: 75/216 passed).

Cause 1 — 17 checks error with: botocore.exceptions.ClientError: An error occurred (InvalidServiceName) when calling the CreateVpcEndpoint operation: The Vpc Endpoint Service 'aws.api.us-west-2.emr-service-cell01' does not exist. No EMR operation is reached.

Cause 2 — e.g. emr.spark-defaults-classification-governs-the-shuffle-partition-count: TimeoutError waiting for 'the shuffle-config step on j-… reaches a terminal state'; last observed Step = {'Id': 's-0000000000001', 'Config': {'Jar': 'command-runner.jar', 'Args': ['spark-submit', …]}, 'Status': {'State': 'PENDING', 'Timeline': {'CreationDateTime': …}}}. emr.pending-spark-step-listed-then-cancelled-on-terminate: TimeoutError 'the pending step leaves PENDING after termination'. emr.scaling-emrfs-read-…: TimeoutError 'the consumer step s-… on j-… reaches a terminal state'. The remaining checks in the cluster are cut off at the 60 s harness budget while polling DescribeStep.

Cause 3 — emr.e2e-notebook-execution-persistent-app-ui-flow: DescribeNotebookExecution = {'NotebookExecutionId': 'ex-…', 'Status': 'FAILED', 'StartTime': …944599, 'EndTime': …444599 (0.5 s later), 'LastStateChangeReason': 'This emulator records notebook executions but runs no notebook, so the execution ended without running.'}; expected FINISHED. emr.notebook-execution-state-transitions-starting-to-finished: 'never reached FINISHED after 3 attempt(s), last status=FAILED'.

Cause 4 — emr.step-added-while-cluster-starting-queues-then-runs: AssertionError: the cluster left ['BOOTSTRAPPING','STARTING'] (now 'WAITING') after only 1 of 3 reads; observations [('PENDING', ['CreationDateTime'])].

Cause 5 — emr.persistent-app-ui-presigned-url-token-preserved: AssertionError: GetPersistentAppUIPresignedURL returned the IDENTICAL PresignedURL twice … 'https://p-0000000000001.emrappui-prod.us-west-2.amazonaws.com/shs/'.

Cause 6 — emr.multi-master-forces-termination-protection-on: AssertionError: a 3-master cluster requested with TerminationProtected=False should report True, got False. emr.terminate-protected-multi-master-cluster-rejected: AssertionError: the call succeeded; real AWS rejects a protected cluster's terminate (expected 400 ValidationException 'Could not shut down one or more job flows since they are termination protected.').

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Replay simparity-behavioral-aws-v1 and read the '### emr' section.
3. Cause 1: create a VPC and subnet, then call ec2 CreateVpcEndpoint with VpcEndpointType=Interface and ServiceName='aws.api.<region>.emr-service-cell01' — expect 200 with a VpcEndpoint, observe 400 InvalidServiceName (replay emr.session-lifecycle-start-get-terminate).
4. Cause 2: RunJobFlow a keep-alive cluster, AddJobFlowSteps a command-runner step, poll DescribeStep — expect COMPLETED, observe PENDING forever (replay emr.pending-spark-step-listed-then-cancelled-on-terminate).
5. Cause 3: StartNotebookExecution against a live cluster with a notebook whose cells succeed, poll DescribeNotebookExecution — expect FINISHED, observe FAILED with the emulator's 'runs no notebook' reason (replay emr.e2e-notebook-execution-run-to-completion).
6. Cause 4: RunJobFlow, then DescribeCluster immediately — expect STARTING/BOOTSTRAPPING, observe WAITING (replay emr.step-added-while-cluster-starting-queues-then-runs).
7. Cause 5: CreatePersistentAppUI, then call GetPersistentAppUIPresignedURL twice — expect two different URLs, observe identical bytes (replay emr.persistent-app-ui-presigned-url-token-preserved).
8. Cause 6: RunJobFlow with 3 primaries and TerminationProtected=False, DescribeCluster, then TerminateJobFlows — expect TerminationProtected=true and a 400 ValidationException, observe false and 200 (replay emr.multi-master-forces-termination-protection-on and emr.terminate-protected-multi-master-cluster-rejected).

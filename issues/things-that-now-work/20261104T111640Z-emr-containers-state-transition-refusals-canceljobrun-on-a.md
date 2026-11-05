Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.cancel-job-run-terminal-state-rejected emr-containers.update-virtual-cluster-on-terminated-cluster-rejected
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.cancel-job-run-terminal-state-rejected|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: emr-containers.update-virtual-cluster-on-terminated-cluster-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 6e4639cd3b0eb321e74af4094a3461ae2de27150

# emr-containers state-transition refusals (CancelJobRun on a terminal job run, UpdateVirtualCluster on a TERMINATED cluster) carry the emulator's own wording instead of AWS's message text

> _Note: this issue was identified against the code base at commit `6e4639cd3b0eb321e74af4094a3461ae2de27150`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelJobRun

## Description
Two net-new root causes in the emr-containers emulator, the same family of defect: the operation correctly refuses the illegal state transition with HTTP 400 / ValidationException, but the `message` member is a sentence of the emulator's own composition rather than the text AWS returns for that error class. Both causes must be fixed and both named checks must replay clean before this issue is closed.

Common note: these are the state-transition refusals, not the not-found refusals. The resolved issue 20261029T185510Z-emr-containers-validationexception-refusals-carry-the-emulator-s.md and the open issue 20261030T171255Z-emr-containers-delete-and-pagination-refusals-diverge-in.md are the same wording family but are scoped to the DeleteJobTemplate / DeleteSecurityConfiguration / CreateJobTemplate-clientToken paths and to the DeleteVirtualCluster / DeleteManagedEndpoint / ListJobTemplates / CancelJobRun NOT-FOUND paths respectively; neither touches the two illegal-state-transition messages below, where the resource exists and only its state forbids the call.

Cause 1 of 2 — CancelJobRun on a job run already in a terminal state.
AWS: Smithy: CancelJobRun, DELETE /virtualclusters/{virtualClusterId}/jobruns/{id}, http code 200; both `id` and `virtualClusterId` are ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$; modeled errors are exactly InternalServerException(500) and ValidationException(400), so a terminal-state job run is necessarily a ValidationException. JobRunState enum = PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED. Recorded behavior: cancelling a job run that is already terminal (here CANCELLED) gives HTTP 400, code ValidationException, message `Job run <id> is not in a cancellable state`. The fixed text is `Job run ` / ` is not in a cancellable state`; the only variable part is the job run id taken from the request URI — the message does NOT name the state the job run is in.
Emulator: same status and same error code (those assertions passed), but the message is `Job run <id> is in state <state> and cannot be cancelled` — a different sentence that additionally interpolates the job run's current state from the emulator's own state store.
Why: the error code is shared by every validation refusal of this operation, so a caller that distinguishes "already terminal, nothing to cancel" (a condition idempotent teardown swallows) from other refusals must key on the message; a matcher written against AWS's sentence never fires against the emulator, and cancellation retry loops therefore treat a benign double-cancel as a hard failure.
Fix: the state-guard branch of CancelJobRun's handler in the emr-containers service crate — emit AWS's sentence with only the request's job run id interpolated, and stop appending the resolved state; keep the HTTP 400 and the ValidationException class, and keep the effect absent (the job run must stay exactly in its terminal state).
Traces: check emr-containers.cancel-job-run-terminal-state-rejected.

Cause 2 of 2 — UpdateVirtualCluster on a virtual cluster that has been terminated.
AWS: Smithy: UpdateVirtualCluster, PATCH /virtualclusters/{id}, http code 200, @idempotent; `id` ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$, `clientToken` required ClientToken @idempotencyToken @length 1..64 @pattern \S, `schedulerConfiguration` optional; errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400). VirtualClusterState enum = RUNNING, TERMINATING, TERMINATED, ARRESTED. Recorded behavior: updating a cluster whose state is TERMINATED gives HTTP 400, code ValidationException, message `Virtual cluster state <state> is not valid to create resource UpdateVirtualCluster.` — fixed text `Virtual cluster state ` / ` is not valid to create resource ` / `.` with the cluster's current state and the operation name as the variable parts; note it does NOT echo the cluster id, and it ends in a period.
Emulator: HTTP 400 and ValidationException (correct), message `Virtual cluster <id> is in state <state> and cannot be updated` — the emulator's own sentence, keyed on the request's id rather than naming the operation, and with no trailing period.
Why: same as Cause 1 — the message is the only discriminator for "the resource is in the wrong state for this call", so clients that branch on it (retry on TERMINATING, give up on TERMINATED) do not match the emulator's refusal; the emulator also discloses the cluster id on a path where AWS discloses only the state.
Fix: the state-guard branch of UpdateVirtualCluster's handler in the emr-containers service crate — emit AWS's sentence, interpolating the cluster's current VirtualClusterState and the operation name, and drop the id from the message; leave the ResourceNotFoundException/describe-path wording untouched, and keep the effect absent (the terminated cluster must not pick up a schedulerConfiguration).
Traces: check emr-containers.update-virtual-cluster-on-terminated-cluster-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: emr-containers 14/20 passed, 6 mismatched, 0 errored. In both causes the status (400) and the error code (ValidationException) assertions PASSED and only the message assertion failed.

Cause 1 — [mismatch] emr-containers.cancel-job-run-terminal-state-rejected:
  positive control PASSED: CancelJobRun on the live job run succeeded and DescribeJobRun reached CANCELLED.
  second CancelJobRun on the now-terminal job run:
    expected message: 'Job run <job-run-id> is not in a cancellable state'
    actual   message: 'Job run <job-run-id> is in state CANCELLED and cannot be cancelled'
  (as recorded: expected 'Job run 000000000843radvcrcdozhy4 is not in a cancellable state', got 'Job run 000000000843radvcrcdozhy4 is in state CANCELLED and cannot be cancelled'; the id is a per-run value and fills the template, the defect is the sentence and the extra interpolated state)

Cause 2 — [mismatch] emr-containers.update-virtual-cluster-on-terminated-cluster-rejected:
  setup PASSED: DeleteVirtualCluster on the second cluster drove it to TERMINATED.
  UpdateVirtualCluster(id=<terminated vc>, clientToken=<tok>, schedulerConfiguration={maxConcurrentJobRuns: 5}):
    expected message: 'Virtual cluster state TERMINATED is not valid to create resource UpdateVirtualCluster.'
    actual   message: 'Virtual cluster <vc-id> is in state TERMINATED and cannot be updated'
  (as recorded: got 'Virtual cluster 000000000843r8ou0ydcgv5qo is in state TERMINATED and cannot be updated'; the vc id is per-run — AWS's sentence has no id at all, its variable parts are the state and the operation name)

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query operation=CancelJobRun,UpdateVirtualCluster and shape=JobRunState,VirtualClusterState:
  CancelJobRun: DELETE /virtualclusters/{virtualClusterId}/jobruns/{id} code 200; errors InternalServerException(500), ValidationException(400).
  UpdateVirtualCluster: PATCH /virtualclusters/{id} code 200, idempotent; clientToken required ClientToken idempotencyToken length{1,64} pattern \S; errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  JobRunState = PENDING|SUBMITTED|RUNNING|FAILED|CANCELLED|CANCEL_PENDING|COMPLETED; VirtualClusterState = RUNNING|TERMINATING|TERMINATED|ARRESTED.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1, checks emr-containers.cancel-job-run-terminal-state-rejected and emr-containers.update-virtual-cluster-on-terminated-cluster-rejected.
3. Cause 1: CreateVirtualCluster -> StartJobRun -> CancelJobRun (succeeds) -> poll DescribeJobRun until state CANCELLED -> CancelJobRun again on the same id. Expect HTTP 400 ValidationException with message exactly 'Job run <id> is not in a cancellable state' (id from the request). Observed 'Job run <id> is in state CANCELLED and cannot be cancelled'. Confirm DescribeJobRun still reports exactly CANCELLED after the refused retry.
4. Cause 2: CreateVirtualCluster -> DeleteVirtualCluster -> poll DescribeVirtualCluster until TERMINATED -> UpdateVirtualCluster(id, clientToken, schedulerConfiguration={maxConcurrentJobRuns:5}). Expect HTTP 400 ValidationException with message exactly 'Virtual cluster state TERMINATED is not valid to create resource UpdateVirtualCluster.'. Observed 'Virtual cluster <id> is in state TERMINATED and cannot be updated'. Confirm the terminated cluster still has no schedulerConfiguration, and that the same update against a RUNNING cluster still succeeds and persists maxConcurrentJobRuns=5.
5. Confirm the not-found wording of the Describe* paths is unchanged — only these two state-guard messages change.
6. Re-replay both checks to confirm the fix; both must pass.

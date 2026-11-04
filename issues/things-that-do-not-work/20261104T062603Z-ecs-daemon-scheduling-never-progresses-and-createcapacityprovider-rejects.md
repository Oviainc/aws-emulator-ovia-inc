Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-daemon-update ecs.daemon-deployment-new-revision-ordering ecs.daemon-deployments-lifecycle-progresses ecs.daemon-managed-rollout ecs.daemon-ops-on-missing-cluster-not-found ecs.daemon-revisions-tracks-updates ecs.describe-daemon-unknown-name-not-found ecs.e2e-daemon-instance-join-schedules-task-consequence ecs.e2e-daemon-rolling-update-across-instances ecs.list-daemons-reflects-create-and-delete
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.concurrent-daemon-update|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: ecs.daemon-deployment-new-revision-ordering|_create_once#3|Timeout|time.sleep(2)
Issue-Key: ecs.daemon-deployments-lifecycle-progresses|_create_capacity_provider#2|Timeout|time.sleep(2)
Issue-Key: ecs.daemon-managed-rollout|_create_once#5|Timeout|time.sleep(2)
Issue-Key: ecs.daemon-ops-on-missing-cluster-not-found|_create_managed_instances_cp#3|Timeout|time.sleep(2)
Issue-Key: ecs.daemon-revisions-tracks-updates|_create_once#2|Timeout|time.sleep(2)
Issue-Key: ecs.describe-daemon-unknown-name-not-found|_create_real_daemon#6|Timeout|time.sleep(2)
Issue-Key: ecs.e2e-daemon-instance-join-schedules-task-consequence|_call_with_role_propagation_retry#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs.e2e-daemon-rolling-update-across-instances|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: ecs.list-daemons-reflects-create-and-delete|_setup_daemon_infra#2|AssertionError|raise AssertionError(f"unreachable: could not create Managed Instances capacity provider: {last_exc}")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS daemon scheduling never progresses, and CreateCapacityProvider rejects a valid Managed Instances monitoring value (2 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDaemons

## Description
Cause 1 of 2 — a DAEMON-scheduled workload never converges.
AWS: a DAEMON service (schedulingStrategy=DAEMON) and the Daemon APIs place exactly one task per eligible container instance, report deployments/revisions as the rollout proceeds, converge after a new revision is registered, and answer read operations against an unknown cluster/daemon with ClusterNotFoundException / the documented not-found error rather than hanging.
Emulator: every daemon check that waits for a daemon to settle, roll over, track revisions, or answer a read, polls until the 60s budget expires (`Error: Check timed out after 60.0s`). Separately filed scaffold-stub issues cover DescribeDaemon, DescribeDaemonDeployments, DeleteDaemon and UpdateDaemon answering NotImplemented; the checks listed here do not reach a stub — they hang because the daemon never reaches a state the poll is waiting for.
Why: a consumer that models per-instance agents (log shippers, monitoring agents) as a DAEMON service gets a resource that is accepted and never converges, and gets no error to react to.
Fix: the ECS daemon/service-scheduler state machine — DAEMON placement and the daemon deployment/revision bookkeeping, plus the not-found paths of the daemon read operations.
Traces: ecs.concurrent-daemon-update, ecs.daemon-deployment-new-revision-ordering, ecs.daemon-deployments-lifecycle-progresses, ecs.daemon-managed-rollout, ecs.daemon-ops-on-missing-cluster-not-found, ecs.daemon-revisions-tracks-updates, ecs.describe-daemon-unknown-name-not-found, ecs.e2e-daemon-instance-join-schedules-task-consequence, ecs.e2e-daemon-rolling-update-across-instances.

Cause 2 of 2 — CreateCapacityProvider refuses a valid Managed Instances monitoring value.
AWS: CreateCapacityProvider accepts a managedInstancesProvider whose instanceLaunchTemplate monitoring is one of the modelled enum values; a request using the documented value is created, not refused.
Emulator: the call is refused with InvalidParameterException and the emulator-authored message 'monitoring should be one of [BASIC,DETAILED]', so the check cannot even provision its subject.
Why: a Managed Instances capacity provider cannot be created at all, and the refusal message is the emulator's own prose rather than AWS's ValidationException/InvalidParameterException wording (fixed text plus the offending member name and the allowed values taken from the model).
Fix: the CreateCapacityProvider handler's managedInstancesProvider validation — accept the modelled monitoring values, and when a value really is invalid refuse with AWS's message template.
Traces: ecs.list-daemons-reflects-create-and-delete.

## Evidence
Cause 1 — expected: the DAEMON service/daemon reaches a steady state with one task per container instance and its deployments/revisions are readable; a read against a missing cluster is refused with ClusterNotFoundException. Actual: `Error: Check timed out after 60.0s` on all nine listed checks.
Cause 2 — expected: CreateCapacityProvider with a managedInstancesProvider succeeds. Actual: `AssertionError: unreachable: could not create Managed Instances capacity provider: An error occurred (InvalidParameterException) when calling the CreateCapacityProvider operation: monitoring should be one of [BASIC,DETAILED]` — the bracketed list and the lower-case sentence are emulator-authored prose, not AWS wording.

## Steps to Reproduce
Cause 1: CreateCluster, register one container instance, create a DAEMON-scheduled service (or a Daemon), then poll its deployments/revisions — it never converges; also call a daemon read against a cluster name that does not exist and observe no refusal.
Cause 2: call CreateCapacityProvider with a managedInstancesProvider.instanceLaunchTemplate carrying the modelled monitoring value and observe the InvalidParameterException above.

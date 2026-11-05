Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container ecs.ephemeral-storage-request-is-provisioned-inside-the-container ecs.run-task-environment-override-injected ecs.submit-attachment-state-change-updates-attachment-status ecs.submit-attachment-state-changes-updates-eni-status
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: ecs.ephemeral-storage-request-is-provisioned-inside-the-container|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: ecs.run-task-environment-override-injected|_poll_until#21|Timeout|return last
Issue-Key: ecs.submit-attachment-state-change-updates-attachment-status|run#11|AssertionError|raise last_error or AssertionError("attachment never reached ATTACHING across retries")
Issue-Key: ecs.submit-attachment-state-changes-updates-eni-status|run#9|AssertionError|assert final_task_arn is not None, f"never won the race to submit ATTACHED first: {last_race_error}"
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs: 2 net-new causes — containers never execute (so in-container placement, workdir/user/ulimit, ephemeralStorage and env overrides are unverifiable) and task ENI attachments skip the ATTACHING window

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new root causes in ECS. None of these six checks was marked ALREADY FILED or OWNED by the replay report. The open issue 20261104T062603Z-ecs-containers-never-execute-no-in-container-behaviour.md covers other checks in this family but does not carry these four replay keys; 20261104T062603Z-ecs-awsvpc-task-enis-never-attach-the-elasticnetworkinterface.md is adjacent to Cause 2 but does not carry its two replay keys.

Cause 1 of 2: a task's container never actually runs, so nothing the task definition requests is observable from inside it.
AWS: RunTask launches the container image with the requested runtime settings and the container's own process observes them. The four checks each read one such setting back from inside the container, via the container's exit code or its stdout: (a) a task naming neither launchType nor capacityProviderStrategy is placed by the cluster's default capacity-provider strategy while an explicit strategy overrides it, and both containers really run (Smithy: RunTask input carries capacityProviderStrategy and launchType, both optional); (b) a container runs in its workingDirectory, as its user, with its nofile ulimits — the probe encodes any unapplied setting as a bit in its exit code (expected 40) and a control container without the settings reports the workdir and uid bits; (c) a task-level ephemeralStorage request is provisioned — a container in a 50 GiB task measures a larger filesystem than its default-storage twin and at least 80% of the request, and the documented floor rejects sizeInGiB=20 while accepting 21 (Smithy: ecs#EphemeralStorage.sizeInGiB — "The minimum supported value is 21 GiB and the maximum supported value is 200 GiB"); (d) a containerOverrides environment value reaches the container process and replaces the task definition's value (exit code 17, not 1), while an override naming an unknown container is rejected with InvalidParameterException and launches no task (Smithy: RunTask errors include InvalidParameterException, httpError 400).
Emulator: all four checks are killed by the harness with "Check timed out after 60.0s" while waiting on the task to produce its in-container evidence. No container process ever runs, so no exit code, no measured filesystem size and no stdout is ever produced; the settings are accepted and echoed by the control plane but never applied anywhere.
Why: a client uses ECS to run code. Every one of these is silently a no-op: a task "RUNNING" with the wrong working directory, the wrong uid, default ulimits, default disk and the task definition's (not the override's) environment is indistinguishable from a correct one, and automation that reads a task's exit code or stdout to decide success never gets a value.
Fix: the RunTask/StartTask task-execution path (container runtime invocation and the propagation of workingDirectory, user, ulimits, task-level ephemeralStorage, containerOverrides.environment and the resolved capacity-provider placement into it), plus the task exit-code/stdout reporting that DescribeTasks and awslogs expose.
Traces: ecs.capprov-cluster-default-strategy-governs-a-task-that-names-no-provider, ecs.container-workdir-user-and-ulimit-apply-inside-the-running-container, ecs.ephemeral-storage-request-is-provisioned-inside-the-container, ecs.run-task-environment-override-injected.

Cause 2 of 2: a Fargate task's ENI attachment is published as ATTACHED immediately, with no ATTACHING window, so SubmitAttachmentStateChanges can never be exercised.
AWS: an awsvpc task's ElasticNetworkInterface attachment passes through ATTACHING before ATTACHED (Smithy: ecs#Attachment.status — "Valid values are PRECREATED, CREATED, ATTACHING, ATTACHED, DETACHING, DETACHED, DELETED, and FAILED"), and SubmitAttachmentStateChanges is the agent-facing call that moves it ATTACHING → ATTACHED; AWS accepts the transition only from ATTACHING and only to ATTACHED (Smithy: SubmitAttachmentStateChanges input attachments: AttachmentStateChanges (required); errors AccessDeniedException 403, ClientException 400, ClusterNotFoundException 400, InvalidParameterException 400, ServerException 500). DescribeTasks reports the resulting status.
Emulator: the attachment is already 'ATTACHED' the first time it is observable, so the ATTACHING window never exists and the submission can never win the race — the checks fail with "attempt 3: attachment reached 'ATTACHED' before an ATTACHING window could be observed" and "never won the race to submit ATTACHED first: the real platform reported ATTACHED itself before our submission".
Why: an agent (or any client emulating one) cannot drive or observe attachment progress; code that waits for ATTACHING, or that submits the ATTACHED transition and expects it to be the one that took effect, behaves differently here than on AWS, and the modeled refusals for a non-ATTACHING source status can never be reached.
Fix: ECS task-attachment state — publish the attachment in ATTACHING on task launch and only move it to ATTACHED when SubmitAttachmentStateChanges (or the platform's own timed transition) reports it, with the source-status validation that call requires.
Traces: ecs.submit-attachment-state-change-updates-attachment-status, ecs.submit-attachment-state-changes-updates-eni-status.

## Evidence
Cause 1 — Expected (AWS): the probe container exits 40 (workdir/user/ulimit all applied); the 50 GiB task measures ≥80% of the request and ≥ its default-storage twin while sizeInGiB=20 is refused and 21 accepted; the overridden env value yields exit code 17 and an unknown-container override is refused with InvalidParameterException; both capacity-provider-placed containers really run. Actual (emulator): all four checks are terminated with `Check timed out after 60.0s` — no container process, no exit code, no stdout, no measured filesystem.
Cause 2 — Expected (AWS): DescribeTasks shows the task's ElasticNetworkInterface attachment in ATTACHING, and SubmitAttachmentStateChanges moves it to ATTACHED. Actual (emulator): AssertionError: "attempt 3: attachment reached 'ATTACHED' before an ATTACHING window could be observed" and AssertionError: "never won the race to submit ATTACHED first: the real platform reported ATTACHED itself before our submission".

## Steps to Reproduce
Cause 1:
1. CreateCluster, then RegisterTaskDefinition (FARGATE/awsvpc) with workingDirectory, user and nofile ulimits set on the container, and a probe command that encodes unapplied settings as bits in its exit code.
2. RunTask and read the container's exit code from DescribeTasks → never produced; check times out at 60s.
3. Repeat with task-level ephemeralStorage sizeInGiB=50 (and 20 / 21 for the floor), and with containerOverrides.environment replacing a task-definition value → no in-container measurement or exit code is ever produced.
4. PutClusterCapacityProviders with a default strategy, then RunTask naming neither launchType nor capacityProviderStrategy → the container never runs.
Cause 2:
1. CreateCluster, RegisterTaskDefinition (awsvpc/FARGATE), RunTask into a Fargate subnet.
2. Poll DescribeTasks for attachments[].status → it is already 'ATTACHED'; ATTACHING is never observed, and SubmitAttachmentStateChanges can never be the call that effects the transition.

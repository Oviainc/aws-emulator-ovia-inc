Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.exec-protection-agent-ops-on-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.exec-protection-agent-ops-on-missing-cluster-not-found|run#48|AssertionError|assert code == "InvalidParameterException", f"SubmitTaskStateChange control: got {code}"
Issue-Commit: a4e4b64c9329e0431735731509414d0e27b157c2

# ECS SubmitTaskStateChange answers 501 NotImplemented, so neither its missing-task refusal (InvalidParameterException) nor its missing-cluster refusal (ClientException "Cluster not found.") ever happens

> _Note: this issue was identified against the code base at commit `a4e4b64c9329e0431735731509414d0e27b157c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitTaskStateChange

## Description
Cause 1 of 1: `SubmitTaskStateChange` is an unimplemented scaffold stub, so every call to it is refused with a non-modelled 501 `NotImplemented` regardless of input. Its agent-facing siblings in the same family are implemented and behave correctly in this same check (`SubmitAttachmentStateChanges` and `SubmitContainerStateChange` both produced the right code and message for both the control and the missing-cluster case, and `ExecuteCommand`, `GetTaskProtection`, `UpdateTaskProtection` and `UpdateContainerAgent` passed), so this is an isolated gap in one operation, not the family.

AWS: `SubmitTaskStateChange` is a real, modelled ECS operation (Smithy: input `SubmitTaskStateChangeRequest` with members `cluster`, `task`, `status`, `reason`, `containers`, `attachments`, `managedAgents`, `pullStartedAt`, `pullStoppedAt`, `executionStoppedAt`; output `SubmitTaskStateChangeResponse` with `acknowledgment`). Its modelled errors are `AccessDeniedException` (HTTP 403), `ClientException` (HTTP 400), `ClusterNotFoundException` (HTTP 400), `InvalidParameterException` (HTTP 400) and `ServerException` (HTTP 500); 501/`NotImplemented` is not among them. Recorded behavior (audited against real AWS, account <leased-account>): called against an EXISTING cluster with a task ARN that names no task, AWS refuses HTTP 400 `InvalidParameterException` with message `The referenced task was not found` — fixed text in full, no variable part. Called with `cluster` naming a cluster that does not exist, AWS refuses HTTP 400 `ClientException` with message `Cluster not found.` — again entirely fixed text. Note the family split the recording establishes: for the agent-facing `Submit*` operations an absent cluster is `ClientException`, not `ClusterNotFoundException` (which is what `ExecuteCommand`/`GetTaskProtection`/`UpdateTaskProtection`/`UpdateContainerAgent` return with message `Cluster not found.`), and the cluster check runs BEFORE the task lookup — the control call proves the task-level refusal is reached only when the cluster resolves.

Emulator: the handler returns `Err(crate::scaffolded::Scaffolded::not_implemented("SubmitTaskStateChange"))` unconditionally, i.e. HTTP 501 with error code `NotImplemented` and message `operation SubmitTaskStateChange is scaffolded but not implemented in this emulator`. The replay observed error code `NotImplemented` where `InvalidParameterException` was required (`AssertionError: SubmitTaskStateChange control: got NotImplemented`); the missing-cluster leg is unreachable for the same reason, so the `ClientException` / `Cluster not found.` refusal is never produced either. No state change is recorded and no `acknowledgment` is returned on the success path.

Why: a client relying on AWS's behavior cannot acknowledge a task state transition at all — the ECS-agent protocol path is dead. Code that distinguishes a bad reference (`InvalidParameterException`, retry pointless) from a missing cluster (`ClientException`) from a server fault (`ServerException`, retry) sees a single 501 for all three, so error-class-driven retry and reconciliation logic takes the wrong branch, and a 501 is not in the operation's modelled error set at all, so a typed SDK surfaces it as an unmodelled/unknown failure.

Fix: implement the operation's handler and its validation — resolve `cluster` first and refuse an unresolved cluster with `ClientException` ("Cluster not found."), then resolve `task` within that cluster and refuse an unresolved task with `InvalidParameterException` ("The referenced task was not found"), then apply the status/timestamp/container/attachment/managed-agent state change to the task in the service state and serialize the `acknowledgment` output. Delete the SCAFFOLDED marker when done.

Traces: failing check `ecs.exec-protection-agent-ops-on-missing-cluster-not-found` (assertion at check line 169, the SubmitTaskStateChange positive control). Handler: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/submit_task_state_change.rs lines 3-6 (SCAFFOLDED marker) and line 181 (the unconditional `not_implemented` return).

## Evidence
Expected (real AWS, per the audited check):
  - SubmitTaskStateChange(cluster=<existing cluster>, task=<ARN of no such task>, status=RUNNING)
      -> HTTP 400, Error.Code = "InvalidParameterException", Error.Message = "The referenced task was not found"
  - SubmitTaskStateChange(cluster=<nonexistent cluster>, task=<ARN under that cluster>, status=RUNNING)
      -> HTTP 400, Error.Code = "ClientException", Error.Message = "Cluster not found."

Actual (emulator, both legs):
  - HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation SubmitTaskStateChange is scaffolded but not implemented in this emulator"

Replay output:
  [mismatch] ecs.exec-protection-agent-ops-on-missing-cluster-not-found
  File "check_ecs_exec_protection_agent_ops_on_missing_cluster_not_found.py", line 169, in run
    assert code == "InvalidParameterException", f"SubmitTaskStateChange control: got {code}"
  AssertionError: SubmitTaskStateChange control: got NotImplemented

Source confirming the cause (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/submit_task_state_change.rs:181):
    Err(crate::scaffolded::Scaffolded::not_implemented("SubmitTaskStateChange"))

Model confirming 501/NotImplemented is not a legal response (Smithy, SubmitTaskStateChange.errors):
    AccessDeniedException(403), ClientException(400), ClusterNotFoundException(400), InvalidParameterException(400), ServerException(500)

Control for the family split (these passed, so the divergence is isolated to SubmitTaskStateChange):
  - SubmitContainerStateChange control -> InvalidParameterException "The referenced task was not found"  (PASS)
  - SubmitContainerStateChange, bogus cluster -> ClientException "Cluster not found."  (PASS)
  - SubmitAttachmentStateChanges, bogus cluster -> ClientException "Cluster not found."  (PASS)

## Steps to Reproduce
1. Boot the emulator and point an ECS client at it.
2. ecs.create_cluster(clusterName="repro-cluster").
3. Positive control — call against the REAL cluster with a task ARN that names no task:
     ecs.submit_task_state_change(cluster="repro-cluster",
       task="arn:aws:ecs:<region>:<account>:task/repro-cluster/00000000000000000000000000000000",
       status="RUNNING")
   Expect HTTP 400 InvalidParameterException "The referenced task was not found".
   Observe HTTP 501 NotImplemented.
4. Missing-cluster leg — repeat with cluster="no-such-cluster-<suffix>" and a task ARN under that name.
   Expect HTTP 400 ClientException "Cluster not found.".
   Observe HTTP 501 NotImplemented.
5. Contrast with the implemented siblings, which answer correctly for both legs:
     ecs.submit_container_state_change(cluster=..., task=..., containerName="app", status="RUNNING")
     ecs.submit_attachment_state_changes(cluster=..., attachments=[{"attachmentArn": ..., "status": "ATTACHED"}])
6. Or replay the check directly: ecs.exec-protection-agent-ops-on-missing-cluster-not-found

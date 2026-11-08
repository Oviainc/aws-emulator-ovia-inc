Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-daemon-update ecs.submit-attachment-state-changes-updates-attachment-status
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.concurrent-daemon-update|run#7|AssertionError|assert code in ("ClientException", "DaemonNotActiveException", "ConflictException"), (
Issue-Key: ecs.submit-attachment-state-changes-updates-attachment-status|_poll_until#5|Timeout|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Commit: 0fba5a4732f94b5ef6351efe96de38b77f8e6c14

# ecs: 2 net-new causes — UpdateDaemon answers a concurrent update with the non-AWS error code NotImplemented, and the awsvpc ENI attachment / SubmitAttachmentStateChanges path never completes

> _Note: this issue was identified against the code base at commit `0fba5a4732f94b5ef6351efe96de38b77f8e6c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDaemon

## Description
Two independent root causes in the ECS surface, both grounded in failing checks the suite lists under `### ecs` and neither marked ALREADY FILED nor OWNED by an open issue. Both must be closed before submitting.

Cause 1 of 2: UpdateDaemon refuses every call — including both racers of a concurrent update — with the error code `NotImplemented`, which ECS does not model.
  AWS: UpdateDaemon(daemonArn, daemonTaskDefinitionArn, capacityProviderArns) re-points an ACTIVE managed daemon at a new daemon task definition and returns {daemonArn, status, createdAt, updatedAt, deploymentArn}; DescribeDaemon then settles ACTIVE and the daemon's latest deployment resolves a target revision carrying one of the submitted task definitions. When two UpdateDaemon calls race, at most one loses, and a losing call is refused only with an error ECS models for the operation (Smithy: UpdateDaemon errors AccessDeniedException @403, ClientException/ClusterNotFoundException/DaemonNotActiveException/DaemonNotFoundException/InvalidParameterException/PlatformUnknownException/UnsupportedFeatureException @400, ServerException @500 — there is no `NotImplemented` member, and the message of each is AWS's fixed text for that class with the daemon ARN / in-flight deployment ARN taken from the request and state).
  Emulator: the call is answered by the unimplemented scaffold stub, whose error code is the literal `NotImplemented` (not an ECS-modeled code at the modeled status), so the check's assertion on the racer's code fails: `AssertionError: expected a losing racer (if any) to fail with a real conflict code, got NotImplemented`. No update is applied and no deployment is produced.
  Why: a client cannot update a managed daemon at all, and the refusal it does get is unclassifiable — botocore maps it to no modeled ECS error, so conflict-retry logic keyed on ClientException/ConflictException/DaemonNotActiveException never triggers and the error is surfaced as an unknown failure.
  Fix: the UpdateDaemon handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/update_daemon.rs) and the daemon/daemon-deployment state it must mutate; while it remains a stub the refusal must at least use a modeled ECS error class, not `NotImplemented`. NOTE: the open issue 20261104T062603Z-ecs-updatedaemon-stub.md records the UpdateDaemon stub itself and the suite did not attach this check's replay key to it (this check fails on an assertion about the error code, not on the raw stub call) — close them together. The open 20261029T220901Z-ecs-express-gateway-and-daemon-operations-skip-validation.md and 20261104T062603Z-ecs-daemon-scheduling-never-progresses-and-createcapacityprovider-rejects.md cover other daemon causes and do not carry this key.
  Traces: ecs.concurrent-daemon-update.

Cause 2 of 2: an awsvpc task's ENI attachment never reaches ATTACHING and SubmitAttachmentStateChanges is never exercisable — the whole check hangs past the 180s harness budget.
  AWS: RunTask with launchType EC2 and an awsvpcConfiguration on a registered container instance creates an ElasticNetworkInterface attachment that DescribeTasks reports in `attachments`, moving PRECREATED -> ATTACHING on its own; the ECS agent then reports it with SubmitAttachmentStateChanges(cluster, attachments=[{attachmentArn, status: ATTACHED}]) (Smithy: SubmitAttachmentStateChanges input {cluster, attachments (required)}, output {acknowledgment}, errors AccessDeniedException @403, ClientException/ClusterNotFoundException/InvalidParameterException @400, ServerException @500) and DescribeTasks then shows that attachment with status ATTACHED. The supported way to register the container instance first is to read an AWS-signed EC2 instance identity document and signature from the instance itself through SSM Run Command.
  Emulator: the check never gets through that chain — it is killed by the harness after 180.0s (`Error: Check timed out after 180.0s`) without ever reaching the ATTACHED assertion, so no attachment is observable in ATTACHING and the agent-plane report can neither succeed nor be refused. Recorded behavior only for the timing: against AWS the same chain completes inside the budget.
  Why: a client emulating the ECS agent, or any test that asserts on an awsvpc task's ENI attachment lifecycle, sees a task with no usable attachment forever and hangs instead of getting an answer.
  Fix: the ECS task state machine's attachment lifecycle (publish the ElasticNetworkInterface attachment and its PRECREATED->ATTACHING transition on DescribeTasks) and the submit_attachment_state_changes handler that must apply the ATTACHED report; upstream, the EC2 instance-identity-document/SSM managed-instance path the registration depends on. The open 20261107T220512Z-ecs-5-net-new-causes-no-obtainable-ec2.md records that upstream agent-plane cause for OTHER check ids under a different replay key (explicit AssertionErrors, not this 180s harness timeout), so this check is not owned by it — close them together.
  Traces: ecs.submit-attachment-state-changes-updates-attachment-status.

## Evidence
Cause 1 — expected: UpdateDaemon applies one of the two racing task definitions, DescribeDaemon stays ACTIVE with a deploymentArn, and a losing racer (if any) is refused with a modeled ECS error (ClientException / DaemonNotActiveException / ConflictException) at HTTP 400. Actual: `AssertionError: expected a losing racer (if any) to fail with a real conflict code, got NotImplemented` — the error code is the scaffold's `NotImplemented`, which ECS does not model for UpdateDaemon; no update is applied.

Cause 2 — expected: DescribeTasks reports attachments[0] {type: ElasticNetworkInterface, status: ATTACHING}, SubmitAttachmentStateChanges(status=ATTACHED) returns an acknowledgment, and DescribeTasks then reports attachments[0].status == ATTACHED. Actual: `Error: Check timed out after 180.0s` — the attachment never becomes observable in ATTACHING, so the submit is never reached.

## Steps to Reproduce
Cause 1: ecs create-cluster; create a Managed Instances capacity provider and wait for ACTIVE; ecs create-daemon with a daemon task definition; register two more daemon task definitions (v2, v3); fire two concurrent ecs update-daemon calls on the same daemonArn, one per revision -> each raises error code `NotImplemented` instead of applying one update and (at most) refusing the loser with a modeled ECS error. Replay ecs.concurrent-daemon-update.

Cause 2: create a VPC with a public subnet; launch an SSM-capable EC2 instance and read its instance identity document + signature; ecs register-container-instance with that pair; ecs register-task-definition with networkMode awsvpc; ecs run-task --launch-type EC2 with an awsvpcConfiguration; poll ecs describe-tasks for attachments[0].status == ATTACHING, then ecs submit-attachment-state-changes --attachments attachmentArn=<arn>,status=ATTACHED and poll for ATTACHED -> the run never gets that far and is cut off at the 180s budget. Replay ecs.submit-attachment-state-changes-updates-attachment-status.

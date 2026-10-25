Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.invalid-parameter-daemon-taskdef-and-namespace
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.invalid-parameter-daemon-taskdef-and-namespace|_error#10|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: eccc631279b459acd1f5ae10e81d9e1b78dcc975

# ecs RegisterDaemonTaskDefinition accepts a container with a negative cpu instead of rejecting it with InvalidParameterException

> _Note: this issue was identified against the code base at commit `eccc631279b459acd1f5ae10e81d9e1b78dcc975`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterDaemonTaskDefinition

## Description
One net-new root cause (Cause 1 of 1), grounded in behavioral check `ecs.invalid-parameter-daemon-taskdef-and-namespace`.

Cause 1 of 1: RegisterDaemonTaskDefinition performs no numeric validation of a container definition's `cpu` (and, by the same gap in the handler, `memory`), so a container reservation that AWS refuses is stored and a revision is minted for it.

AWS: Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json) models RegisterDaemonTaskDefinition with `containerDefinitions` (DaemonContainerDefinitionList) required and declares `InvalidParameterException` (HTTP 400, "The specified parameter isn't valid. Review the available parameters for the API request.") among its errors. The model expresses no numeric range on a container's `cpu`, so the range itself is recorded behavior only: the audited capture shows real AWS refusing a daemon container whose `cpu` is negative with HTTP 400, error code `InvalidParameterException` and message `Invalid 'cpu' setting for container 'app'.` — in that message the fixed text is `Invalid 'cpu' setting for container '` + `'.`, and the quoted token is the offending container definition's own `name` taken from the request (the recording's `app` merely fills that slot; it is not part of the template). The CPU reservation must be a non-negative integer count of CPU units, so a negative value is invalid input and never reaches state.

Emulator: the call SUCCEEDS with HTTP 200 and returns a `daemonTaskDefinitionArn` (replay output: `{'daemonTaskDefinitionArn': 'arn:aws:ecs:...:daemon-task-definition/<family>:1', ResponseMetadata HTTPStatusCode 200}`), i.e. the malformed registration consumed a family revision and is persisted. services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_daemon_task_definition.rs validates only `family` (lines 200-213), the container-definition list bounds (lines 218-240), and each container's `name`/`image`/uniqueness (lines 244-281); nothing inspects a container's `cpu` or `memory` before the record is inserted at lines 314-346.

Why: a client that relies on AWS rejecting an impossible CPU reservation gets a silent acceptance instead. The bad revision becomes ACTIVE and describable, so validation errors that AWS surfaces synchronously at registration time are deferred to (or lost at) daemon placement, and client code written to catch InvalidParameterException and correct the value never sees the error. Because registration also increments the family's revision counter, a refused call in AWS leaves the family untouched while here it burns revision numbers.

Fix: input validation in the RegisterDaemonTaskDefinition handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_daemon_task_definition.rs), in the per-container loop that already checks `name`/`image`, before any state mutation: reject a non-positive/negative container `cpu` (and the equivalent `memory` reservation) with InvalidParameterException carrying AWS's wording, and return before the revision counter is advanced or the record inserted.

Note on scope: the same check goes on to assert InvalidParameterException on DescribeDaemonTaskDefinition / DeleteDaemonTaskDefinition with a malformed identifier, ListDaemonTaskDefinitions with maxResults=0, ListServicesByNamespace with a malformed namespace, and ClientException on DiscoverPollEndpoint with a malformed containerInstance. Replay never reached those calls — it aborted on the accepted RegisterDaemonTaskDefinition above — so they are NOT separately claimed here as observed divergences; the fixer will see them when re-replaying the check id below. Related but not covering this: open issue 20261025T082616Z-ecs-deletedaemontaskdefinition-stub.md (DeleteDaemonTaskDefinition is an unimplemented stub) and 20261025T082616Z-ecs-createdaemon-is-an-unimplemented-scaffold-stub-so.md.

## Evidence
Check: ecs.invalid-parameter-daemon-taskdef-and-namespace (verdict: mismatch).

Expected (real AWS, audited capture): RegisterDaemonTaskDefinition with containerDefinitions=[{name:"app", image:"public.ecr.aws/amazonlinux/amazonlinux:latest", cpu:-100, memory:128, essential:true}] -> HTTP 400, Error.Code = "InvalidParameterException", Error.Message = "Invalid 'cpu' setting for container 'app'." (fixed text "Invalid 'cpu' setting for container '" ... "'."; the quoted name is the request's container name).

Actual (emulator): HTTP 200 with body {"daemonTaskDefinitionArn": "arn:aws:ecs:<region>:<account>:daemon-task-definition/<family>:1"}. Harness assertion: AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'daemonTaskDefinitionArn': 'arn:aws:ecs:us-west-2:000000000000:daemon-task-definition/daemon-taskdef-b970bb5efd0d:1', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.

Code evidence: register_daemon_task_definition.rs has no validation of DaemonContainerDefinition.cpu/.memory anywhere between the engine lookup (line 192) and the insert (line 346).

## Steps to Reproduce
1. Point an ECS client at the emulator.
2. Call RegisterDaemonTaskDefinition with family=<any valid family name> and containerDefinitions=[{"name": "app", "image": "public.ecr.aws/amazonlinux/amazonlinux:latest", "cpu": -100, "memory": 128, "essential": true}].
3. Observe HTTP 200 and a returned daemonTaskDefinitionArn; real AWS returns HTTP 400 InvalidParameterException "Invalid 'cpu' setting for container 'app'.".
4. Call ListDaemonTaskDefinitions(familyPrefix=<family>) / DescribeDaemonTaskDefinition on the returned ARN and observe the malformed revision is ACTIVE and the family's revision counter was consumed.
Full repro: re-replay behavioral check ecs.invalid-parameter-daemon-taskdef-and-namespace.

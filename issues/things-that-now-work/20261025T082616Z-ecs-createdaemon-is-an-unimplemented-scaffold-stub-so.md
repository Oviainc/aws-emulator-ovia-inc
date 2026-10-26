Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.daemon-deployment-timeout-marks-failed
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.daemon-deployment-timeout-marks-failed|run#12|AssertionError|assert code == "InvalidParameterException", (
Issue-Commit: 8a7d0174a619a438b7db3f8bae6f70b79955a1d5

# ecs: CreateDaemon is an unimplemented scaffold stub, so a non-MANAGED_INSTANCES capacity provider is never refused (1 cause)

> _Note: this issue was identified against the code base at commit `8a7d0174a619a438b7db3f8bae6f70b79955a1d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDaemon

## Description
1 net-new root cause in the ecs service.

Cause 1 of 1: `CreateDaemon` answers HTTP 501 `NotImplemented` for every call, so the ECS Managed Daemon control plane cannot be exercised at all and none of the operation's modeled refusals happen.

AWS: `CreateDaemon` creates a daemon in a cluster across the named capacity providers. Smithy (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): CreateDaemon input `daemonName` (required), `daemonTaskDefinitionArn` (required), `capacityProviderArns` (StringList, required), optional `deploymentConfiguration`/`tags`/`propagateTags`/`enableECSManagedTags`/`enableExecuteCommand`/`clientToken`/`critical`; output `daemonArn`, `status` (DaemonStatus: ACTIVE | DELETE_IN_PROGRESS), `createdAt`, `deploymentArn`; modeled errors AccessDeniedException (403), ClientException (400), ClusterNotFoundException (400), InvalidParameterException (400), PlatformUnknownException (400), ServerException (500), UnsupportedFeatureException (400). The model's own operation documentation states "ECS Managed Daemons is only supported for Amazon ECS Managed Instances Capacity Providers", so a `capacityProviderArns` list containing a capacity provider that is not of MANAGED_INSTANCES type is an invalid parameter and AWS refuses the call with HTTP 400 `InvalidParameterException`; the recording shows the message `All capacity providers must be of MANAGED_INSTANCES type for Daemon services` (recorded message, an example of the rule "the request named a capacity provider whose type is not MANAGED_INSTANCES"). AWS must leave no daemon behind for a refused call, so `ListDaemons` on that cluster still returns an empty `daemonSummariesList`.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented` ("operation CreateDaemon is scaffolded but not implemented in this emulator") regardless of input. The handler is a generated stub: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_daemon.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDaemon"))`. Note the surrounding daemon surface DOES answer in this emulator — the check got through `RegisterDaemonTaskDefinition`, `DescribeCapacityProviders(["FARGATE"])` and `ListDaemons` — so CreateDaemon is the single missing piece, not a wholly absent feature area.

Why: a client that provisions ECS Managed Daemons cannot create one at all, and a client whose error handling distinguishes a bad request from a server/unsupported condition is told the wrong thing: it sees a 501 `NotImplemented` (an unrecoverable "this API does not exist" signal, which SDK retry/catch logic treats quite differently) where AWS returns a 400 `InvalidParameterException` naming a fixable input problem. Negative tests that assert the MANAGED_INSTANCES-only constraint pass on AWS and fail here, and any daemon-dependent setup (daemon deployment, drain/replace behaviour, ListDaemons state) cannot be reached.

Fix: implement the `CreateDaemon` handler — services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_daemon.rs — including its input validation (resolve each entry of `capacityProviderArns` and refuse with modeled `InvalidParameterException` when any is not a MANAGED_INSTANCES capacity provider, and `ClusterNotFoundException` for an unknown `clusterArn`) and the daemon record it must write into the ECS service state so `ListDaemons` reflects created daemons and nothing for refused calls; then delete the SCAFFOLDED marker line in that file.

Traces: ecs.daemon-deployment-timeout-marks-failed — asserts `CreateDaemon` with the built-in FARGATE capacity provider is rejected with `InvalidParameterException` and that `ListDaemons` still reports no daemon; it failed with `AssertionError: expected CreateDaemon to reject a non-Managed-Instances capacity provider (FARGATE) with InvalidParameterException, got code='NotImplemented'`.

## Evidence
Check ecs.daemon-deployment-timeout-marks-failed [mismatch]:

Expected (real AWS, per the check's audited recording and the Smithy model):
  CreateDaemon(clusterArn=<cluster>, daemonName=<name>, daemonTaskDefinitionArn=<daemon td>, capacityProviderArns=[<FARGATE cp arn>])
  -> HTTP 400, Error.Code = "InvalidParameterException"
  -> Error.Message (recorded example) = "All capacity providers must be of MANAGED_INSTANCES type for Daemon services"
  then ListDaemons(clusterArn=<cluster>) -> daemonSummariesList == []

Actual (emulator):
  -> HTTP 501, Error.Code = "NotImplemented"
  -> Error.Message = "operation CreateDaemon is scaffolded but not implemented in this emulator"

Replay output:
  File "check_ecs_daemon_deployment_timeout_marks_failed.py", line 50, in run
    assert code == "InvalidParameterException", (
  AssertionError: expected CreateDaemon to reject a non-Managed-Instances capacity provider (FARGATE) with InvalidParameterException, got code='NotImplemented'

Source of the 501, in the checkout:
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_daemon.rs:4   "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`"
  services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_daemon.rs:225 "Err(crate::scaffolded::Scaffolded::not_implemented(\"CreateDaemon\"))"

Neighbouring daemon operations DO answer (the check reached CreateDaemon only after RegisterDaemonTaskDefinition and DescribeCapacityProviders succeeded), so this is scoped to CreateDaemon. No open issue names CreateDaemon (list_existing_issues query='daemon' returns none; the filed daemon stubs are DeleteDaemonTaskDefinition and ListDaemonTaskDefinitions), and the replay report did not mark this check ALREADY FILED or OWNED.

## Steps to Reproduce
1. Boot the emulator and point an ECS client at it.
2. ecs.create_cluster(clusterName="<cl>") -> note clusterArn.
3. ecs.register_daemon_task_definition(family="<fam>", containerDefinitions=[{"name":"c","image":"public.ecr.aws/docker/library/alpine:latest","memory":64,"essential":true}]) -> note daemonTaskDefinitionArn.
4. ecs.describe_capacity_providers(capacityProviders=["FARGATE"]) -> note capacityProviderArn (a non-MANAGED_INSTANCES provider).
5. ecs.create_daemon(clusterArn=<clusterArn>, daemonName="<daemon>", daemonTaskDefinitionArn=<daemonTaskDefinitionArn>, capacityProviderArns=[<FARGATE capacityProviderArn>]).
   Observed: HTTP 501, Error.Code "NotImplemented".
   Expected: HTTP 400, Error.Code "InvalidParameterException" (capacity providers for a daemon must all be MANAGED_INSTANCES).
6. ecs.list_daemons(clusterArn=<clusterArn>) -> must return daemonSummariesList == [] (no daemon created by the refused call).
7. Re-replay behavioral check ecs.daemon-deployment-timeout-marks-failed to confirm.

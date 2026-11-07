Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-daemon-update ecs.daemon-deployment-new-revision-ordering ecs.daemon-deployments-lifecycle-progresses ecs.daemon-managed-rollout ecs.daemon-ops-on-missing-cluster-not-found ecs.daemon-revisions-tracks-updates ecs.delete-cluster-with-capacity-provider-rejected ecs.describe-daemon-unknown-name-not-found ecs.e2e-daemon-instance-join-schedules-task-consequence
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.concurrent-daemon-update|_poll_until#6|AssertionError|raise AssertionError(f"timed out waiting for {description!r} (last={last!r})")
Issue-Key: ecs|CreateCapacityProvider|InvalidParameterException
Issue-Key: ecs.daemon-deployments-lifecycle-progresses|_create_capacity_provider#2|AssertionError|raise AssertionError(f"unreachable: could not create Managed Instances capacity provider: {last_exc}")
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ecs: CreateCapacityProvider refuses a valid Managed Instances provider ("monitoring should be one of [BASIC,DETAILED]"), so no daemon/Managed-Instances scenario can be provisioned (9 checks)

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
REOPENS 20261024T015148Z-ecs-createcapacityprovider-is-an-unimplemented-stub-and-createservice.md: its fix left the call refused. That issue's Cause 1 implemented CreateCapacityProvider (it previously answered NotImplemented); the implemented handler now refuses a well-formed managedInstancesProvider request with HTTP 400 InvalidParameterException, so the operation is still unusable for Managed Instances. The open issue 20261104T062603Z-ecs-daemon-scheduling-never-progresses-and-createcapacityprovider-rejects.md describes this same refusal but carries only the replay key of ecs.list-daemons-reflects-create-and-delete; the nine checks below now fail on it with a different replay key (an AssertionError raised at the CreateCapacityProvider call site instead of the earlier poll timeout), so none of them is owned by it.

Cause 1 of 1: CreateCapacityProvider rejects instanceLaunchTemplate.monitoring.
  AWS: CreateCapacityProvider accepts a managedInstancesProvider and returns HTTP 200 with the created capacityProvider. Smithy: CreateCapacityProvider input {name (required), cluster, autoScalingGroupProvider, managedInstancesProvider: CreateManagedInstancesProviderConfiguration, tags}; CreateManagedInstancesProviderConfiguration requires infrastructureRoleArn + instanceLaunchTemplate (InstanceLaunchTemplate), whose monitoring member targets the enum ManagedInstancesMonitoringOptions with values [BASIC, DETAILED] and is OPTIONAL. Recorded behavior: AWS accepts the request the checks send — instanceLaunchTemplate = {ec2InstanceProfileArn, networkConfiguration {subnets, securityGroups}, monitoring: "basic"} — i.e. the monitoring value is accepted in lower case, and the provider is created. Where a value really is outside the modeled enum, AWS's refusal is the modeled InvalidParameterException (httpError 400) whose message names the offending member and the values the model allows; it is not the emulator's own sentence.
  Emulator: HTTP 400, error code `InvalidParameterException`, message `monitoring should be one of [BASIC,DETAILED]` — a case-sensitive enum comparison that rejects the accepted value. The message is emulator-authored prose: the whole sentence is fixed text with no value from the request in it (no member path, no offending value), so even for a genuinely invalid value it does not match AWS's wording.
  Why: a client cannot create an ECS Managed Instances capacity provider at all. Every downstream behaviour that needs one — DAEMON/daemon rollouts, daemon deployments and revisions, cluster-scoped capacity-provider association, and the refusal of DeleteCluster while a provider is attached — is unreachable, and the client sees a validation error for a request AWS accepts.
  Fix: the managedInstancesProvider/instanceLaunchTemplate validation in the ECS CreateCapacityProvider handler (accept the modeled monitoring values as AWS does, case-insensitively, and treat an omitted monitoring as the default), plus the message template of the InvalidParameterException it raises for a value that really is invalid.
  Traces: ecs.concurrent-daemon-update, ecs.daemon-deployment-new-revision-ordering, ecs.daemon-deployments-lifecycle-progresses, ecs.daemon-managed-rollout, ecs.daemon-ops-on-missing-cluster-not-found, ecs.daemon-revisions-tracks-updates, ecs.delete-cluster-with-capacity-provider-rejected, ecs.describe-daemon-unknown-name-not-found, ecs.e2e-daemon-instance-join-schedules-task-consequence (all listed by the suite under `### ecs`).

## Evidence
Expected (AWS, recorded): CreateCapacityProvider with managedInstancesProvider={infrastructureRoleArn: <role arn>, instanceLaunchTemplate: {ec2InstanceProfileArn: <profile arn>, networkConfiguration: {subnets: [<subnet>], securityGroups: [<sg>]}, monitoring: "basic"}} returns HTTP 200 and a capacityProvider the checks then use.

Actual (emulator), identical text in all nine checks:
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the CreateCapacityProvider operation: monitoring should be one of [BASIC,DETAILED]
surfacing as, e.g.:
  ecs.e2e-daemon-instance-join-schedules-task-consequence — raised directly out of the client call (context.py line 123 wrapper).
  ecs.daemon-deployments-lifecycle-progresses — AssertionError: unreachable: could not create Managed Instances capacity provider: An error occurred (InvalidParameterException) ... monitoring should be one of [BASIC,DETAILED]
  ecs.concurrent-daemon-update — AssertionError: timed out waiting for 'IAM infra/instance roles propagate enough for CreateCapacityProvider' (last=InvalidParameterException('... monitoring should be one of [BASIC,DETAILED]')) — the check retried the call for its whole budget because the refusal looks like role propagation.
The sentence "monitoring should be one of [BASIC,DETAILED]" is entirely fixed emulator text; the role/profile/subnet/security-group ARNs and the provider name in each run are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create an IAM role for the ECS Managed Instances infrastructure role (AmazonECSInfrastructureRolePolicyForManagedInstances) and an EC2 instance profile with AmazonECSInstanceRolePolicyForManagedInstances.
2. ecs create-cluster --cluster-name <c>.
3. ecs create-capacity-provider --name <n> --managed-instances-provider '{"infrastructureRoleArn":"<infra role arn>","instanceLaunchTemplate":{"ec2InstanceProfileArn":"<profile arn>","networkConfiguration":{"subnets":["<subnet>"],"securityGroups":["<sg>"]},"monitoring":"basic"}}'
   -> emulator: 400 InvalidParameterException "monitoring should be one of [BASIC,DETAILED]"; AWS: 200 with the capacityProvider.
4. Repeat with monitoring omitted, and with "BASIC"/"DETAILED", to confirm the comparison is case-sensitive and the member is treated as required.
Replay the nine check ids in trace_ids.
